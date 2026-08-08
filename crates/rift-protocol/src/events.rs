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
        // Reconstruct the original 64-bit slotmap key (idx = low 32 bits,
        // version = high 32 bits, see `protocol_workspace_id`) so the string
        // is a stable, collision-free key instead of two decimal forms glued
        // together (`{1,23}` and `{12,3}` used to render identically).
        write!(f, "{:08}", ((self.version as u64) << 32) | (self.idx as u64))
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

    #[test]
    fn workspace_id_display_is_collision_free() {
        // `idx` is the low 32 bits and `version` the high 32 bits of the
        // original slotmap key (see `protocol_workspace_id`). Concatenating
        // their decimal forms drops positional info, so `{ idx: 1, version: 23 }`
        // and `{ idx: 12, version: 3 }` both used to render as "00000123".
        assert_ne!(
            WorkspaceId { idx: 1, version: 23 }.to_string(),
            WorkspaceId { idx: 12, version: 3 }.to_string()
        );
    }

    #[test]
    fn workspace_id_display_round_trips_the_reconstructed_key() {
        // Concrete oracles for the previously-colliding pair: `idx` is the low
        // 32 bits, `version` the high 32 bits of the original slotmap key.
        assert_eq!(WorkspaceId { idx: 1, version: 23 }.to_string(), "98784247809");
        assert_eq!(WorkspaceId { idx: 12, version: 3 }.to_string(), "12884901900");
        assert_eq!(WorkspaceId { idx: 0, version: 0 }.to_string(), "00000000");

        // Round-trip the documented formula across edge values. This also pins
        // the `as u64` casts (a bare `version << 32` would overflow at u32).
        for (idx, version) in [
            (0u32, 0u32),
            (1u32, 0u32),
            (0u32, 1u32),
            (1u32, 23u32),
            (12u32, 3u32),
            (u32::MAX, 0u32),
            (0u32, u32::MAX),
            (u32::MAX, u32::MAX),
        ] {
            let wid = WorkspaceId { idx, version };
            assert_eq!(
                wid.to_string(),
                format!("{:08}", (wid.version as u64) << 32 | wid.idx as u64)
            );
        }
    }
}
