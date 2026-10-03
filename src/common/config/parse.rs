use std::path::Path;
use std::str::FromStr;

use anyhow::bail;
use serde::Deserialize;

use super::*;
use crate::actor::wm_controller::WmCommand;
use crate::common::collections::HashSet;
use crate::sys::hotkey::Hotkey;

impl Config {
    pub fn read(path: &Path) -> anyhow::Result<Config> {
        let buf = std::fs::read_to_string(path)?;
        Self::parse(&buf)
    }

    pub fn default() -> Config { Self::parse(include_str!("../../../rift.default.toml")).unwrap() }

    /// Validates the entire configuration and returns a list of issues found.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // Validate settings
        issues.extend(self.settings.validate());

        // Validate virtual workspace settings
        issues.extend(self.virtual_workspaces.validate());

        let mode_names: HashSet<_> =
            self.binding_mode_specs.iter().map(|(name, _)| name.as_str()).collect();
        if mode_names.len() != self.binding_mode_specs.len() {
            issues.push("Binding mode names must be unique".to_string());
        }
        if self.binding_mode_specs.first().map(|(name, _)| name.as_str()) != Some("default") {
            issues.push("The default binding mode must be the first mode".to_string());
        }
        for (mode, bindings) in &self.binding_mode_specs {
            for (_, command) in bindings {
                if let WmCommand::Wm(crate::actor::wm_controller::WmCmd::BindingMode(target)) =
                    command
                    && !mode_names.contains(target.as_str())
                {
                    issues.push(format!(
                        "Binding mode `{mode}` references nonexistent mode `{target}`"
                    ));
                }
            }
        }

        issues
    }

    fn normalize_hotkey_string(key: &str) -> String {
        key.split('+')
            .map(|word| {
                let word = word.trim();
                match word.to_ascii_lowercase().as_str() {
                    "up" => "ArrowUp".to_string(),
                    "down" => "ArrowDown".to_string(),
                    "left" => "ArrowLeft".to_string(),
                    "right" => "ArrowRight".to_string(),
                    _ if word.len() == 1 => word.to_ascii_uppercase(),
                    _ => word.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }

    fn expand_modifier_combinations(
        key: &str,
        combinations: &std::collections::BTreeMap<String, String>,
    ) -> String {
        if let Some(plus_pos) = key.find(" + ") {
            let potential_combo = &key[..plus_pos];
            if let Some(combo_value) = combinations.get(potential_combo) {
                let rest = &key[plus_pos + 3..];
                return format!("{} + {}", combo_value, rest);
            }
        }
        key.to_string()
    }

    fn levenshtein(a: &str, b: &str) -> usize {
        let b: Vec<_> = b.chars().collect();
        let mut row: Vec<_> = (0..=b.len()).collect();
        for (i, left) in a.chars().enumerate() {
            let mut diagonal = row[0];
            row[0] = i + 1;
            for (j, right) in b.iter().enumerate() {
                let above = row[j + 1];
                row[j + 1] =
                    (above + 1).min(row[j] + 1).min(diagonal + usize::from(left != *right));
                diagonal = above;
            }
        }
        row[b.len()]
    }

    fn extract_unknown_variant(error: &str) -> Option<String> {
        let rest = error.split_once("unknown variant `")?.1;
        let (unknown, expected) = rest.split_once('`')?;
        let candidates = expected
            .split('`')
            .enumerate()
            .filter(|(i, _)| i % 2 == 1)
            .map(|(_, name)| name)
            .collect::<Vec<_>>();
        Some(format!("{unknown}||{}", candidates.join(",")))
    }

    fn suggest_similar_command(unknown: &str) -> Option<(String, Option<String>)> {
        let (token, candidates) = unknown.split_once("||").unwrap_or((unknown, ""));
        let builtin = crate::actor::wm_controller::WmCmd::snake_case_variants();
        let candidates: Vec<&str> = if candidates.is_empty() {
            builtin.iter().map(|name| name.as_ref()).collect()
        } else {
            candidates.split(',').collect()
        };
        let (best, distance) = candidates
            .into_iter()
            .map(|name| {
                (
                    name,
                    Self::levenshtein(&token.to_lowercase(), &name.to_lowercase()),
                )
            })
            .min_by_key(|(_, distance)| *distance)?;
        if distance > std::cmp::max(3, best.len() / 2) {
            return None;
        }
        let replacement = DEPRECATED_MAP
            .iter()
            .find(|(name, _)| *name == best)
            .map(|(_, replacement)| replacement.to_string());
        Some((best.to_string(), replacement))
    }

    pub(crate) fn parse(buf: &str) -> anyhow::Result<Config> {
        // Attempt to deserialize. If it fails, and the error indicates an unknown enum
        // variant, attempt to provide a helpful suggestion.
        match parse_config_file(buf) {
            Ok(c) => {
                if c.binding_modes.contains_key("default") {
                    bail!("`default` is reserved and cannot be defined in [binding_modes]");
                }

                let mut binding_sets: Vec<_> = c.binding_modes.into_iter().collect();
                binding_sets.sort_by(|a, b| a.0.cmp(&b.0));
                binding_sets.insert(0, ("default".to_string(), c.keys));

                let mut binding_mode_specs = Vec::with_capacity(binding_sets.len());
                let mut keys = Vec::new();
                let mode_names: HashSet<String> =
                    binding_sets.iter().map(|(name, _)| name.clone()).collect();
                for (mode, bindings) in binding_sets {
                    let mut specs = Vec::with_capacity(bindings.len());
                    for (key, cmd) in bindings {
                        let expanded_key =
                            Self::expand_modifier_combinations(&key, &c.modifier_combinations);
                        let normalized_key = Self::normalize_hotkey_string(&expanded_key);
                        let Ok(hotkey) = Hotkey::from_str(&normalized_key) else {
                            bail!("Could not parse hotkey `{key}` in binding mode `{mode}`");
                        };
                        if let WmCommand::Wm(crate::actor::wm_controller::WmCmd::BindingMode(
                            target,
                        )) = &cmd
                            && target != "default"
                            && !mode_names.contains(target)
                        {
                            bail!("Binding mode `{mode}` references nonexistent mode `{target}`");
                        }
                        if mode == "default" {
                            keys.push((hotkey, cmd.clone()));
                        }
                        specs.push((normalized_key, cmd));
                    }
                    binding_mode_specs.push((mode, specs));
                }
                Ok(Config {
                    settings: c.settings,
                    keys,
                    binding_mode_specs,
                    virtual_workspaces: c.virtual_workspaces,
                })
            }
            Err(error) => {
                let message = error.to_string();
                if let Some((suggestion, replacement)) = Self::extract_unknown_variant(&message)
                    .and_then(|token| Self::suggest_similar_command(&token))
                {
                    let note = replacement
                        .map(|replacement| {
                            format!(
                                " Note: `{suggestion}` is deprecated; use `{replacement}` instead."
                            )
                        })
                        .unwrap_or_default();
                    bail!("{message}\nDid you mean `{suggestion}`?{note}");
                }
                bail!("{message}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_suggestion_from_serde_error() {
        let error =
            "unknown variant `toggle_stak`, expected one of `toggle_stack`, `toggle_orientation`";
        let token = Config::extract_unknown_variant(error).unwrap();
        assert_eq!(
            Config::suggest_similar_command(&token).unwrap().0,
            "toggle_stack"
        );
    }
}

// Historical runtime snapshots are used by reactor recording/replay, never TOML writes.
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D>(deserializer: D) -> Result<Config, D::Error>
    where D: serde::Deserializer<'de> {
        #[derive(Deserialize)]
        struct ConfigSerde {
            settings: Settings,
            keys: Vec<(Hotkey, WmCommand)>,
            #[serde(default)]
            key_specs: Vec<(String, WmCommand)>,
            #[serde(default)]
            binding_mode_specs: BindingModeSpecs,
            virtual_workspaces: VirtualWorkspaceSettings,
        }

        let config = ConfigSerde::deserialize(deserializer)?;
        let binding_mode_specs = if config.binding_mode_specs.is_empty() {
            let default_specs = if config.key_specs.is_empty() {
                config
                    .keys
                    .iter()
                    .map(|(hotkey, command)| (hotkey.to_string(), command.clone()))
                    .collect()
            } else {
                config.key_specs
            };
            vec![("default".to_string(), default_specs)]
        } else {
            config.binding_mode_specs
        };

        Ok(Config {
            settings: config.settings,
            keys: config.keys,
            binding_mode_specs,
            virtual_workspaces: config.virtual_workspaces,
        })
    }
}

pub(super) fn migrate_legacy_resize_bindings(document: &mut toml::Value) -> bool {
    let Some(keys) = document.get_mut("keys").and_then(toml::Value::as_table_mut) else {
        return false;
    };

    let mut migrated = false;
    for (_, command) in keys.iter_mut() {
        let legacy_name = match command.as_str() {
            Some("resize_window_grow") => "resize_window_grow",
            Some("resize_window_shrink") => "resize_window_shrink",
            _ => continue,
        };
        *command = toml::Value::Table(toml::map::Map::from_iter([(
            legacy_name.to_string(),
            toml::Value::String("horizontal".to_string()),
        )]));
        migrated = true;
    }
    migrated
}

fn migrate_legacy_window_snapping(document: &mut toml::Value) -> bool {
    let Some(settings) = document.get_mut("settings").and_then(toml::Value::as_table_mut) else {
        return false;
    };
    settings.remove("window_snapping").is_some()
}

pub(super) fn parse_config_file(buf: &str) -> Result<ConfigSource, toml::de::Error> {
    let mut document = toml::from_str::<toml::Value>(buf)?;
    migrate_legacy_resize_bindings(&mut document);
    migrate_legacy_window_snapping(&mut document);
    document.try_into()
}
