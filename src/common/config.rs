use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::bail;
use livesplit_hotkey::Hotkey;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::actor::wm_controller::WmCommand;

const MAX_WORKSPACES: usize = 32;

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

    GetConfig,
    SaveConfig,
    ReloadConfig,
}

pub fn data_dir() -> PathBuf { dirs::home_dir().unwrap().join(".rift") }
pub fn restore_file() -> PathBuf { data_dir().join("layout.ron") }
pub fn config_file() -> PathBuf { dirs::home_dir().unwrap().join(".rift.toml") }

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
    #[serde(default = "default_workspace_names")]
    pub workspace_names: Vec<String>,
    #[serde(default)]
    pub default_workspace: usize,
    #[serde(default)]
    pub app_rules: Vec<AppWorkspaceRule>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct AppWorkspaceRule {
    /// Application bundle identifier (e.g., "com.apple.Terminal")
    pub app_id: Option<String>,
    /// Target workspace index (0 based). If None, window goes to active workspace.
    pub workspace: Option<usize>,
    /// Whether windows should be floating in this workspace
    #[serde(default)]
    pub floating: bool,
    /// Optional: Application name pattern (alternative to app_id)
    pub app_name: Option<String>,
}

impl Default for VirtualWorkspaceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            default_workspace_count: default_workspace_count(),
            auto_assign_windows: true,
            preserve_focus_per_workspace: true,
            workspace_names: default_workspace_names(),
            default_workspace: 0,
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

        // Validate rules and check duplicates in a single pass
        let mut seen_app_ids = crate::common::collections::HashSet::default();
        let mut seen_app_names = crate::common::collections::HashSet::default();

        for (index, rule) in self.app_rules.iter().enumerate() {
            let app_id_empty = rule.app_id.as_ref().map_or(true, |id| id.is_empty());
            if app_id_empty && rule.app_name.is_none() {
                issues.push(format!("App rule {} has no app_id or app_name specified", index));
            }

            if let Some(ref workspace) = rule.workspace {
                if *workspace >= self.default_workspace_count {
                    issues.push(format!(
                        "App rule {} references workspace {} but only {} workspaces will be created",
                        index, workspace, self.default_workspace_count
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

                if !app_id.is_empty() && !seen_app_ids.insert(app_id) {
                    issues.push(format!("Duplicate app_id '{}' in rule {}", app_id, index));
                }
            }

            if let Some(ref app_name) = rule.app_name {
                if !seen_app_names.insert(app_name) {
                    issues.push(format!("Duplicate app_name '{}' in rule {}", app_name, index));
                }
            }
        }

        issues
    }

    pub fn auto_fix(&mut self) -> usize {
        let mut fixes = 0;

        if self.default_workspace_count == 0 {
            self.default_workspace_count = 1;
            fixes += 1;
        }
        if self.default_workspace_count > MAX_WORKSPACES {
            self.default_workspace_count = MAX_WORKSPACES;
            fixes += 1;
        }

        for rule in &mut self.app_rules {
            if let Some(ref workspace) = rule.workspace {
                if *workspace >= self.default_workspace_count {
                    rule.workspace = None;
                    fixes += 1;
                }
            }
        }

        let initial_rule_count = self.app_rules.len();
        self.app_rules.retain(|rule| {
            let app_id_valid = rule.app_id.as_ref().map_or(false, |id| !id.is_empty());
            app_id_valid || rule.app_name.is_some()
        });
        fixes += initial_rule_count - self.app_rules.len();

        fixes
    }

    pub fn auto_fix_values(&mut self) -> usize {
        // for now, the VirtualWorkspaceSettings doesn't have invalid values that need fixing
        0
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    settings: Settings,
    keys: FxHashMap<String, WmCommand>,
    #[serde(default)]
    virtual_workspaces: VirtualWorkspaceSettings,
    /// Modifier combinations that can be reused in key bindings
    /// e.g., "comb1" = "Alt + Shift" allows using "comb1 + C" in keys
    #[serde(default)]
    modifier_combinations: FxHashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub settings: Settings,
    pub keys: Vec<(Hotkey, WmCommand)>,
    pub virtual_workspaces: VirtualWorkspaceSettings,
}

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
    /// Apps that should not trigger automatic workspace switching when activated.
    /// List of bundle identifiers (e.g., "com.apple.Spotlight") that often
    /// inappropriately steal focus and shouldn't cause workspace switches.
    #[serde(default)]
    pub auto_focus_blacklist: Vec<String>,
    #[serde(default)]
    pub layout: LayoutSettings,
    /// Commands to run on startup (e.g., for subscribing to events)
    #[serde(default)]
    pub run_on_start: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(deny_unknown_fields)]
pub struct StackSettings {
    /// Stack offset - how much each stacked window is offset (in pixels)
    /// With the enhanced stacking system, this creates meaningful visible edges
    /// for each window in the stack while the focused window remains fully visible.
    /// Recommended values: 30-50 pixels for good visibility.
    #[serde(default = "default_stack_offset")]
    pub stack_offset: f64,
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

impl Default for StackSettings {
    fn default() -> Self {
        Self {
            stack_offset: default_stack_offset(),
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

        issues
    }

    pub fn auto_fix_values(&mut self) -> usize {
        let mut fixes = 0;

        if self.animation_duration < 0.0 {
            self.animation_duration = default_animation_duration();
            fixes += 1;
        }

        if self.animation_fps <= 0.0 {
            self.animation_fps = default_animation_fps();
            fixes += 1;
        }

        fixes += self.layout.auto_fix_values();

        fixes
    }
}

impl LayoutSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        issues.extend(self.stack.validate());

        issues.extend(self.gaps.validate());

        issues
    }

    pub fn auto_fix_values(&mut self) -> usize {
        let stack_fixes = self.stack.auto_fix_values();
        let gap_fixes = self.gaps.auto_fix_values();

        stack_fixes + gap_fixes
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

    pub fn auto_fix_values(&mut self) -> usize {
        let mut fixes = 0;

        if self.stack_offset < 0.0 {
            self.stack_offset = default_stack_offset();
            fixes += 1;
        }

        fixes
    }
}

impl GapSettings {
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // Validate outer gaps
        issues.extend(self.outer.validate());

        // Validate inner gaps
        issues.extend(self.inner.validate());

        issues
    }

    pub fn auto_fix_values(&mut self) -> usize {
        // Fix outer gaps
        let outer_fixes = self.outer.auto_fix_values();

        // Fix inner gaps
        let inner_fixes = self.inner.auto_fix_values();

        outer_fixes + inner_fixes
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

    /// Attempts to fix outer gap configuration values automatically.
    /// Returns the number of fixes applied.
    pub fn auto_fix_values(&mut self) -> usize {
        let mut fixes = 0;

        if self.top < 0.0 {
            self.top = 0.0;
            fixes += 1;
        }

        if self.left < 0.0 {
            self.left = 0.0;
            fixes += 1;
        }

        if self.bottom < 0.0 {
            self.bottom = 0.0;
            fixes += 1;
        }

        if self.right < 0.0 {
            self.right = 0.0;
            fixes += 1;
        }

        fixes
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

    /// Attempts to fix inner gap configuration values automatically.
    /// Returns the number of fixes applied.
    pub fn auto_fix_values(&mut self) -> usize {
        let mut fixes = 0;

        if self.horizontal < 0.0 {
            self.horizontal = 0.0;
            fixes += 1;
        }

        if self.vertical < 0.0 {
            self.vertical = 0.0;
            fixes += 1;
        }

        fixes
    }
}

// Default for OuterGaps/InnerGaps now derived

fn yes() -> bool { true }

fn default_stack_offset() -> f64 { 40.0 }

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
            modifier_combinations: FxHashMap::default(),
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

    /// Attempts to fix configuration values automatically.
    /// Returns the number of fixes applied.
    pub fn auto_fix_values(&mut self) -> usize {
        let mut fixes = 0;

        // Fix settings
        fixes += self.settings.auto_fix_values();

        // Fix virtual workspace settings
        fixes += self.virtual_workspaces.auto_fix_values();

        fixes
    }

    fn normalize_hotkey_string(key: &str) -> String {
        // Map standalone direction words to Arrow* while preserving all separators/spaces.
        fn map_word(word: &str) -> &str {
            match word {
                "Up" => "ArrowUp",
                "Down" => "ArrowDown",
                "Left" => "ArrowLeft",
                "Right" => "ArrowRight",
                _ => word,
            }
        }

        let mut out = String::with_capacity(key.len());
        let mut word = String::new();

        for ch in key.chars() {
            if ch.is_alphabetic() {
                word.push(ch);
            } else {
                if !word.is_empty() {
                    out.push_str(map_word(&word));
                    word.clear();
                }
                out.push(ch);
            }
        }

        if !word.is_empty() {
            out.push_str(map_word(&word));
        }

        out
    }

    fn expand_modifier_combinations(key: &str, combinations: &FxHashMap<String, String>) -> String {
        if let Some(plus_pos) = key.find(" + ") {
            let potential_combo = &key[..plus_pos];
            if let Some(combo_value) = combinations.get(potential_combo) {
                let rest = &key[plus_pos + 3..];
                return format!("{} + {}", combo_value, rest);
            }
        }
        key.to_string()
    }

    fn parse(buf: &str) -> anyhow::Result<Config> {
        let c: ConfigFile = toml::from_str(&buf)?;
        let mut keys = Vec::new();
        for (key, cmd) in c.keys {
            let expanded_key = Self::expand_modifier_combinations(&key, &c.modifier_combinations);
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

        assert_eq!(Config::normalize_hotkey_string("Alt+Down"), "Alt+ArrowDown");
        assert_eq!(Config::normalize_hotkey_string("Ctrl+Up"), "Ctrl+ArrowUp");
        assert_eq!(Config::normalize_hotkey_string("Shift+Left"), "Shift+ArrowLeft");
        assert_eq!(Config::normalize_hotkey_string("Meta+Right"), "Meta+ArrowRight");

        assert_eq!(Config::normalize_hotkey_string("Alt + H"), "Alt + H");
        assert_eq!(Config::normalize_hotkey_string("Ctrl + Enter"), "Ctrl + Enter");
        assert_eq!(Config::normalize_hotkey_string("F1"), "F1");

        assert_eq!(
            Config::normalize_hotkey_string("Up + Down"),
            "ArrowUp + ArrowDown"
        );
    }

    #[test]
    fn default_config_parses() { super::Config::default(); }

    #[test]
    fn test_expand_modifier_combinations() {
        let mut combinations = FxHashMap::default();
        combinations.insert("comb1".to_string(), "Alt + Shift".to_string());
        combinations.insert("leader".to_string(), "Ctrl + Alt".to_string());

        assert_eq!(
            Config::expand_modifier_combinations("comb1 + C", &combinations),
            "Alt + Shift + C"
        );

        assert_eq!(
            Config::expand_modifier_combinations("leader + Tab", &combinations),
            "Ctrl + Alt + Tab"
        );

        assert_eq!(
            Config::expand_modifier_combinations("Alt + H", &combinations),
            "Alt + H"
        );

        assert_eq!(
            Config::expand_modifier_combinations("unknown + X", &combinations),
            "unknown + X"
        );

        let empty_combinations = FxHashMap::default();
        assert_eq!(
            Config::expand_modifier_combinations("comb1 + C", &empty_combinations),
            "comb1 + C"
        );
    }

    #[test]
    fn test_modifier_combinations_in_config() {
        let config_str = r#"
            [settings]
            animate = false

            [modifier_combinations]
            comb1 = "Alt + Shift"
            leader = "Ctrl + Alt"

            [keys]
            "comb1 + C" = "toggle_space_activated"
            "leader + Tab" = "next_workspace"
            "Alt + H" = { move_focus = "left" }

            [virtual_workspaces]
            enabled = false
        "#;

        let config = Config::parse(config_str).unwrap();
        assert_eq!(config.keys.len(), 3);

        // Check that the combinations were expanded correctly
        // Note: We can't easily check the exact Hotkey values since they're parsed,
        // but we can verify parsing succeeds and the right number of keys are present
        assert!(config.keys.iter().any(|(hotkey, _)| {
            // This would be Alt + Shift + C
            hotkey.key_code == livesplit_hotkey::KeyCode::KeyC
        }));
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config::default();

        let issues = config.validate();
        assert!(issues.is_empty());

        config.settings.animation_duration = -1.0;
        let issues = config.validate();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("animation_duration must be non-negative"));

        let fixes = config.auto_fix_values();
        assert_eq!(fixes, 1);
        assert_eq!(config.settings.animation_duration, 0.3);

        config.settings.layout.stack.stack_offset = -10.0;
        let issues = config.validate();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("stack_offset must be non-negative"));

        let fixes = config.auto_fix_values();
        assert_eq!(fixes, 1);
        assert_eq!(config.settings.layout.stack.stack_offset, 40.0);
    }
}
