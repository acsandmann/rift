pub mod engine;
mod floating;
mod scratchpad;
pub(crate) mod graph;
pub mod systems;
pub mod utils;
mod workspaces;

pub use engine::{EventResponse, LayoutCommand, LayoutEngine, LayoutEvent};
pub(crate) use floating::FloatingManager;
pub(crate) use scratchpad::ScratchpadManager;
pub use graph::{Direction, LayoutKind, Orientation};
pub(crate) use systems::LayoutId;
pub use systems::{
    BspLayoutSystem, LayoutSystem, LayoutSystemKind, MasterStackLayoutSystem,
    ScrollingLayoutSystem, TraditionalLayoutSystem,
};
pub(crate) use workspaces::WorkspaceLayouts;

pub use crate::model::virtual_workspace::{
    VirtualWorkspaceId, VirtualWorkspaceManager, WorkspaceStats,
};
