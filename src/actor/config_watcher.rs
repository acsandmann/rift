use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, thread};

use notify::RecursiveMode;
use notify_debouncer_mini::{
    DebounceEventResult, DebouncedEvent, DebouncedEventKind, new_debouncer,
};
use tracing::{debug, info, trace, warn};

use crate::actor::config::{self as config_actor, Event as ConfigEvent};
use crate::common::config::{self, ConfigCommand};

pub struct ConfigWatcher {
    file: PathBuf,
    real_file: Option<PathBuf>,
    real_file_id: Option<(u64, u64)>,
    config_tx: config_actor::Sender,
    enabled: bool,
}

impl ConfigWatcher {
    pub fn spawn(config_tx: config_actor::Sender, config: config::Config, config_path: PathBuf) {
        thread::Builder::new()
            .name("config-watcher".to_string())
            .spawn(move || {
                let file = config_path;
                let real_file = fs::canonicalize(&file).ok();

                let real_file_id = real_file
                    .as_ref()
                    .and_then(|p| fs::metadata(p).ok())
                    .map(|m| (m.dev(), m.ino()));

                let actor = ConfigWatcher {
                    file,
                    real_file,
                    real_file_id,
                    config_tx,
                    enabled: config.settings.hot_reload,
                };
                crate::sys::executor::Executor::run(async move {
                    if let Err(e) = actor.run().await {
                        warn!("config-watcher: error: {e:?}");
                    }
                })
            })
            .expect("failed to spawn config-watcher thread");
    }

    async fn run(mut self) -> notify::Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DebouncedEvent>();

        let mut debouncer =
            new_debouncer(Duration::from_millis(250), move |res: DebounceEventResult| {
                if let Ok(events) = res {
                    for e in events {
                        if e.kind == DebouncedEventKind::Any {
                            let _ = tx.send(e);
                        }
                    }
                }
            })?;

        let watcher = debouncer.watcher();

        let mut parents: HashSet<PathBuf> = HashSet::new();
        if let Some(p) = self.file.parent() {
            parents.insert(p.to_path_buf());
        }
        if let Some(real) = &self.real_file {
            if let Some(p) = real.parent() {
                parents.insert(p.to_path_buf());
            }
        }

        for dir in parents.iter() {
            watcher.watch(dir, RecursiveMode::NonRecursive)?;
            info!("watching {:?}", dir);
        }

        while let Some(event) = rx.recv().await {
            if !self.is_relevant(&event) {
                continue;
            }

            trace!("change detected (debounced): {:?} {:?}", event.kind, event.path);

            let mut should_reload = self.enabled;

            if !should_reload {
                match crate::common::config::Config::read(&self.file) {
                    Ok(new_cfg) => {
                        if let Ok(current_cfg) = self.query_config().await {
                            if new_cfg.keys != current_cfg.keys {
                                should_reload = true;
                            }
                        }
                    }
                    Err(e) => warn!("Failed to read config file for diff check: {:?}", e),
                }
            }

            if should_reload {
                if self.request_reload().await.is_ok()
                    && let Ok(new_config) = self.query_config().await
                {
                    self.enabled = new_config.settings.hot_reload;
                    debug!("config reloaded successfully");
                }
            }
        }

        Ok(())
    }

    fn is_relevant(&self, event: &DebouncedEvent) -> bool {
        if event.path == self.file {
            return true;
        }

        if let Some(real) = &self.real_file {
            if event.path == *real {
                return true;
            }

            if let Ok(ev_real) = fs::canonicalize(&event.path) {
                if ev_real == *real {
                    return true;
                }
            }

            if let Ok(meta) = fs::metadata(&event.path) {
                if let Some((dev, ino)) = self.real_file_id {
                    if meta.dev() == dev && meta.ino() == ino {
                        return true;
                    }
                }
            }
        }

        event.path.file_name().is_some_and(|n| Some(n) == self.file.file_name())
    }

    async fn request_reload(&self) -> Result<(), String> {
        info!("requesting config reload");
        let (tx, fut) = r#continue::continuation();

        let msg = ConfigEvent::ApplyConfig {
            cmd: ConfigCommand::ReloadConfig,
            response: tx,
        };

        if let Err(e) = self.config_tx.try_send(msg) {
            let tokio::sync::mpsc::error::SendError((_span, msg)) = e;
            match msg {
                ConfigEvent::ApplyConfig { response, .. } => std::mem::forget(response),
                ConfigEvent::QueryConfig(response) => std::mem::forget(response),
            }
            return Err("Config actor unavailable".to_string());
        }

        fut.await
    }

    async fn query_config(&self) -> Result<config::Config, ()> {
        let (tx, fut) = r#continue::continuation();
        let event = ConfigEvent::QueryConfig(tx);
        if let Err(e) = self.config_tx.try_send(event) {
            let tokio::sync::mpsc::error::SendError((_span, event)) = e;
            if let ConfigEvent::QueryConfig(response) = event {
                std::mem::forget(response);
            }
            return Err(());
        }
        Ok(fut.await)
    }
}
