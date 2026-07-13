use tracing::debug;

use crate::actor::app::{AppInfo, AppThreadHandle, Quiet, WindowId};
use crate::actor::reactor::AppState;
use crate::actor::reactor::events::{EventOutcome, WindowDiscoveryRequest};
use crate::actor::reactor::managers::AppManager;
use crate::layout_engine::LayoutEvent;
use crate::sys::app::WindowInfo;
use crate::sys::window_server::WindowServerInfo;

#[derive(Debug)]
pub struct ApplicationLaunchedPayload {
    pub pid: i32,
    pub info: AppInfo,
    pub handle: AppThreadHandle,
    pub visible_windows: Vec<(WindowId, WindowInfo)>,
    pub window_server_info: Vec<WindowServerInfo>,
}

pub fn handle_application_launched(
    apps: &mut AppManager,
    payload: ApplicationLaunchedPayload,
) -> anyhow::Result<EventOutcome> {
    let ApplicationLaunchedPayload {
        pid,
        info,
        handle,
        visible_windows,
        window_server_info,
    } = payload;
    apps.apps.insert(pid, AppState { info: info.clone(), handle });
    Ok(EventOutcome::finalized_event(None, false, false, true)
        .with_window_server_updates(window_server_info)
        .with_discovery(WindowDiscoveryRequest {
            pid,
            new: visible_windows,
            known_visible: Vec::new(),
            app_info: Some(info),
        }))
}

pub fn handle_application_terminated(pid: i32) -> anyhow::Result<EventOutcome> {
    Ok(EventOutcome::finalized_event(None, false, false, true)
        .with_app_request(pid, crate::actor::app::Request::Terminate))
}

pub fn handle_application_thread_terminated(
    apps: &mut AppManager,
    pid: i32,
) -> anyhow::Result<EventOutcome> {
    apps.apps.remove(&pid);
    Ok(EventOutcome::finalized_event(None, false, false, true)
        .with_layout_event(LayoutEvent::AppClosed(pid)))
}

#[derive(Debug, Clone, Copy)]
pub struct ApplicationActivatedPayload {
    pub pid: i32,
    pub quiet: Quiet,
}

pub fn handle_application_activated(
    payload: ApplicationActivatedPayload,
) -> anyhow::Result<EventOutcome> {
    let ApplicationActivatedPayload { pid, quiet } = payload;
    if quiet == Quiet::Yes {
        debug!(
            pid,
            "Skipping auto workspace switch for quiet app activation (initiated by Rift)"
        );
        return Ok(EventOutcome::finalized_event(None, false, false, false));
    }

    Ok(EventOutcome::finalized_event(None, false, false, false).with_application_activation(pid))
}

#[derive(Debug)]
pub struct WindowsDiscoveredPayload {
    pub pid: i32,
    pub new: Vec<(WindowId, WindowInfo)>,
    pub known_visible: Vec<WindowId>,
}

pub fn handle_windows_discovered(
    payload: WindowsDiscoveredPayload,
) -> anyhow::Result<EventOutcome> {
    let WindowsDiscoveredPayload { pid, new, known_visible } = payload;
    Ok(
        EventOutcome::finalized_event(None, false, false, true).with_discovery(
            WindowDiscoveryRequest {
                pid,
                new,
                known_visible,
                app_info: None,
            },
        ),
    )
}
