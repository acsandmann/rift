//! The Reactor's job is to maintain coherence between the system and model state.
//!
//! It takes events from the rest of the system and builds a coherent picture of
//! what is going on. It shares this with the layout actor, and reacts to layout
//! changes by sending requests out to the other actors in the system.

mod animation;
mod main_window;
mod query;
mod replay;

#[cfg(test)]
mod testing;

#[cfg(test)]
mod tests;

use std::collections::hash_map::Entry;
use std::time::Duration;
use std::{mem, thread};

use animation::Animation;
use main_window::MainWindowTracker;
use objc2_app_kit::{NSNormalWindowLevel, NSRunningApplication};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
pub use replay::{Record, replay};
use serde::{Deserialize, Serialize};
use serde_json;
use serde_with::serde_as;
use tracing::{debug, error, info, instrument, trace, warn};

use super::event_tap;
use crate::actor::app::{AppInfo, AppThreadHandle, Quiet, Request, WindowId, WindowInfo, pid_t};
use crate::actor::broadcast::{BroadcastEvent, BroadcastSender};
use crate::actor::raise_manager::{self, RaiseRequest};
use crate::actor::wm_controller::WmEvent;
use crate::actor::{self, menu_bar, stack_line};
use crate::common::collections::{BTreeMap, HashMap, HashSet};
use crate::common::config::Config;
use crate::common::log::{self, MetricsCommand};
use crate::layout_engine::{self as layout, Direction, LayoutCommand, LayoutEngine, LayoutEvent};
use crate::model::VirtualWorkspaceId;
use crate::model::tx_store::WindowTxStore;
use crate::sys::event::MouseState;
use crate::sys::executor::Executor;
use crate::sys::geometry::{CGRectDef, CGRectExt, Round, SameAs};
use crate::sys::power;
use crate::sys::screen::{SpaceId, get_active_space_number};
use crate::sys::timer::Timer;
use crate::sys::window_server::{
    self, WindowServerId, WindowServerInfo, space_is_fullscreen, space_is_user,
    wait_for_native_fullscreen_transition,
};

pub type Sender = actor::Sender<Event>;
type Receiver = actor::Receiver<Event>;
use std::collections::VecDeque;
use std::path::PathBuf;

use crate::model::server::{ApplicationData, LayoutStateData, WindowData, WorkspaceQueryResponse};

#[serde_as]
#[derive(Serialize, Deserialize, Debug)]
pub enum Event {
    /// The screen layout, including resolution, changed. This is always the
    /// first event sent on startup.
    ///
    /// The first vec is the frame for each screen. The main screen is always
    /// first in the list.
    ///
    /// See the `SpaceChanged` event for an explanation of the other parameters.
    ScreenParametersChanged(
        #[serde_as(as = "Vec<CGRectDef>")] Vec<CGRect>,
        Vec<Option<SpaceId>>,
        Vec<WindowServerInfo>,
    ),

    /// The current space changed.
    ///
    /// There is one SpaceId per screen in the last ScreenParametersChanged
    /// event. `None` in the SpaceId vec disables managing windows on that
    /// screen until the next space change.
    ///
    /// A snapshot of visible windows from the window server is also taken and
    /// sent with this message. This allows us to determine more precisely which
    /// windows are visible on a given space, since app actor events like
    /// WindowsDiscovered are not ordered with respect to space events.
    SpaceChanged(Vec<Option<SpaceId>>, Vec<WindowServerInfo>),

    /// An application was launched. This event is also sent for every running
    /// application on startup.
    ///
    /// Both WindowInfo (accessibility) and WindowServerInfo are collected for
    /// any already-open windows when the launch event is sent. Since this
    /// event isn't ordered with respect to the Space events, it is possible to
    /// receive this event for a space we just switched off of.. FIXME. The same
    /// is true of WindowCreated events.
    ApplicationLaunched {
        pid: pid_t,
        info: AppInfo,
        #[serde(skip, default = "replay::deserialize_app_thread_handle")]
        handle: AppThreadHandle,
        is_frontmost: bool,
        main_window: Option<WindowId>,
        visible_windows: Vec<(WindowId, WindowInfo)>,
        window_server_info: Vec<WindowServerInfo>,
    },
    ApplicationTerminated(pid_t),
    ApplicationThreadTerminated(pid_t),
    ApplicationActivated(pid_t, Quiet),
    ApplicationDeactivated(pid_t),
    ApplicationGloballyActivated(pid_t),
    ApplicationGloballyDeactivated(pid_t),
    ApplicationMainWindowChanged(pid_t, Option<WindowId>, Quiet),

    WindowsDiscovered {
        pid: pid_t,
        new: Vec<(WindowId, WindowInfo)>,
        known_visible: Vec<WindowId>,
    },
    WindowCreated(WindowId, WindowInfo, Option<WindowServerInfo>, MouseState),
    WindowDestroyed(WindowId),
    #[serde(skip)]
    WindowServerDestroyed(crate::sys::window_server::WindowServerId, SpaceId),
    #[serde(skip)]
    WindowServerAppeared(crate::sys::window_server::WindowServerId, SpaceId),
    WindowMinimized(WindowId),
    WindowDeminiaturized(WindowId),
    WindowFrameChanged(
        WindowId,
        #[serde(with = "CGRectDef")] CGRect,
        Option<TransactionId>,
        Requested,
        Option<MouseState>,
    ),
    ResyncAppForWindow(WindowServerId),
    MenuOpened,
    WindowIsChangingScreens(WindowServerId),
    MenuClosed,

    /// Left mouse button was released.
    ///
    /// Layout changes are suppressed while the button is down so that they
    /// don't interfere with drags. This event is used to update the layout in
    /// case updates were supressed while the button was down.
    ///
    /// FIXME: This can be interleaved incorrectly with the MouseState in app
    /// actor events.
    MouseUp,
    /// The mouse cursor moved over a new window. Only sent if focus-follows-
    /// mouse is enabled.
    MouseMovedOverWindow(WindowServerId),
    /// System woke from sleep; used to re-subscribe SLS notifications.
    SystemWoke,

    #[serde(skip)]
    MissionControlNativeEntered,
    #[serde(skip)]
    MissionControlNativeExited,

    /// A raise request completed. Used by the raise manager to track when
    /// all raise requests in a sequence have finished.
    RaiseCompleted {
        window_id: WindowId,
        sequence_id: u64,
    },

    /// A raise sequence timed out. Used by the raise manager to clean up
    /// pending raises that took too long.
    RaiseTimeout {
        sequence_id: u64,
    },

    Command(Command),

    #[serde(skip)]
    RegisterWmSender(crate::actor::wm_controller::Sender),

    // Query events with response channels (not serialized)
    #[serde(skip)]
    QueryWorkspaces(r#continue::Sender<WorkspaceQueryResponse>),
    #[serde(skip)]
    QueryWindows {
        space_id: Option<SpaceId>,
        #[serde(skip)]
        response: r#continue::Sender<Vec<WindowData>>,
    },
    #[serde(skip)]
    QueryWindowInfo {
        window_id: WindowId,
        #[serde(skip)]
        response: r#continue::Sender<Option<WindowData>>,
    },
    #[serde(skip)]
    QueryApplications(r#continue::Sender<Vec<ApplicationData>>),
    #[serde(skip)]
    QueryLayoutState {
        space_id: u64,
        #[serde(skip)]
        response: r#continue::Sender<Option<LayoutStateData>>,
    },
    #[serde(skip)]
    QueryMetrics(r#continue::Sender<serde_json::Value>),

    #[serde(skip)]
    ConfigUpdated(Config),

    /// Apply app rules to existing windows when a space is activated
    ApplyAppRulesToExistingWindows {
        pid: pid_t,
        app_info: AppInfo,
        windows: Vec<WindowServerInfo>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Requested(pub bool);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum Command {
    Layout(LayoutCommand),
    Metrics(MetricsCommand),
    Reactor(ReactorCommand),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReactorCommand {
    Debug,
    Serialize,
    SaveAndExit,
    SwitchSpace(Direction),
    FocusWindow {
        window_id: WindowId,
        window_server_id: Option<WindowServerId>,
    },
    SetMissionControlActive(bool),
}

#[derive(Default, Debug, Clone)]
struct FullscreenTrack {
    pids: HashSet<pid_t>,
    last_removed: VecDeque<WindowServerId>,
}

#[derive(Debug, Clone)]
struct DragSession {
    window: WindowId,
    last_frame: CGRect,
    origin_space: Option<SpaceId>,
    settled_space: Option<SpaceId>,
    layout_dirty: bool,
}

use crate::actor::raise_manager::RaiseManager;

pub struct Reactor {
    config: Config,
    apps: HashMap<pid_t, AppState>,
    layout_engine: LayoutEngine,
    windows: HashMap<WindowId, WindowState>,
    window_server_info: HashMap<WindowServerId, WindowServerInfo>,
    window_ids: HashMap<WindowServerId, WindowId>,
    visible_windows: HashSet<WindowServerId>,
    observed_window_server_ids: HashSet<WindowServerId>,
    screens: Vec<Screen>,
    main_window_tracker: MainWindowTracker,
    in_drag: bool,
    is_workspace_switch: bool,
    workspace_switch_generation: u64,
    active_workspace_switch: Option<u64>,
    record: Record,
    event_tap_tx: Option<event_tap::Sender>,
    menu_tx: Option<menu_bar::Sender>,
    stack_line_tx: Option<stack_line::Sender>,
    raise_manager_tx: raise_manager::Sender,
    event_broadcaster: BroadcastSender,
    wm_sender: Option<crate::actor::wm_controller::Sender>,
    app_rules_recently_applied: std::time::Instant,
    last_auto_workspace_switch: Option<AutoWorkspaceSwitch>,
    last_sls_notification_ids: Vec<u32>,
    menu_open_depth: usize,
    mission_control_active: bool,
    suppress_stale_window_cleanup: bool,
    pending_refocus_space: Option<SpaceId>,
    window_notify_tx: Option<crate::actor::window_notify::Sender>,
    window_tx_store: Option<WindowTxStore>,
    drag_manager: crate::actor::drag_swap::DragManager,
    skip_layout_for_window: Option<WindowId>,
    pending_drag_swap: Option<(WindowId, WindowId)>,
    pending_space_change: Option<PendingSpaceChange>,
    active_drag: Option<DragSession>,
    events_tx: Option<Sender>,
    fullscreen_by_space: HashMap<u64, FullscreenTrack>,
    changing_screens: HashSet<WindowServerId>,
    pending_mission_control_refresh: HashSet<pid_t>,
}

#[derive(Debug)]
struct AppState {
    #[allow(unused)]
    pub info: AppInfo,
    pub handle: AppThreadHandle,
}

#[derive(Debug, Clone)]
struct PendingSpaceChange {
    spaces: Vec<Option<SpaceId>>,
    ws_info: Vec<WindowServerInfo>,
}

#[derive(Copy, Clone, Debug)]
struct Screen {
    frame: CGRect,
    space: Option<SpaceId>,
}

#[derive(Clone, Debug)]
struct AutoWorkspaceSwitch {
    occurred_at: std::time::Instant,
    space: SpaceId,
    from_workspace: Option<VirtualWorkspaceId>,
    to_workspace: VirtualWorkspaceId,
}

/// A per-window counter that tracks the last time the reactor sent a request to
/// change the window frame.
#[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionId(u32);

#[derive(Debug)]
struct WindowState {
    #[allow(unused)]
    title: String,
    /// The last known frame of the window. Always includes the last write.
    ///
    /// This value only updates monotonically with respect to writes; in other
    /// words, we only accept reads when we know they come after the last write.
    frame_monotonic: CGRect,
    is_ax_standard: bool,
    is_ax_root: bool,
    is_minimized: bool,
    is_manageable: bool,
    last_sent_txid: TransactionId,
    window_server_id: Option<WindowServerId>,
    #[allow(unused)]
    bundle_id: Option<String>,
    #[allow(unused)]
    bundle_path: Option<PathBuf>,
    ax_role: Option<String>,
    ax_subrole: Option<String>,
}

impl WindowState {
    #[must_use]
    fn next_txid(&mut self) -> TransactionId {
        self.last_sent_txid.0 += 1;
        self.last_sent_txid
    }
}

impl From<WindowInfo> for WindowState {
    fn from(info: WindowInfo) -> WindowState {
        WindowState {
            title: info.title,
            frame_monotonic: info.frame,
            is_ax_standard: info.is_standard,
            is_ax_root: info.is_root,
            is_minimized: info.is_minimized,
            is_manageable: false,
            last_sent_txid: TransactionId::default(),
            window_server_id: info.sys_id,
            bundle_id: info.bundle_id,
            bundle_path: info.path,
            ax_role: info.ax_role,
            ax_subrole: info.ax_subrole,
        }
    }
}

impl Reactor {
    pub fn spawn(
        config: Config,
        layout_engine: LayoutEngine,
        record: Record,
        event_tap_tx: event_tap::Sender,
        broadcast_tx: BroadcastSender,
        menu_tx: menu_bar::Sender,
        stack_line_tx: stack_line::Sender,
        window_notify: Option<(crate::actor::window_notify::Sender, WindowTxStore)>,
    ) -> Sender {
        let (events_tx, events) = actor::channel();
        let events_tx_clone = events_tx.clone();
        thread::Builder::new()
            .name("reactor".to_string())
            .spawn(move || {
                let mut reactor =
                    Reactor::new(config, layout_engine, record, broadcast_tx, window_notify);
                reactor.event_tap_tx.replace(event_tap_tx);
                reactor.menu_tx.replace(menu_tx);
                reactor.stack_line_tx.replace(stack_line_tx);
                reactor.events_tx = Some(events_tx_clone.clone());
                Executor::run(reactor.run(events, events_tx_clone));
            })
            .unwrap();
        events_tx
    }

    pub fn new(
        config: Config,
        layout_engine: LayoutEngine,
        mut record: Record,
        broadcast_tx: BroadcastSender,
        window_notify: Option<(crate::actor::window_notify::Sender, WindowTxStore)>,
    ) -> Reactor {
        // FIXME: Remove apps that are no longer running from restored state.
        record.start(&config, &layout_engine);
        let (raise_manager_tx, _rx) = actor::channel();
        let (window_notify_tx, window_tx_store) = match window_notify {
            Some((tx, store)) => (Some(tx), Some(store)),
            None => (None, None),
        };
        Reactor {
            config: config.clone(),
            apps: HashMap::default(),
            layout_engine,
            windows: HashMap::default(),
            window_ids: HashMap::default(),
            window_server_info: HashMap::default(),
            visible_windows: HashSet::default(),
            observed_window_server_ids: HashSet::default(),
            screens: vec![],
            main_window_tracker: MainWindowTracker::default(),
            in_drag: false,
            is_workspace_switch: false,
            workspace_switch_generation: 0,
            active_workspace_switch: None,
            record,
            event_tap_tx: None,
            menu_tx: None,
            stack_line_tx: None,
            raise_manager_tx,
            event_broadcaster: broadcast_tx,
            wm_sender: None,
            app_rules_recently_applied: std::time::Instant::now(),
            last_auto_workspace_switch: None,
            last_sls_notification_ids: Vec::new(),
            menu_open_depth: 0,
            mission_control_active: false,
            suppress_stale_window_cleanup: false,
            pending_refocus_space: None,
            window_notify_tx,
            window_tx_store,
            drag_manager: crate::actor::drag_swap::DragManager::new(
                config.settings.window_snapping,
            ),
            skip_layout_for_window: None,
            pending_drag_swap: None,
            pending_space_change: None,
            active_drag: None,
            changing_screens: HashSet::default(),
            events_tx: None,
            fullscreen_by_space: HashMap::default(),
            pending_mission_control_refresh: HashSet::default(),
        }
    }

    fn store_txid(&self, wsid: Option<WindowServerId>, txid: TransactionId, target: CGRect) {
        if let (Some(store), Some(id)) = (self.window_tx_store.as_ref(), wsid) {
            store.insert(id, txid, target);
        }
    }

    fn update_txid_entries<I>(&self, entries: I)
    where I: IntoIterator<Item = (WindowServerId, TransactionId, CGRect)> {
        if let Some(store) = self.window_tx_store.as_ref() {
            for (wsid, txid, target) in entries {
                store.insert(wsid, txid, target);
            }
        }
    }

    fn remove_txid_for_window(&self, wsid: Option<WindowServerId>) {
        if let (Some(store), Some(id)) = (self.window_tx_store.as_ref(), wsid) {
            store.remove(&id);
        }
    }

    pub async fn run(mut self, events: Receiver, events_tx: Sender) {
        let (raise_manager_tx, raise_manager_rx) = actor::channel();
        self.raise_manager_tx = raise_manager_tx.clone();

        let event_tap_tx = self.event_tap_tx.clone();
        let reactor_task = self.run_reactor_loop(events);
        let raise_manager_task = RaiseManager::run(raise_manager_rx, events_tx, event_tap_tx);

        let _ = tokio::join!(reactor_task, raise_manager_task);
    }

    async fn run_reactor_loop(mut self, mut events: Receiver) {
        while let Some((span, event)) = events.recv().await {
            let _guard = span.enter();
            self.handle_event(event);
        }
    }

    fn log_event(&self, event: &Event) {
        match event {
            Event::WindowFrameChanged(..) | Event::MouseUp => trace!(?event, "Event"),
            _ => debug!(?event, "Event"),
        }
    }

    #[instrument(name = "reactor::handle_event", skip(self), fields(event=?event))]
    fn handle_event(&mut self, event: Event) {
        self.log_event(&event);
        self.record.on_event(&event);

        if matches!(
            event,
            Event::QueryApplications(..)
                | Event::QueryLayoutState { .. }
                | Event::QueryMetrics(..)
                | Event::QueryWindowInfo { .. }
                | Event::QueryWindows { .. }
                | Event::QueryWorkspaces(..)
        ) {
            return self.handle_query(event);
        }

        let should_update_notifications = matches!(
            &event,
            Event::WindowCreated(..)
                | Event::WindowDestroyed(..)
                | Event::WindowServerDestroyed(..)
                | Event::WindowServerAppeared(..)
                | Event::WindowsDiscovered { .. }
                | Event::ApplicationLaunched { .. }
                | Event::ApplicationTerminated(..)
                | Event::ApplicationThreadTerminated(..)
                | Event::SpaceChanged(..)
                | Event::ScreenParametersChanged(..)
        );

        let raised_window = self.main_window_tracker.handle_event(&event);
        let mut is_resize = false;
        let mut window_was_destroyed = false;

        match event {
            Event::ApplicationLaunched {
                pid,
                info,
                handle,
                visible_windows,
                window_server_info,
                is_frontmost: _,
                main_window: _,
            } => {
                self.apps.insert(pid, AppState { info: info.clone(), handle });
                self.update_partial_window_server_info(window_server_info);
                self.on_windows_discovered_with_app_info(pid, visible_windows, vec![], Some(info));
            }
            Event::ApplyAppRulesToExistingWindows { pid, app_info, windows } => {
                self.app_rules_recently_applied = std::time::Instant::now();

                self.update_partial_window_server_info(windows.clone());

                let all_windows: Vec<WindowId> = windows
                    .iter()
                    .filter_map(|info| self.window_ids.get(&info.id).copied())
                    .filter(|wid| self.window_is_standard(*wid))
                    .collect();

                if !all_windows.is_empty() {
                    self.process_windows_for_app_rules(pid, all_windows, app_info);
                }
            }
            Event::ApplicationTerminated(pid) => {
                if let Some(app) = self.apps.get_mut(&pid) {
                    _ = app.handle.send(Request::Terminate);
                }
            }
            Event::ApplicationThreadTerminated(pid) => {
                // The app actor thread has terminated; remove the stored handle
                // so we don't try to communicate with a dead thread. Do NOT
                // perform per-app window bookkeeping here (e.g. sending
                // LayoutEvent::AppClosed) — a thread exit may be transient and
                // should not cause the layout engine to drop windows for the
                // application. Full application termination (Event::ApplicationTerminated)
                // is responsible for informing other subsystems when windows
                // should be removed.
                // Notify the WM controller that the app thread exited so it can
                // clear any tracking (e.g. known_apps) and allow future launches.
                if let Some(wm) = self.wm_sender.as_ref() {
                    wm.send(WmEvent::AppThreadTerminated(pid));
                }
                self.apps.remove(&pid);
            }
            Event::ApplicationActivated(..)
            | Event::ApplicationDeactivated(..)
            | Event::ApplicationGloballyDeactivated(..)
            | Event::ApplicationMainWindowChanged(..) => {}
            Event::ResyncAppForWindow(wsid) => {
                if let Some(&wid) = self.window_ids.get(&wsid) {
                    if let Some(app_state) = self.apps.get(&wid.pid) {
                        let _ = app_state
                            .handle
                            .send(Request::GetVisibleWindows { force_refresh: true });
                    }
                } else if let Some(info) = self
                    .window_server_info
                    .get(&wsid)
                    .cloned()
                    .or_else(|| window_server::get_window(wsid))
                {
                    if let Some(app_state) = self.apps.get(&info.pid) {
                        let _ = app_state
                            .handle
                            .send(Request::GetVisibleWindows { force_refresh: true });
                    }
                }
            }
            Event::ApplicationGloballyActivated(pid) => {
                self.handle_app_activation_workspace_switch(pid);
            }
            Event::RegisterWmSender(sender) => self.wm_sender = Some(sender),
            Event::WindowsDiscovered { pid, new, known_visible } => {
                self.on_windows_discovered_with_app_info(pid, new, known_visible, None)
            }
            Event::WindowIsChangingScreens(wsid) => {
                self.changing_screens.insert(wsid);
                if let Some((dragged_wid, target_wid)) = self.pending_drag_swap.take() {
                    trace!(
                        ?dragged_wid,
                        ?target_wid,
                        ?wsid,
                        "Clearing pending drag swap; window is moving between spaces"
                    );
                    if self.skip_layout_for_window == Some(dragged_wid) {
                        self.skip_layout_for_window = None;
                    }
                }
                self.drag_manager.reset();
                self.active_drag = None;
                self.in_drag = true;
                if let Some(&wid) = self.window_ids.get(&wsid) {
                    if let Some(frame) = self.windows.get(&wid).map(|window| window.frame_monotonic)
                    {
                        self.ensure_active_drag(wid, &frame);
                    }
                }
            }
            Event::WindowCreated(wid, window, ws_info, mouse_state) => {
                // FIXME: We assume all windows are on the main screen.
                if let Some(wsid) = window.sys_id {
                    self.window_ids.insert(wsid, wid);
                    self.observed_window_server_ids.remove(&wsid);
                }
                if let Some(info) = ws_info {
                    self.observed_window_server_ids.remove(&info.id);
                    self.window_server_info.insert(info.id, info);
                }

                let frame = window.frame;
                let mut window_state: WindowState = window.into();
                let is_manageable = self.compute_window_manageability(&window_state);
                window_state.is_manageable = is_manageable;
                self.store_txid(
                    window_state.window_server_id,
                    window_state.last_sent_txid,
                    window_state.frame_monotonic,
                );
                self.windows.insert(wid, window_state);

                if is_manageable {
                    if let Some(space) = self.best_space_for_window(&frame) {
                        self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                    }
                }
                if mouse_state == MouseState::Down {
                    self.in_drag = true;
                }
            }
            Event::WindowDestroyed(wid) => {
                if !self.windows.contains_key(&wid) {
                    return;
                }
                let window_server_id = self.windows.get(&wid).and_then(|w| w.window_server_id);
                self.remove_txid_for_window(window_server_id);
                if let Some(ws_id) = window_server_id {
                    self.window_ids.remove(&ws_id);
                    self.window_server_info.remove(&ws_id);
                    self.visible_windows.remove(&ws_id);
                } else {
                    debug!(?wid, "Received WindowDestroyed for unknown window - ignoring");
                }
                self.windows.remove(&wid);
                self.send_layout_event(LayoutEvent::WindowRemoved(wid));
                window_was_destroyed = true;

                if let Some((dragged_wid, target_wid)) = self.pending_drag_swap {
                    if dragged_wid == wid || target_wid == wid {
                        trace!(
                            ?wid,
                            "Clearing pending drag swap because a participant window was destroyed"
                        );
                        self.pending_drag_swap = None;
                    }
                }

                let dragged_window = self.drag_manager.dragged();
                let last_target = self.drag_manager.last_target();
                if dragged_window == Some(wid) || last_target == Some(wid) {
                    self.drag_manager.reset();
                    if dragged_window == Some(wid) {
                        self.active_drag = None;
                        self.in_drag = false;
                    }
                }

                if self.skip_layout_for_window == Some(wid) {
                    self.skip_layout_for_window = None;
                }
            }
            Event::WindowServerDestroyed(wsid, sid) => {
                if space_is_fullscreen(sid.get()) {
                    let entry = match self.fullscreen_by_space.entry(sid.get()) {
                        Entry::Occupied(o) => o.into_mut(),
                        Entry::Vacant(v) => v.insert(FullscreenTrack::default()),
                    };
                    if let Some(&wid) = self.window_ids.get(&wsid) {
                        entry.pids.insert(wid.pid);
                        if entry.last_removed.len() >= 5 {
                            let _ = entry.last_removed.pop_front();
                        }
                        entry.last_removed.push_back(wsid);
                        if let Some(app_state) = self.apps.get(&wid.pid) {
                            let _ =
                                app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]));
                        }
                        return;
                    } else if let Some(info) = self.window_server_info.get(&wsid) {
                        entry.pids.insert(info.pid);
                        if entry.last_removed.len() >= 5 {
                            let _ = entry.last_removed.pop_front();
                        }
                        entry.last_removed.push_back(wsid);
                        return;
                    }
                    return;
                } else if space_is_user(sid.get()) {
                    if let Some(&wid) = self.window_ids.get(&wsid) {
                        let _ = self.window_ids.remove(&wsid);
                        self.window_server_info.remove(&wsid);
                        self.visible_windows.remove(&wsid);
                        if let Some(app_state) = self.apps.get(&wid.pid) {
                            let _ =
                                app_state.handle.send(Request::MarkWindowsNeedingInfo(vec![wid]));
                            let _ = app_state
                                .handle
                                .send(Request::GetVisibleWindows { force_refresh: true });
                        }
                        if let Some(tx) = self.events_tx.as_ref() {
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
            Event::WindowServerAppeared(wsid, sid) => {
                if self.window_server_info.contains_key(&wsid)
                    || self.observed_window_server_ids.contains(&wsid)
                {
                    debug!(
                        ?wsid,
                        "Received WindowServerAppeared for known window - ignoring"
                    );
                    return;
                }

                self.observed_window_server_ids.insert(wsid);
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

                    if space_is_fullscreen(sid.get()) {
                        let entry = match self.fullscreen_by_space.entry(sid.get()) {
                            Entry::Occupied(o) => o.into_mut(),
                            Entry::Vacant(v) => v.insert(FullscreenTrack::default()),
                        };
                        entry.pids.insert(window_server_info.pid);
                        if entry.last_removed.len() >= 5 {
                            let _ = entry.last_removed.pop_front();
                        }
                        entry.last_removed.push_back(wsid);
                        if let Some(&wid) = self.window_ids.get(&wsid) {
                            if let Some(app_state) = self.apps.get(&wid.pid) {
                                let _ = app_state
                                    .handle
                                    .send(Request::MarkWindowsNeedingInfo(vec![wid]));
                            }
                        } else if let Some(app_state) = self.apps.get(&window_server_info.pid) {
                            let resync: Vec<_> = self
                                .windows
                                .keys()
                                .copied()
                                .filter(|wid| wid.pid == window_server_info.pid)
                                .collect();
                            if !resync.is_empty() {
                                let _ =
                                    app_state.handle.send(Request::MarkWindowsNeedingInfo(resync));
                            }
                        }
                        return;
                    }

                    self.update_partial_window_server_info(vec![window_server_info]);

                    if !self.apps.contains_key(&window_server_info.pid) {
                        if let Some(app) =
                            NSRunningApplication::runningApplicationWithProcessIdentifier(
                                window_server_info.pid,
                            )
                        {
                            debug!(
                                ?app,
                                "Received WindowServerAppeared for unknown app - synthesizing AppLaunch"
                            );
                            self.wm_sender.as_ref().map(|wm| {
                                wm.send(WmEvent::AppLaunch(
                                    window_server_info.pid,
                                    AppInfo::from(&*app),
                                ))
                            });
                        }
                    } else if let Some(app) = self.apps.get(&window_server_info.pid) {
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
            Event::WindowMinimized(wid) => {
                if let Some(window) = self.windows.get_mut(&wid) {
                    if window.is_minimized {
                        return;
                    }
                    window.is_minimized = true;
                    window.is_manageable = false;
                    if let Some(ws_id) = window.window_server_id {
                        self.visible_windows.remove(&ws_id);
                    }
                    self.send_layout_event(LayoutEvent::WindowRemoved(wid));
                } else {
                    debug!(?wid, "Received WindowMinimized for unknown window - ignoring");
                }
            }
            Event::WindowDeminiaturized(wid) => {
                let (frame, server_id, is_ax_standard, is_ax_root) =
                    match self.windows.get_mut(&wid) {
                        Some(window) => {
                            if !window.is_minimized {
                                return;
                            }
                            window.is_minimized = false;
                            (
                                window.frame_monotonic,
                                window.window_server_id,
                                window.is_ax_standard,
                                window.is_ax_root,
                            )
                        }
                        None => {
                            debug!(
                                ?wid,
                                "Received WindowDeminiaturized for unknown window - ignoring"
                            );
                            return;
                        }
                    };
                let is_manageable = self.compute_manageability_from_parts(
                    server_id,
                    false,
                    is_ax_standard,
                    is_ax_root,
                );
                if let Some(window) = self.windows.get_mut(&wid) {
                    window.is_manageable = is_manageable;
                }

                if is_manageable {
                    if let Some(space) = self.best_space_for_window(&frame) {
                        self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                    }
                }
            }
            Event::WindowFrameChanged(wid, new_frame, last_seen, requested, mouse_state) => {
                if let Some(window) = self.windows.get_mut(&wid) {
                    if self.mission_control_active
                        || window
                            .window_server_id
                            .is_some_and(|wsid| self.changing_screens.contains(&wsid))
                    {
                        return;
                    }
                    let triggered_by_rift =
                        last_seen.is_some_and(|seen| seen == window.last_sent_txid);
                    if let Some(last_seen) = last_seen
                        && last_seen != window.last_sent_txid
                    {
                        // Ignore events that happened before the last time we
                        // changed the size or position of this window. Otherwise
                        // we would update the layout model incorrectly.
                        debug!(?last_seen, ?window.last_sent_txid, "Ignoring frame change");
                        return;
                    }
                    if requested.0 {
                        // TODO: If the size is different from requested, applying a
                        // correction to the model can result in weird feedback
                        // loops, so we ignore these for now.
                        return;
                    }
                    if triggered_by_rift {
                        if let Some(store) = self.window_tx_store.as_ref()
                            && let Some(wsid) = window.window_server_id
                        {
                            if let Some(record) = store.get(&wsid) {
                                if new_frame.same_as(record.target) {
                                    if !window.frame_monotonic.same_as(new_frame) {
                                        debug!(
                                            ?wid,
                                            ?new_frame,
                                            "Final frame matches Rift request"
                                        );
                                        window.frame_monotonic = new_frame;
                                    }
                                    store.remove(&wsid);
                                } else {
                                    trace!(
                                        ?wid,
                                        ?new_frame,
                                        ?record.target,
                                        "Skipping intermediate frame from Rift request"
                                    );
                                }
                            } else if !window.frame_monotonic.same_as(new_frame) {
                                debug!(
                                    ?wid,
                                    ?new_frame,
                                    "Rift frame event missing tx record; updating state"
                                );
                                window.frame_monotonic = new_frame;
                            }
                        } else if !window.frame_monotonic.same_as(new_frame) {
                            debug!(
                                ?wid,
                                ?new_frame,
                                "Rift frame event without store; updating state"
                            );
                            window.frame_monotonic = new_frame;
                        }
                        return;
                    }
                    let old_frame = mem::replace(&mut window.frame_monotonic, new_frame);
                    if old_frame == new_frame {
                        return;
                    }

                    let dragging = mouse_state == Some(MouseState::Down) || self.in_drag;

                    if dragging {
                        if !self.in_drag {
                            self.in_drag = true;
                        }
                        self.ensure_active_drag(wid, &old_frame);
                        self.update_active_drag(wid, &new_frame);
                        if old_frame.size != new_frame.size {
                            self.mark_drag_dirty(wid);
                        }
                        self.maybe_swap_on_drag(wid, new_frame);
                    } else {
                        let screens = self
                            .screens
                            .iter()
                            .flat_map(|screen| Some((screen.space?, screen.frame)))
                            .collect::<Vec<_>>();

                        let old_space = self.best_space_for_window(&old_frame);
                        let new_space = self.best_space_for_window(&new_frame);

                        if old_space != new_space {
                            if self.in_drag
                                || self.active_drag.as_ref().is_some_and(|s| s.window == wid)
                            {
                                if let Some(space) = new_space {
                                    if let Some(session) = self.active_drag.as_mut() {
                                        if session.window == wid {
                                            session.settled_space = Some(space);
                                            session.layout_dirty = true;
                                        }
                                    }
                                }
                            } else {
                                if let Some(space) = new_space {
                                    if let Some(active_ws) =
                                        self.layout_engine.active_workspace(space)
                                    {
                                        let _ = self
                                            .layout_engine
                                            .virtual_workspace_manager_mut()
                                            .assign_window_to_workspace(space, wid, active_ws);
                                    }
                                    self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                                    let _ = self.update_layout(false, false);
                                } else {
                                    self.send_layout_event(LayoutEvent::WindowRemoved(wid));
                                    let _ = self.update_layout(false, false);
                                }
                            }
                        } else if old_frame.size != new_frame.size {
                            self.send_layout_event(LayoutEvent::WindowResized {
                                wid,
                                old_frame,
                                new_frame,
                                screens,
                            });
                            is_resize = true;
                        }
                    }
                }
            }
            Event::ScreenParametersChanged(frames, spaces, ws_info) => {
                info!("screen parameters changed");
                let spaces_all_none = spaces.iter().all(|space| space.is_none());
                self.suppress_stale_window_cleanup = spaces_all_none;
                let mut ws_info_opt = Some(ws_info);
                if frames.is_empty() {
                    if spaces.is_empty() {
                        if !self.screens.is_empty() {
                            self.screens.clear();
                            self.expose_all_spaces();
                        }
                    } else if spaces.len() == self.screens.len() {
                        self.set_screen_spaces(&spaces);
                        if let Some(info) = ws_info_opt.take() {
                            self.finalize_space_change(&spaces, info);
                        }
                    } else {
                        warn!(
                            "Ignoring empty screen update: we have {} screens, but {} spaces",
                            self.screens.len(),
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
                    self.screens = frames
                        .into_iter()
                        .zip(spaces.into_iter())
                        .map(|(frame, space)| Screen { frame, space })
                        .collect();
                    if let Some(info) = ws_info_opt.take() {
                        self.finalize_space_change(&spaces_clone, info);
                    }
                }
                if let Some(info) = ws_info_opt.take() {
                    self.update_complete_window_server_info(info);
                }
                self.try_apply_pending_space_change();
            }
            Event::SpaceChanged(spaces, ws_info) => {
                // TODO: this logic is flawed if multiple spaces are changing at once
                if self.handle_fullscreen_space_transition(&spaces) {
                    return;
                }
                if self.mission_control_active {
                    // dont process whilst mc is active
                    self.pending_space_change = Some(PendingSpaceChange { spaces, ws_info });
                    return;
                }
                let spaces_all_none = spaces.iter().all(|space| space.is_none());
                self.suppress_stale_window_cleanup = spaces_all_none;
                if spaces.len() != self.screens.len() {
                    warn!(
                        "Deferring space change: have {} screens but {} spaces",
                        self.screens.len(),
                        spaces.len()
                    );
                    self.pending_space_change = Some(PendingSpaceChange { spaces, ws_info });
                    return;
                }
                info!("space changed");
                self.pending_space_change = None;
                self.set_screen_spaces(&spaces);
                self.finalize_space_change(&spaces, ws_info);
            }
            Event::MouseUp => {
                self.in_drag = false;

                let mut need_layout_refresh = false;

                if let Some((dragged_wid, target_wid)) = self.pending_drag_swap.take() {
                    trace!(?dragged_wid, ?target_wid, "Performing deferred swap on MouseUp");

                    self.skip_layout_for_window = Some(dragged_wid);

                    if !self.windows.contains_key(&dragged_wid)
                        || !self.windows.contains_key(&target_wid)
                    {
                        trace!(
                            ?dragged_wid,
                            ?target_wid,
                            "Skipping deferred swap; one of the windows no longer exists"
                        );
                    } else {
                        let visible_spaces =
                            self.screens.iter().flat_map(|s| s.space).collect::<Vec<_>>();

                        let swap_space = self
                            .windows
                            .get(&dragged_wid)
                            .and_then(|w| self.best_space_for_window(&w.frame_monotonic))
                            .or_else(|| {
                                self.drag_manager
                                    .origin_frame()
                                    .and_then(|f| self.best_space_for_window(&f))
                            })
                            .or_else(|| self.screens.iter().find_map(|s| s.space));
                        let response = self.layout_engine.handle_command(
                            swap_space,
                            &visible_spaces,
                            layout::LayoutCommand::SwapWindows(dragged_wid, target_wid),
                        );
                        self.handle_layout_response(response);

                        need_layout_refresh = true;
                    }
                }

                let finalize_needs_layout = self.finalize_active_drag();

                self.drag_manager.reset();

                if finalize_needs_layout {
                    need_layout_refresh = true;
                }

                if need_layout_refresh {
                    let _ = self.update_layout(false, false);
                }

                self.skip_layout_for_window = None;
            }
            Event::MenuOpened => {
                debug!("menu opened");
                self.menu_open_depth = self.menu_open_depth.saturating_add(1);
                self.update_focus_follows_mouse_state();
            }
            Event::MenuClosed => {
                if self.menu_open_depth == 0 {
                    debug!("menu closed with zero depth");
                } else {
                    self.menu_open_depth -= 1;
                    self.update_focus_follows_mouse_state();
                }
            }
            Event::MouseMovedOverWindow(wsid) => {
                let Some(&wid) = self.window_ids.get(&wsid) else { return };
                if self.should_raise_on_mouse_over(wid) {
                    self.raise_window(wid, Quiet::No, None);
                }
            }
            Event::SystemWoke => {
                let ids: Vec<u32> = self.window_ids.keys().map(|wsid| wsid.as_u32()).collect();
                crate::sys::window_notify::update_window_notifications(&ids);
                self.last_sls_notification_ids = ids;
            }
            Event::MissionControlNativeEntered => {
                self.set_mission_control_active(true);
            }
            Event::MissionControlNativeExited => {
                if self.mission_control_active {
                    self.set_mission_control_active(false);
                }
                self.refresh_windows_after_mission_control();
            }
            Event::RaiseCompleted { window_id, sequence_id } => {
                let msg = raise_manager::Event::RaiseCompleted { window_id, sequence_id };
                _ = self.raise_manager_tx.send(msg);
            }
            Event::RaiseTimeout { sequence_id } => {
                let msg = raise_manager::Event::RaiseTimeout { sequence_id };
                _ = self.raise_manager_tx.send(msg);
            }
            Event::Command(Command::Layout(cmd)) => {
                info!(?cmd);
                let visible_spaces =
                    self.screens.iter().flat_map(|screen| screen.space).collect::<Vec<_>>();

                let is_workspace_switch = matches!(
                    cmd,
                    LayoutCommand::NextWorkspace(_)
                        | LayoutCommand::PrevWorkspace(_)
                        | LayoutCommand::SwitchToWorkspace(_)
                        | LayoutCommand::SwitchToLastWorkspace
                );
                if is_workspace_switch {
                    if let Some(space) = self.workspace_command_space() {
                        self.store_current_floating_positions(space);
                    }
                    self.workspace_switch_generation =
                        self.workspace_switch_generation.wrapping_add(1);
                    self.active_workspace_switch = Some(self.workspace_switch_generation);
                }

                let response = match &cmd {
                    LayoutCommand::NextWorkspace(_)
                    | LayoutCommand::PrevWorkspace(_)
                    | LayoutCommand::SwitchToWorkspace(_)
                    | LayoutCommand::MoveWindowToWorkspace(_)
                    | LayoutCommand::CreateWorkspace
                    | LayoutCommand::SwitchToLastWorkspace => {
                        if let Some(space) = self.workspace_command_space() {
                            self.layout_engine.handle_virtual_workspace_command(space, &cmd)
                        } else {
                            layout::EventResponse::default()
                        }
                    }
                    _ => self.layout_engine.handle_command(
                        self.workspace_command_space(),
                        &visible_spaces,
                        cmd,
                    ),
                };

                self.is_workspace_switch = is_workspace_switch;
                self.handle_layout_response(response);
            }
            Event::Command(Command::Metrics(cmd)) => log::handle_command(cmd),
            Event::ConfigUpdated(new_cfg) => {
                let old_keys = self.config.keys.clone();

                self.config = new_cfg;
                self.layout_engine.set_layout_settings(&self.config.settings.layout);
                let _ = self.drag_manager.update_config(self.config.settings.window_snapping);

                if let Some(tx) = &self.stack_line_tx {
                    let _ = tx.try_send(crate::actor::stack_line::Event::ConfigUpdated(
                        self.config.clone(),
                    ));
                }

                let _ = self.update_layout(false, true);

                if old_keys != self.config.keys {
                    if let Some(wm) = &self.wm_sender {
                        let _ = wm.send(crate::actor::wm_controller::WmEvent::ConfigUpdated(
                            self.config.clone(),
                        ));
                    }
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::Debug)) => {
                for screen in &self.screens {
                    if let Some(space) = screen.space {
                        self.layout_engine.debug_tree_desc(space, "", true);
                    }
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::Serialize)) => {
                if let Ok(state) = self.serialize_state() {
                    println!("{}", state);
                    return;
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::SaveAndExit)) => {
                match self.layout_engine.save(crate::common::config::restore_file()) {
                    Ok(()) => std::process::exit(0),
                    Err(e) => {
                        error!("Could not save layout: {e}");
                        std::process::exit(3);
                    }
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::SwitchSpace(dir))) => unsafe {
                crate::sys::window_server::switch_space(dir)
            },
            Event::Command(Command::Reactor(ReactorCommand::FocusWindow {
                window_id: wid,
                window_server_id,
            })) => {
                if self.windows.contains_key(&wid) {
                    if let Some(space) = self
                        .windows
                        .get(&wid)
                        .and_then(|w| self.best_space_for_window(&w.frame_monotonic))
                    {
                        self.send_layout_event(LayoutEvent::WindowFocused(space, wid));
                    }

                    let mut app_handles: HashMap<i32, AppThreadHandle> = HashMap::default();
                    if let Some(app) = self.apps.get(&wid.pid) {
                        app_handles.insert(wid.pid, app.handle.clone());
                    }
                    let request = raise_manager::Event::RaiseRequest(RaiseRequest {
                        raise_windows: Vec::new(),
                        focus_window: Some((wid, None)),
                        app_handles,
                    });
                    let _ = self.raise_manager_tx.try_send(request);
                } else if let Some(wsid) = window_server_id {
                    let _ = window_server::make_key_window(wid.pid, wsid);
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::SetMissionControlActive(active))) => {
                self.set_mission_control_active(active);
            }
            _ => (),
        }
        if let Some(raised_window) = raised_window {
            if let Some(space) = self
                .windows
                .get(&raised_window)
                .and_then(|w| self.best_space_for_window(&w.frame_monotonic))
            {
                self.send_layout_event(LayoutEvent::WindowFocused(space, raised_window));
            }
        }

        let mut layout_changed = false;
        if !self.in_drag || window_was_destroyed {
            layout_changed = self.update_layout(is_resize, self.is_workspace_switch);
            self.maybe_send_menu_update();
        }

        self.is_workspace_switch = false;
        if self.active_workspace_switch.is_some() && !layout_changed {
            self.active_workspace_switch = None;
            trace!("Workspace switch stabilized with no further frame changes");
        }

        if should_update_notifications {
            let mut ids: Vec<u32> = self.window_ids.keys().map(|wsid| wsid.as_u32()).collect();
            ids.sort_unstable();

            if ids != self.last_sls_notification_ids {
                crate::sys::window_notify::update_window_notifications(&ids);

                self.last_sls_notification_ids = ids;
            }
        }
    }

    fn create_window_data(&self, window_id: WindowId) -> Option<WindowData> {
        let window_state = self.windows.get(&window_id)?;
        if window_state.is_minimized {
            return None;
        }
        let app = self.apps.get(&window_id.pid)?;

        let preferred_name = app.info.localized_name.clone().or_else(|| app.info.bundle_id.clone());

        Some(WindowData {
            id: window_id,
            title: window_state.title.clone(),
            frame: window_state.frame_monotonic,
            is_floating: self.layout_engine.is_window_floating(window_id),
            is_focused: self.main_window() == Some(window_id),
            bundle_id: preferred_name,
            window_server_id: window_state.window_server_id.map(|wsid| wsid.as_u32()),
        })
    }

    fn update_complete_window_server_info(&mut self, ws_info: Vec<WindowServerInfo>) {
        self.visible_windows.clear();
        self.update_partial_window_server_info(ws_info);
    }

    fn update_partial_window_server_info(&mut self, ws_info: Vec<WindowServerInfo>) {
        // Mark visible windows and remove any corresponding observed WSID markers
        // for ids we now have server info for.
        self.visible_windows.extend(ws_info.iter().map(|info| info.id));
        for info in ws_info.iter() {
            // If we've been observing this server id from SLS callbacks, clear it.
            self.observed_window_server_ids.remove(&info.id);
            self.window_server_info.insert(info.id, *info);

            if let Some(wid) = self.window_ids.get(&info.id).copied() {
                let (server_id, is_minimized, is_ax_standard, is_ax_root) =
                    if let Some(window) = self.windows.get_mut(&wid) {
                        if info.layer == 0 {
                            window.frame_monotonic = info.frame;
                        }
                        (
                            window.window_server_id,
                            window.is_minimized,
                            window.is_ax_standard,
                            window.is_ax_root,
                        )
                    } else {
                        continue;
                    };
                let manageable = self.compute_manageability_from_parts(
                    server_id,
                    is_minimized,
                    is_ax_standard,
                    is_ax_root,
                );
                if let Some(window) = self.windows.get_mut(&wid) {
                    window.is_manageable = manageable;
                }
            }
        }
    }

    fn check_for_new_windows(&mut self) {
        // TODO: Do this correctly/more optimally using CGWindowListCopyWindowInfo
        // (see notes for on_windows_discovered below).
        for app in self.apps.values_mut() {
            // Errors mean the app terminated (and a termination event
            // is coming); ignore.
            _ = app.handle.send(Request::GetVisibleWindows { force_refresh: false });
        }
    }

    fn handle_fullscreen_space_transition(&mut self, spaces: &[Option<SpaceId>]) -> bool {
        if spaces.iter().flatten().any(|s| space_is_fullscreen(s.get())) {
            return true;
        }

        for space in spaces.iter().flatten() {
            if let Some(track) = self.fullscreen_by_space.remove(&space.get()) {
                wait_for_native_fullscreen_transition();
                Timer::sleep(Duration::from_millis(50));
                for pid in track.pids {
                    if let Some(app) = self.apps.get(&pid) {
                        let _ = app.handle.send(Request::GetVisibleWindows { force_refresh: true });
                    }
                }
                if let Some(space) = self.screens.iter().flat_map(|s| s.space).next() {
                    self.pending_refocus_space = Some(space);
                    let _ = self.update_layout(false, false);
                    self.update_focus_follows_mouse_state();
                }
            }
        }

        false
    }

    fn set_screen_spaces(&mut self, spaces: &[Option<SpaceId>]) {
        for (space, screen) in spaces.iter().copied().zip(&mut self.screens) {
            screen.space = space;
        }
    }

    fn finalize_space_change(
        &mut self,
        spaces: &[Option<SpaceId>],
        ws_info: Vec<WindowServerInfo>,
    ) {
        self.suppress_stale_window_cleanup = spaces.iter().all(|space| space.is_none());
        self.expose_all_spaces();
        self.changing_screens.clear();
        if let Some(main_window) = self.main_window() {
            if let Some(space) = self.main_window_space() {
                self.send_layout_event(LayoutEvent::WindowFocused(space, main_window));
            }
        }
        self.update_complete_window_server_info(ws_info);
        self.check_for_new_windows();

        if let Some(space) = spaces.iter().copied().flatten().next() {
            if let Some(workspace_id) = self.layout_engine.active_workspace(space) {
                let workspace_name = self
                    .layout_engine
                    .workspace_name(space, workspace_id)
                    .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));
                let broadcast_event = BroadcastEvent::WorkspaceChanged {
                    workspace_id,
                    workspace_name,
                    space_id: space,
                };
                _ = self.event_broadcaster.send(broadcast_event);
            }
        }
    }

    fn try_apply_pending_space_change(&mut self) {
        if let Some(pending) = self.pending_space_change.take() {
            if pending.spaces.len() == self.screens.len() {
                if self.handle_fullscreen_space_transition(&pending.spaces) {
                    return;
                }
                self.set_screen_spaces(&pending.spaces);
                self.finalize_space_change(&pending.spaces, pending.ws_info);
            } else {
                self.pending_space_change = Some(pending);
            }
        }
    }

    fn on_windows_discovered_with_app_info(
        &mut self,
        pid: pid_t,
        new: Vec<(WindowId, WindowInfo)>,
        known_visible: Vec<WindowId>,
        app_info: Option<AppInfo>,
    ) {
        // If app_info wasn't provided, try to look it up from our running app state so
        // we can apply workspace rules immediately on first discovery.
        let app_info = app_info.or_else(|| self.apps.get(&pid).map(|app| app.info.clone()));

        const MIN_REAL_WINDOW_DIMENSION: f64 = 2.0;

        let known_visible_set: HashSet<WindowId> = known_visible.into_iter().collect();
        let pending_refresh = self.pending_mission_control_refresh.contains(&pid);

        let has_window_server_visibles_without_ax = {
            let known_visible_set = &known_visible_set;
            self.visible_windows
                .iter()
                .filter_map(|wsid| self.window_ids.get(wsid))
                .any(|wid| wid.pid == pid && !known_visible_set.contains(wid))
        };

        let skip_stale_cleanup = self.suppress_stale_window_cleanup
            || pending_refresh
            || self.mission_control_active
            || self.in_drag
            || self.pid_has_changing_screens(pid)
            || self.active_drag.as_ref().map_or(false, |s| s.window.pid == pid)
            || (known_visible_set.is_empty() && !self.has_visible_window_server_ids_for_pid(pid))
            || has_window_server_visibles_without_ax;

        let stale_windows: Vec<WindowId> = if skip_stale_cleanup {
            Vec::new()
        } else {
            self.windows
                .iter()
                .filter_map(|(&wid, state)| {
                    if wid.pid != pid || known_visible_set.contains(&wid) {
                        return None;
                    }

                    if state.is_minimized {
                        return None;
                    }

                    let Some(ws_id) = state.window_server_id else {
                        trace!(
                            ?wid,
                            "Skipping stale cleanup for window without window server id"
                        );
                        return None;
                    };

                    let server_info = self
                        .window_server_info
                        .get(&ws_id)
                        .cloned()
                        .or_else(|| window_server::get_window(ws_id));

                    let info = match server_info {
                        Some(info) => info,
                        None => {
                            trace!(
                                ?wid,
                                ws_id = ?ws_id,
                                "Skipping stale cleanup for window without server info"
                            );
                            return None;
                        }
                    };

                    let width = info.frame.size.width.abs();
                    let height = info.frame.size.height.abs();

                    let unsuitable = !window_server::app_window_suitable(ws_id);
                    let invalid_layer = info.layer != 0;
                    let too_small =
                        width < MIN_REAL_WINDOW_DIMENSION || height < MIN_REAL_WINDOW_DIMENSION;
                    let ordered_in = window_server::window_is_ordered_in(ws_id);
                    let visible_in_snapshot = self.visible_windows.contains(&ws_id);

                    let window_space = self.best_space_for_window(&info.frame);
                    let is_on_visible_space = window_space.map_or(false, |s| {
                        self.screens.iter().flat_map(|sc| sc.space).any(|vs| vs == s)
                    });

                    if unsuitable
                        || invalid_layer
                        || too_small
                        || (is_on_visible_space && !ordered_in && !visible_in_snapshot)
                    {
                        Some(wid)
                    } else {
                        None
                    }
                })
                .collect()
        };

        for wid in stale_windows {
            self.handle_event(Event::WindowDestroyed(wid));
        }

        if pending_refresh {
            self.pending_mission_control_refresh.remove(&pid);
        }

        let time_since_app_rules = self.app_rules_recently_applied.elapsed();
        let app_rules_recently_applied = time_since_app_rules.as_secs() < 1;

        if app_rules_recently_applied && app_info.is_none() && !new.is_empty() {
            // Update state for any newly reported windows, but do not early-return;
            // proceed to emit WindowsOnScreenUpdated so existing mappings are respected
            // without reapplying app rules.
            for i in 0..new.len() {
                let (wid, ref info) = new[i];
                if let Some(wsid) = info.sys_id {
                    self.window_ids.insert(wsid, wid);
                }
                if self.windows.contains_key(&wid) {
                    let manageable = self.compute_manageability_from_parts(
                        info.sys_id,
                        info.is_minimized,
                        info.is_standard,
                        info.is_root,
                    );
                    if let Some(existing) = self.windows.get_mut(&wid) {
                        existing.title = info.title.clone();
                        if info.frame.size.width != 0.0 || info.frame.size.height != 0.0 {
                            existing.frame_monotonic = info.frame;
                        }
                        existing.is_ax_standard = info.is_standard;
                        existing.is_ax_root = info.is_root;
                        existing.is_minimized = info.is_minimized;
                        existing.window_server_id = info.sys_id;
                        existing.bundle_id = info.bundle_id.clone();
                        existing.bundle_path = info.path.clone();
                        existing.ax_role = info.ax_role.clone();
                        existing.ax_subrole = info.ax_subrole.clone();
                        existing.is_manageable = manageable;
                    }
                } else {
                    let mut state: WindowState = WindowState {
                        title: info.title.clone(),
                        frame_monotonic: info.frame,
                        is_ax_standard: info.is_standard,
                        is_ax_root: info.is_root,
                        is_minimized: info.is_minimized,
                        is_manageable: false,
                        last_sent_txid: TransactionId::default(),
                        window_server_id: info.sys_id,
                        bundle_id: info.bundle_id.clone(),
                        bundle_path: info.path.clone(),
                        ax_role: info.ax_role.clone(),
                        ax_subrole: info.ax_subrole.clone(),
                    };
                    let manageable = self.compute_window_manageability(&state);
                    state.is_manageable = manageable;
                    self.windows.insert(wid, state);
                }
            }
            // fall through
        }

        // Note that we rely on the window server info, not accessibility, to
        // tell us which windows are visible.
        //
        // The accessibility APIs report that there are no visible windows when
        // at a login screen, for instance, but there is not a corresponding
        // system notification to use as context. Even if there were, lining
        // them up with the responses we get from the app would be unreliable.
        //
        // We therefore do not let accessibility `.windows()` results remove
        // known windows from the visible list. Doing so incorrectly would cause
        // us to destroy the layout. We do wait for windows to become initially
        // known to accesibility before adding them to the layout, but that is
        // not generally problematic.
        //
        // TODO: Notice when returning from the login screen and ask again for
        // undiscovered windows.
        for (wid, info) in new {
            if let Some(wsid) = info.sys_id {
                self.window_ids.insert(wsid, wid);
            }
            if self.windows.contains_key(&wid) {
                // Refresh existing window state (frame/title/ax attrs/minimized) without
                // losing workspace or layout node mapping.
                let manageable = self.compute_manageability_from_parts(
                    info.sys_id,
                    info.is_minimized,
                    info.is_standard,
                    info.is_root,
                );
                if let Some(existing) = self.windows.get_mut(&wid) {
                    existing.title = info.title.clone();
                    if info.frame.size.width != 0.0 || info.frame.size.height != 0.0 {
                        existing.frame_monotonic = info.frame;
                    }
                    existing.is_ax_standard = info.is_standard;
                    existing.is_ax_root = info.is_root;
                    existing.is_minimized = info.is_minimized;
                    existing.window_server_id = info.sys_id;
                    existing.bundle_id = info.bundle_id.clone();
                    existing.bundle_path = info.path.clone();
                    existing.ax_role = info.ax_role.clone();
                    existing.ax_subrole = info.ax_subrole.clone();
                    existing.is_manageable = manageable;
                }
            } else {
                let mut state: WindowState = info.into();
                let manageable = self.compute_window_manageability(&state);
                state.is_manageable = manageable;
                self.windows.insert(wid, state);
            }
        }
        if !self.windows.iter().any(|(wid, _)| wid.pid == pid) {
            return;
        }
        let mut app_windows: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
        let mut included: HashSet<WindowId> = HashSet::default();
        for wid in self
            .visible_windows
            .iter()
            .flat_map(|wsid| self.window_ids.get(wsid))
            .copied()
            .filter(|wid| wid.pid == pid)
            .filter(|wid| self.window_is_standard(*wid))
        {
            let Some(space) = self.best_space_for_window(&self.windows[&wid].frame_monotonic)
            else {
                continue;
            };
            included.insert(wid);
            app_windows.entry(space).or_default().push(wid);
        }
        // If we have no visible WSIDs (e.g., SpaceChanged provided empty ws_info),
        // fall back to the app-reported known_visible list for this pid.
        for wid in known_visible_set.iter().copied().filter(|wid| wid.pid == pid) {
            if included.contains(&wid) || !self.window_is_standard(wid) {
                continue;
            }
            let Some(state) = self.windows.get(&wid) else { continue };
            let Some(space) = self.best_space_for_window(&state.frame_monotonic) else {
                continue;
            };
            included.insert(wid);
            app_windows.entry(space).or_default().push(wid);
        }
        let screens = self.screens.clone();
        for screen in screens {
            let Some(space) = screen.space else { continue };
            let windows_for_space = app_windows.remove(&space).unwrap_or_default();

            if !windows_for_space.is_empty() {
                for wid in &windows_for_space {
                    let title_opt = self.windows.get(wid).map(|w| w.title.clone());
                    let _ = self
                        .layout_engine
                        .virtual_workspace_manager_mut()
                        .assign_window_with_app_info(
                            *wid,
                            space,
                            app_info.as_ref().and_then(|a| a.bundle_id.as_deref()),
                            app_info.as_ref().and_then(|a| a.localized_name.as_deref()),
                            title_opt.as_deref(),
                            self.windows.get(wid).and_then(|w| w.ax_role.as_deref()),
                            self.windows.get(wid).and_then(|w| w.ax_subrole.as_deref()),
                        );
                }
            }

            let windows_with_titles: Vec<(
                WindowId,
                Option<String>,
                Option<String>,
                Option<String>,
            )> = windows_for_space
                .iter()
                .map(|&wid| {
                    let title_opt = self.windows.get(&wid).map(|w| w.title.clone());
                    let ax_role = self.windows.get(&wid).and_then(|w| w.ax_role.clone());
                    let ax_subrole = self.windows.get(&wid).and_then(|w| w.ax_subrole.clone());
                    (wid, title_opt, ax_role, ax_subrole)
                })
                .collect();

            self.send_layout_event(LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                windows_with_titles.clone(),
                app_info.clone(),
            ));
        }

        if let Some(main_window) = self.main_window() {
            if main_window.pid == pid {
                if let Some(space) = self.main_window_space() {
                    self.send_layout_event(LayoutEvent::WindowFocused(space, main_window));
                }
            }
        }
    }

    fn best_space_for_window(&self, frame: &CGRect) -> Option<SpaceId> {
        let center = frame.mid();
        self.screens
            .iter()
            .find_map(|s| {
                let space = s.space?;
                if s.frame.contains(center) {
                    Some(space)
                } else {
                    None
                }
            })
            .or_else(|| {
                self.screens
                    .iter()
                    .max_by_key(|s| s.frame.intersection(frame).area() as i64)?
                    .space
            })
    }

    fn ensure_active_drag(&mut self, wid: WindowId, frame: &CGRect) {
        let needs_new_session =
            self.active_drag.as_ref().map_or(true, |session| session.window != wid);
        if needs_new_session {
            let origin_space = self.best_space_for_window(frame);
            self.active_drag = Some(DragSession {
                window: wid,
                last_frame: *frame,
                origin_space,
                settled_space: origin_space,
                layout_dirty: false,
            });
        }
        if self.skip_layout_for_window != Some(wid) {
            self.skip_layout_for_window = Some(wid);
        }
    }

    fn update_active_drag(&mut self, wid: WindowId, new_frame: &CGRect) {
        let resolved_space = match self.active_drag.as_ref() {
            Some(session) if session.window == wid => self.resolve_drag_space(session, new_frame),
            _ => return,
        };

        if let Some(session) = self.active_drag.as_mut() {
            if session.window != wid {
                return;
            }
            session.last_frame = *new_frame;
            if session.settled_space != resolved_space {
                session.settled_space = resolved_space;
                session.layout_dirty = true;
                self.skip_layout_for_window = Some(session.window);
            }
        }
    }

    fn mark_drag_dirty(&mut self, wid: WindowId) {
        if let Some(session) = self.active_drag.as_mut() {
            if session.window == wid {
                session.layout_dirty = true;
                if self.skip_layout_for_window != Some(wid) {
                    self.skip_layout_for_window = Some(wid);
                }
            }
        }
    }

    fn drag_space_candidate(&self, frame: &CGRect) -> Option<SpaceId> {
        let center = frame.mid();
        self.screens.iter().find_map(|screen| {
            let space = screen.space?;
            if screen.frame.contains(center) {
                Some(space)
            } else {
                None
            }
        })
    }

    fn resolve_drag_space(&self, session: &DragSession, frame: &CGRect) -> Option<SpaceId> {
        if frame.area() <= 0.0 {
            return session.settled_space.or_else(|| self.best_space_for_window(frame));
        }

        self.drag_space_candidate(frame)
            .or_else(|| self.best_space_for_window(frame))
            .or(session.settled_space)
    }

    fn finalize_active_drag(&mut self) -> bool {
        let Some(session) = self.active_drag.take() else {
            return false;
        };
        let wid = session.window;

        let final_space = self
            .windows
            .get(&wid)
            .and_then(|window| self.best_space_for_window(&window.frame_monotonic));

        if session.origin_space != final_space {
            if session.origin_space.is_some() {
                self.send_layout_event(LayoutEvent::WindowRemoved(wid));
            }
            if let Some(space) = final_space {
                if let Some(active_ws) = self.layout_engine.active_workspace(space) {
                    let _ = self
                        .layout_engine
                        .virtual_workspace_manager_mut()
                        .assign_window_to_workspace(space, wid, active_ws);
                }
                self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
            }
            self.skip_layout_for_window = Some(wid);
            true
        } else if session.layout_dirty {
            self.skip_layout_for_window = Some(wid);
            true
        } else {
            false
        }
    }

    fn pid_has_changing_screens(&self, pid: pid_t) -> bool {
        self.changing_screens.iter().any(|wsid| {
            if let Some(wid) = self.window_ids.get(wsid) {
                wid.pid == pid
            } else if let Some(info) = self.window_server_info.get(wsid) {
                info.pid == pid
            } else if let Some(info) = crate::sys::window_server::get_window(*wsid) {
                info.pid == pid
            } else {
                false
            }
        })
    }

    fn has_visible_window_server_ids_for_pid(&self, pid: pid_t) -> bool {
        self.visible_windows
            .iter()
            .any(|wsid| self.window_ids.get(wsid).map_or(false, |wid| wid.pid == pid))
    }

    fn expose_all_spaces(&mut self) {
        let screens = self.screens.clone();
        for screen in screens {
            let Some(space) = screen.space else { continue };
            let _ = self.layout_engine.virtual_workspace_manager_mut().list_workspaces(space);
            self.send_layout_event(LayoutEvent::SpaceExposed(space, screen.frame.size));
        }
    }

    fn compute_window_manageability(&self, window: &WindowState) -> bool {
        self.compute_manageability_from_parts(
            window.window_server_id,
            window.is_minimized,
            window.is_ax_standard,
            window.is_ax_root,
        )
    }

    fn compute_manageability_from_parts(
        &self,
        window_server_id: Option<WindowServerId>,
        is_minimized: bool,
        is_ax_standard: bool,
        is_ax_root: bool,
    ) -> bool {
        if is_minimized {
            return false;
        }

        if let Some(wsid) = window_server_id {
            if let Some(info) = self.window_server_info.get(&wsid) {
                if info.layer != 0 {
                    return false;
                }
            }
            if window_server::window_is_sticky(wsid) {
                return false;
            }

            if let Some(level) = window_server::window_level(wsid.0) {
                if level != NSNormalWindowLevel {
                    return false;
                }
            }
        }
        is_ax_standard && is_ax_root
    }

    fn window_is_standard(&self, id: WindowId) -> bool {
        self.windows.get(&id).map_or(false, |window| window.is_manageable)
    }

    fn send_layout_event(&mut self, event: LayoutEvent) {
        let event_clone = event.clone();
        let response = self.layout_engine.handle_event(event);
        self.prepare_refocus_after_layout_event(&event_clone);
        self.handle_layout_response(response);
        for space in self.screens.iter().flat_map(|screen| screen.space) {
            self.layout_engine.debug_tree_desc(space, "after event", false);
        }
    }

    // Returns true if the window should be raised on mouse over considering
    // active workspace membership and potential occlusion of other windows above it.
    fn should_raise_on_mouse_over(&self, wid: WindowId) -> bool {
        let Some(window) = self.windows.get(&wid) else {
            return false;
        };

        let candidate_frame = window.frame_monotonic;

        if self.menu_open_depth > 0 {
            trace!(?wid, "Skipping autoraise while menu open");
            return false;
        }

        let Some(space) = self.best_space_for_window(&candidate_frame) else {
            return false;
        };

        if !self.layout_engine.is_window_in_active_workspace(space, wid) {
            trace!("Ignoring mouse over window {:?} - not in active workspace", wid);
            return false;
        }

        let Some(candidate_wsid) = window.window_server_id else {
            return true;
        };
        let order = {
            let space_id = space.get();
            crate::sys::window_server::space_window_list_for_connection(&[space_id], 0, false)
        };
        let candidate_u32 = candidate_wsid.as_u32();

        for above_u32 in order {
            if above_u32 == candidate_u32 {
                break;
            }

            let above_wsid = WindowServerId::new(above_u32);
            let Some(&above_wid) = self.window_ids.get(&above_wsid) else {
                continue;
            };

            let Some(above_state) = self.windows.get(&above_wid) else {
                continue;
            };
            let above_frame = above_state.frame_monotonic;
            if candidate_frame.intersection(&above_frame).same_as(above_frame) {
                return false;
            }
        }

        true
    }

    fn process_windows_for_app_rules(
        &mut self,
        pid: pid_t,
        window_ids: Vec<WindowId>,
        app_info: AppInfo,
    ) {
        if window_ids.is_empty() {
            return;
        }

        let mut windows_by_space: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
        for &wid in &window_ids {
            let Some(state) = self.windows.get(&wid) else { continue };
            let Some(space) = self.best_space_for_window(&state.frame_monotonic) else {
                continue;
            };
            windows_by_space.entry(space).or_default().push(wid);
        }

        for (space, wids) in windows_by_space {
            for wid in &wids {
                let title_opt = self.windows.get(wid).map(|w| w.title.clone());
                let _ =
                    self.layout_engine.virtual_workspace_manager_mut().assign_window_with_app_info(
                        *wid,
                        space,
                        (&app_info.bundle_id).as_deref(),
                        (&app_info.localized_name).as_deref(),
                        title_opt.as_deref(),
                        self.windows.get(wid).and_then(|w| w.ax_role.as_deref()),
                        self.windows.get(wid).and_then(|w| w.ax_subrole.as_deref()),
                    );
            }

            let windows_with_titles: Vec<(
                WindowId,
                Option<String>,
                Option<String>,
                Option<String>,
            )> = wids
                .iter()
                .map(|&wid| {
                    let title_opt = self.windows.get(&wid).map(|w| w.title.clone());
                    let ax_role = self.windows.get(&wid).and_then(|w| w.ax_role.clone());
                    let ax_subrole = self.windows.get(&wid).and_then(|w| w.ax_subrole.clone());
                    (wid, title_opt, ax_role, ax_subrole)
                })
                .collect();

            self.send_layout_event(LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                windows_with_titles,
                Some(app_info.clone()),
            ));
        }
    }

    fn handle_app_activation_workspace_switch(&mut self, pid: pid_t) {
        use objc2_app_kit::NSRunningApplication;

        use crate::sys::app::NSRunningApplicationExt;

        if self.active_workspace_switch.is_some() {
            trace!(
                "Skipping auto workspace switch for pid {} because a workspace switch is in progress",
                pid
            );
            return;
        }

        let visible_spaces: HashSet<SpaceId> =
            self.screens.iter().filter_map(|s| s.space).collect();
        let app_is_on_visible_workspace = self.windows.iter().any(|(wid, window_state)| {
            if wid.pid != pid {
                return false;
            }
            if let Some(space) = self.best_space_for_window(&window_state.frame_monotonic) {
                if visible_spaces.contains(&space) {
                    if let Some(active_workspace) = self.layout_engine.active_workspace(space) {
                        if let Some(window_workspace) = self
                            .layout_engine
                            .virtual_workspace_manager()
                            .workspace_for_window(space, *wid)
                        {
                            return active_workspace == window_workspace;
                        }
                    }
                }
            }
            false
        });

        if app_is_on_visible_workspace {
            debug!("App {} is already on a visible workspace, not switching.", pid);
            return;
        }

        let Some(app) = NSRunningApplication::with_process_id(pid) else {
            return;
        };
        let Some(bundle_id) = app.bundle_id() else {
            return;
        };
        let bundle_id_str = bundle_id.to_string();

        if self.config.settings.auto_focus_blacklist.contains(&bundle_id_str) {
            debug!(
                "App {} is blacklisted for auto-focus workspace switching, ignoring activation",
                bundle_id_str
            );
            return;
        }

        debug!(
            "App activation detected: {} (pid: {}), checking for workspace switch",
            bundle_id_str, pid
        );

        let app_window = self
            .windows
            .keys()
            .find(|wid| wid.pid == pid && self.window_is_standard(**wid))
            .copied();

        let Some(app_window_id) = app_window else {
            return;
        };

        let Some(window_space) =
            self.best_space_for_window(&self.windows.get(&app_window_id).unwrap().frame_monotonic)
        else {
            return;
        };

        let workspace_manager = self.layout_engine.virtual_workspace_manager();
        let Some(window_workspace) =
            workspace_manager.workspace_for_window(window_space, app_window_id)
        else {
            return;
        };

        let Some(current_workspace) = self.layout_engine.active_workspace(window_space) else {
            return;
        };

        if window_workspace != current_workspace {
            const AUTO_SWITCH_BOUNCE_MS: u64 = 300;
            if let Some(last_switch) = &self.last_auto_workspace_switch {
                if last_switch.space == window_space
                    && last_switch.to_workspace == current_workspace
                    && last_switch.from_workspace == Some(window_workspace)
                    && last_switch.occurred_at.elapsed()
                        < std::time::Duration::from_millis(AUTO_SWITCH_BOUNCE_MS)
                {
                    debug!(
                        ?current_workspace,
                        ?window_workspace,
                        "Suppressing auto workspace switch to avoid rapid oscillation"
                    );
                    return;
                }
            }

            let workspaces =
                self.layout_engine.virtual_workspace_manager_mut().list_workspaces(window_space);
            if let Some((workspace_index, _)) =
                workspaces.iter().enumerate().find(|(_, (ws_id, _))| *ws_id == window_workspace)
            {
                debug!(
                    "Auto-switching to workspace {} for activated app (pid: {})",
                    workspace_index, pid
                );

                self.store_current_floating_positions(window_space);
                self.last_auto_workspace_switch = Some(AutoWorkspaceSwitch {
                    occurred_at: std::time::Instant::now(),
                    space: window_space,
                    from_workspace: Some(current_workspace),
                    to_workspace: window_workspace,
                });
                self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
                self.active_workspace_switch = Some(self.workspace_switch_generation);
                self.is_workspace_switch = true;

                let response = self.layout_engine.handle_virtual_workspace_command(
                    window_space,
                    &layout::LayoutCommand::SwitchToWorkspace(workspace_index),
                );
                self.handle_layout_response(response);
            }
        }
    }

    fn handle_layout_response(&mut self, response: layout::EventResponse) {
        if self.in_drag {
            self.is_workspace_switch = false;
            return;
        }

        let mut pending_refocus_space = self.pending_refocus_space.take();
        let layout::EventResponse {
            raise_windows,
            mut focus_window,
        } = response;
        let original_focus = focus_window;

        let mut handled_without_raise = false;

        if raise_windows.is_empty() && focus_window.is_none() {
            if self.is_workspace_switch && !self.in_drag {
                if let Some(wid) = self.window_id_under_cursor() {
                    focus_window = Some(wid);
                } else if self.focus_untracked_window_under_cursor() {
                    handled_without_raise = true;
                }
            } else if let Some(space) = pending_refocus_space.take() {
                if let Some(wid) = self.last_focused_window_in_space(space) {
                    focus_window = Some(wid);
                } else if !self.in_drag {
                    if let Some(wid) = self.window_id_under_cursor() {
                        focus_window = Some(wid);
                    } else if self.focus_untracked_window_under_cursor() {
                        handled_without_raise = true;
                    }
                }
            }
        }

        if let Some(wid) = focus_window {
            if let Some(state) = self.windows.get(&wid) {
                if let Some(wsid) = state.window_server_id {
                    if self.changing_screens.contains(&wsid)
                        || !self.visible_windows.contains(&wsid)
                    {
                        focus_window = None;
                    } else if let Some(space) = self.best_space_for_window(&state.frame_monotonic) {
                        if let Some(active_space) = self.screens.iter().flat_map(|s| s.space).next()
                        {
                            if space != active_space {
                                focus_window = None;
                            }
                        }
                    } else {
                        focus_window = None;
                    }
                }
            }
        }

        if handled_without_raise && raise_windows.is_empty() && focus_window.is_none() {
            self.is_workspace_switch = false;
            return;
        }

        if let Some(space) = pending_refocus_space {
            // Preserve the pending refocus request if it was not consumed above.
            if self.pending_refocus_space.is_none() {
                self.pending_refocus_space = Some(space);
            }
        }

        if raise_windows.is_empty() && focus_window.is_none() && !self.is_workspace_switch {
            return;
        }

        let mut app_handles = HashMap::default();
        for &wid in raise_windows.iter() {
            if let Some(app) = self.apps.get(&wid.pid) {
                app_handles.insert(wid.pid, app.handle.clone());
            }
        }

        if let Some(wid) = original_focus {
            if let Some(app) = self.apps.get(&wid.pid) {
                app_handles.insert(wid.pid, app.handle.clone());
            }
        }

        let mut windows_by_app_and_screen = HashMap::default();
        for &wid in &raise_windows {
            let Some(window) = self.windows.get(&wid) else { continue };
            windows_by_app_and_screen
                .entry((wid.pid, self.best_space_for_window(&window.frame_monotonic)))
                .or_insert(vec![])
                .push(wid);
        }

        let focus_window_with_warp = focus_window.map(|wid| {
            let warp = match self.config.settings.mouse_follows_focus {
                true => self.windows.get(&wid).map(|w| w.frame_monotonic.mid()),
                false => None,
            };
            (wid, warp)
        });

        let msg = raise_manager::Event::RaiseRequest(RaiseRequest {
            raise_windows: windows_by_app_and_screen.into_values().collect(),
            focus_window: focus_window_with_warp,
            app_handles,
        });

        _ = self.raise_manager_tx.send(msg);
    }

    fn maybe_swap_on_drag(&mut self, wid: WindowId, new_frame: CGRect) {
        if !self.in_drag {
            trace!(?wid, "Skipping swap: not in drag (mouse up received)");
            return;
        }

        {
            let Some(window) = self.windows.get(&wid) else {
                return;
            };

            if window
                .window_server_id
                .is_some_and(|wsid| self.changing_screens.contains(&wsid))
            {
                trace!(?wid, "Skipping swap: window is changing screens");
                return;
            }
        }

        let Some(space) = (if self.in_drag {
            self.active_drag
                .as_ref()
                .and_then(|session| session.settled_space)
                .or_else(|| self.best_space_for_window(&new_frame))
        } else {
            self.best_space_for_window(&new_frame)
        }) else {
            return;
        };

        let origin_space_hint =
            self.active_drag.as_ref().and_then(|session| session.origin_space).or_else(|| {
                self.drag_manager
                    .origin_frame()
                    .and_then(|frame| self.best_space_for_window(&frame))
            });

        if let Some(origin_space) = origin_space_hint {
            if origin_space != space {
                if let Some((pending_wid, pending_target)) = self.pending_drag_swap {
                    if pending_wid == wid {
                        trace!(
                            ?wid,
                            ?pending_target,
                            ?origin_space,
                            ?space,
                            "Clearing pending drag swap; dragged window entered new space"
                        );
                        self.pending_drag_swap = None;
                    }
                }
                trace!(
                    ?wid,
                    ?origin_space,
                    ?space,
                    "Resetting drag swap tracking after space change"
                );
                self.drag_manager.reset();
                return;
            }
        }

        if !self.layout_engine.is_window_in_active_workspace(space, wid) {
            return;
        }

        let mut candidates: Vec<(WindowId, CGRect)> = Vec::new();
        for (&other_wid, other_state) in &self.windows {
            if other_wid == wid {
                continue;
            }

            let Some(other_space) = self.best_space_for_window(&other_state.frame_monotonic) else {
                continue;
            };
            if other_space != space
                || !self.layout_engine.is_window_in_active_workspace(space, other_wid)
                || self.layout_engine.is_window_floating(other_wid)
            {
                continue;
            }

            candidates.push((other_wid, other_state.frame_monotonic));
        }

        let previous_pending = self.pending_drag_swap;
        let new_candidate = self.drag_manager.on_frame_change(wid, new_frame, &candidates);
        let active_target = self.drag_manager.last_target();

        if let Some(target_wid) = active_target {
            if new_candidate.is_some()
                || previous_pending.map(|(dragged, target)| (dragged, target))
                    != Some((wid, target_wid))
            {
                trace!(
                    ?wid,
                    ?target_wid,
                    "Detected swap candidate; deferring until MouseUp"
                );
            }

            self.pending_drag_swap = Some((wid, target_wid));

            self.skip_layout_for_window = Some(wid);
        } else {
            if let Some((pending_wid, pending_target)) = previous_pending {
                if pending_wid == wid {
                    trace!(
                        ?wid,
                        ?pending_target,
                        "Clearing pending drag swap; overlap ended before MouseUp"
                    );
                    self.pending_drag_swap = None;
                }
            }

            if self.skip_layout_for_window == Some(wid) {
                self.skip_layout_for_window = None;
            }
        }
        // wait for mouse::up before doing *anything*
    }

    fn window_id_under_cursor(&self) -> Option<WindowId> {
        let wsid = window_server::window_under_cursor()?;
        self.window_ids.get(&wsid).copied()
    }

    fn focus_untracked_window_under_cursor(&mut self) -> bool {
        let Some(wsid) = window_server::window_under_cursor() else {
            return false;
        };
        if self.window_ids.contains_key(&wsid) {
            return false;
        }

        let window_info = self
            .window_server_info
            .get(&wsid)
            .copied()
            .or_else(|| window_server::get_window(wsid));

        let Some(info) = window_info else { return false };
        window_server::make_key_window(info.pid, wsid).is_ok()
    }

    fn last_focused_window_in_space(&self, space: SpaceId) -> Option<WindowId> {
        let active_workspace = self.layout_engine.active_workspace(space)?;
        let wid = self
            .layout_engine
            .virtual_workspace_manager()
            .last_focused_window(space, active_workspace)?;
        let window = self.windows.get(&wid)?;

        if let Some(actual_space) = self.best_space_for_window(&window.frame_monotonic) {
            if actual_space != space {
                return None;
            }
        } else {
            return None;
        }
        if let Some(wsid) = window.window_server_id {
            if self.changing_screens.contains(&wsid) {
                return None;
            }
            if !self.visible_windows.contains(&wsid) {
                return None;
            }
        }
        Some(wid)
    }

    fn request_refocus_if_hidden(&mut self, space: SpaceId, window_id: WindowId) {
        let Some(active_workspace) = self.layout_engine.active_workspace(space) else {
            return;
        };
        let Some(window_workspace) = self
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(space, window_id)
        else {
            return;
        };

        if window_workspace != active_workspace {
            self.pending_refocus_space = Some(space);
        }
    }

    fn prepare_refocus_after_layout_event(&mut self, event: &LayoutEvent) {
        match event {
            LayoutEvent::WindowAdded(space, wid) => {
                self.request_refocus_if_hidden(*space, *wid);
            }
            LayoutEvent::WindowsOnScreenUpdated(space, _, windows, _) => {
                let Some(active_workspace) = self.layout_engine.active_workspace(*space) else {
                    return;
                };
                let manager = self.layout_engine.virtual_workspace_manager();
                let hidden_exists = windows.iter().any(|(wid, _, _, _)| {
                    manager
                        .workspace_for_window(*space, *wid)
                        .map_or(false, |workspace_id| workspace_id != active_workspace)
                });
                if hidden_exists {
                    self.pending_refocus_space = Some(*space);
                }
            }
            _ => {}
        }
    }

    #[instrument(skip(self))]
    fn raise_window(&mut self, wid: WindowId, quiet: Quiet, warp: Option<CGPoint>) {
        let mut app_handles = HashMap::default();
        if let Some(app) = self.apps.get(&wid.pid) {
            app_handles.insert(wid.pid, app.handle.clone());
        }
        _ = self.raise_manager_tx.send(raise_manager::Event::RaiseRequest(RaiseRequest {
            raise_windows: vec![vec![wid]],
            focus_window: Some((wid, warp)),
            app_handles,
        }));
    }

    fn set_focus_follows_mouse_enabled(&self, enabled: bool) {
        if let Some(event_tap_tx) = self.event_tap_tx.as_ref() {
            event_tap_tx.send(event_tap::Request::SetFocusFollowsMouseEnabled(enabled));
        }
    }

    fn update_focus_follows_mouse_state(&self) {
        let should_enable = self.menu_open_depth == 0 && !self.mission_control_active;
        self.set_focus_follows_mouse_enabled(should_enable);
    }

    fn set_mission_control_active(&mut self, active: bool) {
        if self.mission_control_active == active {
            return;
        }
        self.mission_control_active = active;
        self.update_focus_follows_mouse_state();
    }

    fn refresh_windows_after_mission_control(&mut self) {
        debug!("Refreshing window state after Mission Control");
        let ws_info = window_server::get_visible_windows_with_layer(None);
        self.update_partial_window_server_info(ws_info);
        self.pending_mission_control_refresh.clear();
        self.force_refresh_all_windows();
        self.check_for_new_windows();
        let _ = self.update_layout(false, false);
        self.maybe_send_menu_update();
    }

    fn force_refresh_all_windows(&mut self) {
        for (&pid, app) in &self.apps {
            if app.handle.send(Request::GetVisibleWindows { force_refresh: true }).is_ok() {
                self.pending_mission_control_refresh.insert(pid);
            }
        }
    }

    fn main_window(&self) -> Option<WindowId> { self.main_window_tracker.main_window() }

    fn main_window_space(&self) -> Option<SpaceId> {
        // TODO: Optimize this with a cache or something.
        self.best_space_for_window(&self.windows.get(&self.main_window()?)?.frame_monotonic)
    }

    fn workspace_command_space(&self) -> Option<SpaceId> {
        self.main_window_space()
            .or_else(|| get_active_space_number())
            .or_else(|| self.screens.iter().find_map(|screen| screen.space))
    }

    fn store_current_floating_positions(&mut self, space: SpaceId) {
        let floating_windows_in_workspace = self
            .layout_engine
            .windows_in_active_workspace(space)
            .into_iter()
            .filter(|&wid| self.layout_engine.is_window_floating(wid))
            .filter_map(|wid| {
                self.windows.get(&wid).map(|window_state| (wid, window_state.frame_monotonic))
            })
            .collect::<Vec<_>>();

        if !floating_windows_in_workspace.is_empty() {
            self.layout_engine
                .store_floating_window_positions(space, &floating_windows_in_workspace);
        }
    }

    #[instrument(skip(self), fields())]
    pub fn update_layout(&mut self, is_resize: bool, is_workspace_switch: bool) -> bool {
        let screens = self.screens.clone();
        let main_window = self.main_window();
        trace!(?main_window);
        let skip_wid = self.skip_layout_for_window.take().or(self.drag_manager.dragged());
        let mut any_frame_changed = false;
        for screen in screens {
            let Some(space) = screen.space else { continue };
            trace!(?screen);
            let layout = self.layout_engine.calculate_layout_with_virtual_workspaces(
                space,
                screen.frame.clone(),
                self.config.settings.ui.stack_line.thickness(),
                self.config.settings.ui.stack_line.horiz_placement,
                self.config.settings.ui.stack_line.vert_placement,
                |wid| {
                    self.windows
                        .get(&wid)
                        .map(|w| w.frame_monotonic.size)
                        .unwrap_or_else(|| CGSize::new(500.0, 500.0))
                },
            );
            trace!(?layout, "Layout");

            if self.config.settings.ui.stack_line.enabled {
                if let Some(tx) = &self.stack_line_tx {
                    let group_infos =
                        self.layout_engine.collect_group_containers_in_selection_path(
                            space,
                            screen.frame,
                            self.config.settings.ui.stack_line.thickness(),
                            self.config.settings.ui.stack_line.horiz_placement,
                            self.config.settings.ui.stack_line.vert_placement,
                        );

                    let groups: Vec<crate::actor::stack_line::GroupInfo> = group_infos
                        .into_iter()
                        .map(|g| crate::actor::stack_line::GroupInfo {
                            node_id: g.node_id,
                            space_id: space,
                            container_kind: g.container_kind,
                            frame: g.frame,
                            total_count: g.total_count,
                            selected_index: g.selected_index,
                        })
                        .collect();
                    _ = tx.try_send(crate::actor::stack_line::Event::GroupsUpdated {
                        space_id: space,
                        groups,
                    });
                }
            }

            let suppress_animation = is_workspace_switch || self.active_workspace_switch.is_some();
            if suppress_animation {
                let mut per_app: HashMap<pid_t, Vec<(WindowId, CGRect)>> = HashMap::default();
                for &(wid, target_frame) in &layout {
                    // Skip applying a layout frame for the window currently being dragged.
                    if skip_wid == Some(wid) {
                        trace!(?wid, "Skipping layout update for window currently being dragged");
                        continue;
                    }

                    let Some(window) = self.windows.get_mut(&wid) else {
                        debug!(?wid, "Skipping layout - window no longer exists");
                        continue;
                    };
                    let target_frame = target_frame.round();
                    let current_frame = window.frame_monotonic;
                    if target_frame.same_as(current_frame) {
                        continue;
                    }
                    any_frame_changed = true;
                    trace!(
                        ?wid,
                        ?current_frame,
                        ?target_frame,
                        "Instant workspace positioning"
                    );

                    per_app.entry(wid.pid).or_default().push((wid, target_frame));
                }

                for (pid, frames) in per_app.into_iter() {
                    if frames.is_empty() {
                        continue;
                    }

                    let Some(app_state) = self.apps.get(&pid) else {
                        debug!(?pid, "Skipping layout update for app - app no longer exists");
                        continue;
                    };

                    let handle = app_state.handle.clone();

                    let (first_wid, first_target) = frames[0];
                    let mut txid = TransactionId::default();
                    let mut has_txid = false;
                    let mut txid_entries: Vec<(WindowServerId, TransactionId, CGRect)> = Vec::new();
                    if let Some(window) = self.windows.get_mut(&first_wid) {
                        txid = window.next_txid();
                        has_txid = true;
                        if let Some(wsid) = window.window_server_id {
                            txid_entries.push((wsid, txid, first_target));
                        }
                    }

                    if has_txid {
                        for (wid, frame) in frames.iter().skip(1) {
                            if let Some(w) = self.windows.get_mut(wid) {
                                w.last_sent_txid = txid;
                                if let Some(wsid) = w.window_server_id {
                                    txid_entries.push((wsid, txid, *frame));
                                }
                            }
                        }
                        self.update_txid_entries(txid_entries);
                    }

                    let frames_to_send = frames.clone();
                    if let Err(e) = handle.send(Request::SetBatchWindowFrame(frames_to_send, txid))
                    {
                        debug!(
                            ?pid,
                            ?e,
                            "Failed to send batch frame request - app may have quit"
                        );
                        continue;
                    }

                    for (wid, target_frame) in &frames {
                        if let Some(window) = self.windows.get_mut(wid) {
                            window.frame_monotonic = *target_frame;
                        }
                    }
                }
            } else {
                if let Some(active_ws) = self.layout_engine.active_workspace(space) {
                    let mut anim = Animation::new(
                        self.config.settings.animation_fps,
                        self.config.settings.animation_duration,
                        self.config.settings.animation_easing.clone(),
                    );
                    let mut animated_count = 0;

                    let mut animated_wids_wsids: Vec<u32> = Vec::new();
                    for &(wid, target_frame) in &layout {
                        // Skip applying layout frames and animations for the window currently being dragged.
                        if skip_wid == Some(wid) {
                            trace!(
                                ?wid,
                                "Skipping animated layout update for window currently being dragged"
                            );
                            continue;
                        }

                        let target_frame = target_frame.round();
                        let (current_frame, window_server_id, txid) =
                            match self.windows.get_mut(&wid) {
                                Some(window) => {
                                    let current_frame = window.frame_monotonic;
                                    if target_frame.same_as(current_frame) {
                                        continue;
                                    }
                                    let txid = window.next_txid();
                                    let wsid = window.window_server_id;
                                    (current_frame, wsid, txid)
                                }
                                None => {
                                    debug!(?wid, "Skipping - window no longer exists");
                                    continue;
                                }
                            };

                        let Some(app_state) = &self.apps.get(&wid.pid) else {
                            debug!(?wid, "Skipping for window - app no longer exists");
                            continue;
                        };

                        let is_active = self
                            .layout_engine
                            .virtual_workspace_manager()
                            .workspace_for_window(space, wid)
                            .map_or(false, |ws| ws == active_ws);

                        if is_active {
                            trace!(?wid, ?current_frame, ?target_frame, "Animating visible window");
                            animated_wids_wsids.push(wid.idx.into());
                            anim.add_window(
                                &app_state.handle,
                                wid,
                                current_frame,
                                target_frame,
                                false,
                                txid,
                            );
                            animated_count += 1;
                            if let Some(wsid) = window_server_id {
                                self.update_txid_entries([(wsid, txid, target_frame)]);
                            }
                        } else {
                            trace!(
                                ?wid,
                                ?current_frame,
                                ?target_frame,
                                "Direct positioning hidden window"
                            );
                            if let Some(wsid) = window_server_id {
                                self.update_txid_entries([(wsid, txid, target_frame)]);
                            }
                            if let Err(e) = app_state.handle.send(Request::SetWindowFrame(
                                wid,
                                target_frame,
                                txid,
                                true,
                            )) {
                                debug!(?wid, ?e, "Failed to send frame request for hidden window");
                                continue;
                            }
                        }

                        if let Some(window) = self.windows.get_mut(&wid) {
                            window.frame_monotonic = target_frame;
                        }
                    }

                    if animated_count > 0 {
                        if let Some(tx) = &self.window_notify_tx {
                            for wsid in &animated_wids_wsids {
                                let _ = tx.send(
                                    crate::actor::window_notify::Request::IgnoreWindowEvent(
                                        crate::sys::skylight::CGSEventType::Known(
                                            crate::sys::skylight::KnownCGSEvent::WindowMoved,
                                        ),
                                        *wsid,
                                    ),
                                );
                                let _ = tx.send(
                                    crate::actor::window_notify::Request::IgnoreWindowEvent(
                                        crate::sys::skylight::CGSEventType::Known(
                                            crate::sys::skylight::KnownCGSEvent::WindowResized,
                                        ),
                                        *wsid,
                                    ),
                                );
                            }
                        }

                        let low_power = power::is_low_power_mode_enabled();
                        if is_resize || !self.config.settings.animate || low_power {
                            anim.skip_to_end();
                        } else {
                            anim.run();
                        }
                        if let Some(tx) = &self.window_notify_tx {
                            for wsid in &animated_wids_wsids {
                                let _ = tx.send(
                                    crate::actor::window_notify::Request::UnignoreWindowEvent(
                                        crate::sys::skylight::CGSEventType::Known(
                                            crate::sys::skylight::KnownCGSEvent::WindowMoved,
                                        ),
                                        *wsid,
                                    ),
                                );
                                let _ = tx.send(
                                    crate::actor::window_notify::Request::UnignoreWindowEvent(
                                        crate::sys::skylight::CGSEventType::Known(
                                            crate::sys::skylight::KnownCGSEvent::WindowResized,
                                        ),
                                        *wsid,
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
        self.maybe_send_menu_update();
        any_frame_changed
    }
}
