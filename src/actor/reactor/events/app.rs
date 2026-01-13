use tracing::{debug, warn};

use crate::actor::app::{AppInfo, AppThreadHandle, Quiet, WindowId};
use crate::actor::reactor::{AppState, Reactor};
use crate::layout_engine::LayoutEvent;
use crate::sys::app::WindowInfo;
use crate::sys::window_server::{self as window_server, WindowServerId, WindowServerInfo};

pub struct AppEventHandler;

impl AppEventHandler {
    pub fn handle_application_launched(
        reactor: &mut Reactor,
        pid: i32,
        info: AppInfo,
        handle: AppThreadHandle,
        visible_windows: Vec<(WindowId, WindowInfo)>,
        window_server_info: Vec<WindowServerInfo>,
        _is_frontmost: bool,
        _main_window: Option<WindowId>,
    ) {
        reactor.app_manager.apps.insert(pid, AppState { info: info.clone(), handle });
        reactor.update_partial_window_server_info(window_server_info);
        reactor.on_windows_discovered_with_app_info(pid, visible_windows, vec![], Some(info));
    }

    pub fn handle_apply_app_rules_to_existing_windows(
        reactor: &mut Reactor,
        pid: i32,
        app_info: AppInfo,
        windows: Vec<WindowServerInfo>,
    ) {
        reactor.update_partial_window_server_info(windows.clone());

        let all_windows: Vec<WindowId> = windows
            .iter()
            .filter_map(|info| reactor.window_manager.window_ids.get(&info.id).copied())
            .filter(|wid| {
                reactor
                    .window_manager
                    .windows
                    .get(wid)
                    .map_or(false, |window| window.is_manageable)
            })
            .collect();

        if !all_windows.is_empty() {
            let wsids: Vec<WindowServerId> = windows.iter().map(|w| w.id).collect();
            reactor.app_manager.mark_wsids_recent(wsids);
            reactor.process_windows_for_app_rules(pid, all_windows, app_info);
        }
    }

    pub fn handle_application_terminated(reactor: &mut Reactor, pid: i32) {
        if let Some(app) = reactor.app_manager.apps.get_mut(&pid) {
            if let Err(e) = app.handle.send(crate::actor::app::Request::Terminate) {
                warn!("Failed to send Terminate to app {}: {}", pid, e);
            }
        }
    }

    pub fn handle_application_thread_terminated(reactor: &mut Reactor, pid: i32) {
        reactor.app_manager.apps.remove(&pid);
        reactor.send_layout_event(LayoutEvent::AppClosed(pid));
    }

    pub fn handle_resync_app_for_window(reactor: &mut Reactor, wsid: WindowServerId) {
        if let Some(&wid) = reactor.window_manager.window_ids.get(&wsid) {
            if let Some(app_state) = reactor.app_manager.apps.get(&wid.pid) {
                if let Err(e) = app_state
                    .handle
                    .send(crate::actor::app::Request::GetVisibleWindows)
                {
                    warn!("Failed to send GetVisibleWindows to app {}: {}", wid.pid, e);
                }
            }
        } else if let Some(info) = reactor
            .window_server_info_manager
            .window_server_info
            .get(&wsid)
            .cloned()
            .or_else(|| window_server::get_window(wsid))
        {
            if let Some(app_state) = reactor.app_manager.apps.get(&info.pid) {
                if let Err(e) = app_state
                    .handle
                    .send(crate::actor::app::Request::GetVisibleWindows)
                {
                    warn!("Failed to send GetVisibleWindows to app {}: {}", info.pid, e);
                }
            }
        }
    }

    pub fn handle_application_activated(reactor: &mut Reactor, pid: i32, quiet: Quiet) {
        if quiet == Quiet::Yes {
            debug!(
                pid,
                "Skipping auto workspace switch for quiet app activation (initiated by Rift)"
            );
            return;
        }

        reactor.handle_app_activation_workspace_switch(pid);
    }

    pub fn handle_windows_discovered(
        reactor: &mut Reactor,
        pid: i32,
        new: Vec<(WindowId, WindowInfo)>,
        known_visible: Vec<WindowId>,
    ) {
        reactor.on_windows_discovered_with_app_info(pid, new, known_visible, None);
    }
}
