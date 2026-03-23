use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::actor::app::{AppInfo, AppThreadHandle, WindowId, pid_t};
use crate::common::log::MetricsCommand;
use crate::layout_engine::{Direction, LayoutCommand};
use crate::sys::app::WindowInfo;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

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
    ToggleSpaceActivated,
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

#[derive(Debug, Clone)]
pub(crate) struct FullscreenWindowTrack {
    pub(crate) pid: pid_t,
    pub(crate) window_id: Option<WindowId>,
    pub(crate) last_known_user_space: Option<SpaceId>,
    pub(crate) _last_seen_fullscreen_space: SpaceId,
}

#[derive(Debug, Clone)]
pub(crate) struct FullscreenSpaceTrack {
    pub(crate) windows: Vec<FullscreenWindowTrack>,
}

impl Default for FullscreenSpaceTrack {
    fn default() -> Self { FullscreenSpaceTrack { windows: Vec::new() } }
}

#[derive(Debug, Clone)]
pub struct DragSession {
    pub(crate) window: WindowId,
    pub(crate) last_frame: CGRect,
    pub(crate) origin_space: Option<SpaceId>,
    pub(crate) settled_space: Option<SpaceId>,
    pub(crate) layout_dirty: bool,
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
    Open(pid_t),
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

#[derive(Debug)]
pub(crate) struct AppState {
    #[allow(unused)]
    pub(crate) info: AppInfo,
    pub(crate) handle: AppThreadHandle,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingSpaceChange {
    pub(crate) spaces: Vec<Option<SpaceId>>,
}

#[derive(Debug)]
pub(crate) struct WindowState {
    pub(crate) info: WindowInfo,
    /// The last known frame of the window. Always includes the last write.
    ///
    /// This value only updates monotonically with respect to writes; in other
    /// words, we only accept reads when we know they come after the last write.
    pub(crate) frame_monotonic: CGRect,
    pub(crate) is_manageable: bool,
    pub(crate) ignore_app_rule: bool,
    pub(crate) native_tab: Option<NativeTabMembership>,
}

impl From<WindowInfo> for WindowState {
    fn from(info: WindowInfo) -> WindowState {
        WindowState {
            frame_monotonic: info.frame,
            info,
            is_manageable: false,
            ignore_app_rule: false,
            native_tab: None,
        }
    }
}

impl WindowState {
    pub(crate) fn is_native_tab_suppressed(&self) -> bool {
        matches!(
            self.native_tab,
            Some(NativeTabMembership {
                role: NativeTabRole::Suppressed,
                ..
            })
        )
    }

    pub(crate) fn is_effectively_manageable(&self) -> bool {
        self.is_manageable && !self.ignore_app_rule && !self.is_native_tab_suppressed()
    }

    pub(crate) fn matches_filter(&self, filter: WindowFilter) -> bool {
        match filter {
            WindowFilter::Manageable => self.is_manageable && !self.is_native_tab_suppressed(),
            WindowFilter::EffectivelyManageable => self.is_effectively_manageable(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeTabMembership {
    pub(crate) group_id: u32,
    pub(crate) role: NativeTabRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeTabRole {
    Active,
    Suppressed,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum WindowFilter {
    Manageable,
    EffectivelyManageable,
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReactorError {
    #[error("App communication failed: {0}")]
    AppCommunicationFailed(#[from] tokio::sync::mpsc::error::SendError<crate::actor::app::Request>),
    #[error("Stack line communication failed: {0}")]
    StackLineCommunicationFailed(
        #[from] tokio::sync::mpsc::error::TrySendError<crate::actor::stack_line::Event>,
    ),
    #[error("Raise manager communication failed: {0}")]
    RaiseManagerCommunicationFailed(
        #[from] tokio::sync::mpsc::error::SendError<crate::actor::raise_manager::Event>,
    ),
}
