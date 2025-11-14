use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
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
        display_uuid: Option<String>,
    },
    WindowsChanged {
        workspace_id: VirtualWorkspaceId,
        workspace_name: String,
        windows: Vec<String>,
        space_id: SpaceId,
        display_uuid: Option<String>,
    },
    WindowTitleChanged {
        window_id: WindowId,
        workspace_id: VirtualWorkspaceId,
        workspace_index: Option<u64>,
        workspace_name: String,
        previous_title: String,
        new_title: String,
        space_id: SpaceId,
        display_uuid: Option<String>,
    },
}

pub type BroadcastSender = crate::actor::Sender<BroadcastEvent>;
pub type BroadcastReceiver = crate::actor::Receiver<BroadcastEvent>;
