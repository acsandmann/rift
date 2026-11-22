use tracing::{debug, warn};

use super::{Changes, Presentation, Reactor, RefocusState, utils};
use crate::actor::app::{AppInfo, WindowId, WindowInfo, pid_t};
use crate::layout_engine::LayoutEvent;
use crate::model::virtual_workspace::WorkspaceContext;
use crate::model::window_store::NativeFullscreenTransition;
use crate::model::{AppRuleEffects, AppRuleResult, WindowRuleContext};
use crate::sys::geometry::SameAs;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

impl Reactor {
    pub(super) fn assign_observed_window(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        context: WorkspaceContext<'_>,
        app_info: Option<&AppInfo>,
        reapply: bool,
    ) -> Option<AppRuleEffects> {
        match self.layout_manager.layout_engine.assign_window_with_app_info(
            &mut self.state.windows,
            wid,
            context,
            WindowRuleContext {
                app_bundle_id: app_info.and_then(|app| app.bundle_id.as_deref()),
                app_name: app_info.and_then(|app| app.localized_name.as_deref()),
                ..Default::default()
            },
            reapply,
        ) {
            Ok(Some(AppRuleResult::Managed(effects))) => Some(effects),
            Ok(Some(AppRuleResult::Rejected(_))) => {
                self.reconcile_window_membership(changes, wid, None);
                None
            }
            Ok(None) => None,
            Err(error) => {
                warn!(?wid, ?error, "Failed to assign window to workspace");
                None
            }
        }
    }

    /// Reconcile layout participation from the canonical window record.
    /// A placement is supplied only by evidence that can put a window in layout.
    pub(super) fn reconcile_window_membership(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        placement: Option<SpaceId>,
    ) {
        let event = if self.state.windows.is_claimed(wid) {
            return;
        } else if self.state.windows.window(wid).is_some_and(|w| w.manage_override == Some(false)) {
            let engine = &self.layout_manager.layout_engine;
            if self.state.windows.workspace_for_window(wid).is_none()
                && !engine.is_window_floating(wid)
            {
                return;
            }
            LayoutEvent::WindowRemoved(wid)
        } else if self.state.windows.is_temporarily_unavailable(wid) {
            LayoutEvent::WindowSuspended(wid)
        } else if let Some(space) = placement.filter(|_| self.state.windows.is_admitted(wid)) {
            LayoutEvent::WindowAdded(space, wid)
        } else {
            return;
        };
        self.reconcile_layout_event(changes, event);
    }

    pub(super) fn retire_window(&mut self, changes: &mut Changes, wid: WindowId) {
        let Some(record) = self.state.windows.record(wid) else {
            return;
        };
        let wsid = record.window_server_id();
        // A claim may have changed destinations while its slot stayed in the old tree.
        // With no active assignment the layout removes all retained projections.
        if let Some(space) = self
            .state
            .windows
            .remove_claim(wid)
            .and_then(|claim| claim.workspace())
            .and_then(|workspace| {
                self.layout_manager.layout_engine.workspaces().workspace_space(workspace)
            })
        {
            changes.membership_for(Some(space));
            changes.refresh_menu = true;
        }
        self.cancel_window_presentations(vec![wid]);
        self.discard_window_activity(wid, wsid);
        self.reconcile_layout_event(changes, LayoutEvent::WindowRemoved(wid));
        self.layout_manager.layout_engine.retire_window(wid);
        self.state.windows.retire_window(wid);
        self.replace_window_references(wid, None);
        changes.refresh_notifications |= wsid.is_some();
    }

    fn discard_window_activity(&mut self, wid: WindowId, obsolete_native: Option<WindowServerId>) {
        self.drag_manager.forget_window(wid);
        if let Some(wsid) = obsolete_native {
            self.transient_focus.windows.remove(&wsid);
            self.transaction_manager.remove_for_window(wsid);
        }
    }

    pub(super) fn replace_window_identity(
        &mut self,
        old: WindowId,
        replacement: WindowId,
        obsolete_native: Option<WindowServerId>,
    ) {
        self.cancel_window_presentations(vec![old]);
        self.discard_window_activity(old, obsolete_native);
        if obsolete_native.is_none()
            && let Some(id) =
                self.state.windows.record(replacement).and_then(|r| r.window_server_id())
        {
            self.transaction_manager.clear_target_for_window(id);
        }
        self.layout_manager
            .layout_engine
            .transfer_persistent_window_identity(old, replacement);
        self.replace_window_references(old, Some(replacement));
    }

    fn replace_window_references(&mut self, old: WindowId, replacement: Option<WindowId>) {
        self.main_window_tracker.replace_window(old, replacement);
        if let Some((space, window, stamp)) = self.overview_focus
            && window == old
        {
            self.overview_focus = replacement.map(|window| (space, window, stamp));
        }
        let pending = &mut self.window_inventory_manager.refocus_after_refresh;
        if pending.get(&old.pid) == Some(&old) {
            if let Some(window) = replacement {
                pending.insert(window.pid, window);
            } else {
                pending.remove(&old.pid);
            }
        }
    }

    pub(super) fn retire_app_windows(&mut self, changes: &mut Changes, pid: pid_t) {
        let windows: Vec<_> = self.state.windows.window_ids_for_pid(pid).collect();
        for &wid in &windows {
            if let Some(space) = self.assigned_space_for_window_id(wid) {
                changes.membership_for(Some(space));
                changes.arrange(Some(space), Presentation::Animated);
            }
            self.discard_window_activity(
                wid,
                self.state.windows.record(wid).and_then(|r| r.window_server_id()),
            );
            self.layout_manager.layout_engine.retire_window(wid);
        }
        self.cancel_window_presentations(windows);
        self.overview_focus = self.overview_focus.filter(|(_, window, _)| window.pid != pid);
        self.transient_focus.windows.retain(|_, transient| transient.pid != pid);
        self.state.windows.remove_windows_for_app(pid);
        self.reconcile_layout_event(changes, LayoutEvent::AppClosed(pid));
        changes.refresh_notifications = true;
    }

    pub(super) fn invalidate_window_identity(&mut self, changes: &mut Changes, wid: WindowId) {
        if self.state.windows.invalidate_ax_identity(wid) {
            self.reconcile_window_membership(changes, wid, None);
            changes.refresh_notifications = true;
        }
        changes.inventory(wid.pid);
    }

    pub(super) fn observe_created_window(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        info: WindowInfo,
        native: Option<WindowServerInfo>,
    ) {
        if let Some(native) = native {
            self.state.windows.track_window_server_info(native);
        }
        self.observe_ax_window(changes, wid, info);
        self.admit_ax_window(changes, wid);
        changes.refresh_notifications = true;
    }

    /// Update facts once. Inventory projects returns together with its other observations.
    pub(super) fn observe_ax_window(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        mut info: WindowInfo,
    ) -> (bool, bool) {
        if let Some(old) = self.state.windows.record(wid).and_then(|r| r.window_server_id())
            && Some(old) != info.sys_id
        {
            self.cancel_window_presentations(vec![wid]);
            self.discard_window_activity(wid, Some(old));
        }
        let previous = self.state.windows.reconcile_ax_identity(wid, info.sys_id);
        if let Some(previous) = previous {
            self.replace_window_identity(previous, wid, None);
        }
        let was_admitted = self.state.windows.is_admitted(wid);
        let ax_returned = self.state.windows.validate_ax_identity(wid);
        let had_target = info
            .sys_id
            .is_some_and(|id| self.transaction_manager.get_target_frame(id).is_some());
        let minimized = info.is_minimized;
        let (new, restored, geometry_changed) =
            if let Some(window) = self.state.windows.window_mut(wid) {
                let restored = window.info.is_minimized && !minimized;
                let old_frame = window.frame_monotonic;
                if !had_target && (info.frame.size.width != 0.0 || info.frame.size.height != 0.0) {
                    window.frame_monotonic = info.frame;
                }
                let geometry_changed = !old_frame.same_as(window.frame_monotonic)
                    || window.info.is_resizable != info.is_resizable
                    || window.info.min_size != info.min_size
                    || window.info.max_size != info.max_size;
                info.frame = window.info.frame;
                window.info = info;
                (false, restored, geometry_changed)
            } else {
                self.state.windows.insert_window(wid, info.into());
                (true, false, false)
            };
        if minimized || restored {
            self.state.windows.set_minimized_visibility(wid, minimized);
        }
        self.refresh_admission_facts(wid, true);
        let admitted = self.state.windows.is_admitted(wid);
        if minimized || (was_admitted && !admitted) {
            self.reconcile_window_membership(changes, wid, None);
        }
        if admitted
            && geometry_changed
            && let Some(space) = self
                .assigned_space_for_window_id(wid)
                .or_else(|| self.best_space_for_window_id(wid))
        {
            changes.arrange(Some(space), Presentation::Animated);
        }
        (
            new,
            !minimized
                && (restored
                    || ax_returned
                    || previous.is_some()
                    || (!new && admitted && !was_admitted)),
        )
    }

    pub(super) fn observe_window_minimization(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        minimized: bool,
    ) {
        let was_unavailable = self.state.windows.is_temporarily_unavailable(wid);
        // Application hiding can report minimization for a cached AX key.
        let ax_returned = !minimized && self.state.windows.validate_ax_identity(wid);
        let Some(window) = self.state.windows.window_mut(wid) else {
            return;
        };
        if !minimized && !window.info.is_minimized && !ax_returned {
            return;
        }
        window.info.is_minimized = minimized;
        self.state.windows.set_minimized_visibility(wid, minimized);
        self.refresh_admission_facts(wid, true);
        if minimized {
            self.reconcile_window_membership(changes, wid, None);
        } else {
            changes.inventory(wid.pid);
            self.admit_ax_window(changes, wid);
        }
        changes.overview_changed |=
            was_unavailable != self.state.windows.is_temporarily_unavailable(wid);
    }

    fn refresh_admission_facts(&mut self, wid: WindowId, refresh_heuristic: bool) {
        let windows = &mut self.state.windows;
        let Some(window) = windows.window(wid) else { return };
        let manageable = if refresh_heuristic {
            utils::compute_window_info_manageability(&window.info, |id| {
                windows.get_window_server_info(id)
            })
        } else {
            window.is_manageable
        };
        let transient = window.info.sys_id.is_some_and(|id| {
            self.transient_focus
                .windows
                .get(&id)
                .is_some_and(|t| t.native && t.pid == wid.pid)
        }) && !crate::sys::app::is_ibkr_desktop_dialog(&window.info);
        let window = windows.window_mut(wid).unwrap();
        window.native_transient = transient;
        window.is_manageable = manageable;
    }

    /// A classification change can suspend a window immediately. Readmission also
    /// needs positive user-Space presence, supplied here or by native appearance.
    pub(super) fn reconcile_native_admission(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        refresh_heuristic: bool,
        presence: Option<SpaceId>,
    ) -> bool {
        let was_admitted = self.state.windows.is_admitted(wid);
        self.refresh_admission_facts(wid, refresh_heuristic);
        let admitted = self.state.windows.is_admitted(wid);
        if was_admitted && !admitted {
            self.reconcile_window_membership(changes, wid, None);
        } else if !was_admitted && let Some(space) = presence {
            self.admit_native_presence(changes, wid, space);
        }
        !was_admitted && admitted
    }

    fn admit_ax_window(&mut self, changes: &mut Changes, window: WindowId) {
        if !self.state.windows.can_reconcile_admission(window) {
            return;
        }
        if self.state.windows.native_fullscreen_record_for_window(window).is_some() {
            self.restore_native_fullscreen(changes, None, window, None);
            return;
        }
        // Restored windows retain their logical owner even when macOS restores
        // them on the current display or their last frame is parked offscreen.
        let assigned_space = self.assigned_space_for_window_id(window);
        let active_space = assigned_space.or_else(|| {
            let state = self.state.windows.window(window)?;
            self.best_space_for_window(&state.frame_monotonic, state.info.sys_id)
                .filter(|space| self.is_space_active(*space))
                .or_else(|| {
                    state.info.sys_id.is_none().then(|| self.workspace_command_space()).flatten()
                })
        });
        let Some(space) = active_space else { return };
        self.admit_window_in_space(changes, window, space, true);
    }

    fn admit_window_in_space(
        &mut self,
        changes: &mut Changes,
        window: WindowId,
        space: SpaceId,
        pointer_placement: bool,
    ) {
        if let Some(state) = self.state.windows.window(window)
            && let Some(id) = state.info.sys_id
        {
            self.transient_focus.ax(id, window.pid, &state.info, space);
        }
        let app = self.app_manager.apps.get(&window.pid).map(|app| app.info.clone());
        let visible: Vec<_> = self.iter_active_spaces().collect();
        let Some(effects) = self.assign_observed_window(
            changes,
            window,
            WorkspaceContext { space, visible: &visible },
            app.as_ref(),
            false,
        ) else {
            return;
        };
        self.project_observed_window(changes, space, window, effects);
        if pointer_placement
            && effects.source_workspace.is_none()
            && !effects.names_workspace
            && !self.is_dragging_window(window)
            && let Some(owner) = self.assigned_space_for_window_id(window)
        {
            self.place_new_window_on_pointer_display(changes, window, owner);
        }
    }

    pub(super) fn project_observed_window(
        &mut self,
        changes: &mut Changes,
        space: SpaceId,
        window: WindowId,
        effects: AppRuleEffects,
    ) {
        if let Some(state) = self.state.windows.window(window) {
            self.reconcile_layout_event(
                changes,
                LayoutEvent::WindowObserved(space, crate::layout_engine::ResolvedWindow {
                    info: state.layout_info(window),
                    effects,
                }),
            );
        }
    }

    /// Leave fullscreen with the logical workspace intact; placement follows its
    /// current owner through the transfer lifecycle (deferred while dormant), never
    /// the Space macOS returned the window to.
    pub(super) fn restore_native_fullscreen(
        &mut self,
        changes: &mut Changes,
        wsid: Option<WindowServerId>,
        window: WindowId,
        observed: Option<SpaceId>,
    ) {
        if let Some(space) = observed {
            self.state.windows.note_fullscreen_exit(window, space);
        }
        let Some(restored) = self.state.windows.restore_native_identity(wsid, window) else {
            return;
        };
        if let Some(previous) = restored.previous_assignment {
            self.reconcile_layout_event(changes, LayoutEvent::WindowRemoved(previous));
        }
        let Some(window) = restored.window else {
            self.reconcile_window_membership(changes, restored.record.current_window_id, None);
            return;
        };
        // Readiness is checked without rejecting logical ownership. A temporary
        // admission failure keeps both the lifecycle and its preserved layout slot.
        let native = observed.or(match restored.record.transition {
            NativeFullscreenTransition::ExitObserved(space) => space,
            _ => None,
        });
        let engine = &mut self.layout_manager.layout_engine;
        let Some((workspace, target)) = (self.state.windows.workspace_for_window(window))
            .or(restored.record.workspace)
            .and_then(|id| Some((id, engine.workspaces().workspace_space(id)?)))
        else {
            let Some(space) = native else { return };
            self.state.windows.restore_window_from_native_fullscreen(window);
            self.reassign_window_to_authoritative_space(changes, window, space, false);
            return;
        };
        engine.workspaces_mut().assign_window_to_workspace(
            &mut self.state.windows,
            target,
            window,
            workspace,
        );
        // Ownership is recovered before releasing suspension to the existing
        // placement/projection paths. History never confirms physical placement.
        self.state.windows.restore_window_from_native_fullscreen(window);
        let available = self.is_space_active(target);
        if native != Some(target) {
            self.request_window_transfer(changes, window, target, None);
        }
        // Rejoin the tree even while the owner is dormant; arrange only a visible one.
        self.reconcile_window_membership(changes, window, Some(target));
        if available {
            self.refocus_manager.refocus_state = RefocusState::Pending(target);
            changes.arrange(Some(target), Presentation::Animated);
            changes.refresh_ffm = true;
        }
    }

    pub(super) fn reconcile_native_presence(
        &mut self,
        changes: &mut Changes,
        wsid: WindowServerId,
        space: SpaceId,
        window: WindowId,
    ) {
        if self.state.windows.native_fullscreen_record_for_window_server_id(wsid).is_some()
            || self.state.windows.native_fullscreen_record_for_window(window).is_some()
        {
            self.restore_native_fullscreen(changes, Some(wsid), window, Some(space));
            return;
        }
        self.admit_native_presence(changes, window, space);
    }

    fn admit_native_presence(&mut self, changes: &mut Changes, window: WindowId, space: SpaceId) {
        // Appearance can restore participation, but is not an authoritative move.
        let owner = self.assigned_space_for_window_id(window).unwrap_or(space);
        if self.is_space_active(owner) && self.state.windows.is_visible_admitted(window) {
            if self.state.windows.workspace_for_window(window).is_some() {
                self.reconcile_window_membership(changes, window, Some(owner));
            } else {
                self.admit_window_in_space(changes, window, owner, false);
            }
        }
    }

    pub(super) fn reassign_window_to_authoritative_space(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        space: SpaceId,
        preserve_workspace_ordinal: bool,
    ) {
        // A claim holds the destination; release replays this reconciliation.
        if self.is_dragging_window(wid)
            || self.state.windows.is_claimed(wid)
            || self
                .state
                .windows
                .record(wid)
                .and_then(|record| record.pending_transfer())
                .is_some()
        {
            return;
        }
        if !self.state.windows.is_admitted(wid)
            || self.state.windows.is_temporarily_unavailable(wid)
        {
            self.reconcile_window_membership(changes, wid, None);
            return;
        }
        let owner = self.assigned_space_for_window_id(wid);
        if owner.is_none() {
            if self.is_space_active(space) {
                self.admit_window_in_space(changes, wid, space, false);
            }
            return;
        }
        if owner != Some(space) {
            let departed_from = self.state.windows.workspace_for_window(wid);
            let engine = &mut self.layout_manager.layout_engine;
            // A window a departure moved off this display goes back where it was.
            let returning = engine.eviction_ordinal(&self.state.windows, wid, space);
            // Resolve the destination before touching the source: a global-mode
            // Space can own no workspace, and then the current owner is kept.
            engine.workspaces_mut().ensure_space_initialized(space);
            let workspaces = engine.workspaces();
            let remembered =
                returning.and_then(|ordinal| workspaces.workspaces_on(space).nth(ordinal));
            let destination = if remembered.is_some() {
                remembered
            } else if preserve_workspace_ordinal {
                workspaces.workspace_at_window_ordinal(&self.state.windows, space, wid)
            } else {
                engine.ensure_active_workspace_info(space).map(|(id, _)| id)
            };
            let Some(workspace) = destination else {
                debug!(
                    ?wid,
                    ?space,
                    "Native Space has no destination workspace; keeping owner"
                );
                return;
            };
            self.reconcile_layout_event(changes, LayoutEvent::WindowRemovedPreserveFloating(wid));
            let engine = &mut self.layout_manager.layout_engine;
            if !engine.workspaces_mut().assign_window_to_workspace(
                &mut self.state.windows,
                space,
                wid,
                workspace,
            ) {
                return;
            }
            if returning.is_some() {
                engine.take_eviction(wid);
            }
            if remembered.is_none()
                && preserve_workspace_ordinal
                && let Some(from) = departed_from
            {
                engine.note_eviction(&self.state.windows, wid, from);
            }
        }
        if self.is_space_active(space) {
            self.reconcile_window_membership(changes, wid, Some(space));
        }
    }
}
