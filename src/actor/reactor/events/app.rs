use tracing::{debug, warn};

use crate::actor::app::{AppInfo, AppThreadHandle, Quiet, WindowId};
use crate::actor::reactor::{AppState, Reactor};
use crate::layout_engine::LayoutEvent;
use crate::sys::app::WindowInfo;
use crate::sys::window_server::WindowServerInfo;

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
