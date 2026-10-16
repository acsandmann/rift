use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;
use std::{fs, thread};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, warn};

use crate::actor::config::{self as config_actor, Event as ConfigEvent};
use crate::common::config::{self, ConfigCommand};

const DEBOUNCE: Duration = Duration::from_millis(250);

pub struct ConfigWatcher {
    file: PathBuf,
    real_file: Option<PathBuf>,
    real_file_id: Option<(u64, u64)>,
    parents: HashSet<PathBuf>,
    last_text: Option<String>,
    config_tx: config_actor::Sender,
}

impl ConfigWatcher {
    pub fn spawn(config_tx: config_actor::Sender, config_path: PathBuf) {
        thread::Builder::new()
            .name("config-watcher".to_string())
            .spawn(move || {
                if let Err(e) = Self::run(config_tx, config_path) {
                    warn!("config-watcher: error: {e:?}");
                }
            })
            .expect("failed to spawn config-watcher thread");
    }

    fn watch(
        config_tx: config_actor::Sender,
        file: PathBuf,
    ) -> notify::Result<(Self, RecommendedWatcher, Receiver<Event>)> {
        // FSEvents reports absolute paths; preserve the final symlink itself.
        let file = file
            .parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .zip(file.file_name())
            .map(|(parent, name)| parent.join(name))
            .unwrap_or(file);
        let mut actor = Self {
            file,
            real_file: None,
            real_file_id: None,
            parents: HashSet::new(),
            last_text: None,
            config_tx,
        };
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |event| match event {
            Ok(event) => {
                let _ = tx.send(event);
            }
            Err(error) => warn!("config-watcher: notification error: {error}"),
        })?;
        actor.refresh_watches(&mut watcher)?;
        Ok((actor, watcher, rx))
    }

    fn run(config_tx: config_actor::Sender, file: PathBuf) -> notify::Result<()> {
        let (mut actor, mut watcher, rx) = Self::watch(config_tx, file)?;
        while let Ok(event) = rx.recv() {
            if !actor.is_relevant(&event) {
                continue;
            }
            actor.settle(&rx);
            // Watch parents rather than files: atomic saves replace the inode, and a
            // deleted file must remain discoverable. Retargeted symlinks need new parents.
            // A failed refresh must not end hot reload; keep the existing watches.
            if let Err(error) = actor.refresh_watches(&mut watcher) {
                warn!("config-watcher: failed to refresh watches: {error}");
            }
            actor.reload_if_changed();
            if actor.config_tx.is_closed() {
                break;
            }
        }
        Ok(())
    }

    fn settle(&self, rx: &Receiver<Event>) {
        let mut deadline = std::time::Instant::now() + DEBOUNCE;
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                Ok(event) if self.is_relevant(&event) => {
                    deadline = std::time::Instant::now() + DEBOUNCE;
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn refresh_watches(&mut self, watcher: &mut dyn Watcher) -> notify::Result<()> {
        // Keep the last target when it disappears so recreation is still observed.
        let real_file = fs::canonicalize(&self.file).ok().or_else(|| self.real_file.clone());
        let parents: HashSet<_> = self
            .file
            .parent()
            .into_iter()
            .chain(real_file.as_deref().and_then(|p| p.parent()))
            .map(PathBuf::from)
            .collect();
        for dir in parents.difference(&self.parents) {
            watcher.watch(dir, RecursiveMode::NonRecursive)?;
            info!("watching {:?}", dir);
        }
        for dir in self.parents.difference(&parents) {
            watcher.unwatch(dir)?;
        }
        self.parents = parents;
        self.real_file_id = real_file
            .as_ref()
            .and_then(|p| fs::metadata(p).ok())
            .map(|m| (m.dev(), m.ino()));
        self.real_file = real_file;
        Ok(())
    }

    fn is_relevant(&self, event: &Event) -> bool {
        event.paths.iter().any(|path| {
            path == &self.file
                || self.real_file.as_ref() == Some(path)
                || self.parents.contains(path)
                || self
                    .real_file_id
                    .is_some_and(|id| fs::metadata(path).is_ok_and(|m| (m.dev(), m.ino()) == id))
        })
    }

    fn reload_if_changed(&mut self) {
        let text = match fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(error) => {
                // Recreation with the same bytes still deserves a fresh attempt.
                self.last_text = None;
                warn!("Failed to read config file: {error}");
                return;
            }
        };
        if self.last_text.as_ref() == Some(&text) {
            return;
        }
        // Query the actor's authoritative setting, including changes made through Settings.
        let Ok(current) = self.query_config() else { return };
        let text = self.last_text.insert(text);
        if !current.settings.hot_reload {
            match config::Config::parse(text) {
                Ok(new_cfg) if new_cfg.keys != current.keys => {}
                Ok(_) => return,
                Err(error) => {
                    warn!("Failed to parse config file for diff check: {error}");
                    return;
                }
            }
        }
        if let Err(error) = self.request_reload() {
            self.last_text = None;
            warn!("Failed to reload config: {error}");
        } else {
            debug!("config reloaded successfully");
        }
    }

    fn request_reload(&self) -> Result<(), String> {
        info!("requesting config reload");
        let (response, result) = std::sync::mpsc::sync_channel(1);
        self.config_tx
            .try_send(ConfigEvent::ApplyConfig {
                cmd: ConfigCommand::ReloadConfig,
                response,
            })
            .map_err(|_| "Config actor unavailable".to_string())?;
        result.recv().map_err(|_| "Config actor stopped before replying".to_string())?
    }

    fn query_config(&self) -> Result<config::Config, ()> {
        let (response, result) = std::sync::mpsc::sync_channel(1);
        self.config_tx.try_send(ConfigEvent::QueryConfig(response)).map_err(|_| ())?;
        result.recv().map_err(|_| ())
    }
}
