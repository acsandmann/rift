use std::collections::hash_map::Entry;

use objc2_app_kit::NSRunningApplication;
use tracing::{debug, trace, warn};

use crate::actor::app::{Request, WindowId};
use crate::actor::reactor::events::window::WindowEventHandler;
use crate::actor::reactor::{
    FullscreenSpaceTrack, FullscreenWindowTrack, LayoutEvent, Reactor, SpaceEventKind,
    StaleCleanupState,
};
use crate::actor::spaces::{ForwardedSpaceState, TopologyWindowDelta};
use crate::actor::wm_controller::WmEvent;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::app::AppInfo;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

pub struct SpaceEventHandler;

impl SpaceEventHandler {
    pub fn handle_space_state_changed(reactor: &mut Reactor, space_state: ForwardedSpaceState) {
        let pending_space_state = space_state.clone();
        let ForwardedSpaceState {
            screens,
            fullscreen_spaces,
            has_seen_display_set,
            active_spaces,
            menu_bar_space,
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
            releases_lifecycle_refresh_quarantine: _,
            resized_spaces,
            topology_window_delta,
        } = space_state;
        let cfg = reactor.activation_cfg();
        let spaces: Vec<Option<SpaceId>> = screens.iter().map(|screen| screen.space).collect();
        let display_uuids: Vec<Option<String>> =
            screens.iter().map(|screen| screen.display_uuid_owned()).collect();
        let authoritative_spaces: Vec<Option<SpaceId>> = screens
            .iter()
            .map(|screen| screen.space.filter(|space| active_spaces.contains(space)))
            .collect();
        let effective_active_spaces: HashSet<SpaceId> = reactor
            .space_activation_policy
            .compute_active_spaces(cfg, &authoritative_spaces, &display_uuids)
            .into_iter()
            .flatten()
            .collect();

        let command_space_only_update = !display_set_changed
            && !should_force_refresh_layout
            && space_remaps.is_empty()
            && resized_spaces.is_empty()
            && topology_window_delta.is_none()
            && reactor.space_state.screens == screens
            && reactor.space_state.fullscreen_spaces == fullscreen_spaces
            && reactor.active_spaces == effective_active_spaces
            && reactor.space_state.display_space_ids == display_space_ids
            && reactor.space_state.last_user_space_by_display == last_user_space_by_display;

        reactor.space_state.has_seen_display_set = has_seen_display_set;
        reactor.space_state.fullscreen_spaces = fullscreen_spaces;
        reactor.space_state.active_spaces = active_spaces.clone();
        let topology_invalidates_pending_targets = display_set_changed
            || should_force_refresh_layout
            || !space_remaps.is_empty()
            || !resized_spaces.is_empty()
            || topology_window_delta.is_some();

        if command_space_only_update {
            reactor.space_state.menu_bar_space = menu_bar_space;
            reactor.space_state.command_space = command_space;
            return;
        }

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
        reactor.space_state.menu_bar_space = menu_bar_space;
        reactor.space_state.command_space = command_space;
        reactor.space_state.display_space_ids = display_space_ids;
        reactor.space_state.last_user_space_by_display = last_user_space_by_display;

        if topology_invalidates_pending_targets {
            reactor.clear_pending_hidden_window_targets();
        }

        if reactor.is_mission_control_active() {
            reactor.pending_space_change_manager.pending_space_change = Some(pending_space_state);
            return;
        }

        for (previous_space, space) in space_remaps {
            reactor.layout_manager.layout_engine.remap_space(previous_space, space);
        }
        for screen in &reactor.space_state.screens {
            let (Some(space), Some(display_uuid)) = (screen.space, screen.display_uuid_opt())
            else {
                continue;
            };
            reactor
                .layout_manager
                .layout_engine
                .update_space_display(space, Some(display_uuid.to_string()));
        }
        let current_screens = reactor.screens_for_current_spaces();
        reactor.space_activation_policy.on_spaces_updated(cfg, &current_screens);
        reactor.recompute_and_set_active_spaces(&authoritative_spaces);
        reactor.restore_windows_after_fullscreen_exit(&spaces);

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

        if let Some(topology_window_delta) = topology_window_delta {
            apply_topology_window_delta(reactor, topology_window_delta);
        }

        let active_windows = reactor.authoritative_active_space_windows();
        reactor.finalize_space_change(&spaces, active_windows);
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
    // this is functionally a connection dropping to the window server
    pub fn handle_window_server_destroyed(
        reactor: &mut Reactor,
        wsid: WindowServerId,
        sid: SpaceId,
        kind: SpaceEventKind,
    ) {
        if matches!(kind, SpaceEventKind::Fullscreen) {
            let mut layout_changed = false;
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
            record_fullscreen_window(
                reactor,
                sid,
                pid,
                window_id,
                Some(wsid),
                last_known_user_space,
            );
            if let (Some(wid), Some(user_space)) = (window_id, last_known_user_space)
                && reactor.assigned_space_for_window_id(wid) == Some(user_space)
            {
                reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                layout_changed = reactor.is_space_active(user_space);
            }
            if layout_changed && !reactor.is_mission_control_active() {
                let _ = reactor.update_layout_or_warn(false, false);
            }

            if let Some(wid) = window_id
                && let Some(app_state) = reactor.app_manager.apps.get(&wid.pid)
            {
                if let Err(e) = app_state.handle.send(Request::WindowMaybeDestroyed(wid)) {
                    warn!("Failed to send WindowMaybeDestroyed: {}", e);
                }
            }

            return;
        } else if matches!(kind, SpaceEventKind::User) {
            if let Some(current_space) = crate::sys::window_server::window_space(wsid)
                && current_space != sid
            {
                debug!(?wsid, reported_space = ?sid, ?current_space, "Ignoring stale user-space disappearance due to authoritative current space");
                return;
            }
            if reactor.iter_active_spaces().nth(1).is_some()
                && reactor.window_manager.is_window_visible(wsid)
                && let Some(wid) = reactor.window_manager.tracked_window_id(wsid)
                && reactor.hidden_assigned_space_for_window_id(wid).is_none()
                && reactor
                    .assigned_space_for_window_id(wid)
                    .is_some_and(|assigned| assigned != sid)
            {
                debug!(
                    ?wid,
                    ?wsid,
                    reported_space = ?sid,
                    assigned_space = ?reactor.assigned_space_for_window_id(wid),
                    "Ignoring user-space disappearance that conflicts with visible multi-display assignment"
                );
                return;
            }
            if let Some(wid) = reactor.window_manager.tracked_window_id(wsid)
                && reactor.should_ignore_conflicting_user_space_event(wid, sid)
            {
                debug!(
                    ?wid,
                    ?wsid,
                    reported_space = ?sid,
                    assigned_space = ?reactor.assigned_space_for_window_id(wid),
                    authoritative_space = ?reactor.authoritative_space_for_window_id(wid),
                    "Ignoring stale user-space disappearance for moved window"
                );
                return;
            }

            if let Some(wid) = reactor.window_manager.tracked_window_id(wsid) {
                if !crate::sys::window_server::window_is_ordered_in(wsid) {
                    // since the connection has dropped it wont be shown in space_windows_list
                    // so ordered in can be authorative because it doesnt consider
                    // ghost windows that sometimes remain
                    debug!(
                        ?wid,
                        ?wsid,
                        reported_space = ?sid,
                        "Promoting WindowServer disappearance to immediate WindowDestroyed"
                    );
                    let _ = WindowEventHandler::handle_window_destroyed(reactor, wid);
                    return;
                }

                reactor.window_manager.set_window_server_space(wsid, Some(sid));
                reactor.window_manager.mark_window_hidden(wsid);
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
                reactor.window_manager.set_window_server_space(wsid, Some(sid));
                reactor.window_manager.mark_window_hidden(wsid);
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
        if matches!(kind, SpaceEventKind::User)
            && let Some(current_space) = crate::sys::window_server::window_space(wsid)
            && current_space != sid
        {
            debug!(?wsid, reported_space = ?sid, ?current_space, "Ignoring stale user-space appearance due to authoritative current space");
            return;
        }
        if matches!(kind, SpaceEventKind::User)
            && reactor.iter_active_spaces().nth(1).is_some()
            && reactor.window_manager.is_window_visible(wsid)
            && let Some(wid) = reactor.window_manager.tracked_window_id(wsid)
            && reactor.hidden_assigned_space_for_window_id(wid).is_none()
            && reactor
                .assigned_space_for_window_id(wid)
                .is_some_and(|assigned| assigned != sid)
        {
            debug!(
                ?wid,
                ?wsid,
                reported_space = ?sid,
                assigned_space = ?reactor.assigned_space_for_window_id(wid),
                "Ignoring user-space appearance that conflicts with visible multi-display assignment"
            );
            return;
        }

        if matches!(kind, SpaceEventKind::User)
            && let Some(wid) = reactor.window_manager.tracked_window_id(wsid)
            && reactor.should_ignore_conflicting_user_space_event(wid, sid)
        {
            debug!(
                ?wid,
                ?wsid,
                reported_space = ?sid,
                assigned_space = ?reactor.assigned_space_for_window_id(wid),
                authoritative_space = ?reactor.authoritative_space_for_window_id(wid),
                "Ignoring stale user-space appearance for moved window"
            );
            return;
        }

        if matches!(kind, SpaceEventKind::User) {
            reactor.window_manager.set_window_server_space(wsid, Some(sid));
            reactor.window_manager.mark_window_visible(wsid);
            reactor.clear_pending_target_if_confirmed_space(wsid, sid);
        }

        if reactor.window_manager.knows_window_server_id(wsid)
            || reactor.window_manager.is_window_server_observed(wsid)
        {
            if !reactor.is_mission_control_active() {
                match kind {
                    SpaceEventKind::User => {
                        if let Some(wid) = reactor.window_manager.tracked_window_id(wsid) {
                            let layout_changed =
                                restore_fullscreen_window_to_user_space(reactor, wsid, sid, wid)
                                    .unwrap_or_else(|| {
                                        reactor.reassign_window_to_authoritative_space(wid, sid)
                                    });
                            if layout_changed {
                                let _ = reactor.update_layout_or_warn(false, false);
                            }
                        }
                    }
                    SpaceEventKind::Fullscreen => {
                        let mut layout_changed = false;
                        if let Some(wid) = reactor.window_manager.tracked_window_id(wsid) {
                            let last_known_user_space =
                                resolve_last_known_user_space(reactor, Some(wid));
                            record_fullscreen_window(
                                reactor,
                                sid,
                                wid.pid,
                                Some(wid),
                                Some(wsid),
                                last_known_user_space,
                            );
                            if let Some(user_space) = last_known_user_space
                                && reactor.assigned_space_for_window_id(wid) == Some(user_space)
                            {
                                reactor.send_layout_event(
                                    LayoutEvent::WindowRemovedPreserveFloating(wid),
                                );
                                layout_changed = reactor.is_space_active(user_space);
                            }
                        }
                        if layout_changed {
                            let _ = reactor.update_layout_or_warn(false, false);
                        }
                    }
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
                    Some(wsid),
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
    window_id: Option<WindowId>,
) -> Option<SpaceId> {
    window_id
        .and_then(|wid| reactor.best_space_for_window_id(wid))
        .or_else(|| reactor.space_state.iter_known_spaces().next())
}

fn record_fullscreen_window(
    reactor: &mut Reactor,
    sid: SpaceId,
    pid: i32,
    window_id: Option<WindowId>,
    window_server_id: Option<WindowServerId>,
    last_known_user_space: Option<SpaceId>,
) {
    let entry = match reactor.native_fullscreen_tracks.entry(sid.get()) {
        Entry::Occupied(o) => o.into_mut(),
        Entry::Vacant(v) => v.insert(FullscreenSpaceTrack::default()),
    };

    if let Some(existing) = entry.windows.iter_mut().find(|window| {
        window_server_id.is_some() && window.window_server_id == window_server_id
            || window_id.is_some() && window.window_id == window_id
    }) {
        if existing.window_id.is_none() {
            existing.window_id = window_id;
        }
        if existing.window_server_id.is_none() {
            existing.window_server_id = window_server_id;
        }
        if existing.last_known_user_space.is_none() {
            existing.last_known_user_space = last_known_user_space;
        }
        existing._last_seen_fullscreen_space = sid;
        return;
    }

    entry.windows.push(FullscreenWindowTrack {
        pid,
        window_id,
        window_server_id,
        last_known_user_space,
        _last_seen_fullscreen_space: sid,
    });
}

fn apply_topology_window_delta(reactor: &mut Reactor, delta: TopologyWindowDelta) {
    let appeared_by_wsid: HashMap<WindowServerId, SpaceId> = delta.appeared.into_iter().collect();
    let disappeared_by_wsid: HashMap<WindowServerId, SpaceId> =
        delta.disappeared.into_iter().collect();
    let wsids: HashSet<WindowServerId> =
        appeared_by_wsid.keys().chain(disappeared_by_wsid.keys()).copied().collect();

    for wsid in wsids {
        let appeared_space = appeared_by_wsid.get(&wsid).copied();
        let disappeared_space = disappeared_by_wsid.get(&wsid).copied();
        let authoritative_space =
            appeared_space.or_else(|| crate::sys::window_server::window_space(wsid));

        if let Some(target_space) = authoritative_space {
            reactor.window_manager.set_window_server_space(wsid, Some(target_space));
            if reactor.is_space_active(target_space) {
                reactor.window_manager.mark_window_visible(wsid);
            } else {
                reactor.window_manager.mark_window_hidden(wsid);
            }

            if let Some(wid) = reactor.window_manager.tracked_window_id(wsid) {
                let _ = restore_fullscreen_window_to_user_space(reactor, wsid, target_space, wid)
                    .unwrap_or_else(|| {
                        reactor.reassign_window_to_authoritative_space(wid, target_space)
                    });
            }
            continue;
        }

        if let Some(previous_space) = disappeared_space {
            reactor.window_manager.set_window_server_space(wsid, Some(previous_space));
            reactor.window_manager.mark_window_hidden(wsid);
            if let Some(wid) = reactor.window_manager.tracked_window_id(wsid)
                && reactor.assigned_space_for_window_id(wid) == Some(previous_space)
                && reactor.is_space_active(previous_space)
            {
                reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
            }
        }
    }
}

fn restore_fullscreen_window_to_user_space(
    reactor: &mut Reactor,
    wsid: WindowServerId,
    sid: SpaceId,
    wid: WindowId,
) -> Option<bool> {
    // Restore only an exact fullscreen track. A pid can own several real
    // windows, and Electron can create multiple AX ids for one lifecycle, so a
    // pid-level match can revive the wrong same-app window into the original
    // workspace as a layout-only ghost.
    let exact_match = reactor.native_fullscreen_tracks.iter().find_map(|(&key, track)| {
        track
            .windows
            .iter()
            .find(|window| window.window_server_id == Some(wsid) || window.window_id == Some(wid))
            .cloned()
            .map(|window| (key, window))
    });
    let (matched_key, restored_window) = exact_match?;
    for (&key, track) in &mut reactor.native_fullscreen_tracks {
        track.windows.retain(|window| {
            let same_window_server_id = restored_window.window_server_id.is_some()
                && window.window_server_id == restored_window.window_server_id;
            let same_window_id = restored_window.window_id.is_some()
                && window.window_id == restored_window.window_id;
            if key == matched_key {
                return !(same_window_server_id || same_window_id);
            }

            !(same_window_server_id || same_window_id)
        });
    }
    reactor.native_fullscreen_tracks.retain(|_, track| !track.windows.is_empty());

    let owner_window = restored_window
        .window_id
        .filter(|candidate| reactor.window_manager.contains_window(*candidate))
        .or_else(|| {
            restored_window
                .window_server_id
                .and_then(|tracked_wsid| reactor.window_manager.tracked_window_id(tracked_wsid))
        })
        .or_else(|| reactor.window_manager.tracked_window_id(wsid))
        .or_else(|| reactor.window_manager.contains_window(wid).then_some(wid))?;

    if owner_window != wid && reactor.assigned_space_for_window_id(wid).is_some() {
        reactor.send_layout_event(LayoutEvent::WindowRemoved(wid));
    }

    request_visible_windows(reactor, restored_window.pid, "refresh after fullscreen exit");

    Some(
        if reactor.assigned_space_for_window_id(owner_window) == Some(sid) {
            if reactor.is_space_active(sid) {
                reactor.restore_window_to_active_layout_if_visible(owner_window, sid)
            } else {
                false
            }
        } else {
            reactor.reassign_window_to_authoritative_space(owner_window, sid)
        },
    )
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
