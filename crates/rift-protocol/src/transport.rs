use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EventKind, RiftCommand, WindowId};

/// Behavior requested for the lifetime of an external window claim.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WindowClaimFlags(u32);

impl WindowClaimFlags {
    const SUPPORTED_BITS: u32 = Self::SUPPRESS_FOCUS_FOLLOWS_MOUSE.0;
    pub const SUPPRESS_FOCUS_FOLLOWS_MOUSE: Self = Self(1 << 0);

    pub const fn empty() -> Self { Self(0) }

    pub const fn bits(self) -> u32 { self.0 }

    pub const fn from_bits_retain(bits: u32) -> Self { Self(bits) }

    pub const fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }

    pub const fn is_supported(self) -> bool { self.0 & !Self::SUPPORTED_BITS == 0 }
}

impl std::ops::BitOr for WindowClaimFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

/// A request accepted by Rift's Mach IPC server.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiftRequest {
    GetWorkspaces {
        space_id: Option<u64>,
    },
    GetWorkspacesForDisplay {
        display_uuid: String,
    },
    GetDisplays,
    GetWindows {
        space_id: Option<u64>,
    },
    GetWindowsForDisplay {
        display_uuid: String,
    },
    GetWindowInfo {
        window_id: WindowId,
    },
    ClaimWindow {
        window_id: WindowId,
        #[serde(default)]
        flags: WindowClaimFlags,
    },
    ReleaseWindow {
        window_id: WindowId,
    },
    GetLayoutState {
        space_id: Option<u64>,
        workspace_id: Option<usize>,
    },
    GetLayoutStateForDisplay {
        display_uuid: String,
        workspace_id: Option<usize>,
    },
    GetWorkspaceLayouts {
        space_id: Option<u64>,
        workspace_id: Option<usize>,
    },
    GetWorkspaceLayoutsForDisplay {
        display_uuid: String,
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
    use crate::LayoutCommand;

    #[test]
    fn management_requests_round_trip() {
        for request in [
            RiftRequest::ClaimWindow {
                window_id: WindowId { pid: 7, idx: 3 },
                flags: WindowClaimFlags::SUPPRESS_FOCUS_FOLLOWS_MOUSE,
            },
            RiftRequest::ReleaseWindow {
                window_id: WindowId { pid: 7, idx: 3 },
            },
        ] {
            let encoded = serde_json::to_string(&request).unwrap();
            assert_eq!(serde_json::from_str::<RiftRequest>(&encoded).unwrap(), request);
        }
    }

    #[test]
    fn legacy_claim_defaults_to_empty_flags() {
        let request: RiftRequest = serde_json::from_value(serde_json::json!({
            "claim_window": { "window_id": { "pid": 7, "idx": 3 } }
        }))
        .unwrap();
        assert_eq!(request, RiftRequest::ClaimWindow {
            window_id: WindowId { pid: 7, idx: 3 },
            flags: WindowClaimFlags::empty(),
        });
    }

    #[test]
    fn claim_flags_are_an_integer_on_the_wire() {
        let request = RiftRequest::ClaimWindow {
            window_id: WindowId { pid: 7, idx: 3 },
            flags: WindowClaimFlags::SUPPRESS_FOCUS_FOLLOWS_MOUSE,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["claim_window"]["flags"], 1);
        assert_eq!(serde_json::from_value::<RiftRequest>(value).unwrap(), request);
        let unknown = WindowClaimFlags::from_bits_retain(0x8000_0001);
        assert_eq!(
            serde_json::from_str::<WindowClaimFlags>(&serde_json::to_string(&unknown).unwrap())
                .unwrap(),
            unknown
        );
        assert!(!unknown.is_supported());
    }

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
    fn layout_query_allows_the_server_to_select_the_active_space() {
        let request = RiftRequest::GetLayoutState {
            space_id: None,
            workspace_id: None,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "get_layout_state": { "space_id": null, "workspace_id": null }
            })
        );
        assert_eq!(
            serde_json::from_value::<RiftRequest>(serde_json::json!({ "get_layout_state": {} }))
                .unwrap(),
            RiftRequest::GetLayoutState {
                space_id: None,
                workspace_id: None,
            }
        );
    }

    #[test]
    fn display_queries_do_not_change_existing_query_shapes() {
        let legacy = serde_json::json!({ "get_windows": { "space_id": 7 } });
        assert_eq!(
            serde_json::from_value::<RiftRequest>(legacy.clone()).unwrap(),
            RiftRequest::GetWindows { space_id: Some(7) }
        );
        assert_eq!(
            serde_json::to_value(RiftRequest::GetWindows { space_id: Some(7) }).unwrap(),
            legacy
        );

        assert_eq!(
            serde_json::to_value(RiftRequest::GetWorkspacesForDisplay {
                display_uuid: "display-a".into(),
            })
            .unwrap(),
            serde_json::json!({
                "get_workspaces_for_display": { "display_uuid": "display-a" }
            })
        );

        assert_eq!(
            serde_json::to_value(RiftRequest::GetLayoutStateForDisplay {
                display_uuid: "display-a".into(),
                workspace_id: Some(2),
            })
            .unwrap(),
            serde_json::json!({
                "get_layout_state_for_display": {
                    "display_uuid": "display-a",
                    "workspace_id": 2
                }
            })
        );

        assert_eq!(
            serde_json::to_value(RiftRequest::GetWorkspaceLayoutsForDisplay {
                display_uuid: "display-a".into(),
                workspace_id: None,
            })
            .unwrap(),
            serde_json::json!({
                "get_workspace_layouts_for_display": {
                    "display_uuid": "display-a",
                    "workspace_id": null
                }
            })
        );
    }

    #[test]
    fn typed_response_decodes_shared_query_types() {
        let response: RiftResponse<Vec<crate::WorkspaceData>> =
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

    #[test]
    fn legacy_stringified_reactor_commands_still_decode() {
        let request: RiftRequest = serde_json::from_value(serde_json::json!({
            "execute_command": {
                "command": "{\"Reactor\":{\"switch_to_workspace\":5}}",
                "args": []
            }
        }))
        .unwrap();

        assert_eq!(request, RiftRequest::ExecuteCommand {
            command: RiftCommand::Layout(LayoutCommand::SwitchToWorkspace(5)),
        });
    }

    #[test]
    fn legacy_window_id_strings_still_decode() {
        let request: RiftRequest = serde_json::from_value(serde_json::json!({
            "get_window_info": { "window_id": "WindowId { pid: 42, idx: 7 }" }
        }))
        .unwrap();

        assert_eq!(request, RiftRequest::GetWindowInfo {
            window_id: WindowId::new(42, 7).unwrap(),
        });
    }
}
