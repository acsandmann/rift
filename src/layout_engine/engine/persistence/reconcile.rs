use super::matcher::{RestoreCandidate, choose_match};
use super::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReconcileOutcome {
    pub(super) matched: bool,
    pub(super) duplicates_removed: usize,
}

impl LayoutEngine {
    pub(super) fn discard_candidates(&mut self, discarded: Vec<WindowId>) -> usize {
        for window in &discarded {
            // Pending identities are serialized placeholders, not live windows. Once their
            // matching window set is complete, every projection must be removed together or the
            // tree/floating stores retain a ghost that participates in future layouts.
            self.remove_window_from_all_tiling_trees(*window);
            self.floating_positions.remove_window(*window);
            self.floating.remove_floating(*window);
            self.virtual_workspace_manager.forget_window_identity(*window);
            self.window_layout_constraints.remove(window);
            if self.focused_window == Some(*window) {
                self.focused_window = None;
            }
            self.persistence.remove_candidate(*window);
        }
        discarded.len()
    }

    fn discard_unmatched_candidates_matching(
        &mut self,
        mut should_discard: impl FnMut(WindowId) -> bool,
    ) -> usize {
        let discarded: Vec<_> = self
            .persistence
            .pending_windows
            .iter()
            .copied()
            .filter(|window| should_discard(*window))
            .collect();
        self.discard_candidates(discarded)
    }

    pub(super) fn discard_all_unmatched_candidates(&mut self) -> usize {
        self.discard_unmatched_candidates_matching(|_| true)
    }

    pub(crate) fn discard_unmatched_candidates_for_app(
        &mut self,
        pid: pid_t,
        app_id: Option<&str>,
        discovered_spaces: &[SpaceId],
    ) -> usize {
        let discarded = self
            .persistence
            .pending_windows
            .iter()
            .copied()
            .filter(|window| {
                (window.pid == pid
                    || app_id.is_some_and(|app_id| {
                        self.persistence
                            .fingerprint(*window)
                            .and_then(|fingerprint| fingerprint.app_id.as_deref())
                            == Some(app_id)
                    }))
                    && self
                        .restored_locations_for_window(*window)
                        .iter()
                        .any(|(space, _)| discovered_spaces.contains(space))
            })
            .collect();
        self.discard_candidates(discarded)
    }

    pub(super) fn discard_unmatchable_startup_candidates(
        &mut self,
        mut window_server_identity_exists: impl FnMut(WindowId, u32) -> bool,
        mut application_identity_exists: impl FnMut(&str) -> bool,
    ) -> usize {
        let discarded = self
            .persistence
            .pending_windows
            .iter()
            .copied()
            .filter(|window| {
                let Some(fingerprint) = self.persistence.fingerprint(*window) else {
                    return true;
                };
                let exact_window_exists = fingerprint
                    .window_server_id
                    .is_some_and(|id| window_server_identity_exists(*window, id));
                let application_exists =
                    fingerprint.app_id.as_deref().is_some_and(&mut application_identity_exists);
                !exact_window_exists && !application_exists
            })
            .collect();
        // A stale WindowServer id is not enough to reject a candidate: the same application may
        // have restarted and can still supply a title/size/app fuzzy match. Only remove candidates
        // for which neither an exact live window nor an application discovery source exists.
        self.discard_candidates(discarded)
    }

    pub(crate) fn refresh_window_fingerprints(&mut self, window_store: &WindowStore) {
        for (window_id, window) in window_store.iter_windows() {
            // WindowInfo is authoritative for the live app identity. Reusing only the old
            // fingerprint silently drops app_id for newly tracked windows, weakening restore
            // matching and allowing similarly sized windows from different apps to cross-match.
            let app_id = window.info.bundle_id.clone().or_else(|| {
                self.persistence
                    .fingerprint(window_id)
                    .and_then(|fingerprint| fingerprint.app_id.clone())
            });
            self.persistence.record(window_id, WindowFingerprint {
                window_server_id: window.info.sys_id.map(|id| id.as_u32()),
                title: (!window.info.title.trim().is_empty()).then(|| window.info.title.clone()),
                width: window.frame_monotonic.size.width,
                height: window.frame_monotonic.size.height,
                app_id,
            });
        }
    }

    pub(super) fn restored_location_for_window(
        &self,
        window: WindowId,
    ) -> Option<(SpaceId, VirtualWorkspaceId)> {
        self.restored_location_for_window_preferring(window, None)
    }

    pub(super) fn restored_locations_for_window(
        &self,
        window: WindowId,
    ) -> Vec<(SpaceId, VirtualWorkspaceId)> {
        let mut locations = Vec::new();
        // Runtime restore installs every display-size configuration. Candidate discovery must
        // inspect that same complete set; looking only at the active size lets unmatched nodes in
        // dormant configurations bypass pending cleanup and reappear after a resize.
        for (space, workspace, layout) in self.workspace_layouts.all_layouts() {
            let location = (space, workspace);
            if !locations.contains(&location)
                && self.workspace_tree(workspace).contains_window(layout, window)
            {
                locations.push(location);
            }
        }
        for space in self.workspace_layouts.spaces() {
            for (workspace, _) in self.workspace_layouts.active_layouts_for_space(space) {
                let location = (space, workspace);
                if !locations.contains(&location)
                    && self
                        .floating_positions
                        .workspace_positions(space, workspace)
                        .iter()
                        .any(|(candidate, _)| *candidate == window)
                {
                    locations.push(location);
                }
            }
        }
        locations
    }

    fn restored_location_for_window_preferring(
        &self,
        window: WindowId,
        preferred: Option<(SpaceId, VirtualWorkspaceId)>,
    ) -> Option<(SpaceId, VirtualWorkspaceId)> {
        let locations = self.restored_locations_for_window(window);
        preferred
            .filter(|preferred| locations.contains(preferred))
            .or_else(|| locations.into_iter().next())
    }

    fn remove_restored_tiling_duplicates(
        &mut self,
        window: WindowId,
        keep: (SpaceId, VirtualWorkspaceId),
    ) {
        let workspace_ids: Vec<_> = self.virtual_workspace_manager.workspaces.keys().collect();
        for workspace in workspace_ids {
            let Some(entry) = self.virtual_workspace_manager.workspaces.get_mut(workspace) else {
                continue;
            };
            if (entry.space, workspace) != keep {
                entry.layout_system.remove_window_and_rebalance_parent(window);
            }
        }
    }

    pub(super) fn reconcile_restored_window(
        &mut self,
        window_store: &mut WindowStore,
        live_space: SpaceId,
        live: WindowId,
        fingerprint: &WindowFingerprint,
    ) -> ReconcileOutcome {
        if self.persistence.pending_windows.is_empty() {
            return ReconcileOutcome::default();
        }

        let preferred_location = window_store
            .workspace_info_for_window(live)
            .map(|assignment| (assignment.space, assignment.workspace_id));
        let candidates: Vec<_> = self
            .persistence
            .pending_windows
            .iter()
            .filter_map(|window| {
                self.persistence.fingerprint(*window).map(|saved| RestoreCandidate {
                    window: *window,
                    fingerprint: saved,
                    location: self.restored_location_for_window(*window),
                })
            })
            .collect();
        let Some(decision) =
            choose_match(live, live_space, fingerprint, preferred_location, &candidates)
        else {
            return ReconcileOutcome::default();
        };
        let old = decision.selected;
        let duplicates_removed = decision.duplicate_identities.len();

        if decision.exact_identity && !decision.duplicate_identities.is_empty() {
            // A WindowServer id identifies one live window. Old snapshots produced during AX
            // rekey churn can contain several fingerprints for that id; leaving the losers pending
            // makes the next unrelated AX window steal one of their stale workspace locations.
            self.discard_candidates(decision.duplicate_identities);
        }

        // A corrupt/legacy snapshot can contain the same window identity in more than one
        // workspace tree. Prefer the live authoritative assignment when it is one of those
        // locations; otherwise hash/slotmap iteration order can silently choose another
        // workspace and a later app activation appears to move the window at random.
        let restored_location =
            self.restored_location_for_window_preferring(old, preferred_location);
        self.persistence.pending_windows.remove(&old);
        if old != live {
            self.transfer_persistent_window_identity(old, live);
            self.persistence.forget_window(old);
        }
        if let Some((space, workspace)) = restored_location {
            self.remove_restored_tiling_duplicates(live, (space, workspace));
            self.virtual_workspace_manager.retain_window_focus_location(live, workspace);
            self.floating_positions.retain_window_location(live, (space, workspace));
            let _ = self.virtual_workspace_manager.assign_window_to_workspace(
                window_store,
                space,
                live,
                workspace,
            );
        }
        ReconcileOutcome {
            matched: true,
            duplicates_removed,
        }
    }
}
