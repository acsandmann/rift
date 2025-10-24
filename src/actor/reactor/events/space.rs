use std::collections::hash_map::Entry;

use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::CGRect;
use tracing::{debug, info, trace, warn};

use crate::actor::app::{AppInfo, Request};
use crate::actor::reactor::{
    DragState, Event, FullscreenTrack, MissionControlState, PendingSpaceChange, Reactor, Screen,
    StaleCleanupState,
};
use crate::actor::wm_controller::WmEvent;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

pub struct SpaceEventHandler;

impl SpaceEventHandler {
    pub fn handle_window_is_changing_screens(reactor: &mut Reactor, wsid: WindowServerId) {
        reactor.changing_screens.insert(wsid);
        if let DragState::PendingSwap { dragged, target } =
            std::mem::replace(&mut reactor.drag_state, DragState::Inactive)
        {
            trace!(
                ?dragged,
                ?target,
                ?wsid,
                "Clearing pending drag swap; window is moving between spaces"
            );
            if reactor.skip_layout_for_window == Some(dragged) {
                reactor.skip_layout_for_window = None;
            }
        }
        reactor.drag_manager.reset();
        reactor.drag_state = DragState::Inactive;
        // finalize_active_drag will set to Inactive, but since we're starting a new drag, ensure_active_drag will set to Active
        if let Some(&wid) = reactor.window_ids.get(&wsid) {
            if let Some(frame) = reactor.windows.get(&wid).map(|window| window.frame_monotonic) {
                reactor.ensure_active_drag(wid, &frame);
            }
        }
    }

    pub fn handle_window_server_destroyed(
        reactor: &mut Reactor,
        wsid: WindowServerId,
        sid: SpaceId,
    ) {
        if crate::sys::window_server::space_is_fullscreen(sid.get()) {
            let entry = match reactor.fullscreen_by_space.entry(sid.get()) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert(FullscreenTrack::default()),
            };
            if let Some(&wid) = reactor.window_ids.get(&wsid) {
                entry.pids.insert(wid.pid);
                if entry.last_removed.len() >= 5 {
                    let _ = entry.last_removed.pop_front();
                }
                entry.last_removed.push_back(wsid);
                if let Some(app_state) = reactor.apps.get(&wid.pid) {
                    let _ = app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]));
                }
                return;
            } else if let Some(info) = reactor.window_server_info.get(&wsid) {
                entry.pids.insert(info.pid);
                if entry.last_removed.len() >= 5 {
                    let _ = entry.last_removed.pop_front();
                }
                entry.last_removed.push_back(wsid);
                return;
            }
            return;
        } else if crate::sys::window_server::space_is_user(sid.get()) {
            if let Some(&wid) = reactor.window_ids.get(&wsid) {
                let _ = reactor.window_ids.remove(&wsid);
                reactor.window_server_info.remove(&wsid);
                reactor.visible_windows.remove(&wsid);
                if let Some(app_state) = reactor.apps.get(&wid.pid) {
                    let _ = app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]));
                    let _ =
                        app_state.handle.send(Request::GetVisibleWindows { force_refresh: true });
                }
                if let Some(tx) = reactor.events_tx.as_ref() {
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
        if reactor.window_server_info.contains_key(&wsid)
            || reactor.observed_window_server_ids.contains(&wsid)
        {
            debug!(
                ?wsid,
                "Received WindowServerAppeared for known window - ignoring"
            );
            return;
        }

        reactor.observed_window_server_ids.insert(wsid);
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

            if crate::sys::window_server::space_is_fullscreen(sid.get()) {
                let entry = match reactor.fullscreen_by_space.entry(sid.get()) {
                    Entry::Occupied(o) => o.into_mut(),
                    Entry::Vacant(v) => v.insert(FullscreenTrack::default()),
                };
                entry.pids.insert(window_server_info.pid);
                if entry.last_removed.len() >= 5 {
                    let _ = entry.last_removed.pop_front();
                }
                entry.last_removed.push_back(wsid);
                if let Some(&wid) = reactor.window_ids.get(&wsid) {
                    if let Some(app_state) = reactor.apps.get(&wid.pid) {
                        let _ = app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]));
                    }
                } else if let Some(app_state) = reactor.apps.get(&window_server_info.pid) {
                    let resync: Vec<_> = reactor
                        .windows
                        .keys()
                        .copied()
                        .filter(|wid| wid.pid == window_server_info.pid)
                        .collect();
                    if !resync.is_empty() {
                        let _ = app_state.handle.send(Request::MarkWindowsNeedingInfo(resync));
                    }
                }
                return;
            }

            reactor.update_partial_window_server_info(vec![window_server_info]);

            if !reactor.apps.contains_key(&window_server_info.pid) {
                if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(
                    window_server_info.pid,
                ) {
                    debug!(
                        ?app,
                        "Received WindowServerAppeared for unknown app - synthesizing AppLaunch"
                    );
                    reactor.wm_sender.as_ref().map(|wm| {
                        wm.send(WmEvent::AppLaunch(window_server_info.pid, AppInfo::from(&*app)))
                    });
                }
            } else if let Some(app) = reactor.apps.get(&window_server_info.pid) {
                if let Err(err) =
                    app.handle.send(Request::GetVisibleWindows { force_refresh: false })
                {
                    debug!(
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
        frames: Vec<CGRect>,
        spaces: Vec<Option<SpaceId>>,
        ws_info: Vec<WindowServerInfo>,
    ) {
        info!("screen parameters changed");
        let spaces_all_none = spaces.iter().all(|space| space.is_none());
        reactor.stale_cleanup_state = if spaces_all_none {
            StaleCleanupState::Suppressed
        } else {
            StaleCleanupState::Enabled
        };
        let mut ws_info_opt = Some(ws_info);
        if frames.is_empty() {
            if spaces.is_empty() {
                if !reactor.screens.is_empty() {
                    reactor.screens.clear();
                    reactor.expose_all_spaces();
                }
            } else if spaces.len() == reactor.screens.len() {
                reactor.set_screen_spaces(&spaces);
                if let Some(info) = ws_info_opt.take() {
                    reactor.finalize_space_change(&spaces, info);
                }
            } else {
                warn!(
                    "Ignoring empty screen update: we have {} screens, but {} spaces",
                    reactor.screens.len(),
                    spaces.len()
                );
            }
        } else if frames.len() != spaces.len() {
            warn!(
                "Ignoring screen update: got {} frames but {} spaces",
                frames.len(),
                spaces.len()
            );
        } else {
            let spaces_clone = spaces.clone();
            reactor.screens = frames
                .into_iter()
                .zip(spaces.into_iter())
                .map(|(frame, space)| Screen { frame, space })
                .collect();
            if let Some(info) = ws_info_opt.take() {
                reactor.finalize_space_change(&spaces_clone, info);
            }
        }
        if let Some(info) = ws_info_opt.take() {
            reactor.update_complete_window_server_info(info);
        }
        reactor.try_apply_pending_space_change();
    }

    pub fn handle_space_changed(
        reactor: &mut Reactor,
        spaces: Vec<Option<SpaceId>>,
        ws_info: Vec<WindowServerInfo>,
    ) {
        // TODO: this logic is flawed if multiple spaces are changing at once
        if reactor.handle_fullscreen_space_transition(&spaces) {
            return;
        }
        if matches!(reactor.mission_control_state, MissionControlState::Active) {
            // dont process whilst mc is active
            reactor.pending_space_change = Some(PendingSpaceChange { spaces, ws_info });
            return;
        }
        let spaces_all_none = spaces.iter().all(|space| space.is_none());
        reactor.stale_cleanup_state = if spaces_all_none {
            StaleCleanupState::Suppressed
        } else {
            StaleCleanupState::Enabled
        };
        if spaces.len() != reactor.screens.len() {
            warn!(
                "Deferring space change: have {} screens but {} spaces",
                reactor.screens.len(),
                spaces.len()
            );
            reactor.pending_space_change = Some(PendingSpaceChange { spaces, ws_info });
            return;
        }
        info!("space changed");
        reactor.pending_space_change = None;
        reactor.set_screen_spaces(&spaces);
        reactor.finalize_space_change(&spaces, ws_info);
    }

    pub fn handle_mission_control_native_entered(reactor: &mut Reactor) {
        reactor.set_mission_control_active(true);
    }

    pub fn handle_mission_control_native_exited(reactor: &mut Reactor) {
        if matches!(reactor.mission_control_state, MissionControlState::Active) {
            reactor.set_mission_control_active(false);
        }
        reactor.refresh_windows_after_mission_control();
    }
}
