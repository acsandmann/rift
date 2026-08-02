use objc2_core_foundation::CGRect;

use super::reconcile::ReconcileOutcome;
use super::*;
use crate::layout_engine::workspaces::WorkspaceLayoutSnapshot;
use crate::model::VirtualWorkspace;

#[derive(Clone, Copy)]
struct WorkspaceMapping {
    source_space: SpaceId,
    source_workspace: VirtualWorkspaceId,
    target_space: SpaceId,
    target_workspace: VirtualWorkspaceId,
}

/// Complete state for one workspace replacement. Construction is fallible; installation is not.
struct WorkspaceRestoreState {
    target_space: SpaceId,
    target_workspace: VirtualWorkspaceId,
    workspace: VirtualWorkspace,
    layout: WorkspaceLayoutSnapshot,
    floating_positions: Vec<(WindowId, CGRect)>,
    floating_windows: HashSet<WindowId>,
    replaced_windows: HashSet<WindowId>,
}

/// Validated restore transaction. If this value exists, applying it cannot discover a missing
/// workspace halfway through and leave a partially restored engine behind.
struct RestorePlan {
    request: RestoreRequest,
    workspaces: Vec<WorkspaceRestoreState>,
    target_active: Option<VirtualWorkspaceId>,
    fingerprints: Vec<(WindowId, WindowFingerprint)>,
}

impl RestorePlan {
    fn source_space(
        snapshot: &LayoutEngine,
        engine: &LayoutEngine,
        request: RestoreRequest,
    ) -> anyhow::Result<SpaceId> {
        let saved_spaces = snapshot.workspace_layouts.spaces();
        let saved_active = snapshot
            .persistence
            .saved_active_space
            .map(SpaceId::new)
            .filter(|space| saved_spaces.contains(space));
        let preferred = match request.source {
            RestoreSource::SavedActiveSpace => saved_active.or_else(|| {
                saved_spaces.contains(&request.active_space).then_some(request.active_space)
            }),
            RestoreSource::CurrentSpace => saved_spaces
                .contains(&request.active_space)
                .then_some(request.active_space)
                .or_else(|| {
                    // The disk master can still contain the previous launch's SpaceIds even
                    // though startup already repaired the live engine. Display identity bridges
                    // that interval until the next master save.
                    engine
                        .display_uuid_for_space(request.active_space)
                        .and_then(|display| snapshot.display_last_space.get(&display).copied())
                        .filter(|space| saved_spaces.contains(space))
                }),
        };

        if let Some(space) = preferred {
            return Ok(space);
        }
        if saved_spaces.is_empty() {
            Err(anyhow::anyhow!("saved layout contains no macOS spaces"))
        } else if saved_spaces.len() == 1 {
            Ok(*saved_spaces.first().expect("one saved space"))
        } else {
            Err(anyhow::anyhow!(
                "cannot choose a source from {} saved macOS spaces; save the layout again to record its active space",
                saved_spaces.len()
            ))
        }
    }

    fn build(
        mut snapshot: LayoutEngine,
        engine: &LayoutEngine,
        window_store: &WindowStore,
        request: RestoreRequest,
    ) -> anyhow::Result<Self> {
        let source_space = Self::source_space(&snapshot, engine, request)?;
        let source_active = snapshot.virtual_workspace_manager.active_workspace(source_space);
        let mappings = match request.scope {
            RestoreScope::Space => {
                let source = snapshot.virtual_workspace_manager.existing_workspaces(source_space);
                let target =
                    engine.virtual_workspace_manager.existing_workspaces(request.active_space);
                // A space restore is all-or-nothing. `zip` silently truncates, which would split
                // layout ownership between the snapshot and the pre-restore state.
                if source.len() != target.len() {
                    return Err(anyhow::anyhow!(
                        "saved and current spaces have different workspace counts"
                    ));
                }
                source
                    .into_iter()
                    .zip(target)
                    .map(
                        |((source_workspace, _), (target_workspace, _))| WorkspaceMapping {
                            source_space,
                            source_workspace,
                            target_space: request.active_space,
                            target_workspace,
                        },
                    )
                    .collect::<Vec<_>>()
            }
            RestoreScope::Workspace => {
                let target_workspace = engine
                    .virtual_workspace_manager
                    .active_workspace(request.active_space)
                    .ok_or_else(|| anyhow::anyhow!("current space has no active workspace"))?;
                let source_workspace = match request.source {
                    // A portable workspace file represents what was active when it was saved.
                    RestoreSource::SavedActiveSpace => source_active
                        .ok_or_else(|| anyhow::anyhow!("saved space has no active workspace"))?,
                    // A master file represents the complete workspace catalog. Restoring S must
                    // therefore read S's ordinal from the saved native space, regardless of which
                    // workspace happened to be active when Rift last quit.
                    RestoreSource::CurrentSpace => {
                        let target_workspaces = engine
                            .virtual_workspace_manager
                            .existing_workspaces(request.active_space);
                        let target_index = target_workspaces
                            .iter()
                            .position(|(workspace, _)| *workspace == target_workspace)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "current active workspace is missing from its native space"
                                )
                            })?;
                        snapshot
                            .virtual_workspace_manager
                            .existing_workspaces(source_space)
                            .get(target_index)
                            .map(|(workspace, _)| *workspace)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "saved space has no workspace at target index {target_index}"
                                )
                            })?
                    }
                };
                vec![WorkspaceMapping {
                    source_space,
                    source_workspace,
                    target_space: request.active_space,
                    target_workspace,
                }]
            }
        };

        // Validate every source before consuming any snapshot state. This is the transaction
        // boundary: all fallible structural checks belong above it.
        for mapping in &mappings {
            if !snapshot
                .virtual_workspace_manager
                .workspaces
                .contains_key(mapping.source_workspace)
                || !snapshot
                    .workspace_layouts
                    .contains_workspace(mapping.source_space, mapping.source_workspace)
            {
                return Err(anyhow::anyhow!("saved workspace layout is incomplete"));
            }
        }

        // Resolve every candidate location while the snapshot is still intact. The extraction
        // loop below consumes source workspaces; querying locations after that point either omits
        // later candidates or indexes a workspace that has already been removed.
        let restored_locations: HashMap<_, _> = snapshot
            .persistence
            .windows
            .keys()
            .map(|window| (*window, snapshot.restored_locations_for_window(*window)))
            .collect();
        let scoped_windows: HashSet<_> = restored_locations
            .iter()
            .filter_map(|(window, locations)| {
                locations
                    .iter()
                    .copied()
                    .any(|location| {
                        mappings.iter().any(|mapping| {
                            location == (mapping.source_space, mapping.source_workspace)
                        })
                    })
                    .then_some(*window)
            })
            .collect();
        let fingerprints = scoped_windows
            .iter()
            .filter_map(|window| {
                snapshot
                    .persistence
                    .windows
                    .get(window)
                    .cloned()
                    .map(|fingerprint| (*window, fingerprint))
            })
            .collect();

        let mut target_active = None;
        let mut workspaces = Vec::with_capacity(mappings.len());
        for mapping in mappings {
            let source_location = (mapping.source_space, mapping.source_workspace);
            let workspace_windows: HashSet<_> = scoped_windows
                .iter()
                .copied()
                .filter(|window| {
                    restored_locations
                        .get(window)
                        .is_some_and(|locations| locations.contains(&source_location))
                })
                .collect();
            let floating_windows = workspace_windows
                .iter()
                .copied()
                .filter(|window| snapshot.floating.is_floating(*window))
                .collect();
            let floating_positions = snapshot
                .floating_positions
                .workspace_positions(mapping.source_space, mapping.source_workspace);
            let mut replaced_windows: HashSet<_> = engine
                .floating_positions
                .workspace_positions(mapping.target_space, mapping.target_workspace)
                .into_iter()
                .map(|(window, _)| window)
                .collect();
            for (space, workspace, layout) in engine.workspace_layouts.all_layouts() {
                if (space, workspace) == (mapping.target_space, mapping.target_workspace) {
                    replaced_windows.extend(
                        engine
                            .workspace_tree(mapping.target_workspace)
                            .all_windows_in_layout(layout),
                    );
                }
            }
            replaced_windows.extend(
                window_store.workspace_windows(mapping.target_space, mapping.target_workspace),
            );
            let mut workspace = snapshot
                .virtual_workspace_manager
                .workspaces
                .remove(mapping.source_workspace)
                .expect("workspace sources were validated before extraction");
            // A restore replaces layout contents, not the target workspace's identity. Names are
            // configured/current-session metadata and must not be copied from another workspace or
            // from an old master snapshot.
            workspace.name = engine
                .virtual_workspace_manager
                .workspace_info(mapping.target_space, mapping.target_workspace)
                .expect("target workspace was validated before extraction")
                .name
                .clone();
            workspace.space = mapping.target_space;
            let layout = snapshot
                .workspace_layouts
                .snapshot_workspace(mapping.source_space, mapping.source_workspace)
                .expect("workspace layout sources were validated before extraction");
            if source_active == Some(mapping.source_workspace) {
                target_active = Some(mapping.target_workspace);
            }
            workspaces.push(WorkspaceRestoreState {
                target_space: mapping.target_space,
                target_workspace: mapping.target_workspace,
                workspace,
                layout,
                floating_positions,
                floating_windows,
                replaced_windows,
            });
        }

        Ok(Self {
            request,
            workspaces,
            target_active,
            fingerprints,
        })
    }

    fn apply(
        self,
        engine: &mut LayoutEngine,
        window_store: &mut WindowStore,
        live_windows: HashMap<WindowId, WindowFingerprint>,
    ) -> RestoreReport {
        let live_ids = live_windows.keys().copied().collect::<HashSet<_>>();
        let live_floating: HashSet<_> = live_windows
            .keys()
            .copied()
            .filter(|window| engine.floating.is_floating(*window))
            .collect();
        let restored_targets = self
            .workspaces
            .iter()
            .map(|workspace| (workspace.target_space, workspace.target_workspace))
            .collect::<Vec<_>>();
        let workspaces_replaced = self.workspaces.len();
        for workspace in self.workspaces {
            engine.install_workspace_restore_state(workspace);
        }
        if self.request.scope == RestoreScope::Space
            && let Some(target_active) = self.target_active
        {
            engine
                .virtual_workspace_manager
                .active_workspace_per_space
                .insert(self.request.active_space, (None, target_active));
        }
        // Only identities imported by this transaction are restoration candidates. Deriving this
        // set from the whole live persistence map can arm unrelated windows which merely happen
        // to already occupy the target workspace.
        let restored_candidates =
            self.fingerprints.iter().map(|(window, _)| *window).collect::<Vec<_>>();
        for (window, fingerprint) in self.fingerprints {
            engine.persistence.record(window, fingerprint);
        }
        engine.persistence.replace_pending(restored_candidates);

        // A scoped restore must not consume a live identity outside its exact target. WindowId is
        // explicitly process-local and can be reused across sessions. Detach an imported
        // placeholder before matching when it collides with an out-of-scope live window or with a
        // live window whose stronger app/WindowServer identity proves it is a different window.
        // Doing this before reconciliation also allows another genuine candidate to match the live
        // window without the colliding placeholder later deleting that match during cleanup.
        let mut preempted_candidates = 0;
        for (live, fingerprint) in &live_windows {
            if !engine.persistence.pending_windows.contains(live) {
                continue;
            }
            let live_assignment = window_store.workspace_info_for_window(*live);
            let live_space = window_store
                .current_window_server_space_for_window(*live)
                .or_else(|| live_assignment.map(|assignment| assignment.space));
            // WorkspaceStore is the authoritative Rift ownership record. WindowServer space can
            // briefly lag or lead during native-space transitions, so it must not make an
            // externally assigned window eligible for a restore transaction.
            let live_is_in_scope = live_assignment.is_some_and(|assignment| {
                restored_targets.contains(&(assignment.space, assignment.workspace_id))
            });
            let identity_is_compatible = engine
                .persistence
                .fingerprint(*live)
                .is_some_and(|saved| saved.direct_identity_compatible_with(fingerprint));
            if live_is_in_scope && identity_is_compatible {
                continue;
            }
            engine.persistence.pending_windows.remove(live);
            preempted_candidates += 1;
            for &(space, workspace) in &restored_targets {
                engine.workspace_tree_mut(workspace).remove_window(*live);
                engine.floating_positions.remove_workspace_window(space, workspace, *live);
                if engine.virtual_workspace_manager.last_focused_window(space, workspace)
                    == Some(*live)
                {
                    engine
                        .virtual_workspace_manager
                        .set_last_focused_window(space, workspace, None);
                }
            }
            if live_floating.contains(live) {
                engine.floating.add_floating(*live);
                if let Some(live_space) = live_space {
                    engine.floating.add_active(live_space, live.pid, *live);
                }
            } else {
                engine.floating.remove_floating(*live);
            }
            engine.persistence.record(*live, fingerprint.clone());
        }

        let mut report = RestoreReport {
            workspaces_replaced,
            unmatched: preempted_candidates,
            ..RestoreReport::default()
        };
        let mut ordered_live_windows = live_windows.into_iter().collect::<Vec<_>>();
        ordered_live_windows.sort_unstable_by_key(|(window, _)| *window);
        for (live, fingerprint) in ordered_live_windows {
            if !window_store.contains_window(live) {
                continue;
            }
            let live_assignment = window_store.workspace_info_for_window(live);
            let live_space = window_store
                .current_window_server_space_for_window(live)
                .or_else(|| live_assignment.map(|assignment| assignment.space))
                .unwrap_or(self.request.active_space);
            let live_is_in_scope = live_assignment.is_some_and(|assignment| {
                restored_targets.contains(&(assignment.space, assignment.workspace_id))
            });
            if !live_is_in_scope {
                continue;
            }
            let ReconcileOutcome { matched, duplicates_removed } =
                engine.reconcile_restored_window(window_store, live_space, live, &fingerprint);
            report.matched += usize::from(matched);
            report.duplicates_removed += duplicates_removed;
            if !matched {
                // A current window that is absent from the file is not part of the restore
                // transaction. Re-project it using its authoritative assignment and preserve its
                // pre-restore floating state instead of leaving a hole in the replaced workspace.
                if live_floating.contains(&live) {
                    engine.floating.add_floating(live);
                    if let (Some(workspace), Some(window)) = (
                        engine.virtual_workspace_manager.workspace_for_window(
                            window_store,
                            live_space,
                            live,
                        ),
                        window_store.window(live),
                    ) {
                        engine.floating_positions.store(
                            live_space,
                            workspace,
                            live,
                            window.frame_monotonic,
                        );
                    }
                } else {
                    engine.floating.remove_floating(live);
                }
            }
            // Reassert the projection for both matched and unmatched live windows. For matched
            // floating windows this rebuilds the runtime-only active-floating index; for tiled
            // windows the operation is idempotent when reconciliation already replaced the node.
            engine.add_window_to_layout(window_store, live_space, live);
            if engine.focused_window == Some(live)
                && let Some(workspace) = engine.virtual_workspace_manager.workspace_for_window(
                    window_store,
                    live_space,
                    live,
                )
            {
                engine.virtual_workspace_manager.set_last_focused_window(
                    live_space,
                    workspace,
                    Some(live),
                );
            }
            engine.persistence.record(live, fingerprint);
        }
        let still_pending = engine.persistence.pending_len();
        report.unmatched += still_pending;
        let ignored = engine.discard_all_unmatched_candidates();
        debug_assert_eq!(ignored, still_pending);
        engine.normalize_restored_targets(window_store, &live_ids, &restored_targets);
        if report.unmatched > 0 {
            report.warnings.push(RestoreWarning::UnmatchedWindows(report.unmatched));
        }
        report
    }
}

impl LayoutEngine {
    /// Enforce the post-restore ownership boundary independently of matcher behavior. Every
    /// projection in a replaced workspace must represent a currently live window assigned to
    /// that exact workspace, and its tiled/floating representation must agree with runtime state.
    fn normalize_restored_targets(
        &mut self,
        window_store: &WindowStore,
        live_windows: &HashSet<WindowId>,
        targets: &[(SpaceId, VirtualWorkspaceId)],
    ) {
        for &(space, workspace) in targets {
            let expected_assignment =
                crate::model::window_store::WindowWorkspaceInfo { space, workspace_id: workspace };
            let tiled_windows = self
                .workspace_layouts
                .all_layouts()
                .into_iter()
                .filter(|(candidate_space, candidate_workspace, _)| {
                    (*candidate_space, *candidate_workspace) == (space, workspace)
                })
                .flat_map(|(_, _, layout)| {
                    self.workspace_tree(workspace).all_windows_in_layout(layout)
                })
                .collect::<HashSet<_>>();
            for window in tiled_windows {
                let valid = live_windows.contains(&window)
                    && window_store.workspace_info_for_window(window) == Some(expected_assignment)
                    && !self.floating.is_floating(window);
                if !valid {
                    self.workspace_tree_mut(workspace).remove_window(window);
                }
            }

            let positioned_windows = self
                .floating_positions
                .workspace_positions(space, workspace)
                .into_iter()
                .map(|(window, _)| window)
                .collect::<Vec<_>>();
            for window in positioned_windows {
                let valid = live_windows.contains(&window)
                    && window_store.workspace_info_for_window(window) == Some(expected_assignment)
                    && self.floating.is_floating(window);
                if !valid {
                    self.floating_positions.remove_workspace_window(space, workspace, window);
                }
            }

            if self
                .virtual_workspace_manager
                .last_focused_window(space, workspace)
                .is_some_and(|window| {
                    !live_windows.contains(&window)
                        || window_store.workspace_info_for_window(window)
                            != Some(expected_assignment)
                })
            {
                self.virtual_workspace_manager.set_last_focused_window(space, workspace, None);
            }
        }

        // Remove identity-only residue after all target projections have been normalized. A live
        // window may legitimately remain floating in an out-of-scope workspace, so global state
        // is touched only for identities which have neither a live window nor any saved location.
        let locationless = self
            .persistence
            .windows
            .keys()
            .copied()
            .filter(|window| {
                !live_windows.contains(window)
                    && self.restored_locations_for_window(*window).is_empty()
            })
            .collect::<Vec<_>>();
        for window in locationless {
            self.floating.remove_floating(window);
            self.persistence.forget_window(window);
        }
    }

    pub fn restore_layout(
        &mut self,
        path: PathBuf,
        request: RestoreRequest,
        window_store: &mut WindowStore,
        _virtual_workspace_config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) -> anyhow::Result<RestoreReport> {
        let (mut snapshot, schema_version) = Self::load_with_schema_version(&path)?;
        tracing::info!(
            path = %path.display(),
            schema_version,
            scope = ?request.scope,
            active_space = ?request.active_space,
            "Loading persisted layout for restore"
        );
        // The source topology is file data, not a live engine to be reconciled with the current
        // workspace-count setting. Only refresh layout-system settings; the installed workspace
        // inherits the already-hydrated target manager's runtime configuration.
        snapshot.set_layout_settings(layout_settings);
        self.refresh_window_fingerprints(window_store);
        let live_windows = self.persistence.live_fingerprints();
        let plan = RestorePlan::build(snapshot, self, window_store, request)?;
        let report = plan.apply(self, window_store, live_windows);
        tracing::info!(
            path = %path.display(),
            schema_version,
            scope = ?request.scope,
            workspaces_replaced = report.workspaces_replaced,
            windows_matched = report.matched,
            windows_unmatched = report.unmatched,
            duplicates_removed = report.duplicates_removed,
            "Persisted layout restore completed"
        );
        Ok(report)
    }

    /// Compatibility wrapper for callers that only need the matched-window count.
    pub fn restore_saved_layout(
        &mut self,
        path: PathBuf,
        scope: RestoreScope,
        active_space: SpaceId,
        window_store: &mut WindowStore,
        virtual_workspace_config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) -> anyhow::Result<usize> {
        self.restore_layout(
            path,
            RestoreRequest::new(scope, active_space),
            window_store,
            virtual_workspace_config,
            layout_settings,
        )
        .map(|report| report.matched)
    }

    /// Install all state participating in workspace ownership as one operation.
    fn install_workspace_restore_state(&mut self, state: WorkspaceRestoreState) {
        for window in state.replaced_windows {
            self.floating.remove_floating(window);
            self.persistence.forget_window(window);
        }
        self.virtual_workspace_manager.workspaces[state.target_workspace] = state.workspace;
        self.workspace_layouts.install_workspace_snapshot(
            state.target_space,
            state.target_workspace,
            state.layout,
        );
        self.floating_positions.replace_workspace_positions(
            state.target_space,
            state.target_workspace,
            state.floating_positions,
        );
        for window in state.floating_windows {
            self.floating.add_floating(window);
        }
    }
}
