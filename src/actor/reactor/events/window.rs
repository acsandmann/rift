use objc2_core_foundation::CGRect;
use tracing::{debug, trace, warn};

use crate::actor::app::WindowId;
use crate::actor::reactor::events::drag::DragEventHandler;
use crate::actor::reactor::{
    DragState, Quiet, Reactor, Requested, TransactionId, WindowFilter, WindowState, utils,
};
use crate::common::config::LayoutMode;
use crate::layout_engine::LayoutEvent;
use crate::model::WindowVisibility;
use crate::sys::app::WindowInfo as Window;
use crate::sys::event::{MouseState, get_mouse_state};
use crate::sys::geometry::SameAs;
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
            reactor.state.windows.track_window_server_id(wsid, wid);
            reactor.state.windows.clear_window_server_observed(wsid);
        }
        if let Some(info) = ws_info {
            reactor.state.windows.clear_window_server_observed(info.id);
            reactor.state.windows.track_window_server_info(info);
        }

        let frame = window.frame;
        let mut window_state: WindowState = window.into();
        let is_manageable = utils::compute_window_manageability(
            window_state.info.sys_id,
            window_state.info.is_minimized,
            window_state.info.is_standard,
            window_state.info.is_root,
            |wsid| reactor.state.windows.get_window_server_info(wsid),
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
        reactor.state.windows.insert_window(wid, window_state);

        if is_manageable {
            let active_space = active_space_for_window(reactor, &frame, server_id);
            if let Some(space) = active_space {
                if let Some(app_info) =
                    reactor.app_manager.apps.get(&wid.pid).map(|app| app.info.clone())
                {
                    if let Some(wsid) = server_id {
                        reactor.state.windows.mark_wsids_recent(std::iter::once(wsid));
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
        let window_server_id = match reactor.state.windows.window(wid) {
            Some(window) => window.info.sys_id,
            None => return false,
        };

        // Suppress false-positive destructions when on a fullscreen space or during MC.
        // kAXMainWindowChangedNotification triggers remove_stale_windows in app.rs, which
        // calls kAXWindowsAttribute (space-filtered), omitting Desktop windows and emitting
        // WindowDestroyed for them. `get_window()` is a direct Skylight window query
        // rather than an AX space-filtered view, so Some here means the window still exists.
        if !reactor.has_user_space_context() || reactor.is_mission_control_active() {
            if let Some(ws_id) = window_server_id {
                if crate::sys::window_server::get_window(ws_id)
                    .is_some_and(|ws_info| ws_info.pid == wid.pid)
                {
                    return false;
                }
            }
        }

        if let Some(ws_id) = window_server_id {
            reactor.transaction_manager.remove_for_window(ws_id);
            reactor.state.windows.remove_window_server_state(ws_id);
        } else {
            debug!(?wid, "Received WindowDestroyed for unknown window - ignoring");
        }
        reactor.state.windows.remove_window(wid);
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
        let server_id = if let Some(window) = reactor.state.windows.window_mut(wid) {
            if window.info.is_minimized {
                return;
            }
            window.info.is_minimized = true;
            window.is_manageable = false;
            window.info.sys_id
        } else {
            debug!(?wid, "Received WindowMinimized for unknown window - ignoring");
            return;
        };
        if let Some(ws_id) = server_id {
            reactor.state.windows.mark_window_hidden(ws_id);
        }
        reactor.state.windows.set_visibility(wid, WindowVisibility::Minimized);
        reactor.send_layout_event(LayoutEvent::WindowRemoved(wid));
    }

    pub fn handle_window_deminiaturized(reactor: &mut Reactor, wid: WindowId) {
        let (frame, server_id, is_ax_standard, is_ax_root) =
            match reactor.state.windows.window_mut(wid) {
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
            |wsid| reactor.state.windows.get_window_server_info(wsid),
        );
        if let Some(window) = reactor.state.windows.window_mut(wid) {
            window.is_manageable = is_manageable;
        }
        reactor.state.windows.set_visibility(wid, WindowVisibility::Visible);

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
        mouse_state: Option<MouseState>,
    ) -> bool {
        debug!(
            ?wid,
            ?new_frame,
            last_seen=?last_seen,
            requested=?requested,
            mouse_state=?mouse_state,
            window_known=reactor.state.windows.contains_window(wid),
            "WindowFrameChanged event"
        );

        let effective_mouse_state = mouse_state.or_else(|| get_mouse_state());
        let result = (|| -> bool {
            let (server_id, old_frame) = {
                let Some(window) = reactor.state.windows.window(wid) else {
                    return false;
                };

                if reactor.is_mission_control_active() {
                    return false;
                }

                (window.info.sys_id, window.frame_monotonic)
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

            if effective_mouse_state == Some(MouseState::Down) && triggered_by_rift {
                if let Some((wsid, _)) = pending_target {
                    reactor.transaction_manager.clear_target_for_window(wsid);
                }
                triggered_by_rift = false;
                has_pending_request = false;
            }

            if has_pending_request && last_seen.is_some_and(|seen| seen != last_sent_txid) {
                debug!(?last_seen, ?last_sent_txid, "Ignoring frame change");
                return false;
            }

            if triggered_by_rift {
                let Some(window) = reactor.state.windows.window_mut(wid) else {
                    return false;
                };

                if let Some((wsid, target)) = pending_target {
                    if new_frame.same_as(target) {
                        reactor.transaction_manager.clear_target_for_window(wsid);
                        if !window.frame_monotonic.same_as(new_frame) {
                            debug!(?wid, ?new_frame, "Final frame matches Rift request");
                            window.frame_monotonic = new_frame;
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
                        reactor.transaction_manager.clear_target_for_window(wsid);
                    }
                }

                return false;
            }

            if requested.0 {
                if let Some(window) = reactor.state.windows.window_mut(wid) {
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
                    reactor.transaction_manager.clear_target_for_window(wsid);
                }
                return false;
            }

            let old_space_geometry = reactor.geometry_space_for_window(&old_frame, server_id);
            let new_space_geometry = reactor.geometry_space_for_window(&new_frame, server_id);
            let dragging = effective_mouse_state == Some(MouseState::Down) || reactor.is_in_drag();
            let old_space = old_space_geometry;
            let new_space = new_space_geometry;
            let old_active = old_space.is_some_and(|space| reactor.is_space_active(space));
            let new_active = new_space.is_some_and(|space| reactor.is_space_active(space));

            if !old_active && !new_active {
                return false;
            }

            {
                let Some(window) = reactor.state.windows.window_mut(wid) else {
                    return false;
                };
                if window.frame_monotonic.same_as(new_frame) {
                    return false;
                }
                window.frame_monotonic = new_frame;
            }

            if !dragging {
                reactor.drag_manager.skip_layout_for_window = Some(wid);
            }

            if dragging {
                reactor.ensure_active_drag(wid, &old_frame);
                reactor.update_active_drag(wid, &new_frame);
                let is_resize = !old_frame.size.same_as(new_frame.size);
                if is_resize {
                    if active_space_for_window(reactor, &new_frame, server_id).is_some() {
                        let screens = reactor
                            .space_state
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
                    if let Some(target_space) = server_id
                        .and_then(|wsid| reactor.pending_target_space_for_window_server_id(wsid))
                        && reactor.assigned_space_for_window_id(wid) == Some(target_space)
                        && new_space != Some(target_space)
                    {
                        debug!(
                            ?wid,
                            ?old_space,
                            ?new_space,
                            ?target_space,
                            "Ignoring conflicting geometry-only space change after recent cross-display move"
                        );
                        return false;
                    }

                    let keep_assigned_for_scrolling = old_space.is_some_and(|space| {
                        reactor.layout_manager.layout_engine.active_layout_mode_at(space)
                            == LayoutMode::Scrolling
                            && !reactor.layout_manager.layout_engine.is_window_floating(wid)
                            && reactor
                                .layout_manager
                                .layout_engine
                                .virtual_workspace_manager()
                                .workspace_for_window(&reactor.state.windows, space, wid)
                                .is_some()
                    });
                    if keep_assigned_for_scrolling {
                        debug!(
                            ?wid,
                            ?old_space,
                            ?new_space,
                            "Ignoring geometry-only space change for scrolling tiled window"
                        );
                        return false;
                    }

                    reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                    if let Some(space) = new_space {
                        if let Some(wsid) = server_id {
                            reactor.state.windows.set_window_server_space(wsid, Some(space));
                            reactor.state.windows.mark_window_visible(wsid);
                        }
                        if reactor.is_space_active(space) {
                            if let Some(active_ws) =
                                reactor.layout_manager.layout_engine.active_workspace(space)
                            {
                                let assigned = reactor
                                    .layout_manager
                                    .layout_engine
                                    .virtual_workspace_manager_mut()
                                    .assign_window_to_workspace(
                                        &mut reactor.state.windows,
                                        space,
                                        wid,
                                        active_ws,
                                    );
                                if !assigned {
                                    warn!(
                                        "Failed to assign window {:?} to workspace {:?}",
                                        wid, active_ws
                                    );
                                }
                            }
                            reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                        }
                    } else if let Some(wsid) = server_id {
                        reactor.state.windows.set_window_server_space(wsid, None);
                    }
                    let _ = reactor.update_layout_or_warn(false, false);
                } else if !old_frame.size.same_as(new_frame.size) {
                    if let Some(space) = old_space {
                        if reactor.is_space_active(space) {
                            let screens = reactor
                                .space_state
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
        if let Some(window) = reactor.state.windows.window_mut(wid) {
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
        let Some(wid) = reactor.state.windows.tracked_window_id(wsid) else {
            return;
        };
        let should_sync = reactor.should_raise_on_mouse_over(wid);
        let is_main = reactor.main_window() == Some(wid);
        let needs_sync = reactor.layout_manager.layout_engine.focused_window() != Some(wid);

        if !should_sync || (is_main && !needs_sync) {
            return;
        }

        if !is_main {
            reactor.raise_window(wid, Quiet::No, None);
        }

        if let Some(window) = reactor.state.windows.window(wid) {
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
        .state
        .windows
        .window(wid)
        .map(|window| window.matches_filter(WindowFilter::EffectivelyManageable))
        .unwrap_or(false);
    if should_dispatch {
        reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
    }
}

fn handle_mouse_up_if_needed(reactor: &mut Reactor, mouse_state: Option<MouseState>) {
    if reactor.is_mission_control_active() {
        reactor.drag_manager.reset();
        reactor.drag_manager.drag_state = DragState::Inactive;
        reactor.drag_manager.skip_layout_for_window = None;
        return;
    }

    if mouse_state == Some(MouseState::Up)
        && (matches!(
            reactor.drag_manager.drag_state,
            DragState::Active { .. } | DragState::PendingSwap { .. }
        ) || reactor.drag_manager.skip_layout_for_window.is_some())
    {
        DragEventHandler::handle_mouse_up(reactor);
    }
}
