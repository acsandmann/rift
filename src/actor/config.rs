use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::actor::{self, reactor};
use crate::common::config::{Config, ConfigCommand};

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

#[derive(Serialize, Deserialize, Debug)]
pub enum Event {
    #[serde(skip)]
    QueryConfig(r#continue::Sender<Config>),
    #[serde(skip)]
    ApplyConfig {
        cmd: ConfigCommand,
        #[serde(skip)]
        response: r#continue::Sender<Result<(), String>>,
    },
}

pub struct ConfigActor {
    config: Config,
    reactor_tx: reactor::Sender,
    config_path: PathBuf,
}

impl ConfigActor {
    pub fn spawn(config: Config, reactor_tx: reactor::Sender) -> Sender {
        Self::spawn_with_path(config, reactor_tx, crate::common::config::config_file())
    }

    pub fn spawn_with_path(
        config: Config,
        reactor_tx: reactor::Sender,
        config_path: PathBuf,
    ) -> Sender {
        let (tx, rx) = actor::channel();
        std::thread::Builder::new()
            .name("config".to_string())
            .spawn(move || {
                let actor = ConfigActor {
                    config,
                    reactor_tx,
                    config_path,
                };
                crate::sys::executor::Executor::run(actor.run(rx));
            })
            .unwrap();
        tx
    }

    async fn run(mut self, mut events: Receiver) {
        while let Some((_span, event)) = events.recv().await {
            match event {
                Event::QueryConfig(resp) => {
                    let v = self.handle_config_query();
                    let _ = resp.send(v);
                }
                Event::ApplyConfig { cmd, response } => {
                    let res = self.handle_config_command(cmd);
                    let _ = response.send(res);
                }
            }
        }
    }

    fn handle_config_query(&self) -> Config { self.config.clone() }

    fn handle_config_command(&mut self, cmd: ConfigCommand) -> Result<(), String> {
        debug!("Applying config command: {:?}", cmd);

        let mut new_config = self.config.clone();
        let mut config_changed = false;
        let mut errors: Vec<String> = Vec::new();

        macro_rules! set_flag {
            ($path:expr, $value:expr, $name:literal) => {{
                $path = $value;
                config_changed = true;
                info!("Updated {} to: {}", $name, $value);
            }};
        }

        let mut set_range = |name: &str, target: &mut f64, value: f64, min: f64, max: f64| {
            if value >= min && value <= max {
                *target = value;
                config_changed = true;
                info!("Updated {} to: {}", name, value);
            } else {
                errors.push(format!(
                    "Invalid {} value: {}. Must be between {} and {}",
                    name, value, min, max
                ));
            }
        };

        match cmd {
            ConfigCommand::SetAnimate(v) => set_flag!(new_config.settings.animate, v, "animate"),
            ConfigCommand::SetAnimationDuration(v) => set_range(
                "animation_duration",
                &mut new_config.settings.animation_duration,
                v,
                0.0,
                5.0,
            ),
            ConfigCommand::SetAnimationFps(v) => set_range(
                "animation_fps",
                &mut new_config.settings.animation_fps,
                v,
                0.0,
                240.0,
            ),
            ConfigCommand::SetAnimationEasing(v) => {
                new_config.settings.animation_easing = v.clone();
                config_changed = true;
                info!(
                    "Updated animation_easing to: {:?}",
                    new_config.settings.animation_easing
                );
            }
            ConfigCommand::SetMouseFollowsFocus(v) => {
                set_flag!(new_config.settings.mouse_follows_focus, v, "mouse_follows_focus")
            }
            ConfigCommand::SetMouseHidesOnFocus(v) => {
                set_flag!(
                    new_config.settings.mouse_hides_on_focus,
                    v,
                    "mouse_hides_on_focus"
                )
            }
            ConfigCommand::SetFocusFollowsMouse(v) => {
                set_flag!(new_config.settings.focus_follows_mouse, v, "focus_follows_mouse")
            }
            ConfigCommand::SetStackOffset(v) => set_range(
                "stack_offset",
                &mut new_config.settings.layout.stack.stack_offset,
                v,
                0.0,
                200.0,
            ),
            ConfigCommand::SetOuterGaps { top, left, bottom, right } => {
                if [top, left, bottom, right].into_iter().all(|v| v >= 0.0) {
                    let gaps = &mut new_config.settings.layout.gaps.outer;
                    gaps.top = top;
                    gaps.left = left;
                    gaps.bottom = bottom;
                    gaps.right = right;
                    config_changed = true;
                    info!(
                        "Updated outer gaps to: top={}, left={}, bottom={}, right={}",
                        top, left, bottom, right
                    );
                } else {
                    errors.push("Invalid outer gap values. All values must be >= 0.0".to_string());
                }
            }
            ConfigCommand::SetInnerGaps { horizontal, vertical } => {
                if horizontal >= 0.0 && vertical >= 0.0 {
                    let gaps = &mut new_config.settings.layout.gaps.inner;
                    gaps.horizontal = horizontal;
                    gaps.vertical = vertical;
                    config_changed = true;
                    info!(
                        "Updated inner gaps to: horizontal={}, vertical={}",
                        horizontal, vertical
                    );
                } else {
                    errors.push("Invalid inner gap values. All values must be >= 0.0".to_string());
                }
            }
            ConfigCommand::SetWorkspaceNames(names) => {
                if names.len() <= 32 {
                    new_config.virtual_workspaces.workspace_names = names.clone();
                    config_changed = true;
                    info!("Updated workspace names to: {:?}", names);
                } else {
                    errors.push("Too many workspace names provided. Maximum is 32".to_string());
                }
            }

            ConfigCommand::Set { key, value } => match serde_json::to_value(&new_config) {
                Ok(mut cfg_val) => {
                    let parts: Vec<&str> = key.split('.').collect();
                    if parts.is_empty() {
                        errors.push("Empty config key provided".to_string());
                    } else {
                        let mut cur = &mut cfg_val;
                        let mut failed = false;
                        for (i, part) in parts.iter().enumerate() {
                            if i + 1 == parts.len() {
                                if let Some(obj) = cur.as_object_mut() {
                                    obj.insert(part.to_string(), value.clone());
                                } else {
                                    errors.push(format!("Invalid config path: {}", key));
                                    failed = true;
                                }
                            } else if let Some(obj) = cur.as_object_mut() {
                                if !obj.contains_key(*part) {
                                    obj.insert(part.to_string(), serde_json::json!({}));
                                }
                                cur = obj.get_mut(*part).unwrap();
                            } else {
                                errors.push(format!("Invalid config path: {}", key));
                                failed = true;
                                break;
                            }
                        }

                        if !failed {
                            match serde_json::from_value::<Config>(cfg_val) {
                                Ok(cfg2) => {
                                    new_config = cfg2;
                                    config_changed = true;
                                    info!("Updated {} to {}", key, value);
                                }
                                Err(e) => {
                                    errors.push(format!(
                                        "Failed to deserialize config after setting '{}': {}",
                                        key, e
                                    ));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("Failed to serialize config for modification: {}", e))
                }
            },

            ConfigCommand::GetConfig => {
                let config_json = serde_json::to_string_pretty(&self.config)
                    .unwrap_or_else(|e| format!("Error serializing config: {}", e));
                info!("Current config:\n{}", config_json);
                return Ok(());
            }
            ConfigCommand::SaveConfig => match self.save_config_to_file() {
                Ok(()) => {
                    info!("Config saved successfully");
                    return Ok(());
                }
                Err(e) => return Err(format!("Failed to save config: {}", e)),
            },
            ConfigCommand::ReloadConfig => match self.load_config_from_file() {
                Ok(cfg) => {
                    info!("Config reloaded successfully");
                    config_changed = true;
                    new_config = cfg;
                }
                Err(e) => return Err(format!("Failed to reload config: {}", e)),
            },
        }

        if !errors.is_empty() {
            return Err(errors.join("; "));
        }

        let validation_issues = new_config.validate();
        if !validation_issues.is_empty() {
            return Err(validation_issues.join("; "));
        }

        if config_changed {
            let validation_issues = new_config.validate();
            if !validation_issues.is_empty() {
                return Err(validation_issues.join("; "));
            }

            self.config = new_config;

            self.reactor_tx.send(reactor::Event::ConfigUpdated(self.config.clone()));
        }

        Ok(())
    }

    fn save_config_to_file(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = &self.config_path;
        self.config.save(config_path)?;
        Ok(())
    }

    fn load_config_from_file(
        &mut self,
    ) -> Result<crate::common::config::Config, Box<dyn std::error::Error>> {
        let config_path = &self.config_path;

        if config_path.exists() {
            let new_config = crate::common::config::Config::read(config_path)?;
            Ok(new_config)
        } else {
            Err("Config file not found".into())
        }
    }
}
