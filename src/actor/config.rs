use std::path::PathBuf;
use std::sync::mpsc::SyncSender;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::actor::{self, reactor};
use crate::common::config::{Config, ConfigCommand, ConfigDocument, MAX_WORKSPACES};

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

#[derive(Serialize, Deserialize, Debug)]
pub enum Event {
    #[serde(skip)]
    QueryConfig(SyncSender<Config>),
    #[serde(skip)]
    ApplyConfig {
        cmd: ConfigCommand,
        #[serde(skip)]
        response: SyncSender<Result<(), String>>,
    },
}

pub struct ConfigActor {
    config: Config,
    document: ConfigDocument,
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
                let document = if config_path.exists() {
                    ConfigDocument::read(&config_path).expect("startup config must parse")
                } else {
                    ConfigDocument::default()
                };
                let actor = ConfigActor {
                    document,
                    config,
                    reactor_tx,
                    config_path,
                };
                actor.run(rx);
            })
            .unwrap();
        tx
    }

    fn run(mut self, mut events: Receiver) {
        while let Some((_span, event)) = events.blocking_recv() {
            match event {
                Event::QueryConfig(resp) => {
                    let _ = resp.send(self.config.clone());
                }
                Event::ApplyConfig { cmd, response } => {
                    let res = self.handle_config_command(cmd);
                    let _ = response.send(res);
                }
            }
        }
    }

    fn handle_config_command(&mut self, cmd: ConfigCommand) -> Result<(), String> {
        debug!("Applying config command: {:?}", cmd);

        match &cmd {
            ConfigCommand::GetConfig => {
                info!(
                    "Current config:\n{}",
                    serde_json::to_string_pretty(&self.config).map_err(|e| e.to_string())?
                );
                return Ok(());
            }
            ConfigCommand::SaveConfig => {
                return self.save_config_to_file().map_err(|e| e.to_string());
            }
            ConfigCommand::ReloadConfig => {
                self.config = self.load_config_from_file().map_err(|e| e.to_string())?;
            }
            ConfigCommand::Set { key, value } => {
                self.config = self.document.set(key, value).map_err(|e| e.to_string())?;
            }
            _ => {
                let range = match &cmd {
                    ConfigCommand::SetAnimationDuration(v) => {
                        Some(("animation_duration", *v, 0.0, 5.0))
                    }
                    ConfigCommand::SetAnimationFps(v) => Some(("animation_fps", *v, 0.0, 240.0)),
                    ConfigCommand::SetStackOffset(v) => Some(("stack_offset", *v, 0.0, 200.0)),
                    _ => None,
                };
                if let Some((name, value, min, max)) = range
                    && !(min..=max).contains(&value)
                {
                    return Err(format!(
                        "Invalid {name} value: {value}. Must be between {min} and {max}"
                    ));
                }
                if let ConfigCommand::SetWorkspaceNames(names) = &cmd
                    && names.len() > MAX_WORKSPACES
                {
                    return Err(format!(
                        "Too many workspace names provided. Maximum is {MAX_WORKSPACES}"
                    ));
                }
                self.config = self
                    .document
                    .update(|source| {
                        let settings = &mut source.settings;
                        match cmd {
                            ConfigCommand::SetAnimate(v) => settings.animate = v,
                            ConfigCommand::SetAnimationDuration(v) => {
                                settings.animation_duration = v
                            }
                            ConfigCommand::SetAnimationFps(v) => settings.animation_fps = v,
                            ConfigCommand::SetAnimationEasing(v) => settings.animation_easing = v,
                            ConfigCommand::SetMouseFollowsFocus(v) => {
                                settings.mouse_follows_focus = v
                            }
                            ConfigCommand::SetMouseHidesOnFocus(v) => {
                                settings.mouse_hides_on_focus = v
                            }
                            ConfigCommand::SetFocusFollowsMouse(v) => {
                                settings.focus_follows_mouse = v
                            }
                            ConfigCommand::SetStackOffset(v) => {
                                settings.layout.stack.stack_offset = v
                            }
                            ConfigCommand::SetOuterGaps { top, left, bottom, right } => {
                                settings.layout.gaps.outer =
                                    crate::common::config::OuterGaps { top, left, bottom, right };
                            }
                            ConfigCommand::SetInnerGaps { horizontal, vertical } => {
                                settings.layout.gaps.inner =
                                    crate::common::config::InnerGaps { horizontal, vertical };
                            }
                            ConfigCommand::SetWorkspaceNames(names) => {
                                source.virtual_workspaces.workspace_names = names
                            }
                            _ => unreachable!(),
                        }
                    })
                    .map_err(|e| e.to_string())?;
            }
        }
        self.reactor_tx.send(reactor::Event::ConfigUpdated(self.config.clone()));
        Ok(())
    }

    fn save_config_to_file(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = &self.config_path;
        self.document.save(config_path)?;
        Ok(())
    }

    fn load_config_from_file(
        &mut self,
    ) -> Result<crate::common::config::Config, Box<dyn std::error::Error>> {
        let config_path = &self.config_path;

        if config_path.exists() {
            let document = ConfigDocument::read(config_path)?;
            let new_config = document.runtime()?;
            let issues = new_config.validate();
            if !issues.is_empty() {
                return Err(issues.join("; ").into());
            }
            self.document = document;
            Ok(new_config)
        } else {
            Err("Config file not found".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    use super::*;

    #[test]
    fn commands_save_and_reload_the_source_document() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let text = include_str!("../../rift.default.toml");
        std::fs::write(&path, text).unwrap();
        let (reactor_tx, _updates) = actor::channel();
        let mut actor = ConfigActor {
            config: Config::default(),
            document: ConfigDocument::read(&path).unwrap(),
            reactor_tx,
            config_path: path.clone(),
        };
        actor.handle_config_command(ConfigCommand::SetAnimate(true)).unwrap();
        actor.handle_config_command(ConfigCommand::SaveConfig).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text.replace("animate = false", "animate = true")
        );
        let external = format!("# external edit\n{text}");
        std::fs::write(&path, &external).unwrap();
        actor.handle_config_command(ConfigCommand::ReloadConfig).unwrap();
        actor
            .handle_config_command(ConfigCommand::Set {
                key: "settings.animation_duration".into(),
                value: serde_json::json!(0.2),
            })
            .unwrap();
        actor.handle_config_command(ConfigCommand::SaveConfig).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            external.replace("animation_duration = 0.3", "animation_duration = 0.2")
        );
        assert_eq!(Config::read(&path).unwrap().settings.animation_duration, 0.2);
    }

    #[test]
    fn unread_and_dropped_replies_do_not_stall_actor() {
        let (reactor_tx, _updates) = actor::channel();
        let config = Config::default();
        let config_tx = ConfigActor::spawn(config.clone(), reactor_tx);
        let (response, unread) = sync_channel(1);
        config_tx.try_send(Event::QueryConfig(response)).unwrap();
        let (response, dropped) = sync_channel(1);
        drop(dropped);
        config_tx
            .try_send(Event::ApplyConfig {
                cmd: ConfigCommand::GetConfig,
                response,
            })
            .unwrap();
        let (response, result) = sync_channel(1);
        config_tx.try_send(Event::QueryConfig(response)).unwrap();
        assert_eq!(
            serde_json::to_value(result.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap(),
            serde_json::to_value(&config).unwrap()
        );
        assert_eq!(
            serde_json::to_value(unread.recv().unwrap()).unwrap(),
            serde_json::to_value(&config).unwrap()
        );
    }
}
