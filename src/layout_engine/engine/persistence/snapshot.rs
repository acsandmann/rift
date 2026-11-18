use std::collections::btree_map::Entry as BTreeEntry;
use std::collections::hash_map::Entry;

use super::*;

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 5;

fn legacy_schema_version() -> u32 { 0 }

/// Pre-consolidation file representation. It is normalized before any runtime use.
#[derive(Deserialize, Default)]
pub(super) struct LegacyWorkspaceLayouts {
    map: HashMap<
        (SpaceId, VirtualWorkspaceId),
        crate::layout_engine::workspaces::WorkspaceLayoutState,
    >,
}

/// Saved identity evidence. Files keep these flattened at the top level.
#[derive(Serialize, Deserialize, Default)]
pub(super) struct PersistedIdentities {
    #[serde(default, rename = "persisted_windows")]
    windows: BTreeMap<WindowId, WindowFingerprint>,
    /// Native space active when an explicit save was requested.
    ///
    /// A layout file contains every initialized native space. This hint makes a portable file's
    /// intended source unambiguous when its SpaceIds do not exist in the current session.
    #[serde(default)]
    saved_active_space: Option<u64>,
}

/// Owned, versioned representation of the layout file, and the validated snapshot a restore works
/// from once `parse` has returned it.
///
/// Keep this type independent from runtime-only `LayoutEngine` fields. Adding an engine cache,
/// service, or transient index must never silently alter the persistence schema again.
#[derive(Deserialize)]
pub(super) struct PersistedLayout {
    #[serde(default = "legacy_schema_version")]
    pub(super) schema_version: u32,
    #[serde(default)]
    workspace_layouts: LegacyWorkspaceLayouts,
    floating: FloatingManager,
    floating_positions: Box<ron::value::RawValue>,
    #[serde(rename = "virtual_workspace_manager")]
    pub(super) workspaces: WorkspaceStore,
    #[serde(default)]
    space_display_map: HashMap<SpaceId, Option<String>>,
    #[serde(default)]
    pub(super) display_last_space: HashMap<String, SpaceId>,
    #[serde(flatten)]
    identities: PersistedIdentities,
    /// Every saved window, each at exactly one location. Filled by `parse`.
    #[serde(skip)]
    pub(super) pending: HashMap<WindowId, SavedWindow>,
}

/// Borrowed serialization view, avoiding a deep clone of every layout tree during save.
#[derive(Serialize)]
struct PersistedLayoutRef<'a> {
    schema_version: u32,
    floating: &'a FloatingManager,
    floating_positions: &'a FloatingPositionStore,
    #[serde(rename = "virtual_workspace_manager")]
    workspaces: &'a WorkspaceStore,
    space_display_map: &'a HashMap<SpaceId, Option<String>>,
    display_last_space: &'a HashMap<String, SpaceId>,
    #[serde(flatten)]
    identities: &'a PersistedIdentities,
}

impl PersistedLayout {
    pub(super) fn saved_active_space(&self) -> Option<SpaceId> {
        self.identities.saved_active_space.map(SpaceId::new)
    }

    /// Read and fully validate a layout file. Whatever fails here has touched no live state.
    pub(super) fn parse(buf: &str) -> anyhow::Result<Self> {
        let mut persisted: Self = match ron::from_str(buf) {
            Ok(persisted) => persisted,
            Err(original_error) => {
                let Some(migrated) = migrate_legacy_layout_system_tags(buf) else {
                    return Err(original_error.into());
                };
                ron::from_str(&migrated).map_err(|migration_error| {
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
        persisted.workspaces.migrate_legacy_activation();
        persisted
            .workspaces
            .validate_persisted_topology()
            .map_err(|error| anyhow::anyhow!("invalid workspace topology: {error}"))?;
        // Unknown preview-only metadata (including display_fallback) is ignored
        // by serde. Keep its workspace identity rather than guessing whether it
        // has since become intentional user state.
        persisted.workspaces.normalize_loaded_names();
        persisted.migrate_workspace_layouts()?;
        if let Some((window, _)) = persisted.identities.windows.iter().find(|(_, saved)| {
            !saved.width.is_finite()
                || !saved.height.is_finite()
                || saved.width < 0.0
                || saved.height < 0.0
        }) {
            anyhow::bail!("window {window:?} has an invalid persisted size");
        }
        let positions = persisted.migrate_floating_positions()?;
        positions
            .validate_persisted(&persisted.workspaces)
            .map_err(|error| anyhow::anyhow!("invalid floating positions: {error}"))?;
        persisted.resolve_locations(positions);
        Ok(persisted)
    }

    fn migrate_workspace_layouts(&mut self) -> anyhow::Result<()> {
        for ((space, id), state) in std::mem::take(&mut self.workspace_layouts.map) {
            let workspace = self.workspaces.workspaces.get_mut(id).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid workspace layouts: layout state references missing workspace {id:?}"
                )
            })?;
            if workspace.space != space {
                return Err(anyhow::anyhow!(
                    "invalid workspace layouts: workspace {id:?} is stored under the wrong native space"
                ));
            }
            if self.schema_version >= 3 {
                return Err(anyhow::anyhow!(
                    "invalid workspace layouts: legacy layout state in schema 3"
                ));
            }
            workspace.layout_state = state;
        }
        self.workspaces
            .validate_layouts()
            .map_err(|error| anyhow::anyhow!("invalid workspace layouts: {error}"))
    }

    fn migrate_floating_positions(&self) -> anyhow::Result<FloatingPositionStore> {
        if self.schema_version >= 4 {
            return Ok(self.floating_positions.into_rust::<FloatingPositionStore>()?);
        }
        // Versionless files produced by current tools also remain readable.
        if let Ok(current) = self.floating_positions.into_rust::<FloatingPositionStore>() {
            return Ok(current);
        }
        // Validate legacy native keys before discarding the redundant ownership dimension.
        #[serde_with::serde_as]
        #[derive(Deserialize)]
        struct LegacyPositions {
            #[serde_as(as = "HashMap<_, crate::sys::geometry::CGRectDef>")]
            positions: HashMap<(SpaceId, VirtualWorkspaceId, WindowId), CGRect>,
        }
        let legacy = self.floating_positions.into_rust::<LegacyPositions>()?;
        let mut positions = FloatingPositionStore::default();
        for ((space, workspace, window), frame) in legacy.positions {
            if self
                .workspaces
                .workspaces
                .get(workspace)
                .is_none_or(|owner| owner.space != space)
            {
                anyhow::bail!(
                    "invalid floating positions: legacy frame is stored under the wrong native Space"
                );
            }
            positions.store(workspace, window, frame);
        }
        Ok(positions)
    }

    /// Give every saved window exactly one location in a single pass over the file, so nothing
    /// downstream traverses a tree to find one or repairs a second representation.
    ///
    /// Old snapshots can represent a window as both tiled and floating, or in several trees. A
    /// floating marker needs a frame to agree with it; otherwise tiled ownership wins. A serialized
    /// identity without a fingerprint has no evidence with which to match a live window, and a
    /// fingerprint without a location has nowhere to restore it to: both are dropped here.
    fn resolve_locations(&mut self, positions: FloatingPositionStore) {
        let marked = self.floating.take_persisted_windows();
        let windows = &mut self.identities.windows;
        let pending = &mut self.pending;
        for (workspace, window, frame) in positions.into_sorted() {
            if marked.contains(&window)
                && let BTreeEntry::Occupied(entry) = windows.entry(window)
            {
                pending.insert(window, SavedWindow {
                    fingerprint: entry.remove(),
                    workspace,
                    frame: Some(frame),
                });
            }
        }
        for workspace in self.workspaces.catalog().collect::<Vec<_>>() {
            let entry = &mut self.workspaces[workspace];
            let members: Vec<_> = (entry.layout_state.all_layouts())
                .flat_map(|layout| entry.layout_system.all_windows_in_layout(layout))
                .collect();
            for window in members {
                let keep = match pending.entry(window) {
                    Entry::Occupied(saved) => {
                        saved.get().workspace == workspace && saved.get().frame.is_none()
                    }
                    Entry::Vacant(slot) => windows.remove(&window).is_some_and(|fingerprint| {
                        slot.insert(SavedWindow {
                            fingerprint,
                            workspace,
                            frame: None,
                        });
                        true
                    }),
                };
                if !keep {
                    entry.layout_system.remove_window(window);
                }
            }
            if entry
                .last_focused()
                .is_some_and(|window| pending.get(&window).is_none_or(|s| s.workspace != workspace))
            {
                entry.set_last_focused(None);
            }
        }
        if self
            .floating
            .last_focus()
            .is_some_and(|window| pending.get(&window).is_none_or(|saved| saved.frame.is_none()))
        {
            self.floating.set_last_focus(None);
        }
    }

    pub(super) fn into_engine(self) -> LayoutEngine {
        LayoutEngine {
            scroll_boundary: None,
            native_layouts: HashMap::default(),
            native_reconcile_batch: None,
            evicted: HashMap::default(),
            floating: self.floating,
            floating_positions: FloatingPositionStore::default(),
            app_rules: AppRuleEngine::default(),
            focused_window: None,
            window_layout_constraints: HashMap::default(),
            app_min_sizes: HashMap::default(),
            workspaces: self.workspaces,
            layout_settings: LayoutSettings::default(),
            broadcast_tx: None,
            space_display_map: self.space_display_map,
            display_last_space: self.display_last_space,
            persistence: PersistenceState {
                pending: self.pending,
                ..PersistenceState::default()
            },
        }
    }
}

impl LayoutEngine {
    /// Serialize one coherent snapshot. This observes the engine: identities come from the window
    /// store, and pending windows keep their saved evidence until they are matched or discarded.
    pub(super) fn snapshot_string(
        &self,
        window_store: &WindowStore,
        active_space: Option<SpaceId>,
    ) -> String {
        let mut floating = FloatingPositionStore::default();
        let mut identities = PersistedIdentities {
            // Never write an origin hint that has no corresponding saved layout. A stale
            // native-space observation is worse than no hint because it makes a portable file look
            // unambiguous.
            saved_active_space: active_space
                .filter(|space| self.workspaces.layout_spaces().contains(space))
                .map(|space| space.get()),
            ..PersistedIdentities::default()
        };
        for (&window, saved) in &self.persistence.pending {
            identities.windows.insert(window, saved.fingerprint.clone());
            if let Some(frame) = saved.frame {
                floating.store(saved.workspace, window, frame);
            }
        }
        for (window, state) in window_store.iter_windows() {
            // An unassigned window has no restorable location, and tiled windows carry no frame.
            let Some(workspace) = window_store.workspace_for_window(window) else {
                continue;
            };
            identities.windows.insert(window, WindowFingerprint::of(window, state));
            if self.floating.is_floating(window) {
                // A hidden window's live frame is its parked position, not its placement.
                let stored = self.floating_positions.get(workspace, window).filter(|_| {
                    self.workspaces.active_workspace(self.workspaces[workspace].space)
                        != Some(workspace)
                });
                floating.store(workspace, window, stored.unwrap_or(state.frame_monotonic));
            }
        }
        ron::ser::to_string(&PersistedLayoutRef {
            schema_version: CURRENT_SCHEMA_VERSION,
            floating: &self.floating.persisted_view(floating.positioned_windows()),
            floating_positions: &floating,
            workspaces: &self.workspaces,
            space_display_map: &self.space_display_map,
            display_last_space: &self.display_last_space,
            identities: &identities,
        })
        .expect("persisted layout serialization must support all engine layout state")
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
