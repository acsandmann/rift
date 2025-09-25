//! The Reactor's job is to maintain coherence between the system and model state.
//!
//! It takes events from the rest of the system and builds a coherent picture of
//! what is going on. It shares this with the layout actor, and reacts to layout
//! changes by sending requests out to the other actors in the system.

mod animation;
mod main_window;
mod replay;

#[cfg(test)]
mod testing;

use std::{mem, thread};

use animation::Animation;
use main_window::MainWindowTracker;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
pub use replay::{Record, replay};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use tracing::{debug, error, info, instrument, trace, warn};

use super::mouse;
use crate::actor::app::{AppInfo, AppThreadHandle, Quiet, Request, WindowId, WindowInfo, pid_t};
use crate::actor::broadcast::{BroadcastEvent, BroadcastSender};
use crate::actor::raise_manager::{self, RaiseRequest};
use crate::actor::{self, menu_bar, stack_line};
use crate::common::collections::{BTreeMap, HashMap, HashSet};
use crate::common::config::Config;
use crate::common::log::{self, MetricsCommand};
use crate::layout_engine::{self as layout, Direction, LayoutCommand, LayoutEngine, LayoutEvent};
use crate::sys::event::MouseState;
use crate::sys::executor::Executor;
use crate::sys::geometry::{CGRectDef, CGRectExt, Round, SameAs};
use crate::sys::power;
use crate::sys::screen::{SpaceId, get_active_space_number};
use crate::sys::window_server::{self, WindowServerId, WindowServerInfo};

pub type Sender = actor::Sender<Event>;
type Receiver = actor::Receiver<Event>;

use std::path::PathBuf;

use crate::model::server::{
    ApplicationData, LayoutStateData, WindowData, WorkspaceData, WorkspaceQueryResponse,
};
// use crate::model::tree::NodeId; // no longer needed here

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
    WindowServerDestroyed(crate::sys::window_server::WindowServerId),
    #[serde(skip)]
    WindowServerAppeared(crate::sys::window_server::WindowServerId),
    WindowFrameChanged(
        WindowId,
        #[serde(with = "CGRectDef")] CGRect,
        TransactionId,
        Requested,
        Option<MouseState>,
    ),
    MenuOpened,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Command {
    Layout(LayoutCommand),
    Metrics(MetricsCommand),
    Reactor(ReactorCommand),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    mouse_tx: Option<mouse::Sender>,
    menu_tx: Option<menu_bar::Sender>,
    stack_line_tx: Option<stack_line::Sender>,
    raise_manager_tx: raise_manager::Sender,
    event_broadcaster: BroadcastSender,
    app_rules_recently_applied: std::time::Instant,
    last_auto_workspace_switch: Option<std::time::Instant>,
    last_sls_notification_ids: Vec<u32>,
    menu_open_depth: usize,
    mission_control_active: bool,
}

#[derive(Debug)]
struct AppState {
    #[allow(unused)]
    pub info: AppInfo,
    pub handle: AppThreadHandle,
}

#[derive(Copy, Clone, Debug)]
struct Screen {
    frame: CGRect,
    space: Option<SpaceId>,
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
        mouse_tx: mouse::Sender,
        broadcast_tx: BroadcastSender,
        menu_tx: menu_bar::Sender,
        stack_line_tx: stack_line::Sender,
    ) -> Sender {
        let (events_tx, events) = actor::channel();
        let events_tx_clone = events_tx.clone();
        thread::Builder::new()
            .name("reactor".to_string())
            .spawn(move || {
                let mut reactor = Reactor::new(config, layout_engine, record, broadcast_tx);
                reactor.mouse_tx.replace(mouse_tx);
                reactor.menu_tx.replace(menu_tx);
                reactor.stack_line_tx.replace(stack_line_tx);
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
    ) -> Reactor {
        // FIXME: Remove apps that are no longer running from restored state.
        record.start(&config, &layout_engine);
        let (raise_manager_tx, _rx) = actor::channel();
        Reactor {
            config,
            apps: HashMap::default(),
            layout_engine,
            windows: HashMap::default(),
            window_ids: HashMap::default(),
            window_server_info: HashMap::default(),
            visible_windows: HashSet::default(),
            screens: vec![],
            main_window_tracker: MainWindowTracker::default(),
            in_drag: false,
            is_workspace_switch: false,
            workspace_switch_generation: 0,
            active_workspace_switch: None,
            record,
            mouse_tx: None,
            menu_tx: None,
            stack_line_tx: None,
            raise_manager_tx,
            event_broadcaster: broadcast_tx,
            app_rules_recently_applied: std::time::Instant::now(),
            last_auto_workspace_switch: None,
            last_sls_notification_ids: Vec::new(),
            observed_window_server_ids: HashSet::default(),
            menu_open_depth: 0,
            mission_control_active: false,
        }
    }

    pub async fn run(mut self, events: Receiver, events_tx: Sender) {
        let (raise_manager_tx, raise_manager_rx) = actor::channel();
        self.raise_manager_tx = raise_manager_tx.clone();

        let mouse_tx = self.mouse_tx.clone();
        let reactor_task = self.run_reactor_loop(events);
        let raise_manager_task = RaiseManager::run(raise_manager_rx, events_tx, mouse_tx);

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
                self.apps.remove(&pid);
            }
            Event::ApplicationActivated(..)
            | Event::ApplicationDeactivated(..)
            | Event::ApplicationGloballyDeactivated(..)
            | Event::ApplicationMainWindowChanged(..) => {}
            Event::ApplicationGloballyActivated(pid) => {
                self.handle_app_activation_workspace_switch(pid);
            }
            Event::WindowsDiscovered { pid, new, known_visible } => {
                self.on_windows_discovered_with_app_info(pid, new, known_visible, None);
            }
            Event::WindowCreated(wid, window, ws_info, mouse_state) => {
                // TODO: It's possible for a window to be on multiple spaces
                // or move spaces. (Add a test)
                // FIXME: We assume all windows are on the main screen.
                if let Some(wsid) = window.sys_id {
                    self.window_ids.insert(wsid, wid);
                    self.observed_window_server_ids.remove(&wsid);
                }
                let frame = window.frame;
                self.windows.insert(wid, window.into());

                if let Some(info) = ws_info {
                    self.observed_window_server_ids.remove(&info.id);
                    self.window_server_info.insert(info.id, info);
                }

                if let Some(space) = self.best_space_for_window(&frame) {
                    if self.window_is_standard(wid) {
                        self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
                    }
                }
                if mouse_state == MouseState::Down {
                    self.in_drag = true;
                }
            }
            Event::WindowDestroyed(wid) => {
                let window_server_id = self.windows.get(&wid).and_then(|w| w.window_server_id);
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
            }
            Event::WindowServerDestroyed(wsid) => {
                if let Some(wid) = self.window_ids.get(&wsid).copied() {
                    self.handle_event(Event::WindowDestroyed(wid));
                } else {
                    warn!(
                        ?wsid,
                        "Received WindowServerDestroyed for unknown window - ignoring"
                    );
                }
                return;
            }
            Event::WindowServerAppeared(wsid) => {
                if self.window_server_info.contains_key(&wsid)
                    || self.observed_window_server_ids.contains(&wsid)
                {
                    warn!(
                        ?wsid,
                        "Received WindowServerAppeared for known window - ignoring"
                    );
                    return;
                }
                self.observed_window_server_ids.insert(wsid);
            }
            Event::WindowFrameChanged(wid, new_frame, last_seen, requested, mouse_state) => {
                if let Some(window) = self.windows.get_mut(&wid) {
                    if last_seen != window.last_sent_txid {
                        // Ignore events that happened before the last time we
                        // changed the size or position of this window. Otherwise
                        // we would update the layout model incorrectly.
                        debug!(?last_seen, ?window.last_sent_txid, "Ignoring resize");
                        return;
                    }
                    if requested.0 {
                        // TODO: If the size is different from requested, applying a
                        // correction to the model can result in weird feedback
                        // loops, so we ignore these for now.
                        return;
                    }
                    let old_frame = mem::replace(&mut window.frame_monotonic, new_frame);
                    if old_frame == new_frame {
                        return;
                    }
                    let screens = self
                        .screens
                        .iter()
                        .flat_map(|screen| Some((screen.space?, screen.frame)))
                        .collect::<Vec<_>>();

                    if old_frame.size != new_frame.size {
                        self.send_layout_event(LayoutEvent::WindowResized {
                            wid,
                            old_frame,
                            new_frame,
                            screens,
                        });
                        is_resize = true;
                    } else if mouse_state == Some(MouseState::Down) {
                        self.in_drag = true;
                    }
                }
            }
            Event::ScreenParametersChanged(frames, spaces, ws_info) => {
                info!("screen parameters changed");
                self.screens = frames
                    .into_iter()
                    .zip(spaces)
                    .map(|(frame, space)| Screen { frame, space })
                    .collect();
                self.expose_all_spaces();
                self.update_complete_window_server_info(ws_info);
                // FIXME: Update visible windows if space changed
            }
            Event::SpaceChanged(spaces, ws_info) => {
                if spaces.len() != self.screens.len() {
                    warn!(
                        "Ignoring space change event: we have {} spaces, but {} screens",
                        spaces.len(),
                        self.screens.len()
                    );
                    return;
                }
                info!("space changed");
                for (space, screen) in spaces.iter().zip(&mut self.screens) {
                    screen.space = *space;
                }
                self.expose_all_spaces();
                if let Some(main_window) = self.main_window() {
                    let spaces = self.visible_spaces();
                    self.send_layout_event(LayoutEvent::WindowFocused(spaces, main_window));
                }
                self.update_complete_window_server_info(ws_info);
                self.check_for_new_windows();

                if let Some(space) = spaces.first().and_then(|s| *s) {
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
            Event::MouseUp => {
                self.in_drag = false;
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
                        self.main_window_space(),
                        &visible_spaces,
                        cmd,
                    ),
                };

                self.is_workspace_switch = is_workspace_switch;
                self.handle_layout_response(response);
            }
            Event::Command(Command::Metrics(cmd)) => log::handle_command(cmd),
            Event::ConfigUpdated(new_cfg) => {
                self.config = new_cfg;
                let _ = self.update_layout(false, true);
            }
            Event::Command(Command::Reactor(ReactorCommand::Debug)) => {
                for screen in &self.screens {
                    if let Some(space) = screen.space {
                        self.layout_engine.debug_tree_desc(space, "", true);
                    }
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::Serialize)) => {
                let layout_engine_ron = self.layout_engine.serialize_to_string();
                let vwm = self.layout_engine.virtual_workspace_manager_mut();

                let stats = vwm.get_stats();
                let mut workspace_window_counts = serde_json::Map::new();
                for (ws_id, count) in &stats.workspace_window_counts {
                    workspace_window_counts
                        .insert(format!("{:?}", ws_id), serde_json::json!(*count));
                }

                let mut spaces_intermediate: Vec<(
                    u64,
                    Vec<(
                        crate::model::VirtualWorkspaceId,
                        String,
                        bool,
                        Vec<crate::actor::app::WindowId>,
                        Option<crate::actor::app::WindowId>,
                        Vec<(crate::actor::app::WindowId, objc2_core_foundation::CGRect)>,
                    )>,
                )> = Vec::new();

                for screen in &self.screens {
                    if let Some(space) = screen.space {
                        let workspaces = vwm.list_workspaces(space);
                        let active_ws = vwm.active_workspace(space);

                        let mut ws_entries = Vec::new();
                        for (workspace_id, workspace_name) in workspaces {
                            let window_ids: Vec<crate::actor::app::WindowId> =
                                if let Some(ws) = vwm.workspace_info(space, workspace_id) {
                                    ws.windows().collect()
                                } else {
                                    Vec::new()
                                };

                            let last_focused = vwm.last_focused_window(space, workspace_id);

                            let floating_positions =
                                vwm.get_workspace_floating_positions(space, workspace_id);

                            ws_entries.push((
                                workspace_id,
                                workspace_name,
                                active_ws == Some(workspace_id),
                                window_ids,
                                last_focused,
                                floating_positions,
                            ));
                        }

                        spaces_intermediate.push((space.get(), ws_entries));
                    }
                }

                let mut mapping_intermediate: Vec<(
                    u64,
                    crate::actor::app::WindowId,
                    crate::model::VirtualWorkspaceId,
                )> = Vec::new();
                for ((space, window_id), workspace_id) in &vwm.window_to_workspace {
                    mapping_intermediate.push((space.get(), *window_id, *workspace_id));
                }

                let _ = vwm;

                let mut included_windows: HashSet<crate::actor::app::WindowId> = HashSet::default();

                let mut spaces_json = Vec::new();
                for (space_num, ws_entries) in spaces_intermediate {
                    let mut ws_json = Vec::new();
                    for (
                        workspace_id,
                        workspace_name,
                        is_active,
                        window_ids,
                        last_focused,
                        floating_positions,
                    ) in ws_entries
                    {
                        let mut windows_json = Vec::new();
                        for wid in window_ids {
                            if let Some(window_data) = self.create_window_data(wid) {
                                let v = serde_json::to_value(&window_data).unwrap_or_else(
                                    |_| serde_json::json!({ "id": wid.to_debug_string() }),
                                );
                                windows_json.push(v);
                            } else {
                                windows_json
                                    .push(serde_json::json!({ "id": wid.to_debug_string() }));
                            }

                            let _ = included_windows.insert(wid);
                        }

                        let last_focused_json = last_focused.map(|w| w.to_debug_string());

                        let floating_json: Vec<serde_json::Value> = floating_positions
                            .into_iter()
                            .map(|(wid, rect)| {
                                serde_json::json!({
                                    "window": wid.to_debug_string(),
                                    "rect": {
                                        "x": rect.origin.x,
                                        "y": rect.origin.y,
                                        "w": rect.size.width,
                                        "h": rect.size.height
                                    }
                                })
                            })
                            .collect();

                        let id_str = workspace_id.to_string();
                        let digits: String =
                            id_str.chars().filter(|c| c.is_ascii_digit()).collect();
                        let id_num = digits.parse::<u64>().unwrap_or(0);

                        ws_json.push(serde_json::json!({
                            "id": id_str,
                            "id_num": id_num,
                            "name": workspace_name,
                            "is_active": is_active,
                            "windows": windows_json,
                            "last_focused": last_focused_json,
                            "floating_positions": floating_json,
                        }));
                    }

                    spaces_json.push(serde_json::json!({
                        "space": space_num,
                        "workspaces": ws_json,
                    }));
                }

                let mut mapping = Vec::new();
                for (space_num, window_id, workspace_id) in mapping_intermediate {
                    let window_json = if let Some(window_data) = self.create_window_data(window_id)
                    {
                        serde_json::to_value(&window_data).unwrap_or_else(
                            |_| serde_json::json!({ "id": window_id.to_debug_string() }),
                        )
                    } else {
                        serde_json::json!({ "id": window_id.to_debug_string() })
                    };

                    let _ = included_windows.insert(window_id);

                    mapping.push(serde_json::json!({
                        "space": space_num,
                        "window": window_json,
                        "workspace": workspace_id.to_string()
                    }));
                }

                let known_managed_windows: Vec<serde_json::Value> = self
                    .windows
                    .keys()
                    .filter(|w| !included_windows.contains(*w))
                    .map(|w| {
                        if let Some(window_data) = self.create_window_data(*w) {
                            serde_json::to_value(&window_data).unwrap_or_else(
                                |_| serde_json::json!({ "id": w.to_debug_string() }),
                            )
                        } else {
                            serde_json::json!({ "id": w.to_debug_string() })
                        }
                    })
                    .collect();

                let reactor_summary = serde_json::json!({
                    "apps": self.apps.len(),
                    "managed_windows": self.windows.len(),
                    "window_server_info": self.window_server_info.len(),
                    "visible_window_server_ids": self.visible_windows.len(),
                    "screens": self.screens.len(),
                    "known_managed_windows": known_managed_windows,
                });

                let out = serde_json::json!({
                    "layout_engine_ron": layout_engine_ron,
                    "virtual_workspace_manager": {
                        "total_workspaces": stats.total_workspaces,
                        "total_windows": stats.total_windows,
                        "active_spaces": stats.active_spaces,
                        "workspace_window_counts": workspace_window_counts,
                    },
                    "spaces": spaces_json,
                    "window_to_workspace": mapping,
                    "reactor": reactor_summary,
                });

                println!("{}", serde_json::to_string_pretty(&out).unwrap());
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
                    let spaces = self.visible_spaces();
                    self.send_layout_event(LayoutEvent::WindowFocused(spaces, wid));

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

            Event::QueryWorkspaces(response_tx) => {
                let response = self.handle_workspace_query();
                let _ = response_tx.send(response);
            }
            Event::QueryWindows { space_id, response } => {
                let windows = self.handle_windows_query(space_id);
                let _ = response.send(windows);
            }
            Event::QueryWindowInfo { window_id, response } => {
                let window_info = self.handle_window_info_query(window_id);
                let _ = response.send(window_info);
            }
            Event::QueryApplications(response) => {
                let apps = self.handle_applications_query();
                let _ = response.send(apps);
            }
            Event::QueryLayoutState { space_id, response } => {
                let layout_state = self.handle_layout_state_query(space_id);
                let _ = response.send(layout_state);
            }
            Event::QueryMetrics(response) => {
                let metrics = self.handle_metrics_query();
                let _ = response.send(metrics);
            }
        }
        if let Some(raised_window) = raised_window {
            let spaces = self.visible_spaces();
            self.send_layout_event(LayoutEvent::WindowFocused(spaces, raised_window));
        }

        let mut layout_changed = false;
        if !self.in_drag || window_was_destroyed {
            layout_changed = self.update_layout(is_resize, self.is_workspace_switch);
        }

        if !self.in_drag || window_was_destroyed {
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

    fn maybe_send_menu_update(&mut self) {
        let menu_tx = match self.menu_tx.as_ref() {
            Some(tx) => tx.clone(),
            None => return,
        };

        let active_space =
            match self.main_window_space().or_else(|| self.screens.first().and_then(|s| s.space)) {
                Some(space) => space,
                None => return,
            };

        let workspaces = self.handle_workspace_query().workspaces;
        let active_workspace = self.layout_engine.active_workspace(active_space);
        let windows = self.handle_windows_query(Some(active_space));

        let _ = menu_tx.send(menu_bar::Event::Update {
            active_space,
            workspaces,
            active_workspace,
            windows,
        });
    }

    fn handle_workspace_query(&mut self) -> WorkspaceQueryResponse {
        let mut workspaces = Vec::new();

        let space_id =
            get_active_space_number().or_else(|| self.screens.first().and_then(|s| s.space));
        let workspace_list: Vec<(crate::model::VirtualWorkspaceId, String)> =
            if let Some(space) = space_id {
                self.layout_engine.virtual_workspace_manager_mut().list_workspaces(space)
            } else {
                Vec::new()
            };

        for (index, (workspace_id, workspace_name)) in workspace_list.iter().enumerate() {
            let is_active = if let Some(space) = space_id {
                self.layout_engine.active_workspace(space) == Some(*workspace_id)
            } else {
                false
            };

            let workspace_windows_ids: Vec<crate::actor::app::WindowId> =
                if let Some(space) = space_id {
                    if is_active {
                        self.layout_engine.windows_in_active_workspace(space)
                    } else {
                        self.layout_engine
                            .virtual_workspace_manager()
                            .workspace_info(space, *workspace_id)
                            .map(|ws| ws.windows().collect())
                            .unwrap_or_default()
                    }
                } else {
                    Vec::new()
                };

            let predicted_positions = if !is_active {
                if let Some(space) = space_id {
                    let screen_frame = self
                        .screens
                        .iter()
                        .find(|s| s.space == Some(space))
                        .map(|s| s.frame)
                        .or_else(|| self.screens.first().map(|s| s.frame));

                    if let Some(frame) = screen_frame {
                        self.layout_engine.calculate_layout_for_workspace(
                            space,
                            *workspace_id,
                            frame,
                            self.config.settings.ui.stack_line.thickness(),
                            self.config.settings.ui.stack_line.horiz_placement,
                            self.config.settings.ui.stack_line.vert_placement,
                        )
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            let predicted_map: std::collections::HashMap<WindowId, CGRect> =
                predicted_positions.into_iter().collect();

            let mut windows: Vec<WindowData> = Vec::new();
            for wid in workspace_windows_ids.into_iter() {
                if let Some(mut wd) = self.create_window_data(wid) {
                    if !is_active {
                        if let Some(pred) = predicted_map.get(&wid).copied() {
                            wd.frame = pred;
                        }
                    }
                    windows.push(wd);
                }
            }

            workspaces.push(WorkspaceData {
                id: format!("{:?}", workspace_id),
                name: workspace_name.to_string(),
                is_active,
                window_count: windows.len(),
                windows,
                index,
            });
        }

        WorkspaceQueryResponse { workspaces }
    }

    fn handle_windows_query(&self, space_id: Option<SpaceId>) -> Vec<WindowData> {
        let target_space = space_id.or_else(|| self.screens.first().and_then(|s| s.space));

        if let Some(space) = target_space {
            let active_windows = self.layout_engine.windows_in_active_workspace(space);

            active_windows
                .into_iter()
                .filter_map(|wid| self.create_window_data(wid))
                .collect()
        } else {
            self.windows.keys().filter_map(|&wid| self.create_window_data(wid)).collect()
        }
    }

    fn handle_window_info_query(&self, window_id: WindowId) -> Option<WindowData> {
        self.create_window_data(window_id)
    }

    fn handle_applications_query(&self) -> Vec<ApplicationData> {
        self.apps
            .iter()
            .map(|(&pid, app)| {
                let window_count = self.windows.keys().filter(|wid| wid.pid == pid).count();

                let is_frontmost = self
                    .main_window_tracker
                    .main_window()
                    .map(|wid| wid.pid == pid)
                    .unwrap_or(false);

                ApplicationData {
                    pid,
                    bundle_id: app.info.bundle_id.clone(),
                    name: app.info.localized_name.clone().unwrap_or_else(|| "Unknown".to_string()),
                    is_frontmost,
                    window_count,
                }
            })
            .collect()
    }

    fn handle_layout_state_query(&self, space_id_u64: u64) -> Option<LayoutStateData> {
        let space_id = self
            .screens
            .iter()
            .find_map(|screen| screen.space.filter(|s| s.get() == space_id_u64))
            .filter(|_space| space_id_u64 > 0)?;

        let _active_workspace = self.layout_engine.active_workspace(space_id)?;

        let active_windows = self.layout_engine.windows_in_active_workspace(space_id);
        let floating_windows: Vec<WindowId> = active_windows
            .iter()
            .filter(|&&wid| self.layout_engine.is_window_floating(wid))
            .copied()
            .collect();

        let tiled_windows: Vec<WindowId> = active_windows
            .iter()
            .filter(|&&wid| !self.layout_engine.is_window_floating(wid))
            .copied()
            .collect();

        let focused_window = self.main_window();

        Some(LayoutStateData {
            space_id: space_id_u64,
            mode: "tiling".to_string(), // TODO: Determine actual mode
            floating_windows,
            tiled_windows,
            focused_window,
        })
    }

    fn handle_metrics_query(&self) -> serde_json::Value {
        let stats = self.layout_engine.virtual_workspace_manager().get_stats();

        let workspace_stats: crate::common::collections::HashMap<String, usize> = stats
            .workspace_window_counts
            .iter()
            .map(|(id, count)| (format!("{:?}", id), *count))
            .collect();

        serde_json::json!({
               "windows_managed": self.windows.len(),
            "workspaces": stats.total_workspaces,
            "applications": self.apps.len(),
            "screens": self.screens.len(),
            "workspace_stats": workspace_stats,
        })
    }

    fn create_window_data(&self, window_id: WindowId) -> Option<WindowData> {
        let window_state = self.windows.get(&window_id)?;
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
        }

        for info in ws_info.iter().filter(|i| i.layer == 0) {
            let Some(wid) = self.window_ids.get(&info.id) else {
                continue;
            };
            let Some(window) = self.windows.get_mut(wid) else {
                continue;
            };

            window.frame_monotonic = info.frame;
        }
        self.window_server_info.extend(ws_info.into_iter().map(|info| (info.id, info)));
    }

    fn check_for_new_windows(&mut self) {
        // TODO: Do this correctly/more optimally using CGWindowListCopyWindowInfo
        // (see notes for on_windows_discovered below).
        for app in self.apps.values_mut() {
            // Errors mean the app terminated (and a termination event
            // is coming); ignore.
            _ = app.handle.send(Request::GetVisibleWindows);
        }
    }

    fn on_windows_discovered_with_app_info(
        &mut self,
        pid: pid_t,
        new: Vec<(WindowId, WindowInfo)>,
        _known_visible: Vec<WindowId>,
        app_info: Option<AppInfo>,
    ) {
        // If app_info wasn't provided, try to look it up from our running app state so
        // we can apply workspace rules immediately on first discovery.
        let app_info = app_info.or_else(|| self.apps.get(&pid).map(|app| app.info.clone()));

        let time_since_app_rules = self.app_rules_recently_applied.elapsed();
        let app_rules_recently_applied = time_since_app_rules.as_secs() < 1;

        if app_rules_recently_applied && app_info.is_none() {
            self.window_ids
                .extend(new.iter().flat_map(|(wid, info)| info.sys_id.map(|wsid| (wsid, *wid))));
            self.windows.extend(new.into_iter().map(|(wid, info)| (wid, info.into())));
            return;
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
        self.window_ids
            .extend(new.iter().flat_map(|(wid, info)| info.sys_id.map(|wsid| (wsid, *wid))));
        self.windows.extend(new.into_iter().map(|(wid, info)| (wid, info.into())));
        if !self.windows.iter().any(|(wid, _)| wid.pid == pid) {
            return;
        }
        let mut app_windows: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
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
                windows_with_titles,
                app_info.clone(),
            ));
        }

        if let Some(main_window) = self.main_window() {
            if main_window.pid == pid {
                let spaces = self.screens.iter().flat_map(|screen| screen.space).collect();
                self.send_layout_event(LayoutEvent::WindowFocused(spaces, main_window));
            }
        }
    }

    fn best_space_for_window(&self, frame: &CGRect) -> Option<SpaceId> {
        self.screens
            .iter()
            .max_by_key(|s| s.frame.intersection(frame).area() as i64)?
            .space
    }

    fn visible_spaces(&self) -> Vec<SpaceId> {
        self.screens.iter().flat_map(|screen| screen.space).collect()
    }

    fn visible_space_ids_u64(&self) -> Vec<u64> {
        self.visible_spaces().into_iter().map(|sid| sid.get()).collect()
    }

    fn expose_all_spaces(&mut self) {
        let screens = self.screens.clone();
        for screen in screens {
            let Some(space) = screen.space else { continue };
            let _ = self.layout_engine.virtual_workspace_manager_mut().list_workspaces(space);
            self.send_layout_event(LayoutEvent::SpaceExposed(space, screen.frame.size));
        }
    }

    fn window_is_standard(&self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        if let Some(id) = window.window_server_id {
            if let Some(info) = self.window_server_info.get(&id) {
                if info.layer != 0 {
                    return false;
                }
            }
            if window_server::window_is_sticky(id) {
                return false;
            }
        }
        window.is_ax_standard && window.is_ax_root
    }

    fn send_layout_event(&mut self, event: LayoutEvent) {
        let response = self.layout_engine.handle_event(event);
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
        let space_ids: Vec<u64> = self.visible_space_ids_u64();
        if space_ids.is_empty() {
            return true;
        }

        let order =
            crate::sys::window_server::space_window_list_for_connection(&space_ids, 0, false);
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
        let Some(primary_space) = self.screens.iter().find_map(|screen| screen.space) else {
            return;
        };

        if !window_ids.is_empty() {
            for wid in &window_ids {
                let title_opt = self.windows.get(wid).map(|w| w.title.clone());
                let _ =
                    self.layout_engine.virtual_workspace_manager_mut().assign_window_with_app_info(
                        *wid,
                        primary_space,
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
            )> = window_ids
                .iter()
                .map(|&wid| {
                    let title_opt = self.windows.get(&wid).map(|w| w.title.clone());
                    let ax_role = self.windows.get(&wid).and_then(|w| w.ax_role.clone());
                    let ax_subrole = self.windows.get(&wid).and_then(|w| w.ax_subrole.clone());
                    (wid, title_opt, ax_role, ax_subrole)
                })
                .collect();

            self.send_layout_event(LayoutEvent::WindowsOnScreenUpdated(
                primary_space,
                pid,
                windows_with_titles,
                Some(app_info),
            ));
        }
    }

    fn handle_app_activation_workspace_switch(&mut self, pid: pid_t) {
        use objc2_app_kit::NSRunningApplication;

        use crate::sys::app::NSRunningApplicationExt;

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
            self.last_auto_workspace_switch = Some(std::time::Instant::now());

            let workspaces =
                self.layout_engine.virtual_workspace_manager_mut().list_workspaces(window_space);
            if let Some((workspace_index, _)) =
                workspaces.iter().enumerate().find(|(_, (ws_id, _))| *ws_id == window_workspace)
            {
                debug!(
                    "Auto-switching to workspace {} for activated app (pid: {})",
                    workspace_index, pid
                );

                let response = self.layout_engine.handle_virtual_workspace_command(
                    window_space,
                    &layout::LayoutCommand::SwitchToWorkspace(workspace_index),
                );
                self.handle_layout_response(response);
            }
        }
    }

    fn handle_layout_response(&mut self, response: layout::EventResponse) {
        let layout::EventResponse { raise_windows, focus_window } = response;

        if raise_windows.is_empty() && focus_window.is_none() && !self.is_workspace_switch {
            return;
        }

        let mut app_handles = HashMap::default();
        for &wid in raise_windows.iter().chain(&focus_window) {
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
        if let Some(mouse_tx) = self.mouse_tx.as_ref() {
            mouse_tx.send(mouse::Request::SetFocusFollowsMouseEnabled(enabled));
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

    fn main_window(&self) -> Option<WindowId> { self.main_window_tracker.main_window() }

    fn main_window_space(&self) -> Option<SpaceId> {
        // TODO: Optimize this with a cache or something.
        self.best_space_for_window(&self.windows.get(&self.main_window()?)?.frame_monotonic)
    }

    fn workspace_command_space(&self) -> Option<SpaceId> {
        self.main_window_space()
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

                    let first_wid = frames[0].0;
                    let txid = if let Some(window) = self.windows.get_mut(&first_wid) {
                        let tx = window.next_txid();
                        for (wid, _) in frames.iter().skip(1) {
                            if let Some(w) = self.windows.get_mut(wid) {
                                w.last_sent_txid = tx;
                            }
                        }
                        tx
                    } else {
                        TransactionId::default()
                    };

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

                    for (wid, target_frame) in frames {
                        if let Some(window) = self.windows.get_mut(&wid) {
                            window.frame_monotonic = target_frame;
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

                    for &(wid, target_frame) in &layout {
                        let target_frame = target_frame.round();
                        let Some(window) = self.windows.get_mut(&wid) else {
                            debug!(?wid, "Skipping - window no longer exists");
                            continue;
                        };
                        let current_frame = window.frame_monotonic;
                        if target_frame.same_as(current_frame) {
                            continue;
                        }
                        let Some(app_state) = &self.apps.get(&wid.pid) else {
                            debug!(?wid, "Skipping for window - app no longer exists");
                            continue;
                        };
                        let txid = window.next_txid();

                        let is_active = self
                            .layout_engine
                            .virtual_workspace_manager()
                            .workspace_for_window(space, wid)
                            .map_or(false, |ws| ws == active_ws);

                        if is_active {
                            trace!(?wid, ?current_frame, ?target_frame, "Animating visible window");
                            /*let pid = wid.pid;
                            let heavy = match (&window.bundle_id, &window.bundle_path) {
                                (Some(bundle_id), Some(bundle_path)) => {
                                    is_heavy(pid, bundle_id, bundle_path)
                                }
                                _ => false,
                            };
                            anim.add_window(
                                handle,
                                wid,
                                current_frame,
                                target_frame,
                                screen.frame,
                                txid,
                                heavy,
                            );*/
                            anim.add_window(
                                &app_state.handle,
                                wid,
                                current_frame,
                                target_frame,
                                false,
                                txid,
                            );
                            animated_count += 1;
                        } else {
                            trace!(
                                ?wid,
                                ?current_frame,
                                ?target_frame,
                                "Direct positioning hidden window"
                            );
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
                        window.frame_monotonic = target_frame;
                    }

                    if animated_count > 0 {
                        let low_power = power::is_low_power_mode_enabled();
                        if is_resize || !self.config.settings.animate || low_power {
                            anim.skip_to_end();
                        } else {
                            anim.run();
                        }
                    }
                }
            }
        }
        self.maybe_send_menu_update();
        any_frame_changed
    }
}

#[cfg(test)]
pub mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};
    use test_log::test;

    use super::testing::*;
    use super::*;
    use crate::actor::app::Request;
    use crate::layout_engine::{Direction, LayoutEngine};
    use crate::sys::window_server::WindowServerId;

    #[test]
    fn it_ignores_stale_resize_events() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.))],
            vec![Some(SpaceId::new(1))],
            vec![],
        ));

        reactor.handle_events(apps.make_app(1, make_windows(2)));
        let requests = apps.requests();
        assert!(!requests.is_empty());
        let events_1 = apps.simulate_events_for_requests(requests);

        reactor.handle_events(apps.make_app(2, make_windows(2)));
        assert!(!apps.requests().is_empty());

        for event in dbg!(events_1) {
            reactor.handle_event(event);
        }
        let requests = apps.requests();
        assert!(
            requests.is_empty(),
            "got requests when there should have been none: {requests:?}"
        );
    }

    #[test]
    fn it_sends_writes_when_stale_read_state_looks_same_as_written_state() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.))],
            vec![Some(SpaceId::new(1))],
            vec![],
        ));

        reactor.handle_events(apps.make_app(1, make_windows(2)));
        let events_1 = apps.simulate_events();
        let state_1 = apps.windows.clone();
        assert!(!state_1.is_empty());

        for event in events_1 {
            reactor.handle_event(event);
        }
        assert!(apps.requests().is_empty());

        reactor.handle_events(apps.make_app(2, make_windows(1)));
        let _events_2 = apps.simulate_events();

        reactor.handle_event(Event::WindowDestroyed(WindowId::new(2, 1)));
        let _events_3 = apps.simulate_events();
        let state_3 = apps.windows;

        // These should be the same, because we should have resized the first
        // two windows both at the beginning, and at the end when the third
        // window was destroyed.
        for (wid, state) in dbg!(state_1) {
            assert!(state_3.contains_key(&wid), "{wid:?} not in {state_3:#?}");
            assert_eq!(state.frame, state_3[&wid].frame);
        }
    }

    #[test]
    fn it_manages_windows_on_enabled_spaces() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![full_screen],
            vec![Some(SpaceId::new(1))],
            vec![],
        ));

        reactor.handle_events(apps.make_app(1, make_windows(1)));

        let _events = apps.simulate_events();
        assert_eq!(
            full_screen,
            apps.windows.get(&WindowId::new(1, 1)).expect("Window was not resized").frame,
        );
    }

    #[test]
    fn it_ignores_windows_on_disabled_spaces() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![full_screen],
            vec![None],
            vec![],
        ));

        reactor.handle_events(apps.make_app(1, make_windows(1)));

        let state_before = apps.windows.clone();
        let _events = apps.simulate_events();
        assert_eq!(state_before, apps.windows, "Window should not have been moved",);

        // Make sure it doesn't choke on destroyed events for ignored windows.
        reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 1)));
        reactor.handle_event(Event::WindowCreated(
            WindowId::new(1, 2),
            make_window(2),
            None,
            MouseState::Up,
        ));
        reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 2)));
    }

    #[test]
    fn it_keeps_discovered_windows_on_their_initial_screen() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        let screen2 = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![screen1, screen2],
            vec![Some(SpaceId::new(1)), Some(SpaceId::new(2))],
            vec![],
        ));

        let mut windows = make_windows(2);
        windows[1].frame.origin = CGPoint::new(1100., 100.);
        reactor.handle_events(apps.make_app(1, windows));

        let _events = apps.simulate_events();
        assert_eq!(
            screen1,
            apps.windows.get(&WindowId::new(1, 1)).expect("Window was not resized").frame,
        );
        assert_eq!(
            screen2,
            apps.windows.get(&WindowId::new(1, 2)).expect("Window was not resized").frame,
        );
    }

    #[test]
    fn it_ignores_windows_on_nonzero_layers() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![full_screen],
            vec![Some(SpaceId::new(1))],
            vec![WindowServerInfo {
                id: WindowServerId::new(1),
                pid: 1,
                layer: 10,
                frame: CGRect::ZERO,
            }],
        ));

        reactor.handle_events(apps.make_app_with_opts(1, make_windows(1), None, true, false));

        let state_before = apps.windows.clone();
        let _events = apps.simulate_events();
        assert_eq!(state_before, apps.windows, "Window should not have been moved",);

        // Make sure it doesn't choke on destroyed events for ignored windows.
        reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 1)));
        reactor.handle_event(Event::WindowCreated(
            WindowId::new(1, 2),
            make_window(2),
            None,
            MouseState::Up,
        ));
        reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 2)));
    }

    #[test]
    fn handle_layout_response_groups_windows_by_app_and_screen() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let (raise_manager_tx, mut raise_manager_rx) = actor::channel();
        reactor.raise_manager_tx = raise_manager_tx;
        let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        let screen2 = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![screen1, screen2],
            vec![Some(SpaceId::new(1)), Some(SpaceId::new(2))],
            vec![],
        ));

        reactor.handle_events(apps.make_app(1, make_windows(2)));

        let mut windows = make_windows(2);
        windows[1].frame.origin = CGPoint::new(1100., 100.);
        reactor.handle_events(apps.make_app(2, windows));

        let _events = apps.simulate_events();
        while raise_manager_rx.try_recv().is_ok() {}

        reactor.handle_layout_response(layout::EventResponse {
            raise_windows: vec![
                WindowId::new(1, 1),
                WindowId::new(1, 2),
                WindowId::new(2, 1),
                WindowId::new(2, 2),
            ],
            focus_window: None,
        });
        let msg = raise_manager_rx.try_recv().expect("Should have sent an event").1;
        match msg {
            raise_manager::Event::RaiseRequest(RaiseRequest {
                raise_windows,
                focus_window,
                app_handles: _,
            }) => {
                let raise_windows: HashSet<Vec<WindowId>> = raise_windows.into_iter().collect();
                let expected = [
                    vec![WindowId::new(1, 1), WindowId::new(1, 2)],
                    vec![WindowId::new(2, 1)],
                    vec![WindowId::new(2, 2)],
                ]
                .into_iter()
                .collect();
                assert_eq!(raise_windows, expected);
                assert!(focus_window.is_none());
            }
            _ => panic!("Unexpected event: {msg:?}"),
        }
    }

    #[test]
    fn handle_layout_response_includes_handles_for_raise_and_focus_windows() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let (raise_manager_tx, mut raise_manager_rx) = actor::channel();
        reactor.raise_manager_tx = raise_manager_tx;

        reactor.handle_events(apps.make_app(1, make_windows(1)));
        reactor.handle_events(apps.make_app(2, make_windows(1)));

        let _events = apps.simulate_events();
        while raise_manager_rx.try_recv().is_ok() {}
        reactor.handle_layout_response(layout::EventResponse {
            raise_windows: vec![WindowId::new(1, 1)],
            focus_window: Some(WindowId::new(2, 1)),
        });
        let msg = raise_manager_rx.try_recv().expect("Should have sent an event").1;
        match msg {
            raise_manager::Event::RaiseRequest(RaiseRequest { app_handles, .. }) => {
                assert!(app_handles.contains_key(&1));
                assert!(app_handles.contains_key(&2));
            }
            _ => panic!("Unexpected event: {msg:?}"),
        }
    }

    #[test]
    fn it_preserves_layout_after_login_screen() {
        // TODO: This would be better tested with a more complete simulation.
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let space = SpaceId::new(1);
        let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![full_screen],
            vec![Some(space)],
            vec![],
        ));

        reactor.handle_events(apps.make_app_with_opts(
            1,
            make_windows(3),
            Some(WindowId::new(1, 1)),
            true,
            true,
        ));
        reactor.handle_event(Event::ApplicationGloballyActivated(1));
        apps.simulate_until_quiet(&mut reactor);
        let default = reactor.layout_engine.calculate_layout(
            space,
            full_screen,
            0.0,
            crate::common::config::HorizontalPlacement::Top,
            crate::common::config::VerticalPlacement::Right,
        );

        assert!(reactor.layout_engine.selected_window(space).is_some());
        reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::MoveNode(
            Direction::Up,
        ))));
        apps.simulate_until_quiet(&mut reactor);
        let modified = reactor.layout_engine.calculate_layout(
            space,
            full_screen,
            0.0,
            crate::common::config::HorizontalPlacement::Top,
            crate::common::config::VerticalPlacement::Right,
        );
        assert_ne!(default, modified);

        reactor.handle_event(Event::ScreenParametersChanged(
            vec![CGRect::ZERO],
            vec![None],
            vec![],
        ));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![full_screen],
            vec![Some(space)],
            (1..=3)
                .map(|n| WindowServerInfo {
                    pid: 1,
                    id: WindowServerId::new(n),
                    layer: 0,
                    frame: CGRect::ZERO,
                })
                .collect(),
        ));
        let requests = apps.requests();
        for request in requests {
            match request {
                Request::GetVisibleWindows => {
                    // Simulate the login screen condition: No windows are
                    // considered visible by the accessibility API, but they are
                    // from the window server API in the event above.
                    reactor.handle_event(Event::WindowsDiscovered {
                        pid: 1,
                        new: vec![],
                        known_visible: vec![],
                    });
                }
                req => {
                    let events = apps.simulate_events_for_requests(vec![req]);
                    for event in events {
                        reactor.handle_event(event);
                    }
                }
            }
        }
        apps.simulate_until_quiet(&mut reactor);

        assert_eq!(
            reactor.layout_engine.calculate_layout(
                space,
                full_screen,
                0.0,
                crate::common::config::HorizontalPlacement::Top,
                crate::common::config::VerticalPlacement::Right,
            ),
            modified
        );
    }

    #[test]
    fn it_fixes_window_sizes_after_screen_config_changes() {
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![full_screen],
            vec![Some(SpaceId::new(1))],
            vec![],
        ));

        reactor.handle_events(apps.make_app(1, make_windows(1)));

        let _events = apps.simulate_events();
        assert_eq!(
            full_screen,
            apps.windows.get(&WindowId::new(1, 1)).expect("Window was not resized").frame,
        );

        // Simulate the system resizing a window after it recognizes an old
        // configurations. Resize events are not sent in this case.
        reactor.handle_event(Event::ScreenParametersChanged(
            vec![
                full_screen,
                CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.)),
            ],
            vec![Some(SpaceId::new(1)), None],
            vec![WindowServerInfo {
                id: WindowServerId::new(1),
                pid: 1,
                layer: 0,
                frame: CGRect::new(CGPoint::new(500., 0.), CGSize::new(500., 500.)),
            }],
        ));

        let _events = apps.simulate_events();
        assert_eq!(
            full_screen,
            apps.windows.get(&WindowId::new(1, 1)).expect("Window was not resized").frame,
        );
    }

    #[test]
    fn it_doesnt_crash_after_main_window_closes() {
        use Direction::*;
        use Event::*;
        use LayoutCommand::*;

        use super::Command::*;
        use super::Reactor;
        let mut apps = Apps::new();
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let space = SpaceId::new(1);
        reactor.handle_event(ScreenParametersChanged(
            vec![CGRect::ZERO],
            vec![Some(space)],
            vec![],
        ));
        assert_eq!(None, reactor.main_window());

        reactor.handle_event(ApplicationGloballyActivated(1));
        reactor.handle_events(apps.make_app_with_opts(
            1,
            make_windows(2),
            Some(WindowId::new(1, 1)),
            true,
            true,
        ));

        reactor.handle_event(WindowDestroyed(WindowId::new(1, 1)));
        reactor.handle_event(Command(Layout(MoveFocus(Left))));
    }
}
