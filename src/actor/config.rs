use std::path::PathBuf;
use std::sync::mpsc::SyncSender;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::actor::{self, reactor};
use crate::common::config::{Config, ConfigCommand, ConfigDocument, ConfigSource, MAX_WORKSPACES};

pub type Sender = actor::Sender<Event>;
pub type Receiver = actor::Receiver<Event>;

/// A typed transaction executed only against the actor’s authoritative source.
pub type SourceEdit = Box<dyn FnOnce(&mut ConfigSource) -> Result<(), String> + Send>;

pub struct SourceSnapshot {
    pub revision: u64,
    pub source: ConfigSource,
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    #[serde(skip)]
    QuerySource(tokio::sync::oneshot::Sender<Result<ConfigSource, String>>),
    #[serde(skip)]
    ReloadSource(tokio::sync::oneshot::Sender<Result<SourceSnapshot, String>>),
    #[serde(skip)]
    QuerySourceSince {
        revision: Option<u64>,
        response: tokio::sync::oneshot::Sender<Result<Option<SourceSnapshot>, String>>,
    },
    #[serde(skip)]
    EditSource {
        edit: SourceEdit,
        response: tokio::sync::oneshot::Sender<Result<SourceSnapshot, String>>,
    },
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
    source_revision: u64,
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
                    source_revision: 0,
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
                Event::ReloadSource(response) => {
                    let result = self
                        .handle_config_command(ConfigCommand::ReloadConfig)
                        .and_then(|()| self.source_snapshot());
                    let _ = response.send(result);
                }
                Event::QuerySourceSince { revision, response } => {
                    let result = if revision == Some(self.source_revision) {
                        Ok(None)
                    } else {
                        self.source_snapshot().map(Some)
                    };
                    let _ = response.send(result);
                }
                Event::QuerySource(response) => {
                    let _ = response.send(self.document.source().map_err(|e| e.to_string()));
                }
                Event::EditSource { edit, response } => {
                    let result = self.edit_source(edit).map(|source| SourceSnapshot {
                        revision: self.source_revision,
                        source,
                    });
                    let _ = response.send(result);
                }
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

    fn source_snapshot(&self) -> Result<SourceSnapshot, String> {
        self.document
            .source()
            .map(|source| SourceSnapshot {
                revision: self.source_revision,
                source,
            })
            .map_err(|e| e.to_string())
    }

    fn edit_source(&mut self, edit: SourceEdit) -> Result<ConfigSource, String> {
        let mut candidate = self.document.clone();
        let mut edit_result = Ok(());
        let config = candidate
            .update(|source| edit_result = edit(source))
            .map_err(|e| e.to_string())?;
        edit_result?;
        // Persistence is part of the transaction: failed saves never publish or commit.
        candidate.save(&self.config_path).map_err(|e| e.to_string())?;
        let source = candidate.source().map_err(|e| e.to_string())?;
        self.document = candidate;
        self.config = config;
        self.source_revision += 1;
        self.reactor_tx.send(reactor::Event::ConfigUpdated(self.config.clone()));
        Ok(source)
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
        self.source_revision += 1;
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
    fn source_snapshots_skip_echoes_and_preserve_external_reload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        ConfigDocument::default().save(&path).unwrap();
        let (reactor, _updates) = actor::channel();
        let actor = ConfigActor::spawn_with_path(Config::default(), reactor, path.clone());
        let query = |revision| {
            let (response, result) = tokio::sync::oneshot::channel();
            actor.send(Event::QuerySourceSince { revision, response });
            result.blocking_recv().unwrap().unwrap()
        };
        let initial = query(None).unwrap();
        assert!(query(Some(initial.revision)).is_none());
        let (response, result) = tokio::sync::oneshot::channel();
        let animate = !initial.source.settings.animate;
        actor.send(Event::EditSource {
            edit: Box::new(move |s| {
                s.settings.animate = animate;
                Ok(())
            }),
            response,
        });
        let edited = result.blocking_recv().unwrap().unwrap();
        assert!(edited.revision > initial.revision);
        assert_eq!(edited.source.settings.animate, animate);
        assert!(
            query(Some(edited.revision)).is_none(),
            "edit echo must not return another source"
        );
        let (response, result) = tokio::sync::oneshot::channel();
        actor.send(Event::EditSource {
            edit: Box::new(|_| Err("rejected".into())),
            response,
        });
        assert!(result.blocking_recv().unwrap().is_err());
        assert!(
            query(Some(edited.revision)).is_none(),
            "failed edit must not advance the source"
        );
        let mut external = ConfigDocument::read(&path).unwrap();
        external.update(|s| s.settings.animate = !animate).unwrap();
        external.save(&path).unwrap();
        let (response, result) = tokio::sync::oneshot::channel();
        actor.send(Event::ReloadSource(response));
        let reloaded = result.blocking_recv().unwrap().unwrap();
        assert!(reloaded.revision > edited.revision);
        assert_eq!(reloaded.source.settings.animate, !animate);
        assert!(query(Some(edited.revision)).is_some());
    }

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
            source_revision: 0,
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
    fn settings_transactions_save_source_and_rollback_validation_and_io_failures() {
        use crate::actor::wm_controller::{WmCmd, WmCommand};
        use crate::common::config::{
            AppWorkspaceRule, LayoutMode, WorkspaceLayoutRule, WorkspaceSelector,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let text = "# personal settings\n[settings]\nanimate = false # retain this comment\n[virtual_workspaces]\nworkspace_names = [\"Main\", \"Code\"]\n[keys]\n";
        std::fs::write(&path, text).unwrap();
        let document = ConfigDocument::read(&path).unwrap();
        let (reactor_tx, mut updates) = actor::channel();
        let mut actor = ConfigActor {
            config: document.runtime().unwrap(),
            document,
            reactor_tx,
            config_path: path.clone(),
            source_revision: 0,
        };
        let source = actor
            .edit_source(Box::new(|s| {
                s.settings.animate = true;
                s.virtual_workspaces.workspace_names[1] = "Development".into();
                s.virtual_workspaces.workspace_rules.push(WorkspaceLayoutRule {
                    workspace: WorkspaceSelector::Index(1),
                    layout: LayoutMode::Scrolling,
                });
                s.virtual_workspaces.app_rules.push(AppWorkspaceRule {
                    app_id: Some("com.apple.Safari".into()),
                    manage: Some(false),
                    ..Default::default()
                });
                s.virtual_workspaces.app_rules.push(AppWorkspaceRule {
                    app_id: Some("com.apple.Terminal".into()),
                    ..Default::default()
                });
                s.virtual_workspaces.app_rules.swap(0, 1);
                s.binding_modes.insert(
                    "resize".into(),
                    [(
                        "Alt + R".into(),
                        WmCommand::Wm(WmCmd::BindingMode("default".into())),
                    )]
                    .into(),
                );
                s.keys.insert("Alt + H".into(), WmCommand::Wm(WmCmd::CloseWindow));
                Ok(())
            }))
            .unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.starts_with(
                "# personal settings\n[settings]\nanimate = true # retain this comment"
            )
        );
        assert!(!saved.contains("animation_duration"));
        assert_eq!(source.virtual_workspaces.workspace_names[1], "Development");
        assert_eq!(
            source.virtual_workspaces.app_rules[0].app_id.as_deref(),
            Some("com.apple.Terminal")
        );
        assert_eq!(Config::read(&path).unwrap().binding_mode_specs.len(), 2);
        assert!(updates.try_recv().is_ok());
        let document_before = actor.document.to_string();
        assert!(
            actor
                .edit_source(Box::new(|s| {
                    s.virtual_workspaces.app_rules[0].title_regex = Some("[".into());
                    Ok(())
                }))
                .is_err()
        );
        assert!(
            actor
                .edit_source(Box::new(|s| {
                    s.keys.insert("Option + H".into(), WmCommand::Wm(WmCmd::NextWorkspace));
                    Ok(())
                }))
                .is_err()
        );
        assert_eq!(actor.document.to_string(), document_before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        assert!(updates.try_recv().is_err());
        actor.config_path = directory.path().to_path_buf(); // atomic replacement of a directory must fail
        assert!(
            actor
                .edit_source(Box::new(|s| {
                    s.settings.animate = false;
                    Ok(())
                }))
                .is_err()
        );
        assert!(actor.config.settings.animate);
        assert_eq!(actor.document.to_string(), document_before);
        assert!(updates.try_recv().is_err());
        actor.config_path = path.clone();
        actor
            .edit_source(Box::new(|s| {
                s.virtual_workspaces.app_rules.remove(1);
                s.keys.remove("Alt + H");
                s.binding_modes.remove("resize");
                Ok(())
            }))
            .unwrap();
        let roundtrip = ConfigDocument::read(&path).unwrap().source().unwrap();
        assert_eq!(roundtrip.virtual_workspaces.app_rules.len(), 1);
        assert!(roundtrip.keys.is_empty());
        assert!(roundtrip.binding_modes.is_empty());
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
