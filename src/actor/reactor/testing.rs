use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tracing::debug;

use super::{Event, EventOutcome, Reactor, Record, Requested, ScreenInfo, TransactionId};
use crate::actor;
use crate::actor::app::{AppThreadHandle, Quiet, Request, WindowId};
use crate::actor::spaces::ForwardedSpaceState;
use crate::common::collections::BTreeMap;
use crate::common::config::Config;
use crate::layout_engine::{LayoutCommand, LayoutEngine};
use crate::sys::app::{AppInfo, WindowInfo, pid_t};
use crate::sys::geometry::SameAs;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

impl Reactor {
    pub fn new_for_test(layout: LayoutEngine) -> Reactor {
        let mut config = Config::default();
        config.settings.default_disable = false;
        config.settings.animate = false;
        let record = Record::new_for_test(tempfile::NamedTempFile::new().unwrap());
        let (broadcast_tx, _) = actor::channel();
        Reactor::new(config, layout, record, broadcast_tx, None, false)
    }

    pub fn handle_events(&mut self, events: Vec<Event>) {
        for event in events {
            self.handle_event(event);
        }
    }

    pub fn test_workspace_ids(
        &mut self,
        space: crate::sys::screen::SpaceId,
    ) -> Vec<crate::model::virtual_workspace::VirtualWorkspaceId> {
        self.layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .list_workspaces(space)
            .iter()
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn test_workspace(
        &mut self,
        space: crate::sys::screen::SpaceId,
        index: usize,
    ) -> crate::model::virtual_workspace::VirtualWorkspaceId {
        self.test_workspace_ids(space)[index]
    }

    pub fn test_workspace_for_window(
        &self,
        space: crate::sys::screen::SpaceId,
        wid: WindowId,
    ) -> Option<crate::model::virtual_workspace::VirtualWorkspaceId> {
        self.layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&self.state.windows, space, wid)
    }

    pub fn test_window_server_id(&self, wid: WindowId) -> WindowServerId {
        self.state
            .windows
            .window(wid)
            .and_then(|window| window.info.sys_id)
            .expect("test window should have a WindowServer identity")
    }

    pub fn test_active_workspace_windows(&self, space: SpaceId) -> Vec<WindowId> {
        self.layout_manager
            .layout_engine
            .windows_in_active_workspace(&self.state.windows, space)
    }

    pub fn test_workspace_windows(
        &self,
        space: SpaceId,
        workspace: crate::model::virtual_workspace::VirtualWorkspaceId,
    ) -> Vec<WindowId> {
        self.layout_manager.layout_engine.virtual_workspace_manager().workspace_windows(
            &self.state.windows,
            space,
            workspace,
        )
    }

    pub fn assign_test_window_to_workspace(
        &mut self,
        space: SpaceId,
        wid: WindowId,
        workspace: crate::model::virtual_workspace::VirtualWorkspaceId,
    ) -> bool {
        self.layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut self.state.windows, space, wid, workspace)
    }

    pub fn set_test_active_workspace(
        &mut self,
        space: SpaceId,
        workspace: crate::model::virtual_workspace::VirtualWorkspaceId,
    ) -> bool {
        self.layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .set_active_workspace(space, workspace)
    }

    pub fn handle_test_workspace_command(&mut self, space: SpaceId, command: &LayoutCommand) {
        let _ = self.layout_manager.layout_engine.handle_virtual_workspace_command(
            &mut self.state.windows,
            space,
            command,
        );
    }

    pub fn handle_test_layout_command(&mut self, command: LayoutCommand) {
        self.handle_event(Event::Command(crate::model::reactor::Command::Layout(command)));
    }

    pub(crate) fn dispatch_test_layout_command(&mut self, command: LayoutCommand) -> EventOutcome {
        self.dispatch_workflow(Event::Command(crate::model::reactor::Command::Layout(command)))
            .expect("test layout command should dispatch")
    }

    pub fn mark_test_window_visible_in_space(&mut self, wsid: WindowServerId, space: SpaceId) {
        self.state.windows.set_window_server_space(wsid, Some(space));
        self.state.windows.mark_window_visible(wsid);
    }

    pub fn discover_test_windows(
        &mut self,
        pid: pid_t,
        new: Vec<(WindowId, WindowInfo)>,
        known_visible: Vec<WindowId>,
    ) {
        self.handle_event(Event::WindowsDiscovered { pid, new, known_visible });
    }

    pub fn add_test_app(&mut self, pid: pid_t) {
        self.add_test_app_with_info(pid, "com.test.app", "Test App");
    }

    pub fn add_test_app_with_info(&mut self, pid: pid_t, bundle_id: &str, name: &str) {
        let (app_tx, _app_rx) = actor::channel();
        self.app_manager.apps.insert(pid, super::AppState {
            info: AppInfo {
                bundle_id: Some(bundle_id.to_string()),
                localized_name: Some(name.to_string()),
            },
            handle: AppThreadHandle::new_for_test(app_tx),
        });
    }

    pub fn add_test_window(
        &mut self,
        wid: WindowId,
        wsid: WindowServerId,
        space: Option<SpaceId>,
        frame: CGRect,
    ) {
        self.add_test_window_with_manageability(wid, wsid, space, frame, true);
    }

    pub fn add_test_window_with_manageability(
        &mut self,
        wid: WindowId,
        wsid: WindowServerId,
        space: Option<SpaceId>,
        frame: CGRect,
        is_manageable: bool,
    ) {
        self.track_test_window_server_info(wsid, wid.pid, frame);
        self.state.windows.mark_window_visible(wsid);
        self.insert_test_window(wid, wsid, space, frame, is_manageable);
    }

    pub fn track_test_window_server_info(
        &mut self,
        wsid: WindowServerId,
        pid: pid_t,
        frame: CGRect,
    ) {
        self.state.windows.track_window_server_info(WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame,
            min_frame: frame.size,
            max_frame: frame.size,
        });
    }

    pub fn insert_test_window(
        &mut self,
        wid: WindowId,
        wsid: WindowServerId,
        space: Option<SpaceId>,
        frame: CGRect,
        is_manageable: bool,
    ) {
        self.state.windows.track_window_server_id(wsid, wid);
        self.state.windows.set_window_server_space(wsid, space);
        self.insert_test_window_state(wid, frame, Some(wsid), is_manageable);
    }

    pub fn insert_test_window_state(
        &mut self,
        wid: WindowId,
        frame: CGRect,
        sys_id: Option<WindowServerId>,
        is_manageable: bool,
    ) {
        self.state.windows.insert_window(wid, super::WindowState {
            info: WindowInfo {
                is_standard: true,
                is_root: true,
                is_minimized: false,
                is_resizable: true,
                min_size: None,
                max_size: None,
                title: format!("Window {wid:?}"),
                frame,
                sys_id,
                bundle_id: None,
                path: None,
                ax_role: None,
                ax_subrole: None,
            },
            frame_monotonic: frame,
            is_manageable,
            manage_override: None,
        });
    }
}

/// The default reactor used by the tests. Keep the individual tests focused on
/// the behavior they exercise instead of repeating the production wiring.
pub fn test_reactor() -> Reactor {
    test_reactor_with_workspace_settings(&crate::common::config::VirtualWorkspaceSettings::default())
}

pub fn test_reactor_with_workspace_settings(
    workspace_settings: &crate::common::config::VirtualWorkspaceSettings,
) -> Reactor {
    Reactor::new_for_test(LayoutEngine::new(
        workspace_settings,
        &crate::common::config::LayoutSettings::default(),
        None,
    ))
}

pub fn test_reactor_with_workspace_count(count: usize) -> Reactor {
    let mut settings = crate::common::config::VirtualWorkspaceSettings::default();
    settings.default_workspace_count = count;
    test_reactor_with_workspace_settings(&settings)
}

pub fn make_screen_snapshots(frames: Vec<CGRect>, spaces: Vec<Option<SpaceId>>) -> Vec<ScreenInfo> {
    assert_eq!(frames.len(), spaces.len());
    frames
        .into_iter()
        .zip(spaces.into_iter())
        .enumerate()
        .map(|(idx, (frame, space))| ScreenInfo {
            id: crate::sys::screen::ScreenId::new(idx as u32),
            frame,
            space,
            display_uuid: format!("test-display-{idx}"),
            name: None,
        })
        .collect()
}

pub fn space_state_event(frames: Vec<CGRect>, spaces: Vec<Option<SpaceId>>) -> Event {
    space_state_event_from_screens(make_screen_snapshots(frames, spaces))
}

pub fn space_state_event_with(
    frames: Vec<CGRect>,
    spaces: Vec<Option<SpaceId>>,
    update: impl FnOnce(&mut ForwardedSpaceState),
) -> Event {
    let mut state = forwarded_space_state(make_screen_snapshots(frames, spaces));
    update(&mut state);
    Event::SpaceStateChanged(state)
}

pub fn space_state_event_from_screens(screens: Vec<ScreenInfo>) -> Event {
    Event::SpaceStateChanged(forwarded_space_state(screens))
}

pub fn forwarded_space_state(screens: Vec<ScreenInfo>) -> ForwardedSpaceState {
    let command_space = screens.iter().find_map(|screen| screen.space);
    let active_spaces = screens.iter().filter_map(|screen| screen.space).collect();
    ForwardedSpaceState {
        screens,
        fullscreen_spaces: Default::default(),
        has_seen_display_set: false,
        active_spaces,
        menu_bar_space: command_space,
        command_space,
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }
}

pub fn fullscreen_startup_space_state(
    screen: CGRect,
    display_uuid: String,
    user_space: SpaceId,
    fullscreen_space: SpaceId,
) -> Event {
    let mut state = forwarded_space_state(vec![ScreenInfo {
        id: crate::sys::screen::ScreenId::new(0),
        frame: screen,
        space: None,
        display_uuid,
        name: None,
    }]);
    state.fullscreen_spaces.insert(fullscreen_space);
    state.has_seen_display_set = true;
    state.active_spaces.clear();
    state.menu_bar_space = None;
    state.command_space = None;
    state
        .last_user_space_by_display
        .insert(state.screens[0].display_uuid.clone(), user_space);
    Event::SpaceStateChanged(state)
}

/*impl Drop for Reactor {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }

        if let Some(temp) = self.record.temp() {
            temp.as_file().flush().unwrap();
            // Attempt to run the replay tool if available; ignore if it's not present
            let replay_attempt = std::panic::catch_unwind(|| {
                let mut cmd = test_bin::get_test_bin("examples/devtool");
                cmd.arg("replay").arg(temp.path());
                println!("Replaying recorded data:\n{cmd:?}");
                cmd.spawn().unwrap().wait().unwrap().success()
            });
            if let Ok(false) = replay_attempt {
                // Tool executed but returned error; still ignore in tests
            }
        }
    }
}*/

pub fn make_window(idx: usize) -> WindowInfo {
    let window = make_window_info(
        CGRect::new(
            CGPoint::new(100.0 * f64::from(idx as u32), 100.0),
            CGSize::new(50.0, 50.0),
        ),
        Some(WindowServerId::new(idx as u32)),
        &format!("Window{idx}"),
        None,
    );
    window
}

pub fn make_window_info(
    frame: CGRect,
    sys_id: Option<WindowServerId>,
    title: &str,
    bundle_id: Option<&str>,
) -> WindowInfo {
    WindowInfo {
        is_standard: true,
        is_root: true,
        is_minimized: false,
        is_resizable: true,
        min_size: None,
        max_size: None,
        title: title.to_string(),
        frame,
        sys_id,
        bundle_id: bundle_id.map(str::to_string),
        path: None,
        ax_role: None,
        ax_subrole: None,
    }
}

pub fn make_windows(count: usize) -> Vec<WindowInfo> { (1..=count).map(make_window).collect() }

pub struct Apps {
    tx: actor::Sender<Request>,
    rx: actor::Receiver<Request>,
    pub windows: BTreeMap<WindowId, TestWindowState>,
}

#[derive(Default, PartialEq, Debug, Clone)]
pub struct TestWindowState {
    pub last_seen_txid: TransactionId,
    pub last_sent_txid: TransactionId,
    pub animating: bool,
    pub frame: CGRect,
}

impl Apps {
    pub fn new() -> Apps {
        let (tx, rx) = actor::channel();
        Apps {
            tx,
            rx,
            windows: BTreeMap::new(),
        }
    }

    pub fn make_app(&mut self, pid: pid_t, windows: Vec<WindowInfo>) -> Vec<Event> {
        let frontmost = windows.first().map(|_| WindowId::new(pid, 1));
        self.make_app_with_opts(pid, windows, frontmost, false, true)
    }

    pub fn make_app_with_opts(
        &mut self,
        pid: pid_t,
        windows: Vec<WindowInfo>,
        main_window: Option<WindowId>,
        is_frontmost: bool,
        with_ws_info: bool,
    ) -> Vec<Event> {
        let windows: Vec<WindowInfo> = windows
            .into_iter()
            .enumerate()
            .map(|(idx, mut info)| {
                // Keep synthetic window-server ids unique across apps so tests
                // exercise the same invariants as production.
                info.sys_id = Some(WindowServerId::new(
                    (pid as u32).saturating_mul(10_000) + idx as u32 + 1,
                ));
                info
            })
            .collect();

        for (id, info) in (1..).map(|idx| WindowId::new(pid, idx)).zip(&windows) {
            self.windows.insert(id, TestWindowState {
                frame: info.frame,
                ..Default::default()
            });
        }
        let handle = AppThreadHandle::new_for_test(self.tx.clone());
        vec![Event::ApplicationLaunched {
            pid,
            info: AppInfo {
                bundle_id: Some(format!("com.testapp{pid}")),
                localized_name: Some(format!("TestApp{pid}")),
            },
            handle,
            is_frontmost,
            main_window,
            window_server_info: if with_ws_info {
                windows
                    .iter()
                    .map(|info| WindowServerInfo {
                        pid,
                        id: info.sys_id.unwrap(),
                        layer: 0,
                        frame: info.frame,
                        min_frame: CGSize::ZERO,
                        max_frame: CGSize::ZERO,
                    })
                    .collect()
            } else {
                Default::default()
            },
            visible_windows: (1..).map(|idx| WindowId::new(pid, idx)).zip(windows).collect(),
        }]
    }

    pub fn requests(&mut self) -> Vec<Request> {
        let mut requests = Vec::new();
        while let Ok((_, req)) = self.rx.try_recv() {
            requests.push(req);
        }
        requests
    }

    pub fn simulate_until_quiet(&mut self, reactor: &mut Reactor) {
        let mut requests = self.requests();
        while !requests.is_empty() {
            for event in self.simulate_events_for_requests(requests) {
                reactor.handle_event(event);
            }
            requests = self.requests();
        }
    }

    pub fn make_app_and_settle(
        &mut self,
        reactor: &mut Reactor,
        pid: pid_t,
        windows: Vec<WindowInfo>,
    ) {
        reactor.handle_events(self.make_app(pid, windows));
        self.simulate_until_quiet(reactor);
    }

    pub fn make_app_and_settle_on_screen(
        &mut self,
        reactor: &mut Reactor,
        screen: CGRect,
        space: SpaceId,
        pid: pid_t,
        windows: Vec<WindowInfo>,
    ) {
        reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
        self.make_app_and_settle(reactor, pid, windows);
    }

    pub fn simulate_events(&mut self) -> Vec<Event> {
        let requests = self.requests();
        self.simulate_events_for_requests(requests)
    }

    pub fn simulate_events_for_requests(&mut self, requests: Vec<Request>) -> Vec<Event> {
        let mut events = vec![];
        let mut got_visible_windows = false;
        for request in requests {
            debug!(?request);
            match request {
                Request::Terminate => break,
                Request::WindowMaybeDestroyed(_) => {}
                Request::GetVisibleWindows => {
                    if got_visible_windows {
                        continue;
                    }
                    got_visible_windows = true;
                    let mut app_windows = BTreeMap::<pid_t, Vec<WindowId>>::new();
                    for &wid in self.windows.keys() {
                        app_windows.entry(wid.pid).or_default().push(wid);
                    }
                    for (pid, windows) in app_windows {
                        events.push(Event::WindowsDiscovered {
                            pid,
                            new: vec![],
                            known_visible: windows,
                        });
                    }
                }
                Request::ApplicationGloballyActivated(pid) => {
                    events.push(Event::ApplicationActivated(pid, Quiet::No));
                }
                Request::SetWindowFrame(wid, frame, txid, _) => {
                    let window = self.windows.entry(wid).or_default();
                    window.last_seen_txid = txid;
                    let old_frame = window.frame;
                    window.frame = frame;
                    if !window.animating && !old_frame.same_as(frame) {
                        events.push(Event::WindowFrameChanged(
                            wid,
                            frame,
                            Some(txid),
                            Requested(true),
                            None,
                        ));
                    }
                }
                Request::SetBatchWindowFrame(frames, txid, _) => {
                    for (wid, frame) in frames {
                        let window = self.windows.entry(wid).or_default();
                        window.last_seen_txid = txid;
                        let old_frame = window.frame;
                        window.frame = frame;
                        if !window.animating && !old_frame.same_as(frame) {
                            events.push(Event::WindowFrameChanged(
                                wid,
                                frame,
                                Some(txid),
                                Requested(true),
                                None,
                            ));
                        }
                    }
                }
                Request::SetWorkspaceSwitchPositions(positions, txid, _) => {
                    for (wid, position) in positions {
                        let window = self.windows.entry(wid).or_default();
                        window.last_seen_txid = txid;
                        let old_frame = window.frame;
                        window.frame.origin = position;
                        if !window.animating && !old_frame.same_as(window.frame) {
                            events.push(Event::WindowFrameChanged(
                                wid,
                                window.frame,
                                Some(txid),
                                Requested(true),
                                None,
                            ));
                        }
                    }
                }
                Request::SetWindowPos(wid, pos, txid, _) => {
                    let window = self.windows.entry(wid).or_default();
                    window.last_seen_txid = txid;
                    let old_frame = window.frame;
                    window.frame.origin = pos;
                    if !window.animating && !old_frame.same_as(window.frame) {
                        events.push(Event::WindowFrameChanged(
                            wid,
                            window.frame,
                            Some(txid),
                            Requested(true),
                            None,
                        ));
                    }
                }
                Request::AnimationFrame { wid, frame, set_size, txid } => {
                    let window = self.windows.entry(wid).or_default();
                    window.last_seen_txid = txid;
                    let old_frame = window.frame;
                    if set_size {
                        window.frame = frame;
                    } else {
                        window.frame.origin = frame.origin;
                    }
                    if !window.animating && !old_frame.same_as(window.frame) {
                        events.push(Event::WindowFrameChanged(
                            wid,
                            window.frame,
                            Some(txid),
                            Requested(true),
                            None,
                        ));
                    }
                }
                Request::BeginWindowAnimation(wid) => {
                    self.windows.entry(wid).or_default().animating = true;
                }
                Request::EndWindowAnimation(wid) => {
                    let window = self.windows.entry(wid).or_default();
                    window.animating = false;
                    events.push(Event::WindowFrameChanged(
                        wid,
                        window.frame,
                        Some(window.last_seen_txid),
                        Requested(true),
                        None,
                    ));
                }
                Request::Raise(..) => todo!(),
                Request::CloseWindow(..) => todo!(),
            }
        }
        debug!(?events);
        events
    }
}

pub fn test_context() -> (Apps, Reactor) { (Apps::new(), test_reactor()) }

pub fn test_context_with_workspace_count(count: usize) -> (Apps, Reactor) {
    let mut settings = crate::common::config::VirtualWorkspaceSettings::default();
    settings.default_workspace_count = count;
    (Apps::new(), test_reactor_with_workspace_settings(&settings))
}
