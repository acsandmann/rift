/// The actor broadcast channel uses the protocol event directly. This keeps
/// the server and client on one event definition instead of maintaining a
/// runtime copy that must be translated before IPC.
pub use rift_protocol::RiftEvent as BroadcastEvent;
use slotmap::Key;

use crate::actor::app::WindowId;
use crate::model::virtual_workspace::VirtualWorkspaceId;

pub type BroadcastSender = crate::actor::Sender<BroadcastEvent>;
pub type BroadcastReceiver = crate::actor::Receiver<BroadcastEvent>;

pub fn protocol_workspace_id(id: VirtualWorkspaceId) -> rift_protocol::WorkspaceId {
    let value = id.data().as_ffi();
    rift_protocol::WorkspaceId {
        idx: value as u32,
        version: (value >> 32) as u32,
    }
}

pub fn protocol_window_id(id: WindowId) -> rift_protocol::WindowId { id.into() }
