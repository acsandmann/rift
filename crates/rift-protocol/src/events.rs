use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{LayoutKind, WindowId};

/// Events available through the Mach subscription API.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    WorkspaceChanged,
    WindowsChanged,
    WindowTitleChanged,
    FocusedWindowChanged,
    StacksChanged,
    #[serde(rename = "*")]
    All,
}

impl EventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceChanged => "workspace_changed",
            Self::WindowsChanged => "windows_changed",
            Self::WindowTitleChanged => "window_title_changed",
            Self::FocusedWindowChanged => "focused_window_changed",
            Self::StacksChanged => "stacks_changed",
            Self::All => "*",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
}

/// The typed payload delivered for a subscription event.
///
/// This intentionally mirrors the existing JSON event shape so older Lua and
/// CLI clients can continue consuming the same payloads unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RiftEvent {
    WorkspaceChanged {
        space_id: u64,
        workspace_id: WorkspaceId,
        workspace_name: String,
        display_uuid: Option<String>,
    },
    WindowsChanged {
        workspace_id: WorkspaceId,
        workspace_name: String,
        windows: Vec<String>,
        space_id: u64,
        display_uuid: Option<String>,
    },
    WindowTitleChanged {
        window_id: WindowId,
        workspace_id: WorkspaceId,
        workspace_index: Option<u64>,
        workspace_name: String,
        previous_title: String,
        new_title: String,
        space_id: u64,
        display_uuid: Option<String>,
    },
    FocusedWindowChanged {
        window_id: WindowId,
        workspace_id: WorkspaceId,
        workspace_index: Option<u64>,
        workspace_name: String,
        space_id: u64,
        display_uuid: Option<String>,
    },
    StacksChanged {
        workspace_id: WorkspaceId,
        workspace_index: Option<u64>,
        workspace_name: String,
        stacks: Vec<StackInfo>,
        active_workspace_has_fullscreen: bool,
        space_id: u64,
        display_uuid: Option<String>,
    },
}

impl RiftEvent {
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::WorkspaceChanged { .. } => EventKind::WorkspaceChanged,
            Self::WindowsChanged { .. } => EventKind::WindowsChanged,
            Self::WindowTitleChanged { .. } => EventKind::WindowTitleChanged,
            Self::FocusedWindowChanged { .. } => EventKind::FocusedWindowChanged,
            Self::StacksChanged { .. } => EventKind::StacksChanged,
        }
    }

    pub const fn space_id(&self) -> u64 {
        match self {
            Self::WorkspaceChanged { space_id, .. }
            | Self::WindowsChanged { space_id, .. }
            | Self::WindowTitleChanged { space_id, .. }
            | Self::FocusedWindowChanged { space_id, .. }
            | Self::StacksChanged { space_id, .. } => *space_id,
        }
    }

    pub fn display_uuid(&self) -> Option<&str> {
        match self {
            Self::WorkspaceChanged { display_uuid, .. }
            | Self::WindowsChanged { display_uuid, .. }
            | Self::WindowTitleChanged { display_uuid, .. }
            | Self::FocusedWindowChanged { display_uuid, .. }
            | Self::StacksChanged { display_uuid, .. } => display_uuid.as_deref(),
        }
    }
}

/// The serialized identity of a virtual workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId {
    pub idx: u32,
    pub version: u32,
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08}", format!("{}{}", self.idx, self.version))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StackInfo {
    pub container_kind: LayoutKind,
    pub total_count: usize,
    pub selected_index: usize,
    pub windows: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_event_preserves_the_legacy_wire_shape() {
        let event = RiftEvent::WorkspaceChanged {
            space_id: 42,
            workspace_id: WorkspaceId { idx: 3, version: 1 },
            workspace_name: "main".into(),
            display_uuid: Some("display".into()),
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "type": "workspace_changed",
                "space_id": 42,
                "workspace_id": { "idx": 3, "version": 1 },
                "workspace_name": "main",
                "display_uuid": "display"
            })
        );
    }
}
