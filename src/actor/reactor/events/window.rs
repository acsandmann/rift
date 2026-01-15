use objc2_core_foundation::CGRect;
use tracing::{debug, trace, warn};

use crate::actor::app::WindowId;
use crate::actor::reactor::events::drag::DragEventHandler;
use crate::actor::reactor::{
    DragState, MissionControlState, Quiet, Reactor, Requested, TransactionId, WindowState, utils,
};
use crate::layout_engine::LayoutEvent;
use crate::sys::app::WindowInfo as Window;
use crate::sys::event::{MouseState, get_mouse_state};
use crate::sys::geometry::SameAs;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

pub struct WindowEventHandler;

impl WindowEventHandler {
    pub fn handle_window_created(
        reactor: &mut Reactor,
        wid: WindowId,
        window: Window,
        ws_info: Option<WindowServerInfo>,
        _mouse_state: Option<MouseState>,
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
        let is_manageable = utils::compute_window_manageability(
            window_state.window_server_id,
            window_state.is_minimized,
            window_state.is_ax_standard,
            window_state.is_ax_root,
            &reactor.window_server_info_manager.window_server_info,
        );
        window_state.is_manageable = is_manageable;
        if let Some(wsid) = window_state.window_server_id {
            reactor.transaction_manager.store_txid(
                wsid,
                reactor.transaction_manager.get_last_sent_txid(wsid),
                window_state.frame_monotonic,
            );
        }

        let server_id = window_state.window_server_id;
        reactor.window_manager.windows.insert(wid, window_state);

        if is_manageable {
            if let Some(space) = reactor.best_space_for_window(&frame, server_id) {
                if reactor.is_space_active(space) {
                    if let Some(app_info) =
                        reactor.app_manager.apps.get(&wid.pid).map(|app| app.info.clone())
                    {
                        if let Some(wsid) = server_id {
                            reactor.app_manager.mark_wsids_recent(std::iter::once(wsid));
                        }
                        reactor.process_windows_for_app_rules(wid.pid, vec![wid], app_info);
                    }
                    let should_dispatch = reactor
                        .window_manager
                        .windows
                        .get(&wid)
                        .map(|window| window.is_effectively_manageable())
                        .unwrap_or(false);
                    if should_dispatch {
                        reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                    }
                }
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
        if let Some(ws_id) = window_server_id {
            reactor.transaction_manager.remove_for_window(ws_id);
            reactor.window_manager.window_ids.remove(&ws_id);
            reactor.window_server_info_manager.window_server_info.remove(&ws_id);
            reactor.window_manager.visible_windows.remove(&ws_id);
        } else {
            debug!(?wid, "Received WindowDestroyed for unknown window - ignoring");
        }
        reactor.window_manager.windows.remove(&wid);
        reactor.send_layout_event(LayoutEvent::WindowRemoved(wid));

        if let DragState::PendingSwap { session, target } = &reactor.drag_manager.drag_state {
            if session.window == wid || *target == wid {
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
        let is_manageable = utils::compute_window_manageability(
            server_id,
            false,
            is_ax_standard,
            is_ax_root,
            &reactor.window_server_info_manager.window_server_info,
        );
        if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
            window.is_manageable = is_manageable;
        }

        if is_manageable {
            if let Some(space) = reactor.best_space_for_window(&frame, server_id) {
                if reactor.is_space_active(space) {
                    let should_dispatch = reactor
                        .window_manager
                        .windows
                        .get(&wid)
                        .map(|window| window.is_effectively_manageable())
                        .unwrap_or(false);
                    if should_dispatch {
                        reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                    }
                }
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
        debug!(
            ?wid,
            ?new_frame,
            last_seen=?last_seen,
            requested=?requested,
            mouse_state=?mouse_state,
            window_known=reactor.window_manager.windows.contains_key(&wid),
            "WindowFrameChanged event"
        );

        let effective_mouse_state = mouse_state.or_else(|| get_mouse_state());
        let event_mouse_state = mouse_state;
        let result = (|| -> bool {
            let (server_id, old_frame) = {
                let Some(window) = reactor.window_manager.windows.get_mut(&wid) else {
                    return false;
                };

                if matches!(
                    reactor.mission_control_manager.mission_control_state,
                    MissionControlState::Active
                ) || window
                    .window_server_id
                    .is_some_and(|wsid| reactor.space_manager.changing_screens.contains(&wsid))
                {
                    return false;
                }

                let server_id = window.window_server_id;
                let old_frame = window.frame_monotonic;

                (server_id, old_frame)
            };

            let pending_target = server_id.and_then(|wsid| {
                reactor.transaction_manager.get_target_frame(wsid).map(|target| (wsid, target))
            });

            let last_sent_txid = server_id
                .map(|wsid| reactor.transaction_manager.get_last_sent_txid(wsid))
                .unwrap_or_default();

            let mut has_pending_request = pending_target.is_some();
            let mut triggered_by_rift =
                has_pending_request && last_seen.is_some_and(|seen| seen == last_sent_txid);

            if event_mouse_state == Some(MouseState::Down) && triggered_by_rift {
                if let Some((wsid, _)) = pending_target {
                    reactor.transaction_manager.remove_for_window(wsid);
                }
                triggered_by_rift = false;
                has_pending_request = false;
            }

            if has_pending_request {
                if let Some(last_seen) = last_seen
                    && last_seen != last_sent_txid
                {
                    debug!(?last_seen, ?last_sent_txid, "Ignoring frame change");
                    return false;
                }
            }

            if requested.0 {
                return false;
            }

            if triggered_by_rift {
                let Some(window) = reactor.window_manager.windows.get_mut(&wid) else {
                    return false;
                };

                if let Some((wsid, target)) = pending_target {
                    if new_frame.same_as(target) {
                        if !window.frame_monotonic.same_as(new_frame) {
                            debug!(?wid, ?new_frame, "Final frame matches Rift request");
                            window.frame_monotonic = new_frame;
                        }
                        reactor.transaction_manager.remove_for_window(wsid);
                    } else {
                        trace!(
                            ?wid,
                            ?new_frame,
                            ?target,
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
                    if let Some(wsid) = window.window_server_id {
                        reactor.transaction_manager.remove_for_window(wsid);
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

            let old_space = reactor.best_space_for_window(&old_frame, server_id);
            let new_space = reactor.best_space_for_window(&new_frame, server_id);
            let old_active = old_space.map(|space| reactor.is_space_active(space)).unwrap_or(false);
            let new_active = new_space.map(|space| reactor.is_space_active(space)).unwrap_or(false);

            if !old_active && !new_active {
                return false;
            }

            let window_server_id = {
                let Some(window) = reactor.window_manager.windows.get_mut(&wid) else {
                    return false;
                };
                let old_frame2 = std::mem::replace(&mut window.frame_monotonic, new_frame);
                if old_frame2 == new_frame {
                    return false;
                }
                window.window_server_id
            };

            let dragging = event_mouse_state == Some(MouseState::Down)
                || matches!(
                    reactor.drag_manager.drag_state,
                    DragState::Active { .. } | DragState::PendingSwap { .. }
                );

            if !dragging && !triggered_by_rift {
                reactor.drag_manager.skip_layout_for_window = Some(wid);
            }

            if dragging {
                reactor.ensure_active_drag(wid, &old_frame);
                reactor.update_active_drag(wid, &new_frame);
                let is_resize = old_frame.size != new_frame.size;
                if is_resize {
                    let screens = reactor
                        .space_manager
                        .screens
                        .iter()
                        .filter_map(|screen| {
                            let display_uuid = if screen.display_uuid.is_empty() {
                                None
                            } else {
                                Some(screen.display_uuid.clone())
                            };
                            Some((screen.space?, screen.frame, display_uuid))
                        })
                        .collect::<Vec<_>>();
                    if let Some(space) = reactor.best_space_for_window(&new_frame, window_server_id)
                        && reactor.is_space_active(space)
                    {
                        reactor.send_layout_event(LayoutEvent::WindowResized {
                            wid,
                            old_frame,
                            new_frame,
                            screens,
                        });
                    }
                } else {
                    reactor.maybe_swap_on_drag(wid, new_frame);
                }
            } else {
                let screens = reactor
                    .space_manager
                    .screens
                    .iter()
                    .filter_map(|screen| {
                        let space = screen.space?;
                        let display_uuid = if screen.display_uuid.is_empty() {
                            None
                        } else {
                            Some(screen.display_uuid.clone())
                        };
                        Some((space, screen.frame, display_uuid))
                    })
                    .collect::<Vec<_>>();

                if old_space != new_space {
                    if matches!(
                        reactor.drag_manager.drag_state,
                        DragState::Active { .. } | DragState::PendingSwap { .. }
                    ) || matches!(
                        &reactor.drag_manager.drag_state,
                        DragState::Active { session } if session.window == wid
                    ) {
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
                        reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                        if let Some(space) = new_space {
                            if reactor.is_space_active(space) {
                                if let Some(active_ws) =
                                    reactor.layout_manager.layout_engine.active_workspace(space)
                                {
                                    let assigned = reactor
                                        .layout_manager
                                        .layout_engine
                                        .virtual_workspace_manager_mut()
                                        .assign_window_to_workspace(space, wid, active_ws);
                                    if !assigned {
                                        warn!(
                                            "Failed to assign window {:?} to workspace {:?}",
                                            wid, active_ws
                                        );
                                    }
                                }
                                reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                            }
                        }
                        let _ = reactor.update_layout_or_warn(false, false);
                    }
                } else if old_frame.size != new_frame.size {
                    if let Some(space) = old_space {
                        if reactor.is_space_active(space) {
                            reactor.send_layout_event(LayoutEvent::WindowResized {
                                wid,
                                old_frame,
                                new_frame,
                                screens,
                            });
                            return true;
                        }
                    }
                    return false;
                }
            }
            false
        })();
        handle_mouse_up_if_needed(reactor, effective_mouse_state);
        result
    }

    pub fn handle_window_title_changed(reactor: &mut Reactor, wid: WindowId, new_title: String) {
        if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
            let previous_title = window.title.clone();
            if previous_title == new_title {
                return;
            }
            window.title = new_title.clone();
            reactor.broadcast_window_title_changed(wid, previous_title, new_title);
            reactor.maybe_reapply_app_rules_for_window(wid);
        }
    }

    pub fn handle_mouse_moved_over_window(reactor: &mut Reactor, wsid: WindowServerId) {
        let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) else {
            return;
        };
        if !reactor.is_window_on_active_space(wid) {
            return;
        }
        if !reactor.should_raise_on_mouse_over(wid) {
            return;
        }

        reactor.raise_window(wid, Quiet::No, None);

        let space = reactor.window_manager.windows.get(&wid).and_then(|window| {
            reactor.best_space_for_window(&window.frame_monotonic, window.window_server_id)
        });

        if let Some(space) = space {
            if reactor.is_space_active(space) {
                reactor.send_layout_event(LayoutEvent::WindowFocused(space, wid));
            }
        }
    }
}

fn handle_mouse_up_if_needed(reactor: &mut Reactor, mouse_state: Option<MouseState>) {
    if mouse_state == Some(MouseState::Up)
        && (matches!(
            reactor.drag_manager.drag_state,
            DragState::Active { .. } | DragState::PendingSwap { .. }
        ) || reactor.drag_manager.skip_layout_for_window.is_some())
    {
        DragEventHandler::handle_mouse_up(reactor);
    }
}
