use std::collections::hash_map::Entry;

use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::CGSize;
use tracing::{debug, info, trace, warn};

use crate::actor::app::Request;
use crate::actor::reactor::{
    Event, FullscreenSpaceTrack, FullscreenWindowTrack, LayoutEvent, PendingSpaceChange, Reactor,
    ScreenInfo, StaleCleanupState,
};
use crate::actor::wm_controller::WmEvent;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::app::AppInfo;
use crate::sys::screen::{ScreenId, SpaceId};
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

pub struct SpaceEventHandler;

impl SpaceEventHandler {
    // spacewindowappeared/destroyed happen a lot when a display is connected/disconnected
    // since they are literally when a window enters or leaves a space and each display has its own space(s)
    pub fn handle_window_server_destroyed(
        reactor: &mut Reactor,
        wsid: WindowServerId,
        sid: SpaceId,
    ) {
        if crate::sys::window_server::space_is_fullscreen(sid.get()) {
            let (pid, window_id) = if let Some(&wid) = reactor.window_manager.window_ids.get(&wsid)
            {
                (wid.pid, Some(wid))
            } else if let Some(info) =
                reactor.window_server_info_manager.window_server_info.get(&wsid)
            {
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
        } else if crate::sys::window_server::space_is_user(sid.get()) {
            if let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) {
                reactor.window_manager.window_ids.remove(&wsid);
                reactor.window_server_info_manager.window_server_info.remove(&wsid);
                reactor.window_manager.visible_windows.remove(&wsid);
                if let Some(app_state) = reactor.app_manager.apps.get(&wid.pid) {
                    if let Err(e) = app_state.handle.send(Request::WindowMaybeDestroyed(wid)) {
                        warn!("Failed to send WindowMaybeDestroyed: {}", e);
                    }
                }
                if let Some(tx) = reactor.communication_manager.events_tx.as_ref() {
                    tx.send(Event::WindowDestroyed(wid));
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
    ) {
        if reactor.window_server_info_manager.window_server_info.contains_key(&wsid)
            || reactor.window_manager.observed_window_server_ids.contains(&wsid)
        {
            debug!(
                ?wsid,
                "Received WindowServerAppeared for known window - ignoring"
            );
            return;
        }

        reactor.window_manager.observed_window_server_ids.insert(wsid);
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

            if crate::sys::window_server::space_is_fullscreen(sid.get()) {
                let window_id = reactor.window_manager.window_ids.get(&wsid).copied();
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

    pub fn handle_screen_parameters_changed(
        reactor: &mut Reactor,
        screens: Vec<ScreenInfo>,
        ws_info: Vec<WindowServerInfo>,
    ) {
        let previous_displays: HashSet<String> =
            reactor.space_manager.screens.iter().map(|s| s.display_uuid.clone()).collect();
        let new_displays: HashSet<String> =
            screens.iter().map(|s| s.display_uuid.clone()).collect();
        let displays_changed = previous_displays != new_displays;

        // IMPORTANT:
        // Only treat display UUID set changes as a "topology change" once we have a prior known set.
        // On startup (previous_displays is empty), we'll always see displays_changed=true, but that
        // should not trigger topology relayout pending. If it does, we can get stuck in a state where
        // SpaceChanged updates are suppressed/dropped around login window transitions.
        //
        // Once we've seen a non-empty display set, allow topology changes that pass through empty
        // (all displays unplugged/replugged).
        let should_trigger_topology = displays_changed
            && (reactor.space_manager.has_seen_display_set || !previous_displays.is_empty());

        if displays_changed {
            let active_list: Vec<String> = new_displays.iter().cloned().collect();
            reactor.layout_manager.layout_engine.prune_display_state(&active_list);
        }
        if !new_displays.is_empty() {
            reactor.space_manager.has_seen_display_set = true;
        }

        if screens.is_empty() {
            update_stale_cleanup_state(reactor, true);
            if !reactor.space_manager.screens.is_empty() {
                reactor.space_manager.screens.clear();
                reactor.expose_all_spaces();
            }

            reactor.recompute_and_set_active_spaces(&[]);
            reactor.update_complete_window_server_info(ws_info);
        } else {
            let spaces: Vec<Option<SpaceId>> = screens.iter().map(|s| s.space).collect();
            let previous_sizes: HashMap<ScreenId, CGSize> = reactor
                .space_manager
                .screens
                .iter()
                .map(|screen| (screen.id, screen.frame.size))
                .collect();
            reactor.space_manager.screens = screens;
            let resized_screens: HashSet<ScreenId> = reactor
                .space_manager
                .screens
                .iter()
                .filter_map(|screen| {
                    let new_size = screen.frame.size;
                    match previous_sizes.get(&screen.id) {
                        Some(previous) => {
                            let width_changed =
                                previous.width.round() as i32 != new_size.width.round() as i32;
                            let height_changed =
                                previous.height.round() as i32 != new_size.height.round() as i32;
                            if width_changed || height_changed {
                                Some(screen.id)
                            } else {
                                None
                            }
                        }
                        None => Some(screen.id),
                    }
                })
                .collect();

            let cfg = reactor.activation_cfg();
            // IMPORTANT: Do not reset login-window state here. When the lock screen / fast user
            // switching activates the login window, WM emits raw space snapshots and global
            // activation events. The activation policy must preserve the current login-window
            // flag across screen parameter changes so it can keep all spaces disabled while
            // login window is active.
            let screens = reactor.screens_for_current_spaces();
            reactor.space_activation_policy.on_spaces_updated(cfg, &screens);

            reactor.recompute_and_set_active_spaces(&spaces);

            // Do not remap layout state across reconnects; new space ids can churn and
            // remapping has caused windows to oscillate. Keep existing state and only
            // update the screen→space mapping.
            reactor.reconcile_spaces_with_display_history(&spaces, false);
            if !resized_screens.is_empty() {
                let resized_info: Vec<(SpaceId, CGSize)> = reactor
                    .space_manager
                    .screens
                    .iter()
                    .filter(|screen| resized_screens.contains(&screen.id))
                    .filter_map(|screen| screen.space.map(|s| (s, screen.frame.size)))
                    .collect();

                for (space, size) in resized_info {
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
            }
            reactor.finalize_space_change(&spaces, ws_info);
        }
        reactor.try_apply_pending_space_change();

        // Mark that we should perform a one-shot relayout after spaces are applied,
        // so windows return to their prior displays post-topology change.
        if should_trigger_topology {
            reactor.pending_space_change_manager.topology_relayout_pending = true;
        }
    }

    pub fn handle_space_changed(
        reactor: &mut Reactor,
        mut spaces: Vec<Option<SpaceId>>,
        ws_info: Vec<WindowServerInfo>,
    ) {
        // Also drop any space update that reports more spaces than screens; these are
        // transient and can reorder active workspaces across displays.
        if spaces.len() > reactor.space_manager.screens.len() {
            warn!(
                "Dropping oversize spaces vector (screens={}, spaces_len={})",
                reactor.space_manager.screens.len(),
                spaces.len()
            );
            return;
        }
        // If a topology change is in-flight, ignore space updates that don't match the
        // current screen count; wait for the matching vector before applying changes.
        if reactor.pending_space_change_manager.topology_relayout_pending
            && spaces.len() != reactor.space_manager.screens.len()
        {
            warn!(
                "Dropping space change during topology change (screens={}, spaces_len={})",
                reactor.space_manager.screens.len(),
                spaces.len()
            );
            return;
        }
        // TODO: this logic is flawed if multiple spaces are changing at once
        if reactor.handle_fullscreen_space_transition(&mut spaces) {
            return;
        }
        if reactor.is_mission_control_active() {
            // dont process whilst mc is active
            reactor.pending_space_change_manager.pending_space_change =
                Some(PendingSpaceChange { spaces, ws_info });
            return;
        }
        let spaces_all_none = spaces.iter().all(|space| space.is_none());
        if spaces_all_none {
            update_stale_cleanup_state(reactor, true);
            if spaces.len() == reactor.space_manager.screens.len() {
                reactor.set_screen_spaces(&spaces);
            }
            reactor.recompute_and_set_active_spaces(&spaces);
            return;
        }
        if spaces.len() != reactor.space_manager.screens.len() {
            warn!(
                "Ignoring space change: have {} screens but {} spaces",
                reactor.space_manager.screens.len(),
                spaces.len()
            );
            return;
        }

        update_stale_cleanup_state(reactor, false);

        let cfg = reactor.activation_cfg();
        let screens = reactor.screens_for_spaces(&spaces);
        reactor.space_activation_policy.on_spaces_updated(cfg, &screens);

        reactor.recompute_and_set_active_spaces(&spaces);

        reactor.reconcile_spaces_with_display_history(&spaces, false);
        info!("space changed");
        reactor.set_screen_spaces(&spaces);
        reactor.finalize_space_change(&spaces, ws_info);

        // If a topology change was detected earlier, perform a one-shot refresh/layout
        // now that we have a consistent space vector matching the screens.
        if reactor.pending_space_change_manager.topology_relayout_pending {
            reactor.pending_space_change_manager.topology_relayout_pending = false;
            reactor.force_refresh_all_windows();
            let _ = reactor.update_layout_or_warn_with(
                false,
                false,
                "Layout update failed after topology change",
            );
        }
    }

    pub fn handle_mission_control_native_entered(reactor: &mut Reactor) {
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
        .filter(|space| crate::sys::window_server::space_is_user(space.get()))
        .or_else(|| {
            reactor
                .space_manager
                .iter_known_spaces()
                .find(|space| crate::sys::window_server::space_is_user(space.get()))
        })
}

fn record_fullscreen_window(
    reactor: &mut Reactor,
    sid: SpaceId,
    pid: i32,
    window_id: Option<crate::actor::app::WindowId>,
    last_known_user_space: Option<SpaceId>,
) {
    let entry = match reactor.space_manager.fullscreen_by_space.entry(sid.get()) {
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
