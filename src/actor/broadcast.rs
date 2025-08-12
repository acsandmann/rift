use serde::{Deserialize, Serialize};

use crate::layout_engine::VirtualWorkspaceId;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum BroadcastEvent {
    WorkspaceChanged {
        workspace_id: VirtualWorkspaceId,
        workspace_name: String,
    },
    WindowsChanged {
        workspace_id: VirtualWorkspaceId,
        workspace_name: String,
        windows: Vec<String>,
    },
}
