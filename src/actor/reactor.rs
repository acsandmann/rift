//! The Reactor's job is to maintain coherence between the system and model state.
//!
//! It takes events from the rest of the system and builds a coherent picture of
//! what is going on. It shares this with the layout actor, and reacts to layout
//! changes by sending requests out to the other actors in the system.

mod animation;
mod error;
mod events;
mod main_window;
mod managers;
mod query;
mod replay;
pub mod transaction_manager;
mod utils;

#[cfg(test)]
mod testing;

#[cfg(test)]
mod tests;

use std::thread;
use std::time::Duration;

use events::app::AppEventHandler;
use events::command::CommandEventHandler;
use events::drag::DragEventHandler;
use events::space::SpaceEventHandler;
use events::system::SystemEventHandler;
use events::window::WindowEventHandler;
use main_window::MainWindowTracker;
use managers::LayoutManager;
use objc2_core_foundation::{CGPoint, CGRect};
pub use replay::{Record, replay};
use serde::{Deserialize, Serialize};
use serde_json;
use serde_with::serde_as;
use tracing::{debug, instrument, trace, warn};
use transaction_manager::TransactionId;

use super::event_tap;
use crate::actor::app::{AppInfo, AppThreadHandle, Quiet, Request, WindowId, WindowInfo, pid_t};
use crate::actor::broadcast::{BroadcastEvent, BroadcastSender};
use crate::actor::raise_manager::{self, RaiseManager, RaiseRequest};
use crate::actor::reactor::events::window_discovery::WindowDiscoveryHandler;
use crate::actor::{self, menu_bar, stack_line};
use crate::common::collections::{BTreeMap, HashMap, HashSet};
use crate::common::config::Config;
use crate::common::log::MetricsCommand;
use crate::layout_engine::{self as layout, Direction, LayoutCommand, LayoutEngine, LayoutEvent};
use crate::model::VirtualWorkspaceId;
use crate::model::tx_store::WindowTxStore;
use crate::model::virtual_workspace::AppRuleResult;
use crate::sys::event::MouseState;
use crate::sys::executor::Executor;
use crate::sys::geometry::{CGRectDef, CGRectExt};
use crate::sys::screen::{ScreenId, SpaceId, get_active_space_number};
use crate::sys::timer::Timer;
use crate::sys::window_server::{
    self, WindowServerId, WindowServerInfo, current_cursor_location, space_is_fullscreen,
    wait_for_native_fullscreen_transition, window_level,
};

pub type Sender = actor::Sender<Event>;
type Receiver = actor::Receiver<Event>;
use std::collections::VecDeque;
use std::path::PathBuf;

use crate::model::server::{
    ApplicationData, DisplayData, LayoutStateData, WindowData, WorkspaceData,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    #[serde(with = "CGRectDef")]
    pub frame: CGRect,
    pub space: Option<SpaceId>,
    pub display_uuid: String,
    pub name: Option<String>,
    pub screen_id: u32,
}

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
    ScreenParametersChanged(Vec<ScreenSnapshot>, Vec<WindowServerInfo>),

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

    /// Which spaces Rift is currently managing (subset of the SpaceChanged list).
    ActiveSpacesChanged(Vec<Option<SpaceId>>),

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
    WindowCreated(
        WindowId,
        WindowInfo,
        Option<WindowServerInfo>,
        Option<MouseState>,
    ),
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
    WindowTitleChanged(WindowId, String),
    ResyncAppForWindow(WindowServerId),
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
    QueryWorkspaces {
        space_id: Option<SpaceId>,
        #[serde(skip)]
        response: r#continue::Sender<Vec<WorkspaceData>>,
    },
    #[serde(skip)]
    QueryWindows {
        space_id: Option<SpaceId>,
        #[serde(skip)]
        response: r#continue::Sender<Vec<WindowData>>,
    },
    #[serde(skip)]
    QueryActiveWorkspace {
        space_id: Option<SpaceId>,
        #[serde(skip)]
        response: r#continue::Sender<Option<VirtualWorkspaceId>>,
    },
    #[serde(skip)]
    QueryDisplays(r#continue::Sender<Vec<DisplayData>>),
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum Command {
    Layout(LayoutCommand),
    Metrics(MetricsCommand),
    Reactor(ReactorCommand),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum DisplaySelector {
    Direction(Direction),
    Index(usize),
    Uuid(String),
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
    ShowMissionControlAll,
    ShowMissionControlCurrent,
    DismissMissionControl,
    MoveMouseToDisplay(DisplaySelector),
    FocusDisplay(DisplaySelector),
    CloseWindow {
        window_server_id: Option<WindowServerId>,
    },
    MoveWindowToDisplay {
        selector: DisplaySelector,
        window_id: Option<u32>,
    },
}

#[derive(Default, Debug, Clone)]
struct FullscreenTrack {
    pids: HashSet<pid_t>,
    last_removed: VecDeque<WindowServerId>,
}

#[derive(Debug, Clone)]
pub struct DragSession {
    window: WindowId,
    last_frame: CGRect,
    origin_space: Option<SpaceId>,
    settled_space: Option<SpaceId>,
    layout_dirty: bool,
}

#[derive(Debug, Clone)]
pub enum DragState {
    Inactive,
    Active {
        session: DragSession,
    },
    PendingSwap {
        session: DragSession,
        target: WindowId,
    },
}

#[derive(Debug, Clone)]
pub enum MissionControlState {
    Inactive,
    Active,
    Transitioning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuState {
    Closed,
    Open(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSwitchState {
    Inactive,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSwitchOrigin {
    Manual,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleCleanupState {
    Enabled,
    Suppressed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RefocusState {
    None,
    Pending(SpaceId),
}

pub struct Reactor {
    config_manager: managers::ConfigManager,
    app_manager: managers::AppManager,
    layout_manager: managers::LayoutManager,
    window_manager: managers::WindowManager,
    window_server_info_manager: managers::WindowServerInfoManager,
    space_manager: managers::SpaceManager,
    main_window_tracker_manager: managers::MainWindowTrackerManager,
    drag_manager: managers::DragManager,
    workspace_switch_manager: managers::WorkspaceSwitchManager,
    recording_manager: managers::RecordingManager,
    communication_manager: managers::CommunicationManager,
    notification_manager: managers::NotificationManager,
    transaction_manager: transaction_manager::TransactionManager,
    menu_manager: managers::MenuManager,
    mission_control_manager: managers::MissionControlManager,
    refocus_manager: managers::RefocusManager,
    pending_space_change_manager: managers::PendingSpaceChangeManager,
    active_spaces: HashSet<SpaceId>,
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

#[derive(Clone, Debug)]
struct Screen {
    frame: CGRect,
    space: Option<SpaceId>,
    display_uuid: String,
    name: Option<String>,
    screen_id: ScreenId,
}

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
    ignore_app_rule: bool,
    window_server_id: Option<WindowServerId>,
    #[allow(unused)]
    bundle_id: Option<String>,
    #[allow(unused)]
    bundle_path: Option<PathBuf>,
    ax_role: Option<String>,
    ax_subrole: Option<String>,
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
            ignore_app_rule: false,
            window_server_id: info.sys_id,
            bundle_id: info.bundle_id,
            bundle_path: info.path,
            ax_role: info.ax_role,
            ax_subrole: info.ax_subrole,
        }
    }
}

impl WindowState {
    fn is_effectively_manageable(&self) -> bool { self.is_manageable && !self.ignore_app_rule }
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
                reactor.communication_manager.event_tap_tx = Some(event_tap_tx);
                reactor.menu_manager.menu_tx = Some(menu_tx);
                reactor.communication_manager.stack_line_tx = Some(stack_line_tx);
                reactor.communication_manager.events_tx = Some(events_tx_clone.clone());
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
            Some((tx, store)) => (Some(tx), store),
            None => (None, WindowTxStore::new()),
        };
        Reactor {
            config_manager: managers::ConfigManager { config: config.clone() },
            app_manager: managers::AppManager::new(),
            layout_manager: managers::LayoutManager { layout_engine },
            window_manager: managers::WindowManager {
                windows: HashMap::default(),
                window_ids: HashMap::default(),
                visible_windows: HashSet::default(),
                observed_window_server_ids: HashSet::default(),
            },
            window_server_info_manager: managers::WindowServerInfoManager {
                window_server_info: HashMap::default(),
            },
            space_manager: managers::SpaceManager {
                screens: vec![],
                fullscreen_by_space: HashMap::default(),
                changing_screens: HashSet::default(),
                screen_space_by_id: HashMap::default(),
            },
            main_window_tracker_manager: managers::MainWindowTrackerManager {
                main_window_tracker: MainWindowTracker::default(),
            },
            drag_manager: managers::DragManager {
                drag_state: DragState::Inactive,
                drag_swap_manager: crate::actor::drag_swap::DragManager::new(
                    config.settings.window_snapping,
                ),
                skip_layout_for_window: None,
            },
            workspace_switch_manager: managers::WorkspaceSwitchManager {
                workspace_switch_state: WorkspaceSwitchState::Inactive,
                workspace_switch_generation: 0,
                active_workspace_switch: None,
                pending_workspace_switch_origin: None,
                pending_workspace_mouse_warp: None,
            },
            recording_manager: managers::RecordingManager { record },
            communication_manager: managers::CommunicationManager {
                event_tap_tx: None,
                stack_line_tx: None,
                raise_manager_tx,
                event_broadcaster: broadcast_tx,
                wm_sender: None,
                events_tx: None,
            },
            notification_manager: managers::NotificationManager {
                last_sls_notification_ids: Vec::new(),
                _window_notify_tx: window_notify_tx,
            },
            transaction_manager: transaction_manager::TransactionManager::new(window_tx_store),
            menu_manager: managers::MenuManager {
                menu_state: MenuState::Closed,
                menu_tx: None,
            },
            mission_control_manager: managers::MissionControlManager {
                mission_control_state: MissionControlState::Inactive,
                pending_mission_control_refresh: HashSet::default(),
            },
            refocus_manager: managers::RefocusManager {
                stale_cleanup_state: StaleCleanupState::Enabled,
                refocus_state: RefocusState::None,
            },
            pending_space_change_manager: managers::PendingSpaceChangeManager {
                pending_space_change: None,
                topology_relayout_pending: false,
            },
            active_spaces: HashSet::default(),
        }
    }

    fn set_active_spaces(&mut self, spaces: &[Option<SpaceId>]) {
        self.active_spaces.clear();
        for space in spaces.iter().flatten().copied() {
            self.active_spaces.insert(space);
        }
    }

    fn is_space_active(&self, space: SpaceId) -> bool { self.active_spaces.contains(&space) }

    // fn store_txid(&self, wsid: Option<WindowServerId>, txid: TransactionId, target: CGRect) {
    //     self.transaction_manager.store_txid(wsid, txid, target);
    // }
    //
    // fn update_txid_entries<I>(&self, entries: I)
    // where
    //     I: IntoIterator<Item = (WindowServerId, TransactionId, CGRect)>,
    // {
    //     self.transaction_manager.update_entries(entries);
    // }
    //
    // fn remove_txid_for_window(&self, wsid: Option<WindowServerId>) {
    //     self.transaction_manager.remove_for_window(wsid);
    // }

    fn is_in_drag(&self) -> bool {
        matches!(
            self.drag_manager.drag_state,
            DragState::Active { .. } | DragState::PendingSwap { .. }
        )
    }

    fn is_mission_control_active(&self) -> bool {
        matches!(
            self.mission_control_manager.mission_control_state,
            MissionControlState::Active
        )
    }

    fn get_pending_drag_swap(&self) -> Option<(WindowId, WindowId)> {
        if let DragState::PendingSwap { session, target } = &self.drag_manager.drag_state {
            Some((session.window, *target))
        } else {
            None
        }
    }

    fn get_active_drag_session(&self) -> Option<&DragSession> {
        if let DragState::Active { session } = &self.drag_manager.drag_state {
            Some(session)
        } else {
            None
        }
    }

    fn get_active_drag_session_mut(&mut self) -> Option<&mut DragSession> {
        if let DragState::Active { session } = &mut self.drag_manager.drag_state {
            Some(session)
        } else {
            None
        }
    }

    fn take_active_drag_session(&mut self) -> Option<DragSession> {
        match std::mem::replace(&mut self.drag_manager.drag_state, DragState::Inactive) {
            DragState::Active { session } => Some(session),
            DragState::PendingSwap { session, .. } => Some(session),
            _ => None,
        }
    }

    pub async fn run(mut self, events: Receiver, events_tx: Sender) {
        let (raise_manager_tx, raise_manager_rx) = actor::channel();
        self.communication_manager.raise_manager_tx = raise_manager_tx.clone();

        let event_tap_tx = self.communication_manager.event_tap_tx.clone();
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
        self.recording_manager.record.on_event(&event);

        if matches!(
            event,
            Event::QueryApplications(..)
                | Event::QueryLayoutState { .. }
                | Event::QueryMetrics(..)
                | Event::QueryWindowInfo { .. }
                | Event::QueryWindows { .. }
                | Event::QueryWorkspaces { .. }
                | Event::QueryActiveWorkspace { .. }
                | Event::QueryDisplays(..)
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

        let raised_window =
            self.main_window_tracker_manager.main_window_tracker.handle_event(&event);
        let mut is_resize = false;
        let mut window_was_destroyed = false;

        match event {
            Event::ApplicationLaunched {
                pid,
                info,
                handle,
                visible_windows,
                window_server_info,
                is_frontmost,
                main_window,
            } => {
                AppEventHandler::handle_application_launched(
                    self,
                    pid,
                    info,
                    handle,
                    visible_windows,
                    window_server_info,
                    is_frontmost,
                    main_window,
                );
            }
            Event::ApplyAppRulesToExistingWindows { pid, app_info, windows } => {
                AppEventHandler::handle_apply_app_rules_to_existing_windows(
                    self, pid, app_info, windows,
                );
            }
            Event::ApplicationTerminated(pid) => {
                AppEventHandler::handle_application_terminated(self, pid);
            }
            Event::ApplicationThreadTerminated(pid) => {
                AppEventHandler::handle_application_thread_terminated(self, pid);
            }
            Event::ApplicationActivated(pid, quiet) => {
                AppEventHandler::handle_application_activated(self, pid, quiet);
            }
            Event::ApplicationDeactivated(..)
            | Event::ApplicationGloballyDeactivated(..)
            | Event::ApplicationMainWindowChanged(..) => {}
            Event::ResyncAppForWindow(wsid) => {
                AppEventHandler::handle_resync_app_for_window(self, wsid);
            }
            // we know rely on axapp's activation event (https://github.com/acsandmann/rift/issues/108)
            Event::ApplicationGloballyActivated(_) => {}
            Event::RegisterWmSender(sender) => {
                SystemEventHandler::handle_register_wm_sender(self, sender)
            }
            Event::WindowsDiscovered { pid, new, known_visible } => {
                AppEventHandler::handle_windows_discovered(self, pid, new, known_visible);
            }
            Event::WindowCreated(wid, window, ws_info, mouse_state) => {
                WindowEventHandler::handle_window_created(self, wid, window, ws_info, mouse_state);
            }
            Event::WindowDestroyed(wid) => {
                window_was_destroyed = WindowEventHandler::handle_window_destroyed(self, wid);
            }
            Event::WindowServerDestroyed(wsid, sid) => {
                SpaceEventHandler::handle_window_server_destroyed(self, wsid, sid);
            }
            Event::WindowServerAppeared(wsid, sid) => {
                SpaceEventHandler::handle_window_server_appeared(self, wsid, sid);
            }
            Event::WindowMinimized(wid) => {
                WindowEventHandler::handle_window_minimized(self, wid);
            }
            Event::WindowDeminiaturized(wid) => {
                WindowEventHandler::handle_window_deminiaturized(self, wid);
            }
            Event::WindowFrameChanged(wid, new_frame, last_seen, requested, mouse_state) => {
                is_resize = WindowEventHandler::handle_window_frame_changed(
                    self,
                    wid,
                    new_frame,
                    last_seen,
                    requested,
                    mouse_state,
                );
            }
            Event::WindowTitleChanged(wid, new_title) => {
                WindowEventHandler::handle_window_title_changed(self, wid, new_title);
            }
            Event::ScreenParametersChanged(screens, ws_info) => {
                SpaceEventHandler::handle_screen_parameters_changed(self, screens, ws_info);
            }
            Event::ActiveSpacesChanged(spaces) => {
                SpaceEventHandler::handle_active_spaces_changed(self, spaces);
            }
            Event::SpaceChanged(spaces, ws_info) => {
                SpaceEventHandler::handle_space_changed(self, spaces, ws_info);
            }
            Event::MouseUp => {
                DragEventHandler::handle_mouse_up(self);
            }
            Event::MenuOpened => SystemEventHandler::handle_menu_opened(self),
            Event::MenuClosed => SystemEventHandler::handle_menu_closed(self),
            Event::MouseMovedOverWindow(wsid) => {
                WindowEventHandler::handle_mouse_moved_over_window(self, wsid);
            }
            Event::SystemWoke => SystemEventHandler::handle_system_woke(self),
            Event::MissionControlNativeEntered => {
                SpaceEventHandler::handle_mission_control_native_entered(self);
            }
            Event::MissionControlNativeExited => {
                SpaceEventHandler::handle_mission_control_native_exited(self);
            }
            Event::RaiseCompleted { window_id, sequence_id } => {
                SystemEventHandler::handle_raise_completed(self, window_id, sequence_id);
            }
            Event::RaiseTimeout { sequence_id } => {
                SystemEventHandler::handle_raise_timeout(self, sequence_id);
            }
            Event::Command(Command::Layout(cmd)) => {
                CommandEventHandler::handle_command_layout(self, cmd);
            }
            Event::Command(Command::Metrics(cmd)) => {
                CommandEventHandler::handle_command_metrics(self, cmd);
            }
            Event::ConfigUpdated(new_cfg) => {
                CommandEventHandler::handle_config_updated(self, new_cfg);
            }
            Event::Command(Command::Reactor(ReactorCommand::Debug)) => {
                CommandEventHandler::handle_command_reactor_debug(self);
            }
            Event::Command(Command::Reactor(ReactorCommand::Serialize)) => {
                CommandEventHandler::handle_command_reactor_serialize(self);
            }
            Event::Command(Command::Reactor(ReactorCommand::SaveAndExit)) => {
                CommandEventHandler::handle_command_reactor_save_and_exit(self);
            }
            Event::Command(Command::Reactor(ReactorCommand::SwitchSpace(dir))) => {
                CommandEventHandler::handle_command_reactor_switch_space(self, dir);
            }
            Event::Command(Command::Reactor(ReactorCommand::FocusWindow {
                window_id: wid,
                window_server_id,
            })) => {
                CommandEventHandler::handle_command_reactor_focus_window(
                    self,
                    wid,
                    window_server_id,
                );
            }
            Event::Command(Command::Reactor(ReactorCommand::ShowMissionControlAll)) => {
                CommandEventHandler::handle_command_reactor_show_mission_control_all(self);
            }
            Event::Command(Command::Reactor(ReactorCommand::ShowMissionControlCurrent)) => {
                CommandEventHandler::handle_command_reactor_show_mission_control_current(self);
            }
            Event::Command(Command::Reactor(ReactorCommand::DismissMissionControl)) => {
                CommandEventHandler::handle_command_reactor_dismiss_mission_control(self);
            }
            Event::Command(Command::Reactor(ReactorCommand::MoveMouseToDisplay(selector))) => {
                CommandEventHandler::handle_command_reactor_move_mouse_to_display(self, &selector);
            }
            Event::Command(Command::Reactor(ReactorCommand::FocusDisplay(selector))) => {
                CommandEventHandler::handle_command_reactor_focus_display(self, &selector);
            }
            Event::Command(Command::Reactor(ReactorCommand::MoveWindowToDisplay {
                selector,
                window_id,
            })) => {
                CommandEventHandler::handle_command_reactor_move_window_to_display(
                    self, &selector, window_id,
                );
            }
            Event::Command(Command::Reactor(ReactorCommand::CloseWindow { window_server_id })) => {
                CommandEventHandler::handle_command_reactor_close_window(self, window_server_id)
            }
            _ => (),
        }
        if let Some(raised_window) = raised_window {
            if let Some(space) =
                self.window_manager.windows.get(&raised_window).and_then(|w| {
                    self.best_space_for_window(&w.frame_monotonic, w.window_server_id)
                })
            {
                self.send_layout_event(LayoutEvent::WindowFocused(space, raised_window));
            }
        }

        let mut layout_changed = false;
        if !self.is_in_drag() || window_was_destroyed {
            layout_changed = self
                .update_layout(
                    is_resize,
                    matches!(
                        self.workspace_switch_manager.workspace_switch_state,
                        WorkspaceSwitchState::Active
                    ),
                )
                .unwrap_or_else(|e| {
                    warn!("Layout update failed: {}", e);
                    false
                });
            self.maybe_send_menu_update();
        }

        self.workspace_switch_manager.mark_workspace_switch_inactive();
        if self.workspace_switch_manager.active_workspace_switch.is_some() && !layout_changed {
            self.workspace_switch_manager.active_workspace_switch = None;
            trace!("Workspace switch stabilized with no further frame changes");
        }

        // Execute deferred mouse warp after workspace switch completes
        if let Some(wid) = self.workspace_switch_manager.pending_workspace_mouse_warp.take() {
            if let Some(window) = self.window_manager.windows.get(&wid) {
                let window_center = window.frame_monotonic.mid();
                if self.space_manager.screens.iter().any(|s| s.frame.contains(window_center)) {
                    if let Some(event_tap_tx) = self.communication_manager.event_tap_tx.as_ref() {
                        event_tap_tx.send(crate::actor::event_tap::Request::Warp(window_center));
                    }
                }
            }
        }

        if should_update_notifications {
            let mut ids: Vec<u32> =
                self.window_manager.window_ids.keys().map(|wsid| wsid.as_u32()).collect();
            ids.sort_unstable();

            if ids != self.notification_manager.last_sls_notification_ids {
                crate::sys::window_notify::update_window_notifications(&ids);

                self.notification_manager.last_sls_notification_ids = ids;
            }
        }
    }

    fn create_window_data(&self, window_id: WindowId) -> Option<WindowData> {
        let window_state = self.window_manager.windows.get(&window_id)?;
        if window_state.is_minimized {
            return None;
        }
        let app = self.app_manager.apps.get(&window_id.pid)?;

        let preferred_name = app.info.localized_name.clone().or_else(|| app.info.bundle_id.clone());

        Some(WindowData {
            id: window_id,
            title: window_state.title.clone(),
            frame: window_state.frame_monotonic,
            is_floating: self.layout_manager.layout_engine.is_window_floating(window_id),
            is_focused: self.main_window() == Some(window_id),
            bundle_id: preferred_name,
            window_server_id: window_state.window_server_id.map(|wsid| wsid.as_u32()),
        })
    }

    fn update_complete_window_server_info(&mut self, ws_info: Vec<WindowServerInfo>) {
        self.window_manager.visible_windows.clear();
        self.update_partial_window_server_info(ws_info);
    }

    fn update_partial_window_server_info(&mut self, ws_info: Vec<WindowServerInfo>) {
        // Mark visible windows and remove any corresponding observed WSID markers
        // for ids we now have server info for.
        self.window_manager.visible_windows.extend(ws_info.iter().map(|info| info.id));
        for info in ws_info.iter() {
            // If we've been observing this server id from SLS callbacks, clear it.
            self.window_manager.observed_window_server_ids.remove(&info.id);
            self.window_server_info_manager.window_server_info.insert(info.id, *info);

            if let Some(wid) = self.window_manager.window_ids.get(&info.id).copied() {
                let (server_id, is_minimized, is_ax_standard, is_ax_root) =
                    if let Some(window) = self.window_manager.windows.get_mut(&wid) {
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
                let manageable = utils::compute_window_manageability(
                    server_id,
                    is_minimized,
                    is_ax_standard,
                    is_ax_root,
                    &self.window_server_info_manager.window_server_info,
                );
                if let Some(window) = self.window_manager.windows.get_mut(&wid) {
                    window.is_manageable = manageable;
                }
            }
        }
    }

    fn check_for_new_windows(&mut self) {
        // TODO: Do this correctly/more optimally using CGWindowListCopyWindowInfo
        // (see notes for on_windows_discovered below).
        for app in self.app_manager.apps.values_mut() {
            // Errors mean the app terminated (and a termination event
            // is coming); ignore.
            _ = app.handle.send(Request::GetVisibleWindows { force_refresh: false });
        }
    }

    fn handle_fullscreen_space_transition(&mut self, spaces: &mut Vec<Option<SpaceId>>) -> bool {
        let mut saw_fullscreen = false;
        let mut all_fullscreen = !spaces.is_empty();
        let mut refresh_spaces = Vec::new();

        for slot in spaces.iter_mut() {
            match slot {
                Some(space) if space_is_fullscreen(space.get()) => {
                    saw_fullscreen = true;
                    *slot = None;
                }
                Some(space) => {
                    all_fullscreen = false;
                    refresh_spaces.push(*space);
                }
                None => {
                    all_fullscreen = false;
                }
            }
        }

        if saw_fullscreen && all_fullscreen {
            return true;
        }

        for space in refresh_spaces {
            if let Some(track) = self.space_manager.fullscreen_by_space.remove(&space.get()) {
                wait_for_native_fullscreen_transition();
                Timer::sleep(Duration::from_millis(50));
                for pid in track.pids {
                    if let Some(app) = self.app_manager.apps.get(&pid) {
                        if let Err(e) =
                            app.handle.send(Request::GetVisibleWindows { force_refresh: true })
                        {
                            warn!("Failed to send GetVisibleWindows to app {}: {}", pid, e);
                        }
                    }
                }
                self.refocus_manager.refocus_state = RefocusState::Pending(space);
                self.update_layout(false, false).unwrap_or_else(|e| {
                    warn!("Layout update failed: {}", e);
                    false
                });
                self.update_focus_follows_mouse_state();
            }
        }

        false
    }

    fn set_screen_spaces(&mut self, spaces: &[Option<SpaceId>]) {
        for (space, screen) in spaces.iter().copied().zip(&mut self.space_manager.screens) {
            screen.space = space;
        }
        self.update_screen_space_map();
    }

    fn reconcile_spaces_with_display_history(
        &mut self,
        spaces: &[Option<SpaceId>],
        allow_remap: bool,
    ) {
        let mut seen_displays: HashSet<String> = HashSet::default();

        for (screen, space_opt) in self.space_manager.screens.iter().zip(spaces.iter()) {
            let Some(space) = space_opt else {
                continue;
            };
            if screen.display_uuid.is_empty() {
                continue;
            }
            if !seen_displays.insert(screen.display_uuid.clone()) {
                continue;
            }

            let seen_before =
                self.layout_manager.layout_engine.display_seen_before(&screen.display_uuid);
            let last_space = if allow_remap && seen_before {
                self.layout_manager
                    .layout_engine
                    .last_space_for_display_uuid(&screen.display_uuid)
            } else {
                None
            };

            // When a display reconnects, remap the most recent space observed for
            // that display to the newly reported space so layout state follows the
            // monitor. During routine space switches (allow_remap=false), we simply
            // record the mapping without remapping.
            if allow_remap {
                if let Some(previous_space) = last_space {
                    if previous_space != *space {
                        self.layout_manager.layout_engine.remap_space(previous_space, *space);
                    }
                }
            }
            self.layout_manager
                .layout_engine
                .update_space_display(*space, Some(screen.display_uuid.clone()));
        }
    }

    fn update_screen_space_map(&mut self) {
        let valid_screen_ids: HashSet<ScreenId> =
            self.space_manager.screens.iter().map(|screen| screen.screen_id).collect();
        for screen in &self.space_manager.screens {
            if let Some(space) = screen.space {
                self.space_manager.screen_space_by_id.insert(screen.screen_id, space);
                trace!("screen {:?} -> space {:?}", screen.screen_id, space);
            } else {
                let removed = self.space_manager.screen_space_by_id.remove(&screen.screen_id);
                trace!("screen {:?} -> None (removed={:?})", screen.screen_id, removed);
            }
        }
        trace!("final map = {:?}", self.space_manager.screen_space_by_id);
        self.space_manager
            .screen_space_by_id
            .retain(|screen_id, _| valid_screen_ids.contains(screen_id));
    }

    fn finalize_space_change(
        &mut self,
        spaces: &[Option<SpaceId>],
        ws_info: Vec<WindowServerInfo>,
    ) {
        self.refocus_manager.stale_cleanup_state = if spaces.iter().all(|space| space.is_none()) {
            StaleCleanupState::Suppressed
        } else {
            StaleCleanupState::Enabled
        };
        self.expose_all_spaces();
        self.space_manager.changing_screens.clear();
        if let Some(main_window) = self.main_window() {
            if let Some(space) = self.main_window_space() {
                self.send_layout_event(LayoutEvent::WindowFocused(space, main_window));
            }
        }
        self.update_complete_window_server_info(ws_info);
        self.check_for_new_windows();

        if let Some(space) = spaces.iter().copied().flatten().next() {
            if let Some(workspace_id) = self.layout_manager.layout_engine.active_workspace(space) {
                let workspace_name = self
                    .layout_manager
                    .layout_engine
                    .workspace_name(space, workspace_id)
                    .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));
                let display_uuid = self.space_manager.screen_by_space(space).and_then(|screen| {
                    if screen.display_uuid.is_empty() {
                        None
                    } else {
                        Some(screen.display_uuid.clone())
                    }
                });
                let broadcast_event = BroadcastEvent::WorkspaceChanged {
                    workspace_id,
                    workspace_name,
                    space_id: space,
                    display_uuid,
                };
                _ = self.communication_manager.event_broadcaster.send(broadcast_event);
            }
        }
    }

    fn broadcast_window_title_changed(
        &mut self,
        window_id: WindowId,
        previous_title: String,
        new_title: String,
    ) {
        if previous_title != new_title
            && let Some(space) = self.best_space_for_window_id(window_id)
            && let Some(workspace_id) = self.layout_manager.layout_engine.active_workspace(space)
        {
            let workspace_index = self.layout_manager.layout_engine.active_workspace_idx(space);

            let workspace_name = self
                .layout_manager
                .layout_engine
                .workspace_name(space, workspace_id)
                .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));

            let display_uuid = self.space_manager.screen_by_space(space).and_then(|screen| {
                if screen.display_uuid.is_empty() {
                    None
                } else {
                    Some(screen.display_uuid.clone())
                }
            });

            let event = BroadcastEvent::WindowTitleChanged {
                window_id,
                workspace_id,
                workspace_index,
                workspace_name,
                previous_title,
                new_title,
                space_id: space,
                display_uuid,
            };
            let _ = self.communication_manager.event_broadcaster.send(event);
        }
    }

    fn maybe_reapply_app_rules_for_window(&mut self, window_id: WindowId) {
        if !self.config_manager.config.virtual_workspaces.reapply_app_rules_on_title_change {
            return;
        }

        let (is_manageable, wsid) = match self.window_manager.windows.get(&window_id) {
            Some(window_state) => (window_state.is_manageable, window_state.window_server_id),
            None => return,
        };

        if !is_manageable {
            return;
        }

        let app_info = match self.app_manager.apps.get(&window_id.pid) {
            Some(app_state) => app_state.info.clone(),
            None => return,
        };

        if let Some(window_server_id) = wsid {
            self.app_manager.mark_wsids_recent(std::iter::once(window_server_id));
        }

        self.process_windows_for_app_rules(window_id.pid, vec![window_id], app_info);
    }

    fn try_apply_pending_space_change(&mut self) {
        if let Some(mut pending) = self.pending_space_change_manager.pending_space_change.take() {
            if pending.spaces.len() == self.space_manager.screens.len() {
                if self.handle_fullscreen_space_transition(&mut pending.spaces) {
                    return;
                }
                self.set_screen_spaces(&pending.spaces);
                self.finalize_space_change(&pending.spaces, pending.ws_info);
            } else {
                self.pending_space_change_manager.pending_space_change = Some(pending);
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
        WindowDiscoveryHandler::handle_discovery(self, pid, new, known_visible, app_info);
    }

    fn best_space_for_window(
        &self,
        frame: &CGRect,
        window_server_id: Option<WindowServerId>,
    ) -> Option<SpaceId> {
        if let Some(server_id) = window_server_id {
            if let Some(space) = crate::sys::window_server::window_space(server_id) {
                if self.space_manager.screen_by_space(space).is_some() {
                    return Some(space);
                }
            }
        }

        self.best_space_for_frame(frame)
    }

    fn best_space_for_frame(&self, frame: &CGRect) -> Option<SpaceId> {
        let center = frame.mid();
        self.space_manager
            .screens
            .iter()
            .find_map(|screen| {
                let space = self.space_manager.space_for_screen(screen)?;
                if screen.frame.contains(center) {
                    Some(space)
                } else {
                    None
                }
            })
            .or_else(|| {
                self.space_manager
                    .screens
                    .iter()
                    .filter_map(|screen| {
                        let space = self.space_manager.space_for_screen(screen)?;
                        let area = screen.frame.intersection(frame).area() as i64;
                        Some((area, space))
                    })
                    .max_by_key(|(area, _)| *area)
                    .map(|(_, space)| space)
            })
    }

    fn ensure_active_drag(&mut self, wid: WindowId, frame: &CGRect) {
        let needs_new_session =
            self.get_active_drag_session().map_or(true, |session| session.window != wid);
        if needs_new_session {
            let server_id =
                self.window_manager.windows.get(&wid).and_then(|window| window.window_server_id);
            let origin_space = self.best_space_for_window(frame, server_id);
            let session = DragSession {
                window: wid,
                last_frame: *frame,
                origin_space,
                settled_space: origin_space,
                layout_dirty: false,
            };
            self.drag_manager.drag_state = DragState::Active { session };
        }
        if self.drag_manager.skip_layout_for_window != Some(wid) {
            self.drag_manager.skip_layout_for_window = Some(wid);
        }
    }

    fn update_active_drag(&mut self, wid: WindowId, new_frame: &CGRect) {
        let resolved_space = match self.get_active_drag_session() {
            Some(session) if session.window == wid => self.resolve_drag_space(session, new_frame),
            _ => return,
        };

        if let Some(session) = self.get_active_drag_session_mut() {
            if session.window != wid {
                return;
            }
            let frame_changed = session.last_frame != *new_frame;
            session.last_frame = *new_frame;
            if frame_changed {
                session.layout_dirty = true;
            }
            if session.settled_space != resolved_space {
                session.settled_space = resolved_space;
                session.layout_dirty = true;
                self.drag_manager.skip_layout_for_window = Some(session.window);
            }
        }
    }

    fn mark_drag_dirty(&mut self, wid: WindowId) {
        if let Some(session) = self.get_active_drag_session_mut() {
            if session.window == wid {
                session.layout_dirty = true;
                if self.drag_manager.skip_layout_for_window != Some(wid) {
                    self.drag_manager.skip_layout_for_window = Some(wid);
                }
            }
        }
    }

    fn drag_space_candidate(&self, frame: &CGRect) -> Option<SpaceId> {
        let center = frame.mid();
        self.space_manager.screens.iter().find_map(|screen| {
            let space = self.space_manager.space_for_screen(screen)?;
            if screen.frame.contains(center) {
                Some(space)
            } else {
                None
            }
        })
    }

    fn resolve_drag_space(&self, session: &DragSession, frame: &CGRect) -> Option<SpaceId> {
        let server_id = self
            .window_manager
            .windows
            .get(&session.window)
            .and_then(|window| window.window_server_id);
        if frame.area() <= 0.0 {
            return session.settled_space.or_else(|| self.best_space_for_window(frame, server_id));
        }

        self.drag_space_candidate(frame)
            .or_else(|| self.best_space_for_window(frame, server_id))
            .or(session.settled_space)
    }

    fn best_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.window_manager.windows.get(&wid).and_then(|window| {
            self.best_space_for_window(&window.frame_monotonic, window.window_server_id)
        })
    }

    fn finalize_active_drag(&mut self) -> bool {
        let Some(session) = self.take_active_drag_session() else {
            return false;
        };
        let wid = session.window;

        // During a drag the window server can continue reporting the origin
        // space even after the user has moved the window onto another display.
        // Trust the drag session’s resolved space (or the final frame’s screen)
        // before falling back to the server-reported space so that cross-display
        // drags do not snap the window back to the original monitor.
        let final_space = session
            .settled_space
            .or_else(|| self.best_space_for_frame(&session.last_frame))
            .or_else(|| self.best_space_for_window_id(wid));

        let needs_layout = if session.origin_space != final_space {
            if session.origin_space.is_some() {
                self.send_layout_event(LayoutEvent::WindowRemoved(wid));
            }
            if let Some(space) = final_space {
                if let Some(active_ws) = self.layout_manager.layout_engine.active_workspace(space) {
                    let assigned = self
                        .layout_manager
                        .layout_engine
                        .virtual_workspace_manager_mut()
                        .assign_window_to_workspace(space, wid, active_ws);
                    if !assigned {
                        warn!("Failed to assign window {:?} to workspace {:?}", wid, active_ws);
                    }
                }
                self.send_layout_event(LayoutEvent::WindowAdded(space, wid));
            }
            self.drag_manager.skip_layout_for_window = Some(wid);
            true
        } else if session.layout_dirty {
            self.drag_manager.skip_layout_for_window = Some(wid);
            true
        } else {
            false
        };

        if let Some(space) = final_space {
            if self.layout_manager.layout_engine.is_window_floating(wid) {
                if let Some(ws_id) = self
                    .layout_manager
                    .layout_engine
                    .virtual_workspace_manager()
                    .workspace_for_window(space, wid)
                    .or_else(|| self.layout_manager.layout_engine.active_workspace(space))
                {
                    self.layout_manager
                        .layout_engine
                        .virtual_workspace_manager_mut()
                        .store_floating_position(space, ws_id, wid, session.last_frame);
                }
            }
        }

        needs_layout
    }

    fn pid_has_changing_screens(&self, pid: pid_t) -> bool {
        self.space_manager.changing_screens.iter().any(|wsid| {
            if let Some(wid) = self.window_manager.window_ids.get(wsid) {
                wid.pid == pid
            } else if let Some(info) = self.window_server_info_manager.window_server_info.get(wsid)
            {
                info.pid == pid
            } else if let Some(info) = crate::sys::window_server::get_window(*wsid) {
                info.pid == pid
            } else {
                false
            }
        })
    }

    fn has_visible_window_server_ids_for_pid(&self, pid: pid_t) -> bool {
        self.window_manager.visible_windows.iter().any(|wsid| {
            self.window_manager.window_ids.get(wsid).map_or(false, |wid| wid.pid == pid)
        })
    }

    fn expose_all_spaces(&mut self) {
        let screens = self.space_manager.screens.clone();
        for screen in screens {
            let Some(space) = self.space_manager.space_for_screen(&screen) else {
                continue;
            };
            self.layout_manager
                .layout_engine
                .virtual_workspace_manager_mut()
                .list_workspaces(space);
            self.send_layout_event(LayoutEvent::SpaceExposed(space, screen.frame.size));
        }
    }

    fn window_is_standard(&self, id: WindowId) -> bool {
        self.window_manager
            .windows
            .get(&id)
            .map_or(false, |window| window.is_effectively_manageable())
    }

    fn send_layout_event(&mut self, event: LayoutEvent) {
        let event_clone = event.clone();
        let response = self.layout_manager.layout_engine.handle_event(event);
        self.prepare_refocus_after_layout_event(&event_clone);
        self.handle_layout_response(response, None);
        for space in self.space_manager.iter_known_spaces() {
            self.layout_manager.layout_engine.debug_tree_desc(space, "after event", false);
        }
    }

    // Returns true if the window should be raised on mouse over considering
    // active workspace membership and potential occlusion of other windows above it.
    fn should_raise_on_mouse_over(&self, wid: WindowId) -> bool {
        let Some(window) = self.window_manager.windows.get(&wid) else {
            return false;
        };

        if !window.is_effectively_manageable()
            && !self.layout_manager.layout_engine.is_window_floating(wid)
        {
            return false;
        }

        let candidate_frame = window.frame_monotonic;

        if matches!(self.menu_manager.menu_state, MenuState::Open(_)) {
            trace!(?wid, "Skipping autoraise while menu open");
            return false;
        }

        let Some(space) = self.best_space_for_window(&candidate_frame, window.window_server_id)
        else {
            return false;
        };

        if !self.layout_manager.layout_engine.is_window_in_active_workspace(space, wid) {
            trace!("Ignoring mouse over window {:?} - not in active workspace", wid);
            return false;
        }

        let Some(candidate_wsid) = window.window_server_id else {
            return true;
        };

        for child_wsid in window_server::associated_windows(candidate_wsid) {
            if let Some(&child_wid) = self.window_manager.window_ids.get(&child_wsid)
                && let Some(child_state) = self.window_manager.windows.get(&child_wid)
                && matches!(
                    child_state.ax_role.as_deref(),
                    Some("AXSheet") | Some("AXDrawer")
                )
            {
                trace!(
                    ?candidate_wsid,
                    "Skipping autoraise while child sheet/drawer exists"
                );
                return false;
            }
        }

        let order = {
            let space_id = space.get();
            crate::sys::window_server::space_window_list_for_connection(&[space_id], 0, false)
        };
        let candidate_u32 = candidate_wsid.as_u32();
        let candidate_level = window_level(candidate_u32);

        for above_u32 in order {
            if above_u32 == candidate_u32 {
                break;
            }

            let above_wsid = WindowServerId::new(above_u32);
            let Some(&above_wid) = self.window_manager.window_ids.get(&above_wsid) else {
                continue;
            };

            if !self.layout_manager.layout_engine.is_window_floating(above_wid) {
                continue;
            }

            let Some(above_state) = self.window_manager.windows.get(&above_wid) else {
                continue;
            };
            let above_frame = above_state.frame_monotonic;
            if !candidate_frame.contains_rect(above_frame) {
                continue;
            }

            let above_level = window_level(above_u32);
            if candidate_level == above_level {
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
            let Some(state) = self.window_manager.windows.get(&wid) else {
                continue;
            };
            let Some(space) =
                self.best_space_for_window(&state.frame_monotonic, state.window_server_id)
            else {
                continue;
            };
            windows_by_space.entry(space).or_default().push(wid);
        }

        for (space, wids) in windows_by_space {
            let mut manageable_windows: Vec<WindowId> = Vec::new();

            for wid in &wids {
                let assign_result = {
                    let window = self.window_manager.windows.get(wid);
                    self.layout_manager
                        .layout_engine
                        .virtual_workspace_manager_mut()
                        .assign_window_with_app_info(
                            *wid,
                            space,
                            app_info.bundle_id.as_deref(),
                            app_info.localized_name.as_deref(),
                            window.map(|w| w.title.as_str()),
                            window.and_then(|w| w.ax_role.as_deref()),
                            window.and_then(|w| w.ax_subrole.as_deref()),
                        )
                };

                match assign_result {
                    Ok(AppRuleResult::Managed(_)) => {
                        if let Some(window) = self.window_manager.windows.get_mut(wid) {
                            window.ignore_app_rule = false;
                        }
                        manageable_windows.push(*wid);
                    }
                    Ok(AppRuleResult::Unmanaged) => {
                        if let Some(window) = self.window_manager.windows.get_mut(wid) {
                            window.ignore_app_rule = true;
                        }

                        let needs_removal = {
                            let engine = &self.layout_manager.layout_engine;
                            engine
                                .virtual_workspace_manager()
                                .workspace_for_window(space, *wid)
                                .is_some()
                                || engine.is_window_floating(*wid)
                        };
                        if needs_removal {
                            self.send_layout_event(LayoutEvent::WindowRemoved(*wid));
                        }
                    }
                    Err(e) => {
                        warn!("Failed to assign window {:?} to workspace: {:?}", wid, e);
                        if let Some(window) = self.window_manager.windows.get_mut(wid) {
                            window.ignore_app_rule = false;
                        }
                        manageable_windows.push(*wid);
                    }
                }
            }

            if manageable_windows.is_empty() {
                continue;
            }

            let windows_with_titles: Vec<(
                WindowId,
                Option<String>,
                Option<String>,
                Option<String>,
            )> = manageable_windows
                .iter()
                .map(|&wid| {
                    let title_opt = self.window_manager.windows.get(&wid).map(|w| w.title.clone());
                    let ax_role =
                        self.window_manager.windows.get(&wid).and_then(|w| w.ax_role.clone());
                    let ax_subrole =
                        self.window_manager.windows.get(&wid).and_then(|w| w.ax_subrole.clone());
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

        if self.workspace_switch_manager.active_workspace_switch.is_some() {
            trace!(
                "Skipping auto workspace switch for pid {} because a workspace switch is in progress",
                pid
            );
            return;
        }

        if self.workspace_switch_manager.manual_switch_in_progress() {
            debug!(
                "Skipping auto workspace switch for pid {} because a manual switch is in progress",
                pid
            );
            return;
        }

        if let Some(active_space) = get_active_space_number()
            && space_is_fullscreen(active_space.get())
        {
            debug!(
                "Skipping auto workspace switch for pid {} because the active space is fullscreen",
                pid
            );
            return;
        }

        if let Some(wsid) = self.activation_from_unmanageable_window(pid) {
            debug!(
                ?wsid,
                "Skipping auto workspace switch for pid {} because the activated window is not manageable",
                pid
            );
            return;
        }

        let visible_spaces: HashSet<SpaceId> =
            self.space_manager.screens.iter().filter_map(|s| s.space).collect();
        let app_is_on_visible_workspace =
            self.window_manager.windows.iter().any(|(wid, window_state)| {
                if wid.pid != pid {
                    return false;
                }
                if let Some(space) = self.best_space_for_window(
                    &window_state.frame_monotonic,
                    window_state.window_server_id,
                ) {
                    if visible_spaces.contains(&space) {
                        if let Some(active_workspace) =
                            self.layout_manager.layout_engine.active_workspace(space)
                        {
                            if let Some(window_workspace) = self
                                .layout_manager
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

        if self
            .config_manager
            .config
            .settings
            .auto_focus_blacklist
            .contains(&bundle_id_str)
        {
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
            .main_window()
            .filter(|wid| wid.pid == pid && self.window_is_standard(*wid))
            .or_else(|| {
                self.window_manager
                    .windows
                    .keys()
                    .find(|wid| wid.pid == pid && self.window_is_standard(**wid))
                    .copied()
            });

        let Some(app_window_id) = app_window else {
            return;
        };

        let Some(window_state) = self.window_manager.windows.get(&app_window_id) else {
            return;
        };
        let Some(window_space) = self
            .best_space_for_window(&window_state.frame_monotonic, window_state.window_server_id)
        else {
            return;
        };

        let workspace_manager = self.layout_manager.layout_engine.virtual_workspace_manager();
        let Some(window_workspace) =
            workspace_manager.workspace_for_window(window_space, app_window_id)
        else {
            return;
        };

        let Some(current_workspace) =
            self.layout_manager.layout_engine.active_workspace(window_space)
        else {
            return;
        };

        if window_workspace != current_workspace {
            let workspaces = self
                .layout_manager
                .layout_engine
                .virtual_workspace_manager_mut()
                .list_workspaces(window_space);
            if let Some((workspace_index, _)) =
                workspaces.iter().enumerate().find(|(_, (ws_id, _))| *ws_id == window_workspace)
            {
                debug!(
                    "Auto-switching to workspace {} for activated app (pid: {})",
                    workspace_index, pid
                );

                self.store_current_floating_positions(window_space);
                self.workspace_switch_manager
                    .start_workspace_switch(WorkspaceSwitchOrigin::Auto);

                let response = self.layout_manager.layout_engine.handle_virtual_workspace_command(
                    window_space,
                    &layout::LayoutCommand::SwitchToWorkspace(workspace_index),
                );
                self.handle_layout_response(response, Some(window_space));
            }
        }
    }

    fn handle_layout_response(
        &mut self,
        response: layout::EventResponse,
        workspace_switch_space: Option<SpaceId>,
    ) {
        if self.is_in_drag() {
            self.workspace_switch_manager.mark_workspace_switch_inactive();
            return;
        }

        let mut pending_refocus_space =
            match std::mem::replace(&mut self.refocus_manager.refocus_state, RefocusState::None) {
                RefocusState::Pending(space) => Some(space),
                RefocusState::None => None,
            };
        let layout::EventResponse {
            raise_windows,
            mut focus_window,
        } = response;
        let original_focus = focus_window;

        let mut handled_without_raise = false;

        if raise_windows.is_empty() && focus_window.is_none() {
            if matches!(
                self.workspace_switch_manager.workspace_switch_state,
                WorkspaceSwitchState::Active
            ) && !self.is_in_drag()
            {
                if let Some(wid) = self.window_id_under_cursor() {
                    // Only focus if it's different from the currently focused window to prevent duplicate focus
                    if self.main_window() != Some(wid) {
                        focus_window = Some(wid);
                    }
                } else if self.focus_untracked_window_under_cursor() {
                    handled_without_raise = true;
                } else if self.config_manager.config.settings.mouse_follows_focus {
                    let skip_center_warp = workspace_switch_space
                        .map(|space| {
                            self.layout_manager
                                .layout_engine
                                .windows_in_active_workspace(space)
                                .is_empty()
                        })
                        .unwrap_or(false);

                    if !skip_center_warp {
                        let mut center_space = workspace_switch_space;
                        if center_space.is_none() {
                            center_space = self.workspace_command_space();
                        }

                        if let Some(space) = center_space {
                            if let Some(screen) = self.space_manager.screen_by_space(space) {
                                let center = screen.frame.mid();
                                if let Some(event_tap_tx) =
                                    self.communication_manager.event_tap_tx.as_ref()
                                {
                                    event_tap_tx
                                        .send(crate::actor::event_tap::Request::Warp(center));
                                    handled_without_raise = true;
                                }
                            }
                        }
                    }
                }
            } else if let Some(space) = pending_refocus_space.take() {
                if let Some(wid) = self.last_focused_window_in_space(space) {
                    focus_window = Some(wid);
                } else if !self.is_in_drag() {
                    if let Some(wid) = self.window_id_under_cursor() {
                        focus_window = Some(wid);
                    } else if self.focus_untracked_window_under_cursor() {
                        handled_without_raise = true;
                    } else if self.config_manager.config.settings.mouse_follows_focus {
                        if let Some(screen) = self.space_manager.screen_by_space(space) {
                            let center = screen.frame.mid();
                            if let Some(event_tap_tx) =
                                self.communication_manager.event_tap_tx.as_ref()
                            {
                                event_tap_tx.send(crate::actor::event_tap::Request::Warp(center));
                                handled_without_raise = true;
                            }
                        }
                    }
                }
            }
        }

        let require_visible_focus = matches!(
            self.workspace_switch_manager.workspace_switch_state,
            WorkspaceSwitchState::Inactive
        );

        if let Some(wid) = focus_window {
            if let Some(state) = self.window_manager.windows.get(&wid) {
                if let Some(wsid) = state.window_server_id {
                    if self.space_manager.changing_screens.contains(&wsid)
                        || (require_visible_focus
                            && !self.window_manager.visible_windows.contains(&wsid))
                    {
                        focus_window = None;
                    } else if let Some(space) =
                        self.best_space_for_window(&state.frame_monotonic, state.window_server_id)
                    {
                        let active_spaces: std::collections::HashSet<_> =
                            self.space_manager.screens.iter().filter_map(|s| s.space).collect();
                        if !active_spaces.contains(&space) {
                            focus_window = None;
                        }
                    } else {
                        focus_window = None;
                    }
                }
            }
        }

        if handled_without_raise && raise_windows.is_empty() && focus_window.is_none() {
            self.workspace_switch_manager.mark_workspace_switch_inactive();
            return;
        }

        if let Some(space) = pending_refocus_space {
            // Preserve the pending refocus request if it was not consumed above.
            if matches!(self.refocus_manager.refocus_state, RefocusState::None) {
                self.refocus_manager.refocus_state = RefocusState::Pending(space);
            }
        }

        if raise_windows.is_empty()
            && focus_window.is_none()
            && matches!(
                self.workspace_switch_manager.workspace_switch_state,
                WorkspaceSwitchState::Inactive
            )
        {
            return;
        }

        let mut app_handles = HashMap::default();
        for &wid in raise_windows.iter() {
            if let Some(app) = self.app_manager.apps.get(&wid.pid) {
                app_handles.insert(wid.pid, app.handle.clone());
            }
        }

        if let Some(wid) = original_focus {
            if let Some(app) = self.app_manager.apps.get(&wid.pid) {
                app_handles.insert(wid.pid, app.handle.clone());
            }
        }

        let mut windows_by_app_and_screen = HashMap::default();
        for &wid in &raise_windows {
            let Some(window) = self.window_manager.windows.get(&wid) else {
                continue;
            };
            windows_by_app_and_screen
                .entry((
                    wid.pid,
                    self.best_space_for_window(&window.frame_monotonic, window.window_server_id),
                ))
                .or_insert(vec![])
                .push(wid);
        }

        let focus_window_with_warp = focus_window.map(|wid| {
            let warp = match self.config_manager.config.settings.mouse_follows_focus {
                true => {
                    if self.workspace_switch_manager.workspace_switch_state
                        == WorkspaceSwitchState::Active
                    {
                        // During workspace switches, defer mouse warping until after layout completes
                        self.workspace_switch_manager.pending_workspace_mouse_warp = Some(wid);
                        None
                    } else {
                        self.window_manager.windows.get(&wid).and_then(|w| {
                            let window_center = w.frame_monotonic.mid();
                            // Only warp if the window center is actually on a screen
                            if self
                                .space_manager
                                .screens
                                .iter()
                                .any(|s| s.frame.contains(window_center))
                            {
                                Some(window_center)
                            } else {
                                None
                            }
                        })
                    }
                }
                false => None,
            };
            (wid, warp)
        });

        let msg = raise_manager::Event::RaiseRequest(RaiseRequest {
            raise_windows: windows_by_app_and_screen.into_values().collect(),
            focus_window: focus_window_with_warp,
            app_handles,
        });

        if let Err(e) = self.communication_manager.raise_manager_tx.try_send(msg) {
            warn!("Failed to send raise request to raise manager: {}", e);
        }
    }

    fn maybe_swap_on_drag(&mut self, wid: WindowId, new_frame: CGRect) {
        if !self.is_in_drag() {
            trace!(?wid, "Skipping swap: not in drag (mouse up received)");
            return;
        }

        let server_id = {
            let Some(window) = self.window_manager.windows.get(&wid) else {
                return;
            };

            if window
                .window_server_id
                .is_some_and(|wsid| self.space_manager.changing_screens.contains(&wsid))
            {
                trace!(?wid, "Skipping swap: window is changing screens");
                return;
            }

            window.window_server_id
        };

        let Some(space) = (if self.is_in_drag() {
            self.get_active_drag_session()
                .and_then(|session| session.settled_space)
                .or_else(|| self.best_space_for_window(&new_frame, server_id))
        } else {
            self.best_space_for_window(&new_frame, server_id)
        }) else {
            return;
        };

        let origin_space_hint = self
            .get_active_drag_session()
            .and_then(|session| session.origin_space)
            .or_else(|| {
                self.drag_manager
                    .origin_frame()
                    .and_then(|frame| self.best_space_for_window(&frame, server_id))
            });

        if let Some(origin_space) = origin_space_hint {
            if origin_space != space {
                if let Some((pending_wid, pending_target)) = self.get_pending_drag_swap() {
                    if pending_wid == wid {
                        trace!(
                            ?wid,
                            ?pending_target,
                            ?origin_space,
                            ?space,
                            "Clearing pending drag swap; dragged window entered new space"
                        );
                        self.drag_manager.drag_state = DragState::Inactive;
                    }
                }
                trace!(
                    ?wid,
                    ?origin_space,
                    ?space,
                    "Resetting drag swap tracking after space change"
                );
                self.drag_manager.drag_swap_manager.reset();
                return;
            }
        }

        if !self.layout_manager.layout_engine.is_window_in_active_workspace(space, wid) {
            return;
        }

        let mut candidates: Vec<(WindowId, CGRect)> = Vec::new();
        for (&other_wid, other_state) in &self.window_manager.windows {
            if other_wid == wid {
                continue;
            }

            let Some(other_space) = self
                .best_space_for_window(&other_state.frame_monotonic, other_state.window_server_id)
            else {
                continue;
            };
            if other_space != space
                || !self
                    .layout_manager
                    .layout_engine
                    .is_window_in_active_workspace(space, other_wid)
                || self.layout_manager.layout_engine.is_window_floating(other_wid)
            {
                continue;
            }

            candidates.push((other_wid, other_state.frame_monotonic));
        }

        let previous_pending = self.get_pending_drag_swap();
        let new_candidate =
            self.drag_manager.drag_swap_manager.on_frame_change(wid, new_frame, &candidates);
        let active_target = self.drag_manager.drag_swap_manager.last_target();

        if let Some(target_wid) = active_target {
            if new_candidate.is_some() || previous_pending != Some((wid, target_wid)) {
                trace!(
                    ?wid,
                    ?target_wid,
                    "Detected swap candidate; deferring until MouseUp"
                );
            }

            if let Some(session) = self.take_active_drag_session() {
                self.drag_manager.drag_state =
                    DragState::PendingSwap { session, target: target_wid };
            } else {
                trace!(
                    ?wid,
                    ?target_wid,
                    "Skipping pending swap; no active drag session"
                );
                self.drag_manager.drag_state = DragState::Inactive;
                self.drag_manager.skip_layout_for_window = None;
                return;
            }

            self.drag_manager.skip_layout_for_window = Some(wid);
        } else {
            if let Some((pending_wid, pending_target)) = previous_pending {
                if pending_wid == wid {
                    trace!(
                        ?wid,
                        ?pending_target,
                        "Clearing pending drag swap; overlap ended before MouseUp"
                    );
                    if let Some(session) = self.take_active_drag_session() {
                        self.drag_manager.drag_state = DragState::Active { session };
                    } else {
                        self.drag_manager.drag_state = DragState::Inactive;
                    }
                }
            }

            if self.drag_manager.skip_layout_for_window == Some(wid) {
                self.drag_manager.skip_layout_for_window = None;
            }
        }
        // wait for mouse::up before doing *anything*
    }

    fn window_id_under_cursor(&self) -> Option<WindowId> {
        let wsid = window_server::window_under_cursor()?;
        self.window_manager.window_ids.get(&wsid).copied()
    }

    fn activation_from_unmanageable_window(&self, pid: pid_t) -> Option<WindowServerId> {
        let wsid = window_server::window_under_cursor()?;
        let wid = *self.window_manager.window_ids.get(&wsid)?;
        if wid.pid != pid {
            return None;
        }
        let window = self.window_manager.windows.get(&wid)?;
        if window.is_effectively_manageable() {
            return None;
        }
        Some(wsid)
    }

    fn focus_untracked_window_under_cursor(&mut self) -> bool {
        let Some(wsid) = window_server::window_under_cursor() else {
            return false;
        };
        if self.window_manager.window_ids.contains_key(&wsid) {
            return false;
        }

        let window_info = self
            .window_server_info_manager
            .window_server_info
            .get(&wsid)
            .copied()
            .or_else(|| window_server::get_window(wsid));

        let Some(info) = window_info else { return false };
        window_server::make_key_window(info.pid, wsid).is_ok()
    }

    fn last_focused_window_in_space(&self, space: SpaceId) -> Option<WindowId> {
        let active_workspace = self.layout_manager.layout_engine.active_workspace(space)?;
        let wid = self
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .last_focused_window(space, active_workspace)?;
        let window = self.window_manager.windows.get(&wid)?;

        if let Some(actual_space) =
            self.best_space_for_window(&window.frame_monotonic, window.window_server_id)
        {
            if actual_space != space {
                return None;
            }
        } else {
            return None;
        }
        if let Some(wsid) = window.window_server_id {
            if self.space_manager.changing_screens.contains(&wsid) {
                return None;
            }
            if !self.window_manager.visible_windows.contains(&wsid) {
                return None;
            }
        }
        Some(wid)
    }

    fn request_refocus_if_hidden(&mut self, space: SpaceId, window_id: WindowId) {
        let Some(active_workspace) = self.layout_manager.layout_engine.active_workspace(space)
        else {
            return;
        };
        let Some(window_workspace) = self
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(space, window_id)
        else {
            return;
        };

        if window_workspace != active_workspace {
            self.refocus_manager.refocus_state = RefocusState::Pending(space);
        }
    }

    fn prepare_refocus_after_layout_event(&mut self, event: &LayoutEvent) {
        match event {
            LayoutEvent::WindowAdded(space, wid) => {
                self.request_refocus_if_hidden(*space, *wid);
            }
            LayoutEvent::WindowsOnScreenUpdated(space, _, windows, _) => {
                let Some(active_workspace) =
                    self.layout_manager.layout_engine.active_workspace(*space)
                else {
                    return;
                };
                let manager = self.layout_manager.layout_engine.virtual_workspace_manager();
                let hidden_exists = windows.iter().any(|(wid, _, _, _)| {
                    manager
                        .workspace_for_window(*space, *wid)
                        .map_or(false, |workspace_id| workspace_id != active_workspace)
                });
                if hidden_exists {
                    self.refocus_manager.refocus_state = RefocusState::Pending(*space);
                }
            }
            _ => {}
        }
    }

    #[instrument(skip(self))]
    fn raise_window(&mut self, wid: WindowId, quiet: Quiet, warp: Option<CGPoint>) {
        let mut app_handles = HashMap::default();
        if let Some(app) = self.app_manager.apps.get(&wid.pid) {
            app_handles.insert(wid.pid, app.handle.clone());
        }
        _ = self
            .communication_manager
            .raise_manager_tx
            .send(raise_manager::Event::RaiseRequest(RaiseRequest {
                raise_windows: vec![vec![wid]],
                focus_window: Some((wid, warp)),
                app_handles,
            }));
    }

    fn set_focus_follows_mouse_enabled(&self, enabled: bool) {
        if let Some(event_tap_tx) = self.communication_manager.event_tap_tx.as_ref() {
            event_tap_tx.send(event_tap::Request::SetFocusFollowsMouseEnabled(enabled));
        }
    }

    fn update_focus_follows_mouse_state(&self) {
        let should_enable = matches!(self.menu_manager.menu_state, MenuState::Closed)
            && !self.is_mission_control_active();
        self.set_focus_follows_mouse_enabled(should_enable);
    }

    fn set_mission_control_active(&mut self, active: bool) {
        let new_state = if active {
            MissionControlState::Active
        } else {
            MissionControlState::Inactive
        };
        if matches!(
            self.mission_control_manager.mission_control_state,
            MissionControlState::Active
        ) == active
        {
            return;
        }
        self.mission_control_manager.mission_control_state = new_state;
        self.update_focus_follows_mouse_state();
    }

    fn refresh_windows_after_mission_control(&mut self) {
        debug!("Refreshing window state after Mission Control");
        let ws_info = window_server::get_visible_windows_with_layer(None);
        self.update_partial_window_server_info(ws_info);
        self.mission_control_manager.pending_mission_control_refresh.clear();
        self.force_refresh_all_windows();
        self.check_for_new_windows();
        self.update_layout(false, false).unwrap_or_else(|e| {
            warn!("Layout update failed: {}", e);
            false
        });
        self.maybe_send_menu_update();
    }

    fn force_refresh_all_windows(&mut self) {
        for (&pid, app) in &self.app_manager.apps {
            if app.handle.send(Request::GetVisibleWindows { force_refresh: true }).is_ok() {
                self.mission_control_manager.pending_mission_control_refresh.insert(pid);
            }
        }
    }

    fn request_close_window(&mut self, wid: WindowId) {
        if let Some(app) = self.app_manager.apps.get(&wid.pid) {
            if let Err(err) = app.handle.send(Request::CloseWindow(wid)) {
                warn!(?wid, "Failed to send close window request: {}", err);
            }
        }
    }

    fn main_window(&self) -> Option<WindowId> {
        self.main_window_tracker_manager.main_window_tracker.main_window()
    }

    fn main_window_space(&self) -> Option<SpaceId> {
        // TODO: Optimize this with a cache or something.
        let wid = self.main_window()?;
        self.best_space_for_window_id(wid)
    }

    fn workspace_command_space(&self) -> Option<SpaceId> {
        if let Some(space) = self.space_for_cursor_screen() {
            return Some(space);
        }

        if let Some(space) = self.main_window_space() {
            return Some(space);
        }

        if let Some(space) = get_active_space_number() {
            return Some(space);
        }

        self.space_manager.first_known_space()
    }

    fn space_for_cursor_screen(&self) -> Option<SpaceId> {
        current_cursor_location().ok().and_then(|point| self.space_for_point(point))
    }

    fn space_for_point(&self, point: CGPoint) -> Option<SpaceId> {
        self.screen_for_point(point)
            .or_else(|| self.closest_screen_to_point(point))
            .and_then(|screen| self.space_for_screen(screen))
    }

    fn screen_for_point(&self, point: CGPoint) -> Option<&Screen> {
        self.space_manager.screens.iter().find(|screen| screen.frame.contains(point))
    }

    fn closest_screen_to_point(&self, point: CGPoint) -> Option<&Screen> {
        self.space_manager.screens.iter().min_by(|a, b| {
            let da = Self::rectangle_distance_sq(a.frame, point);
            let db = Self::rectangle_distance_sq(b.frame, point);
            da.total_cmp(&db)
        })
    }

    fn space_for_screen(&self, screen: &Screen) -> Option<SpaceId> {
        screen
            .space
            .or_else(|| self.space_manager.screen_space_by_id.get(&screen.screen_id).copied())
    }

    fn rectangle_distance_sq(frame: CGRect, point: CGPoint) -> f64 {
        let min_x = frame.origin.x;
        let max_x = frame.origin.x + frame.size.width;
        let min_y = frame.origin.y;
        let max_y = frame.origin.y + frame.size.height;

        let dx = if point.x < min_x {
            min_x - point.x
        } else if point.x > max_x {
            point.x - max_x
        } else {
            0.0
        };

        let dy = if point.y < min_y {
            min_y - point.y
        } else if point.y > max_y {
            point.y - max_y
        } else {
            0.0
        };

        dx * dx + dy * dy
    }

    fn current_screen_center(&self) -> Option<CGPoint> {
        if let Ok(point) = current_cursor_location() {
            if let Some(screen) =
                self.space_manager.screens.iter().find(|screen| screen.frame.contains(point))
            {
                return Some(screen.frame.mid());
            }
        }

        if let Some(space) = self.main_window_space() {
            if let Some(screen) = self.space_manager.screen_by_space(space) {
                return Some(screen.frame.mid());
            }
        }

        if let Some(space) = get_active_space_number() {
            if let Some(screen) = self.space_manager.screen_by_space(space) {
                return Some(screen.frame.mid());
            }
        }

        self.space_manager.screens.first().map(|screen| screen.frame.mid())
    }

    fn screen_for_direction_from_point(
        &self,
        origin: CGPoint,
        direction: Direction,
    ) -> Option<&Screen> {
        fn interval_gap(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> f64 {
            if a_max < b_min {
                b_min - a_max
            } else if b_max < a_min {
                a_min - b_max
            } else {
                0.0
            }
        }

        let mut best: Option<(f64, f64, &Screen)> = None;

        for screen in &self.space_manager.screens {
            let frame = screen.frame;

            if frame.contains(origin) {
                continue;
            }

            let min = frame.min();
            let max = frame.max();

            let (primary_dist, orth_gap) = match direction {
                Direction::Left => {
                    if max.x > origin.x {
                        continue;
                    }
                    (origin.x - max.x, interval_gap(min.y, max.y, origin.y, origin.y))
                }
                Direction::Right => {
                    if min.x < origin.x {
                        continue;
                    }
                    (min.x - origin.x, interval_gap(min.y, max.y, origin.y, origin.y))
                }
                Direction::Up => {
                    // Smaller y means visually "up".
                    if max.y > origin.y {
                        continue;
                    }
                    (origin.y - max.y, interval_gap(min.x, max.x, origin.x, origin.x))
                }
                Direction::Down => {
                    if min.y < origin.y {
                        continue;
                    }
                    (min.y - origin.y, interval_gap(min.x, max.x, origin.x, origin.x))
                }
            };

            let should_replace = best.as_ref().map_or(true, |(best_primary, best_orth, _)| {
                primary_dist < *best_primary
                    || (primary_dist == *best_primary && orth_gap < *best_orth)
            });

            if should_replace {
                best = Some((primary_dist, orth_gap, screen));
            }
        }

        best.map(|(_, _, screen)| screen)
    }

    fn screen_for_selector(
        &self,
        selector: &DisplaySelector,
        origin_override: Option<CGPoint>,
    ) -> Option<&Screen> {
        match selector {
            DisplaySelector::Direction(direction) => {
                let origin = origin_override.or_else(|| self.current_screen_center())?;
                self.screen_for_direction_from_point(origin, *direction)
            }
            DisplaySelector::Index(index) => self.space_manager.screens.get(*index),
            DisplaySelector::Uuid(uuid) => {
                self.space_manager.screens.iter().find(|screen| screen.display_uuid == *uuid)
            }
        }
    }

    fn store_current_floating_positions(&mut self, space: SpaceId) {
        let floating_windows_in_workspace = self
            .layout_manager
            .layout_engine
            .windows_in_active_workspace(space)
            .into_iter()
            .filter(|&wid| self.layout_manager.layout_engine.is_window_floating(wid))
            .filter_map(|wid| {
                self.window_manager
                    .windows
                    .get(&wid)
                    .map(|window_state| (wid, window_state.frame_monotonic))
            })
            .collect::<Vec<_>>();

        if !floating_windows_in_workspace.is_empty() {
            self.layout_manager
                .layout_engine
                .store_floating_window_positions(space, &floating_windows_in_workspace);
        }
    }

    #[instrument(skip(self), fields())]
    pub fn update_layout(
        &mut self,
        is_resize: bool,
        is_workspace_switch: bool,
    ) -> Result<bool, error::ReactorError> {
        LayoutManager::update_layout(self, is_resize, is_workspace_switch)
    }
}
