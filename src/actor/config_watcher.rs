use std::path::PathBuf;
use std::thread;
use std::time::{Duration};

use notify::{Config as NotifyConfig, Event, EventKind, PollWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, warn};

use crate::actor::config::{self as config_actor, Event as ConfigEvent};
use crate::common::config::{self, ConfigCommand};

pub struct ConfigWatcher {
    file: PathBuf,
    config_tx: config_actor::Sender,
}

impl ConfigWatcher {
    pub fn spawn(config_tx: config_actor::Sender) {
        thread::Builder::new()
            .name("config-watcher".to_string())
            .spawn(move || {
                let actor = ConfigWatcher {
                    file: config::config_file(),
                    config_tx,
                };
                crate::sys::executor::Executor::run(async move {
                    if let Err(e) = actor.run().await {
                        warn!("config-watcher: error: {e:?}");
                    }
                })
            })
            .expect("failed to spawn config-watcher thread");
    }

    async fn run(self) -> notify::Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();

        let mut watcher = PollWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            NotifyConfig::default()
                .with_poll_interval(Duration::from_secs(1))
                .with_compare_contents(true),
        )?;

        watcher.watch(&self.file, RecursiveMode::NonRecursive)?;

        info!("watching {:?}", self.file);

        loop {
            match rx.recv().await {
                Some(Ok(event)) => {
                    if self.is_relevant(&event) {
                        debug!("change detected: {:?}", event.kind);
                        self.request_reload();
                    } else {
                        debug!("ignoring unrelated event: {:?}", event.kind);
                    }
                }
                Some(Err(e)) => {
                    warn!("watch error: {e:?}");
                }
                None => {
                    warn!("channel closed, exiting");
                    break;
                }
            }
        }

        Ok(())
    }

    fn is_relevant(&self, event: &Event) -> bool {
        match event.kind {
            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => event
                .paths
                .iter()
                .any(|p| p == &self.file || p.file_name() == self.file.file_name()),
            _ => false,
        }
    }

    fn request_reload(&self) {
        info!("requesting config reload");
        let (tx, _) = r#continue::continuation();

        let msg = ConfigEvent::ApplyConfig {
            cmd: ConfigCommand::ReloadConfig,
            response: tx,
        };

        self.config_tx.send(msg);
    }
}
