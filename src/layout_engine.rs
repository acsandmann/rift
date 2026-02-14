pub mod engine;
mod floating;
pub(crate) mod graph;
pub mod systems;
pub mod utils;
mod workspaces;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowConstraint {
    pub max_w: Option<f64>,
    pub max_h: Option<f64>,
}

impl WindowConstraint {
    pub fn cap_for_axis(self, horizontal: bool) -> Option<f64> {
        if horizontal { self.max_w } else { self.max_h }
    }
}

pub use engine::{EventResponse, LayoutCommand, LayoutEngine, LayoutEvent};
pub(crate) use floating::FloatingManager;
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
