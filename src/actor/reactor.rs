//! The Reactor's job is to maintain coherence between the system and model state.
//!
//! It takes events from the rest of the system and builds a coherent picture of
//! what is going on. It shares this with the layout actor, and reacts to layout
//! changes by sending requests out to the other actors in the system.

mod animation;
mod changes;
mod claims;
mod events;
mod gesture;
pub(crate) use crate::layout_engine::WorkspaceDropRequest as OverviewDrop;
mod main_window;
mod managers;
mod native_tabs;
mod query;
mod replay;
pub mod transaction_manager;
mod transient_focus;
mod utils;
mod window_lifecycle;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use animation::AnimationSender;
use changes::{Changes, Presentation, SpaceChange};
use events::{
    drag as interaction_workflow, space as topology_workflow, system as system_workflow,
    window as window_workflow,
};
use main_window::MainWindowTracker;
use managers::LayoutManager;
use objc2_core_foundation::{CGPoint, CGRect};
pub use replay::{Record, replay};
use rift_protocol::DirectionalDistance;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use tokio::sync::oneshot;
use tracing::{debug, instrument, trace, warn};
use transaction_manager::TransactionId;

use super::input;
use crate::actor::app::{
    AppInfo, AppThreadHandle, Quiet, Request, WindowId, WindowInfo, WindowInventoryToken, pid_t,
};
use crate::actor::raise_manager::{self, RaiseManager, RaiseRequest};
use crate::actor::reactor::events::window_discovery;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::WmEvent;
use crate::actor::{self, menu_bar, stack_line};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::Config;
use crate::layout_engine::{self as layout, Direction, LayoutEngine, LayoutEvent};
use crate::model::RiftState;
use crate::model::app_rules::{AppRuleFrame, AppRulePlacement, AppRuleResize};
use crate::model::broadcast::{
    BroadcastEvent, BroadcastSender, protocol_window_id, protocol_workspace_id,
};
use crate::model::space_activation::{SpaceActivationConfig, SpaceActivationPolicy};
use crate::model::tx_store::WindowTxStore;
use crate::model::window_store::NativeFullscreenTransition;
use crate::sys::event::MouseState;
use crate::sys::executor::Executor;
use crate::sys::geometry::{CGRectDef, CGRectExt, SameAs};
pub use crate::sys::screen::ScreenInfo;
use crate::sys::screen::{SpaceId, order_visible_spaces_by_position};
use crate::sys::window_server::{
    self, WindowServerId, WindowServerInfo, window_level, window_sub_level,
};

pub type Sender = actor::Sender<Event>;
type Receiver = actor::Receiver<Event>;
pub use query::ReactorQueryHandle;

pub(crate) use crate::model::reactor::{AppState, WindowState};
pub use crate::model::reactor::{
    Command, DisplaySelector, MenuState, MissionControlState, ReactorCommand, RefocusState,
    Requested, StaleCleanupState, WorkspaceSwitchOrigin,
};

#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct MouseFocusPublisher(Arc<MouseFocusState>);

#[derive(Debug, Default)]
struct MouseFocusState {
    latest: parking_lot::Mutex<Option<CGPoint>>,
}

impl MouseFocusPublisher {
    pub(crate) fn publish(
        &self,
        sender: &Sender,
        point: CGPoint,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<(tracing::Span, Event)>> {
        let mut latest = self.0.latest.lock();
        let needs_wake = latest.is_none();
        *latest = Some(point);
        if !needs_wake {
            return Ok(());
        }
        sender.try_send(Event::MouseFocusPending(self.clone())).inspect_err(|_| {
            *latest = None;
        })
    }

    fn take_latest(&self) -> Option<CGPoint> { self.0.latest.lock().take() }
}

#[cfg(test)]
mod mouse_focus_publisher_tests {
    use super::*;

    #[test]
    fn replaces_pending_focus_candidate_and_queues_one_wake() {
        let (sender, mut receiver) = actor::channel();
        let publisher = MouseFocusPublisher::default();
        for x in [123.0, 456.0, 789.0] {
            publisher.publish(&sender, CGPoint::new(x, 10.0)).unwrap();
        }

        let (_, Event::MouseFocusPending(wake)) = receiver.try_recv().unwrap() else {
            panic!("expected coalesced mouse-focus wake");
        };
        assert!(receiver.try_recv().is_err());
        assert_eq!(wake.take_latest(), Some(CGPoint::new(789.0, 10.0)));
        // The same position must be reconsidered after inventory/focus changes.
        publisher.publish(&sender, CGPoint::new(789.0, 10.0)).unwrap();
        let (_, Event::MouseFocusPending(wake)) = receiver.try_recv().unwrap() else {
            panic!("expected another wake for a stationary hover");
        };
        assert_eq!(wake.take_latest(), Some(CGPoint::new(789.0, 10.0)));
    }
}

#[derive(Clone)]
pub struct ReactorHandle {
    sender: Sender,
    queries: ReactorQueryHandle,
}

impl ReactorHandle {
    pub fn new(sender: Sender, queries: ReactorQueryHandle) -> Self { Self { sender, queries } }

    pub fn sender(&self) -> Sender { self.sender.clone() }

    pub fn send(&self, event: Event) { self.sender.send(event) }

    pub fn try_send(
        &self,
        event: Event,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<(tracing::Span, Event)>> {
        self.sender.try_send(event)
    }
}

impl std::ops::Deref for ReactorHandle {
    type Target = ReactorQueryHandle;

    fn deref(&self) -> &Self::Target { &self.queries }
}

use crate::model::server::RuntimeWindowData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceEventKind {
    User,
    Fullscreen,
}

/// Destination geometry is prepared by the reactor while the overview covers the desktop.
#[derive(Debug)]
pub struct OverviewTarget {
    pub(crate) workspace: crate::model::VirtualWorkspaceId,
    pub(crate) window: Option<WindowId>,
    pub(crate) stamp: Instant,
    pub(crate) frames: Vec<(WindowId, CGRect)>,
}

#[serde_as]
#[derive(Serialize, Deserialize, Debug)]
pub enum Event {
    #[serde(skip)]
    CameraFinished,
    #[serde(skip)]
    OverviewSelectWorkspace {
        display: String,
        workspace: crate::model::VirtualWorkspaceId,
        window: Option<WindowId>,
        transition: Option<Instant>,
    },
    #[serde(skip)]
    OverviewDrop {
        intent: OverviewDrop,
        reply: std::sync::mpsc::SyncSender<bool>,
    },
    #[serde(skip)]
    SpaceStateChanged(ForwardedSpaceState),
    #[serde(skip)]
    ActiveDisplayChanged {
        menu_bar_space: Option<SpaceId>,
        command_space: Option<SpaceId>,
    },
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
    #[serde(skip)]
    AppActorExited(pid_t, AppThreadHandle),
    ApplicationActivated(pid_t, Quiet),
    ApplicationDeactivated(pid_t),
    ApplicationGloballyActivated(pid_t),
    ApplicationGloballyDeactivated(pid_t),
    ApplicationMainWindowChanged(pid_t, Option<WindowId>, Quiet),
    /// Authoritative focus resolved from WindowServer's key-focus process and
    /// the z-ordered windows on the active native space.
    #[serde(skip)]
    WindowServerFocusChanged(WindowId, SpaceId),

    WindowsDiscovered {
        pid: pid_t,
        token: WindowInventoryToken,
        successful: bool,
        new: Vec<(WindowId, WindowInfo)>,
        known_visible: Vec<WindowId>,
    },
    #[serde(skip)]
    WindowInventoryRefreshRequested(pid_t),
    #[serde(skip)]
    RaiseTargetsMissing {
        windows: Vec<WindowId>,
        sequence_id: u64,
    },
    WindowCreated(
        WindowId,
        WindowInfo,
        Option<WindowServerInfo>,
        Option<MouseState>,
    ),
    WindowDestroyed(WindowId),
    /// this event is only for the sls windowclosed event that provides a wsid
    #[serde(skip)]
    WindowClosed(WindowServerId),
    #[serde(skip)]
    WindowServerHidden(WindowServerId),
    #[serde(skip)]
    WindowServerUnhidden(WindowServerId),
    /// The AXUIElement became invalid, but that is not proof that its native
    /// WindowServer window was destroyed. This commonly happens before macOS
    /// publishes sleep/session lifecycle notifications.
    WindowInvalidated(WindowId, WindowInvalidationSource),
    #[serde(skip)]
    WindowServerDestroyed(
        crate::sys::window_server::WindowServerId,
        SpaceId,
        SpaceEventKind,
    ),
    #[serde(skip)]
    WindowServerAppeared(
        crate::sys::window_server::WindowServerId,
        SpaceId,
        SpaceEventKind,
    ),
    WindowMinimized(WindowId),
    WindowDeminiaturized(WindowId),
    WindowFrameChanged(
        WindowId,
        #[serde(with = "CGRectDef")] CGRect,
        Option<TransactionId>,
        Requested,
        Option<MouseState>,
    ),
    WindowFrameWriteFailed(WindowId, TransactionId),
    WindowTitleChanged(WindowId, String),
    MenuOpened(pid_t),
    MenuClosed(pid_t),

    /// A mouse button was released, allowing an owned drag to finish and arrange.
    MouseUp(crate::actor::drag::MouseButton),
    /// Keyboard input not generated by Rift, observed by the input event tap.
    KeyboardInput,
    /// A hover resolved by the reactor from the latest input position.
    MouseMoved(WindowServerId),
    /// Coalesced wake for the latest pointer position from the input thread.
    #[serde(skip)]
    MouseFocusPending(MouseFocusPublisher),
    /// Coalesced wake for native/modifier drag pointer motion.
    #[serde(skip)]
    DragMotionPending(crate::actor::drag::DragMotionPublisher),
    #[serde(skip)]
    Gesture(crate::actor::gesture::Lifecycle),
    #[serde(skip)]
    DragMotion(crate::actor::drag::DragMotion),
    #[serde(skip)]
    ModifierMouseDown {
        button: crate::actor::drag::MouseButton,
        point: CGPoint,
        action: crate::common::config::MouseAction,
    },
    #[serde(skip)]
    DragCancel,
    /// Forwarded by the spaces actor after wake has been observed.
    ///
    /// The spaces actor is the authority for sleep/lock/display lifecycle.
    /// The reactor resubscribes notifications and suppresses synthetic activation;
    /// topology authority arrives separately in revisioned observations.
    SystemWoke,
    #[serde(skip)]
    SessionDidBecomeActive,

    #[serde(skip)]
    TopologyInvalidated(u64),

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

    #[serde(skip)]
    Query(query::QueryRequest),

    #[serde(skip)]
    InstallIpc(crate::ipc::InstallRequest),
    #[serde(skip)]
    ReleaseClaims(crate::ipc::ReleaseClaims),

    BindingModeChanged {
        mode: String,
    },

    Command(Command),

    #[serde(skip)]
    RegisterSenders {
        wm: crate::actor::wm_controller::Sender,
        spaces: crate::actor::spaces::Sender,
        topology_guard: crate::actor::spaces::TopologyGuard,
    },

    #[serde(skip)]
    ConfigUpdated(Config),
}

/// The AX-side observation that caused an app actor to discard its handle.
/// None of these observations prove that the WindowServer window is gone.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WindowInvalidationSource {
    AxDestroyedNotification,
    InvalidUiElement,
    StaleAxElement,
}

pub struct Reactor {
    pub config: Config,
    pub one_space: bool,
    pub(crate) binding_mode: String,
    app_manager: managers::AppManager,
    layout_manager: managers::LayoutManager,
    pub(crate) state: RiftState,
    space_state: ForwardedSpaceState,
    // Presentation geometry survives empty/dormant topology samples; it is not ownership evidence.
    last_screen_frames: HashMap<SpaceId, CGRect>,
    space_activation_policy: SpaceActivationPolicy,
    spaces_tx: Option<crate::actor::spaces::Sender>,
    topology_guard: crate::actor::spaces::TopologyGuard,
    main_window_tracker: MainWindowTracker,
    pending_mouse_focus: Option<(WindowId, Instant)>,
    overview_focus: Option<(SpaceId, WindowId, Instant)>,
    mouse_inventory_hit: Option<WindowServerId>,
    drag_manager: managers::DragManager,
    workspace_switch_manager: managers::WorkspaceSwitchManager,
    recording_manager: managers::RecordingManager,
    communication_manager: managers::CommunicationManager,
    notification_manager: managers::NotificationManager,
    transaction_manager: transaction_manager::TransactionManager,
    menu_manager: managers::MenuManager,
    mission_control_manager: managers::MissionControlManager,
    window_inventory_manager: managers::WindowInventoryManager,
    refocus_manager: managers::RefocusManager,
    suppress_auto_workspace_switch_until_input: bool,
    pending_space_change_manager: managers::PendingSpaceChangeManager,
    transient_focus: transient_focus::TransientFocusGuard,
    mouse_raise_token: tokio_util::sync::CancellationToken,
    active_spaces: HashSet<SpaceId>,
    startup_ready: Option<oneshot::Sender<()>>,
    pub animation_tx: Option<AnimationSender>,
    viewport_gesture: Option<gesture::ViewportSession>,
    presentations: HashMap<SpaceId, animation::ViewportHandle>,
    #[cfg(test)]
    layout_calculations: Vec<SpaceId>,
    #[cfg(test)]
    native_focus_for_removal: Option<WindowId>,
    #[cfg(test)]
    native_tab_successor: Option<(WindowId, CGRect)>,
}

impl Reactor {
    pub fn spawn(
        config: Config,
        layout_engine: LayoutEngine,
        record: Record,
        input_tx: input::Sender,
        broadcast_tx: BroadcastSender,
        menu_tx: menu_bar::Sender,
        stack_line_tx: stack_line::Sender,
        window_notify: Option<(crate::actor::window_notify::Sender, WindowTxStore)>,
        one_space: bool,
        native_motion_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> (ReactorHandle, oneshot::Receiver<()>) {
        let (events_tx, events) = actor::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let events_tx_clone = events_tx.clone();
        let mut reactor = Reactor::new(
            config,
            layout_engine,
            record,
            broadcast_tx,
            window_notify,
            one_space,
        );
        reactor.startup_ready = Some(ready_tx);
        reactor.drag_manager.native_motion_active = native_motion_active;
        reactor.communication_manager.input_tx = Some(input_tx);
        reactor.menu_manager.menu_tx = Some(menu_tx);
        reactor.communication_manager.stack_line_tx = Some(stack_line_tx);
        reactor.communication_manager.events_tx = Some(events_tx_clone.clone());
        let query_handle = ReactorQueryHandle::new(events_tx_clone.clone());
        thread::Builder::new()
            .name("reactor".to_string())
            .spawn(move || {
                Executor::run(Reactor::run(reactor, events, events_tx_clone));
            })
            .unwrap();
        (ReactorHandle::new(events_tx, query_handle), ready_rx)
    }

    pub fn new(
        config: Config,
        layout_engine: LayoutEngine,
        mut record: Record,
        broadcast_tx: BroadcastSender,
        window_notify: Option<(crate::actor::window_notify::Sender, WindowTxStore)>,
        one_space: bool,
    ) -> Reactor {
        // FIXME: Remove apps that are no longer running from restored state.
        record.start(&config, &layout_engine);
        let (raise_manager_tx, _rx) = actor::channel();
        let (window_notify_tx, window_tx_store) = match window_notify {
            Some((tx, store)) => (Some(tx), store),
            None => (None, WindowTxStore::new()),
        };
        let reactor = Reactor {
            config: config.clone(),
            one_space,
            binding_mode: "default".into(),
            app_manager: managers::AppManager::new(),
            layout_manager: managers::LayoutManager { layout_engine },
            state: RiftState::default(),
            space_state: ForwardedSpaceState::default(),
            last_screen_frames: HashMap::default(),
            space_activation_policy: SpaceActivationPolicy::new(),
            main_window_tracker: MainWindowTracker::default(),
            pending_mouse_focus: None,
            overview_focus: None,
            mouse_inventory_hit: None,
            drag_manager: managers::DragManager {
                actor: crate::actor::drag::DragActor::new(config.settings.drag_drop),
                native_motion_active: std::sync::Arc::default(),
                preview: None,
                preview_enabled: config.settings.drag_drop.enabled
                    && config.settings.drag_drop.preview,
                haptics_enabled: config.settings.drag_drop.haptics_enabled,
                deferred_layouts: Vec::new(),
            },
            workspace_switch_manager: managers::WorkspaceSwitchManager {
                active_workspace_switch: None,
                pending_workspace_switch_origin: None,
            },
            recording_manager: managers::RecordingManager { record },
            communication_manager: managers::CommunicationManager {
                input_tx: None,
                stack_line_tx: None,
                raise_manager_tx,
                event_broadcaster: broadcast_tx,
                wm_sender: None,
                events_tx: None,
            },
            notification_manager: managers::NotificationManager {
                last_sls_notification_ids: Vec::new(),
                last_layout_modes_by_space: HashMap::default(),
                _window_notify_tx: window_notify_tx,
            },
            transaction_manager: transaction_manager::TransactionManager::new(window_tx_store),
            menu_manager: managers::MenuManager {
                menu_state: MenuState::Closed,
                menu_tx: None,
                last_projection_signature: None,
            },
            mission_control_manager: managers::MissionControlManager {
                mission_control_state: MissionControlState::Inactive,
            },
            window_inventory_manager: managers::WindowInventoryManager {
                next_request_id: 0,
                in_flight: HashMap::default(),
                pending: HashSet::default(),
                refocus_after_refresh: HashMap::default(),
            },
            refocus_manager: managers::RefocusManager {
                stale_cleanup_state: StaleCleanupState::Enabled,
                refocus_state: RefocusState::None,
            },
            suppress_auto_workspace_switch_until_input: false,
            pending_space_change_manager: managers::PendingSpaceChangeManager {
                pending_space_change: None,
            },
            spaces_tx: None,
            topology_guard: Default::default(),
            transient_focus: Default::default(),
            mouse_raise_token: Default::default(),
            active_spaces: HashSet::default(),
            startup_ready: None,
            animation_tx: None,
            viewport_gesture: None,
            presentations: HashMap::default(),
            #[cfg(test)]
            layout_calculations: Vec::new(),
            #[cfg(test)]
            native_focus_for_removal: None,
            #[cfg(test)]
            native_tab_successor: None,
        };
        reactor
    }

    fn set_active_spaces(&mut self, spaces: &[Option<SpaceId>]) {
        self.active_spaces.clear();
        for space in spaces.iter().flatten().copied() {
            self.active_spaces.insert(space);
        }
    }

    fn is_space_active(&self, space: SpaceId) -> bool { self.active_spaces.contains(&space) }

    fn iter_active_spaces(&self) -> impl Iterator<Item = SpaceId> + '_ {
        self.active_spaces.iter().copied()
    }

    fn invalidate_native_topology(&mut self, revision: u64) {
        if revision <= self.space_state.revision {
            return;
        }
        self.space_state.revision = revision;
        self.space_state.authoritative = false;
        self.drag_manager.reset();
        self.defer_window_inventory_refresh();
    }

    fn abandon_window_inventories_from_instability(&mut self) {
        // AX requests issued before sleep or display teardown are not guaranteed to
        // reply. Keeping them in `in_flight` prevents the authoritative recovery
        // snapshot from requesting replacement AX elements, leaving every affected
        // window unusable until its application is activated manually.
        let abandoned = self
            .window_inventory_manager
            .in_flight
            .drain()
            .map(|(pid, _)| pid)
            .collect::<Vec<_>>();
        self.window_inventory_manager.pending.extend(abandoned);
    }

    fn window_needs_inventory(&self, wid: WindowId) -> bool {
        self.state.windows.window(wid).is_none_or(|window| {
            // A rejected window is stale only if it now passes the normal
            // eligibility checks. Minimized, rule-excluded, and heuristic-
            // excluded windows must not trigger an AX scan just for focus.
            !window.is_admitted()
                && window.manage_override != Some(false)
                && utils::compute_window_info_manageability(&window.info, |wsid| {
                    self.state.windows.get_window_server_info(wsid)
                })
        })
    }

    fn app_needs_window_inventory(&self, pid: pid_t) -> bool {
        let mut windows = self.state.windows.window_ids_for_pid(pid).peekable();
        if windows.peek().is_none() {
            return true;
        }
        let mut has_stale_window = false;
        for wid in windows {
            if self.state.windows.window(wid).is_some_and(WindowState::is_admitted) {
                return false;
            }
            has_stale_window |= self.window_needs_inventory(wid);
        }
        has_stale_window
    }

    fn request_window_inventory(&mut self, pid: pid_t) {
        if self.refreshes_blocked()
            || self.pending_space_change_manager.pending_space_change.is_some()
            || self.window_inventory_manager.in_flight.contains_key(&pid)
        {
            self.window_inventory_manager.pending.insert(pid);
            return;
        }

        let Some(app) = self.app_manager.apps.get(&pid) else {
            self.window_inventory_manager.pending.remove(&pid);
            return;
        };
        self.window_inventory_manager.next_request_id =
            self.window_inventory_manager.next_request_id.wrapping_add(1);
        let token = WindowInventoryToken {
            request_id: self.window_inventory_manager.next_request_id,
            topology_revision: self.space_state.revision,
        };
        if app.handle.send(Request::RefreshWindowInventory(token)).is_ok() {
            self.window_inventory_manager.in_flight.insert(pid, token);
            self.window_inventory_manager.pending.remove(&pid);
        }
    }

    fn forget_window_inventory(&mut self, pid: pid_t) {
        self.window_inventory_manager.in_flight.remove(&pid);
        self.window_inventory_manager.pending.remove(&pid);
        self.window_inventory_manager.refocus_after_refresh.remove(&pid);
    }

    fn finish_window_inventory(
        &mut self,
        pid: pid_t,
        token: WindowInventoryToken,
        successful: bool,
    ) -> bool {
        if self.window_inventory_manager.in_flight.get(&pid).copied() != Some(token) {
            return false;
        }
        self.window_inventory_manager.in_flight.remove(&pid);

        let accepted = successful
            && !self.refreshes_blocked()
            && self.pending_space_change_manager.pending_space_change.is_none()
            && token.topology_revision == self.space_state.revision;
        if !accepted && successful {
            self.window_inventory_manager.pending.insert(pid);
        }
        if self.window_inventory_manager.pending.remove(&pid) {
            self.request_window_inventory(pid);
        }
        accepted
    }

    fn active_space_ids(&self) -> Vec<u64> {
        self.active_spaces.iter().map(|space| space.get()).collect()
    }

    fn activation_cfg(&self) -> SpaceActivationConfig {
        SpaceActivationConfig {
            default_disable: self.config.settings.default_disable,
            one_space: self.one_space,
        }
    }

    fn display_uuids_for_current_screens(&self) -> Vec<Option<String>> {
        self.space_state
            .screens
            .iter()
            .map(|screen| screen.display_uuid_owned())
            .collect()
    }

    #[cfg(test)]
    fn raw_spaces_for_current_screens(&self) -> Vec<Option<SpaceId>> {
        self.space_state.screens.iter().map(|s| s.space).collect()
    }

    fn display_uuid_for_space(&self, space: SpaceId) -> Option<String> {
        self.space_state
            .screen_by_space(space)
            .and_then(|screen| screen.display_uuid_owned())
    }

    fn expose_space_if_known(&mut self, space: SpaceId) {
        let Some(screen) = self.space_state.screen_by_space(space) else {
            return;
        };
        let size = screen.frame.size;
        let display = screen.display_uuid_opt();
        let _ = self
            .layout_manager
            .layout_engine
            .handle_event(&mut self.state.windows, LayoutEvent::SpaceExposed(space, size));
        self.layout_manager.layout_engine.update_space_display(space, display);
    }

    fn recompute_and_set_active_spaces(
        &mut self,
        changes: &mut Changes,
        spaces: &[Option<SpaceId>],
    ) {
        let previous = self.set_active_spaces_from_snapshot(spaces);
        if previous != self.active_spaces {
            self.handle_active_space_change(changes, &previous);
            self.reconcile_authoritative_active_window_snapshot(
                changes,
                self.authoritative_active_space_windows(),
                true,
                &[],
            );
            changes.inventory_all = true;
        }
    }

    fn set_active_spaces_from_snapshot(&mut self, spaces: &[Option<SpaceId>]) -> HashSet<SpaceId> {
        let cfg = self.activation_cfg();
        let display_uuids = self.display_uuids_for_current_screens();
        let active_spaces =
            self.space_activation_policy.compute_active_spaces(cfg, spaces, &display_uuids);
        let previous = self.active_spaces.clone();
        self.set_active_spaces(&active_spaces);
        previous
    }

    fn recompute_and_set_active_spaces_from_current_screens(&mut self, changes: &mut Changes) {
        let raw_spaces = self.authoritative_spaces_for_current_screens();
        self.recompute_and_set_active_spaces(changes, &raw_spaces);
    }

    fn authoritative_spaces_for_current_screens(&self) -> Vec<Option<SpaceId>> {
        self.space_state
            .screens
            .iter()
            .map(|screen| {
                screen.space.filter(|space| self.space_state.active_spaces.contains(space))
            })
            .collect()
    }

    fn handle_active_space_change(&mut self, changes: &mut Changes, previous: &HashSet<SpaceId>) {
        let activated: Vec<_> = self.active_spaces.difference(previous).copied().collect();
        for space in &activated {
            self.expose_space_if_known(*space);
        }
        self.apply_app_rules_for_activated_spaces(changes, &activated);
    }

    fn apply_app_rules_for_activated_spaces(
        &mut self,
        changes: &mut Changes,
        activated: &[SpaceId],
    ) {
        if activated.is_empty() {
            return;
        }
        let mut windows_by_pid: HashMap<pid_t, Vec<WindowId>> = HashMap::default();

        for (&wsid, &space) in &self.space_state.active_window_spaces {
            if !activated.contains(&space) {
                continue;
            }
            let Some(wid) = self.state.windows.tracked_window_id(wsid) else {
                continue;
            };
            if !self.state.windows.can_reconcile_admission(wid) {
                continue;
            }
            windows_by_pid.entry(wid.pid).or_default().push(wid);
        }

        for (pid, window_ids) in windows_by_pid {
            let Some(app_state) = self.app_manager.apps.get(&pid) else {
                continue;
            };

            self.process_windows_for_app_rules(changes, window_ids, app_state.info.clone(), false);
        }
    }

    fn request_space_snapshot(&self) {
        if let Some(tx) = &self.spaces_tx {
            tx.send(crate::actor::spaces::Event::ReconcileWindowSpaces);
        }
    }

    fn defer_native_space_move(&mut self, wsid: WindowServerId, space: SpaceId) -> bool {
        let Some(wid) = self.state.windows.tracked_window_id(wsid) else {
            return false;
        };
        if self.is_known_fullscreen_window(wsid)
            || self.assigned_space_for_window_id(wid).is_none_or(|assigned| assigned == space)
        {
            return false;
        }
        self.state
            .windows
            .observe_native_space(wsid, space, self.is_space_active(space));
        self.request_space_snapshot();
        true
    }

    fn authoritative_active_space_windows(&self) -> Vec<(WindowServerId, Option<SpaceId>)> {
        let mut membership: Vec<_> = self
            .space_state
            .active_window_spaces
            .iter()
            .filter(|(_, space)| self.is_space_active(**space))
            .map(|(&wsid, &space)| (wsid, Some(space)))
            .collect();
        membership.sort_by_key(|(wsid, _)| *wsid);
        membership
    }

    fn reconcile_authoritative_active_window_snapshot(
        &mut self,
        changes: &mut Changes,
        active_windows: Vec<(WindowServerId, Option<SpaceId>)>,
        preserve_missing_assignments: bool,
        invalidated_spaces: &[SpaceId],
    ) {
        if active_windows.is_empty() && !self.space_state.membership_complete {
            return;
        }
        for &(wsid, observed) in &active_windows {
            if let Some(window) = self.state.windows.tracked_window_id(wsid)
                && let Some(operation) =
                    self.state.windows.record(window).and_then(|record| record.pending_transfer())
                && self.space_state.membership_complete
                && Some(operation.requested_space) == observed
            {
                self.state.windows.confirm_transfer(
                    window,
                    operation.generation,
                    self.space_state.revision,
                    observed,
                );
            }
        }
        let native: Vec<_> = active_windows
            .iter()
            .map(|&(wsid, observed)| {
                let pending = self
                    .state
                    .windows
                    .tracked_window_id(wsid)
                    .and_then(|wid| self.state.windows.record(wid))
                    .and_then(|record| record.pending_transfer())
                    .is_some();
                (
                    wsid,
                    if pending {
                        observed
                    } else {
                        self.resolve_native_space(wsid, observed)
                    },
                )
            })
            .collect();
        let missing = self.state.windows.reconcile_native_snapshot(
            self.layout_manager.layout_engine.workspaces(),
            &native,
            &self.active_spaces,
        );
        for wsid in missing {
            if self
                .state
                .windows
                .tracked_window_id(wsid)
                .and_then(|wid| self.state.windows.record(wid))
                .and_then(|record| record.pending_transfer())
                .is_some()
            {
                continue;
            }
            // Only a unique live membership can establish a destination for
            // a window omitted by the accepted inventory.
            let membership = window_server::window_spaces(wsid);
            let resolved = match membership.as_slice() {
                [space] => Some(*space),
                _ => None,
            };
            if let Some(fullscreen) = resolved.filter(|space| self.is_fullscreen_space(*space)) {
                let user = self
                    .state
                    .windows
                    .tracked_window_id(wsid)
                    .and_then(|wid| self.assigned_space_for_window_id(wid));
                if let Some(wid) =
                    self.state.windows.observe_native_fullscreen(wsid, fullscreen, user, None)
                {
                    self.reconcile_window_membership(changes, wid, None);
                }
                continue;
            }
            // Live membership can precede the accepted Space inventory. Stale
            // absence alone must not change layout participation or ownership.
            if resolved.is_some_and(|space| self.is_space_active(space)) {
                continue;
            }
            let inactive_target =
                resolved.filter(|space| !self.is_space_active(*space)).filter(|space| {
                    #[cfg(test)]
                    {
                        let _ = space;
                        true
                    }
                    #[cfg(not(test))]
                    {
                        window_server::space_is_user(space.get())
                    }
                });
            // Fullscreen topology can precede a positive entry observation.
            // Preserve ownership, but another window's fullscreen is not evidence
            // that this window is logically unavailable.
            if inactive_target.is_none() && !self.space_state.fullscreen_spaces.is_empty() {
                continue;
            }
            if let Some((wid, target)) = self.state.windows.reconcile_native_absence(
                self.layout_manager.layout_engine.workspaces(),
                wsid,
                &self.active_spaces,
                inactive_target,
                preserve_missing_assignments,
            ) {
                if let Some(space) = target {
                    let preserve_ordinal = self
                        .assigned_space_for_window_id(wid)
                        .is_some_and(|assigned| invalidated_spaces.contains(&assigned));
                    self.reassign_window_to_authoritative_space(
                        changes,
                        wid,
                        space,
                        preserve_ordinal,
                    );
                } else {
                    self.reconcile_window_membership(changes, wid, None);
                }
            }
        }
        self.reconcile_windows_in_authoritative_active_snapshot(
            changes,
            &active_windows,
            invalidated_spaces,
        );
    }

    fn is_login_window_pid(&self, pid: pid_t) -> bool {
        self.app_manager.apps.get(&pid).and_then(|a| a.info.bundle_id.as_deref())
            == Some("com.apple.loginwindow")
    }

    fn clear_pending_hidden_window_targets(&self) {
        for (wid, window) in self.state.windows.iter_windows() {
            if self.hidden_assigned_space_for_window_id(wid).is_none() {
                continue;
            }
            if let Some(wsid) = window.info.sys_id {
                self.transaction_manager.clear_target_for_window(wsid);
            }
        }
    }

    fn is_in_drag(&self) -> bool { self.drag_manager.actor.is_active() }

    fn is_dragging_window(&self, window: WindowId) -> bool {
        self.drag_manager.actor.source().is_some_and(|source| source.window == window)
    }

    fn can_start_drag(&self, window: WindowId) -> bool {
        !self.is_in_drag()
            && !self.is_mission_control_active()
            && !self.refreshes_blocked()
            && self.state.windows.is_admitted(window)
            && self.state.windows.can_place_window(window)
    }

    fn can_start_native_drag(&self, window: WindowId, frame: CGRect) -> bool {
        self.can_start_drag(window)
            && (self.communication_manager.events_tx.is_none()
                || (crate::sys::event::get_mouse_state() == Some(MouseState::Down)
                    && window_server::current_cursor_location()
                        .is_ok_and(|point| super::drag::contains(frame, point, 12.0))))
    }

    fn take_drag_authority(&mut self, window: WindowId, frame: CGRect) {
        self.cancel_window_presentations(vec![window]);
        self.layout_manager
            .layout_engine
            .forget_native_layout_for_window(&self.state.windows, window);
        self.state.windows.cancel_transfer(window);
        if let Some(window) = self.state.windows.window_mut(window) {
            window.frame_monotonic = frame;
            if let Some(server) = window.info.sys_id {
                self.transaction_manager.invalidate_for_window(server);
            }
        }
    }

    fn begin_native_drag(
        &mut self,
        window: WindowId,
        origin_frame: CGRect,
        last_frame: CGRect,
        origin_space: Option<SpaceId>,
        current_space: Option<SpaceId>,
    ) {
        self.take_drag_authority(window, origin_frame);
        let tiled = !self.layout_manager.layout_engine.is_window_floating(window);
        let scene = if tiled
            && self.drag_manager.actor.drop_enabled()
            && origin_frame.size.same_as(last_frame.size)
        {
            origin_space.map(|space| self.drag_scene(window, space)).unwrap_or_default()
        } else {
            Default::default()
        };
        self.drag_manager.actor.begin_native(
            crate::actor::drag::DragSource {
                window,
                origin_frame,
                last_frame,
                origin_space,
                current_space,
                tiled,
            },
            scene,
        );
    }

    fn is_mission_control_active(&self) -> bool {
        matches!(
            self.mission_control_manager.mission_control_state,
            MissionControlState::Active
        )
    }

    async fn run(reactor: Reactor, events: Receiver, events_tx: Sender) {
        let (raise_manager_tx, raise_manager_rx) = actor::channel();
        let (animation_tx, animation_rx) = AnimationSender::channel();
        let reactor = Rc::new(RefCell::new(reactor));
        let input_tx = {
            let mut reactor = reactor.borrow_mut();
            reactor.communication_manager.raise_manager_tx = raise_manager_tx.clone();
            reactor.animation_tx = Some(animation_tx);
            reactor.communication_manager.input_tx.clone()
        };
        let reactor_task = Self::run_reactor_loop(reactor, events);
        let raise_manager_task = RaiseManager::run(raise_manager_rx, events_tx, input_tx);
        let presenter = std::thread::Builder::new()
            .name("rift-presenter".into())
            .spawn(move || animation::AnimationManager::run(animation_rx))
            .expect("presentation thread");
        let _ = tokio::join!(reactor_task, raise_manager_task);
        let _ = presenter.join();
    }

    async fn run_reactor_loop(reactor: Rc<RefCell<Reactor>>, mut events: Receiver) {
        const MAX_EVENT_BATCH: usize = 64;

        while let Some((span, event)) = events.recv().await {
            let _guard = span.enter();
            Self::handle_thread_event(&reactor, event);
            for _ in 1..MAX_EVENT_BATCH {
                let Ok((span, event)) = events.try_recv() else {
                    break;
                };
                let _guard = span.enter();
                Self::handle_thread_event(&reactor, event);
            }
        }
        reactor.borrow_mut().gesture_event(crate::actor::gesture::Lifecycle::Reset);
    }

    fn handle_thread_event(reactor: &Rc<RefCell<Reactor>>, event: Event) {
        match event {
            Event::InstallIpc(request) => crate::ipc::install_mach_server(reactor.clone(), request),
            Event::ReleaseClaims(release) => release.apply(&mut reactor.borrow_mut()),
            Event::MouseFocusPending(publisher) => {
                if reactor.borrow().transient_blocks_mouse_focus() {
                    publisher.take_latest();
                    return;
                }
                if reactor.borrow().viewport_gesture.as_ref().is_some_and(|s| !s.released) {
                    publisher.take_latest();
                    return;
                }
                if let Some(point) = publisher.take_latest() {
                    // Resolve against WindowServer when processing the latest position,
                    // rather than preserving an ID from an earlier input callback.
                    if let Some(window) = window_server::get_window_at_point(point) {
                        reactor.borrow_mut().handle_loop_event(Event::MouseMoved(window));
                    } else {
                        trace!(?point, "No window at mouse position");
                    }
                }
            }
            Event::DragMotionPending(publisher) => {
                let motion = publisher.take_latest();
                let mut reactor = reactor.borrow_mut();
                if reactor.drag_manager.actor.is_active()
                    && let Some(motion) = motion
                {
                    reactor.handle_loop_event(Event::DragMotion(motion));
                }
            }
            event => reactor.borrow_mut().handle_loop_event(event),
        }
    }

    fn handle_loop_event(&mut self, event: Event) {
        Self::note_windowserver_activity(&event);
        // Check before fast paths (mouse, gestures, queries that commit a
        // presentation) and before queued invalidation reaches this thread.
        if self.should_quarantine_unstable_topology(&event) {
            self.observe_transient_lifecycle(&event);
            trace!(?event, "quarantined while native topology is unstable");
            return;
        }
        let event = match event {
            Event::Gesture(event) => {
                self.gesture_event(event);
                return;
            }
            Event::BindingModeChanged { mode } => {
                if self.binding_mode != mode {
                    let previous_mode = std::mem::replace(&mut self.binding_mode, mode.clone());
                    if let Some(tx) = &self.menu_manager.menu_tx {
                        tx.send(menu_bar::Event::BindingModeChanged(mode.clone()));
                    }
                    let _ = self
                        .communication_manager
                        .event_broadcaster
                        .send(BroadcastEvent::BindingModeChanged { previous_mode, mode });
                }
                return;
            }
            Event::CameraFinished => {
                self.commit_presentations();
                return;
            }
            Event::Query(req) => {
                if matches!(
                    req,
                    query::QueryRequest::Workspaces { .. }
                        | query::QueryRequest::Windows { .. }
                        | query::QueryRequest::WindowInfo { .. }
                        | query::QueryRequest::LayoutState { .. }
                ) {
                    self.commit_presentations();
                }
                self.handle_query_request(req);
                return;
            }
            Event::MouseMoved(wsid) => {
                self.suppress_auto_workspace_switch_until_input = false;
                if let Some(window) = self.state.windows.tracked_window_id(wsid)
                    && (self.main_window() == Some(window)
                        || crate::sys::app::is_own_window_focused(window))
                    && self.layout_manager.layout_engine.focused_window() == Some(window)
                {
                    if let Some(space) = self.assigned_space_for_window_id(window)
                        && self.is_space_active(space)
                        && self.space_state.screen_by_space(space).is_some()
                    {
                        self.space_state.command_space = Some(space);
                    }
                    self.mouse_inventory_hit = None;
                    // Refresh the command display without native queries or
                    // reconciliation when focus already matches the hit.
                    return;
                }
                Event::MouseMoved(wsid)
            }
            event => event,
        };
        #[cfg(any(test, debug_assertions))]
        let high_frequency = matches!(&event, Event::DragMotion(..));
        self.handle_event(event);
        #[cfg(any(test, debug_assertions))]
        if !high_frequency {
            self.state.windows.debug_assert_invariants();
        }
    }

    pub(crate) fn handle_ipc_command(&mut self, command: Command) {
        self.handle_loop_event(Event::Command(command));
    }

    fn note_windowserver_activity(event: &Event) {
        let wsid = match event {
            Event::WindowFrameChanged(wid, ..) => Some(wid.idx.get()),
            Event::WindowCreated(wid, ..) => Some(wid.idx.get()),
            Event::WindowDestroyed(wid) | Event::WindowInvalidated(wid, _) => Some(wid.idx.get()),
            Event::WindowMinimized(wid) => Some(wid.idx.get()),
            Event::WindowDeminiaturized(wid) => Some(wid.idx.get()),
            Event::MouseMoved(..) => None,
            Event::WindowClosed(wsid) => Some(wsid.as_u32()),
            Event::WindowServerHidden(wsid) | Event::WindowServerUnhidden(wsid) => {
                Some(wsid.as_u32())
            }
            Event::WindowServerDestroyed(wsid, ..) => Some(wsid.as_u32()),
            Event::WindowServerAppeared(wsid, ..) => Some(wsid.as_u32()),
            _ => None,
        };
        if let Some(wsid) = wsid {
            window_server::note_windowserver_activity(wsid);
        }
    }

    fn log_event(&self, event: &Event) {
        match event {
            Event::DragMotion(..) => {}
            Event::WindowFrameChanged(..) | Event::MouseUp(_) | Event::MouseMoved(..) => {
                trace!(?event, "Event")
            }
            _ => debug!(?event, "Event"),
        }
    }

    fn should_update_notifications(event: &Event) -> bool {
        matches!(
            event,
            Event::WindowCreated(..)
                | Event::WindowDestroyed(..)
                | Event::WindowClosed(..)
                | Event::WindowInvalidated(..)
                | Event::WindowServerDestroyed(..)
                | Event::WindowServerAppeared(..)
                | Event::WindowsDiscovered { .. }
                | Event::ApplicationLaunched { .. }
                | Event::ApplicationTerminated(..)
                | Event::ApplicationThreadTerminated(..)
                | Event::SpaceStateChanged(..)
        )
    }

    fn should_quarantine_unstable_topology(&self, event: &Event) -> bool {
        if !self.refreshes_blocked() {
            return false;
        }

        matches!(
            event,
            Event::WindowCreated(..)
                | Event::WindowDestroyed(..)
                | Event::WindowInvalidated(..)
                | Event::WindowServerDestroyed(..)
                | Event::WindowServerAppeared(..)
                | Event::WindowFrameChanged(..)
                | Event::WindowMinimized(..)
                | Event::WindowDeminiaturized(..)
                | Event::WindowTitleChanged(..)
                | Event::WindowClosed(..)
                | Event::WindowServerHidden(..)
                | Event::WindowServerUnhidden(..)
                | Event::WindowServerFocusChanged(..)
                | Event::MouseMoved(..)
                | Event::DragMotion(..)
                | Event::ModifierMouseDown { .. }
                | Event::Gesture(..)
                | Event::Command(Command::Layout(..))
                | Event::Command(Command::Reactor(
                    ReactorCommand::MoveWindowToDisplay { .. }
                        | ReactorCommand::MoveWorkspaceToDisplay { .. }
                ))
        )
    }

    fn refreshes_blocked(&self) -> bool {
        !self.space_state.authoritative
            || self.topology_guard.invalidated_revision() > self.space_state.revision
    }

    /// A sample older than the accepted model, a topology invalidation, or a held newer
    /// sample can no longer be committed.
    fn space_revision_superseded(&self, revision: u64) -> bool {
        let held = self
            .pending_space_change_manager
            .pending_space_change
            .as_ref()
            .map_or(0, |pending| pending.revision);
        revision
            < self
                .space_state
                .revision
                .max(self.topology_guard.invalidated_revision())
                .max(held)
    }

    /// Remaps, resizes and refresh flags are deltas against the producer's own history, so a
    /// sample dropped before commit would orphan the workspaces keyed by its old Space ids.
    /// Geometry and membership are not carried: the next admitted sample owns them.
    fn carry_superseded_space_deltas(&mut self, stale: ForwardedSpaceState) {
        let slot = &mut self.pending_space_change_manager.pending_space_change;
        *slot = Some(match slot.take() {
            None => stale,
            Some(held) if held.revision < stale.revision => {
                topology_workflow::merge_space_deltas(held, stale)
            }
            Some(held) => topology_workflow::merge_space_deltas(stale, held),
        });
    }

    fn defer_window_inventory_refresh(&mut self) {
        self.window_inventory_manager
            .pending
            .extend(self.app_manager.apps.keys().copied());
    }

    fn flush_deferred_window_inventory_refresh(&mut self) {
        if self.refreshes_blocked() {
            return;
        }
        let pending: Vec<_> = self.window_inventory_manager.pending.iter().copied().collect();
        for pid in pending {
            if !self.window_inventory_manager.in_flight.contains_key(&pid) {
                self.request_window_inventory(pid);
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        // This signal only releases the lifecycle guard. Ordinary typing must
        // not commit camera presentations or trigger layout/focus side effects.
        if matches!(event, Event::KeyboardInput) {
            self.suppress_auto_workspace_switch_until_input = false;
            self.recording_manager.record.on_event(&event);
            return;
        }
        if matches!(event, Event::DragMotion(..)) {
            self.handle_event_inner(event);
        } else {
            self.handle_event_traced(event);
        }
    }

    #[instrument(name = "reactor::handle_event", skip(self), fields(event=?event))]
    fn handle_event_traced(&mut self, event: Event) { self.handle_event_inner(event) }

    fn handle_event_inner(&mut self, event: Event) {
        let transient_count = self.transient_focus.windows.len();
        self.observe_transient_lifecycle(&event);
        let may_make_ready = matches!(&event, Event::SpaceStateChanged(_));
        let previously_focused_window = self.main_window();
        let claim_suppressed_ffm =
            self.state.windows.suppresses_focus_follows_mouse(previously_focused_window);
        let mut changes = Changes::default();
        let error = self.dispatch(event, &mut changes).err();
        changes.refresh_notifications |= transient_count != self.transient_focus.windows.len();
        let focused_window = self.main_window();
        changes.refresh_ffm |= claim_suppressed_ffm
            != self.state.windows.suppresses_focus_follows_mouse(focused_window);
        let ready_space = if error.is_none() && may_make_ready && self.startup_ready.is_some() {
            self.default_query_space()
                .filter(|space| self.space_state.screen_by_space(*space).is_some())
        } else {
            None
        };
        if let Some(space) = ready_space {
            self.expose_space_if_known(space);
        }
        self.finish(
            changes,
            focused_window.filter(|_| focused_window != previously_focused_window),
            error,
        );
        if ready_space.is_some()
            && let Some(tx) = self.startup_ready.take()
        {
            let _ = tx.send(());
        }
        self.drag_manager.sync_motion_gate();
    }

    fn dispatch(&mut self, mut event: Event, changes: &mut Changes) -> anyhow::Result<()> {
        if let Event::AppActorExited(pid, handle) = &event {
            if !self.app_manager.apps.get(pid).is_some_and(|app| app.handle.same_actor(handle)) {
                return Ok(());
            }
            event = Event::ApplicationThreadTerminated(*pid);
        }
        match &event {
            Event::ApplicationTerminated(pid) => {
                let windows = self.state.windows.window_ids_for_pid(*pid).collect();
                self.cancel_window_presentations(windows);
            }
            Event::SpaceStateChanged(snapshot)
                if !snapshot.authoritative
                    || self.space_revision_superseded(snapshot.revision)
                    || (!snapshot.presentation_changed(&self.space_state)
                        && !self.refreshes_blocked()) =>
            {
                // Restated membership is not a topology transition. Cancelling here
                // can strand a window midway through an otherwise valid animation.
            }
            Event::MissionControlNativeEntered
            | Event::TopologyInvalidated(_)
            | Event::SpaceStateChanged(_)
            | Event::SystemWoke => {
                let windows = self.state.windows.iter_windows().map(|(wid, _)| wid).collect();
                self.cancel_window_presentations(windows);
            }
            _ => {}
        }
        // These notifications neither inspect camera position nor edit geometry.
        if !matches!(
            event,
            Event::WindowFrameChanged(_, _, _, Requested(true), _)
                | Event::WindowFrameWriteFailed(..)
                | Event::WindowTitleChanged(..)
                | Event::MenuOpened(_)
                | Event::MenuClosed(_)
                | Event::WindowInventoryRefreshRequested(_)
                | Event::RaiseCompleted { .. }
                | Event::RaiseTimeout { .. }
                | Event::ApplicationDeactivated(_)
                | Event::ApplicationGloballyDeactivated(_)
                | Event::SessionDidBecomeActive
                | Event::ActiveDisplayChanged { .. }
        ) {
            self.commit_presentations();
        }
        self.log_event(&event);
        self.recording_manager.record.on_event(&event);

        // Wake/unlock produces synthetic activation notifications as loginwindow
        // yields focus back to the pre-sleep application. Only real input makes
        // a subsequent activation a trustworthy request to follow an app to a
        // different virtual workspace.
        if matches!(
            event,
            Event::MouseUp(_) | Event::MouseMoved(_) | Event::Command(_)
        ) {
            self.suppress_auto_workspace_switch_until_input = false;
        }

        match event {
            Event::TopologyInvalidated(revision) => {
                self.invalidate_native_topology(revision);
                return Ok(());
            }
            Event::SystemWoke => {
                self.drag_manager.reset();
                self.suppress_auto_workspace_switch_until_input = true;
                changes.refresh_notifications = true;
                return Ok(());
            }
            Event::SessionDidBecomeActive => {
                self.suppress_auto_workspace_switch_until_input = true;
                return Ok(());
            }
            _ => {}
        }

        let should_update_notifications = Self::should_update_notifications(&event);
        let duplicate_global_activation = matches!(
            &event,
            Event::ApplicationGloballyActivated(pid)
                if self.main_window_tracker.is_globally_frontmost(*pid)
        );

        // Reject before updating focus or inventory state.
        if let Event::ApplicationLaunched { pid, handle, .. } = &event
            && self.app_manager.reject_duplicate(*pid, handle)
        {
            return Ok(());
        }

        let previous_native_focus = self.main_window_tracker.window_server_focus();
        let raised_window = self.main_window_tracker.handle_event(&event);
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
                let _ = (is_frontmost, main_window);
                self.forget_window_inventory(pid);
                self.app_manager.apps.insert(pid, AppState { info, handle });
                if self.spaces_tx.is_some()
                    && let Some(app) = self.app_manager.apps.get_mut(&pid)
                {
                    app.handle.set_topology_authority(
                        &self.topology_guard,
                        self.space_state
                            .revision
                            .saturating_sub(u64::from(!self.space_state.authoritative)),
                    );
                }
                self.update_partial_window_server_info(changes, window_server_info);
                self.on_windows_discovered_with_app_info(
                    changes,
                    pid,
                    visible_windows,
                    Vec::new(),
                    None,
                );
                if self.main_window_tracker.is_globally_frontmost(pid) {
                    self.send_app_request(pid, Request::ApplicationGloballyActivated(pid));
                }
                self.reconcile_observed_focus(changes, raised_window);
                return Ok(());
            }
            Event::ApplicationTerminated(pid) => {
                self.send_app_request(pid, Request::Terminate);
                return Ok(());
            }
            Event::ApplicationThreadTerminated(pid) => {
                self.forget_window_inventory(pid);
                self.clear_menu_state_for_pid(pid);
                self.retire_app_windows(changes, pid);
                changes.refresh_menu |= self.app_manager.apps.remove(&pid).is_some();
                return Ok(());
            }
            Event::ApplicationActivated(pid, quiet) => {
                self.clear_menu_state_for_non_owner(pid);
                if quiet == Quiet::No && self.main_window_tracker.is_globally_frontmost(pid) {
                    // Prefer the app thread's resolved main window; pending
                    // AX admission can leave the previous main window cached.
                    // Some apps leave AXMainWindow unset, so a sole admitted
                    // window is the only target Rift can follow.
                    let activation_window = if self.state.windows.has_pending_window_for_pid(pid) {
                        None
                    } else {
                        self.main_window_tracker
                            .app_main_window(pid)
                            .or_else(|| self.sole_admitted_window(pid))
                    };
                    self.auto_switch_to_window_workspace(changes, activation_window);
                    self.reconcile_observed_focus(changes, activation_window);
                } else {
                    self.reconcile_observed_focus(changes, raised_window);
                }
                return Ok(());
            }
            Event::ApplicationDeactivated(pid) | Event::ApplicationGloballyDeactivated(pid) => {
                self.clear_menu_state_for_pid(pid);
            }
            Event::ApplicationGloballyActivated(pid) => {
                if duplicate_global_activation {
                    trace!(pid, "Ignoring duplicate global application activation");
                    changes.refresh_notifications |= should_update_notifications;
                    return Ok(());
                }
                self.clear_menu_state_for_non_owner(pid);
                if !self.is_login_window_pid(pid) {
                    if let Some(app) = self.app_manager.apps.get(&pid) {
                        let _ = app.handle.send(Request::ApplicationGloballyActivated(pid));
                    }
                    // Recover missing/stale inventory, but leave apps with only
                    // intentionally excluded windows alone.
                    if self.app_needs_window_inventory(pid) {
                        self.request_window_inventory(pid);
                    }
                }
                // The app thread will resolve the current AX main window and
                // emit ApplicationActivated. Do not replay cached focus here.
                changes.refresh_notifications |= should_update_notifications;
                return Ok(());
            }
            Event::WindowServerFocusChanged(window, reported_space) => {
                if self.window_needs_inventory(window) {
                    // Recover stale admission before the cached-layout-focus
                    // fast path as well, without rescanning excluded windows.
                    self.request_window_inventory(window.pid);
                }
                if self.state.windows.contains_window(window)
                    && self.is_space_active(reported_space)
                    && self.space_state.screen_by_space(reported_space).is_some()
                {
                    self.space_state.command_space = Some(reported_space);
                    // Same-app focus changes have no activation edge. Cross-app
                    // changes still wait for the app thread's resolved activation.
                    if previous_native_focus
                        .is_some_and(|previous| previous.pid == window.pid && previous != window)
                    {
                        if self.auto_switch_to_window_workspace(changes, Some(window)) {
                            return Ok(());
                        }
                    }
                }
                if self.layout_manager.layout_engine.focused_window() == Some(window) {
                    changes.space(reported_space).tabs_changed = true;
                    if let Some(input_tx) = &self.communication_manager.input_tx {
                        _ = input_tx.send(crate::actor::input::Request::EnforceHidden);
                    }
                    return Ok(());
                }
                if !self.state.windows.contains_window(window) {
                    return Ok(());
                }
                if self.is_space_active(reported_space) {
                    self.reconcile_layout_event(
                        changes,
                        LayoutEvent::WindowFocused(reported_space, window),
                    );
                }
                return Ok(());
            }
            Event::RegisterSenders { wm, spaces, topology_guard } => {
                self.spaces_tx = Some(spaces);
                self.topology_guard = topology_guard;
                for app in self.app_manager.apps.values_mut() {
                    app.handle.set_topology_authority(
                        &self.topology_guard,
                        self.space_state
                            .revision
                            .saturating_sub(u64::from(!self.space_state.authoritative)),
                    );
                }
                self.communication_manager.wm_sender = Some(wm);
                return Ok(());
            }
            Event::WindowInventoryRefreshRequested(pid) => {
                self.request_window_inventory(pid);
                return Ok(());
            }
            Event::RaiseTargetsMissing { windows, sequence_id } => {
                let Some(first) = windows.first() else {
                    return Ok(());
                };
                self.window_inventory_manager.refocus_after_refresh.insert(first.pid, *first);
                self.request_window_inventory(first.pid);
                for window_id in windows {
                    self.submit_raise(raise_manager::Event::RaiseCompleted {
                        window_id,
                        sequence_id,
                    });
                }
                return Ok(());
            }
            Event::WindowsDiscovered {
                pid,
                token,
                successful,
                new,
                known_visible,
            } => {
                if !self.finish_window_inventory(pid, token, successful) {
                    debug!(pid, ?token, successful, "Discarding stale AX window inventory");
                    return Ok(());
                }
                let refocus = self
                    .window_inventory_manager
                    .refocus_after_refresh
                    .remove(&pid)
                    .filter(|target| new.iter().any(|(wid, _)| wid == target));
                self.on_windows_discovered_with_app_info(changes, pid, new, known_visible, None);
                if let Some(space) =
                    refocus.and_then(|window| self.assigned_space_for_window_id(window))
                {
                    changes.arrange(Some(space), Presentation::Animated);
                }
                self.reconcile_observed_focus(changes, raised_window);
                return Ok(());
            }
            Event::WindowCreated(wid, mut window, ws_info, _) => {
                self.replace_native_tab(wid, &mut window);
                self.observe_created_window(changes, wid, window, ws_info);
                self.reconcile_observed_focus(changes, raised_window);
                return Ok(());
            }
            Event::WindowDestroyed(wid) => {
                // macOS can replace AXUIElements during lifecycle/display churn while the
                // native window remains alive. Recovery already schedules a stable refresh,
                // so preserve topology until then. Outside churn, retain the original AX
                // destruction behavior and remove the window immediately.
                if self.refreshes_blocked() {
                    return Ok(());
                }

                if self.retain_native_tab_slot_on_departure(wid) {
                    return Ok(());
                }

                if self.state.windows.suspend_fullscreen_ax_identity(wid) {
                    self.reconcile_window_membership(changes, wid, None);
                } else {
                    self.retire_window(changes, wid);
                }
                self.reconcile_observed_focus(changes, raised_window);
                return Ok(());
            }
            Event::WindowClosed(wsid) => {
                let Some(wid) = self.state.windows.tracked_window_id(wsid) else {
                    self.state.windows.remove_window_server_state(wsid);
                    return Ok(());
                };
                if self.retain_native_tab_slot_on_departure(wid) {
                    return Ok(());
                }
                self.retire_window(changes, wid);
                self.reconcile_observed_focus(changes, raised_window);
                return Ok(());
            }
            Event::WindowServerHidden(wsid) | Event::WindowServerUnhidden(wsid) => {
                let visible = matches!(event, Event::WindowServerUnhidden(_));
                // WindowServer hidden/unhidden means ordered out/in, not AX
                // minimization or application hiding. Keep lifecycle suspension
                // independent of this native visibility evidence.
                if let Some(pid) = self.state.windows.observe_native_visibility(wsid, visible) {
                    self.request_window_inventory(pid);
                }
                return Ok(());
            }
            Event::WindowInvalidated(wid, source) => {
                trace!(?wid, ?source, "AX identity invalidated");
                self.invalidate_window_identity(changes, wid);
                return Ok(());
            }
            Event::WindowServerDestroyed(wsid, sid, kind) => {
                let resolved = matches!(kind, SpaceEventKind::User)
                    .then(|| self.resolve_native_space(wsid, None))
                    .flatten();
                self.transient_focus.depart(wsid, sid, resolved);
                if matches!(kind, SpaceEventKind::User)
                    && let Some(space) = resolved.filter(|space| *space != sid)
                    && self.defer_native_space_move(wsid, space)
                {
                    return Ok(());
                }
                let tracked_window = self.state.windows.tracked_window_id(wsid);
                if matches!(kind, SpaceEventKind::User)
                    && tracked_window
                        .is_some_and(|wid| self.retain_native_tab_slot_on_departure(wid))
                {
                    return Ok(());
                }
                let ordered_in = matches!(kind, SpaceEventKind::User)
                    .then(|| window_server::window_ordered_in(wsid))
                    .flatten();
                let observations = topology_workflow::WindowServerDestroyedObservations {
                    resolved_space: resolved,
                    ordered_in,
                    suitable: (ordered_in == Some(false))
                        .then(|| window_server::app_window_suitability(wsid))
                        .flatten(),
                };
                self.reconcile_native_departure(changes, wsid, sid, kind, observations);
                return Ok(());
            }
            Event::WindowServerAppeared(wsid, sid, kind) => {
                let resolved = self.resolve_native_space(wsid, Some(sid));
                let tracked_window = self.state.windows.tracked_window_id(wsid);
                let queried = window_server::get_window_with_transient(wsid)
                    .filter(|(info, _)| tracked_window.is_none_or(|wid| wid.pid == info.pid));
                if let Some((info, native)) = queried {
                    let space = resolved.unwrap_or(sid);
                    if tracked_window.is_some() {
                        self.state.windows.track_window_server_info(info);
                    }
                    let windows = &self.state.windows;
                    self.transient_focus.native(info.id, info.pid, space, native);
                    if !native
                        && let Some(wid) = tracked_window
                        && let Some(window) = windows.window(wid)
                    {
                        self.transient_focus.ax(wsid, info.pid, &window.info, space);
                    }
                    self.transient_focus.visibility(wsid, true);
                }
                let readmitted = tracked_window.is_some_and(|wid| {
                    self.reconcile_native_admission(changes, wid, queried.is_some(), None)
                });
                if matches!(kind, SpaceEventKind::User)
                    && let Some(space) = resolved
                    && self.defer_native_space_move(wsid, space)
                {
                    if let Some(wid) = tracked_window.filter(|_| readmitted) {
                        self.reconcile_native_presence(changes, wsid, space, wid);
                    }
                    return Ok(());
                }
                let last_known_user_space = tracked_window
                    .and_then(|window| self.best_space_for_window_id(window))
                    .or_else(|| self.space_state.iter_known_spaces().next());
                let observations = topology_workflow::WindowServerAppearedObservations {
                    resolved_space: resolved,
                    last_known_user_space,
                    window_server_info: queried.map(|(info, _)| info),
                };
                self.reconcile_native_appearance(changes, wsid, sid, kind, observations);
                return Ok(());
            }
            Event::WindowMinimized(wid) | Event::WindowDeminiaturized(wid) => {
                self.observe_window_minimization(
                    changes,
                    wid,
                    matches!(event, Event::WindowMinimized(_)),
                );
                return Ok(());
            }
            Event::WindowFrameWriteFailed(wid, txid) => {
                if let Some(server) = self.state.windows.window(wid).and_then(|w| w.info.sys_id) {
                    self.transaction_manager.store.fail(server, txid);
                }
                // Retry on the next arrange pass. A busy app must not drive its own retry loop.
                return Ok(());
            }
            Event::WindowFrameChanged(wid, new_frame, last_seen, requested, mouse_state) => {
                if self.is_mission_control_active() {
                    return Ok(());
                }
                let mut effective_mouse_state = mouse_state;
                // A delayed move from an app can invalidate a parked frame even when
                // WindowServer reports Rift's transaction id. Drop the stale hide
                // target so the next layout can re-park the window.
                if !requested.0
                    && mouse_state != Some(MouseState::Down)
                    && let Some(window) = self.state.windows.window(wid)
                    && let Some(wsid) = window.info.sys_id
                    && let Some(write) = self.transaction_manager.store.get(&wsid)
                    && last_seen == Some(write.txid)
                    && write.target().is_some_and(|target| !new_frame.same_as(target))
                    && self.hidden_assigned_space_for_window_id(wid).is_some()
                {
                    self.transaction_manager.clear_target_for_window(wsid);
                }
                let needs_layout = window_workflow::frame_change_needs_layout(
                    &mut self.state,
                    &self.transaction_manager,
                    wid,
                    new_frame,
                    last_seen,
                    requested.0,
                    &mut effective_mouse_state,
                );
                // Modifier drags get their real release from the input tap.
                let infer_release = self.is_dragging_window(wid)
                    && self.drag_manager.actor.kind()
                        != Some(crate::actor::drag::DragKind::ModifierMove)
                    && effective_mouse_state.or_else(crate::sys::event::get_mouse_state)
                        == Some(MouseState::Up);
                if !needs_layout {
                    if infer_release {
                        self.infer_left_mouse_up(changes);
                    }
                    self.reconcile_observed_focus(changes, raised_window);
                    return Ok(());
                }
                let (server_id, old_frame) = self
                    .state
                    .windows
                    .window(wid)
                    .map(|window| (window.info.sys_id, window.frame_monotonic))
                    .unwrap_or((None, new_frame));
                let old_space = self.geometry_space_for_window(&old_frame, server_id);
                let new_space = self.geometry_space_for_window(&new_frame, server_id);
                let assigned_space = self.assigned_space_for_window_id(wid);
                let active =
                    |space: Option<SpaceId>| space.is_some_and(|s| self.is_space_active(s));
                if effective_mouse_state == Some(MouseState::Down)
                    && (active(old_space) || active(new_space))
                    && self.can_start_native_drag(wid, new_frame)
                {
                    self.begin_native_drag(
                        wid,
                        old_frame,
                        new_frame,
                        assigned_space.or(old_space),
                        new_space,
                    );
                }
                let dragging = self.is_dragging_window(wid);
                let (old_space, new_space) =
                    if !dragging && assigned_space.is_some() && new_space != assigned_space {
                        self.request_space_snapshot();
                        (assigned_space, assigned_space)
                    } else {
                        (old_space, new_space)
                    };
                let crossing = !dragging && old_space != new_space;
                let old_space_active = old_space.is_some_and(|s| self.is_space_active(s));
                let new_space_active = new_space.is_some_and(|s| self.is_space_active(s));
                let pending_target_space = server_id
                    .filter(|_| crossing)
                    .and_then(|server| self.pending_target_space_for_window_server_id(server));
                let keep_assigned_for_scrolling = crossing
                    && old_space.is_some_and(|space| {
                        self.layout_manager.layout_engine.active_layout_mode_at(space)
                            == crate::common::config::LayoutMode::Scrolling
                            && !self.layout_manager.layout_engine.is_window_floating(wid)
                            && assigned_space == Some(space)
                    });
                let active_resize_space = if dragging && !old_frame.size.same_as(new_frame.size) {
                    assigned_space
                        .or_else(|| self.best_space_for_window(&new_frame, server_id))
                        .filter(|space| self.is_space_active(*space))
                        .or_else(|| {
                            server_id.is_none().then(|| self.workspace_command_space()).flatten()
                        })
                } else {
                    None
                };
                let screens = if active_resize_space.is_some() {
                    self.space_state
                        .screens
                        .iter()
                        .filter_map(|screen| {
                            Some((screen.space?, screen.frame, screen.display_uuid_owned()))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                self.handle_window_frame_changed(
                    changes,
                    window_workflow::WindowFrameChangedPayload {
                        window: wid,
                        new_frame,
                        old_space,
                        new_space,
                        old_space_active,
                        new_space_active,
                        active_resize_space,
                        pending_target_space,
                        assigned_space,
                        keep_assigned_for_scrolling,
                        screens,
                    },
                );
                if infer_release {
                    self.infer_left_mouse_up(changes);
                }
                self.reconcile_observed_focus(changes, raised_window);
                return Ok(());
            }
            Event::WindowTitleChanged(wid, new_title) => {
                if self.config.settings.ui.stack_line.enabled
                    && let Some(tx) = &self.communication_manager.stack_line_tx
                {
                    tx.send(stack_line::Event::TitleChanged(wid, new_title.clone()));
                }
                if let Some(window) = self.state.windows.window_mut(wid)
                    && window.info.title != new_title
                {
                    let old_title = std::mem::replace(&mut window.info.title, new_title.clone());
                    self.maybe_reapply_app_rules_for_window(changes, wid);
                    changes.title_changed = Some((wid, old_title, new_title));
                }
                self.reconcile_observed_focus(changes, raised_window);
                return Ok(());
            }
            Event::SpaceStateChanged(snapshot) => {
                self.reconcile_space_snapshot(changes, snapshot);
                return Ok(());
            }
            Event::ActiveDisplayChanged { menu_bar_space, command_space } => {
                self.space_state.menu_bar_space = menu_bar_space;
                self.space_state.command_space = command_space;
                return Ok(());
            }
            Event::DragMotion(motion) => {
                let mut intent_changed = self.drag_manager.actor.motion(motion);
                if self.drag_manager.actor.kind()
                    == Some(crate::actor::drag::DragKind::ModifierMove)
                {
                    let current_space =
                        self.screen_for_point(motion.point)
                            .and_then(|screen| screen.space)
                            .or_else(|| {
                                self.drag_manager.actor.source().and_then(|source| {
                                    self.best_space_for_frame(&source.last_frame)
                                })
                            });
                    if self.drag_manager.actor.update_current_space(current_space) {
                        intent_changed |= self.drag_manager.actor.motion(motion);
                    }
                }
                if intent_changed {
                    self.resolve_drag_preview();
                }
                let Some((window, new_frame)) = self.drag_manager.actor.interactive_update() else {
                    return Ok(());
                };
                self.write_window_frame(window, new_frame, true, None);
                return Ok(());
            }
            Event::DragCancel => {
                self.cancel_drag(changes);
                return Ok(());
            }
            Event::ModifierMouseDown { button, point, action } => {
                if action == crate::common::config::MouseAction::None
                    || !self.drag_manager.actor.drop_enabled()
                {
                    return Ok(());
                }
                let source = self.window_id_under_cursor().and_then(|window| {
                    let state = self.state.windows.window(window)?;
                    self.can_start_drag(window).then_some((
                        window,
                        state.frame_monotonic,
                        state.info.sys_id,
                    ))
                });
                let Some((window, frame, server_id)) = source else {
                    return Ok(());
                };
                let assigned_space = self.assigned_space_for_window_id(window);
                self.take_drag_authority(window, frame);
                let space =
                    assigned_space.or_else(|| self.best_space_for_window(&frame, server_id));
                let tiled = !self.layout_manager.layout_engine.is_window_floating(window);
                let scene = if tiled {
                    space.map(|space| self.drag_scene(window, space)).unwrap_or_default()
                } else {
                    Default::default()
                };
                self.drag_manager.actor.begin_modifier(
                    crate::actor::drag::DragSource {
                        window,
                        origin_frame: frame,
                        last_frame: frame,
                        origin_space: space,
                        current_space: space,
                        tiled,
                    },
                    button,
                    point,
                    action,
                    scene,
                );
                self.drag_manager.sync_preview();
                return Ok(());
            }
            Event::OverviewSelectWorkspace {
                display,
                workspace,
                window,
                transition,
            } => {
                let Some(space) = self
                    .layout_manager
                    .layout_engine
                    .workspaces()
                    .workspaces
                    .get(workspace)
                    .map(|workspace| workspace.space)
                else {
                    return Ok(());
                };
                if !self.is_space_active(space) {
                    return Ok(());
                }
                let changed =
                    self.layout_manager.layout_engine.workspaces().active_workspace(space)
                        != Some(workspace);
                if let Some(stamp) = transition {
                    self.overview_focus = window.map(|window| (space, window, stamp));
                }
                if transition.is_some() || self.overview_focus.is_some() {
                    self.stop_camera(space);
                }
                self.store_current_floating_positions(space);
                let mut response = self.layout_manager.layout_engine.activate_workspace(
                    &mut self.state.windows,
                    space,
                    workspace,
                    window,
                );
                if window.is_some() {
                    self.layout_manager.layout_engine.commit_workspace_focus(
                        &mut self.state.windows,
                        space,
                        response.focus_window,
                    );
                }
                response.changed = changed || window.is_some();
                if changed {
                    changes.workspace_changed(space);
                    self.workspace_switch_manager
                        .start_workspace_switch(space, WorkspaceSwitchOrigin::Manual);
                }
                if response.changed {
                    let policy = if transition.is_some() || self.overview_focus.is_some() {
                        Presentation::Instant
                    } else if changed {
                        Presentation::WorkspaceSwitch
                    } else {
                        Presentation::Animated
                    };
                    changes.arrange(Some(space), policy);
                }
                changes.overview_activation = transition.map(|stamp| (workspace, window, stamp));
                // Follow only a workspace that lives on another display.
                if self
                    .space_state
                    .screen_by_space(space)
                    .is_some_and(|screen| screen.display_uuid != display)
                {
                    changes.display_warp = Some(space);
                }
                if let Some(window) = window.filter(|&window| response.focus_window == Some(window))
                {
                    // An explicit overview pick can select a parked window whose
                    // native visibility has yet to catch up.
                    self.focus_window(changes, window, None, Some(space));
                } else {
                    self.reconcile_layout_response(changes, response, Some(space));
                }
                return Ok(());
            }
            Event::OverviewDrop { intent, reply } => {
                let source = self.state.windows.workspace_for_window(intent.window);
                let destination = self
                    .layout_manager
                    .layout_engine
                    .workspaces()
                    .workspaces
                    .get(intent.workspace)
                    .map(|ws| ws.space);
                let valid = self.state.windows.window(intent.window).is_some_and(|window| {
                    self.state.windows.is_admitted(intent.window)
                        && window.info.is_standard
                        && !window.info.is_minimized
                }) && source.zip(destination).is_some_and(|(source, destination)| {
                    [
                        self.layout_manager.layout_engine.workspaces()[source].space,
                        destination,
                    ]
                    .into_iter()
                    .all(|space| {
                        self.space_state.screen_by_space(space).is_some()
                            && !self.is_fullscreen_space(space)
                    })
                });
                let changed = valid
                    && self
                        .layout_manager
                        .layout_engine
                        .relocate_window_with_drop(&mut self.state.windows, &intent);
                let _ = reply.send(changed);
                if !changed {
                    return Ok(());
                }
                let source_space =
                    self.layout_manager.layout_engine.workspaces()[source.unwrap()].space;
                let destination = destination.unwrap();
                changes.move_between(source_space, destination, Presentation::Animated);
                if source_space != destination {
                    let frame = intent.frame.unwrap_or_else(|| {
                        let destination = self
                            .space_state
                            .screens
                            .iter()
                            .find(|s| s.space == Some(destination))
                            .unwrap()
                            .frame;
                        let source = self
                            .space_state
                            .screens
                            .iter()
                            .find(|s| s.space == Some(source_space))
                            .map(|s| s.frame)
                            .unwrap_or(destination);
                        let mut frame =
                            self.state.windows.window(intent.window).unwrap().frame_monotonic;
                        frame.origin.x += destination.origin.x - source.origin.x;
                        frame.origin.y += destination.origin.y - source.origin.y;
                        frame
                    });
                    self.send_transfer_frame(intent.window, destination, Some(frame), true, None);
                }
                return Ok(());
            }
            Event::MouseUp(button) => {
                self.reconcile_mouse_up(changes, button);
                return Ok(());
            }
            Event::MenuOpened(pid) => {
                system_workflow::handle_menu_opened(&mut self.menu_manager, pid);
                changes.refresh_ffm = true;
                return Ok(());
            }
            Event::MenuClosed(pid) => {
                changes.refresh_ffm |=
                    system_workflow::handle_menu_closed(&mut self.menu_manager, pid);
                return Ok(());
            }
            Event::MouseMoved(wsid) => {
                if self.transient_blocks_mouse_focus() {
                    return Ok(());
                }
                let window = self.state.windows.tracked_window_id(wsid);
                if window.is_some_and(|window| {
                    self.pending_mouse_focus.is_some_and(|(pending, started)| {
                        pending == window && started.elapsed() < Duration::from_secs(1)
                    })
                }) {
                    return Ok(());
                }
                if window.is_none() {
                    if self.mouse_inventory_hit == Some(wsid) {
                        return Ok(());
                    }
                    self.mouse_inventory_hit = Some(wsid);
                    trace!(?wsid, "Mouse hit window missing from inventory");
                    if let Some(info) = self
                        .state
                        .windows
                        .get_window_server_info(wsid)
                        .or_else(|| window_server::get_window(wsid))
                        && info.layer == 0
                        && !self.window_inventory_manager.in_flight.contains_key(&info.pid)
                    {
                        self.request_window_inventory(info.pid);
                    }
                    return Ok(());
                }
                self.mouse_inventory_hit = None;
                let active_space = window.and_then(|window| {
                    self.state.windows.window(window).and_then(|state| {
                        self.best_space_for_window(&state.frame_monotonic, state.info.sys_id)
                            .filter(|space| self.is_space_active(*space))
                            .or_else(|| {
                                state
                                    .info
                                    .sys_id
                                    .is_none()
                                    .then(|| self.workspace_command_space())
                                    .flatten()
                            })
                    })
                });
                let window = window.unwrap();
                if !self.should_raise_on_mouse_over(window, active_space) {
                    return Ok(());
                }
                let is_main = self.main_window() == Some(window)
                    || crate::sys::app::is_own_window_focused(window);
                if is_main && self.layout_manager.layout_engine.focused_window() == Some(window) {
                    return Ok(());
                }
                if let Some(space) = active_space {
                    self.reconcile_layout_event(changes, LayoutEvent::WindowFocused(space, window));
                }
                if !is_main {
                    if self.mouse_raise_token.is_cancelled() {
                        self.mouse_raise_token = Default::default();
                    }
                    let mut app_handles = HashMap::default();
                    self.insert_app_handle_for_window(&mut app_handles, window);
                    changes.focus = Some(raise_manager::Event::MouseRaiseRequest(
                        RaiseRequest {
                            raise_windows: vec![vec![window]],
                            focus_window: Some((window, None)),
                            app_handles,
                            focus_quiet: Quiet::Yes,
                        },
                        self.mouse_raise_token.clone(),
                    ));
                    self.pending_mouse_focus = Some((window, Instant::now()));
                }
                return Ok(());
            }
            Event::MissionControlNativeEntered => {
                self.drag_manager.reset();
                self.set_mission_control_active(true);
                return Ok(());
            }
            Event::MissionControlNativeExited => {
                self.set_mission_control_active(false);
                self.try_apply_pending_space_change(changes);
                self.refresh_windows_after_mission_control(changes);
                return Ok(());
            }
            Event::RaiseCompleted { window_id, sequence_id } => {
                if self.pending_mouse_focus.is_some_and(|(window, _)| window == window_id) {
                    self.pending_mouse_focus = None;
                }
                self.submit_raise(raise_manager::Event::RaiseCompleted { window_id, sequence_id });
                if let Some((space, selected, _)) =
                    self.overview_focus.filter(|(_, selected, _)| *selected == window_id)
                {
                    self.overview_focus = None;
                    // The native key-focus query can still report the outgoing
                    // app at raise completion. Keep the explicit overview pick
                    // authoritative; the cover separately verifies native focus.
                    self.stop_camera(space);
                    self.reconcile_layout_event(
                        changes,
                        LayoutEvent::WindowFocused(space, selected),
                    );
                    changes.arrange(Some(space), Presentation::Instant);
                }
                return Ok(());
            }
            Event::RaiseTimeout { sequence_id } => {
                self.submit_raise(raise_manager::Event::RaiseTimeout { sequence_id });
                return Ok(());
            }
            Event::ConfigUpdated(new_cfg) => {
                if new_cfg.virtual_workspaces.mode == crate::common::config::WorkspaceMode::Global
                    && !crate::sys::screen::displays_have_separate_spaces()
                {
                    warn!(
                        "Global workspaces require separate native Spaces for displays; retaining current configuration"
                    );
                    return Ok(());
                }
                // Apply changed pins before balancing their remaining workspaces.
                let pins = |engine: &LayoutEngine| -> Vec<_> {
                    let pins = engine.workspaces().rule_displays();
                    pins.map(|(id, display)| (id, display.to_owned())).collect()
                };
                let before = pins(&self.layout_manager.layout_engine);
                self.apply_config(changes, new_cfg)?;
                let after = pins(&self.layout_manager.layout_engine);
                if before != after {
                    let workspaces = self.layout_manager.layout_engine.workspaces();
                    let visible_pin_changed = before.iter().chain(&after).any(|pin| {
                        before.contains(pin) != after.contains(pin)
                            && workspaces
                                .workspace_space(pin.0)
                                .is_some_and(|space| self.active_spaces.contains(&space))
                    });
                    let activated: HashSet<_> = after
                        .into_iter()
                        .filter(|pin| !before.contains(pin))
                        .map(|(id, _)| id)
                        .collect();
                    let workspaces = self.layout_manager.layout_engine.workspaces_mut();
                    for &id in &activated {
                        workspaces[id].rule_overridden = false;
                        workspaces[id].display_manually_selected = false;
                    }
                    self.reconcile_workspace_placements(
                        changes,
                        &self.active_spaces.clone(),
                        &self.space_state.screens.clone(),
                        false,
                        Some(&activated),
                        &[],
                        &[],
                    );
                    if visible_pin_changed && !self.refreshes_blocked() {
                        self.balance_display_workspaces(changes, &[], &HashSet::default());
                    }
                }
                return Ok(());
            }
            Event::Command(Command::Metrics(cmd)) => {
                crate::common::log::handle_command(cmd);
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::OpenSettings)) => {
                if let Some(tx) = &self.menu_manager.menu_tx {
                    tx.send(menu_bar::Event::OpenSettings);
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::Debug)) => {
                for space in self.space_state.iter_known_spaces() {
                    self.layout_manager.layout_engine.debug_tree_desc(space, "", true);
                }
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::SaveAndExit)) => {
                if let Err(error) = self.layout_manager.layout_engine.save_current_layout(
                    crate::common::config::restore_file(),
                    &self.state.windows,
                    self.active_display_space(),
                ) {
                    tracing::error!("Could not save master file: {error}");
                    // Keep the live layout available for retry if the save fails.
                    println!("Could not save master file; Rift is still running: {error}");
                    return Ok(());
                }
                std::process::exit(0);
            }
            Event::Command(Command::Reactor(ReactorCommand::SaveLayout { path })) => {
                self.layout_manager.layout_engine.save_current_layout(
                    path.clone(),
                    &self.state.windows,
                    self.active_display_space(),
                )?;
                tracing::info!(path = %path.display(), "Saved layout");
                println!("Saved layout to {}", path.display());
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::RestoreLayout {
                path,
                scope,
                source,
            })) => {
                let Some(active_space) = self.active_display_space() else {
                    println!("Could not restore saved layout: no active macOS space is available");
                    return Ok(());
                };
                let request = layout::RestoreRequest { scope, active_space, source };
                let visible: Vec<_> = self.iter_active_spaces().collect();
                let report = self.layout_manager.layout_engine.restore_layout(
                    path,
                    request,
                    &visible,
                    &mut self.state.windows,
                    &self.config.settings.layout,
                );
                match report {
                    Ok(report) => {
                        for &(window, from, to) in &report.moved {
                            // A restored floating frame is already on the display it returns to.
                            let floating =
                                self.layout_manager.layout_engine.is_window_floating(window);
                            let source = if floating { to } else { from };
                            if floating {
                                self.state.windows.clear_floating_display_geometry(window);
                            }
                            let source = self.space_state.screen_by_space(source).map(|s| s.frame);
                            self.request_window_transfer(changes, window, to, source);
                        }
                        changes.arrange(None, Presentation::Animated);
                        changes.membership_for(None);
                        changes.refresh_notifications = true;
                        println!("{}", report.summary());
                    }
                    Err(error) => {
                        tracing::error!(?scope, %error, "Could not restore saved layout");
                        println!("Could not restore saved layout: {error}");
                    }
                }
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::Serialize)) => {
                println!("{}", self.serialize_state()?);
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::SwitchSpace(direction))) => {
                unsafe { window_server::switch_space(direction) };
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::ToggleSpaceActivated)) => {
                self.toggle_space(changes);
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::BindingMode(mode))) => {
                if let Some(input_tx) = &self.communication_manager.input_tx {
                    input_tx.send(input::Request::SetBindingMode(mode));
                }
            }
            Event::Command(Command::Reactor(ReactorCommand::ShowMissionControlAll)) => {
                self.mission_control_command(
                    crate::actor::wm_controller::WmCmd::ShowMissionControlAll,
                );
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::ShowMissionControlCurrent)) => {
                self.mission_control_command(
                    crate::actor::wm_controller::WmCmd::ShowMissionControlCurrent,
                );
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::DismissMissionControl)) => {
                self.mission_control_command(
                    crate::actor::wm_controller::WmCmd::DismissMissionControl,
                );
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::CloseWindow { window_server_id })) => {
                let server = window_server_id.map(WindowServerId::new);
                let target = server.map_or_else(
                    || self.main_window(),
                    |id| self.state.windows.tracked_window_id(id),
                );
                if let Some(window) = target {
                    self.request_close_window(window.pid, server);
                } else {
                    warn!(?server, "Close target not found");
                }
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::FocusWindow {
                window_id,
                window_server_id,
            })) => {
                let window_id = WindowId::new(window_id.pid, window_id.idx);
                let window_server_id = window_server_id.map(WindowServerId::new);
                let resolved_space = self.best_space_for_window_id(window_id).or_else(|| {
                    self.state.windows.window(window_id).and_then(|window| {
                        self.best_space_for_window(&window.frame_monotonic, window.info.sys_id)
                    })
                });
                self.focus_window(changes, window_id, window_server_id, resolved_space);
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::MoveMouseToDisplay(selector))) => {
                self.focus_display(changes, selector, true);
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::FocusDisplay(selector))) => {
                self.focus_display(changes, selector, false);
                return Ok(());
            }
            Event::Command(Command::Layout(command)) => {
                self.dispatch_layout_command(changes, command, self.command_context_space(), false);
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::MoveWindowToDisplay {
                selector,
                window_id,
            })) => {
                if self.is_in_drag() {
                    warn!("Ignoring move-window-to-display while a drag is active");
                    return Ok(());
                }
                let command_space = self.workspace_command_space();
                let windows = &self.state.windows;
                let workspaces = self.layout_manager.layout_engine.workspaces();
                let resolved_window = match window_id {
                    Some(index) => workspaces.resolve_window_index(
                        windows,
                        command_space,
                        |space| self.is_space_active(space),
                        index,
                    ),
                    None => self.main_window().or_else(|| self.window_id_under_cursor()),
                };
                let Some((window, source_space)) = resolved_window
                    .filter(|&window| windows.can_move_assigned_window(window))
                    .and_then(|window| Some((window, self.assigned_space_for_window_id(window)?)))
                else {
                    warn!("Move window to display ignored: no unique eligible assigned window");
                    return Ok(());
                };
                let origin = self
                    .space_state
                    .screen_by_space(source_space)
                    .map(|screen| screen.frame.mid())
                    .or_else(|| self.current_screen_center());
                let Some(target_screen) = self.screen_for_selector(&selector, origin).cloned()
                else {
                    warn!(
                        ?selector,
                        "Move window to display ignored: target display not found"
                    );
                    return Ok(());
                };
                let Some(target_space) =
                    target_screen.space.filter(|space| self.is_space_active(*space))
                else {
                    warn!(
                        ?selector,
                        "Move window to display ignored: target space unavailable"
                    );
                    return Ok(());
                };
                if source_space == target_space {
                    return Ok(());
                }
                if self.ensure_window_move_destination(changes, target_space).is_some() {
                    // Fence retained scrolling frames before installing the destination frame.
                    self.cancel_window_presentations(vec![window]);
                    self.move_window_to_display(changes, window, source_space, target_space);
                }
                return Ok(());
            }
            Event::Command(Command::Reactor(ReactorCommand::MoveWorkspaceToDisplay {
                selector,
                wrap_around,
            })) => {
                if self.is_in_drag() {
                    warn!("Ignoring move-workspace-to-display while a drag is active");
                    return Ok(());
                }
                let Some(source_space) = self.workspace_command_space() else {
                    warn!("Move workspace to display ignored: source space unavailable");
                    return Ok(());
                };
                let origin = self
                    .space_state
                    .screen_by_space(source_space)
                    .map(|screen| screen.frame.mid())
                    .or_else(|| self.current_screen_center());
                let target_screen = if wrap_around {
                    self.screen_for_selector_wrapping(&selector, origin)
                } else {
                    self.screen_for_selector(&selector, origin)
                };
                let Some(target_screen) = target_screen.cloned() else {
                    warn!(
                        ?selector,
                        "Move workspace to display ignored: target display not found"
                    );
                    return Ok(());
                };
                let Some(target_space) =
                    target_screen.space.filter(|space| self.is_space_active(*space))
                else {
                    warn!(
                        ?selector,
                        "Move workspace to display ignored: target space unavailable"
                    );
                    return Ok(());
                };
                // An explicit destination overrides the pin even while its display
                // is unavailable; reconnecting must not undo the user's choice.
                let workspaces = self.layout_manager.layout_engine.workspaces();
                let rule_override = workspaces.active_workspace(source_space).and_then(|active| {
                    let (_, display) = workspaces.rule_displays().find(|(id, _)| *id == active)?;
                    let preferred = crate::sys::screen::resolve_display_preference(
                        display,
                        &self.space_state.screens,
                        |screen| {
                            screen.space.is_some_and(|space| self.active_spaces.contains(&space))
                        },
                    );
                    let overridden = preferred
                        .is_none_or(|screen| screen.display_uuid != target_screen.display_uuid);
                    Some((active, overridden))
                });

                if source_space == target_space {
                    if let Some((id, overridden)) = rule_override {
                        self.layout_manager.layout_engine.workspaces_mut()[id].rule_overridden =
                            overridden;
                    }
                    let response =
                        self.layout_manager.layout_engine.move_active_workspace_to_space(
                            &mut self.state.windows,
                            source_space,
                            target_space,
                            target_screen.frame.size,
                        );
                    self.reconcile_layout_response(changes, response, None);
                    return Ok(());
                }

                self.store_current_floating_positions(source_space);
                let response = self.layout_manager.layout_engine.move_active_workspace_to_space(
                    &mut self.state.windows,
                    source_space,
                    target_space,
                    target_screen.frame.size,
                );
                if response.changed {
                    self.workspace_switch_manager
                        .start_workspace_switch(target_space, WorkspaceSwitchOrigin::Manual);
                    changes.move_between(source_space, target_space, Presentation::WorkspaceSwitch);
                    changes.workspace_changed(source_space);
                    changes.workspace_changed(target_space);
                    let source =
                        self.space_state.screen_by_space(source_space).map(|screen| screen.frame);
                    for &window in &response.transferred_windows {
                        self.request_window_transfer(changes, window, target_space, source);
                    }
                }
                self.reconcile_layout_response(changes, response, Some(target_space));
                if let Some((id, overridden)) = rule_override {
                    let workspaces = self.layout_manager.layout_engine.workspaces_mut();
                    if workspaces[id].space == target_space {
                        workspaces[id].rule_overridden = overridden;
                    }
                }
                return Ok(());
            }
            _ => (),
        }

        self.reconcile_observed_focus(changes, raised_window);
        changes.refresh_notifications |= should_update_notifications;
        Ok(())
    }

    fn infer_left_mouse_up(&mut self, changes: &mut Changes) {
        let button = crate::actor::drag::MouseButton::Left;
        let event = Event::MouseUp(button);
        self.log_event(&event);
        self.recording_manager.record.on_event(&event);
        self.suppress_auto_workspace_switch_until_input = false;
        self.reconcile_mouse_up(changes, button);
    }

    fn reconcile_mouse_up(
        &mut self,
        changes: &mut Changes,
        button: crate::actor::drag::MouseButton,
    ) {
        if self.is_mission_control_active()
            || (self.is_in_drag() && !self.drag_manager.actor.owns_button(button))
        {
            return;
        }
        self.finish_drag(changes, button);
        if let Some(window) = self.window_id_under_cursor()
            && let Some(space) = self
                .assigned_space_for_window_id(window)
                .or_else(|| self.best_space_for_window_id(window))
        {
            self.reconcile_layout_event(changes, LayoutEvent::WindowFocused(space, window));
        }
    }

    fn reconcile_space_snapshot(&mut self, changes: &mut Changes, snapshot: ForwardedSpaceState) {
        if !snapshot.authoritative {
            return;
        }
        if self.space_revision_superseded(snapshot.revision) {
            // A held older sample contributes deltas only.
            if self.refreshes_blocked() {
                self.carry_superseded_space_deltas(snapshot);
            }
            return;
        }
        let recovering = self.refreshes_blocked();
        if recovering {
            self.abandon_window_inventories_from_instability();
        } else if snapshot.revision != self.space_state.revision {
            self.window_inventory_manager
                .pending
                .extend(self.window_inventory_manager.in_flight.keys().copied());
        }
        self.handle_authoritative_space_snapshot(changes, snapshot);
        if recovering {
            changes.flush_inventory = true;
            changes.inventory_all = false;
        }
    }

    fn reconcile_observed_focus(&mut self, changes: &mut Changes, window: Option<WindowId>) {
        if let Some(window) = window
            && let Some(space) = self
                .best_space_for_window_id(window)
                .filter(|space| self.is_space_active(*space))
        {
            self.reconcile_layout_event(changes, LayoutEvent::WindowFocused(space, window));
        }
    }

    fn write_window_frame(
        &mut self,
        window: WindowId,
        frame: CGRect,
        interactive: bool,
        bounds: Option<managers::ScrollingDisplayBounds>,
    ) -> bool {
        if self.refreshes_blocked()
            || (interactive && !self.is_in_drag())
            || (self.is_dragging_window(window) && !interactive)
            || !self.state.windows.can_place_window(window)
        {
            return false;
        }
        let Some(app) = self.app_manager.apps.get(&window.pid) else {
            return false;
        };
        if !app.handle.can_write_frames() {
            return false;
        }
        let window_server_id =
            self.state.windows.window(window).and_then(|window| window.info.sys_id);
        let transaction = if let Some(window_server_id) = window_server_id {
            self.transaction_manager.begin(window_server_id, frame)
        } else {
            TransactionId::default()
        };
        if interactive {
            app.handle.send_interactive_frame(
                window,
                frame,
                false,
                transaction,
                crate::actor::app::FrameSource::Drag,
            );
        } else {
            let bounded;
            let handle = if bounds.is_some() {
                bounded = app.handle.with_scrolling_bounds(bounds);
                &bounded
            } else {
                &app.handle
            };
            if let Err(error) =
                handle.send(Request::set_window_frame(window, frame, transaction, true))
            {
                warn!(window = ?window, %error, "failed to write requested window frame");
                return false;
            }
        }
        true
    }

    fn submit_raise(&self, request: raise_manager::Event) {
        trace!(?request, "Submitting native raise or focus");
        if let Err(error) = self.communication_manager.raise_manager_tx.try_send(request) {
            warn!(%error, "failed to send raise request");
        }
    }

    fn send_app_request(&self, pid: pid_t, request: Request) {
        if let Some(app) = self.app_manager.apps.get(&pid)
            && let Err(error) = app.handle.send(request)
        {
            warn!(pid, %error, "failed to send application request");
        }
    }

    fn update_service_config(&mut self) {
        let config = &self.config;
        if let Some(tx) = &self.communication_manager.stack_line_tx
            && let Err(error) = tx.try_send(stack_line::Event::ConfigUpdated(config.clone()))
        {
            warn!(%error, "failed to update stack line config");
        }
        if let Some(tx) = &self.menu_manager.menu_tx
            && let Err(error) =
                tx.try_send(menu_bar::Event::ConfigUpdated(Box::new(config.clone())))
        {
            warn!(%error, "failed to update menu bar config");
        }
        if let Some(wm) = &self.communication_manager.wm_sender {
            wm.send(WmEvent::ConfigUpdated(config.clone()));
        }
    }

    fn finish(
        &mut self,
        mut changes: Changes,
        focused_window: Option<WindowId>,
        error: Option<anyhow::Error>,
    ) {
        if !self.mouse_raise_token.is_cancelled() && self.transient_blocks_mouse_focus() {
            self.mouse_raise_token.cancel();
            let _ = self
                .communication_manager
                .raise_manager_tx
                .try_send(raise_manager::Event::MouseRaisesCancelled);
        }

        self.retire_presentations();
        self.resolve_fallback_focus(&mut changes);
        if changes.focus.is_none()
            && let Some(space) = changes.desktop_focus
        {
            self.focus_desktop_if_active_workspace_empty(space);
        }
        let dragging = self.is_in_drag();
        let layout_modes_may_have_changed = changes.has_layouts();
        let blocked = self.refreshes_blocked() || self.is_mission_control_active();
        if dragging {
            changes.defer_drag_layouts(|space, policy, changed| {
                self.drag_manager.defer_layout(space, policy, changed)
            });
            if !blocked && changes.has_layouts() {
                // A permitted commit covers earlier work for this space at the same
                // or a weaker presentation policy. Keep global requests deferred.
                self.drag_manager.deferred_layouts.retain(|&(scope, policy, changed)| {
                    let Some(space) = scope else { return true };
                    if changes.for_space(space).presentation < Some(policy)
                        || !self.active_spaces.contains(&space)
                        || self.space_state.screen_by_space(space).is_none()
                    {
                        return true;
                    }
                    changes.space(space).layout_changed |= changed;
                    false
                });
            }
        } else if !blocked {
            for (space, policy, changed) in self.drag_manager.deferred_layouts.drain(..) {
                changes.arrange(space, policy).layout_changed |= changed;
            }
        }
        let arrange = !blocked && changes.has_layouts();
        let layout_changed = arrange && LayoutManager::commit(self, &mut changes);
        if arrange && dragging {
            self.refresh_active_drag_scene();
        }
        for &(space, change) in changes.spaces() {
            if change.tabs_changed && (!arrange || changes.for_space(space).presentation.is_none())
            {
                LayoutManager::refresh_stack_tabs(self, space);
            }
        }
        if arrange || changes.refresh_menu {
            self.maybe_send_menu_update();
        }
        if layout_changed && changes.drop_haptic && !cfg!(test) {
            let _ = crate::sys::haptics::perform_haptic(
                crate::common::config::HapticPattern::LevelChange,
            );
        }
        if layout_changed
            && let Some(window) = changes.post_arrange_mouse_warp
            && let Some(center) = self.window_center_on_known_screen(window)
        {
            self.warp_mouse(center);
        }
        if let Some(focus) = changes.focus.take() {
            self.submit_raise(focus);
        }
        if let Some(point) = changes.mouse_warp {
            self.warp_mouse(point);
        }

        if let Some((workspace, window, stamp)) = changes.overview_activation
            && let Some(wm) = &self.communication_manager.wm_sender
        {
            let frames = self
                .state
                .windows
                .workspace_windows(workspace)
                .into_iter()
                .filter_map(|id| {
                    let state = self.state.windows.window(id)?;
                    (state.is_admitted() && !state.info.is_minimized && state.info.is_standard)
                        .then_some((id, state.frame_monotonic))
                })
                .collect();
            wm.send(WmEvent::OverviewActivationTarget(OverviewTarget {
                workspace,
                window,
                stamp,
                frames,
            }));
        }
        if (changes.overview_changed
            || changes.refresh_notifications
            || changes.selection_changed.is_some()
            || focused_window.is_some()
            || layout_changed)
            && self.config.settings.ui.mission_control.enabled
            && let Some(wm) = &self.communication_manager.wm_sender
        {
            wm.send(crate::actor::wm_controller::WmEvent::OverviewChanged);
        }
        self.workspace_switch_manager.mark_workspace_switch_inactive();
        if self.workspace_switch_manager.active_workspace_switch.is_some() && !layout_changed {
            self.workspace_switch_manager.active_workspace_switch = None;
            trace!("Workspace switch stabilized with no further frame changes");
        }

        // Resolve focus requests that did not need arrangement, and relative display warps.
        let focused = changes.focus_warp.take();
        if let Some(space) = changes.display_warp.take() {
            // Land at the same spot within the newly focused window that the cursor
            // held in the old one, or the display itself when the workspace is empty.
            let to = focused
                .or_else(|| self.layout_manager.layout_engine.selected_window(space))
                .and_then(|wid| self.window_frame_on_known_screen(wid))
                .or_else(|| self.space_state.screen_by_space(space).map(|screen| screen.frame));
            let from = self
                .main_window()
                .and_then(|wid| self.state.windows.window(wid))
                .map(|window| window.frame_monotonic);
            if let Some(to) = to
                && let Some(input_tx) = &self.communication_manager.input_tx
            {
                _ = input_tx.send(crate::actor::input::Request::WarpRelative { from, to });
            }
        } else if let Some(center) = focused.and_then(|wid| self.window_center_on_known_screen(wid))
        {
            self.warp_mouse(center);
        }

        if changes.refresh_notifications {
            let mut ids: Vec<u32> = self
                .state
                .windows
                .iter_tracked_window_server_ids()
                .chain(self.transient_focus.windows.keys().copied())
                .map(|wsid| wsid.as_u32())
                .collect();
            ids.sort_unstable();
            ids.dedup();

            if ids != self.notification_manager.last_sls_notification_ids {
                crate::sys::window_notify::update_window_notifications(&ids);

                self.notification_manager.last_sls_notification_ids = ids;
            }
        }
        if changes.refresh_ffm {
            self.update_focus_follows_mouse_state();
        }
        if layout_modes_may_have_changed || arrange {
            self.update_event_tap_layout_mode();
        }
        if changes.has_publications() && !self.communication_manager.event_broadcaster.is_closed() {
            for screen in &self.space_state.screens {
                let Some(space) = screen.space else { continue };
                let change = changes.for_space(space);
                if !arrange || !self.is_space_active(space) || change.presentation.is_none() {
                    self.publish_space_changes(space, change);
                }
            }
            for &(space, change) in changes.spaces() {
                if (change.workspace_changed || change.windows_changed)
                    && self.space_state.screen_by_space(space).is_none()
                {
                    self.publish_space_changes(space, change);
                }
            }
        }
        if let Some(space) = changes.selection_changed {
            self.broadcast_layout_state_changed(
                space,
                rift_protocol::EventKind::SelectionChanged,
                None,
            );
        }
        if let Some((window, old, new)) = changes.title_changed {
            self.broadcast_window_title_changed(window, old, new);
        }
        if let Some(window) = focused_window {
            self.broadcast_focused_window_changed(window);
        }
        if changes.inventory_all {
            self.request_window_inventories();
        }
        if changes.flush_inventory {
            self.flush_deferred_window_inventory_refresh();
        }
        for pid in changes.inventory.into_iter().chain(changes.extra_inventory) {
            self.request_window_inventory(pid);
        }
        if let Some(error) = error {
            warn!(%error, "reactor workflow failed");
        }
    }

    fn create_window_data(&self, window_id: WindowId) -> Option<RuntimeWindowData> {
        let window_state = self.state.windows.window(window_id)?;
        let externally_managed = self.state.windows.is_claimed(window_id);
        if (!self.state.windows.is_admitted(window_id)
            || self.state.windows.is_temporarily_unavailable(window_id))
            && !externally_managed
        {
            return None;
        }
        let app = self.app_manager.apps.get(&window_id.pid)?;

        let app_name = app.info.localized_name.clone();
        let bundle_id = app.info.bundle_id.clone();

        Some(RuntimeWindowData {
            layout_frame: None,
            id: window_id,
            is_floating: self.layout_manager.layout_engine.is_window_floating(window_id),
            externally_managed,
            is_focused: self.main_window() == Some(window_id),
            layout_position: None,
            app_name,
            info: WindowInfo {
                title: window_state.info.title.clone(),
                frame: window_state.frame_monotonic,
                bundle_id,
                ..window_state.info.clone()
            },
        })
    }

    fn update_complete_window_server_info(
        &mut self,
        changes: &mut Changes,
        ws_info: Vec<WindowServerInfo>,
    ) {
        self.state.windows.clear_visible_windows();
        self.update_partial_window_server_info(changes, ws_info);
    }

    fn update_partial_window_server_info(
        &mut self,
        changes: &mut Changes,
        ws_info: impl IntoIterator<Item = WindowServerInfo>,
    ) {
        for info in ws_info {
            if let Some(wid) = self.state.windows.observe_native_window(info) {
                let presence = self
                    .state
                    .windows
                    .native_space(wid)
                    .or_else(|| self.assigned_space_for_window_id(wid));
                self.reconcile_native_admission(changes, wid, true, presence);
            }
        }
    }

    fn request_window_inventories(&mut self) {
        // AX discovery remains the source of truth for enumerating app windows.
        // Native-space membership/visibility is supplied separately by the spaces
        // actor; do not replace this with the global CG on-screen window list.
        if self.refreshes_blocked() {
            self.defer_window_inventory_refresh();
            return;
        }

        let pids: Vec<_> = self.app_manager.apps.keys().copied().collect();
        for pid in pids {
            if !self
                .window_inventory_manager
                .in_flight
                .get(&pid)
                .is_some_and(|token| token.topology_revision == self.space_state.revision)
            {
                self.request_window_inventory(pid);
            }
        }
    }

    /// Recover confirmed exits once, retaining the record while AX is unavailable.
    fn reconcile_native_fullscreen_exits(&mut self, changes: &mut Changes, query_membership: bool) {
        let exits: Vec<_> = self
            .state
            .windows
            .iter_native_fullscreen_records()
            .filter(|record| {
                matches!(record.transition, NativeFullscreenTransition::ExitObserved(_))
            })
            .collect();
        for record in exits {
            // Destruction alone, or a listing while some fullscreen Space (perhaps
            // a replacement) exists, is corroborated by one live membership query.
            // Discovery already queried; it waits for the next accepted topology.
            if record.transition == NativeFullscreenTransition::ExitObserved(None)
                || !self.space_state.fullscreen_spaces.is_empty()
            {
                if !query_membership {
                    continue;
                }
                if let Some(wsid) = record.window_server_id {
                    let membership = window_server::window_spaces(wsid);
                    if let Some(space) =
                        membership.iter().copied().find(|space| self.is_fullscreen_space(*space))
                    {
                        // macOS can replace a fullscreen Space during display churn
                        // without the window ever returning to normal management.
                        self.state.windows.observe_native_fullscreen(
                            wsid,
                            space,
                            record.last_known_user_space,
                            None,
                        );
                        continue;
                    }
                    if membership.iter().any(|space| {
                        *space == record.fullscreen_space
                            || !self
                                .space_state
                                .display_space_ids
                                .values()
                                .any(|spaces| spaces.contains(space))
                    }) {
                        continue;
                    }
                    if membership.len() == 1 {
                        self.state
                            .windows
                            .note_fullscreen_exit(record.current_window_id, membership[0]);
                    }
                }
            }
            self.restore_native_fullscreen(
                changes,
                record.window_server_id,
                record.current_window_id,
                None,
            );
        }
    }

    fn is_fullscreen_space(&self, space: SpaceId) -> bool {
        self.space_state.fullscreen_spaces.contains(&space)
    }

    fn finalize_space_change(
        &mut self,
        changes: &mut Changes,
        spaces: &[Option<SpaceId>],
        active_windows: Vec<(WindowServerId, Option<SpaceId>)>,
        preserve_missing_assignments: bool,
        invalidated_spaces: &[SpaceId],
    ) {
        self.refocus_manager.stale_cleanup_state = if spaces.iter().all(|space| space.is_none()) {
            StaleCleanupState::Suppressed
        } else {
            StaleCleanupState::Enabled
        };
        if let Some(main_window) = self.main_window() {
            if let Some(space) = self.main_window_space() {
                self.reconcile_layout_event(
                    changes,
                    LayoutEvent::WindowFocused(space, main_window),
                );
            }
        }
        self.reconcile_authoritative_active_window_snapshot(
            changes,
            active_windows,
            preserve_missing_assignments,
            invalidated_spaces,
        );
        changes.inventory_all = true;

        if let Some(space) = self.workspace_command_space() {
            changes.desktop_focus = Some(space);
        }
        if let Some(space) = self
            .workspace_command_space()
            .or_else(|| spaces.iter().copied().flatten().find(|space| self.is_space_active(*space)))
        {
            changes.space(space).workspace_changed = true;
        }
    }

    fn publish_space_changes(&self, space: SpaceId, change: SpaceChange) {
        if (!change.workspace_changed && !change.windows_changed)
            || self.communication_manager.event_broadcaster.is_closed()
        {
            return;
        }
        let engine = &self.layout_manager.layout_engine;
        let Some(workspace) = engine.workspaces().active_workspace(space) else {
            return;
        };
        let workspace_name = &engine.workspaces()[workspace].name;
        let workspace_id = protocol_workspace_id(workspace);
        let display_uuid = self
            .display_uuid_for_space(space)
            .or_else(|| engine.display_uuid_for_space(space).map(str::to_owned));
        if change.workspace_changed {
            self.publish(BroadcastEvent::WorkspaceChanged {
                workspace_id,
                workspace_name: workspace_name.clone(),
                space_id: space.get(),
                display_uuid: display_uuid.clone(),
            });
        }
        if change.windows_changed {
            let windows = engine
                .workspaces()
                .windows_in_active_workspace(&self.state.windows, space)
                .iter()
                .map(WindowId::to_debug_string)
                .collect();
            self.publish(BroadcastEvent::WindowsChanged {
                workspace_id,
                workspace_name: workspace_name.clone(),
                windows,
                space_id: space.get(),
                display_uuid,
            });
        }
    }

    fn publish(&self, event: BroadcastEvent) {
        trace!(kind = ?event.kind(), "Publishing reactor state");
        self.communication_manager.event_broadcaster.send(event);
    }

    fn broadcast_window_title_changed(
        &mut self,
        window_id: WindowId,
        previous_title: String,
        new_title: String,
    ) {
        if previous_title != new_title
            && let Some(space) = self.best_space_for_window_id(window_id)
            && self.is_space_active(space)
            && let Some(workspace_id) =
                self.layout_manager.layout_engine.workspaces().active_workspace(space)
        {
            let workspace_index =
                self.layout_manager.layout_engine.workspaces().active_workspace_idx(space);

            let workspace_name = self
                .layout_manager
                .layout_engine
                .workspace_name(space, workspace_id)
                .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));

            let display_uuid = self.display_uuid_for_space(space);

            let event = BroadcastEvent::WindowTitleChanged {
                window_id: protocol_window_id(window_id),
                workspace_id: protocol_workspace_id(workspace_id),
                workspace_index,
                workspace_name,
                previous_title,
                new_title,
                space_id: space.get(),
                display_uuid,
            };
            self.publish(event);
        }
    }

    fn broadcast_focused_window_changed(&self, window_id: WindowId) {
        if let Some(space) = self.best_space_for_window_id(window_id)
            && self.is_space_active(space)
            && let Some(workspace_id) =
                self.layout_manager.layout_engine.workspaces().active_workspace(space)
        {
            let workspace_index =
                self.layout_manager.layout_engine.workspaces().active_workspace_idx(space);
            let workspace_name = self
                .layout_manager
                .layout_engine
                .workspace_name(space, workspace_id)
                .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));
            let display_uuid = self.display_uuid_for_space(space);

            let event = BroadcastEvent::FocusedWindowChanged {
                window_id: protocol_window_id(window_id),
                workspace_id: protocol_workspace_id(workspace_id),
                workspace_index,
                workspace_name,
                space_id: space.get(),
                display_uuid,
            };
            self.publish(event);
        }
    }

    fn broadcast_layout_state_changed(
        &self,
        space: SpaceId,
        kind: rift_protocol::EventKind,
        frames: Option<&[(WindowId, CGRect)]>,
    ) {
        if !self.communication_manager.event_broadcaster.is_closed()
            && self.is_space_active(space)
            && let Some(workspace_id) =
                self.layout_manager.layout_engine.workspaces().active_workspace(space)
            && let Some(layout) =
                self.query_layout_state_with_frames(Some(space.get()), None, frames)
        {
            let workspace_index =
                self.layout_manager.layout_engine.workspaces().active_workspace_idx(space);
            let workspace_name = self
                .layout_manager
                .layout_engine
                .workspace_name(space, workspace_id)
                .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));
            let workspace_id = protocol_workspace_id(workspace_id);
            let space_id = space.get();
            let display_uuid = self.display_uuid_for_space(space);
            let event = match kind {
                rift_protocol::EventKind::LayoutChanged => BroadcastEvent::LayoutChanged {
                    workspace_id,
                    workspace_index,
                    workspace_name,
                    space_id,
                    display_uuid,
                    layout,
                },
                rift_protocol::EventKind::SelectionChanged => BroadcastEvent::SelectionChanged {
                    workspace_id,
                    workspace_index,
                    workspace_name,
                    space_id,
                    display_uuid,
                    layout,
                },
                _ => return,
            };
            self.publish(event);
        }
    }

    fn maybe_reapply_app_rules_for_window(&mut self, changes: &mut Changes, window_id: WindowId) {
        if !self.config.virtual_workspaces.reapply_app_rules_on_title_change {
            return;
        }

        let app_info = match self.app_manager.apps.get(&window_id.pid) {
            Some(app_state) => app_state.info.clone(),
            None => return,
        };

        self.process_windows_for_app_rules(changes, [window_id], app_info, true);
    }

    fn handle_authoritative_space_snapshot(
        &mut self,
        changes: &mut Changes,
        mut space_state: ForwardedSpaceState,
    ) {
        if let Some(pending) = self.pending_space_change_manager.pending_space_change.take() {
            // Keep accepted display continuity while the virtual model is deferred.
            // Geometry and membership always come from the newest revision.
            space_state = topology_workflow::merge_space_deltas(pending, space_state);
        }
        if self.is_mission_control_active() {
            self.space_state.authoritative = false;
            self.pending_space_change_manager.pending_space_change = Some(space_state);
            return;
        }
        let analysis = topology_workflow::analyze_space_snapshot(
            &self.space_state,
            &self.active_spaces,
            &self.space_activation_policy,
            self.activation_cfg(),
            &space_state,
        );
        // Compare all native Spaces, including inactive ones. A current-Space
        // switch alone does not invalidate ownership.
        let sole_display = match (
            self.space_state.screens.as_slice(),
            space_state.screens.as_slice(),
        ) {
            ([previous], [current]) if previous.display_uuid == current.display_uuid => {
                current.display_uuid_opt()
            }
            _ => None,
        };
        let mut invalidated_spaces = Vec::new();
        let mut lost_local_desktops = Vec::new();
        if space_state.display_set_changed || space_state.should_force_refresh_layout {
            for (display, spaces) in &self.space_state.display_space_ids {
                let current = space_state.display_space_ids.get(display);
                for &space in spaces {
                    if current.is_some_and(|ids| ids.contains(&space))
                        || space_state.space_remaps.iter().any(|(old, _)| *old == space)
                    {
                        continue;
                    }
                    invalidated_spaces.push(space);
                    // The native inventory also knows the owners of unvisited desktops.
                    if sole_display == Some(display.as_str())
                        && !space_state.display_space_ids.values().any(|ids| ids.contains(&space))
                    {
                        lost_local_desktops.push(space);
                    }
                }
            }
        }
        if !space_state.membership_complete && !invalidated_spaces.is_empty() {
            // Retry exhaustion is not evidence of ownership. SpacesActor owns paced
            // resampling; retain the logical model until membership is observable.
            self.space_state.authoritative = false;
            self.pending_space_change_manager.pending_space_change = Some(space_state);
            return;
        }
        self.space_state.revision = space_state.revision;
        self.state.windows.accept_topology_revision(space_state.revision);
        self.space_state.authoritative = true;
        if self.spaces_tx.is_some() {
            for app in self.app_manager.apps.values_mut() {
                app.handle.set_topology_authority(&self.topology_guard, space_state.revision);
            }
        }
        let ForwardedSpaceState {
            screens,
            fullscreen_spaces,
            destroyed_spaces,
            active_spaces,
            menu_bar_space,
            command_space,
            display_space_ids,
            space_remaps,
            display_set_changed,
            should_force_refresh_layout,
            membership_complete,
            resized_spaces,
            active_window_spaces,
            ..
        } = space_state;
        self.space_state.active_window_spaces = active_window_spaces;
        self.space_state.membership_complete = membership_complete;
        let activation_config = self.activation_cfg();
        let topology_workflow::SpaceSnapshotAnalysis {
            spaces,
            authoritative_spaces,
            command_space_only_update,
            invalidates_pending_targets,
        } = analysis;

        let current_display_spaces = screens
            .iter()
            .filter_map(|screen| {
                screen
                    .space
                    .map(|space| (space, screen.display_uuid.clone(), screen.frame.size))
            })
            .collect::<Vec<_>>();
        self.layout_manager.layout_engine.resolve_saved_spaces(
            &mut self.state.windows,
            &current_display_spaces,
            screens.len(),
            &display_space_ids,
        );

        let empty = HashMap::default();
        self.state.windows.note_fullscreen_exits(
            &fullscreen_spaces,
            &destroyed_spaces,
            if membership_complete {
                &self.space_state.active_window_spaces
            } else {
                &empty
            },
        );
        self.space_state.fullscreen_spaces = fullscreen_spaces;
        self.space_state.active_spaces = active_spaces;
        if command_space_only_update {
            self.space_state.menu_bar_space = menu_bar_space;
            self.space_state.command_space = command_space;
            self.advance_workspace_transfers(changes);
            changes.refresh_menu = true;
            return;
        }
        if self.config.settings.ui.mission_control.enabled {
            changes.overview_changed = true;
        }
        if display_set_changed {
            if let Some(tx) = &self.animation_tx {
                let _ = tx.send(animation::Message::Displays(
                    screens.iter().map(|s| s.id.as_u32()).collect(),
                ));
            }
        }
        self.space_state.menu_bar_space = menu_bar_space;
        self.space_state.command_space = command_space;
        self.space_state.display_space_ids = display_space_ids;

        if screens.is_empty() {
            self.refocus_manager.stale_cleanup_state = StaleCleanupState::Suppressed;
            if !self.space_state.screens.is_empty() {
                self.space_state.screens.clear();
                self.expose_all_spaces();
            }
            self.recompute_and_set_active_spaces(changes, &[]);
            self.update_complete_window_server_info(changes, Vec::new());
            self.try_apply_pending_space_change(changes);
            return;
        }

        self.refocus_manager.stale_cleanup_state = StaleCleanupState::Enabled;
        let previous_screens = std::mem::replace(&mut self.space_state.screens, screens);
        // macOS may resize windows without AX notifications during display changes.
        // Reapply the complete layout even when cached frames still match.
        let force_frames = display_set_changed
            || previous_screens.iter().map(|s| (&s.display_uuid, s.frame)).ne(self
                .space_state
                .screens
                .iter()
                .map(|s| (&s.display_uuid, s.frame)));
        if invalidates_pending_targets {
            self.clear_pending_hidden_window_targets();
        }
        if display_set_changed || !space_remaps.is_empty() {
            self.state.windows.resume_deferred_transfers();
        }
        // Without structural change only a display whose Space changed is arranged,
        // and nothing for one now showing a fullscreen Space.
        changes.refresh_notifications = true;
        if force_frames {
            changes.arrange(None, Presentation::Force);
        } else if should_force_refresh_layout || invalidates_pending_targets {
            changes.arrange(None, Presentation::Animated);
        } else {
            for screen in &self.space_state.screens {
                if let Some(space) = screen.space
                    && !previous_screens.iter().any(|previous| {
                        previous.display_uuid == screen.display_uuid
                            && previous.space == screen.space
                    })
                {
                    changes.arrange(Some(space), Presentation::Animated);
                }
            }
        }
        let display_count = self.space_state.screens.len();
        // Preserve the shared catalog through remaps on either side of a display change.
        // A remap can replace a destination catalog only if both topologies are local.
        if display_count > 1 {
            self.layout_manager
                .layout_engine
                .workspaces_mut()
                .update_display_count(display_count);
        }
        for (previous_space, space) in space_remaps {
            if let Some(frame) = self.last_screen_frames.remove(&previous_space) {
                self.last_screen_frames.insert(space, frame);
            }
            self.space_activation_policy.on_space_remapped(previous_space, space);
            self.layout_manager.layout_engine.remap_space(
                &mut self.state.windows,
                previous_space,
                space,
            );
        }
        if display_count == 1 {
            self.layout_manager
                .layout_engine
                .workspaces_mut()
                .update_display_count(display_count);
        }
        for screen in &self.space_state.screens {
            let (Some(space), Some(display_uuid)) = (screen.space, screen.display_uuid_opt())
            else {
                continue;
            };
            self.layout_manager
                .layout_engine
                .update_space_display(space, Some(display_uuid));
        }
        self.space_activation_policy
            .on_spaces_updated(activation_config, &self.space_state.screens);
        let previously_active = self.set_active_spaces_from_snapshot(&authoritative_spaces);
        let engine = &self.layout_manager.layout_engine;
        let mut refits = Vec::new();
        for screen in &self.space_state.screens {
            let Some(space) = screen.space.filter(|space| self.active_spaces.contains(space))
            else {
                continue;
            };
            if let Some(&source) =
                self.last_screen_frames.get(&space).filter(|&&frame| frame != screen.frame)
            {
                refits.extend(
                    engine
                        .workspaces()
                        .workspaces_on(space)
                        .map(|workspace| (workspace, space, source, screen.frame)),
                );
            }
        }
        if display_set_changed
            || invalidates_pending_targets
            || previous_screens
                .iter()
                .map(|s| (s.display_uuid_opt(), s.space, s.frame))
                .ne(self
                    .space_state
                    .screens
                    .iter()
                    .map(|s| (s.display_uuid_opt(), s.space, s.frame)))
        {
            self.reconcile_workspace_placements(
                changes,
                &previously_active,
                &previous_screens,
                invalidates_pending_targets,
                None,
                &invalidated_spaces,
                &lost_local_desktops,
            );
        }
        // Transfers already fitted their frames; refit only workspaces whose native
        // desktop stayed put while its display geometry changed.
        for (workspace, space, source, target) in refits {
            let engine = &mut self.layout_manager.layout_engine;
            if engine.workspaces().workspace_space(workspace) != Some(space) {
                continue;
            }
            for window in self.state.windows.workspace_windows(workspace) {
                if !engine.is_window_floating(window)
                    || self.state.windows.native_fullscreen_record_for_window(window).is_some()
                {
                    continue;
                }
                if let Some(frame) = engine.get_floating_position(workspace, window).or_else(|| {
                    self.state
                        .windows
                        .can_place_window(window)
                        .then(|| self.state.windows.record(window)?.frame())
                        .flatten()
                }) {
                    let projected = self.state.windows.project_floating_frame(
                        window,
                        frame,
                        Some(source),
                        target,
                    );
                    if projected != frame {
                        engine.store_floating_position(workspace, window, projected);
                        changes.arrange(Some(space), Presentation::Animated);
                    }
                }
            }
        }
        // Initialize the shared catalog before any individual Space can initialize it.
        self.expose_all_spaces();
        self.advance_workspace_transfers(changes);
        self.reconcile_native_fullscreen_exits(changes, true);

        for (space, _) in resized_spaces {
            if self.is_space_active(space) {
                changes.arrange(Some(space), Presentation::Force);
            }
        }
        let active_windows = self.authoritative_active_space_windows();
        self.finalize_space_change(
            changes,
            &spaces,
            active_windows,
            !membership_complete,
            &invalidated_spaces,
        );
        self.handle_active_space_change(changes, &previously_active);
        self.try_apply_pending_space_change(changes);
        self.last_screen_frames.extend(
            self.space_state
                .screens
                .iter()
                .filter_map(|screen| Some((screen.space?, screen.frame))),
        );
    }

    /// Global-mode visibility is derived here from the accepted snapshot and activation policy.
    /// A connected display on a different desktop is dormant, never a display departure.
    fn reconcile_workspace_placements(
        &mut self,
        changes: &mut Changes,
        previously_active: &HashSet<SpaceId>,
        previous_screens: &[ScreenInfo],
        preserve_membership: bool,
        pins_only: Option<&HashSet<crate::model::VirtualWorkspaceId>>,
        invalidated_spaces: &[SpaceId],
        lost_local_desktops: &[SpaceId],
    ) {
        use crate::model::virtual_workspace::WorkspaceTransferReason;
        if self.refreshes_blocked() {
            return;
        }
        if self.layout_manager.layout_engine.workspaces().settings.mode
            != crate::common::config::WorkspaceMode::Global
        {
            return;
        }
        let screens = &self.space_state.screens;
        let live = |screen: &ScreenInfo| {
            screen.space.is_some_and(|space| self.active_spaces.contains(&space))
        };
        // Native desktop switches (including entering fullscreen) do not change
        // physical ownership. Retry deferred pins when a user desktop returns.
        let resolve_pins = pins_only.is_some()
            || preserve_membership
            || screens.len() != previous_screens.len()
            || screens.iter().any(|screen| {
                previous_screens
                    .iter()
                    .find(|old| old.display_uuid == screen.display_uuid)
                    .is_none_or(|old| {
                        old.frame != screen.frame || (old.space.is_none() && screen.space.is_some())
                    })
            });
        // Plan only from the accepted topology. Switching away from a desktop
        // still in the native inventory is dormancy; losing it is departure,
        // even when its display stays connected and borrows another desktop.
        let placements: Vec<_> = {
            let engine = &self.layout_manager.layout_engine;
            // Resolve each configured display once, rather than scanning all pins
            // and displays again for every workspace.
            let mut rule_targets = HashMap::default();
            engine
                .workspaces()
                .catalog()
                .enumerate()
                .filter(|(_, id)| pins_only.is_none_or(|ids| ids.contains(id)))
                .filter_map(|(index, id)| {
                    let workspace = &engine.workspaces()[id];
                    if lost_local_desktops.contains(&workspace.space) {
                        return None;
                    }
                    let owner = engine.display_uuid_for_space(workspace.space);
                    let home = workspace
                        .home_space
                        .zip(workspace.preferred_display.as_deref())
                        .filter(|_| pins_only.is_none());
                    let native_owner =
                        self.space_state.display_space_ids.iter().find_map(|(display, spaces)| {
                            spaces.contains(&workspace.space).then_some(display.as_str())
                        });
                    let desktop_lost =
                        invalidated_spaces.contains(&workspace.space) && native_owner.is_none();
                    let home_target = home.and_then(|(space, display)| {
                        let target = screens.iter().position(|screen| {
                            live(screen) && screen.display_uuid_opt() == Some(display)
                        })?;
                        let returned = !previous_screens
                            .iter()
                            .any(|screen| screen.display_uuid_opt() == Some(display))
                            && self
                                .space_state
                                .display_space_ids
                                .get(display)
                                .is_some_and(|spaces| !spaces.contains(&space));
                        (screens[target].space == Some(space) || returned).then_some(target)
                    });
                    let rehome =
                        home_target.filter(|&target| screens[target].space != workspace.home_space);
                    let ruled = engine
                        .workspaces()
                        .matching_rules(index, &workspace.name)
                        .find_map(|rule| rule.display.as_deref())
                        .filter(|_| !workspace.rule_overridden && screens.len() > 1)
                        .and_then(|rule_display| {
                            *rule_targets.entry(rule_display).or_insert_with(|| {
                                crate::sys::screen::resolve_display_preference(
                                    rule_display,
                                    screens,
                                    live,
                                )
                                .and_then(|target| {
                                    screens.iter().position(|screen| std::ptr::eq(screen, target))
                                })
                            })
                        });
                    let placement = if let Some(target) = ruled {
                        ((resolve_pins || desktop_lost)
                            && (native_owner.or(owner) != screens[target].display_uuid_opt()
                                || desktop_lost))
                            .then_some((
                                target,
                                if desktop_lost {
                                    WorkspaceTransferReason::DisplayDeparture
                                } else {
                                    WorkspaceTransferReason::Restoration
                                },
                            ))
                    } else if let Some(target) =
                        home_target.filter(|&target| screens[target].space != Some(workspace.space))
                    {
                        Some((target, WorkspaceTransferReason::Restoration))
                    } else if pins_only.is_none()
                        && (desktop_lost
                            || (home.is_some()
                                && owner.is_some_and(|display| {
                                    !screens
                                        .iter()
                                        .any(|screen| screen.display_uuid_opt() == Some(display))
                                        && (previously_active.contains(&workspace.space)
                                            || engine.last_space_for_display_uuid(display)
                                                == Some(workspace.space))
                                })))
                    {
                        screens
                            .iter()
                            .position(|screen| live(screen) && screen.display_uuid_opt() == owner)
                            .or_else(|| screens.iter().position(live))
                            .map(|target| (target, WorkspaceTransferReason::DisplayDeparture))
                    } else {
                        None
                    };
                    (rehome.is_some() || placement.is_some()).then_some((id, rehome, placement))
                })
                .collect()
        };
        // A native desktop switch on a connected display keeps its own catalog.
        let display_available =
            self.space_state.screens.iter().filter(|screen| live(screen)).any(|screen| {
                !previous_screens.iter().any(|old| {
                    old.display_uuid == screen.display_uuid
                        && (old.space.is_some()
                            || screen
                                .space
                                .is_some_and(|space| self.last_screen_frames.contains_key(&space)))
                })
            });
        // Commit ownership and remembered presentation together, then dispatch
        // native effects through the same boundary as commands and app rules.
        let mut restored = HashSet::default();
        for (id, rehome, placement) in placements {
            if let Some(target) = rehome {
                let workspaces = self.layout_manager.layout_engine.workspaces_mut();
                let old = workspaces[id].home_space.unwrap();
                let target = self.space_state.screens[target].space.unwrap();
                workspaces[id].home_space = Some(target);
                if let Some(activation) = workspaces.active_workspace_per_space.get_mut(&old)
                    && activation.remembered.or(activation.current) == Some(id)
                {
                    activation.remembered = None;
                    if workspaces[id].space == target {
                        workspaces.set_active_workspace(target, id);
                    } else {
                        workspaces
                            .active_workspace_per_space
                            .entry(target)
                            .or_default()
                            .remembered = Some(id);
                    }
                }
            }
            let Some((target, reason)) = placement else { continue };
            if reason == WorkspaceTransferReason::Restoration {
                restored.insert(id);
            }
            let target = self.space_state.screens[target].space.unwrap();
            self.place_workspace(changes, id, target, previous_screens, reason);
        }
        if pins_only.is_none() && display_available {
            self.balance_display_workspaces(changes, previous_screens, &restored);
        }
        // The topology handler exposes displays after restoration.
        // A config-only pin change has no topology finalization, so expose it here.
        if pins_only.is_some() {
            self.expose_all_spaces();
        }
        if preserve_membership {
            let engine = &self.layout_manager.layout_engine;
            let intents: Vec<_> = self
                .state
                .windows
                .iter_workspace_assignments()
                .filter_map(|(window, assignment)| {
                    let record = self.state.windows.record(window)?;
                    let target = engine.workspaces().workspaces.get(assignment)?.space;
                    // Native membership will reassign windows from a lost local desktop.
                    if self.is_dragging_window(window) || lost_local_desktops.contains(&target) {
                        return None;
                    }
                    if record
                        .pending_transfer()
                        .is_some_and(|operation| operation.requested_space == target)
                    {
                        return None;
                    }
                    let observed = record
                        .window_server_id()
                        .and_then(|id| self.space_state.active_window_spaces.get(&id).copied());
                    if observed == Some(target) && self.space_state.membership_complete {
                        return None;
                    }
                    let frame =
                        engine.get_floating_position(assignment, window).or_else(|| record.frame());
                    Some((window, target, frame))
                })
                .collect();
            for (window, target, frame) in intents {
                // Unknown membership preserves intent without issuing a speculative write.
                self.state.windows.request_transfer(window, frame, target, false);
            }
        }
    }

    fn balance_display_workspaces(
        &mut self,
        changes: &mut Changes,
        previous_screens: &[ScreenInfo],
        restored: &HashSet<crate::model::VirtualWorkspaceId>,
    ) {
        let displays: Vec<_> = self
            .screens_in_physical_order()
            .into_iter()
            .filter_map(|screen| screen.space.filter(|space| self.active_spaces.contains(space)))
            .collect();
        let workspaces = self.layout_manager.layout_engine.workspaces();
        for (id, target) in workspaces.plan_display_balance(&displays, restored) {
            self.place_workspace(
                changes,
                id,
                target,
                previous_screens,
                crate::model::virtual_workspace::WorkspaceTransferReason::DisplayArrival,
            );
        }
    }

    fn place_workspace(
        &mut self,
        changes: &mut Changes,
        id: crate::model::VirtualWorkspaceId,
        target: SpaceId,
        previous_screens: &[ScreenInfo],
        reason: crate::model::virtual_workspace::WorkspaceTransferReason,
    ) {
        let Some(screen) = self.space_state.screen_by_space(target) else {
            return;
        };
        let source = self.layout_manager.layout_engine.workspaces()[id].space;
        let source_frame = previous_screens
            .iter()
            .chain(&self.space_state.screens)
            .find(|screen| screen.space == Some(source))
            .map(|screen| screen.frame)
            .or_else(|| self.last_screen_frames.get(&source).copied());
        let response = self.layout_manager.layout_engine.transfer_workspace(
            &mut self.state.windows,
            id,
            target,
            screen.frame.size,
            screen.display_uuid_opt(),
            reason,
        );
        for window in response.transferred_windows {
            self.request_window_transfer(changes, window, target, source_frame);
        }
        if response.changed {
            changes.move_between(source, target, Presentation::Animated);
            changes.workspace_changed(source);
            changes.workspace_changed(target);
        }
    }

    /// Prepare physical intent from accepted geometry. An unavailable destination
    /// keeps the original frame until a later accepted topology can place it.
    fn request_window_transfer(
        &mut self,
        changes: &mut Changes,
        window: WindowId,
        target: SpaceId,
        source_frame: Option<CGRect>,
    ) {
        if self.is_dragging_window(window) {
            return;
        }
        let screen = self
            .space_state
            .screen_by_space(target)
            .filter(|_| self.is_space_active(target))
            .map(|screen| screen.frame);
        let engine = &self.layout_manager.layout_engine;
        let frame = self
            .state
            .windows
            .workspace_for_window(window)
            .filter(|_| engine.is_window_floating(window))
            .and_then(|workspace| engine.get_floating_position(workspace, window))
            .or_else(|| self.state.windows.record(window)?.frame())
            .map(|frame| {
                screen.map_or(frame, |screen| {
                    self.frame_on_display(window, frame, source_frame, screen)
                })
            });
        let available = screen.is_some()
            && self.state.windows.can_place_window(window)
            && !self.refreshes_blocked()
            && !self.is_mission_control_active();
        if available
            && !self.is_in_drag()
            && self.layout_manager.layout_engine.workspaces().is_window_in_active_workspace(
                &self.state.windows,
                target,
                window,
            )
            && !self.layout_manager.layout_engine.is_window_floating(window)
            && self.layout_manager.layout_engine.active_layout_mode_at(target)
                != crate::common::config::LayoutMode::Floating
        {
            // The destination tile is calculated once from the fully reconciled model.
            self.state.windows.request_transfer(window, None, target, true);
            changes.arrange(Some(target), Presentation::Animated);
            changes.transfer_tile(window, target, frame);
        } else {
            self.send_transfer_frame(window, target, frame, available, None);
        }
    }

    fn send_transfer_frame(
        &mut self,
        window: WindowId,
        target: SpaceId,
        frame: Option<CGRect>,
        available: bool,
        bounds: Option<managers::ScrollingDisplayBounds>,
    ) -> bool {
        if self.state.windows.request_transfer(window, frame, target, available)
            && let Some(frame) = frame
        {
            self.cancel_window_presentations(vec![window]);
            self.layout_manager.layout_engine.prepare_window_placement(
                &mut self.state.windows,
                window,
                frame,
            );
            return self.write_window_frame(window, frame, false, bounds);
        }
        false
    }

    fn advance_workspace_transfers(&mut self, changes: &mut Changes) {
        if self.refreshes_blocked() {
            return;
        }
        if self.space_state.membership_complete {
            let pending: Vec<_> = self
                .state
                .windows
                .iter_windows()
                .filter_map(|(window, _)| {
                    let record = self.state.windows.record(window)?;
                    let operation = record.pending_transfer()?;
                    let target = operation.requested_space;
                    Some((window, target, operation))
                })
                .collect();
            for (window, target, operation) in pending {
                let available = self.is_space_active(target);
                let observed = self
                    .state
                    .windows
                    .record(window)
                    .and_then(|record| record.window_server_id())
                    .and_then(|id| self.space_state.active_window_spaces.get(&id).copied());
                let was_unavailable =
                    operation.status == crate::model::window_store::TransferStatus::Unavailable;
                if observed == Some(target) {
                    self.state.windows.confirm_transfer(
                        window,
                        operation.generation,
                        self.space_state.revision,
                        observed,
                    );
                } else if !changes
                    .tiled_transfers()
                    .any(|(planned, space, _)| planned == window && space == target)
                    && let Some(operation) = self.state.windows.advance_transfer(
                        window,
                        self.space_state.revision,
                        available,
                    )
                    && let Some(frame) = operation.requested_frame
                {
                    if let Some(screen) = self.space_state.screen_by_space(target) {
                        // Intent recorded while unavailable may describe another display.
                        let source = self.screen_for_point(frame.mid()).map(|source| source.frame);
                        let frame = if was_unavailable {
                            self.frame_on_display(window, frame, source, screen.frame)
                        } else {
                            frame
                        };
                        self.state.windows.request_transfer(window, Some(frame), target, true);
                        self.layout_manager.layout_engine.prepare_window_placement(
                            &mut self.state.windows,
                            window,
                            frame,
                        );
                        changes.arrange(Some(target), Presentation::Animated);
                        self.write_window_frame(window, frame, false, None);
                    }
                }
            }
        }
    }

    fn try_apply_pending_space_change(&mut self, changes: &mut Changes) {
        if self.is_mission_control_active() {
            return;
        }
        if let Some(pending) = self.pending_space_change_manager.pending_space_change.take() {
            // Re-enter the same admission/commit path: valid pending snapshots can
            // release a hold, while superseded ones stay pending for their deltas.
            self.reconcile_space_snapshot(changes, pending);
        }
    }

    fn on_windows_discovered_with_app_info(
        &mut self,
        changes: &mut Changes,
        pid: pid_t,
        mut new: Vec<(WindowId, WindowInfo)>,
        known_visible: Vec<WindowId>,
        app_info: Option<AppInfo>,
    ) {
        // Rebind the visible tab before inventory retirement removes its old slot.
        for (wid, info) in &mut new {
            self.replace_native_tab(*wid, info);
        }
        let app_info =
            app_info.or_else(|| self.app_manager.apps.get(&pid).map(|app| app.info.clone()));
        // AX can observe a native move before the display callback. It may refresh
        // window metadata, but existing workspace ownership comes from the snapshot.
        // Resolve each native identity once for this inventory observation.
        let mut native_spaces = HashMap::default();
        let mut fullscreen_observations = HashMap::default();
        for wsid in self
            .state
            .windows
            .window_ids_for_pid(pid)
            .filter_map(|wid| self.state.windows.record(wid)?.window_server_id())
            .chain(new.iter().filter_map(|(_, info)| info.sys_id))
        {
            native_spaces.entry(wsid).or_insert_with(|| {
                let recovering = self
                    .state
                    .windows
                    .native_fullscreen_record_for_window_server_id(wsid)
                    .is_some()
                    || self
                        .state
                        .windows
                        .pending_native_fullscreen_record_for_window_server_id(wsid)
                        .is_some();
                if recovering || self.state.windows.tracked_window_id(wsid).is_none() {
                    // Multiple memberships are transitional, even if one is a
                    // user Space. Do not let the generic user-Space preference
                    // turn contradictory evidence into a completed exit.
                    let spaces = window_server::window_spaces(wsid);
                    let live = (spaces.len() == 1).then(|| spaces[0]);
                    if let Some(space) = live.filter(|space| {
                        self.is_fullscreen_space(*space)
                            || self
                                .space_state
                                .display_space_ids
                                .values()
                                .any(|spaces| spaces.contains(space))
                    }) {
                        fullscreen_observations.insert(wsid, space);
                    }
                    live.or_else(|| {
                        (!recovering)
                            .then(|| self.state.windows.window_server_space(wsid))
                            .flatten()
                    })
                } else {
                    self.resolve_native_space(
                        wsid,
                        self.space_state.active_window_spaces.get(&wsid).copied(),
                    )
                }
            });
        }
        if self.state.windows.window_ids_for_pid(pid).any(|wid| {
            let assigned = self.assigned_space_for_window_id(wid);
            let native = self
                .state
                .windows
                .record(wid)
                .and_then(|record| record.window_server_id())
                .and_then(|wsid| native_spaces[&wsid]);
            assigned.is_some() && native.is_some() && assigned != native
        }) {
            self.request_space_snapshot();
        }
        // A returned native identity protects its previous AX key until rekeying.
        let observed = known_visible.iter().copied().chain(new.iter().flat_map(|(wid, info)| {
            std::iter::once(*wid)
                .chain(info.sys_id.and_then(|wsid| self.state.windows.tracked_window_id(wsid)))
        }));
        let retired = if matches!(
            self.refocus_manager.stale_cleanup_state,
            StaleCleanupState::Suppressed
        ) || self.is_mission_control_active()
            || self.is_in_drag()
        {
            Vec::new()
        } else {
            self.state.windows.inventory_retirements(
                pid,
                observed,
                |wid| {
                    self.state
                        .windows
                        .record(wid)
                        .and_then(|record| record.window_server_id())
                        .and_then(|wsid| native_spaces[&wsid])
                        .or_else(|| self.assigned_space_for_window_id(wid))
                        .is_some_and(|space| !self.is_space_active(space))
                        || self.hidden_assigned_space_for_window_id(wid).is_some()
                },
                |wsid, cached_info| crate::model::window_store::InventoryWindowObservation {
                    info: cached_info.or_else(|| window_server::get_window(wsid)),
                    suitable: window_server::app_window_suitability(wsid),
                    ordered_in: window_server::window_ordered_in(wsid),
                },
            )
        };
        for wid in retired {
            self.retire_window(changes, wid);
        }
        let mut new_window_ids = Vec::new();
        let mut resumed = HashSet::default();
        for (wid, info) in new {
            if let Some(id) = info.sys_id
                && let Some(space) =
                    native_spaces[&id].or_else(|| self.best_space_for_frame(&info.frame))
            {
                self.transient_focus.ax(id, wid.pid, &info, space);
            }
            let (new, returned) = self.observe_ax_window(changes, wid, info);
            if new {
                new_window_ids.push(wid);
            }
            if returned {
                resumed.insert(wid);
            }
        }
        for (wsid, space) in fullscreen_observations {
            if self.is_fullscreen_space(space) {
                let user = self
                    .state
                    .windows
                    .tracked_window_id(wsid)
                    .and_then(|window| self.assigned_space_for_window_id(window));
                if let Some(window) =
                    self.state.windows.observe_native_fullscreen(wsid, space, user, Some(pid))
                {
                    self.reconcile_window_membership(changes, window, None);
                }
            } else if let Some(window) = self.state.windows.tracked_window_id(wsid) {
                self.state
                    .windows
                    .observe_native_space(wsid, space, self.is_space_active(space));
                self.restore_native_fullscreen(changes, Some(wsid), window, Some(space));
            }
        }
        self.reconcile_native_fullscreen_exits(changes, false);
        let has_admitted_windows = new_window_ids
            .iter()
            .any(|wid| self.state.windows.window(*wid).is_some_and(WindowState::is_admitted));

        let window_spaces = self
            .state
            .windows
            .window_ids_for_pid(pid)
            .filter(|wid| self.state.windows.contains_window(*wid))
            .map(|wid| {
                let native = self
                    .state
                    .windows
                    .record(wid)
                    .and_then(|record| record.window_server_id())
                    .and_then(|wsid| native_spaces[&wsid]);
                (wid, self.discovery_spaces_for_window(wid, native))
            })
            .collect();
        self.transient_focus.windows.retain(|id, t| {
            t.native || t.pid != pid || self.state.windows.tracked_window_id(*id).is_some()
        });
        let active_spaces = self
            .space_state
            .screens
            .iter()
            .filter_map(|screen| screen.space)
            .filter(|space| self.is_space_active(*space))
            .collect();
        let focused_window = self
            .focused_window_for_discovery(pid, &window_spaces)
            .filter(|(_, wid)| !has_admitted_windows || new_window_ids.contains(wid));
        self.reconcile_discovered_windows(changes, window_discovery::EmitLayoutPayload {
            pid,
            known_visible: &known_visible,
            resumed: &resumed,
            app_info: &app_info,
            window_spaces,
            active_spaces,
            focused_window,
        });
        changes.refresh_notifications = true;
    }

    fn best_space_for_window(
        &self,
        frame: &CGRect,
        window_server_id: Option<WindowServerId>,
    ) -> Option<SpaceId> {
        self.space_for_window_observation(frame, window_server_id, || {
            window_server_id.and_then(|wsid| self.resolve_native_space(wsid, None))
        })
    }

    fn space_for_window_observation(
        &self,
        frame: &CGRect,
        wsid: Option<WindowServerId>,
        native: impl FnOnce() -> Option<SpaceId>,
    ) -> Option<SpaceId> {
        if wsid.is_some_and(|wsid| self.is_known_fullscreen_window(wsid)) {
            return None;
        }
        native()
            .or_else(|| self.hidden_assigned_space_for_frame(wsid, frame))
            .or_else(|| self.best_space_for_frame(frame))
    }

    fn best_space_for_frame(&self, frame: &CGRect) -> Option<SpaceId> {
        let center = frame.mid();
        self.screen_for_point(center).and_then(|screen| screen.space).or_else(|| {
            self.space_state
                .screens
                .iter()
                .filter_map(|screen| {
                    let space = screen.space?;
                    let area = screen.frame.intersection(frame).area() as i64;
                    if area > 0 { Some((area, space)) } else { None }
                })
                .max_by_key(|(area, _)| *area)
                .map(|(_, space)| space)
        })
    }

    fn drag_scene(&self, source: WindowId, space: SpaceId) -> crate::actor::drag::DragScene {
        interaction_workflow::build_drag_scene(&self.state, &self.layout_manager, source, space)
    }

    fn resolve_drag_preview(&mut self) {
        let Some(source) = self.drag_manager.actor.source() else {
            return;
        };
        while let Some(mut intent) = self.drag_manager.actor.intent() {
            if intent.window == source.window
                && !matches!(intent.action, crate::layout_engine::WindowDropAction::Move(_))
            {
                self.drag_manager.actor.set_preview(intent, Some(source.origin_frame));
                break;
            }
            let preview = loop {
                let preview = self.space_state.screen_by_space(intent.space).and_then(|screen| {
                    self.layout_manager.layout_engine.drop_preview_frame(
                        intent.space,
                        source.window,
                        intent.window,
                        intent.action,
                        screen.frame,
                        screen.display_uuid_opt(),
                        &self.config.settings.ui.stack_line,
                    )
                });
                let Some(result) = preview else { break None };
                let action =
                    result.action(intent.action, self.config.settings.drag_drop.drop_action);
                if action == intent.action {
                    break Some(result);
                }
                intent.action = action;
            };
            if !self.drag_manager.actor.set_preview(intent, preview.map(|result| result.frame)) {
                break;
            }
        }
        self.drag_manager.sync_preview();
    }

    fn refresh_active_drag_scene(&mut self) {
        let Some(source) = self.drag_manager.actor.source() else {
            return;
        };
        if !source.tiled
            || !matches!(
                self.drag_manager.actor.kind(),
                Some(
                    crate::actor::drag::DragKind::NativeMove
                        | crate::actor::drag::DragKind::ModifierMove
                )
            )
        {
            return;
        }
        let Some(space) = source.current_space else { return };
        let scene = self.drag_scene(source.window, space);
        self.drag_manager.actor.replace_scene(scene);
        self.resolve_drag_preview();
    }

    #[cfg(test)]
    fn ensure_active_drag(&mut self, wid: WindowId, frame: &CGRect) {
        if self.drag_manager.actor.source().is_none_or(|source| source.window != wid) {
            let server_id = self.state.windows.window(wid).and_then(|window| window.info.sys_id);
            let origin_space = self.best_space_for_window(frame, server_id);
            self.drag_manager.actor.begin_native(
                crate::actor::drag::DragSource {
                    window: wid,
                    origin_frame: *frame,
                    last_frame: *frame,
                    origin_space,
                    current_space: origin_space,
                    tiled: true,
                },
                crate::actor::drag::DragScene::default(),
            );
        }
    }

    fn best_space_for_window_state(&self, window: &WindowState) -> Option<SpaceId> {
        self.best_space_for_window(&window.frame_monotonic, window.info.sys_id)
    }

    fn hidden_assigned_space_for_frame(
        &self,
        window_server_id: Option<WindowServerId>,
        _frame: &CGRect,
    ) -> Option<SpaceId> {
        let wsid = window_server_id?;
        let wid = self.state.windows.tracked_window_id(wsid)?;
        let assigned_space = self.assigned_space_for_window_id(wid)?;
        if !self.is_space_active(assigned_space)
            || !self.window_in_non_active_workspace(assigned_space, wid)
        {
            return None;
        }

        Some(assigned_space)
    }

    fn hidden_assigned_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        let window = self.state.windows.window(wid)?;
        self.hidden_assigned_space_for_frame(window.info.sys_id, &window.frame_monotonic)
    }

    fn assigned_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.layout_manager
            .layout_engine
            .workspaces()
            .workspace_space(self.state.windows.workspace_for_window(wid)?)
    }

    fn pending_target_space_for_window_server_id(&self, wsid: WindowServerId) -> Option<SpaceId> {
        let wid = self.state.windows.tracked_window_id(wsid)?;
        if let Some(space) = self
            .state
            .windows
            .record(wid)
            .and_then(|record| record.pending_transfer())
            .map(|operation| operation.requested_space)
        {
            return Some(space);
        }
        let target_frame = self.transaction_manager.get_target_frame(wsid)?;
        let assigned_space = self.assigned_space_for_window_id(wid)?;
        let target_space = self
            .hidden_assigned_space_for_frame(Some(wsid), &target_frame)
            .or_else(|| self.best_space_for_frame(&target_frame))?;
        (target_space == assigned_space).then_some(target_space)
    }

    fn reconcile_windows_in_authoritative_active_snapshot(
        &mut self,
        changes: &mut Changes,
        active_windows: &[(WindowServerId, Option<SpaceId>)],
        invalidated_spaces: &[SpaceId],
    ) {
        if self.refreshes_blocked() {
            self.defer_window_inventory_refresh();
            return;
        }

        let windows: Vec<_> = active_windows
            .iter()
            .filter_map(|&(wsid, observed_space)| {
                let wid = self.state.windows.tracked_window_id(wsid)?;
                if self.state.windows.native_fullscreen_record_for_window(wid).is_some()
                    || self
                        .state
                        .windows
                        .record(wid)
                        .and_then(|record| record.pending_transfer())
                        .is_some()
                {
                    return None;
                }
                let authoritative_space =
                    self.state.windows.window_server_space(wsid).or(observed_space)?;
                Some((wid, authoritative_space))
            })
            .collect();
        self.layout_manager
            .layout_engine
            .begin_native_reconcile(&self.state.windows, &windows);
        for (wid, authoritative_space) in windows {
            let preserve_ordinal = self
                .assigned_space_for_window_id(wid)
                .is_some_and(|space| invalidated_spaces.contains(&space));
            self.reassign_window_to_authoritative_space(
                changes,
                wid,
                authoritative_space,
                preserve_ordinal,
            );
        }
        self.layout_manager.layout_engine.finish_native_reconcile(&self.state.windows);
    }

    #[cfg(test)]
    fn reconcile_windows_with_authoritative_spaces(&mut self) {
        let active_windows: Vec<_> = self
            .state
            .windows
            .iter_windows()
            .filter_map(|(wid, state)| {
                let wsid = state.info.sys_id?;
                Some((wsid, self.authoritative_space_for_window_id(wid)))
            })
            .collect();
        let mut changes = Changes::default();
        self.reconcile_windows_in_authoritative_active_snapshot(&mut changes, &active_windows, &[]);
        self.finish(changes, None, None);
    }

    fn current_reported_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.state
            .windows
            .window(wid)
            .and_then(|window| window.info.sys_id)
            .and_then(|wsid| self.resolve_native_space(wsid, None))
    }

    fn authoritative_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.current_reported_space_for_window_id(wid)
            .or_else(|| self.assigned_space_for_window_id(wid))
    }

    pub(crate) fn resolve_native_space(
        &self,
        wsid: WindowServerId,
        observation: Option<SpaceId>,
    ) -> Option<SpaceId> {
        let transferring = self
            .state
            .windows
            .tracked_window_id(wsid)
            .and_then(|wid| self.state.windows.record(wid))
            .and_then(|record| record.pending_transfer())
            .is_some();
        if transferring {
            return observation.or_else(|| self.state.windows.window_server_space(wsid));
        }
        let pending = self.pending_target_space_for_window_server_id(wsid);
        let live =
            if observation.is_none() || pending.is_some_and(|target| observation != Some(target)) {
                window_server::window_space(wsid)
            } else {
                None
            };
        let resolved = self.state.windows.resolve_native_space(wsid, observation, pending, live);
        trace!(
            ?wsid,
            ?observation,
            ?pending,
            ?live,
            ?resolved,
            "Resolved native space"
        );
        resolved
    }

    fn best_space_for_window_id(&self, wid: WindowId) -> Option<SpaceId> {
        self.authoritative_space_for_window_id(wid).or_else(|| {
            self.state
                .windows
                .window(wid)
                .and_then(|window| self.best_space_for_window_state(window))
        })
    }

    fn discovery_spaces_for_window(
        &self,
        wid: WindowId,
        native: Option<SpaceId>,
    ) -> (Option<SpaceId>, Option<SpaceId>) {
        let authoritative = native.or_else(|| self.assigned_space_for_window_id(wid));
        if self.state.windows.native_fullscreen_record_for_window(wid).is_some() {
            return (authoritative, None);
        }
        let discovery = self.assigned_space_for_window_id(wid).or(authoritative).or_else(|| {
            let window = self.state.windows.window(wid)?;
            self.best_space_for_frame(&window.frame_monotonic).filter(|space| {
                self.is_space_active(*space)
                    || !window.info.sys_id.is_some_and(|wsid| self.is_known_fullscreen_window(wsid))
            })
        });
        // A placeholder assignment supplies ownership but cannot admit an AX window.
        (
            authoritative,
            discovery.filter(|_| self.state.windows.contains_window(wid)),
        )
    }

    pub(crate) fn geometry_space_for_window(
        &self,
        frame: &CGRect,
        window_server_id: Option<WindowServerId>,
    ) -> Option<SpaceId> {
        if let Some(wsid) = window_server_id
            && self.is_known_fullscreen_window(wsid)
        {
            return None;
        }

        if let Some(space) = self.hidden_assigned_space_for_frame(window_server_id, frame) {
            return Some(space);
        }

        self.best_space_for_frame(frame)
    }

    fn is_known_fullscreen_window(&self, wsid: WindowServerId) -> bool {
        self.state.windows.is_window_server_id_native_fullscreen_suspended(wsid)
    }

    fn window_frame_on_known_screen(&self, wid: WindowId) -> Option<CGRect> {
        let frame = self.state.windows.window(wid)?.frame_monotonic;
        self.screen_for_point(frame.mid()).map(|_| frame)
    }

    fn window_center_on_known_screen(&self, wid: WindowId) -> Option<CGPoint> {
        self.window_frame_on_known_screen(wid).map(|frame| frame.mid())
    }

    pub fn warp_mouse(&mut self, point: CGPoint) {
        let Some(input_tx) = self.communication_manager.input_tx.clone() else {
            return;
        };
        _ = input_tx.send(crate::actor::input::Request::Warp(point));
    }

    fn insert_app_handle_for_window(
        &self,
        app_handles: &mut HashMap<pid_t, AppThreadHandle>,
        wid: WindowId,
    ) {
        if let Some(app) = self.app_manager.apps.get(&wid.pid) {
            app_handles.entry(wid.pid).or_insert_with(|| app.handle.clone());
        }
    }

    fn expose_all_spaces(&mut self) {
        let spaces: Vec<SpaceId> = (self.screens_in_physical_order().into_iter())
            .filter_map(|screen| screen.space)
            .filter(|space| self.is_space_active(*space))
            .collect();
        let screens = &self.space_state.screens;
        let rule_space = |preferences: &[String]| {
            crate::sys::screen::resolve_display_preference(preferences, screens, |screen| {
                screen.space.is_some_and(|space| spaces.contains(&space))
            })
            .and_then(|screen| screen.space)
        };
        self.layout_manager.layout_engine.workspaces_mut().ensure_visible_spaces(
            &spaces,
            screens.len(),
            rule_space,
        );
        for space in spaces {
            self.expose_space_if_known(space);
        }
    }

    fn window_is_standard(&self, id: WindowId) -> bool { self.state.windows.is_admitted(id) }

    fn visible_spaces_for_layout(&self) -> (Vec<SpaceId>, HashMap<SpaceId, CGRect>) {
        let visible = self.space_state.screens.iter().filter_map(|screen| {
            let space = screen.space.filter(|&space| self.is_space_active(space))?;
            Some((space, screen.frame))
        });
        let spaces = order_visible_spaces_by_position(
            visible.clone().map(|(space, frame)| (space, frame.mid())),
        );
        (spaces, visible.collect())
    }

    fn reconcile_layout_event(&mut self, changes: &mut Changes, event: LayoutEvent) {
        if let LayoutEvent::WindowFocused(space, window) = event
            && (!self.is_space_active(space)
                || !self.state.windows.is_admitted(window)
                || self.window_in_non_active_workspace(space, window))
        {
            return;
        }
        let suspended = match event {
            LayoutEvent::WindowSuspended(window) => {
                self.drag_manager.forget_window(window);
                Some(window)
            }
            _ => None,
        };
        let transfer = if let LayoutEvent::WindowObserved(source, resolved) = &event {
            let window = resolved.info.window_id;
            let target =
                self.layout_manager.layout_engine.workspaces()[resolved.effects.workspace_id].space;
            let native = self.state.windows.native_space(window).unwrap_or(*source);
            let pending =
                self.state.windows.record(window).and_then(|record| record.pending_transfer());
            pending
                .map_or(native != target, |operation| operation.requested_space != target)
                .then_some((window, target))
        } else {
            None
        };

        let previous_focus = matches!(event, LayoutEvent::WindowFocused(..))
            .then(|| self.layout_manager.layout_engine.focused_window());
        let event_space = match &event {
            #[cfg(test)]
            LayoutEvent::TestWindowsObserved(space, ..) => Some(*space),
            LayoutEvent::WindowFocused(space, _) => Some(*space),
            LayoutEvent::WindowAdded(space, window) => {
                self.assigned_space_for_window_id(*window).or(Some(*space))
            }
            LayoutEvent::WindowObserved(_, resolved) => self
                .layout_manager
                .layout_engine
                .workspaces()
                .workspace_space(resolved.effects.workspace_id),
            LayoutEvent::WindowRemoved(wid)
            | LayoutEvent::WindowRemovedPreserveFloating(wid)
            | LayoutEvent::WindowSuspended(wid) => self
                .assigned_space_for_window_id(*wid)
                .or_else(|| self.layout_manager.layout_engine.space_with_window(*wid)),
            _ => None,
        };
        let membership_event = matches!(
            event,
            LayoutEvent::WindowAdded(..)
                | LayoutEvent::WindowObserved(..)
                | LayoutEvent::WindowRemoved(..)
                | LayoutEvent::WindowRemovedPreserveFloating(..)
                | LayoutEvent::WindowSuspended(..)
        );
        let source = match &event {
            LayoutEvent::WindowObserved(_, resolved) => resolved
                .effects
                .source_workspace
                .and_then(|id| self.layout_manager.layout_engine.workspaces().workspace_space(id)),
            _ => None,
        };
        let removed = match event {
            LayoutEvent::WindowRemoved(window) => Some(window),
            _ => None,
        };
        let removes_assignment =
            removed.is_some_and(|window| self.state.windows.workspace_for_window(window).is_some());
        let focus_desktop = removed.is_some_and(|window| {
            self.layout_manager.layout_engine.focused_window() == Some(window)
        });
        self.prepare_refocus_before_removal(&event);
        let admitted = match &event {
            LayoutEvent::WindowAdded(_, wid) => Some(*wid),
            LayoutEvent::WindowObserved(_, window) => Some(window.info.window_id),
            _ => None,
        };
        let layout_outcome =
            self.layout_manager.layout_engine.handle_event(&mut self.state.windows, event);
        let response = layout_outcome.response;
        if let Some(window) = suspended.filter(|_| response.changed) {
            // Preserve the viewport target set by the layout change.
            self.cancel_window_frame_writes(vec![window]);
        }
        let resumed_transfer = admitted.and_then(|window| {
            let operation = self.state.windows.record(window)?.pending_transfer()?;
            (operation.status == crate::model::window_store::TransferStatus::Unavailable
                && self.state.windows.can_place_window(window))
            .then_some((window, operation.requested_space))
        });
        if let Some((window, target)) = transfer.or(resumed_transfer) {
            self.request_window_transfer(changes, window, target, None);
        }
        match layout_outcome.app_rules.frame {
            Some(AppRuleFrame::Place(placement)) => self.apply_app_rule_placement(placement),
            Some(AppRuleFrame::Resize(resize)) if self.apply_app_rule_resize(resize) => {
                changes.arrange(Some(resize.space), Presentation::Resize).layout_changed = true;
            }
            _ => {}
        }
        if previous_focus
            .is_some_and(|previous| previous != self.layout_manager.layout_engine.focused_window())
            && let Some(input_tx) = &self.communication_manager.input_tx
        {
            _ = input_tx.send(crate::actor::input::Request::HideOnFocus);
        }
        if let Some((space, window)) = event_space.zip(admitted) {
            self.request_refocus_if_hidden(space, window);
        }
        if response.changed
            && let Some(space) = event_space
        {
            changes.space(space).layout_changed = true;
            if membership_event && let Some(source) = source {
                changes.move_between(source, space, Presentation::Animated);
            } else {
                changes.arrange(Some(space), Presentation::Animated);
                if membership_event {
                    changes.membership_for(Some(space));
                }
            }
            changes.space(space).during_drag |= removed.is_some();
        } else if !membership_event && let Some(space) = event_space {
            changes.space(space).tabs_changed = true;
        }
        if let Some(window) = removed {
            self.state.windows.cancel_transfer(window);
        }
        if removes_assignment && let Some(space) = event_space {
            changes.membership_for(Some(space));
        }
        changes.refresh_menu |= removes_assignment || (membership_event && response.changed);
        if response.focus_window.is_some() || !response.raise_windows.is_empty() {
            self.reconcile_layout_response(changes, response, None);
        }
        if let Some(request) = layout_outcome.app_rules.workspace_focus {
            self.store_current_floating_positions(request.space);
            self.workspace_switch_manager
                .start_workspace_switch(request.space, WorkspaceSwitchOrigin::Auto);
            let response = self.layout_manager.layout_engine.activate_workspace(
                &mut self.state.windows,
                request.space,
                request.workspace,
                Some(request.window),
            );
            if response.changed {
                changes.workspace_changed(request.space);
                changes.arrange(Some(request.space), Presentation::WorkspaceSwitch);
            }
            self.reconcile_layout_response(changes, response, None);
        }
        if focus_desktop {
            changes.desktop_focus = self.workspace_command_space();
        }
    }

    fn apply_app_rule_placement(&mut self, placement: AppRulePlacement) {
        if self.is_dragging_window(placement.window)
            || !self.state.windows.can_place_window(placement.window)
        {
            return;
        }
        let Some(window) = self.state.windows.window(placement.window) else {
            return;
        };
        let screen = if placement.position.is_some() {
            let Some(screen) = self.space_state.screen_by_space(placement.space) else {
                warn!(window = ?placement.window, space = ?placement.space, "App-rule position needs screen geometry");
                return;
            };
            screen.frame
        } else {
            CGRect::default()
        };
        let frame = placement.resolve_frame(window.frame_monotonic, screen);
        let current = window
            .info
            .sys_id
            .and_then(|id| self.transaction_manager.get_target_frame(id))
            .unwrap_or(window.frame_monotonic);
        if frame.same_as(current) {
            return;
        }
        self.state.windows.clear_floating_display_geometry(placement.window);
        self.layout_manager.layout_engine.prepare_window_placement(
            &mut self.state.windows,
            placement.window,
            frame,
        );
        if let Some(workspace) = self.state.windows.workspace_for_window(placement.window)
            && self.layout_manager.layout_engine.workspaces()[workspace].layout_mode()
                == crate::common::config::LayoutMode::Floating
        {
            self.layout_manager.layout_engine.store_floating_position(
                workspace,
                placement.window,
                frame,
            );
        }
        self.write_window_frame(placement.window, frame, false, None);
    }

    fn apply_app_rule_resize(&mut self, resize: AppRuleResize) -> bool {
        if self.is_dragging_window(resize.window)
            || !self.state.windows.can_place_window(resize.window)
        {
            return false;
        }
        let Some(window) = self.state.windows.window(resize.window) else {
            return false;
        };
        let Some(screen) = self.space_state.screen_by_space(resize.space) else {
            warn!(window = ?resize.window, space = ?resize.space, "App-rule resize needs screen geometry");
            return false;
        };
        let old_frame = window.frame_monotonic;
        let mut new_frame = old_frame;
        new_frame.size.width = resize.size.w.unwrap_or(old_frame.size.width);
        new_frame.size.height = resize.size.h.unwrap_or(old_frame.size.height);
        if old_frame.same_as(new_frame) {
            return false;
        }
        self.layout_manager.layout_engine.apply_app_rule_resize(
            resize,
            old_frame,
            new_frame,
            screen.frame,
            screen.display_uuid_opt(),
        );
        true
    }

    fn observe_transient_lifecycle(&mut self, event: &Event) {
        match *event {
            Event::WindowDestroyed(wid) => {
                let id = self.state.windows.record(wid).and_then(|w| w.window_server_id());
                if let Some(id) = id {
                    self.transient_focus.visibility(id, false);
                }
            }
            Event::WindowClosed(id) => {
                self.transient_focus.windows.remove(&id);
            }
            Event::WindowServerHidden(id) | Event::WindowServerUnhidden(id) => {
                self.transient_focus
                    .visibility(id, matches!(event, Event::WindowServerUnhidden(_)));
            }
            Event::ApplicationTerminated(pid) => {
                self.transient_focus.windows.retain(|_, t| t.pid != pid);
            }
            _ => {}
        }
    }

    fn transient_blocks_mouse_focus(&self) -> bool {
        self.transient_focus.windows.iter().any(|(id, t)| {
            t.visible
                && self.main_window_tracker.is_globally_frontmost(t.pid)
                && self.active_spaces.contains(&t.space)
                // A rule-admitted AXDialog window is managed content. Its guard
                // entry comes from the heuristic the rule overrode, so it must
                // not disable focus-follows-mouse. Native transients always block.
                && (t.native
                    || self
                        .state
                        .windows
                        .tracked_window_id(*id)
                        .is_none_or(|wid| !self.state.windows.is_admitted(wid)))
        })
    }

    // Returns true if the window should be raised on mouse over considering
    // active workspace membership and potential occlusion of floating windows above it.
    pub(crate) fn should_raise_on_mouse_over(&self, wid: WindowId, space: Option<SpaceId>) -> bool {
        let Some(window) = self.state.windows.window(wid) else {
            return false;
        };

        if !self.state.windows.is_admitted(wid) || !self.state.windows.can_place_window(wid) {
            trace!(
                ?wid,
                "Skipping mouse focus for a window outside admission policy"
            );
            return false;
        }

        let candidate_frame = window.frame_monotonic;

        if matches!(self.menu_manager.menu_state, MenuState::Open(_)) {
            trace!(?wid, "Skipping autoraise while menu open");
            return false;
        }

        let Some(space) = space else {
            trace!(?wid, "Skipping mouse focus without a resolved space");
            return false;
        };
        if !self.is_space_active(space) {
            trace!(?wid, ?space, "Skipping mouse focus on an inactive space");
            return false;
        }

        if !self.layout_manager.layout_engine.workspaces().is_window_in_active_workspace(
            &self.state.windows,
            space,
            wid,
        ) {
            trace!("Ignoring mouse over window {:?} - not in active workspace", wid);
            return false;
        }

        let Some(candidate_wsid) = window.info.sys_id else {
            return true;
        };

        // Native stacking order matters only if another tracked floating
        // window could be completely covered by this raise.
        let could_occlude = self.state.windows.iter_windows().any(|(other, state)| {
            other != wid
                && state.info.sys_id.is_some()
                && self.layout_manager.layout_engine.is_window_floating(other)
                && candidate_frame.contains_rect(state.frame_monotonic)
        });
        if !could_occlude {
            return true;
        }

        let order = {
            let space_id = space.get();
            crate::sys::window_server::space_window_list_for_connection(&[space_id], 0, false)
        };
        let candidate_u32 = candidate_wsid.as_u32();
        let mut candidate_levels = None;

        for above_u32 in order {
            if above_u32 == candidate_u32 {
                break;
            }

            let above_wsid = WindowServerId::new(above_u32);
            let Some(above_wid) = self.state.windows.tracked_window_id(above_wsid) else {
                continue;
            };

            if !self.layout_manager.layout_engine.is_window_floating(above_wid) {
                continue;
            }

            let Some(above_state) = self.state.windows.window(above_wid) else {
                continue;
            };
            let above_frame = above_state.frame_monotonic;
            if !candidate_frame.contains_rect(above_frame) {
                continue;
            }

            let (candidate_level, candidate_sub_level) =
                *candidate_levels.get_or_insert_with(|| {
                    (window_level(candidate_u32), window_sub_level(candidate_u32))
                });
            let above_level = window_level(above_u32);
            let above_sub_level = window_sub_level(above_u32);
            if candidate_level
                .zip(above_level)
                .is_some_and(|(candidate, above)| candidate == above)
                && candidate_sub_level == above_sub_level
            {
                return false;
            }
        }

        true
    }

    fn process_windows_for_app_rules(
        &mut self,
        changes: &mut Changes,
        window_ids: impl IntoIterator<Item = WindowId>,
        app_info: AppInfo,
        reapply_effects: bool,
    ) {
        // Per-display rule assignment needs only the current Space, while a
        // shared workspace rule can resolve across all visible displays.
        let visible = (self.active_spaces.len() > 1
            && self.layout_manager.layout_engine.workspaces().settings.mode
                == crate::common::config::WorkspaceMode::Global)
            .then(|| self.iter_active_spaces().collect::<Vec<_>>());
        for wid in window_ids {
            if !self.state.windows.is_claimed(wid)
                && !self.state.windows.can_reconcile_admission(wid)
            {
                continue;
            }
            let Some(space) = self
                .assigned_space_for_window_id(wid)
                .or_else(|| self.best_space_for_window_id(wid))
                .filter(|space| self.is_space_active(*space))
            else {
                continue;
            };
            let engine = &self.layout_manager.layout_engine;
            let was_floating = engine.is_window_floating(wid);
            let effects = self.assign_observed_window(
                changes,
                wid,
                crate::model::virtual_workspace::WorkspaceContext {
                    space,
                    visible: visible.as_deref().unwrap_or(std::slice::from_ref(&space)),
                },
                Some(&app_info),
                reapply_effects,
            );
            if let Some(effects) = effects
                && (reapply_effects
                    || effects.source_workspace != Some(effects.workspace_id)
                    || was_floating != effects.should_float(was_floating)
                    || self.state.windows.is_temporarily_unavailable(wid))
            {
                self.project_observed_window(changes, space, wid, effects);
            }
        }
    }

    fn sole_admitted_window(&self, pid: pid_t) -> Option<WindowId> {
        let mut windows = self
            .state
            .windows
            .window_ids_for_pid(pid)
            .filter(|wid| self.state.windows.is_admitted(*wid));
        let window = windows.next()?;
        windows.next().is_none().then_some(window)
    }

    fn auto_switch_to_window_workspace(
        &mut self,
        changes: &mut Changes,
        window: Option<WindowId>,
    ) -> bool {
        let Some(window) = window.filter(|wid| self.window_is_standard(*wid)) else {
            return false;
        };
        if self.state.windows.is_temporarily_unavailable(window) {
            return false;
        }
        let pid = window.pid;
        let Some(space) = self.best_space_for_window_id(window) else {
            return false;
        };
        let workspaces = self.layout_manager.layout_engine.workspaces();
        let Some(workspace) = workspaces.workspace_for_window(&self.state.windows, space, window)
        else {
            return false;
        };
        if workspaces.active_workspace(space).is_none_or(|active| active == workspace) {
            return false;
        }
        // A newly appearing native window can leave focus temporarily pointing
        // at the app's old window until AX admission completes.
        if self.state.windows.has_pending_window_for_pid(pid) {
            return false;
        }

        if self.suppress_auto_workspace_switch_until_input {
            debug!(
                pid,
                "Skipping auto workspace switch for lifecycle-restored activation before user input"
            );
            return false;
        }

        if self.workspace_switch_manager.active_workspace_switch.is_some() {
            trace!(
                "Skipping auto workspace switch for pid {} because a workspace switch is in progress",
                pid
            );
            return false;
        }

        if self.workspace_switch_manager.manual_switch_in_progress() {
            debug!(
                "Skipping auto workspace switch for pid {} because a manual switch is in progress",
                pid
            );
            return false;
        }

        if let Some(active_space) = self.raw_command_space()
            && self.is_fullscreen_space(active_space)
        {
            debug!(
                "Skipping auto workspace switch for pid {} because the active space is fullscreen",
                pid
            );
            return false;
        }

        if let Some(wsid) = self.activation_from_unmanageable_window(pid) {
            debug!(
                ?wsid,
                "Skipping auto workspace switch for pid {} because the activated window is not manageable",
                pid
            );
            return false;
        }

        let Some(bundle_id_str) =
            self.app_manager.apps.get(&pid).and_then(|app| app.info.bundle_id.as_ref())
        else {
            return false;
        };

        if self.config.settings.auto_focus_blacklist.contains(bundle_id_str) {
            debug!(
                "App {} is blacklisted for auto-focus workspace switching, ignoring activation",
                bundle_id_str
            );
            return false;
        }

        debug!(?workspace, pid, "Auto-switching to focused window's workspace");
        self.store_current_floating_positions(space);
        self.workspace_switch_manager
            .start_workspace_switch(space, WorkspaceSwitchOrigin::Auto);
        let response = self.layout_manager.layout_engine.activate_workspace(
            &mut self.state.windows,
            space,
            workspace,
            Some(window),
        );
        changes.workspace_changed(space);
        changes.arrange(Some(space), Presentation::WorkspaceSwitch);
        self.reconcile_layout_response(changes, response, None);
        true
    }

    fn reconcile_layout_response(
        &mut self,
        changes: &mut Changes,
        response: layout::EventResponse,
        fallback_space: Option<SpaceId>,
    ) {
        let workspace_switch_space = response.workspace_space.or(fallback_space);
        if let Some(dir) = response.boundary_hit
            && !self.is_in_drag()
            && self.config.settings.layout.scrolling.gestures.propagate_to_workspace_swipe
        {
            let invert = self.config.settings.layout.scrolling.gestures.invert_horizontal;
            let next = match dir {
                Direction::Left => Some(invert),
                Direction::Right => Some(!invert),
                _ => None,
            };
            if let (Some(next), Some(space)) = (
                next,
                workspace_switch_space.or_else(|| self.command_context_space()),
            ) && self.swipe_workspace(
                changes,
                space,
                Some(self.config.settings.gestures.skip_empty),
                next,
                self.config
                    .settings
                    .gestures
                    .haptics_enabled
                    .then_some(self.config.settings.gestures.haptic_pattern),
            ) {
                return;
            }
        }

        if let Some(space) = response.workspace_space {
            self.space_state.command_space = Some(space);
            if let Some(screen) = self.space_state.screen_by_space(space)
                && crate::sys::screen::set_active_menu_bar_display_uuid(&screen.display_uuid)
            {
                self.space_state.menu_bar_space = Some(space);
            }
        }
        if self.is_in_drag() {
            self.workspace_switch_manager.mark_workspace_switch_inactive();
            return;
        }

        if !response.changed
            && let Some(window) = response.focus_window
        {
            if let Some(space) =
                workspace_switch_space.or_else(|| self.best_space_for_window_id(window))
            {
                changes.space(space).tabs_changed = true;
            }
        }

        let layout::EventResponse {
            changed: _,
            raise_windows,
            focus_window,
            ..
        } = response;

        if let Some(space) = fallback_space {
            if focus_window.is_some() || response.changed {
                changes.fallback_focus = Some((space, focus_window));
            }
            self.stage_raise(changes, raise_windows, None, Quiet::Yes);
            return;
        }
        let focus_window = if let Some(space) = workspace_switch_space
            && self.workspace_switch_manager.active_workspace_switch == Some(space)
            && self.workspace_switch_manager.pending_workspace_switch_origin.is_some()
        {
            // The requested window may still be ordered out while the workspace
            // is being unparked. It must already resolve to the destination Space.
            focus_window
                .filter(|&wid| self.is_workspace_focus_candidate(space, wid))
                .or_else(|| self.visible_focus_candidate_in_active_workspace(space, None))
        } else {
            focus_window.filter(|&window| {
                self.layout_focus_space(window).is_some()
                    || workspace_switch_space
                        .is_some_and(|space| self.can_select_window(space, window))
            })
        };
        if let Some(window) = focus_window {
            if let Some(space) = workspace_switch_space {
                self.layout_manager.layout_engine.commit_workspace_focus(
                    &mut self.state.windows,
                    space,
                    Some(window),
                );
            }
            self.stage_layout_focus(
                changes,
                raise_windows,
                window,
                workspace_switch_space.map_or(Quiet::No, |_| Quiet::Yes),
            );
        } else {
            if let Some(space) = workspace_switch_space
                && response.changed
            {
                changes.fallback_focus = Some((space, None));
            }
            self.stage_raise(changes, raise_windows, None, Quiet::No);
        }
    }

    fn can_select_window(&self, space: SpaceId, window: WindowId) -> bool {
        self.is_space_active(space)
            && self.state.windows.is_admitted(window)
            && !self.state.windows.is_temporarily_unavailable(window)
            && !self.window_in_non_active_workspace(space, window)
    }

    fn layout_focus_space(&self, window: WindowId) -> Option<SpaceId> {
        if !self.state.windows.is_admitted(window) || !self.state.windows.can_place_window(window) {
            return None;
        }
        self.assigned_space_for_window_id(window)
            .or_else(|| self.best_space_for_window_id(window))
            .filter(|&space| self.can_select_window(space, window))
    }

    fn resolve_fallback_focus(&mut self, changes: &mut Changes) {
        if self.is_in_drag() {
            return;
        }
        if changes.focus.as_ref().is_some_and(|request| match request {
            raise_manager::Event::RaiseRequest(request) => request.focus_window.is_some(),
            raise_manager::Event::MouseRaiseRequest(..) => true,
            _ => false,
        }) {
            self.refocus_manager.refocus_state = RefocusState::None;
            return;
        }
        let (space, preferred, workspace_switch) =
            if let Some((space, window)) = changes.fallback_focus {
                self.refocus_manager.refocus_state = RefocusState::None;
                (space, window, true)
            } else if let RefocusState::Pending(space) = self.refocus_manager.refocus_state {
                self.refocus_manager.refocus_state = RefocusState::None;
                (
                    space,
                    self.layout_manager.layout_engine.selected_window(space),
                    false,
                )
            } else {
                return;
            };
        if !self.is_space_active(space) {
            return;
        }
        let mut window = self.visible_focus_candidate_in_active_workspace(space, preferred);
        if window.is_none() {
            if self.focus_desktop_if_active_workspace_empty(space) {
                if changes.desktop_focus == Some(space) {
                    changes.desktop_focus = None;
                }
                return;
            }
            if let Some(under_cursor) = self.window_id_under_cursor() {
                window = Some(under_cursor).filter(|&wid| {
                    self.state.windows.can_place_window(wid)
                        && self.is_workspace_focus_candidate(space, wid)
                });
            } else if !self.focus_untracked_window_under_cursor()
                && self.config.settings.mouse_follows_focus
                && let Some(screen) = self.space_state.screen_by_space(space)
            {
                self.warp_mouse(screen.frame.mid());
            }
        }
        if let Some(window) = window {
            if workspace_switch {
                self.layout_manager.layout_engine.commit_workspace_focus(
                    &mut self.state.windows,
                    space,
                    Some(window),
                );
            }
            if !workspace_switch || self.main_window() != Some(window) {
                self.stage_layout_focus(changes, Vec::new(), window, Quiet::Yes);
            }
        }
    }

    fn stage_layout_focus(
        &mut self,
        changes: &mut Changes,
        windows: Vec<WindowId>,
        window: WindowId,
        quiet: Quiet,
    ) {
        changes.focus_warp = None;
        let warp = {
            if changes.display_warp.is_some()
                || (self.config.settings.mouse_follows_focus
                    && (self.workspace_switch_manager.pending_workspace_switch_origin.is_some()
                        || self.assigned_space_for_window_id(window).is_some_and(|space| {
                            self.layout_manager.layout_engine.active_layout_mode_at(space)
                                == crate::common::config::LayoutMode::Scrolling
                        })))
            {
                changes.focus_warp = Some(window);
                None
            } else if self.config.settings.mouse_follows_focus {
                self.window_center_on_known_screen(window)
            } else {
                None
            }
        };
        self.stage_raise(changes, windows, Some((window, warp)), quiet);
    }

    fn stage_focus(&mut self, changes: &mut Changes, window: WindowId) {
        changes.focus_warp = None;
        self.stage_raise(changes, Vec::new(), Some((window, None)), Quiet::Yes);
    }

    fn stage_raise(
        &self,
        changes: &mut Changes,
        windows: Vec<WindowId>,
        focus: Option<(WindowId, Option<CGPoint>)>,
        quiet: Quiet,
    ) {
        if focus.is_none() && windows.is_empty() {
            return;
        }
        let mut request = match changes.focus.take() {
            Some(raise_manager::Event::RaiseRequest(request)) => request,
            _ => RaiseRequest {
                raise_windows: Vec::new(),
                focus_window: None,
                app_handles: HashMap::default(),
                focus_quiet: quiet,
            },
        };
        for window in windows {
            let Some(space) = self.layout_focus_space(window) else {
                continue;
            };
            if let Some(group) = request.raise_windows.iter_mut().find(|group| {
                group[0].pid == window.pid
                    && self.assigned_space_for_window_id(group[0]) == Some(space)
            }) {
                if !group.contains(&window) {
                    group.push(window);
                }
            } else {
                self.insert_app_handle_for_window(&mut request.app_handles, window);
                request.raise_windows.push(vec![window]);
            }
        }
        if let Some((window, _)) = focus {
            self.insert_app_handle_for_window(&mut request.app_handles, window);
            request.focus_window = focus;
            request.focus_quiet = quiet;
        }
        if request.focus_window.is_some() || !request.raise_windows.is_empty() {
            changes.focus = Some(raise_manager::Event::RaiseRequest(request));
        }
    }

    pub(crate) fn window_id_under_cursor(&self) -> Option<WindowId> {
        self.tracked_window_under_cursor().map(|(_, wid)| wid)
    }

    fn window_server_id_under_cursor(&self) -> Option<WindowServerId> {
        window_server::window_under_cursor()
    }

    fn tracked_window_under_cursor(&self) -> Option<(WindowServerId, WindowId)> {
        let wsid = self.window_server_id_under_cursor()?;
        let wid = self.state.windows.tracked_window_id(wsid)?;
        Some((wsid, wid))
    }

    fn activation_from_unmanageable_window(&self, pid: pid_t) -> Option<WindowServerId> {
        let (wsid, wid) = self.tracked_window_under_cursor()?;
        self.state.windows.window(wid)?;
        (wid.pid == pid && !self.state.windows.is_admitted(wid)).then_some(wsid)
    }

    fn focus_untracked_window_under_cursor(&mut self) -> bool {
        let Some(wsid) = self.window_server_id_under_cursor() else {
            return false;
        };
        if self.state.windows.tracked_window_id(wsid).is_some() {
            return false;
        }

        let window_info = self
            .state
            .windows
            .get_window_server_info(wsid)
            .or_else(|| window_server::get_window(wsid));

        let Some(info) = window_info else { return false };
        // The untracked-window fallback exists for ordinary application
        // windows that are intentionally outside Rift's model. Desktop,
        // menu-bar, Dock, and other system surfaces use nonzero layers and
        // must never be made key merely because the pointer crossed them.
        if info.layer != 0 {
            trace!(
                ?wsid,
                layer = info.layer,
                "Skipping non-application surface under cursor"
            );
            return false;
        }
        window_server::make_key_window(info.pid, wsid).is_ok()
    }

    fn focus_desktop_if_active_workspace_empty(&mut self, space: SpaceId) -> bool {
        if !self.is_space_active(space)
            || !self
                .layout_manager
                .layout_engine
                .workspaces()
                .windows_in_active_workspace(&self.state.windows, space)
                .is_empty()
        {
            return false;
        }
        let Some(screen) = self.space_state.screen_by_space(space) else {
            return false;
        };
        if !window_server::focus_desktop_window(screen) {
            return false;
        }

        self.layout_manager.layout_engine.commit_workspace_focus(
            &mut self.state.windows,
            space,
            None,
        );
        true
    }

    fn last_focused_window_in_space(&self, space: SpaceId) -> Option<WindowId> {
        let active_workspace =
            self.layout_manager.layout_engine.workspaces().active_workspace(space)?;
        let wid = self
            .layout_manager
            .layout_engine
            .workspaces()
            .last_focused_window(space, active_workspace)?;
        let window = self.state.windows.window(wid)?;

        if self.best_space_for_window_id(wid)? != space {
            return None;
        }
        if window
            .info
            .sys_id
            .is_some_and(|wsid| !self.state.windows.is_window_visible(wsid))
        {
            return None;
        }
        Some(wid)
    }

    fn visible_focus_candidate_in_active_workspace(
        &self,
        space: SpaceId,
        preferred: Option<WindowId>,
    ) -> Option<WindowId> {
        let is_visible_in_space = |wid| {
            self.state.windows.can_place_window(wid)
                && self.is_workspace_focus_candidate(space, wid)
        };

        if let Some(wid) = preferred.filter(|wid| is_visible_in_space(*wid)) {
            return Some(wid);
        }

        if let Some(wid) =
            self.last_focused_window_in_space(space).filter(|wid| is_visible_in_space(*wid))
        {
            return Some(wid);
        }

        self.layout_manager
            .layout_engine
            .workspaces()
            .windows_in_active_workspace(&self.state.windows, space)
            .into_iter()
            .find(|wid| is_visible_in_space(*wid))
    }

    fn is_workspace_focus_candidate(&self, space: SpaceId, wid: WindowId) -> bool {
        let windows = &self.state.windows;
        windows.window(wid).is_some_and(|window| window.info.sys_id.is_some())
            && windows.is_admitted(wid)
            && !windows.is_temporarily_unavailable(wid)
            && self.is_space_active(space)
            && self
                .layout_manager
                .layout_engine
                .workspaces()
                .active_workspace(space)
                .is_some_and(|workspace| windows.workspace_for_window(wid) == Some(workspace))
            && self.best_space_for_window_id(wid) == Some(space)
    }

    fn request_refocus_if_hidden(&mut self, space: SpaceId, window_id: WindowId) {
        // Membership refreshes also observe unfocused windows on inactive
        // workspaces. Only hidden focus needs a replacement; otherwise this
        // would activate that display's selection and warp the cursor back.
        if (self.main_window() == Some(window_id)
            || self.layout_manager.layout_engine.focused_window() == Some(window_id))
            && self.window_in_non_active_workspace(space, window_id)
        {
            self.refocus_manager.refocus_state = RefocusState::Pending(space);
        }
    }

    fn window_in_non_active_workspace(&self, space: SpaceId, window_id: WindowId) -> bool {
        let Some(active_workspace) =
            self.layout_manager.layout_engine.workspaces().active_workspace(space)
        else {
            return false;
        };
        self.layout_manager
            .layout_engine
            .workspaces()
            .workspace_for_window(&self.state.windows, space, window_id)
            .is_some_and(|window_workspace| window_workspace != active_workspace)
    }

    fn prepare_refocus_before_removal(&mut self, event: &LayoutEvent) {
        if !matches!(event, LayoutEvent::AppClosed(_) | LayoutEvent::WindowRemoved(_)) {
            return;
        }
        let focused = self.layout_manager.layout_engine.focused_window().or_else(|| {
            // AX suspension clears current focus before authoritative removal arrives.
            // Use workspace history without visibility filtering: the departing window
            // may already be ordered out or absent from the window store.
            let space = self.workspace_command_space()?;
            let workspaces = self.layout_manager.layout_engine.workspaces();
            workspaces.last_focused_window(space, workspaces.active_workspace(space)?)
        });
        let removed_focus = match event {
            LayoutEvent::AppClosed(pid) => focused.filter(|wid| wid.pid == *pid),
            LayoutEvent::WindowRemoved(wid) if focused == Some(*wid) => focused,
            _ => None,
        };
        if let Some(wid) = removed_focus
            && let Some(space) = self
                .layout_manager
                .layout_engine
                .space_with_window(wid)
                .filter(|space| self.is_space_active(*space))
                .or_else(|| self.workspace_command_space())
        {
            // A native tab departure can arrive before either AX or the debounced
            // WindowServer focus notification. Sample native focus before raising
            // a fallback, otherwise that raise can steal focus from the new tab.
            #[cfg(not(test))]
            let native_focus = matches!(event, LayoutEvent::WindowRemoved(_))
                .then(|| window_server::key_focused_window(space))
                .flatten();
            #[cfg(test)]
            let native_focus = self.native_focus_for_removal;
            if matches!(event, LayoutEvent::WindowRemoved(_))
                && let Some(native) = native_focus
                && native.pid == wid.pid
                && native != wid
            {
                let successor = self
                    .state
                    .windows
                    .tracked_window_id(WindowServerId::new(native.idx.get()))
                    .unwrap_or(native);
                if successor != wid && !self.window_in_non_active_workspace(space, successor) {
                    if !self.state.windows.contains_window(successor) {
                        self.request_window_inventory(successor.pid);
                    }
                    debug!(?wid, ?successor, "Preserving native focus during window removal");
                    return;
                }
            }
            self.refocus_manager.refocus_state = RefocusState::Pending(space);
        }
    }

    #[instrument(skip(self))]
    fn clear_menu_state_for_pid(&mut self, pid: pid_t) {
        if matches!(self.menu_manager.menu_state, MenuState::Open(owner) if owner == pid) {
            debug!(pid, "Clearing menu-open state for deactivated app");
            self.menu_manager.menu_state = MenuState::Closed;
            self.update_focus_follows_mouse_state();
        }
    }

    fn clear_menu_state_for_non_owner(&mut self, pid: pid_t) {
        if matches!(self.menu_manager.menu_state, MenuState::Open(owner) if owner != pid) {
            debug!(pid, "Clearing stale menu-open state after app focus changed");
            self.menu_manager.menu_state = MenuState::Closed;
            self.update_focus_follows_mouse_state();
        }
    }

    fn set_focus_follows_mouse_enabled(&self, enabled: bool) {
        if let Some(input_tx) = self.communication_manager.input_tx.as_ref() {
            input_tx.send(input::Request::SetFocusFollowsMouseEnabled(enabled));
        }
    }

    fn update_focus_follows_mouse_state(&mut self) {
        let should_enable = self.config.settings.focus_follows_mouse
            && matches!(self.menu_manager.menu_state, MenuState::Closed)
            && !self.is_mission_control_active()
            && !self.state.windows.suppresses_focus_follows_mouse(self.main_window());
        self.set_focus_follows_mouse_enabled(should_enable);
    }

    fn update_event_tap_layout_mode(&mut self) {
        let Some(input_tx) = self.communication_manager.input_tx.as_ref() else {
            return;
        };

        let last_modes = &self.notification_manager.last_layout_modes_by_space;
        let mut modes: Vec<(SpaceId, crate::common::config::LayoutMode)> =
            Vec::with_capacity(self.space_state.screens.len());
        let mut changed = false;

        for screen in &self.space_state.screens {
            let Some(space) = screen.space else {
                continue;
            };

            // Native visibility does not imply Rift management is enabled. Do
            // not let input claim gestures that the reactor will reject.
            if !self.is_space_active(space) {
                continue;
            }

            // Keep first occurrence only if multiple screens briefly report the same space.
            if modes.iter().any(|(existing, _)| *existing == space) {
                continue;
            }

            let engine = &self.layout_manager.layout_engine;
            let mut mode = engine.active_layout_mode_at(space);
            // This cache routes gesture candidates, not public layout mode. An
            // empty/fullscreen strip must still permit workspace navigation.
            if mode == crate::common::config::LayoutMode::Scrolling &&
                !engine.workspaces().active_layout_for_space(space).is_some_and(|(ws, layout)| {
                    matches!(&engine.workspaces()[ws].layout_system,
                        crate::layout_engine::LayoutSystemKind::Scrolling(system) if system.viewport_gesture_available(layout))
                }) {
                mode = crate::common::config::LayoutMode::Traditional;
            }
            if last_modes.get(&space).copied() != Some(mode) {
                changed = true;
            }
            modes.push((space, mode));
        }

        if !changed && modes.len() == last_modes.len() {
            return;
        }

        let modes_by_space = modes.iter().copied().collect();
        self.notification_manager.last_layout_modes_by_space = modes_by_space;
        input_tx.send(crate::actor::input::Request::LayoutModesChanged(modes));
    }

    fn set_mission_control_active(&mut self, active: bool) {
        let new_state = if active {
            MissionControlState::Active
        } else {
            MissionControlState::Inactive
        };
        if self.is_mission_control_active() == active {
            return;
        }
        self.mission_control_manager.mission_control_state = new_state;
        self.update_focus_follows_mouse_state();
    }

    fn refresh_windows_after_mission_control(&mut self, changes: &mut Changes) {
        debug!("Refreshing window state after Mission Control");
        // AX inventories are space-filtered, so only request one when a user Space
        // is active. The inventory coordinator rejects replies from older topology.
        if !self.has_user_space_context() {
            return;
        }
        let active_windows = self.authoritative_active_space_windows();
        self.refresh_windows_after_mission_control_with_active_windows(changes, active_windows);
    }

    fn refresh_windows_after_mission_control_with_active_windows(
        &mut self,
        changes: &mut Changes,
        active_windows: Vec<(WindowServerId, Option<SpaceId>)>,
    ) {
        if self.refreshes_blocked() {
            self.defer_window_inventory_refresh();
            return;
        }

        // Mission Control can move windows between native spaces without emitting a
        // matching destroy/appear pair for the origin space. Reconcile the active
        // spaces from the same space-aware WS-id list used everywhere else so we do
        // not depend on the global CG on-screen window list during recovery.
        self.reconcile_authoritative_active_window_snapshot(
            changes,
            active_windows,
            !self.space_state.membership_complete,
            &[],
        );
        changes.inventory_all = true;
        changes.arrange(None, Presentation::Animated);
    }

    fn has_user_space_context(&self) -> bool {
        self.raw_command_space().is_some_and(|space| !self.is_fullscreen_space(space))
    }

    fn request_close_window(&mut self, pid: pid_t, window_server_id: Option<WindowServerId>) {
        if let Some(app) = self.app_manager.apps.get(&pid) {
            if let Err(err) = app.handle.send(Request::CloseWindow(window_server_id)) {
                warn!(
                    pid,
                    ?window_server_id,
                    "Failed to send close window request: {}",
                    err
                );
            }
        }
    }

    pub(crate) fn main_window(&self) -> Option<WindowId> { self.main_window_tracker.main_window() }

    fn main_window_space(&self) -> Option<SpaceId> {
        // TODO: Optimize this with a cache or something.
        let wid = self.main_window()?;
        self.best_space_for_window_id(wid)
    }

    /// Window discovery is scoped to one application. It may restore that
    /// application's current focus after its windows have been inserted into
    /// the layout, but it must never replay another application's global main
    /// window. Requiring the command space also prevents a refresh racing an
    /// active-display change from restoring focus on the display being left.
    fn focused_window_for_discovery(
        &self,
        pid: pid_t,
        spaces: &HashMap<WindowId, (Option<SpaceId>, Option<SpaceId>)>,
    ) -> Option<(SpaceId, WindowId)> {
        let window = self.main_window().filter(|window| window.pid == pid)?;
        let &(authoritative, discovery) = spaces.get(&window)?;
        let space = authoritative.or_else(|| {
            let wsid = self.state.windows.record(window)?.window_server_id();
            (!wsid.is_some_and(|wsid| self.is_known_fullscreen_window(wsid)))
                .then_some(discovery)
                .flatten()
        })?;
        (self.workspace_command_space() == Some(space)).then_some((space, window))
    }

    fn raw_command_space(&self) -> Option<SpaceId> { self.space_state.command_space }

    fn active_display_space(&self) -> Option<SpaceId> {
        self.raw_command_space()
            .filter(|space| {
                self.space_state.active_spaces.contains(space)
                    && self.space_state.screens.iter().any(|screen| screen.space == Some(*space))
            })
            .or_else(|| {
                self.space_state
                    .screens
                    .iter()
                    .filter_map(|screen| screen.space)
                    .find(|space| self.space_state.active_spaces.contains(space))
            })
    }

    fn workspace_command_space(&self) -> Option<SpaceId> {
        self.active_display_space().filter(|space| self.is_space_active(*space))
    }

    fn command_context_space(&self) -> Option<SpaceId> {
        self.workspace_command_space().or_else(|| {
            self.layout_manager
                .layout_engine
                .focused_window()
                .and_then(|wid| {
                    self.assigned_space_for_window_id(wid)
                        .or_else(|| self.best_space_for_window_id(wid))
                })
                .filter(|space| self.is_space_active(*space))
                .or_else(|| self.main_window_space().filter(|space| self.is_space_active(*space)))
        })
    }

    fn screen_for_point(&self, point: CGPoint) -> Option<&ScreenInfo> {
        self.space_state.screens.iter().find(|screen| screen.frame.contains(point))
    }

    fn current_screen_center(&self) -> Option<CGPoint> {
        if let Some(space) = self.raw_command_space() {
            if let Some(screen) = self.space_state.screen_by_space(space) {
                return Some(screen.frame.mid());
            }
        }

        self.space_state.screens.first().map(|screen| screen.frame.mid())
    }

    fn screen_for_direction_from_point(
        &self,
        origin: CGPoint,
        direction: Direction,
        excluded_origin: Option<CGPoint>,
    ) -> Option<&ScreenInfo> {
        fn interval_gap(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> f64 {
            if a_max < b_min {
                b_min - a_max
            } else if b_max < a_min {
                a_min - b_max
            } else {
                0.0
            }
        }

        let mut best: Option<(f64, f64, &ScreenInfo)> = None;

        for screen in &self.space_state.screens {
            let frame = screen.frame;

            if frame.contains(origin) || excluded_origin.is_some_and(|point| frame.contains(point))
            {
                continue;
            }

            let min = frame.min();
            let max = frame.max();

            let (edge, orth_gap) = match direction {
                Direction::Left => (
                    CGPoint::new(max.x, origin.y),
                    interval_gap(min.y, max.y, origin.y, origin.y),
                ),
                Direction::Right => (
                    CGPoint::new(min.x, origin.y),
                    interval_gap(min.y, max.y, origin.y, origin.y),
                ),
                Direction::Up => (
                    CGPoint::new(origin.x, max.y),
                    interval_gap(min.x, max.x, origin.x, origin.x),
                ),
                Direction::Down => (
                    CGPoint::new(origin.x, min.y),
                    interval_gap(min.x, max.x, origin.x, origin.x),
                ),
            };
            let Some(primary_dist) =
                (origin.x, origin.y).distance_in_direction((edge.x, edge.y), direction)
            else {
                continue;
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
    ) -> Option<&ScreenInfo> {
        match selector {
            DisplaySelector::Direction(direction) => {
                let origin = origin_override.or_else(|| self.current_screen_center())?;
                self.screen_for_direction_from_point(origin, *direction, None)
            }
            DisplaySelector::Index(index) => self.screens_in_physical_order().get(*index).copied(),
            DisplaySelector::Uuid(uuid) => {
                self.space_state.screens.iter().find(|screen| screen.display_uuid == *uuid)
            }
        }
    }

    fn screen_for_selector_wrapping(
        &self,
        selector: &DisplaySelector,
        origin_override: Option<CGPoint>,
    ) -> Option<&ScreenInfo> {
        if let Some(screen) = self.screen_for_selector(selector, origin_override) {
            return Some(screen);
        }
        let DisplaySelector::Direction(direction) = selector else {
            return None;
        };
        let origin = origin_override.or_else(|| self.current_screen_center())?;
        let screens = &self.space_state.screens;
        let wrapped_origin = match direction {
            Direction::Right => CGPoint::new(
                screens
                    .iter()
                    .map(|screen| screen.frame.min().x)
                    .min_by(|a, b| a.total_cmp(b))?
                    - 1.0,
                origin.y,
            ),
            Direction::Left => CGPoint::new(
                screens
                    .iter()
                    .map(|screen| screen.frame.max().x)
                    .max_by(|a, b| a.total_cmp(b))?
                    + 1.0,
                origin.y,
            ),
            Direction::Down => CGPoint::new(
                origin.x,
                screens
                    .iter()
                    .map(|screen| screen.frame.min().y)
                    .min_by(|a, b| a.total_cmp(b))?
                    - 1.0,
            ),
            Direction::Up => CGPoint::new(
                origin.x,
                screens
                    .iter()
                    .map(|screen| screen.frame.max().y)
                    .max_by(|a, b| a.total_cmp(b))?
                    + 1.0,
            ),
        };
        // The source can be the outermost display on both edges when displays
        // overlap along this axis (for example, vertically stacked displays).
        // Wrapping must still choose another display.
        self.screen_for_direction_from_point(wrapped_origin, *direction, Some(origin))
    }

    fn frame_on_display(
        &mut self,
        window: WindowId,
        frame: CGRect,
        source: Option<CGRect>,
        target: CGRect,
    ) -> CGRect {
        if self.layout_manager.layout_engine.is_window_floating(window) {
            self.state.windows.project_floating_frame(window, frame, source, target)
        } else {
            crate::sys::geometry::frame_on_display(frame, source, target, Default::default())
        }
    }

    /// In global mode, open a new window on the pointer's display unless an app rule
    /// names its workspace: launchers can activate an app on another display.
    fn place_new_window_on_pointer_display(
        &mut self,
        changes: &mut Changes,
        window: WindowId,
        space: SpaceId,
    ) {
        let engine = &self.layout_manager.layout_engine;
        if engine.workspaces().settings.mode != crate::common::config::WorkspaceMode::Global {
            return;
        }
        let Some(target_screen) = window_server::current_cursor_location()
            .ok()
            .and_then(|point| self.screen_for_point(point))
            .filter(|screen| screen.space.is_some_and(|s| s != space && self.is_space_active(s)))
            .cloned()
        else {
            return;
        };
        if !self.state.windows.window(window).is_some_and(|state| state.info.is_standard) {
            return;
        }
        self.move_window_to_display(changes, window, space, target_screen.space.unwrap());
    }

    /// A user-initiated transfer may add a workspace to an exposed empty display.
    fn ensure_window_move_destination(
        &mut self,
        changes: &mut Changes,
        space: SpaceId,
    ) -> Option<crate::model::VirtualWorkspaceId> {
        let workspaces = self.layout_manager.layout_engine.workspaces_mut();
        if let Some(id) = workspaces.active_workspace(space) {
            return Some(id);
        }
        if workspaces.effective_mode() != crate::common::config::WorkspaceMode::Global {
            return None;
        }
        match workspaces.create_workspace(space, None) {
            Ok(id) => {
                self.expose_space_if_known(space);
                changes.workspace_changed(space);
                Some(id)
            }
            Err(error) => {
                warn!(?space, ?error, "Cannot move window to an empty display");
                None
            }
        }
    }

    /// Moves one window into the destination's active workspace when available.
    fn move_window_to_display(
        &mut self,
        changes: &mut Changes,
        window: WindowId,
        source: SpaceId,
        target: SpaceId,
    ) {
        let Some(screen) = self.space_state.screen_by_space(target) else {
            return;
        };
        let response = self.layout_manager.layout_engine.move_window_to_space(
            &mut self.state.windows,
            source,
            target,
            screen.frame.size,
            window,
        );
        if !response.changed {
            return;
        }
        let source_frame = self.space_state.screen_by_space(source).map(|screen| screen.frame);
        changes.move_between(source, target, Presentation::Animated);
        self.request_window_transfer(changes, window, target, source_frame);
        self.reconcile_layout_response(changes, response, None);
    }

    fn follow_focused_window_to_its_display(&mut self, previous_space: Option<SpaceId>) {
        let Some(space) = self
            .layout_manager
            .layout_engine
            .focused_window()
            .and_then(|window| self.assigned_space_for_window_id(window))
            .filter(|space| Some(*space) != previous_space && self.is_space_active(*space))
        else {
            return;
        };
        let Some(uuid) = self.display_uuid_for_space(space) else {
            return;
        };
        if crate::sys::screen::set_active_menu_bar_display_uuid(&uuid) {
            self.space_state.menu_bar_space = Some(space);
        }
        self.space_state.command_space = Some(space);
    }

    fn screens_in_physical_order(&self) -> Vec<&ScreenInfo> {
        let mut screens: Vec<&ScreenInfo> = self.space_state.screens.iter().collect();
        screens.sort_by(|a, b| {
            (a.frame.origin.x.total_cmp(&b.frame.origin.x))
                .then(a.frame.origin.y.total_cmp(&b.frame.origin.y))
                .then_with(|| a.display_uuid.cmp(&b.display_uuid))
        });
        screens
    }

    fn store_current_floating_positions(&mut self, space: SpaceId) {
        let floating_windows_in_workspace = self
            .layout_manager
            .layout_engine
            .workspaces()
            .windows_in_active_workspace(&self.state.windows, space)
            .into_iter()
            .filter(|&wid| self.layout_manager.layout_engine.is_window_floating(wid))
            .filter_map(|wid| {
                self.state
                    .windows
                    .window(wid)
                    .map(|window_state| (wid, window_state.frame_monotonic))
            })
            .collect::<Vec<_>>();

        if !floating_windows_in_workspace.is_empty() {
            self.layout_manager
                .layout_engine
                .store_floating_window_positions(space, &floating_windows_in_workspace);
        }
    }
}
