use super::snapshot::PersistedLayout;
use super::*;

/// Crash-safe replacement of `path`. Returning success means the complete file reached the
/// filesystem, because SaveAndExit terminates the process immediately afterwards.
fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let (temporary, mut file) = loop {
        let sequence = SAVE_TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let suffix = format!("{}.{sequence}.tmp", std::process::id());
        let temporary = path.with_extension(match path.extension() {
            Some(extension) => format!("{}.{suffix}", extension.to_string_lossy()),
            None => suffix,
        });
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // The rename is the commit point; syncing the directory makes it durable.
        fs::rename(&temporary, path)?;
        if let Some(parent) = parent {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

impl LayoutEngine {
    /// Check that a layout file can be loaded, without building an engine from it.
    pub fn validate_file(path: PathBuf) -> anyhow::Result<()> {
        PersistedLayout::read(&path).map(|_| ())
    }

    /// Load the master snapshot used for process startup and report its persisted coverage.
    pub fn load_for_startup_restore(path: PathBuf) -> anyhow::Result<Self> {
        let snapshot = PersistedLayout::read(&path)?;
        let schema_version = snapshot.schema_version;
        let mut engine = snapshot.into_engine();
        engine.await_topology();
        let unavailable_windows = engine.discard_unmatchable_startup_candidates(
            |window, id| {
                crate::sys::window_server::get_window(
                    crate::sys::window_server::WindowServerId::new(id),
                )
                .is_some_and(|info| info.pid == window.pid)
            },
            crate::sys::app::is_bundle_running,
        );
        tracing::info!(
            path = %path.display(),
            schema_version,
            native_spaces = engine.persistence.unresolved_spaces.len(),
            virtual_workspaces = engine.workspaces.workspaces.len(),
            restore_candidates = engine.persistence.pending.len(),
            unavailable_windows_ignored = unavailable_windows,
            "Loaded persisted layout for startup restore"
        );
        Ok(engine)
    }

    /// Hold every saved destination until `resolve_saved_spaces` has proven it.
    pub(super) fn await_topology(&mut self) {
        self.persistence.awaiting_topology = true;
        self.persistence.unresolved_spaces =
            self.workspaces.workspaces.values().map(|workspace| workspace.space).collect();
    }

    pub(crate) fn deserialize_from_str(buf: &str) -> anyhow::Result<Self> {
        Ok(PersistedLayout::parse(buf)?.into_engine())
    }

    /// Capture and atomically save one coherent snapshot, leaving the engine untouched. A file
    /// that would not load is never committed.
    pub fn save_current_layout(
        &self,
        path: PathBuf,
        window_store: &WindowStore,
        active_space: Option<SpaceId>,
    ) -> std::io::Result<()> {
        // Until the topology is known, a live window can sit in a different workspace from its own
        // pending slot, so this state has no one coherent snapshot.
        if self.persistence.awaiting_topology && !self.persistence.pending.is_empty() {
            return Err(std::io::Error::other(
                "saved layout is still waiting for displays",
            ));
        }
        let serialized = self.snapshot_string(window_store, active_space);
        PersistedLayout::parse(&serialized).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?;
        write_atomically(&path, &serialized)
    }

    pub fn serialize_to_string(&self, window_store: &WindowStore) -> String {
        self.snapshot_string(window_store, None)
    }

    pub fn finish_loading(
        &mut self,
        virtual_workspace_config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
        broadcast_tx: Option<BroadcastSender>,
    ) -> Result<(), String> {
        let plan = self.workspaces.plan_settings_names(virtual_workspace_config, true)?;
        self.broadcast_tx = broadcast_tx;
        self.apply_layout_settings(layout_settings);
        self.app_rules = AppRuleEngine::new(&virtual_workspace_config.app_rules);
        self.workspaces.apply_settings(virtual_workspace_config, layout_settings, plan);
        Ok(())
    }
}

impl PersistedLayout {
    pub(super) fn read(path: &Path) -> anyhow::Result<Self> {
        let mut buf = String::new();
        File::open(path)?.read_to_string(&mut buf)?;
        Self::parse(&buf)
    }
}
