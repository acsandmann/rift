use std::collections::BTreeMap;
use std::path::PathBuf;

use regex::RegexBuilder;
pub use rift_protocol::{AnimationEasing, ConfigCommand, LayoutMode, WorkspaceSelector};
use serde::{Deserialize, Serialize};

use super::{ConfigEnum, ConfigSchema};
use crate::actor::wm_controller::WmCommand;
use crate::common::collections::HashMap;
use crate::sys::hotkey::{Hotkey, HotkeySpec};

pub const MAX_WORKSPACES: usize = 128;

// TODO: when to remove these?
pub(super) const DEPRECATED_MAP: &[(&str, &str)] = &[
    ("stack_windows", "toggle_stack"),
    ("unstack_windows", "toggle_stack"),
    ("toggle_tile_orientation", "toggle_orientation"),
];

pub fn data_dir() -> PathBuf { dirs::home_dir().unwrap().join(".rift") }
pub fn restore_file() -> PathBuf { data_dir().join("layout.ron") }
pub fn config_file() -> PathBuf {
    dirs::home_dir().unwrap().join(".config").join("rift").join("config.toml")
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct VirtualWorkspaceSettings {
    #[serde(default = "yes")]
    /// Organize windows into separate virtual workspaces.
    #[setting(ignore)]
    pub enabled: bool,
    #[serde(default = "default_workspace_count")]
    /// Number of virtual workspaces.
    #[setting(
        label = "Workspace count",
        custom,
        aliases = "workspace count number desktops spaces"
    )]
    pub default_workspace_count: usize,
    #[serde(default = "yes")]
    /// Automatically assign new windows to a workspace.
    // #[setting(
    //     label = "Auto-assign windows",
    //     aliases = "automatically assign apps workspaces"
    // )]
    #[setting(ignore)]
    pub auto_assign_windows: bool,
    #[serde(default = "yes")]
    /// Remember the focused window in each workspace.
    #[setting(label = "Preserve workspace focus", aliases = "remember active focus")]
    pub preserve_focus_per_workspace: bool,
    #[serde(default)]
    /// Return to the previous workspace when the active workspace is selected again.
    #[setting(
        label = "Switch back when selecting active workspace",
        aliases = "back and forth previous desktop"
    )]
    pub workspace_auto_back_and_forth: bool,
    #[serde(default, alias = "prevent_wrapping_around")]
    /// Stop workspace navigation at the first and last workspace.
    #[setting(label = "Prevent wrapping", aliases = "cycle last first workspace")]
    pub prevent_wrapping: bool,
    #[serde(default = "default_workspace_names")]
    /// Names assigned to virtual workspaces.
    #[setting(
        label = "Workspaces",
        custom,
        aliases = "names rename layout desktop space count number add remove"
    )]
    pub workspace_names: Vec<String>,
    #[serde(default)]
    /// Workspace used for new windows without another assignment.
    #[setting(label = "Default workspace", custom, aliases = "startup desktop space")]
    pub default_workspace: usize,
    #[serde(default)]
    /// Recheck workspace rules when a window title changes.
    #[setting(
        label = "Reapply rules when titles change",
        aliases = "application title updated match workspace"
    )]
    pub reapply_app_rules_on_title_change: bool,
    #[serde(default)]
    /// Assign matching applications and windows to workspaces.
    #[setting(label = "App rules", custom)]
    pub app_rules: Vec<AppWorkspaceRule>,
    #[serde(default)]
    /// Choose layouts for individual workspaces.
    #[setting(label = "Workspace rules", custom)]
    pub workspace_rules: Vec<WorkspaceLayoutRule>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLayoutRule {
    pub workspace: WorkspaceSelector,
    pub layout: LayoutMode,
}

// Allow specifying a workspace by numeric index or by name in the config.
// This supports both `workspace = 2` and `workspace = "coding"` in app rules.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct AppWorkspaceRule {
    /// Application bundle identifier (e.g., "com.apple.Terminal")
    #[setting(custom)]
    pub app_id: Option<String>,
    /// Target workspace index (0 based) OR workspace name. If None, window goes to active workspace.
    #[setting(custom)]
    pub workspace: Option<WorkspaceSelector>,
    #[serde(default)]
    #[setting(custom)]
    pub floating: bool,
    /// Initial normalized position for a floating window. `(0, 0)` is the top-left
    /// and `(1, 1)` is the bottom-right of the available screen area.
    #[setting(custom)]
    pub position: Option<AppRulePosition>,
    /// Preferred window size in logical pixels.
    #[setting(custom)]
    pub size: Option<AppRuleSize>,
    /// Focus the window after applying this rule, switching virtual workspaces if needed.
    #[serde(default)]
    #[setting(custom)]
    pub focus: bool,
    /// An explicit management override. `false` makes the window invisible to Rift;
    /// `true` overrides normal manageability heuristics for a visible window. When
    /// omitted, the matching rule leaves Rift's normal manageability decision intact.
    #[serde(default)]
    #[setting(custom)]
    pub manage: Option<bool>,
    #[setting(custom)]
    pub app_name: Option<String>,
    /// Optional: Regular expression to match window title (applies to window.title)
    ///
    /// If present, this regex will be used when attempting to match a window by
    /// title.
    #[setting(custom)]
    pub title_regex: Option<String>,
    /// Optional: Substring to search for in window title (applies to window.title)
    ///
    /// If present, rift will internally treat this as a substring match and will
    /// construct a regex to match titles containing this substring. This allows
    /// people who don't want to write full regexes to match by a simple substring.
    #[setting(custom)]
    pub title_substring: Option<String>,

    /// Optional: Accessibility role to match (AXRole). If present, it must be a
    /// non-empty string and will be compared against the accessibility role
    /// reported by the AX APIs for a window (exact string match).
    #[setting(custom)]
    pub ax_role: Option<String>,

    /// Accessibility subrole must be non-empty and is compared against the subrole
    /// reported by the AX APIs for a window (exact string match).
    #[setting(custom)]
    pub ax_subrole: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct AppRulePosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct AppRuleSize {
    pub w: Option<f64>,
    pub h: Option<f64>,
}

impl Default for VirtualWorkspaceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            default_workspace_count: default_workspace_count(),
            auto_assign_windows: true,
            preserve_focus_per_workspace: true,
            workspace_auto_back_and_forth: false,
            prevent_wrapping: false,
            workspace_names: default_workspace_names(),
            default_workspace: 0,
            reapply_app_rules_on_title_change: false,
            app_rules: Vec::new(),
            workspace_rules: Vec::new(),
        }
    }
}

impl VirtualWorkspaceSettings {
    pub fn resize(&mut self, count: usize) {
        self.default_workspace_count = count;
        // Invalid counts are left for the shared validator, without large allocations.
        if !(1..=MAX_WORKSPACES).contains(&count) {
            return;
        }
        let removed = self.workspace_names.iter().skip(count).cloned().collect::<Vec<_>>();
        self.workspace_names.truncate(count);
        self.default_workspace = self.default_workspace.min(count - 1);
        let removed_selector = |v: &WorkspaceSelector| match v {
            WorkspaceSelector::Index(i) => *i >= count,
            WorkspaceSelector::Name(n) => removed.contains(n),
        };
        self.workspace_rules.retain(|r| !removed_selector(&r.workspace));
        for r in &mut self.app_rules {
            if r.workspace.as_ref().is_some_and(removed_selector) {
                r.workspace = None;
            }
        }
    }

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

            if let Some(position) = rule.position {
                if !position.x.is_finite()
                    || !position.y.is_finite()
                    || !(0.0..=1.0).contains(&position.x)
                    || !(0.0..=1.0).contains(&position.y)
                {
                    issues.push(format!(
                        "App rule {} position x and y must be finite values between 0 and 1",
                        index
                    ));
                }
                if !rule.floating {
                    issues.push(format!(
                        "App rule {} specifies position, but position only applies when floating = true",
                        index
                    ));
                }
            }

            if let Some(size) = rule.size {
                if size.w.is_none() && size.h.is_none() {
                    issues.push(format!(
                        "App rule {} size must specify at least one of w or h",
                        index
                    ));
                }
                if size.w.is_some_and(|value| !value.is_finite() || value <= 0.0)
                    || size.h.is_some_and(|value| !value.is_finite() || value <= 0.0)
                {
                    issues.push(format!(
                        "App rule {} size dimensions must be finite positive values",
                        index
                    ));
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
                let has_specific_match = rule.app_id.is_some()
                    || rule.title_regex.is_some()
                    || rule.title_substring.is_some()
                    || rule.ax_role.is_some()
                    || rule.ax_subrole.is_some();
                if !has_specific_match && !seen_app_names.insert(app_name) {
                    issues.push(format!("Duplicate app_name '{}' in rule {}", app_name, index));
                }
            }

            if let Some(ref title_re) = rule.title_regex {
                if title_re.is_empty() {
                    issues.push(format!("App rule {} has empty title_regex", index));
                } else if let Err(error) =
                    RegexBuilder::new(title_re).case_insensitive(true).build()
                {
                    issues.push(format!(
                        "App rule {} has invalid title_regex '{}': {}",
                        index, title_re, error
                    ));
                } else if !seen_title_regexes.insert(title_re) {
                    issues.push(format!("Duplicate title_regex '{}' in rule {}", title_re, index));
                }
            }

            if rule.manage == Some(false)
                && (rule.workspace.is_some()
                    || rule.floating
                    || rule.position.is_some()
                    || rule.size.is_some()
                    || rule.focus)
            {
                issues.push(format!(
                    "App rule {} sets manage = false, so its workspace, floating, position, size, and focus effects are ignored",
                    index
                ));
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

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigSource {
    pub settings: Settings,
    pub keys: std::collections::BTreeMap<String, WmCommand>,
    #[serde(default)]
    pub binding_modes:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, WmCommand>>,
    #[serde(default)]
    pub virtual_workspaces: VirtualWorkspaceSettings,
    /// Modifier combinations that can be reused in key bindings
    /// e.g., "comb1" = "Alt + Shift" allows using "comb1 + C" in keys
    #[serde(default)]
    pub modifier_combinations: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Config {
    pub settings: Settings,
    pub keys: Vec<(Hotkey, WmCommand)>,
    #[serde(default)]
    pub binding_mode_specs: BindingModeSpecs,
    pub virtual_workspaces: VirtualWorkspaceSettings,
}

pub type BindingModeSpecs = Vec<(String, Vec<(String, WmCommand)>)>;

unsafe impl Send for Config {}
unsafe impl Sync for Config {}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
#[setting(group = "general")]
pub struct Settings {
    #[serde(default)]
    /// Animate windows when their layout changes.
    #[setting(
        label = "Animate window changes",
        group = "general",
        aliases = "animation transitions motion"
    )]
    pub animate: bool,
    #[serde(default = "default_animation_duration")]
    /// Duration of a window animation, in seconds.
    // #[setting(
    //     label = "Duration (seconds)",
    //     group = "general",
    //     enabled_by = "animate",
    //     aliases = "animation speed seconds"
    // )]
    #[setting(ignore)]
    pub animation_duration: f64,
    #[serde(default = "default_animation_fps")]
    /// Maximum number of frames per second during a window animation.
    // #[setting(
    //     label = "Frame rate",
    //     group = "general",
    //     enabled_by = "animate",
    //     aliases = "animation fps performance"
    // )]
    #[setting(ignore)]
    pub animation_fps: f64,
    #[serde(default)]
    /// The timing curve used for window animations.
    // #[setting(
    //     label = "Animation easing",
    //     group = "general",
    //     choices,
    //     enabled_by = "animate",
    //     aliases = "transition curve"
    // )]
    #[setting(ignore)]
    pub animation_easing: AnimationEasing,
    #[serde(default = "yes")]
    /// Start Rift with automatic tiling turned off.
    #[setting(
        label = "Start with tiling disabled",
        group = "general",
        aliases = "floating startup automatic tiling"
    )]
    pub default_disable: bool,
    #[serde(default = "yes")]
    /// Move the pointer into the window when focus changes.
    #[setting(
        label = "Move pointer to focused window",
        group = "pointer",
        aliases = "mouse follows focus cursor warp"
    )]
    pub mouse_follows_focus: bool,
    #[serde(default = "yes")]
    /// Hide the pointer after moving it into the focused window.
    #[setting(
        label = "Hide pointer after focusing",
        group = "pointer",
        enabled_by = "mouse_follows_focus",
        aliases = "hide mouse cursor"
    )]
    pub mouse_hides_on_focus: bool,
    #[serde(default = "yes")]
    /// Focus a window when the pointer moves over it.
    #[setting(
        label = "Focus follows pointer",
        group = "pointer",
        order = 0,
        aliases = "ffm focus follows mouse hover autofocus"
    )]
    pub focus_follows_mouse: bool,
    /// Treat vertically stacked displays as a horizontal pointer chain.
    /// `top-to-bottom` maps higher displays to the left; `bottom-to-top` reverses it.
    #[serde(default)]
    #[setting(ignore)]
    pub horizontal_mouse_warp: Option<HorizontalMouseWarp>,
    /// Hotkey that disables focus-follows-mouse while held.
    /// Accepts either a full hotkey (e.g. "Ctrl + A") or a modifier-only spec (e.g. "Ctrl")
    #[serde(default)]
    #[setting(ignore)]
    pub focus_follows_mouse_disable_hotkey: Option<HotkeySpec>,
    /// Apps that should not trigger automatic workspace switching when activated.
    /// List of bundle identifiers (e.g., "com.apple.Spotlight") that often
    /// inappropriately steal focus and shouldn't cause workspace switches.
    #[serde(default)]
    #[setting(
        label = "Autofocus blacklist",
        group = "advanced",
        custom,
        aliases = "focus exclude ignore app"
    )]
    pub auto_focus_blacklist: Vec<String>,
    #[serde(default)]
    #[setting(ignore)]
    pub layout: LayoutSettings,
    #[serde(default)]
    #[setting(ignore)]
    pub ui: UiSettings,
    /// Trackpad gesture settings
    #[serde(default)]
    #[setting(ignore)]
    pub gestures: GestureSettings,
    /// Modifier-assisted dragging and tiled-window drop settings.
    #[serde(default)]
    #[setting(ignore)]
    pub drag_drop: DragDropSettings,

    /// Commands to run on startup (e.g., for subscribing to events)
    #[serde(default)]
    #[setting(
        label = "Startup commands",
        group = "advanced",
        custom,
        aliases = "launch run shell exec"
    )]
    pub run_on_start: Vec<String>,

    /// Enable hot-reloading of the config file when it changes
    #[serde(default = "yes")]
    /// Reload the configuration automatically when the file changes.
    #[setting(
        label = "Reload config when edited externally",
        group = "advanced",
        aliases = "configuration file automatic reload toml"
    )]
    pub hot_reload: bool,
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum HorizontalMouseWarp {
    TopToBottom,
    BottomToTop,
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
#[setting(group = "main")]
pub struct GestureSettings {
    /// Enable horizontal swipes to switch virtual workspaces
    #[serde(default)]
    #[setting(label = "Enabled", group = "main")]
    pub enabled: bool,
    /// If true, consume horizontal swipe events owned by Rift so macOS and the
    /// foreground app do not also handle them.
    #[serde(default = "yes")]
    #[setting(label = "Consume macOS workspace swipe", group = "main")]
    pub consume_dock_swipe: bool,
    /// Invert horizontal direction (swap next/prev)
    #[serde(default)]
    #[setting(label = "Invert direction", group = "main")]
    pub invert_horizontal_swipe: bool,
    /// Maximum absolute Y delta allowed for the gesture to count as horizontal
    #[serde(default = "default_swipe_vertical_tolerance")]
    #[setting(label = "Vertical tolerance", group = "advanced")]
    pub swipe_vertical_tolerance: f64,
    /// If true, attempt to skip empty workspaces on swipe (if supported)
    #[serde(default)]
    #[setting(label = "Skip empty workspaces", group = "main")]
    pub skip_empty: bool,
    /// Number of fingers required for swipe (default = 3)
    #[serde(default = "default_swipe_fingers")]
    #[setting(label = "Fingers", group = "main")]
    pub fingers: usize,
    /// Normalized horizontal distance (0..1) required to fire a swipe
    #[serde(default = "default_distance_pct")]
    #[setting(label = "Distance (%)", group = "advanced", scale = 100.0)]
    pub distance_pct: f64,
    /// Enable haptic feedback when a swipe commits
    #[serde(default = "yes")]
    #[setting(label = "Haptic feedback", group = "main")]
    pub haptics_enabled: bool,
    /// Haptic feedback pattern (generic | alignment | level_change)
    #[serde(default)]
    #[setting(label = "Haptic pattern", group = "advanced", choices)]
    pub haptic_pattern: HapticPattern,
}

impl Default for GestureSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            consume_dock_swipe: true,
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

///
/// Serialized values are `cmd`, `alt`, `shift`, `ctrl`, and `fn`.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, Eq, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MouseModifier {
    /// Command (⌘). The alias `command` is also accepted.
    #[serde(alias = "command")]
    #[setting(label = "⌘ Command")]
    Cmd,
    /// Option (⌥). The alias `option` is also accepted.
    #[serde(alias = "option")]
    #[setting(label = "⌥ Option")]
    Alt,
    #[setting(label = "⇧ Shift")]
    Shift,
    #[serde(alias = "control")]
    #[setting(label = "⌃ Control")]
    Ctrl,
    /// Globe/Fn. This is the default because it rarely conflicts with apps.
    #[default]
    #[setting(label = "Fn")]
    Fn,
}

/// Operation reserved for a modifier-plus-mouse-button gesture.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, Eq, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    /// Do not capture this button; the click is delivered normally.
    #[setting(label = "Pass through")]
    None,
    /// Move a window from anywhere inside it.
    #[default]
    #[setting(label = "Move window")]
    Move,
}

/// Action used when a tiled window is released in another tile's center zone.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, Eq, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MouseDropAction {
    /// Exchange the source and target's logical layout positions.
    #[default]
    Swap,
    /// Move the source into the target's stack/group and select the source.
    Stack,
}

/// Modifier mouse actions and native tiled-window drop settings.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct DragDropSettings {
    /// Enable trackpad feedback after a successful window drop.
    #[serde(default = "yes")]
    #[setting(label = "Haptic feedback", order = 7)]
    pub haptics_enabled: bool,
    /// Enables native drag targeting and modifier mouse actions.
    #[serde(default = "yes")]
    #[setting(order = 0)]
    pub enabled: bool,
    /// Modifier held with `action1` or `action2`.
    #[serde(default)]
    #[setting(choices, order = 1)]
    pub modifier: MouseModifier,
    /// Left-button action while the configured modifier is held.
    #[serde(default)]
    #[setting(label = "Primary button", choices, order = 2)]
    pub action1: MouseAction,
    /// Right-button action while the configured modifier is held.
    #[serde(default = "default_mouse_action_none")]
    #[setting(label = "Secondary button", choices, order = 3)]
    pub action2: MouseAction,
    /// Center-zone action for tiled move drops.
    #[serde(default)]
    #[setting(label = "Drop in center", choices, order = 4)]
    pub drop_action: MouseDropAction,
    /// Depth of each edge zone as a fraction of the destination window's size.
    /// Valid values are `0.10..=0.45`; the default is `0.25`.
    #[serde(default = "default_drop_zone_fraction")]
    #[setting(label = "Center drop zone (%)", scale = 100.0, order = 5)]
    pub drop_zone_fraction: f64,
    /// Shows a translucent, rounded WindowServer overlay for the pending drop.
    /// The overlay is updated only when the target tile or drop zone changes.
    #[serde(default = "yes")]
    #[setting(label = "Show drop preview", order = 6)]
    pub preview: bool,
}

impl Default for DragDropSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            haptics_enabled: true,
            modifier: MouseModifier::Fn,
            action1: MouseAction::Move,
            action2: MouseAction::None,
            drop_action: MouseDropAction::Swap,
            drop_zone_fraction: default_drop_zone_fraction(),
            preview: true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MenuBarDisplayMode {
    #[default]
    All,
    Active,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum ActiveWorkspaceLabel {
    #[default]
    Index,
    Name,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDisplayStyle {
    #[default]
    Layout,
    Label,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct MenuBarSettings {
    #[serde(default = "yes")]
    /// Show Rift and workspace information in the menu bar.
    #[setting(label = "Show menu bar indicator")]
    pub enabled: bool,
    #[serde(default)]
    /// Include workspaces without windows in the menu bar.
    #[setting(label = "Show empty workspaces", enabled_by = "enabled")]
    pub show_empty: bool,
    #[serde(default)]
    /// Choose whether to show all workspaces or only the active one.
    #[setting(label = "Workspaces to show", choices, enabled_by = "enabled")]
    pub mode: MenuBarDisplayMode,
    #[serde(default)]
    /// Choose the label shown for the active workspace.
    #[setting(label = "Active workspace label", choices, enabled_by = "enabled")]
    pub active_label: ActiveWorkspaceLabel,
    #[serde(default)]
    /// Show workspace labels or layout indicators.
    #[setting(label = "Display style", choices, enabled_by = "enabled")]
    pub display_style: WorkspaceDisplayStyle,
    #[serde(default = "default_layout_folder")]
    /// Folder containing custom layout indicator images.
    #[setting(label = "Layout folder", custom, enabled_by = "enabled")]
    pub layout_folder: PathBuf,
}

impl MenuBarSettings {
    pub fn resolved_layout_folder(&self) -> PathBuf {
        let Ok(relative) = self.layout_folder.strip_prefix("~") else {
            return self.layout_folder.clone();
        };
        dirs::home_dir()
            .map(|home| home.join(relative))
            .unwrap_or_else(|| self.layout_folder.clone())
    }
}

impl Default for MenuBarSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            show_empty: false,
            mode: MenuBarDisplayMode::default(),
            active_label: ActiveWorkspaceLabel::default(),
            display_style: WorkspaceDisplayStyle::default(),
            layout_folder: default_layout_folder(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct StackLineSettings {
    #[serde(default)]
    /// Show the experimental stack indicator.
    #[setting(label = "Enabled")]
    pub enabled: bool,
    #[serde(default)]
    /// Choose whether hovering or clicking selects a window.
    #[setting(label = "Interaction", choices, enabled_by = "enabled")]
    pub hover: StackLineHoverMode,
    #[serde(default = "default_stack_line_thickness")]
    /// Thickness of the stack indicator, in points.
    #[setting(label = "Thickness (points)", enabled_by = "enabled")]
    pub thickness: f64,
    #[serde(default)]
    /// Where to place a horizontal stack indicator.
    #[setting(label = "Horizontal placement", choices, enabled_by = "enabled")]
    pub horiz_placement: HorizontalPlacement,
    #[serde(default)]
    /// Where to place a vertical stack indicator.
    #[setting(label = "Vertical placement", choices, enabled_by = "enabled")]
    pub vert_placement: VerticalPlacement,
    #[serde(default = "default_stack_line_spacing")]
    /// Space between stack segments, in points.
    #[setting(label = "Spacing (points)", enabled_by = "enabled")]
    pub spacing: f64,
    /// Color of the selected stack segment, with normalized RGBA components.
    #[serde(default = "default_stack_line_selected_color")]
    #[setting(label = "Selected color", custom)]
    pub selected_color: Color,
    /// Color of unselected stack segments, with normalized RGBA components.
    #[serde(default = "default_stack_line_unselected_color")]
    #[setting(label = "Unselected color", custom)]
    pub unselected_color: Color,
    /// Color of segment borders and separators, with normalized RGBA components.
    #[serde(default = "default_stack_line_border_color")]
    #[setting(label = "Border color", custom)]
    pub border_color: Color,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct Color {
    #[serde(default)]
    pub r: f64,
    #[serde(default)]
    pub g: f64,
    #[serde(default)]
    pub b: f64,
    #[serde(default = "default_color_alpha")]
    pub a: f64,
}

fn default_color_alpha() -> f64 { 1.0 }

impl Color {
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self { Self { r, g, b, a } }
}

impl Default for Color {
    fn default() -> Self { Self::new(0.0, 0.0, 0.0, 1.0) }
}

fn default_stack_line_selected_color() -> Color { Color::new(0.0, 0.5, 1.0, 1.0) }

fn default_stack_line_unselected_color() -> Color { Color::new(0.8, 0.8, 0.8, 1.0) }

fn default_stack_line_border_color() -> Color { Color::new(0.6, 0.6, 0.6, 1.0) }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum StackLineHoverMode {
    Click,
    #[default]
    Hover,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct MissionControlSettings {
    /// Include workspaces without windows in Overview.
    #[serde(default = "yes")]
    #[setting(label = "Show empty workspaces", enabled_by = "enabled")]
    pub show_empty_workspaces: bool,
    #[serde(default = "yes")]
    /// Show a preview of each window in Overview.
    #[setting(label = "Window previews", enabled_by = "enabled")]
    pub window_previews: bool,
    #[serde(default)]
    /// Enable the workspace Overview. Requires restarting Rift.
    #[setting(label = "Enabled", order = 0)]
    pub enabled: bool,
    #[serde(default)]
    /// Fade Overview in and out.
    #[setting(label = "Fade transitions", enabled_by = "enabled")]
    pub fade_enabled: bool,
    #[serde(default = "default_mission_control_fade_duration_ms")]
    /// Duration of the Overview fade animation, in milliseconds.
    #[setting(label = "Fade duration (milliseconds)", enabled_by = "enabled")]
    pub fade_duration_ms: f64,
}

fn default_mission_control_fade_duration_ms() -> f64 { 180.0 }

fn default_drop_zone_fraction() -> f64 { 0.25 }

fn default_mouse_action_none() -> MouseAction { MouseAction::None }

fn default_master_stack_ratio() -> f64 { 0.6 }

fn default_master_stack_count() -> usize { 1 }

fn default_scrolling_column_width_ratio() -> f64 { 0.7 }
fn default_scrolling_preset_column_widths() -> Vec<f64> { vec![1.0 / 3.0, 0.5, 2.0 / 3.0] }

fn default_scrolling_min_column_width_ratio() -> f64 { 0.3 }

fn default_scrolling_max_column_width_ratio() -> f64 { 0.9 }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum HorizontalPlacement {
    #[default]
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum VerticalPlacement {
    #[default]
    Left,
    Right,
}

impl StackLineSettings {
    pub fn thickness(&self) -> f64 { if self.enabled { self.thickness } else { 0.0 } }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowInsertionPoint {
    /// Insert a new window immediately after the current selection.
    #[default]
    NextToSelection,
    /// Append a new window at the end of the layout tree.
    EndOfTree,
}

/// Options understood by every layout system.
///
/// These fields are flattened into both `[settings.layout]` and every
/// per-layout table. A per-layout value overrides the layout-wide value.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct BaseLayoutSettings {
    /// Where newly managed windows are inserted.
    #[serde(default)]
    #[setting(label = "New window position", custom)]
    pub window_insertion_point: Option<WindowInsertionPoint>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct TraditionalLayoutSettings {
    #[serde(flatten)]
    #[setting(ignore)]
    pub base: BaseLayoutSettings,
    /// Use Sway-style sibling normalization when inserting nodes. New nodes receive the
    /// average sibling weight instead of splitting the selected node's share.
    #[serde(default = "yes")]
    #[setting(label = "Keep windows equally sized")]
    pub equalize_nodes: bool,
}

impl Default for TraditionalLayoutSettings {
    fn default() -> Self {
        Self {
            base: BaseLayoutSettings::default(),
            equalize_nodes: true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct BspLayoutSettings {
    #[serde(flatten)]
    #[setting(ignore)]
    pub base: BaseLayoutSettings,
    /// Center a lone window at this width-to-height ratio.
    #[setting(label = "Single window aspect ratio", custom)]
    pub single_window_aspect_ratio: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct LayoutSettings {
    /// Settings inherited by every layout type unless overridden by its table.
    #[serde(flatten)]
    #[setting(ignore)]
    pub base: BaseLayoutSettings,
    /// Layout used by workspaces without their own layout override.
    #[serde(default)]
    #[setting(label = "Default layout", choices)]
    pub mode: LayoutMode,
    /// Traditional layout configuration
    #[serde(default)]
    #[setting(ignore)]
    pub traditional: TraditionalLayoutSettings,
    /// BSP layout configuration
    #[serde(default)]
    #[setting(ignore)]
    pub bsp: BspLayoutSettings,
    /// Stack system configuration
    #[serde(default)]
    #[setting(ignore)]
    pub stack: StackSettings,
    /// Master/stack layout configuration
    #[serde(default)]
    #[setting(ignore)]
    pub master_stack: MasterStackSettings,
    #[serde(default)]
    #[setting(ignore)]
    pub gaps: GapSettings,
    /// Scrolling layout configuration (niri-style columns)
    #[serde(default)]
    #[setting(ignore)]
    pub scrolling: ScrollingLayoutSettings,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollingLayoutSettings {
    #[serde(flatten)]
    #[setting(ignore)]
    pub base: BaseLayoutSettings,
    /// Whether to animate windows moving in the scrolling layout
    /// HIGHLY RECOMMENDED to leave this enabled.
    #[serde(default = "default_scrolling_animate")]
    #[setting(
        label = "Animate navigation",
        custom,
        aliases = "animation transitions motion"
    )]
    pub animate: Option<bool>,
    /// Default width of the active column, as a fraction of the screen width.
    #[serde(default = "default_scrolling_column_width_ratio")]
    #[setting(
        label = "Default column width",
        custom,
        aliases = "column width sizing percentages tiling"
    )]
    pub column_width_ratio: f64,
    /// Proportional column widths cycled in configured order.
    #[serde(default = "default_scrolling_preset_column_widths")]
    #[setting(
        label = "Width presets",
        custom,
        aliases = "column width presets cycle sizes"
    )]
    pub preset_column_widths: Vec<f64>,
    /// Keep a window's existing column width when it enters scrolling layout.
    #[serde(default = "yes")]
    #[setting(label = "Preserve window sizes")]
    pub preserve_window_sizes: bool,
    /// Fill the usable width when a workspace has only one scrolling column.
    /// The column's stored width is restored when another column is added.
    #[serde(default)]
    #[setting(label = "Expand single column", custom)]
    pub expand_single_column: bool,
    /// Minimum column width ratio allowed by resize commands.
    #[serde(default = "default_scrolling_min_column_width_ratio")]
    #[setting(
        label = "Minimum width",
        scale = 100.0,
        aliases = "minimum column width sizing percentages tiling"
    )]
    pub min_column_width_ratio: f64,
    /// Maximum column width ratio allowed by resize commands.
    #[serde(default = "default_scrolling_max_column_width_ratio")]
    #[setting(
        label = "Maximum width",
        scale = 100.0,
        aliases = "maximum column width sizing percentages tiling"
    )]
    pub max_column_width_ratio: f64,
    /// Sparse width overrides keyed by display UUID.
    #[serde(default)]
    #[setting(label = "Display settings", custom)]
    pub per_display: HashMap<String, ScrollingWidthOverride>,
    /// Alignment for the focused column (left, center, right).
    #[serde(default)]
    #[setting(label = "Alignment", choices)]
    pub alignment: ScrollingAlignment,
    /// Horizontal focus navigation behavior:
    /// - niri: reveal only as needed based on navigation direction.
    /// - anchored: always align focused column to `alignment`.
    /// Gesture release pans freely in niri mode and snaps to alignment in anchored mode.
    #[serde(default)]
    #[setting(label = "Focus navigation", choices)]
    pub focus_navigation_style: ScrollingFocusNavigationStyle,
    /// Trackpad gestures for scrolling layout
    #[serde(default)]
    #[setting(ignore)]
    pub gestures: ScrollingGestureSettings,
}

fn default_scrolling_animate() -> Option<bool> { Some(true) }

impl Default for ScrollingLayoutSettings {
    fn default() -> Self {
        Self {
            base: BaseLayoutSettings::default(),
            animate: Some(true),
            column_width_ratio: default_scrolling_column_width_ratio(),
            preset_column_widths: default_scrolling_preset_column_widths(),
            preserve_window_sizes: true,
            expand_single_column: false,
            min_column_width_ratio: default_scrolling_min_column_width_ratio(),
            max_column_width_ratio: default_scrolling_max_column_width_ratio(),
            per_display: HashMap::default(),
            alignment: ScrollingAlignment::default(),
            focus_navigation_style: ScrollingFocusNavigationStyle::default(),
            gestures: ScrollingGestureSettings::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ScrollingWidthOverride {
    pub column_width_ratio: Option<f64>,
    pub min_column_width_ratio: Option<f64>,
    pub max_column_width_ratio: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MasterStackSide {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum ScrollingAlignment {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum ScrollingFocusNavigationStyle {
    #[default]
    Niri,
    Anchored,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct MasterStackSettings {
    #[serde(flatten)]
    #[setting(ignore)]
    pub base: BaseLayoutSettings,
    /// Fraction of space reserved for the master area (0.05..0.95)
    #[serde(default = "default_master_stack_ratio")]
    #[setting(label = "Master width", custom)]
    pub master_ratio: f64,
    /// Number of windows kept in the master area (>= 1)
    #[serde(default = "default_master_stack_count")]
    #[setting(label = "Master windows")]
    pub master_count: usize,
    #[serde(default)]
    #[setting(label = "Master side", choices)]
    pub master_side: MasterStackSide,
    /// Where new windows are inserted when the master area is already full
    #[serde(default = "default_master_stack_new_window_placement")]
    #[setting(label = "New windows", choices)]
    pub new_window_placement: MasterStackNewWindowPlacement,
    /// Orientation arrangement for the master area (override default derived from master_side)
    #[serde(default)]
    #[setting(label = "Master arrangement", custom)]
    pub master_arrangement: Option<crate::layout_engine::Orientation>,
    /// Orientation arrangement for the stack area (override default derived from master_side)
    #[serde(default)]
    #[setting(label = "Stack arrangement", custom)]
    pub stack_arrangement: Option<crate::layout_engine::Orientation>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MasterStackNewWindowPlacement {
    Master,
    Stack,
    Focused,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, ConfigSchema)]
#[serde(rename_all = "snake_case")]
pub struct ScrollingGestureSettings {
    /// Enable continuous horizontal viewport gestures
    #[serde(default)]
    #[setting(order = 0)]
    pub enabled: bool,
    /// Animate gesture release independently of structural layout animations.
    /// When omitted, inherit the scrolling layout/global animation setting.
    #[serde(default)]
    #[setting(label = "Animate gestures", custom, order = 4)]
    pub animate: Option<bool>,
    /// Invert horizontal direction (swap left/right)
    #[serde(default)]
    #[setting(label = "Invert horizontal direction", order = 1)]
    pub invert_horizontal: bool,
    /// Maximum absolute Y delta allowed for the gesture to count as horizontal
    #[serde(default = "default_swipe_vertical_tolerance")]
    #[setting(order = 5)]
    pub vertical_tolerance: f64,
    /// Number of fingers required for scroll gesture
    #[serde(default = "default_swipe_fingers")]
    #[setting(order = 2)]
    pub fingers: usize,
    /// Retained for config compatibility; continuous scrolling uses a small intent dead zone
    #[deprecated(since = "0.6.3")]
    #[serde(default = "default_distance_pct")]
    #[setting(ignore)]
    pub distance_pct: f64,
    /// If true, scrolling past the end of the strip will trigger a workspace switch
    #[serde(default)]
    #[setting(label = "Continue into workspace swipe", order = 3)]
    pub propagate_to_workspace_swipe: bool,
    /// Edge travel in working-area widths required on release for one workspace switch.
    /// Measured directly as a fraction of the working-area width.
    #[serde(default = "default_overscroll_threshold")]
    #[setting(label = "Workspace switch threshold (%)", scale = 100.0, order = 6)]
    pub workspace_switch_threshold: f64,
}

impl Default for ScrollingGestureSettings {
    #[allow(deprecated)]
    fn default() -> Self {
        Self {
            enabled: false,
            animate: None,
            invert_horizontal: false,
            vertical_tolerance: default_swipe_vertical_tolerance(),
            fingers: default_swipe_fingers(),
            distance_pct: default_distance_pct(),
            propagate_to_workspace_swipe: false,
            workspace_switch_threshold: default_overscroll_threshold(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum StackDefaultOrientation {
    Perpendicular,
    Same,
    Horizontal,
    Vertical,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct StackSettings {
    #[serde(flatten)]
    #[setting(ignore)]
    pub base: BaseLayoutSettings,
    /// Stack offset - how much each stacked window is offset (in pixels)
    /// With the enhanced stacking system, this creates meaningful visible edges
    /// for each window in the stack while the focused window remains fully visible.
    /// Recommended values: 30-50 pixels for good visibility.
    #[serde(default = "default_stack_offset")]
    #[setting(label = "Window offset (points)", aliases = "stack offset overlap")]
    pub stack_offset: f64,

    /// Default orientation behavior when stacking windows.
    /// Options:
    /// - "perpendicular" (default): choose the perpendicular orientation to the parent layout
    /// - "same": use the same orientation as the parent layout
    /// - "horizontal"/"vertical": explicitly use a specific orientation
    #[serde(default = "default_stack_orientation")]
    #[setting(label = "Orientation", choices)]
    pub default_orientation: StackDefaultOrientation,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct GapSettings {
    #[serde(default)]
    pub outer: OuterGaps,
    #[serde(default)]
    pub inner: InnerGaps,
    #[serde(default)]
    pub per_display: HashMap<String, GapOverride>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct OuterGaps {
    #[serde(default)]
    #[setting(label = "Top", custom)]
    pub top: f64,
    #[serde(default)]
    #[setting(label = "Left", custom)]
    pub left: f64,
    #[serde(default)]
    #[setting(label = "Bottom", custom)]
    pub bottom: f64,
    #[serde(default)]
    #[setting(label = "Right", custom)]
    pub right: f64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default, ConfigSchema)]
#[serde(deny_unknown_fields)]
pub struct InnerGaps {
    #[serde(default)]
    #[setting(label = "Horizontal", custom)]
    pub horizontal: f64,
    #[serde(default)]
    #[setting(label = "Vertical", custom)]
    pub vertical: f64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct GapOverride {
    #[serde(default)]
    pub outer: Option<OuterGaps>,
    #[serde(default)]
    pub inner: Option<InnerGaps>,
}

impl Default for StackSettings {
    fn default() -> Self {
        Self {
            base: BaseLayoutSettings::default(),
            stack_offset: default_stack_offset(),
            default_orientation: default_stack_orientation(),
        }
    }
}

impl Default for MasterStackSettings {
    fn default() -> Self {
        Self {
            base: BaseLayoutSettings::default(),
            master_ratio: default_master_stack_ratio(),
            master_count: default_master_stack_count(),
            master_side: MasterStackSide::Left,
            new_window_placement: default_master_stack_new_window_placement(),
            master_arrangement: None,
            stack_arrangement: None,
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

        if !(0.10..=0.45).contains(&self.drag_drop.drop_zone_fraction) {
            issues.push(format!(
                "drag_drop.drop_zone_fraction must be between 0.10 and 0.45, got {}",
                self.drag_drop.drop_zone_fraction
            ));
        }

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
    pub fn base_for(&self, mode: LayoutMode) -> &BaseLayoutSettings {
        match mode {
            LayoutMode::Traditional => &self.traditional.base,
            LayoutMode::Bsp => &self.bsp.base,
            LayoutMode::Stack => &self.stack.base,
            LayoutMode::MasterStack => &self.master_stack.base,
            LayoutMode::Scrolling => &self.scrolling.base,
            LayoutMode::Floating => &self.base,
        }
    }

    pub fn window_insertion_point_for(&self, mode: LayoutMode) -> WindowInsertionPoint {
        self.base_for(mode)
            .window_insertion_point
            .or(self.base.window_insertion_point)
            .unwrap_or_default()
    }

    pub fn resolved_base_for(&self, mode: LayoutMode) -> BaseLayoutSettings {
        BaseLayoutSettings {
            window_insertion_point: Some(self.window_insertion_point_for(mode)),
        }
    }

    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        issues.extend(self.stack.validate());

        issues.extend(self.master_stack.validate());

        issues.extend(self.gaps.validate());

        issues.extend(self.scrolling.validate());

        issues
    }
}

impl ScrollingLayoutSettings {
    pub fn widths_for_display(&self, display_uuid: Option<&str>) -> (f64, f64, f64) {
        let override_ = display_uuid.and_then(|uuid| self.per_display.get(uuid));
        (
            override_.and_then(|o| o.column_width_ratio).unwrap_or(self.column_width_ratio),
            override_
                .and_then(|o| o.min_column_width_ratio)
                .unwrap_or(self.min_column_width_ratio),
            override_
                .and_then(|o| o.max_column_width_ratio)
                .unwrap_or(self.max_column_width_ratio),
        )
    }

    pub fn effective_for_display(&self, display_uuid: Option<&str>) -> Self {
        let mut resolved = self.clone();
        (
            resolved.column_width_ratio,
            resolved.min_column_width_ratio,
            resolved.max_column_width_ratio,
        ) = self.widths_for_display(display_uuid);
        resolved.per_display.clear();
        resolved
    }

    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        Self::validate_widths("layout.scrolling", self.widths_for_display(None), &mut issues);
        for uuid in self.per_display.keys() {
            Self::validate_widths(
                &format!("layout.scrolling.per_display[{uuid}]"),
                self.widths_for_display(Some(uuid)),
                &mut issues,
            );
        }

        if self.gestures.vertical_tolerance < 0.0 {
            issues.push(format!(
                "layout.scrolling.gestures.vertical_tolerance must be non-negative, got {}",
                self.gestures.vertical_tolerance
            ));
        }

        issues
    }

    fn validate_widths(path: &str, (ratio, min, max): (f64, f64, f64), issues: &mut Vec<String>) {
        if !(0.0..=1.0).contains(&ratio) {
            issues.push(format!(
                "{path}.column_width_ratio must be between 0.0 and 1.0, got {ratio}",
            ));
        }
        if !(0.0..=1.0).contains(&min) {
            issues.push(format!(
                "{path}.min_column_width_ratio must be between 0.0 and 1.0, got {min}",
            ));
        }
        if !(0.0..=1.0).contains(&max) {
            issues.push(format!(
                "{path}.max_column_width_ratio must be between 0.0 and 1.0, got {max}",
            ));
        }
        if min > max {
            issues.push(format!(
                "{path}.min_column_width_ratio ({min}) must be <= max_column_width_ratio ({max})",
            ));
        }
        if !(min..=max).contains(&ratio) {
            issues.push(format!(
                "{path}.column_width_ratio ({ratio}) must be within min/max bounds",
            ));
        }
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

impl MasterStackSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if !(0.05..=0.95).contains(&self.master_ratio) {
            issues.push(format!(
                "master_stack.master_ratio must be between 0.05 and 0.95, got {}",
                self.master_ratio
            ));
        }

        if self.master_count == 0 {
            issues.push("master_stack.master_count must be at least 1".to_string());
        }

        issues
    }
}

impl GapSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        issues.extend(self.outer.validate());

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
    pub fn validate(&self) -> Vec<String> {
        [
            ("top", self.top),
            ("left", self.left),
            ("bottom", self.bottom),
            ("right", self.right),
        ]
        .into_iter()
        .filter(|(_, value)| *value < 0.0)
        .map(|(name, value)| format!("outer.{name} gap must be non-negative, got {value}"))
        .collect()
    }
}

impl InnerGaps {
    pub fn validate(&self) -> Vec<String> {
        [("horizontal", self.horizontal), ("vertical", self.vertical)]
            .into_iter()
            .filter(|(_, value)| *value < 0.0)
            .map(|(name, value)| format!("inner.{name} gap must be non-negative, got {value}"))
            .collect()
    }
}

fn yes() -> bool { true }

fn default_stack_offset() -> f64 { 40.0 }

pub fn default_stack_orientation() -> StackDefaultOrientation {
    StackDefaultOrientation::Perpendicular
}

fn default_master_stack_new_window_placement() -> MasterStackNewWindowPlacement {
    MasterStackNewWindowPlacement::Master
}

fn default_animation_duration() -> f64 { 0.3 }

fn default_animation_fps() -> f64 { 100.0 }

fn default_layout_folder() -> PathBuf { PathBuf::from("~/.config/rift/layouts") }

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
fn default_overscroll_threshold() -> f64 { 0.55 }

fn default_stack_line_spacing() -> f64 { 1.0 }
fn default_stack_line_thickness() -> f64 { 20.0 }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum HapticPattern {
    Generic,
    Alignment,
    #[default]
    LevelChange,
}

impl ConfigSource {
    pub fn keymap(&self, mode: &str) -> Option<&BTreeMap<String, WmCommand>> {
        if mode == "default" {
            Some(&self.keys)
        } else {
            self.binding_modes.get(mode)
        }
    }

    pub fn keymap_mut(&mut self, mode: &str) -> Result<&mut BTreeMap<String, WmCommand>, String> {
        if mode == "default" {
            Ok(&mut self.keys)
        } else {
            self.binding_modes.get_mut(mode).ok_or_else(|| "Keymap no longer exists".into())
        }
    }

    fn check_keymap_name(&self, name: &str, old: Option<&str>) -> Result<(), String> {
        if name.is_empty()
            || name == "default"
            || self.binding_modes.contains_key(name) && old != Some(name)
        {
            Err("Choose a unique keymap name".into())
        } else {
            Ok(())
        }
    }

    pub fn create_keymap(&mut self, name: String) -> Result<(), String> {
        self.check_keymap_name(&name, None)?;
        self.binding_modes.insert(name, Default::default());
        Ok(())
    }

    pub fn rename_keymap(&mut self, old: &str, name: String) -> Result<(), String> {
        self.check_keymap_name(&name, Some(old))?;
        let bindings = self.binding_modes.remove(old).ok_or("Mode no longer exists")?;
        self.binding_modes.insert(name.clone(), bindings);
        self.rewrite_keymap_references(old, &name);
        Ok(())
    }

    pub fn delete_keymap(&mut self, name: &str) -> Result<(), String> {
        self.binding_modes.remove(name).ok_or("Mode no longer exists")?;
        self.rewrite_keymap_references(name, "default");
        Ok(())
    }

    fn rewrite_keymap_references(&mut self, old: &str, target: &str) {
        use rift_protocol::ReactorCommand;

        use crate::actor::reactor::Command;
        use crate::actor::wm_controller::WmCmd;
        for command in self
            .keys
            .values_mut()
            .chain(self.binding_modes.values_mut().flat_map(|map| map.values_mut()))
        {
            match command {
                WmCommand::Wm(WmCmd::BindingMode(mode))
                | WmCommand::ReactorCommand(Command::Reactor(ReactorCommand::BindingMode(mode)))
                    if mode == old =>
                {
                    target.clone_into(mode)
                }
                _ => {}
            }
        }
    }
}
