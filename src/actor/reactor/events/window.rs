use objc2_core_foundation::CGRect;
use tracing::{debug, trace};

use crate::actor::app::WindowId;
use crate::actor::reactor::{
    DragState, MissionControlState, Quiet, Reactor, Requested, TransactionId, WindowState,
};
use crate::layout_engine::LayoutEvent;
use crate::sys::app::WindowInfo as Window;
use crate::sys::event::MouseState;
use crate::sys::geometry::SameAs;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

pub struct WindowEventHandler;

impl WindowEventHandler {
    pub fn handle_window_created(
        reactor: &mut Reactor,
        wid: WindowId,
        window: Window,
        ws_info: Option<WindowServerInfo>,
        _mouse_state: MouseState,
    ) {
        // FIXME: We assume all windows are on the main screen.
        if let Some(wsid) = window.sys_id {
            reactor.window_manager.window_ids.insert(wsid, wid);
            reactor.window_manager.observed_window_server_ids.remove(&wsid);
        }
        if let Some(info) = ws_info {
            reactor.window_manager.observed_window_server_ids.remove(&info.id);
            reactor.window_server_info_manager.window_server_info.insert(info.id, info);
        }

        let frame = window.frame;
        let mut window_state: WindowState = window.into();
        let is_manageable = reactor.compute_window_manageability(&window_state);
        window_state.is_manageable = is_manageable;
        reactor.store_txid(
            window_state.window_server_id,
            window_state.last_sent_txid,
            window_state.frame_monotonic,
        );
        reactor.window_manager.windows.insert(wid, window_state);

        if is_manageable {
            if let Some(space) = reactor.best_space_for_window(&frame) {
                reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
            }
        }
        // TODO: drag state is maybe managed by ensure_active_drag
        // if mouse_state == MouseState::Down {
        //     reactor.drag_manager.drag_state = DragState::Active { ... };
        // }
    }

    pub fn handle_window_destroyed(reactor: &mut Reactor, wid: WindowId) -> bool {
        if !reactor.window_manager.windows.contains_key(&wid) {
            return false;
        }
        let window_server_id =
            reactor.window_manager.windows.get(&wid).and_then(|w| w.window_server_id);
        reactor.remove_txid_for_window(window_server_id);
        if let Some(ws_id) = window_server_id {
            reactor.window_manager.window_ids.remove(&ws_id);
            reactor.window_server_info_manager.window_server_info.remove(&ws_id);
            reactor.window_manager.visible_windows.remove(&ws_id);
        } else {
            debug!(?wid, "Received WindowDestroyed for unknown window - ignoring");
        }
        reactor.window_manager.windows.remove(&wid);
        reactor.send_layout_event(LayoutEvent::WindowRemoved(wid));

        if let DragState::PendingSwap { dragged, target } = &reactor.drag_manager.drag_state {
            if *dragged == wid || *target == wid {
                trace!(
                    ?wid,
                    "Clearing pending drag swap because a participant window was destroyed"
                );
                reactor.drag_manager.drag_state = DragState::Inactive;
            }
        }

        let dragged_window = reactor.drag_manager.dragged();
        let last_target = reactor.drag_manager.last_target();
        if dragged_window == Some(wid) || last_target == Some(wid) {
            reactor.drag_manager.reset();
            if dragged_window == Some(wid) {
                reactor.drag_manager.drag_state = DragState::Inactive;
            }
        }

        if reactor.drag_manager.skip_layout_for_window == Some(wid) {
            reactor.drag_manager.skip_layout_for_window = None;
        }
        true
    }

    pub fn handle_window_minimized(reactor: &mut Reactor, wid: WindowId) {
        if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
            if window.is_minimized {
                return;
            }
            window.is_minimized = true;
            window.is_manageable = false;
            if let Some(ws_id) = window.window_server_id {
                reactor.window_manager.visible_windows.remove(&ws_id);
            }
            reactor.send_layout_event(LayoutEvent::WindowRemoved(wid));
        } else {
            debug!(?wid, "Received WindowMinimized for unknown window - ignoring");
        }
    }

    pub fn handle_window_deminiaturized(reactor: &mut Reactor, wid: WindowId) {
        let (frame, server_id, is_ax_standard, is_ax_root) =
            match reactor.window_manager.windows.get_mut(&wid) {
                Some(window) => {
                    if !window.is_minimized {
                        return;
                    }
                    window.is_minimized = false;
                    (
                        window.frame_monotonic,
                        window.window_server_id,
                        window.is_ax_standard,
                        window.is_ax_root,
                    )
                }
                None => {
                    debug!(
                        ?wid,
                        "Received WindowDeminiaturized for unknown window - ignoring"
                    );
                    return;
                }
            };
        let is_manageable =
            reactor.compute_manageability_from_parts(server_id, false, is_ax_standard, is_ax_root);
        if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
            window.is_manageable = is_manageable;
        }

        if is_manageable {
            if let Some(space) = reactor.best_space_for_window(&frame) {
                reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
            }
        }
    }

    pub fn handle_window_frame_changed(
        reactor: &mut Reactor,
        wid: WindowId,
        new_frame: CGRect,
        last_seen: Option<TransactionId>,
        requested: Requested,
        mouse_state: Option<MouseState>,
    ) -> bool {
        if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
            if matches!(
                reactor.mission_control_manager.mission_control_state,
                MissionControlState::Active
            ) || window
                .window_server_id
                .is_some_and(|wsid| reactor.space_manager.changing_screens.contains(&wsid))
            {
                return false;
            }
            let triggered_by_rift = last_seen.is_some_and(|seen| seen == window.last_sent_txid);
            if let Some(last_seen) = last_seen
                && last_seen != window.last_sent_txid
            {
                // Ignore events that happened before the last time we
                // changed the size or position of this window. Otherwise
                // we would update the layout model incorrectly.
                debug!(?last_seen, ?window.last_sent_txid, "Ignoring frame change");
                return false;
            }
            if requested.0 {
                // TODO: If the size is different from requested, applying a
                // correction to the model can result in weird feedback
                // loops, so we ignore these for now.
                return false;
            }
            if triggered_by_rift {
                if let Some(store) = reactor.notification_manager.window_tx_store.as_ref()
                    && let Some(wsid) = window.window_server_id
                {
                    if let Some(record) = store.get(&wsid) {
                        if new_frame.same_as(record.target) {
                            if !window.frame_monotonic.same_as(new_frame) {
                                debug!(?wid, ?new_frame, "Final frame matches Rift request");
                                window.frame_monotonic = new_frame;
                            }
                            store.remove(&wsid);
                        } else {
                            trace!(
                                ?wid,
                                ?new_frame,
                                ?record.target,
                                "Skipping intermediate frame from Rift request"
                            );
                        }
                    } else if !window.frame_monotonic.same_as(new_frame) {
                        debug!(
                            ?wid,
                            ?new_frame,
                            "Rift frame event missing tx record; updating state"
                        );
                        window.frame_monotonic = new_frame;
                    }
                } else if !window.frame_monotonic.same_as(new_frame) {
                    debug!(
                        ?wid,
                        ?new_frame,
                        "Rift frame event without store; updating state"
                    );
                    window.frame_monotonic = new_frame;
                }
                return false;
            }
            let old_frame = std::mem::replace(&mut window.frame_monotonic, new_frame);
            if old_frame == new_frame {
                return false;
            }

            let dragging = mouse_state == Some(MouseState::Down)
                || matches!(
                    reactor.drag_manager.drag_state,
                    DragState::Active { .. } | DragState::PendingSwap { .. }
                );

            if dragging {
                reactor.ensure_active_drag(wid, &old_frame);
                reactor.update_active_drag(wid, &new_frame);
                if old_frame.size != new_frame.size {
                    reactor.mark_drag_dirty(wid);
                }
                reactor.maybe_swap_on_drag(wid, new_frame);
            } else {
                let screens = reactor
                    .space_manager
                    .screens
                    .iter()
                    .flat_map(|screen| Some((screen.space?, screen.frame)))
                    .collect::<Vec<_>>();

                let old_space = reactor.best_space_for_window(&old_frame);
                let new_space = reactor.best_space_for_window(&new_frame);

                if old_space != new_space {
                    if matches!(
                        reactor.drag_manager.drag_state,
                        DragState::Active { .. } | DragState::PendingSwap { .. }
                    ) || matches!(&reactor.drag_manager.drag_state, DragState::Active { session } if session.window == wid)
                    {
                        if let Some(space) = new_space {
                            if let DragState::Active { session } =
                                &mut reactor.drag_manager.drag_state
                            {
                                if session.window == wid {
                                    session.settled_space = Some(space);
                                    session.layout_dirty = true;
                                }
                            }
                        }
                    } else {
                        if let Some(space) = new_space {
                            if let Some(active_ws) =
                                reactor.layout_manager.layout_engine.active_workspace(space)
                            {
                                let _ = reactor
                                    .layout_manager
                                    .layout_engine
                                    .virtual_workspace_manager_mut()
                                    .assign_window_to_workspace(space, wid, active_ws);
                            }
                            reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                            let _ = reactor.update_layout(false, false);
                        } else {
                            reactor.send_layout_event(LayoutEvent::WindowRemoved(wid));
                            let _ = reactor.update_layout(false, false);
                        }
                    }
                } else if old_frame.size != new_frame.size {
                    reactor.send_layout_event(LayoutEvent::WindowResized {
                        wid,
                        old_frame,
                        new_frame,
                        screens,
                    });
                    return true;
                }
            }
        }
        false
    }

    pub fn handle_mouse_moved_over_window(reactor: &mut Reactor, wsid: WindowServerId) {
        let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) else {
            return;
        };
        if reactor.should_raise_on_mouse_over(wid) {
            reactor.raise_window(wid, Quiet::No, None);
        }
    }
}
