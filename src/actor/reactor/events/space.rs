use std::collections::hash_map::Entry;

use objc2_app_kit::NSRunningApplication;
use tracing::{debug, trace, warn};

use crate::actor::app::Request;
use crate::actor::reactor::{
    FullscreenSpaceTrack, FullscreenWindowTrack, LayoutEvent, Reactor, SpaceEventKind,
    StaleCleanupState,
};
use crate::actor::spaces::{ForwardedSpaceState, TopologyWindowDelta};
use crate::actor::wm_controller::WmEvent;
use crate::sys::app::AppInfo;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

pub struct SpaceEventHandler;

impl SpaceEventHandler {
    pub fn handle_space_state_changed(reactor: &mut Reactor, space_state: ForwardedSpaceState) {
        let pending_space_state = space_state.clone();
        let ForwardedSpaceState {
            screens,
            fullscreen_by_space,
            has_seen_display_set,
            active_spaces,
            command_space,
            display_space_ids,
            last_user_space_by_display,
            space_remaps,
            display_set_changed,
            // These are upstream derivation hints. By the time the reactor consumes a
            // snapshot, the meaningful effects are already carried by `display_set_changed`,
            // `space_remaps`, `resized_spaces`, and `should_force_refresh_layout`.
            topology_changed: _,
            allow_space_remap: _,
            should_force_refresh_layout,
            resized_spaces,
            topology_window_delta,
        } = space_state;

        reactor.space_state.has_seen_display_set = has_seen_display_set;
        reactor.space_state.fullscreen_by_space = fullscreen_by_space;
        let spaces: Vec<Option<SpaceId>> = screens.iter().map(|screen| screen.space).collect();

        if display_set_changed {
            let active_list: Vec<String> =
                screens.iter().map(|screen| screen.display_uuid.clone()).collect();
            reactor.layout_manager.layout_engine.prune_display_state(&active_list);
        }

        if screens.is_empty() {
            update_stale_cleanup_state(reactor, true);
            if !reactor.space_state.screens.is_empty() {
                reactor.space_state.screens.clear();
                reactor.expose_all_spaces();
            }

            reactor.recompute_and_set_active_spaces(&[]);
            reactor.update_complete_window_server_info(Vec::new());
            reactor.try_apply_pending_space_change();
            return;
        }

        update_stale_cleanup_state(reactor, false);
        reactor.space_state.screens = screens;
        reactor.space_state.command_space = command_space;
        reactor.space_state.display_space_ids = display_space_ids;
        reactor.space_state.last_user_space_by_display = last_user_space_by_display;

        if reactor.is_mission_control_active() {
            reactor.pending_space_change_manager.pending_space_change = Some(pending_space_state);
            return;
        }

        let cfg = reactor.activation_cfg();
        let current_screens = reactor.screens_for_current_spaces();
        reactor.space_activation_policy.on_spaces_updated(cfg, &current_screens);
        reactor.apply_authoritative_active_spaces(active_spaces);
        reactor.restore_windows_after_fullscreen_exit(&spaces);
        for screen in &reactor.space_state.screens {
            let (Some(space), Some(display_uuid)) = (screen.space, screen.display_uuid_opt()) else {
                continue;
            };
            reactor
                .layout_manager
                .layout_engine
                .update_space_display(space, Some(display_uuid.to_string()));
        }
        for (previous_space, space) in space_remaps {
            reactor
                .layout_manager
                .layout_engine
                .remap_space(previous_space, space);
        }

        for (space, size) in resized_spaces {
            if !reactor.is_space_active(space) {
                continue;
            }
            reactor
                .layout_manager
                .layout_engine
                .virtual_workspace_manager_mut()
                .list_workspaces(space);
            reactor.send_layout_event(LayoutEvent::SpaceExposed(space, size));
        }

        if let Some(TopologyWindowDelta {
            appeared,
            disappeared,
            ..
        }) = topology_window_delta
        {
            for (wsid, sid) in disappeared {
                SpaceEventHandler::handle_window_server_destroyed(
                    reactor,
                    wsid,
                    sid,
                    SpaceEventKind::User,
                );
            }
            for (wsid, sid) in appeared {
                SpaceEventHandler::handle_window_server_appeared(
                    reactor,
                    wsid,
                    sid,
                    SpaceEventKind::User,
                );
            }
        }

        let ws_info = reactor.authoritative_window_snapshot_for_active_spaces();
        reactor.finalize_space_change(&spaces, ws_info);
        reactor.try_apply_pending_space_change();

        if should_force_refresh_layout {
            reactor.force_refresh_all_windows();
            let _ = reactor.update_layout_or_warn_with(
                false,
                false,
                "Layout update failed after topology change",
            );
        }
    }

    // spacewindowappeared/destroyed happen a lot when a display is connected/disconnected
    // since they are literally when a window enters or leaves a space and each display has its own space(s)
    pub fn handle_window_server_destroyed(
        reactor: &mut Reactor,
        wsid: WindowServerId,
        sid: SpaceId,
        kind: SpaceEventKind,
    ) {
        if matches!(kind, SpaceEventKind::Fullscreen) {
            let (pid, window_id) = if let Some(wid) = reactor.window_manager.tracked_window_id(wsid)
            {
                (wid.pid, Some(wid))
            } else if let Some(info) = reactor.window_manager.get_window_server_info(wsid) {
                (info.pid, None)
            } else {
                // We don't know who owned this fullscreen window.
                return;
            };

            let last_known_user_space = resolve_last_known_user_space(reactor, window_id);
            record_fullscreen_window(reactor, sid, pid, window_id, last_known_user_space);

            if let Some(wid) = window_id
                && let Some(app_state) = reactor.app_manager.apps.get(&wid.pid)
            {
                if let Err(e) = app_state.handle.send(Request::WindowMaybeDestroyed(wid)) {
                    warn!("Failed to send WindowMaybeDestroyed: {}", e);
                }
            }

            return;
        } else if matches!(kind, SpaceEventKind::User) {
            reactor.window_manager.set_window_server_space(wsid, Some(sid));
            reactor.window_manager.mark_window_hidden(wsid);
            if let Some(wid) = reactor.window_manager.tracked_window_id(wsid) {
                let layout_changed = reactor.assigned_space_for_window_id(wid) == Some(sid);
                if layout_changed {
                    reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                }
                if layout_changed && !reactor.is_mission_control_active() {
                    let _ = reactor.update_layout_or_warn(false, false);
                }
                if let Some(app_state) = reactor.app_manager.apps.get(&wid.pid) {
                    if let Err(e) = app_state.handle.send(Request::WindowMaybeDestroyed(wid)) {
                        warn!("Failed to send WindowMaybeDestroyed: {}", e);
                    }
                }
            } else {
                debug!(
                    ?wsid,
                    "Received WindowServerDestroyed for unknown window - ignoring"
                );
            }
            return;
        }
    }

    pub fn handle_window_server_appeared(
        reactor: &mut Reactor,
        wsid: WindowServerId,
        sid: SpaceId,
        kind: SpaceEventKind,
    ) {
        if matches!(kind, SpaceEventKind::User) {
            reactor.window_manager.set_window_server_space(wsid, Some(sid));
            reactor.window_manager.mark_window_visible(wsid);
        }

        if reactor.window_manager.knows_window_server_id(wsid)
            || reactor.window_manager.is_window_server_observed(wsid)
        {
            if matches!(kind, SpaceEventKind::User)
                && !reactor.is_mission_control_active()
                && let Some(wid) = reactor.window_manager.tracked_window_id(wsid)
            {
                let layout_changed = reactor.reassign_window_to_authoritative_space(wid, sid);
                if layout_changed {
                    let _ = reactor.update_layout_or_warn(false, false);
                }
            }
            debug!(
                ?wsid,
                "Received WindowServerAppeared for known window - ignoring"
            );
            return;
        }

        reactor.window_manager.mark_window_server_observed(wsid);
        // TODO: figure out why this is happening, we should really know about this app,
        // why dont we get notifications that its being launched?
        if let Some(window_server_info) = crate::sys::window_server::get_window(wsid) {
            if window_server_info.layer != 0 {
                trace!(
                    ?wsid,
                    layer = window_server_info.layer,
                    "Ignoring non-normal window"
                );
                return;
            }

            // Filter out very small windows (likely tooltips or similar UI elements)
            // that shouldn't be managed by the window manager
            const MIN_MANAGEABLE_WINDOW_SIZE: f64 = 50.0;
            if window_server_info.frame.size.width < MIN_MANAGEABLE_WINDOW_SIZE
                || window_server_info.frame.size.height < MIN_MANAGEABLE_WINDOW_SIZE
            {
                trace!(
                    ?wsid,
                    "Ignoring tiny window ({}x{}) - likely tooltip",
                    window_server_info.frame.size.width,
                    window_server_info.frame.size.height
                );
                return;
            }

            if matches!(kind, SpaceEventKind::Fullscreen) {
                let window_id = reactor.window_manager.tracked_window_id(wsid);
                let last_known_user_space = resolve_last_known_user_space(reactor, window_id);
                record_fullscreen_window(
                    reactor,
                    sid,
                    window_server_info.pid,
                    window_id,
                    last_known_user_space,
                );
                request_visible_windows(
                    reactor,
                    window_server_info.pid,
                    "refresh after fullscreen appearance",
                );

                return;
            }

            reactor.update_partial_window_server_info(vec![window_server_info]);

            if !reactor.app_manager.apps.contains_key(&window_server_info.pid) {
                if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(
                    window_server_info.pid,
                ) {
                    debug!(
                        ?app,
                        "Received WindowServerAppeared for unknown app - synthesizing AppLaunch"
                    );
                    reactor.communication_manager.wm_sender.as_ref().map(|wm| {
                        wm.send(WmEvent::AppLaunch(window_server_info.pid, AppInfo::from(&*app)))
                    });
                }
            } else if let Some(app) = reactor.app_manager.apps.get(&window_server_info.pid) {
                if let Err(err) = app.handle.send(Request::GetVisibleWindows) {
                    warn!(
                        pid = window_server_info.pid,
                        ?wsid,
                        ?err,
                        "Failed to refresh windows after WindowServerAppeared"
                    );
                }
            }
        }
    }

    pub fn handle_mission_control_native_entered(reactor: &mut Reactor) {
        reactor.drag_manager.reset();
        reactor.drag_manager.drag_state = crate::actor::reactor::DragState::Inactive;
        reactor.drag_manager.skip_layout_for_window = None;
        reactor.set_mission_control_active(true);
    }

    pub fn handle_mission_control_native_exited(reactor: &mut Reactor) {
        if reactor.is_mission_control_active() {
            reactor.set_mission_control_active(false);
        }
        reactor.repair_spaces_after_mission_control();
        reactor.refresh_windows_after_mission_control();
    }
}

fn resolve_last_known_user_space(
    reactor: &Reactor,
    window_id: Option<crate::actor::app::WindowId>,
) -> Option<SpaceId> {
    window_id
        .and_then(|wid| reactor.best_space_for_window_id(wid))
        .or_else(|| reactor.space_state.iter_known_spaces().next())
}

fn record_fullscreen_window(
    reactor: &mut Reactor,
    sid: SpaceId,
    pid: i32,
    window_id: Option<crate::actor::app::WindowId>,
    last_known_user_space: Option<SpaceId>,
) {
    let entry = match reactor.space_state.fullscreen_by_space.entry(sid.get()) {
        Entry::Occupied(o) => o.into_mut(),
        Entry::Vacant(v) => v.insert(FullscreenSpaceTrack::default()),
    };

    entry.windows.push(FullscreenWindowTrack {
        pid,
        window_id,
        last_known_user_space,
        _last_seen_fullscreen_space: sid,
    });
}

fn request_visible_windows(reactor: &Reactor, pid: i32, context: &str) {
    if let Some(app_state) = reactor.app_manager.apps.get(&pid) {
        if let Err(e) = app_state.handle.send(Request::GetVisibleWindows) {
            warn!("Failed to {}: {}", context, e);
        }
    }
}

fn update_stale_cleanup_state(reactor: &mut Reactor, spaces_all_none: bool) {
    reactor.refocus_manager.stale_cleanup_state = if spaces_all_none {
        StaleCleanupState::Suppressed
    } else {
        StaleCleanupState::Enabled
    };
}
