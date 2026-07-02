use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use crate::layout_engine::{LayoutKind, VirtualWorkspaceId};
use crate::sys::screen::SpaceId;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct StackInfo {
    pub container_kind: LayoutKind,
    pub total_count: usize,
    pub selected_index: usize,
    pub windows: Vec<String>,
}

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
    StacksChanged {
        workspace_id: VirtualWorkspaceId,
        workspace_index: Option<u64>,
        workspace_name: String,
        stacks: Vec<StackInfo>,
        active_workspace_has_fullscreen: bool,
        space_id: SpaceId,
        display_uuid: Option<String>,
    },
}

pub type BroadcastSender = crate::actor::Sender<BroadcastEvent>;
pub type BroadcastReceiver = crate::actor::Receiver<BroadcastEvent>;
