use serde::{Deserialize, Serialize};

use crate::layout_engine::VirtualWorkspaceId;
use crate::sys::screen::SpaceId;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum BroadcastEvent {
    WorkspaceChanged {
        space_id: SpaceId,
        workspace_id: VirtualWorkspaceId,
        workspace_name: String,
    },
    WindowsChanged {
        workspace_id: VirtualWorkspaceId,
        workspace_name: String,
        windows: Vec<String>,
    },
}

pub type BroadcastSender = crate::actor::Sender<BroadcastEvent>;
pub type BroadcastReceiver = crate::actor::Receiver<BroadcastEvent>;
