use objc2_core_foundation::CGRect;
use tracing::{debug, trace, warn};

use crate::actor::app::WindowId;
use crate::actor::reactor::events::drag::DragEventHandler;
use crate::actor::reactor::{
    DragState, FrameChangeKind, Quiet, Reactor, Requested, TransactionId, WindowFilter,
    WindowState, utils,
};
use crate::layout_engine::{LayoutEvent, WindowConstraint};
use crate::sys::app::WindowInfo as Window;
use crate::sys::event::{MouseState, get_mouse_state};
use crate::sys::geometry::{IsWithin, SameAs};
use crate::sys::screen::SpaceId;
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
            window_state.info.sys_id,
            window_state.info.is_minimized,
            window_state.info.is_standard,
            window_state.info.is_root,
            &reactor.window_server_info_manager.window_server_info,
        );
        window_state.is_manageable = is_manageable;
        if let Some(wsid) = window_state.info.sys_id {
            reactor.transaction_manager.store_txid(
                wsid,
                reactor.transaction_manager.get_last_sent_txid(wsid),
                window_state.frame_monotonic,
            );
        }

        let server_id = window_state.info.sys_id;
        reactor.window_manager.windows.insert(wid, window_state);

        if is_manageable {
            let active_space = active_space_for_window(reactor, &frame, server_id);
            if let Some(space) = active_space {
                if let Some(app_info) =
                    reactor.app_manager.apps.get(&wid.pid).map(|app| app.info.clone())
                {
                    if let Some(wsid) = server_id {
                        reactor.app_manager.mark_wsids_recent(std::iter::once(wsid));
                    }
                    reactor.process_windows_for_app_rules(wid.pid, vec![wid], app_info);
                }
                maybe_dispatch_window_added_in_space(reactor, wid, space);
            }
        }
        // TODO: drag state is maybe managed by ensure_active_drag
        // if mouse_state == MouseState::Down {
        //     reactor.drag_manager.drag_state = DragState::Active { ... };
        // }
    }

    pub fn handle_window_destroyed(reactor: &mut Reactor, wid: WindowId) -> bool {
        let window_server_id = match reactor.window_manager.windows.get(&wid) {
            Some(window) => window.info.sys_id,
            None => return false,
        };
        if let Some(ws_id) = window_server_id {
            reactor.transaction_manager.remove_for_window(ws_id);
            reactor.window_manager.window_ids.remove(&ws_id);
            reactor.window_server_info_manager.window_server_info.remove(&ws_id);
            reactor.window_manager.visible_windows.remove(&ws_id);
        } else {
            debug!(?wid, "Received WindowDestroyed for unknown window - ignoring");
        }
        reactor.window_manager.windows.remove(&wid);
        reactor.constraint_probes.remove(&wid);
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
            if window.info.is_minimized {
                return;
            }
            window.info.is_minimized = true;
            window.is_manageable = false;
            if let Some(ws_id) = window.info.sys_id {
                reactor.window_manager.visible_windows.remove(&ws_id);
            }
            reactor.constraint_probes.remove(&wid);
            reactor.send_layout_event(LayoutEvent::WindowRemoved(wid));
        } else {
            debug!(?wid, "Received WindowMinimized for unknown window - ignoring");
        }
    }

    pub fn handle_window_deminiaturized(reactor: &mut Reactor, wid: WindowId) {
        let (frame, server_id, is_ax_standard, is_ax_root) =
            match reactor.window_manager.windows.get_mut(&wid) {
                Some(window) => {
                    if !window.info.is_minimized {
                        return;
                    }
                    window.info.is_minimized = false;
                    (
                        window.frame_monotonic,
                        window.info.sys_id,
                        window.info.is_standard,
                        window.info.is_root,
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
            let active_space = active_space_for_window(reactor, &frame, server_id);
            if let Some(space) = active_space {
                maybe_dispatch_window_added_in_space(reactor, wid, space);
            }
        }
    }

    pub fn handle_window_frame_changed(
        reactor: &mut Reactor,
        wid: WindowId,
        new_frame: CGRect,
        last_seen: Option<TransactionId>,
        requested: Requested,
        change_kind: FrameChangeKind,
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
        let result = (|| -> bool {
            let (server_id, old_frame) = {
                let Some(window) = reactor.window_manager.windows.get(&wid) else {
                    return false;
                };

                if reactor.is_mission_control_active()
                    || window
                        .info
                        .sys_id
                        .is_some_and(|wsid| reactor.space_manager.changing_screens.contains(&wsid))
                {
                    return false;
                }

                (window.info.sys_id, window.frame_monotonic)
            };

            if let Some(probe) = reactor.constraint_probes.get(&wid).copied() {
                if last_seen.is_some_and(|seen| seen == probe.txid) {
                    if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
                        window.frame_monotonic = new_frame;
                    }
                    if let Some(wsid) = server_id {
                        reactor.transaction_manager.remove_for_window(wsid);
                    }
                    let max_w = if new_frame.size.width.is_within(0.1, probe.target.size.width) {
                        None
                    } else if new_frame.size.width < probe.target.size.width {
                        Some(new_frame.size.width)
                    } else {
                        None
                    };
                    let max_h = if new_frame.size.height.is_within(0.1, probe.target.size.height) {
                        None
                    } else if new_frame.size.height < probe.target.size.height {
                        Some(new_frame.size.height)
                    } else {
                        None
                    };
                    let constraint = WindowConstraint { max_w, max_h };
                    reactor.layout_manager.layout_engine.set_window_constraint(wid, constraint);
                    reactor.constraint_probes.remove(&wid);
                    if let Some(app_info) =
                        reactor.app_manager.apps.get(&wid.pid).map(|app| app.info.clone())
                    {
                        reactor.process_windows_for_app_rules(wid.pid, vec![wid], app_info);
                    } else if let Some(space) =
                        active_space_for_window(reactor, &new_frame, server_id)
                    {
                        maybe_dispatch_window_added_in_space(reactor, wid, space);
                    }
                    return true;
                }
            }

            let pending_target = server_id.and_then(|wsid| {
                reactor.transaction_manager.get_target_frame(wsid).map(|target| (wsid, target))
            });

            let last_sent_txid = server_id
                .map(|wsid| reactor.transaction_manager.get_last_sent_txid(wsid))
                .unwrap_or_default();

            let mut has_pending_request = pending_target.is_some();
            let mut triggered_by_rift =
                has_pending_request && last_seen.is_some_and(|seen| seen == last_sent_txid);

            if mouse_state == Some(MouseState::Down) && triggered_by_rift {
                if let Some((wsid, _)) = pending_target {
                    reactor.transaction_manager.remove_for_window(wsid);
                }
                triggered_by_rift = false;
                has_pending_request = false;
            }

            if has_pending_request && last_seen.is_some_and(|seen| seen != last_sent_txid) {
                debug!(?last_seen, ?last_sent_txid, "Ignoring frame change");
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
                    } else if change_kind == FrameChangeKind::Resize
                        && !new_frame.size.same_as(target.size)
                    {
                        debug!(?wid, ?new_frame, ?target, "Resize constrained by window");
                        window.frame_monotonic = new_frame;
                        reactor.transaction_manager.remove_for_window(wsid);
                        let _ = window;

                        if active_space_for_window(reactor, &new_frame, server_id).is_some() {
                            let screens = reactor
                                .space_manager
                                .screens
                                .iter()
                                .filter_map(|screen| {
                                    let display_uuid = screen.display_uuid_owned();
                                    Some((screen.space?, screen.frame, display_uuid))
                                })
                                .collect::<Vec<_>>();
                            let resize_from = target;
                            reactor.send_layout_event(LayoutEvent::WindowResized {
                                wid,
                                old_frame: resize_from,
                                new_frame,
                                screens,
                            });
                            return true;
                        }
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
                    if let Some(wsid) = window.info.sys_id {
                        reactor.transaction_manager.remove_for_window(wsid);
                    }
                }

                return false;
            }

            if requested.0 {
                if let Some(window) = reactor.window_manager.windows.get_mut(&wid) {
                    if !window.frame_monotonic.same_as(new_frame) {
                        debug!(
                            ?wid,
                            ?new_frame,
                            "Requested frame change without pending tx; syncing state"
                        );
                        window.frame_monotonic = new_frame;
                    }
                }
                if let Some(wsid) = server_id {
                    reactor.transaction_manager.remove_for_window(wsid);
                }
                return false;
            }

            let old_space = reactor.best_space_for_window(&old_frame, server_id);
            let new_space = reactor.best_space_for_window(&new_frame, server_id);
            let old_active = old_space.is_some_and(|space| reactor.is_space_active(space));
            let new_active = new_space.is_some_and(|space| reactor.is_space_active(space));

            if !old_active && !new_active {
                return false;
            }

            {
                let Some(window) = reactor.window_manager.windows.get_mut(&wid) else {
                    return false;
                };
                if window.frame_monotonic == new_frame {
                    return false;
                }
                window.frame_monotonic = new_frame;
            }

            let dragging = mouse_state == Some(MouseState::Down) || reactor.is_in_drag();

            if !dragging {
                reactor.drag_manager.skip_layout_for_window = Some(wid);
            }

            if dragging {
                reactor.ensure_active_drag(wid, &old_frame);
                reactor.update_active_drag(wid, &new_frame);
                let is_resize = old_frame.size != new_frame.size;
                if is_resize {
                    if active_space_for_window(reactor, &new_frame, server_id).is_some() {
                        let screens = reactor
                            .space_manager
                            .screens
                            .iter()
                            .filter_map(|screen| {
                                let display_uuid = screen.display_uuid_owned();
                                Some((screen.space?, screen.frame, display_uuid))
                            })
                            .collect::<Vec<_>>();
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
                if old_space != new_space {
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
                            maybe_dispatch_window_added_in_space(reactor, wid, space);
                        }
                    }
                    let _ = reactor.update_layout_or_warn(false, false);
                } else if old_frame.size != new_frame.size {
                    if let Some(space) = old_space {
                        if reactor.is_space_active(space) {
                            let screens = reactor
                                .space_manager
                                .screens
                                .iter()
                                .filter_map(|screen| {
                                    let space = screen.space?;
                                    let display_uuid = screen.display_uuid_owned();
                                    Some((space, screen.frame, display_uuid))
                                })
                                .collect::<Vec<_>>();
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
            let previous_title = window.info.title.clone();
            if previous_title == new_title {
                return;
            }
            window.info.title = new_title.clone();
            reactor.broadcast_window_title_changed(wid, previous_title, new_title);
            reactor.maybe_reapply_app_rules_for_window(wid);
        }
    }

    pub fn handle_mouse_moved_over_window(reactor: &mut Reactor, wsid: WindowServerId) {
        let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) else {
            return;
        };
        if !reactor.should_raise_on_mouse_over(wid) {
            return;
        }

        reactor.raise_window(wid, Quiet::No, None);

        if let Some(window) = reactor.window_manager.windows.get(&wid) {
            if let Some(space) =
                active_space_for_window(reactor, &window.frame_monotonic, window.info.sys_id)
            {
                reactor.send_layout_event(LayoutEvent::WindowFocused(space, wid));
            }
        }
    }
}

fn active_space_for_window(
    reactor: &Reactor,
    frame: &CGRect,
    server_id: Option<WindowServerId>,
) -> Option<SpaceId> {
    let best = reactor.best_space_for_window(frame, server_id);
    if let Some(space) = best.filter(|space| reactor.is_space_active(*space)) {
        return Some(space);
    }

    // Some apps publish AX windows before the window server id/space is ready.
    // Fall back to the active command context so new windows land on the intended display.
    if server_id.is_none() {
        return reactor.workspace_command_space();
    }

    None
}

fn maybe_dispatch_window_added_in_space(reactor: &mut Reactor, wid: WindowId, space: SpaceId) {
    let should_dispatch = reactor
        .window_manager
        .windows
        .get(&wid)
        .map(|window| window.matches_filter(WindowFilter::EffectivelyManageable))
        .unwrap_or(false);
    if should_dispatch {
        if reactor.maybe_start_constraint_probe(wid, space) {
            return;
        }
        reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
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
