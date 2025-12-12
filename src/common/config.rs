use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::collections::HashMap;
use crate::actor::wm_controller::WmCommand;
use crate::sys::hotkey::{Hotkey, HotkeySpec};

const MAX_WORKSPACES: usize = 32;

// TODO: when to remove these?
const DEPRECATED_MAP: &[(&str, &str)] = &[
    ("stack_windows", "toggle_stack"),
    ("unstack_windows", "toggle_stack"),
    ("toggle_tile_orientation", "toggle_orientation"),
];

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ConfigCommand {
    SetAnimate(bool),
    SetAnimationDuration(f64),
    SetAnimationFps(f64),
    SetAnimationEasing(AnimationEasing),

    SetMouseFollowsFocus(bool),
    SetMouseHidesOnFocus(bool),
    SetFocusFollowsMouse(bool),

    SetStackOffset(f64),
    SetOuterGaps {
        top: f64,
        left: f64,
        bottom: f64,
        right: f64,
    },
    SetInnerGaps {
        horizontal: f64,
        vertical: f64,
    },

    SetWorkspaceNames(Vec<String>),

    /// Generic setter for arbitrary config paths using dot-separated keys.
    /// Example: key = "settings.animate", value = true
    Set {
        key: String,
        value: Value,
    },

    GetConfig,
    SaveConfig,
    ReloadConfig,
}

pub fn data_dir() -> PathBuf { dirs::home_dir().unwrap().join(".rift") }
pub fn restore_file() -> PathBuf { data_dir().join("layout.ron") }
pub fn config_file() -> PathBuf {
    dirs::home_dir().unwrap().join(".config").join("rift").join("config.toml")
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct VirtualWorkspaceSettings {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_workspace_count")]
    pub default_workspace_count: usize,
    #[serde(default = "yes")]
    pub auto_assign_windows: bool,
    #[serde(default = "yes")]
    pub preserve_focus_per_workspace: bool,
    #[serde(default = "no")]
    pub workspace_auto_back_and_forth: bool,
    #[serde(default = "default_workspace_names")]
    pub workspace_names: Vec<String>,
    #[serde(default)]
    pub default_workspace: usize,
    #[serde(default)]
    pub reapply_app_rules_on_title_change: bool,
    #[serde(default)]
    pub app_rules: Vec<AppWorkspaceRule>,
}

// Allow specifying a workspace by numeric index or by name in the config.
// This supports both `workspace = 2` and `workspace = "coding"` in app rules.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Eq)]
#[serde(untagged)]
pub enum WorkspaceSelector {
    Index(usize),
    Name(String),
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct AppWorkspaceRule {
    /// Application bundle identifier (e.g., "com.apple.Terminal")
    pub app_id: Option<String>,
    /// Target workspace index (0 based) OR workspace name. If None, window goes to active workspace.
    pub workspace: Option<WorkspaceSelector>,
    /// Whether windows should be floating in this workspace
    #[serde(default)]
    pub floating: bool,
    /// Whether Rift should manage matching windows (defaults to true). `false` makes the
    /// window invisible to Rift (no tiling, floating, or assignments).
    #[serde(default = "yes")]
    pub manage: bool,
    /// Optional: Application name pattern (alternative to app_id)
    pub app_name: Option<String>,
    /// Optional: Regular expression to match window title (applies to window.title)
    ///
    /// If present, this regex will be used when attempting to match a window by
    /// title.
    pub title_regex: Option<String>,
    /// Optional: Substring to search for in window title (applies to window.title)
    ///
    /// If present, rift will internally treat this as a substring match and will
    /// construct a regex to match titles containing this substring. This allows
    /// people who don't want to write full regexes to match by a simple substring.
    pub title_substring: Option<String>,

    /// Optional: Accessibility role to match (AXRole). If present, it must be a
    /// non-empty string and will be compared against the accessibility role
    /// reported by the AX APIs for a window (exact string match).
    pub ax_role: Option<String>,

    /// Optional: Accessibility subrole to match (AXSubrole). If present, it must be a
    /// non-empty string and will be compared against the accessibility subrole
    /// reported by the AX APIs for a window (exact string match).
    pub ax_subrole: Option<String>,
}

impl Default for VirtualWorkspaceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            default_workspace_count: default_workspace_count(),
            auto_assign_windows: true,
            preserve_focus_per_workspace: true,
            workspace_auto_back_and_forth: false,
            workspace_names: default_workspace_names(),
            default_workspace: 0,
            reapply_app_rules_on_title_change: false,
            app_rules: Vec::new(),
        }
    }
}

impl VirtualWorkspaceSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.default_workspace_count == 0 {
            issues.push("default_workspace_count must be at least 1".to_string());
        }
        if self.default_workspace_count > MAX_WORKSPACES {
            issues.push(format!(
                "default_workspace_count should not exceed {} for performance reasons",
                MAX_WORKSPACES
            ));
        }

        if self.workspace_names.len() > self.default_workspace_count {
            issues.push("More workspace names provided than default_workspace_count".to_string());
        }

        if self.default_workspace >= self.default_workspace_count {
            issues.push(format!(
                "default_workspace ({}) must be less than default_workspace_count ({})",
                self.default_workspace, self.default_workspace_count
            ));
        }

        // Validate rules and check duplicates in a single pass
        let mut seen_app_ids = crate::common::collections::HashSet::default();
        let mut seen_app_names = crate::common::collections::HashSet::default();
        let mut seen_title_regexes = crate::common::collections::HashSet::default();
        let mut seen_title_substrings = crate::common::collections::HashSet::default();
        let mut seen_ax_roles = crate::common::collections::HashSet::default();
        let mut seen_ax_subroles = crate::common::collections::HashSet::default();

        for (index, rule) in self.app_rules.iter().enumerate() {
            let app_id_empty = rule.app_id.as_ref().map_or(true, |id| id.is_empty());
            if app_id_empty
                && rule.app_name.is_none()
                && rule.title_regex.is_none()
                && rule.title_substring.is_none()
                && rule.ax_role.is_none()
                && rule.ax_subrole.is_none()
            {
                issues.push(format!(
                    "App rule {} has no app_id, app_name, title_regex, or title_substring specified",
                    index
                ));
            }

            if let Some(ref workspace) = rule.workspace {
                if let WorkspaceSelector::Index(idx) = workspace {
                    if *idx >= self.default_workspace_count {
                        issues.push(format!(
                            "App rule {} references workspace {} but only {} workspaces will be created",
                            index, idx, self.default_workspace_count
                        ));
                    }
                }
            }

            if let Some(ref app_id) = rule.app_id {
                if !app_id.is_empty() && !app_id.contains('.') {
                    issues.push(format!(
                        "App rule {} has suspicious app_id '{}' (should be bundle identifier like 'com.example.app')",
                        index, app_id
                    ));
                }

                let has_specific_match = rule.app_name.is_some()
                    || rule.title_regex.is_some()
                    || rule.title_substring.is_some()
                    || rule.ax_role.is_some()
                    || rule.ax_subrole.is_some();
                if !app_id.is_empty() && !has_specific_match && !seen_app_ids.insert(app_id) {
                    issues.push(format!("Duplicate app_id '{}' in rule {}", app_id, index));
                }
            }

            if let Some(ref app_name) = rule.app_name {
                if !seen_app_names.insert(app_name) {
                    issues.push(format!("Duplicate app_name '{}' in rule {}", app_name, index));
                }
            }

            if let Some(ref title_re) = rule.title_regex {
                if title_re.is_empty() {
                    issues.push(format!("App rule {} has empty title_regex", index));
                } else if !seen_title_regexes.insert(title_re) {
                    issues.push(format!("Duplicate title_regex '{}' in rule {}", title_re, index));
                }
            }

            if let Some(ref title_sub) = rule.title_substring {
                if title_sub.is_empty() {
                    issues.push(format!("App rule {} has empty title_substring", index));
                } else if !seen_title_substrings.insert(title_sub) {
                    issues.push(format!(
                        "Duplicate title_substring '{}' in rule {}",
                        title_sub, index
                    ));
                }
            }

            if let Some(ref ax_role) = rule.ax_role {
                if ax_role.is_empty() {
                    issues.push(format!("App rule {} has empty ax_role", index));
                } else if !seen_ax_roles.insert(ax_role) {
                    issues.push(format!("Duplicate ax_role '{}' in rule {}", ax_role, index));
                }
            }

            if let Some(ref ax_sub) = rule.ax_subrole {
                if ax_sub.is_empty() {
                    issues.push(format!("App rule {} has empty ax_subrole", index));
                } else if !seen_ax_subroles.insert(ax_sub) {
                    issues.push(format!("Duplicate ax_subrole '{}' in rule {}", ax_sub, index));
                }
            }
        }

        issues
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    settings: Settings,
    keys: HashMap<String, WmCommand>,
    #[serde(default)]
    virtual_workspaces: VirtualWorkspaceSettings,
    /// Modifier combinations that can be reused in key bindings
    /// e.g., "comb1" = "Alt + Shift" allows using "comb1 + C" in keys
    #[serde(default)]
    modifier_combinations: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub settings: Settings,
    pub keys: Vec<(Hotkey, WmCommand)>,
    pub virtual_workspaces: VirtualWorkspaceSettings,
}

unsafe impl Send for Config {}
unsafe impl Sync for Config {}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "yes")]
    pub animate: bool,
    #[serde(default = "default_animation_duration")]
    pub animation_duration: f64,
    #[serde(default = "default_animation_fps")]
    pub animation_fps: f64,
    #[serde(default)]
    pub animation_easing: AnimationEasing,
    #[serde(default = "yes")]
    pub default_disable: bool,
    #[serde(default = "yes")]
    pub mouse_follows_focus: bool,
    #[serde(default = "yes")]
    pub mouse_hides_on_focus: bool,
    #[serde(default = "yes")]
    pub focus_follows_mouse: bool,
    /// Hotkey that disables focus-follows-mouse while held.
    /// Accepts either a full hotkey (e.g. "Ctrl + A") or a modifier-only spec (e.g. "Ctrl")
    #[serde(default)]
    pub focus_follows_mouse_disable_hotkey: Option<HotkeySpec>,
    /// Apps that should not trigger automatic workspace switching when activated.
    /// List of bundle identifiers (e.g., "com.apple.Spotlight") that often
    /// inappropriately steal focus and shouldn't cause workspace switches.
    #[serde(default)]
    pub auto_focus_blacklist: Vec<String>,
    #[serde(default)]
    pub layout: LayoutSettings,
    #[serde(default)]
    pub ui: UiSettings,
    /// Trackpad gesture settings
    #[serde(default)]
    pub gestures: GestureSettings,

    #[serde(default)]
    pub window_snapping: WindowSnappingSettings,

    /// Commands to run on startup (e.g., for subscribing to events)
    #[serde(default)]
    pub run_on_start: Vec<String>,

    /// Whether to reapply app rules when a window title changes.
    /// Enable hot-reloading of the config file when it changes
    #[serde(default = "yes")]
    pub hot_reload: bool,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, Copy)]
#[serde(rename_all = "snake_case")]
pub enum AnimationEasing {
    #[default]
    EaseInOut,
    Linear,
    EaseInSine,
    EaseOutSine,
    EaseInOutSine,
    EaseInQuad,
    EaseOutQuad,
    EaseInOutQuad,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInQuart,
    EaseOutQuart,
    EaseInOutQuart,
    EaseInQuint,
    EaseOutQuint,
    EaseInOutQuint,
    EaseInExpo,
    EaseOutExpo,
    EaseInOutExpo,
    EaseInCirc,
    EaseOutCirc,
    EaseInOutCirc,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct UiSettings {
    #[serde(default)]
    pub menu_bar: MenuBarSettings,
    #[serde(default)]
    pub stack_line: StackLineSettings,
    #[serde(default)]
    pub mission_control: MissionControlSettings,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct GestureSettings {
    /// Enable horizontal swipes to switch virtual workspaces
    #[serde(default = "no")]
    pub enabled: bool,
    /// Invert horizontal direction (swap next/prev)
    #[serde(default)]
    pub invert_horizontal_swipe: bool,
    /// Maximum absolute Y delta allowed for the gesture to count as horizontal
    #[serde(default = "default_swipe_vertical_tolerance")]
    pub swipe_vertical_tolerance: f64,
    /// If true, attempt to skip empty workspaces on swipe (if supported)
    #[serde(default)]
    pub skip_empty: bool,
    /// Number of fingers required for swipe (default = 3)
    #[serde(default = "default_swipe_fingers")]
    pub fingers: usize,
    /// Normalized horizontal distance (0..1) required to fire a swipe
    #[serde(default = "default_distance_pct")]
    pub distance_pct: f64,
    /// Enable haptic feedback when a swipe commits
    #[serde(default = "yes")]
    pub haptics_enabled: bool,
    /// Haptic feedback pattern (generic | alignment | level_change)
    #[serde(default)]
    pub haptic_pattern: HapticPattern,
}

impl Default for GestureSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            invert_horizontal_swipe: false,
            swipe_vertical_tolerance: default_swipe_vertical_tolerance(),
            skip_empty: true,
            fingers: default_swipe_fingers(),
            distance_pct: default_distance_pct(),
            haptics_enabled: true,
            haptic_pattern: HapticPattern::LevelChange,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, Copy)]
#[serde(deny_unknown_fields)]
pub struct WindowSnappingSettings {
    #[serde(default = "default_drag_swap_fraction")]
    pub drag_swap_fraction: f64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum MenuBarDisplayMode {
    #[default]
    All,
    Active,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActiveWorkspaceLabel {
    #[default]
    Index,
    Name,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDisplayStyle {
    #[default]
    Layout,
    Label,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct MenuBarSettings {
    #[serde(default = "no")]
    pub enabled: bool,
    #[serde(default = "no")]
    pub show_empty: bool,
    #[serde(default)]
    pub mode: MenuBarDisplayMode,
    #[serde(default)]
    pub active_label: ActiveWorkspaceLabel,
    #[serde(default)]
    pub display_style: WorkspaceDisplayStyle,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct StackLineSettings {
    #[serde(default = "no")]
    pub enabled: bool,
    #[serde(default)]
    pub thickness: f64,
    #[serde(default)]
    pub horiz_placement: HorizontalPlacement,
    #[serde(default)]
    pub vert_placement: VerticalPlacement,
    /// Distance to position the stack line away from the window edge (in points)
    /// This creates spacing between the window and the stack line
    #[serde(default = "default_stack_line_spacing")]
    pub spacing: f64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct MissionControlSettings {
    #[serde(default = "no")]
    pub enabled: bool,
    #[serde(default = "no")]
    pub fade_enabled: bool,
    #[serde(default = "default_mission_control_fade_duration_ms")]
    pub fade_duration_ms: f64,
}

fn default_mission_control_fade_duration_ms() -> f64 { 180.0 }

fn default_drag_swap_fraction() -> f64 { 0.3 }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum HorizontalPlacement {
    #[default]
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerticalPlacement {
    #[default]
    Left,
    Right,
}

impl StackLineSettings {
    pub fn thickness(&self) -> f64 { if self.enabled { self.thickness } else { 0.0 } }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct LayoutSettings {
    /// Layout mode: "traditional" (i3/sway style containers)
    #[serde(default)]
    pub mode: LayoutMode,
    /// Stack system configuration
    #[serde(default)]
    pub stack: StackSettings,
    /// Gap configuration for window spacing
    #[serde(default)]
    pub gaps: GapSettings,
}

/// Layout mode enum
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    /// Traditional container-based tiling (i3/sway style)
    #[default]
    Traditional,
    /// Binary space partitioning tiling
    Bsp,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum StackDefaultOrientation {
    Perpendicular,
    Same,
    Horizontal,
    Vertical,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct StackSettings {
    /// Stack offset - how much each stacked window is offset (in pixels)
    /// With the enhanced stacking system, this creates meaningful visible edges
    /// for each window in the stack while the focused window remains fully visible.
    /// Recommended values: 30-50 pixels for good visibility.
    #[serde(default = "default_stack_offset")]
    pub stack_offset: f64,

    /// Default orientation behavior when stacking windows.
    /// Options:
    /// - "perpendicular" (default): choose the perpendicular orientation to the parent layout
    /// - "same": use the same orientation as the parent layout
    /// - "horizontal"/"vertical": explicitly use a specific orientation
    #[serde(default = "default_stack_orientation")]
    pub default_orientation: StackDefaultOrientation,
}

/// Gap configuration for window spacing
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct GapSettings {
    /// Outer gaps (space between windows and screen edges)
    #[serde(default)]
    pub outer: OuterGaps,
    /// Inner gaps (space between windows)
    #[serde(default)]
    pub inner: InnerGaps,
    /// Display-specific gap overrides keyed by display UUID
    #[serde(default)]
    pub per_display: HashMap<String, GapOverride>,
}

/// Outer gap configuration (space between windows and screen edges)
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct OuterGaps {
    /// Gap at the top of the screen
    #[serde(default)]
    pub top: f64,
    /// Gap at the left of the screen
    #[serde(default)]
    pub left: f64,
    /// Gap at the bottom of the screen
    #[serde(default)]
    pub bottom: f64,
    /// Gap at the right of the screen
    #[serde(default)]
    pub right: f64,
}

/// Inner gap configuration (space between windows)
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct InnerGaps {
    /// Horizontal gap between windows
    #[serde(default)]
    pub horizontal: f64,
    /// Vertical gap between windows
    #[serde(default)]
    pub vertical: f64,
}

/// Overrides for gaps on a per-display basis
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct GapOverride {
    /// Override outer gaps completely for the display
    #[serde(default)]
    pub outer: Option<OuterGaps>,
    /// Override inner gaps completely for the display
    #[serde(default)]
    pub inner: Option<InnerGaps>,
}

impl Default for StackSettings {
    fn default() -> Self {
        Self {
            stack_offset: default_stack_offset(),
            default_orientation: default_stack_orientation(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.animation_duration < 0.0 {
            issues.push(format!(
                "animation_duration must be non-negative, got {}",
                self.animation_duration
            ));
        }

        if self.animation_fps <= 0.0 {
            issues.push(format!(
                "animation_fps must be positive, got {}",
                self.animation_fps
            ));
        }

        issues.extend(self.layout.validate());

        if self.gestures.swipe_vertical_tolerance < 0.0 {
            issues.push(format!(
                "gestures.swipe_vertical_tolerance must be non-negative, got {}",
                self.gestures.swipe_vertical_tolerance
            ));
        }

        issues
    }
}

impl LayoutSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        issues.extend(self.stack.validate());

        issues.extend(self.gaps.validate());

        issues
    }
}

impl StackSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.stack_offset < 0.0 {
            issues.push(format!(
                "stack_offset must be non-negative, got {}",
                self.stack_offset
            ));
        }

        issues
    }
}

impl GapSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // Validate outer gaps
        issues.extend(self.outer.validate());

        // Validate inner gaps
        issues.extend(self.inner.validate());

        for (uuid, overrides) in &self.per_display {
            if let Some(outer) = &overrides.outer {
                for issue in outer.validate() {
                    issues.push(format!("per_display[{uuid}] {issue}"));
                }
            }
            if let Some(inner) = &overrides.inner {
                for issue in inner.validate() {
                    issues.push(format!("per_display[{uuid}] {issue}"));
                }
            }
        }

        issues
    }

    pub fn effective_for_display(&self, display_uuid: Option<&str>) -> GapSettings {
        let mut resolved = GapSettings {
            outer: self.outer.clone(),
            inner: self.inner.clone(),
            per_display: HashMap::default(),
        };
        if let Some(uuid) = display_uuid {
            if let Some(overrides) = self.per_display.get(uuid) {
                if let Some(outer_override) = &overrides.outer {
                    resolved.outer = outer_override.clone();
                }
                if let Some(inner_override) = &overrides.inner {
                    resolved.inner = inner_override.clone();
                }
            }
        }
        resolved
    }
}

impl OuterGaps {
    /// Validates outer gap configuration values and returns a list of issues found.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.top < 0.0 {
            issues.push(format!("outer.top gap must be non-negative, got {}", self.top));
        }

        if self.left < 0.0 {
            issues.push(format!("outer.left gap must be non-negative, got {}", self.left));
        }

        if self.bottom < 0.0 {
            issues.push(format!(
                "outer.bottom gap must be non-negative, got {}",
                self.bottom
            ));
        }

        if self.right < 0.0 {
            issues.push(format!(
                "outer.right gap must be non-negative, got {}",
                self.right
            ));
        }

        issues
    }
}

impl InnerGaps {
    /// Validates inner gap configuration values and returns a list of issues found.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.horizontal < 0.0 {
            issues.push(format!(
                "inner.horizontal gap must be non-negative, got {}",
                self.horizontal
            ));
        }

        if self.vertical < 0.0 {
            issues.push(format!(
                "inner.vertical gap must be non-negative, got {}",
                self.vertical
            ));
        }

        issues
    }
}

fn yes() -> bool { true }

fn default_stack_offset() -> f64 { 40.0 }

fn default_stack_orientation() -> StackDefaultOrientation { StackDefaultOrientation::Perpendicular }

fn default_animation_duration() -> f64 { 0.3 }

fn default_animation_fps() -> f64 { 100.0 }

#[allow(dead_code)]
fn no() -> bool { false }

fn default_workspace_count() -> usize { 4 }

fn default_workspace_names() -> Vec<String> {
    vec![
        "Main".to_string(),
        "Development".to_string(),
        "Communication".to_string(),
        "Utilities".to_string(),
    ]
}

// Interpreted as normalized fraction when <= 1.0. If > 1.0 and <= 100.0,
// it is treated as a percentage (e.g. 40.0 -> 0.40).
fn default_swipe_vertical_tolerance() -> f64 { 0.4 }
fn default_swipe_fingers() -> usize { 3 }
fn default_distance_pct() -> f64 { 0.08 }

fn default_stack_line_spacing() -> f64 { 0.0 }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum HapticPattern {
    Generic,
    Alignment,
    #[default]
    LevelChange,
}

impl Config {
    pub fn read(path: &Path) -> anyhow::Result<Config> {
        let buf = std::fs::read_to_string(path)?;
        Self::parse(&buf)
    }

    pub fn default() -> Config { Self::parse(include_str!("../../rift.default.toml")).unwrap() }

    /// Save the current config to a file
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let config_file = ConfigFile {
            settings: self.settings.clone(),
            keys: self
                .keys
                .iter()
                .map(|(hotkey, command)| {
                    let hotkey_str = format!("{:?}", hotkey);
                    (hotkey_str, command.clone())
                })
                .collect(),
            virtual_workspaces: self.virtual_workspaces.clone(),
            modifier_combinations: HashMap::default(),
        };

        let toml_string = toml::to_string_pretty(&config_file)?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, toml_string.as_bytes())?;

        Ok(())
    }

    /// Validates the entire configuration and returns a list of issues found.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // Validate settings
        issues.extend(self.settings.validate());

        // Validate virtual workspace settings
        issues.extend(self.virtual_workspaces.validate());

        issues
    }

    fn normalize_hotkey_string(key: &str) -> String {
        let mut out = String::with_capacity(key.len());
        let mut word = String::new();

        for ch in key.chars() {
            if ch.is_alphabetic() {
                word.push(ch);
            } else {
                if !word.is_empty() {
                    let token = if word.len() == 1 {
                        word.to_ascii_uppercase()
                    } else {
                        match word.to_lowercase().as_str() {
                            "up" => "ArrowUp".to_string(),
                            "down" => "ArrowDown".to_string(),
                            "left" => "ArrowLeft".to_string(),
                            "right" => "ArrowRight".to_string(),
                            _ => word.clone(),
                        }
                    };
                    out.push_str(&token);
                    word.clear();
                }
                out.push(ch);
            }
        }

        if !word.is_empty() {
            let token = if word.len() == 1 {
                word.to_ascii_uppercase()
            } else {
                match word.to_lowercase().as_str() {
                    "up" => "ArrowUp".to_string(),
                    "down" => "ArrowDown".to_string(),
                    "left" => "ArrowLeft".to_string(),
                    "right" => "ArrowRight".to_string(),
                    _ => word.clone(),
                }
            };
            out.push_str(&token);
        }

        out
    }

    fn expand_modifier_combinations(key: &str, combinations: &HashMap<String, String>) -> String {
        if let Some(plus_pos) = key.find(" + ") {
            let potential_combo = &key[..plus_pos];
            if let Some(combo_value) = combinations.get(potential_combo) {
                let rest = &key[plus_pos + 3..];
                return format!("{} + {}", combo_value, rest);
            }
        }
        key.to_string()
    }

    /// no need to pull in a dep for just this
    fn levenshtein(a: &str, b: &str) -> usize {
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let mut d = vec![vec![0usize; b_chars.len() + 1]; a_chars.len() + 1];
        for i in 0..=a_chars.len() {
            d[i][0] = i;
        }
        for j in 0..=b_chars.len() {
            d[0][j] = j;
        }
        for i in 1..=a_chars.len() {
            for j in 1..=b_chars.len() {
                let cost = if a_chars[i - 1] == b_chars[j - 1] {
                    0
                } else {
                    1
                };
                d[i][j] = std::cmp::min(
                    std::cmp::min(d[i - 1][j] + 1, d[i][j - 1] + 1),
                    d[i - 1][j - 1] + cost,
                );
            }
        }
        d[a_chars.len()][b_chars.len()]
    }

    // Extracts an "unknown variant `...`" token from serde error string when present.
    // Additionally, if serde's error message contains an "expected" list (backtick-delimited),
    // embed those expected tokens alongside the unknown token using the separator "||".
    // The resulting returned string may therefore be:
    //   - "unknown_token" (no expected candidates found)
    //   - "unknown_token||cand1,cand2,..." (candidates appended)
    fn extract_unknown_variant(err: &str) -> Option<String> {
        let needle = "unknown variant `";
        if let Some(start) = err.find(needle) {
            let rest = &err[start + needle.len()..];
            if let Some(end) = rest.find('`') {
                let unknown = rest[..end].to_string();

                // Collect all backtick-enclosed tokens in the error message and
                // treat them as candidate variants (excluding the unknown itself).
                let mut variants: Vec<String> = Vec::new();
                let mut i = 0usize;
                while let Some(open) = err[i..].find('`') {
                    let open_abs = i + open + 1;
                    if let Some(close_off) = err[open_abs..].find('`') {
                        let close_abs = open_abs + close_off;
                        let token = &err[open_abs..close_abs];
                        if token != unknown {
                            variants.push(token.to_string());
                        }
                        i = close_abs + 1;
                    } else {
                        break;
                    }
                }

                if !variants.is_empty() {
                    // dedupe while preserving order
                    let mut seen = std::collections::HashSet::new();
                    let mut deduped = Vec::new();
                    for v in variants {
                        if seen.insert(v.clone()) {
                            deduped.push(v);
                        }
                    }
                    return Some(format!("{}||{}", unknown, deduped.join(",")));
                }

                return Some(unknown);
            }
        }

        if let Some(unknown_pos) = err.find("unknown") {
            if let Some(backtick_pos) = err[unknown_pos..].find('`') {
                let rest = &err[unknown_pos + backtick_pos + 1..];
                if let Some(end) = rest.find('`') {
                    return Some(rest[..end].to_string());
                }
            }
        }
        None
    }

    // Provide suggestion by comparing the unknown token to a list of known commands.
    // If the `unknown` string was produced by `extract_unknown_variant` and contains
    // an embedded serde candidate list (format: "token||cand1,cand2"), prefer those
    // candidates when computing the best suggestion. Otherwise fall back to the
    // conservative builtin list.
    //
    // Returns the best candidate if its distance is within a reasonable threshold.
    fn suggest_similar_command(unknown: &str) -> Option<(String, Option<String>)> {
        // Detect if `unknown` was augmented with serde-provided expected variants.
        let (unknown_token, serde_candidates): (String, Option<Vec<String>>) =
            if let Some(idx) = unknown.find("||") {
                let (u, rest) = unknown.split_at(idx);
                let rest = &rest[2..];
                let candidates: Vec<String> = rest
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (u.to_lowercase(), Some(candidates))
            } else {
                (unknown.to_lowercase(), None)
            };

        // Choose candidate set: prefer serde-provided ones when available.
        let mut best: Option<(String, usize)> = None;

        if let Some(cands) = serde_candidates {
            for cand in cands.iter() {
                let cand_norm = cand.to_lowercase();
                let dist = Self::levenshtein(&unknown_token, &cand_norm);
                if best.is_none() || dist < best.as_ref().unwrap().1 {
                    best = Some((cand.clone(), dist));
                }
            }
        } else {
            // Use dynamically generated builtin candidates.
            let builtin_candidates = crate::actor::wm_controller::WmCommand::builtin_candidates();
            for cand in builtin_candidates.iter() {
                let dist = Self::levenshtein(&unknown_token, &cand.to_lowercase());
                if best.is_none() || dist < best.as_ref().unwrap().1 {
                    best = Some((cand.to_string(), dist));
                }
            }
        }

        if let Some((best_cand, dist)) = best {
            // Heuristic threshold: allow suggestions if distance is <= half the length (or <=3).
            let threshold = std::cmp::max(3usize, best_cand.len() / 2);
            if dist <= threshold {
                // If the best candidate is in deprecated map, return the non-deprecated suggestion.
                let mut replacement = None;
                for &(dep, repl) in DEPRECATED_MAP.iter() {
                    if dep == best_cand {
                        replacement = Some(repl.to_string());
                        break;
                    }
                }
                return Some((best_cand.to_string(), replacement));
            }
        }

        // Also check if the unknown token itself matched a deprecated name exactly
        for &(dep, repl) in DEPRECATED_MAP.iter() {
            if dep == unknown_token {
                return Some((repl.to_string(), None)); // recommend replacement
            }
        }

        None
    }

    fn parse(buf: &str) -> anyhow::Result<Config> {
        // Attempt to deserialize. If it fails, and the error indicates an unknown enum
        // variant, attempt to provide a helpful suggestion.
        match toml::from_str::<ConfigFile>(&buf) {
            Ok(c) => {
                let mut keys = Vec::new();
                for (key, cmd) in c.keys {
                    let expanded_key =
                        Self::expand_modifier_combinations(&key, &c.modifier_combinations);
                    let normalized_key = Self::normalize_hotkey_string(&expanded_key);
                    let Ok(hotkey) = Hotkey::from_str(&normalized_key) else {
                        bail!("Could not parse hotkey: {key}");
                    };
                    keys.push((hotkey, cmd));
                }
                Ok(Config {
                    settings: c.settings,
                    keys,
                    virtual_workspaces: c.virtual_workspaces,
                })
            }
            Err(e) => {
                let msg = e.to_string();
                if let Some(unknown_token) = Self::extract_unknown_variant(&msg) {
                    if let Some((suggestion, deprecated_replacement)) =
                        Self::suggest_similar_command(&unknown_token)
                    {
                        if let Some(repl) = deprecated_replacement {
                            bail!(
                                "{msg}\nDid you mean `{}`? Note: `{}` is deprecated; use `{}` instead.",
                                suggestion,
                                suggestion,
                                repl
                            );
                        } else {
                            bail!("{msg}\nDid you mean `{}`?", suggestion);
                        }
                    } else {
                        bail!("{msg}");
                    }
                } else {
                    bail!("{msg}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_hotkey_string() {
        assert_eq!(
            Config::normalize_hotkey_string("Alt + Shift + Down"),
            "Alt + Shift + ArrowDown"
        );
        assert_eq!(Config::normalize_hotkey_string("Ctrl + Up"), "Ctrl + ArrowUp");
        assert_eq!(
            Config::normalize_hotkey_string("Shift + Left"),
            "Shift + ArrowLeft"
        );
        assert_eq!(
            Config::normalize_hotkey_string("Meta + Right"),
            "Meta + ArrowRight"
        );
    }

    #[test]
    fn test_modifier_combinations_in_config() {
        let toml = r#"
            [settings]
            animate = false

            [modifier_combinations]
            comb1 = "Alt + Shift"
            leader = "Ctrl + Alt"

            [keys]
            "comb1 + C" = "toggle_space_activated"
            "leader + Tab" = "next_workspace"
            "Alt + H" = { move_focus = "left" }
        "#;

        let cfg = Config::parse(toml).unwrap();
        // We expect keys to be parsed into hotkeys
        assert!(!cfg.keys.is_empty());
    }

    #[test]
    fn test_levenshtein_suggests() {
        let err =
            "unknown variant `toggle_stak`, expected one of `toggle_stack`, `toggle_orientation`";
        let token = Config::extract_unknown_variant(err).unwrap();
        assert_eq!(token, "toggle_stak||toggle_stack,toggle_orientation");
        let suggestion = Config::suggest_similar_command(&token);
        assert!(suggestion.is_some());
        let (s, _maybe_dep) = suggestion.unwrap();
        assert_eq!(s, "toggle_stack");
    }
}
