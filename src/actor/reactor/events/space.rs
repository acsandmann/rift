use std::collections::hash_map::Entry;

use objc2_app_kit::NSRunningApplication;
use tracing::{debug, info, trace, warn};

use crate::actor::app::Request;
use crate::actor::reactor::{
    Event, FullscreenTrack, MissionControlState, PendingSpaceChange, Reactor, Screen,
    ScreenSnapshot, StaleCleanupState,
};
use crate::actor::wm_controller::WmEvent;
use crate::sys::app::AppInfo;
use crate::sys::screen::{ScreenId, SpaceId};
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

pub struct SpaceEventHandler;

impl SpaceEventHandler {
    pub fn handle_window_server_destroyed(
        reactor: &mut Reactor,
        wsid: WindowServerId,
        sid: SpaceId,
    ) {
        if crate::sys::window_server::space_is_fullscreen(sid.get()) {
            let entry = match reactor.space_manager.fullscreen_by_space.entry(sid.get()) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert(FullscreenTrack::default()),
            };
            if let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) {
                entry.pids.insert(wid.pid);
                if entry.last_removed.len() >= 5 {
                    entry.last_removed.pop_front();
                }
                entry.last_removed.push_back(wsid);
                if let Some(app_state) = reactor.app_manager.apps.get(&wid.pid) {
                    if let Err(e) =
                        app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]))
                    {
                        warn!("Failed to send MarkWindowsNeedingInfo: {}", e);
                    }
                }
                return;
            } else if let Some(info) =
                reactor.window_server_info_manager.window_server_info.get(&wsid)
            {
                entry.pids.insert(info.pid);
                if entry.last_removed.len() >= 5 {
                    entry.last_removed.pop_front();
                }
                entry.last_removed.push_back(wsid);
                return;
            }
            return;
        } else if crate::sys::window_server::space_is_user(sid.get()) {
            if let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) {
                reactor.window_manager.window_ids.remove(&wsid);
                reactor.window_server_info_manager.window_server_info.remove(&wsid);
                reactor.window_manager.visible_windows.remove(&wsid);
                if let Some(app_state) = reactor.app_manager.apps.get(&wid.pid) {
                    if let Err(e) =
                        app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]))
                    {
                        warn!("Failed to send MarkWindowsNeedingInfo: {}", e);
                    }
                    let _ =
                        app_state.handle.send(Request::GetVisibleWindows { force_refresh: true });
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
                let entry = match reactor.space_manager.fullscreen_by_space.entry(sid.get()) {
                    Entry::Occupied(o) => o.into_mut(),
                    Entry::Vacant(v) => v.insert(FullscreenTrack::default()),
                };
                entry.pids.insert(window_server_info.pid);
                if entry.last_removed.len() >= 5 {
                    entry.last_removed.pop_front();
                }
                entry.last_removed.push_back(wsid);
                if let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) {
                    if let Some(app_state) = reactor.app_manager.apps.get(&wid.pid) {
                        if let Err(e) =
                            app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]))
                        {
                            warn!("Failed to send MarkWindowsNeedingInfo: {}", e);
                        }
                    }
                } else if let Some(app_state) =
                    reactor.app_manager.apps.get(&window_server_info.pid)
                {
                    let resync: Vec<_> = reactor
                        .window_manager
                        .windows
                        .keys()
                        .copied()
                        .filter(|wid| wid.pid == window_server_info.pid)
                        .collect();
                    if !resync.is_empty() {
                        if let Err(e) =
                            app_state.handle.send(Request::MarkWindowsNeedingInfo(resync))
                        {
                            warn!("Failed to send MarkWindowsNeedingInfo: {}", e);
                        }
                    }
                }
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
                if let Err(err) =
                    app.handle.send(Request::GetVisibleWindows { force_refresh: false })
                {
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
        screens: Vec<ScreenSnapshot>,
        ws_info: Vec<WindowServerInfo>,
    ) {
        let spaces: Vec<Option<SpaceId>> = screens.iter().map(|s| s.space).collect();
        let spaces_all_none = spaces.iter().all(|space| space.is_none());
        reactor.refocus_manager.stale_cleanup_state = if spaces_all_none {
            StaleCleanupState::Suppressed
        } else {
            StaleCleanupState::Enabled
        };
        let mut ws_info_opt = Some(ws_info);
        if screens.is_empty() {
            if !reactor.space_manager.screens.is_empty() {
                reactor.space_manager.screens.clear();
                reactor.space_manager.screen_space_by_id.clear();
                reactor.expose_all_spaces();
            }
        } else {
            reactor.space_manager.screens = screens
                .into_iter()
                .map(|snapshot| Screen {
                    frame: snapshot.frame,
                    space: snapshot.space,
                    display_uuid: snapshot.display_uuid,
                    name: snapshot.name,
                    screen_id: ScreenId::new(snapshot.screen_id),
                })
                .collect();
            reactor.update_screen_space_map();
            if let Some(info) = ws_info_opt.take() {
                reactor.finalize_space_change(&spaces, info);
            }
        }
        if let Some(info) = ws_info_opt.take() {
            reactor.update_complete_window_server_info(info);
        }
        reactor.try_apply_pending_space_change();
    }

    pub fn handle_space_changed(
        reactor: &mut Reactor,
        mut spaces: Vec<Option<SpaceId>>,
        ws_info: Vec<WindowServerInfo>,
    ) {
        // TODO: this logic is flawed if multiple spaces are changing at once
        if reactor.handle_fullscreen_space_transition(&mut spaces) {
            return;
        }
        if matches!(
            reactor.mission_control_manager.mission_control_state,
            MissionControlState::Active
        ) {
            // dont process whilst mc is active
            reactor.pending_space_change_manager.pending_space_change =
                Some(PendingSpaceChange { spaces, ws_info });
            return;
        }
        let spaces_all_none = spaces.iter().all(|space| space.is_none());
        reactor.refocus_manager.stale_cleanup_state = if spaces_all_none {
            StaleCleanupState::Suppressed
        } else {
            StaleCleanupState::Enabled
        };
        if spaces_all_none {
            if spaces.len() == reactor.space_manager.screens.len() {
                reactor.set_screen_spaces(&spaces);
            }
            return;
        }
        if spaces.len() != reactor.space_manager.screens.len() {
            warn!(
                "Deferring space change: have {} screens but {} spaces",
                reactor.space_manager.screens.len(),
                spaces.len()
            );
            reactor.pending_space_change_manager.pending_space_change =
                Some(PendingSpaceChange { spaces, ws_info });
            return;
        }
        info!("space changed");
        reactor.pending_space_change_manager.pending_space_change = None;
        reactor.set_screen_spaces(&spaces);
        reactor.finalize_space_change(&spaces, ws_info);
    }

    pub fn handle_mission_control_native_entered(reactor: &mut Reactor) {
        reactor.set_mission_control_active(true);
    }

    pub fn handle_mission_control_native_exited(reactor: &mut Reactor) {
        if matches!(
            reactor.mission_control_manager.mission_control_state,
            MissionControlState::Active
        ) {
            reactor.set_mission_control_active(false);
        }
        reactor.refresh_windows_after_mission_control();
    }
}
