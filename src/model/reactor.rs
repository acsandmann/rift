use objc2_core_foundation::CGRect;
pub use rift_protocol::{DisplaySelector, ReactorCommand};
use serde::{Deserialize, Serialize};

use crate::actor::app::{AppInfo, AppThreadHandle, WindowId, pid_t};
use crate::common::log::MetricsCommand;
use crate::layout_engine::{LayoutCommand, WindowLayoutInfo};
use crate::model::WindowStore;
use crate::sys::app::WindowInfo;
use crate::sys::screen::SpaceId;

/// All mutable domain state is owned by the reactor thread.
///
/// Workspace topology is still carried by the layout coordinator during this
/// migration, but window identity, native-space observations, and workspace
/// assignments have one explicit owner here. Cross-store operations receive
/// this store by reference instead of retaining an alias to it.
#[derive(Debug, Default)]
pub struct RiftState {
    pub windows: WindowStore,
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
pub(crate) struct WindowState {
    pub(crate) info: WindowInfo,
    /// The last known frame of the window. Always includes the last write.
    ///
    /// This value only updates monotonically with respect to writes; in other
    /// words, we only accept reads when we know they come after the last write.
    pub(crate) frame_monotonic: CGRect,
    /// Rift/macOS heuristic result, kept separately from an explicit app-rule
    /// override so every discovered window can remain tracked.
    pub(crate) is_manageable: bool,
    pub(crate) manage_override: Option<bool>,
}

impl From<WindowInfo> for WindowState {
    fn from(info: WindowInfo) -> WindowState {
        WindowState {
            frame_monotonic: info.frame,
            info,
            is_manageable: false,
            manage_override: None,
        }
    }
}

impl WindowState {
    pub(crate) fn layout_info(&self, wid: WindowId) -> WindowLayoutInfo {
        (
            wid,
            Some(self.info.title.clone()),
            self.info.ax_role.clone(),
            self.info.ax_subrole.clone(),
            self.info.is_resizable,
            self.frame_monotonic.size,
            self.info.min_size,
            self.info.max_size,
        )
    }

    /// The single admission policy used by every layout-facing caller.
    pub(crate) fn is_admitted(&self) -> bool {
        self.is_admitted_with_override(self.manage_override)
    }

    pub(crate) fn is_admitted_with_override(&self, manage_override: Option<bool>) -> bool {
        !self.info.is_minimized && manage_override.unwrap_or(self.is_manageable)
    }

    pub(crate) fn can_reconcile_admission(&self) -> bool { !self.info.is_minimized }
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

#[cfg(test)]
mod tests {
    use rift_protocol::{RestoreScope, RestoreSource};

    use super::*;

    #[test]
    fn legacy_restore_command_defaults_to_portable_source_policy() {
        let mut serialized = serde_json::to_value(ReactorCommand::RestoreLayout {
            path: "layout.ron".into(),
            scope: RestoreScope::Workspace,
            source: RestoreSource::CurrentSpace,
        })
        .unwrap();
        serialized
            .get_mut("restore_layout")
            .and_then(serde_json::Value::as_object_mut)
            .unwrap()
            .remove("source");

        let restored: ReactorCommand = serde_json::from_value(serialized).unwrap();

        assert_eq!(restored, ReactorCommand::RestoreLayout {
            path: "layout.ron".into(),
            scope: RestoreScope::Workspace,
            source: RestoreSource::SavedActiveSpace,
        });
    }
}
