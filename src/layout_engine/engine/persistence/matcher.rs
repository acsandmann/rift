use super::*;

/// Which saved destinations a live window may adopt.
#[derive(Clone, Copy)]
pub(super) enum MatchScope<'a> {
    /// Discovery: the window's current native Space.
    Space(SpaceId),
    /// An explicit restore: exactly the workspaces it replaced.
    Workspaces(&'a [VirtualWorkspaceId]),
}

impl LayoutEngine {
    fn saved_window_in_scope(&self, saved: &SavedWindow, scope: MatchScope<'_>) -> bool {
        match scope {
            MatchScope::Workspaces(targets) => targets.contains(&saved.workspace),
            MatchScope::Space(space) => {
                let destination = self.workspaces[saved.workspace].space;
                // A shared workspace carries its windows between native Spaces, while a
                // per-display workspace is defined by the one it belongs to.
                !self.persistence.unresolved_spaces.contains(&destination)
                    && (destination == space
                        || self.workspaces.settings.mode == WorkspaceMode::Global)
            }
        }
    }

    /// The closest saved size among same-app, same-title candidates; a tie is rejected rather than
    /// assigning a slot arbitrarily. Size only disambiguates: it is layout output, so it must not
    /// veto a restarted application's unique title.
    fn fuzzy_saved_window(
        &self,
        live: &WindowFingerprint,
        scope: MatchScope<'_>,
    ) -> Option<WindowId> {
        let mut closest = None;
        let mut closest_delta = f64::INFINITY;
        let mut tied = false;
        for (&window, saved) in &self.persistence.pending {
            let fingerprint = &saved.fingerprint;
            if !fingerprint.same_known_app(live)
                || !fingerprint.same_title(live)
                || !self.saved_window_in_scope(saved, scope)
            {
                continue;
            }
            let delta = fingerprint.size_delta(live);
            if delta < closest_delta {
                closest = Some(window);
                closest_delta = delta;
                tied = false;
            } else if delta == closest_delta {
                tied = true;
            }
        }
        (!tied).then_some(closest).flatten()
    }

    /// Resolve what a live window means for pending restoration; true if it adopted a placement.
    ///
    /// Exact evidence proves its placeholder stale wherever it is: the window stays put if the
    /// destination is out of scope, and duplicates of its server id go with it. A slot under the
    /// window's own id that it did not select describes a different window it must not inherit.
    pub(super) fn reconcile_saved_window(
        &mut self,
        window_store: &mut WindowStore,
        window: WindowId,
        live: &WindowFingerprint,
        scope: MatchScope<'_>,
    ) -> bool {
        let pending = &self.persistence.pending;
        let assigned = window_store.workspace_for_window(window);
        let direct = pending
            .get(&window)
            .is_some_and(|saved| saved.fingerprint.same_process_window(live));
        let exact = direct.then_some(window).or_else(|| {
            pending
                .iter()
                .filter(|(_, saved)| saved.fingerprint.same_server_window(live))
                .max_by_key(|(candidate, saved)| {
                    (
                        self.saved_window_in_scope(saved, scope),
                        Some(saved.workspace) == assigned,
                        std::cmp::Reverse(**candidate),
                    )
                })
                .map(|(candidate, _)| *candidate)
        });
        let selected = exact.or_else(|| self.fuzzy_saved_window(live, scope));
        let stale = pending
            .iter()
            .filter(|(candidate, saved)| {
                Some(**candidate) != selected
                    && (**candidate == window
                        || (exact.is_some()
                            && live.window_server_id.is_some()
                            && saved.fingerprint.window_server_id == live.window_server_id))
            })
            .map(|(candidate, _)| *candidate)
            .collect();
        self.discard_saved_windows(stale);
        let Some(selected) = selected else {
            return false;
        };
        let saved = self.persistence.pending.remove(&selected).expect("selected window is pending");
        if self.saved_window_in_scope(&saved, scope) {
            self.adopt_saved_window(window_store, selected, window, saved);
            return true;
        }
        self.discard_saved_window(selected, saved);
        false
    }

    fn adopt_saved_window(
        &mut self,
        window_store: &mut WindowStore,
        old: WindowId,
        live: WindowId,
        saved: SavedWindow,
    ) {
        if let Some(previous) =
            window_store.workspace_for_window(live).filter(|id| *id != saved.workspace)
        {
            self.workspaces[previous].layout_system.remove_window(live);
            self.floating_positions.remove_workspace_window(previous, live);
        }
        self.transfer_persistent_window_identity(old, live);
        if let Some(frame) = saved.frame {
            self.workspaces[saved.workspace].layout_system.remove_window(live);
            self.floating.add_floating(live);
            self.floating_positions.store(saved.workspace, live, frame);
        } else {
            self.floating.remove_floating(live);
            self.floating_positions.remove_workspace_window(saved.workspace, live);
        }
        let space = self.workspaces[saved.workspace].space;
        let _ =
            self.workspaces
                .assign_window_to_workspace(window_store, space, live, saved.workspace);
    }

    pub(super) fn discard_saved_window(&mut self, window: WindowId, saved: SavedWindow) {
        let workspace = &mut self.workspaces[saved.workspace];
        if saved.frame.is_none() {
            workspace.layout_system.remove_window(window);
        }
        if workspace.last_focused() == Some(window) {
            workspace.set_last_focused(None);
        }
        if self.floating.last_focus() == Some(window) {
            self.floating.set_last_focus(None);
        }
    }

    pub(super) fn discard_saved_windows(&mut self, windows: Vec<WindowId>) -> usize {
        let count = windows.len();
        for window in windows {
            self.forget_persisted_window(window);
        }
        count
    }

    pub(crate) fn discard_unmatched_candidates_for_app(
        &mut self,
        pid: pid_t,
        app_id: Option<&str>,
        discovered_spaces: &[SpaceId],
    ) -> usize {
        if self.persistence.awaiting_topology && !self.persistence.pending.is_empty() {
            let discovery = (pid, app_id.map(str::to_owned), discovered_spaces.to_vec());
            self.persistence.awaited_discoveries.push(discovery);
        }
        if !self.is_restoring() {
            return 0;
        }
        let finished = self
            .persistence
            .pending
            .iter()
            .filter(|(window, saved)| {
                (window.pid == pid
                    || app_id.is_some_and(|id| saved.fingerprint.app_id.as_deref() == Some(id)))
                    && discovered_spaces.contains(&self.workspaces[saved.workspace].space)
            })
            .map(|(window, _)| *window)
            .collect();
        self.discard_saved_windows(finished)
    }

    /// Catch up on what happened while the topology was unknown: windows assigned then, in
    /// discovery's own terms, and then the discoveries that finished.
    pub(super) fn settle_awaited_windows(&mut self, window_store: &mut WindowStore) {
        let assigned: Vec<_> = (window_store.iter_windows())
            .filter_map(|(window, state)| {
                let workspace = window_store.workspace_for_window(window)?;
                Some((window, WindowFingerprint::of(window, state), workspace))
            })
            .collect();
        for (window, live, workspace) in assigned {
            let scope = MatchScope::Space(self.workspaces[workspace].space);
            if self.is_restoring()
                && self.reconcile_saved_window(window_store, window, &live, scope)
            {
                let owner = window_store.workspace_for_window(window).unwrap_or(workspace);
                self.add_window_to_layout(window_store, self.workspaces[owner].space, window);
            }
        }
        for (pid, app_id, spaces) in std::mem::take(&mut self.persistence.awaited_discoveries) {
            self.discard_unmatched_candidates_for_app(pid, app_id.as_deref(), &spaces);
        }
    }

    /// A stale WindowServer id is not enough to reject a candidate: the same application may have
    /// restarted and can still supply a fuzzy match. Only candidates with neither a running
    /// application nor an exact live window are removed, so the per-window query runs only for a
    /// window whose application is not running.
    pub(super) fn discard_unmatchable_startup_candidates(
        &mut self,
        mut window_server_identity_exists: impl FnMut(WindowId, u32) -> bool,
        mut application_identity_exists: impl FnMut(&str) -> bool,
    ) -> usize {
        let mut running: HashMap<&str, bool> = HashMap::default();
        let unmatchable = self
            .persistence
            .pending
            .iter()
            .filter(|(window, saved)| {
                let application_exists =
                    saved.fingerprint.app_id.as_deref().is_some_and(|app_id| {
                        *running
                            .entry(app_id)
                            .or_insert_with(|| application_identity_exists(app_id))
                    });
                !application_exists
                    && !saved
                        .fingerprint
                        .window_server_id
                        .is_some_and(|id| window_server_identity_exists(**window, id))
            })
            .map(|(window, _)| *window)
            .collect();
        self.discard_saved_windows(unmatchable)
    }
}
