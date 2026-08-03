//! Shared, platform-neutral protocol types for Rift.
//!
//! The server owns the runtime model and the client owns the Mach transport,
//! but both use these types at the wire boundary. JSON encoding remains an
//! implementation detail of the transport crates.

mod commands;
mod events;
mod queries;

pub use commands::{
    ConfigCommand, Direction, DisplaySelector, LayoutCommand, LayoutMode, MetricsCommand,
    ReactorCommand, ResizeOrientation, RestoreScope, RestoreSource, RiftCommand, WorkspaceSelector,
};
pub use events::EventKind;
pub use queries::{
    ApplicationData, DisplayData, LayoutStateData, Point, Rect, Size, WindowData, WindowId,
    WorkspaceData, WorkspaceLayoutData,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request accepted by Rift's Mach IPC server.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiftRequest {
    GetWorkspaces {
        space_id: Option<u64>,
    },
    GetDisplays,
    GetWindows {
        space_id: Option<u64>,
    },
    GetWindowInfo {
        window_id: WindowId,
    },
    GetLayoutState {
        space_id: u64,
    },
    GetWorkspaceLayouts {
        space_id: Option<u64>,
        workspace_id: Option<usize>,
    },
    GetApplications,
    GetMetrics,
    GetConfig,
    ExecuteCommand {
        command: RiftCommand,
    },
    Subscribe {
        event: EventKind,
    },
    Unsubscribe {
        event: EventKind,
    },
    SubscribeCli {
        event: EventKind,
        command: String,
        args: Vec<String>,
    },
    UnsubscribeCli {
        event: EventKind,
    },
    ListCliSubscriptions,
}

/// The response envelope returned by Rift's Mach IPC server.
///
/// The success payload is generic so query methods can decode directly into
/// shared protocol types. Errors remain JSON values for forward-compatible
/// structured error fields.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RiftResponse<T = Value> {
    Success { data: T },
    Error { error: Value },
}

impl<T> RiftResponse<T> {
    pub fn into_result(self) -> Result<T, Value> {
        match self {
            Self::Success { data } => Ok(data),
            Self::Error { error } => Err(error),
        }
    }
}

/// The compatibility response type for callers that intentionally want raw
/// JSON values.
pub type JsonRiftResponse = RiftResponse<Value>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_typed_command_wire_shape() {
        let request = RiftRequest::ExecuteCommand {
            command: RiftCommand::Layout(LayoutCommand::NextWindow),
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "execute_command": { "command": { "layout": "next_window" } }
            })
        );
    }

    #[test]
    fn typed_response_decodes_shared_query_types() {
        let response: RiftResponse<Vec<WorkspaceData>> =
            serde_json::from_value(serde_json::json!({ "data": [{
                "id": "workspace-1",
                "index": 0,
                "name": "main",
                "layout_mode": "bsp",
                "is_active": true,
                "window_count": 0,
                "windows": []
            }] }))
            .unwrap();

        assert_eq!(response.into_result().unwrap()[0].name, "main");
    }
}
