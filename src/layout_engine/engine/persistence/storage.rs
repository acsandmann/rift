use super::snapshot::{CURRENT_SCHEMA_VERSION, PersistedLayout};
use super::*;

impl LayoutEngine {
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        Self::load_with_schema_version(&path).map(|(engine, _)| engine)
    }

    /// Load the master snapshot used for process startup and report its persisted coverage.
    /// Validation and menu previews continue to use `load` without emitting restore logs.
    pub fn load_for_startup_restore(path: PathBuf) -> anyhow::Result<Self> {
        let (mut engine, schema_version) = Self::load_with_schema_version(&path)?;
        engine.startup_restore_pending = true;
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
            native_spaces = engine.workspace_layouts.spaces().len(),
            virtual_workspaces = engine.virtual_workspace_manager.workspaces.len(),
            saved_windows = engine.persistence.windows.len(),
            restore_candidates = engine.persistence.pending_len(),
            unavailable_windows_ignored = unavailable_windows,
            "Loaded persisted layout for startup restore"
        );
        Ok(engine)
    }

    pub(super) fn load_with_schema_version(path: &Path) -> anyhow::Result<(Self, u32)> {
        let mut buf = String::new();
        File::open(path)?.read_to_string(&mut buf)?;
        Self::deserialize_from_str_with_schema_version(&buf)
    }

    pub(crate) fn deserialize_from_str(buf: &str) -> anyhow::Result<Self> {
        Self::deserialize_from_str_with_schema_version(buf).map(|(engine, _)| engine)
    }

    fn deserialize_from_str_with_schema_version(buf: &str) -> anyhow::Result<(Self, u32)> {
        let persisted = match PersistedLayout::deserialize(buf) {
            Ok(persisted) => persisted,
            Err(original_error) => {
                let Some(migrated) = migrate_legacy_layout_system_tags(buf) else {
                    return Err(original_error.into());
                };
                PersistedLayout::deserialize(&migrated).map_err(|migration_error| {
                    anyhow::anyhow!(
                        "could not parse layout file ({original_error}); compatibility migration also failed ({migration_error})"
                    )
                })?
            }
        };
        if persisted.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(anyhow::anyhow!(
                "layout schema version {} is newer than supported version {}",
                persisted.schema_version,
                CURRENT_SCHEMA_VERSION,
            ));
        }
        persisted
            .virtual_workspace_manager
            .validate_persisted_topology()
            .map_err(|error| anyhow::anyhow!("invalid workspace topology: {error}"))?;
        persisted
            .workspace_layouts
            .validate_persisted(&persisted.virtual_workspace_manager)
            .map_err(|error| anyhow::anyhow!("invalid workspace layouts: {error}"))?;
        persisted
            .floating_positions
            .validate_persisted(&persisted.virtual_workspace_manager)
            .map_err(|error| anyhow::anyhow!("invalid floating positions: {error}"))?;
        persisted.persistence.validate()?;
        let schema_version = persisted.schema_version;
        let mut engine = persisted.into_engine();
        let fingerprinted: HashSet<_> = engine.persistence.windows.keys().copied().collect();
        let mut unmatchable = HashSet::default();
        for (_, workspace, layout) in engine.workspace_layouts.all_layouts() {
            unmatchable.extend(
                engine
                    .workspace_tree(workspace)
                    .all_windows_in_layout(layout)
                    .into_iter()
                    .filter(|window| !fingerprinted.contains(window)),
            );
        }
        unmatchable.extend(
            engine
                .floating
                .persisted_windows()
                .into_iter()
                .chain(engine.floating_positions.persisted_windows())
                .filter(|window| !fingerprinted.contains(window)),
        );
        // A serialized identity without a fingerprint has no evidence with which to match a live
        // AX window. Remove every projection at the file boundary; admitting it into runtime
        // state would create a ghost that no later discovery-completion cleanup could identify.
        engine.discard_candidates(unmatchable.into_iter().collect());
        // Only fingerprints backed by saved layout state are restoration candidates. A
        // locationless fingerprint can remain briefly after scope replacement or lifecycle
        // churn; arming it would let it steal an unrelated future window despite having nowhere
        // meaningful to restore that window to.
        let pending = engine
            .persistence
            .windows
            .keys()
            .copied()
            .filter(|window| engine.restored_location_for_window(*window).is_some())
            .collect::<Vec<_>>();
        engine.persistence.replace_pending(pending);
        Ok((engine, schema_version))
    }

    pub fn save(&self, path: PathBuf) -> std::io::Result<()> {
        self.virtual_workspace_manager
            .validate_persisted_topology()
            .and_then(|_| {
                self.workspace_layouts.validate_persisted(&self.virtual_workspace_manager)
            })
            .and_then(|_| {
                self.floating_positions.validate_persisted(&self.virtual_workspace_manager)
            })
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        self.persistence
            .validate()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        if let Some(parent) = &parent {
            fs::create_dir_all(parent)?;
        }
        let serialized = self.serialize_to_string();
        let (temporary, mut file) = loop {
            let sequence = SAVE_TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
            let temporary_extension = path
                .extension()
                .map(|extension| {
                    format!(
                        "{}.{}.{}.tmp",
                        extension.to_string_lossy(),
                        std::process::id(),
                        sequence
                    )
                })
                .unwrap_or_else(|| format!("{}.{}.tmp", std::process::id(), sequence));
            let temporary = path.with_extension(temporary_extension);
            match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary) {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = (|| {
            file.write_all(serialized.as_bytes())?;
            // SaveAndExit terminates the process immediately after this returns. Flush the file
            // itself before the atomic rename so a successful shutdown save means the complete
            // snapshot has reached the filesystem, not merely userspace/kernel write buffers.
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)?;
            // The rename is the commit point. Sync its directory as well so an immediate
            // SaveAndExit cannot acknowledge a rename that is still only in filesystem metadata
            // cache.
            if let Some(parent) = &parent {
                File::open(parent)?.sync_all()?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    /// Capture live fingerprint and floating-frame inputs, then atomically save one coherent
    /// snapshot. Callers should prefer this over coordinating preparation and `save` themselves.
    pub fn save_current_layout(
        &mut self,
        path: PathBuf,
        window_store: &WindowStore,
        active_space: Option<SpaceId>,
    ) -> std::io::Result<()> {
        self.refresh_window_fingerprints(window_store);
        // Never write an origin hint that has no corresponding saved layout. A stale native-space
        // observation is worse than no hint because it makes a portable file look unambiguous.
        self.persistence.set_saved_active_space(
            active_space.filter(|space| self.workspace_layouts.spaces().contains(space)),
        );
        for (window, state) in window_store.iter_windows() {
            if self.floating.is_floating(window) {
                let Some(assignment) = window_store.workspace_info_for_window(window) else {
                    // An unassigned live window has no restorable location. Keep its fingerprint
                    // for lifecycle continuity, but never serialize a stale frame from an older
                    // assignment as if it were current.
                    self.floating_positions.remove_window(window);
                    continue;
                };
                self.floating_positions
                    .retain_window_location(window, (assignment.space, assignment.workspace_id));
                self.floating_positions.store(
                    assignment.space,
                    assignment.workspace_id,
                    window,
                    state.frame_monotonic,
                );
            } else {
                // Floating frames are type-specific state. A tiled window retaining one creates a
                // second persisted location and makes later reconciliation order-dependent.
                self.floating_positions.remove_window(window);
            }
        }
        self.save(path)
    }

    /// Reconcile startup-only native SpaceId churn using the display identity saved in the master
    /// file. Normal space switches must never call this path: a new current space on a display is
    /// ordinarily a distinct layout, not a renamed old space.
    pub fn reconcile_startup_spaces(
        &mut self,
        window_store: &mut WindowStore,
        current_spaces: &[(SpaceId, String)],
    ) {
        if !self.startup_restore_pending {
            return;
        }
        self.startup_restore_pending = false;

        let saved_spaces = self.workspace_layouts.spaces();
        let mut remaps = Vec::new();
        for (current, display_uuid) in current_spaces {
            if saved_spaces.contains(current) {
                continue;
            }
            let Some(saved) = self.display_last_space.get(display_uuid).copied() else {
                continue;
            };
            if saved != *current
                && saved_spaces.contains(&saved)
                && !remaps.iter().any(|(old, _)| *old == saved)
            {
                remaps.push((saved, *current));
            }
        }

        // Stage through unused ids so a swap/cycle cannot overwrite another display's snapshot.
        let mut next_temporary = saved_spaces
            .iter()
            .map(|space| space.get())
            .chain(current_spaces.iter().map(|(space, _)| space.get()))
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let staged = remaps
            .into_iter()
            .map(|(old, new)| {
                let temporary = SpaceId::new(next_temporary);
                next_temporary = next_temporary.saturating_add(1);
                self.remap_space(window_store, old, temporary);
                (temporary, new)
            })
            .collect::<Vec<_>>();
        for (temporary, new) in staged {
            self.remap_space(window_store, temporary, new);
        }
    }

    pub fn serialize_to_string(&self) -> String { PersistedLayout::serialize_engine(self) }

    pub fn finish_loading(
        &mut self,
        virtual_workspace_config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
        broadcast_tx: Option<BroadcastSender>,
    ) {
        self.broadcast_tx = broadcast_tx;
        self.set_layout_settings(layout_settings);
        self.app_rules = AppRuleEngine::new(&virtual_workspace_config.app_rules);
        self.virtual_workspace_manager
            .update_settings(virtual_workspace_config, layout_settings);
    }
}

pub(super) fn migrate_legacy_layout_system_tags(input: &str) -> Option<String> {
    const TAGS: [(&str, &str); 5] = [
        ("(kind:\"traditional\",", "traditional(("),
        ("(kind:\"bsp\",", "bsp(("),
        ("(kind:\"master_stack\",", "master_stack(("),
        ("(kind:\"scrolling\",", "scrolling(("),
        ("(kind:\"stack\",", "stack(("),
    ];

    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut changed = false;
    while cursor < input.len() {
        let Some((start, needle, replacement)) = TAGS
            .iter()
            .filter_map(|(needle, replacement)| {
                input[cursor..]
                    .find(needle)
                    .map(|offset| (cursor + offset, *needle, *replacement))
            })
            .min_by_key(|(start, _, _)| *start)
        else {
            output.push_str(&input[cursor..]);
            break;
        };

        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (offset, ch) in input[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        output.push_str(&input[cursor..start]);
        output.push_str(replacement);
        output.push_str(&input[start + needle.len()..end]);
        output.push_str("))");
        cursor = end + 1;
        changed = true;
    }
    changed.then_some(output)
}
