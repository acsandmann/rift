use objc2_core_foundation::CGRect;

use super::super::*;
use crate::actor::reactor::transaction_manager::TransactionManager;
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

        if self.drag_manager.actor.update_native(wid, new_frame) {
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

            if old_space == new_space
                || keep_assigned_for_scrolling
                || (pending_target_space.is_some()
                    && assigned_space == pending_target_space
                    && new_space != pending_target_space)
            {
                return;
            }
            if let Some(server) = server_id {
                if let Some(space) = new_space {
                    self.state.windows.observe_native_space(server, space, true);
                } else {
                    self.state.windows.set_window_server_space(server, None);
                }
            }
            if let Some(space) = new_space.filter(|_| new_space_active) {
                self.reassign_window_to_authoritative_space(changes, wid, space, false);
            }
        }
    }
}
