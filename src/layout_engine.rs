pub mod binary_tree;
pub mod engine;
mod floating;
pub(crate) mod graph;
pub mod resize;
pub mod systems;
pub mod utils;
mod workspaces;

pub use engine::{EventResponse, LayoutCommand, LayoutEngine, LayoutEvent};
pub(crate) use floating::FloatingManager;
pub use graph::{Direction, LayoutKind, Orientation};
pub use resize::{ResizeCorner, ResizeDelta, ResizeMode, ResizeValue};
pub(crate) use systems::LayoutId;
pub use systems::{
    BspLayoutSystem, DwindleLayoutSystem, LayoutSystem, LayoutSystemKind, TraditionalLayoutSystem,
};
pub(crate) use workspaces::WorkspaceLayouts;

pub use crate::model::virtual_workspace::{
    VirtualWorkspaceId, VirtualWorkspaceManager, WorkspaceStats,
};

/// Captures the most recent layout frame used for a layout calculation so that
/// helpers (e.g., cursor-based insertion) can be pure and stateless.
#[derive(Clone)]
pub struct LayoutFrame {
    pub screen: objc2_core_foundation::CGRect,
    pub gaps: crate::common::config::GapSettings,
}
