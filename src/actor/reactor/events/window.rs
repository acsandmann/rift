use objc2_core_foundation::CGRect;

use super::super::*;
use crate::actor::reactor::transaction_manager::TransactionManager;
use crate::model::WindowVisibility;
use crate::sys::event::MouseState;

#[derive(Debug)]
pub struct WindowFrameChangedPayload {
    pub window: WindowId,
    pub new_frame: CGRect,
    pub old_space: Option<SpaceId>,
    pub new_space: Option<SpaceId>,
    pub old_space_active: bool,
    pub new_space_active: bool,
    pub active_resize_space: Option<SpaceId>,
    pub pending_target_space: Option<SpaceId>,
    pub assigned_space: Option<SpaceId>,
    pub keep_assigned_for_scrolling: bool,
    pub screens: Vec<(SpaceId, CGRect, Option<String>)>,
}

pub fn frame_change_needs_layout(
    state: &mut crate::model::RiftState,
    transactions: &TransactionManager,
    wid: WindowId,
    new_frame: CGRect,
    last_seen: Option<TransactionId>,
    requested: bool,
    mouse_state: &mut Option<MouseState>,
) -> bool {
    let Some(window) = state.windows.window_mut(wid) else {
        return false;
    };
    let server = window.info.sys_id;
    let record = server.and_then(|server| transactions.store.get(&server));

    if requested {
        if let (Some(seen), Some(record)) = (last_seen, record)
            && seen != record.txid
        {
            return false;
        }
        if let Some(server) = server {
            if let Some(seen) = last_seen {
                transactions.store.acknowledge(server, seen, new_frame);
            } else {
                transactions.clear_target_for_window(server);
            }
        }
        window.frame_monotonic = new_frame;
        return false;
    }

    if window.frame_monotonic.same_as(new_frame) {
        return false;
    }
    if mouse_state.is_none() {
        *mouse_state = crate::sys::event::get_mouse_state();
    }
    if let Some(record) = record
        && let Some(target) = record.target()
    {
        if new_frame.same_as(target) {
            window.frame_monotonic = new_frame;
            return false;
        }
        if *mouse_state != Some(MouseState::Down) && last_seen.is_some() {
            // Native notifications can describe an intermediate AX frame. Only
            // the app's readback completes a write.
            if last_seen == Some(record.txid) {
                window.frame_monotonic = new_frame;
            }
            return false;
        }
    }
    if let Some(server) = server {
        transactions.clear_target_for_window(server);
    }
    true
}

impl Reactor {
    pub(in crate::actor::reactor) fn create_window(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        window: WindowInfo,
        ws_info: Option<WindowServerInfo>,
    ) {
        if let Some(previous) = self.state.windows.reconcile_ax_identity(wid, window.sys_id) {
            self.layout_manager
                .layout_engine
                .transfer_persistent_window_identity(previous, wid);
        }
        if let Some(info) = ws_info {
            self.state.windows.clear_window_server_observed(info.id);
            self.state.windows.track_window_server_info(info);
        }
        let rule_override =
            self.state.windows.window(wid).and_then(|window| window.manage_override);
        let mut window_state: WindowState = window.into();
        window_state.manage_override = rule_override;
        self.state.windows.insert_window(wid, window_state);
        let _ = utils::refresh_heuristic(&mut self.state, wid);
        changes.refresh_notifications = true;
        if self.state.windows.can_reconcile_admission(wid) {
            self.finalize_created_window(changes, wid);
        }
    }

    pub(in crate::actor::reactor) fn destroy_window(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
    ) {
        if self.state.windows.suspend_fullscreen_ax_identity(wid) {
            self.reconcile_layout_event(changes, LayoutEvent::WindowSuspended(wid));
        } else if let Some(record) = self.state.windows.remove_window(wid) {
            self.retire_window(changes, (wid, record.window_server_id()));
        }
    }

    pub(in crate::actor::reactor) fn retire_window(
        &mut self,
        changes: &mut Changes,
        (wid, wsid): (WindowId, Option<WindowServerId>),
    ) {
        if let Some(wsid) = wsid {
            self.transaction_manager.remove_for_window(wsid);
            changes.refresh_notifications = true;
        }
        self.drag_manager.forget_window(wid);
        self.reconcile_layout_event(changes, LayoutEvent::WindowRemoved(wid));
    }

    pub(in crate::actor::reactor) fn reconcile_window_visibility(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        visible: bool,
    ) {
        if !visible {
            if self.state.windows.observe_minimized(wid) {
                self.reconcile_layout_event(changes, LayoutEvent::WindowSuspended(wid));
            }
            return;
        }
        let ax_returned = self.state.windows.validate_ax_identity(wid);
        let Some(window) = self.state.windows.window_mut(wid) else {
            debug!(?wid, "Received WindowDeminiaturized for unknown window");
            return;
        };
        if !window.info.is_minimized && !ax_returned {
            return;
        }
        window.info.is_minimized = false;
        let _ = utils::refresh_heuristic(&mut self.state, wid);
        // Inventory needs positive native visibility when other app windows are visible.
        match self.state.windows.record(wid).and_then(|record| record.window_server_id()) {
            Some(wsid) => {
                self.state.windows.mark_window_visible(wsid);
            }
            None => self.state.windows.set_visibility(wid, WindowVisibility::Visible),
        }
        changes.inventory(wid.pid);
        if !self.state.windows.is_claimed(wid) {
            self.finalize_created_window(changes, wid);
        }
    }

    pub(in crate::actor::reactor) fn handle_window_frame_changed(
        &mut self,
        changes: &mut Changes,
        payload: WindowFrameChangedPayload,
    ) {
        let WindowFrameChangedPayload {
            window: wid,
            new_frame,
            old_space,
            new_space,
            old_space_active,
            new_space_active,
            active_resize_space,
            pending_target_space,
            assigned_space,
            keep_assigned_for_scrolling,
            screens,
        } = payload;
        let Some(window) = self.state.windows.window(wid) else {
            return;
        };
        let server_id = window.info.sys_id;
        let old_frame = window.frame_monotonic;

        if !old_space_active && !new_space_active {
            return;
        }
        if old_frame.same_as(new_frame) {
            return;
        }
        if let Some(window) = self.state.windows.window_mut(wid) {
            window.frame_monotonic = new_frame;
        }
        // The external owner has geometry authority; keep only native facts current.
        if self.state.windows.is_claimed(wid) {
            if old_space != new_space
                && let Some(server) = server_id
            {
                match new_space {
                    Some(space) => _ = self.state.windows.observe_native_space(server, space, true),
                    None => self.state.windows.set_window_server_space(server, None),
                }
            }
            return;
        }
        // An adjusted acknowledgement of our own frame write is not a new native drag.
        if matches!(
            self.drag_manager.actor.kind(),
            Some(crate::actor::drag::DragKind::ModifierMove)
        ) && self.drag_manager.actor.source().is_some_and(|source| source.window == wid)
        {
            return;
        }
        self.state.windows.clear_floating_display_geometry(wid);
        // External moves as well as resizes are authoritative for floating layouts.
        // Requested frame acknowledgements have already been filtered by the classifier.
        if let Some(space) = assigned_space.or(old_space)
            && Some(space) == new_space
            && let Some(workspace) = self
                .layout_manager
                .layout_engine
                .workspaces()
                .workspace_for_window(&self.state.windows, space, wid)
            && self.layout_manager.layout_engine.is_window_floating(wid)
        {
            self.layout_manager
                .layout_engine
                .store_floating_position(workspace, wid, new_frame);
        }

        if self.drag_manager.actor.update_native(wid, new_frame, new_space) {
            self.drag_manager.sync_preview();
            if !old_frame.size.same_as(new_frame.size) && active_resize_space.is_some() {
                self.reconcile_layout_event(changes, LayoutEvent::WindowResized {
                    wid,
                    old_frame,
                    new_frame,
                    screens: screens.into(),
                });
            }
        } else {
            for space in [assigned_space.or(old_space), new_space].into_iter().flatten() {
                changes.arrange(Some(space), Presentation::Animated);
            }

            if old_space != new_space {
                if pending_target_space.is_some()
                    && assigned_space == pending_target_space
                    && new_space != pending_target_space
                {
                    return;
                }
                if keep_assigned_for_scrolling {
                    return;
                }
                self.reconcile_layout_event(
                    changes,
                    LayoutEvent::WindowRemovedPreserveFloating(wid),
                );
                if let Some(space) = new_space {
                    if let Some(server) = server_id {
                        self.state.windows.observe_native_space(server, space, true);
                    }
                    if new_space_active
                        && self.state.windows.window(wid).is_some_and(WindowState::is_admitted)
                    {
                        if let Some(workspace) =
                            self.layout_manager.layout_engine.workspaces().active_workspace(space)
                        {
                            let _ = self
                                .layout_manager
                                .layout_engine
                                .workspaces_mut()
                                .assign_window_to_workspace(
                                    &mut self.state.windows,
                                    space,
                                    wid,
                                    workspace,
                                );
                        }
                        self.reconcile_layout_event(changes, LayoutEvent::WindowAdded(space, wid));
                    }
                } else if let Some(server) = server_id {
                    self.state.windows.set_window_server_space(server, None);
                }
            }
        }
    }
}
