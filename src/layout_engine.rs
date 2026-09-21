pub mod engine;
mod floating;
pub(crate) mod graph;
pub mod systems;
pub mod utils;
mod workspaces;

pub(crate) use engine::WindowLayoutInfo;
pub use engine::{
    EventResponse, LayoutCommand, LayoutEngine, LayoutEvent, LayoutEventOutcome, ResolvedWindow,
    RestoreReport, RestoreRequest, RestoreScope, RestoreSource, RestoreWarning,
};
pub(crate) use floating::FloatingManager;
pub use graph::{Direction, LayoutKind, Orientation, ResizeOrientation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowDropAction {
    Swap,
    Stack,
    Insert(Direction),
}

impl From<crate::common::config::MouseDropAction> for WindowDropAction {
    fn from(action: crate::common::config::MouseDropAction) -> Self {
        match action {
            crate::common::config::MouseDropAction::Swap => Self::Swap,
            crate::common::config::MouseDropAction::Stack => Self::Stack,
        }
    }
}

pub(crate) struct WindowDropRequest {
    pub source: crate::actor::app::WindowId,
    pub target: crate::actor::app::WindowId,
    pub space: crate::sys::screen::SpaceId,
    pub action: WindowDropAction,
}
pub(crate) use systems::LayoutId;
pub use systems::{
    BspLayoutSystem, LayoutSystem, LayoutSystemKind, MasterStackLayoutSystem,
    ScrollingLayoutSystem, StackLayoutSystem, TraditionalLayoutSystem,
};
pub(crate) use workspaces::WorkspaceLayouts;

pub use crate::model::virtual_workspace::{VirtualWorkspaceId, WorkspaceStats, WorkspaceStore};

#[cfg(test)]
mod drop_preview_tests;
