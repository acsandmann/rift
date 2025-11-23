pub mod engine;
mod floating;
pub(crate) mod graph;
pub mod systems;
pub mod utils;
mod workspaces;

pub use engine::{EventResponse, LayoutCommand, LayoutEngine, LayoutEvent};
pub(crate) use floating::FloatingManager;
pub use graph::{Direction, LayoutKind, Orientation};
pub(crate) use systems::LayoutId;
pub use systems::{
    BspLayoutSystem, DwindleLayoutSystem, LayoutSystem, LayoutSystemKind, TraditionalLayoutSystem,
};
pub(crate) use workspaces::WorkspaceLayouts;

pub use crate::model::virtual_workspace::{
    VirtualWorkspaceId, VirtualWorkspaceManager, WorkspaceStats,
};
