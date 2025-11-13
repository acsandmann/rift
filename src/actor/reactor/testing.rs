use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use tracing::debug;

use super::{Event, Reactor, Record, Requested, ScreenSnapshot, TransactionId};
use crate::actor;
use crate::actor::app::{AppThreadHandle, Request, WindowId};
use crate::common::collections::BTreeMap;
use crate::common::config::Config;
use crate::layout_engine::LayoutEngine;
use crate::sys::app::{AppInfo, WindowInfo, pid_t};
use crate::sys::geometry::SameAs;
use crate::sys::screen::{ScreenId, ScreenSpace, SpaceId};
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

impl Reactor {
    pub fn new_for_test(layout: LayoutEngine) -> Reactor {
        let mut config = Config::default();
        config.settings.default_disable = false;
        config.settings.animate = false;
        let record = Record::new_for_test(tempfile::NamedTempFile::new().unwrap());
        let (broadcast_tx, _) = actor::channel();
        Reactor::new(config, layout, record, broadcast_tx, None)
    }

    pub fn handle_events(&mut self, events: Vec<Event>) {
        for event in events {
            self.handle_event(event);
        }
    }
}

pub fn make_screen_snapshots(
    frames: Vec<CGRect>,
    screen_spaces: Vec<ScreenSpace>,
) -> Vec<ScreenSnapshot> {
    assert_eq!(frames.len(), screen_spaces.len());
    frames
        .into_iter()
        .zip(screen_spaces.into_iter())
        .map(|(frame, screen_space)| ScreenSnapshot {
            frame,
            space: screen_space.space,
            display_uuid: format!("test-display-{}", screen_space.screen_id.as_u32()),
            name: None,
            screen_id: screen_space.screen_id.as_u32(),
        })
        .collect()
}

pub fn screen_space_list(spaces: Vec<Option<SpaceId>>) -> Vec<ScreenSpace> {
    spaces
        .into_iter()
        .enumerate()
        .map(|(idx, space)| ScreenSpace {
            screen_id: ScreenId::new(idx as u32),
            space,
        })
        .collect()
}

pub fn screen_params_event(
    frames: Vec<CGRect>,
    spaces: Vec<Option<SpaceId>>,
    ws_info: Vec<WindowServerInfo>,
) -> Event {
    let screen_spaces = screen_space_list(spaces);
    Event::ScreenParametersChanged(make_screen_snapshots(frames, screen_spaces.clone()), ws_info)
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
    WindowInfo {
        is_standard: true,
        is_root: true,
        is_minimized: false,
        title: format!("Window{idx}"),
        frame: CGRect::new(
            CGPoint::new(100.0 * f64::from(idx as u32), 100.0),
            CGSize::new(50.0, 50.0),
        ),
        // TODO: This is wrong and conflicts with windows from other apps.
        sys_id: Some(WindowServerId::new(idx as u32)),
        bundle_id: None,
        path: None,
        ax_role: None,
        ax_subrole: None,
    }
}

pub fn make_windows(count: usize) -> Vec<WindowInfo> { (1..=count).map(make_window).collect() }

pub struct Apps {
    tx: actor::Sender<Request>,
    rx: actor::Receiver<Request>,
    pub windows: BTreeMap<WindowId, WindowState>,
}

#[derive(Default, PartialEq, Debug, Clone)]
pub struct WindowState {
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
        for (id, info) in (1..).map(|idx| WindowId::new(pid, idx)).zip(&windows) {
            self.windows.insert(id, WindowState {
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
                Request::MarkWindowsNeedingInfo(_) => {}
                Request::GetVisibleWindows { .. } => {
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
                Request::SetBatchWindowFrame(frames, txid) => {
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
            }
        }
        debug!(?events);
        events
    }
}
