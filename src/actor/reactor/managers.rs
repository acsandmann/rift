use std::time::Instant;

use objc2_core_foundation::CGRect;

use super::main_window::MainWindowTracker;
use super::replay::Record;
use super::{
    AppState, AutoWorkspaceSwitch, Event, FullscreenTrack, PendingSpaceChange, Screen, WindowState,
};
use crate::actor;
use crate::actor::app::{WindowId, pid_t};
use crate::actor::broadcast::BroadcastSender;
use crate::actor::drag_swap::DragManager as DragSwapManager;
use crate::actor::{event_tap, menu_bar, raise_manager, stack_line, window_notify, wm_controller};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{Config, WindowSnappingSettings};
use crate::layout_engine::LayoutEngine;
use crate::model::tx_store::WindowTxStore;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

/// Manages window state and lifecycle
pub struct WindowManager {
    pub windows: HashMap<WindowId, WindowState>,
    pub window_ids: HashMap<WindowServerId, WindowId>,
    pub visible_windows: HashSet<WindowServerId>,
    pub observed_window_server_ids: HashSet<WindowServerId>,
}

/// Manages application state and rules
pub struct AppManager {
    pub apps: HashMap<pid_t, AppState>,
    pub app_rules_recently_applied: Instant,
}

/// Manages space and screen state
pub struct SpaceManager {
    pub screens: Vec<Screen>,
    pub fullscreen_by_space: HashMap<u64, FullscreenTrack>,
    pub changing_screens: HashSet<WindowServerId>,
}

/// Manages drag operations and window swapping
pub struct DragManager {
    pub drag_state: super::DragState,
    pub drag_swap_manager: DragSwapManager,
    pub skip_layout_for_window: Option<WindowId>,
}

impl DragManager {
    pub fn reset(&mut self) { self.drag_swap_manager.reset(); }

    pub fn last_target(&self) -> Option<WindowId> { self.drag_swap_manager.last_target() }

    pub fn dragged(&self) -> Option<WindowId> { self.drag_swap_manager.dragged() }

    pub fn origin_frame(&self) -> Option<CGRect> { self.drag_swap_manager.origin_frame() }

    pub fn update_config(&mut self, config: WindowSnappingSettings) {
        self.drag_swap_manager.update_config(config);
    }
}

/// Manages window notifications and transaction store
pub struct NotificationManager {
    pub last_sls_notification_ids: Vec<u32>,
    pub window_notify_tx: Option<window_notify::Sender>,
    pub window_tx_store: Option<WindowTxStore>,
}

/// Manages menu state and interactions
pub struct MenuManager {
    pub menu_state: super::MenuState,
    pub menu_tx: Option<menu_bar::Sender>,
}

/// Manages Mission Control state
pub struct MissionControlManager {
    pub mission_control_state: super::MissionControlState,
    pub pending_mission_control_refresh: HashSet<pid_t>,
}

/// Manages workspace switching state
pub struct WorkspaceSwitchManager {
    pub workspace_switch_state: super::WorkspaceSwitchState,
    pub workspace_switch_generation: u64,
    pub active_workspace_switch: Option<u64>,
    pub last_auto_workspace_switch: Option<AutoWorkspaceSwitch>,
}

/// Manages refocus and cleanup state
pub struct RefocusManager {
    pub stale_cleanup_state: super::StaleCleanupState,
    pub refocus_state: super::RefocusState,
}

/// Manages communication channels to other actors
pub struct CommunicationManager {
    pub event_tap_tx: Option<event_tap::Sender>,
    pub stack_line_tx: Option<stack_line::Sender>,
    pub raise_manager_tx: raise_manager::Sender,
    pub event_broadcaster: BroadcastSender,
    pub wm_sender: Option<wm_controller::Sender>,
    pub events_tx: Option<actor::Sender<Event>>,
}

/// Manages recording state
pub struct RecordingManager {
    pub record: Record,
}

/// Manages configuration state
pub struct ConfigManager {
    pub config: Config,
}

/// Manages layout engine state
pub struct LayoutManager {
    pub layout_engine: LayoutEngine,
}

/// Manages window server information
pub struct WindowServerInfoManager {
    pub window_server_info: HashMap<WindowServerId, WindowServerInfo>,
}

/// Manages main window tracking
pub struct MainWindowTrackerManager {
    pub main_window_tracker: MainWindowTracker,
}

/// Manages pending space changes
pub struct PendingSpaceChangeManager {
    pub pending_space_change: Option<PendingSpaceChange>,
}
