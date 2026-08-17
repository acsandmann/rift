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
pub(crate) use systems::LayoutId;
pub use systems::{
    BspLayoutSystem, LayoutSystem, LayoutSystemKind, MasterStackLayoutSystem,
    ScrollingLayoutSystem, StackLayoutSystem, TraditionalLayoutSystem,
};
pub(crate) use workspaces::WorkspaceLayouts;

pub use crate::model::virtual_workspace::{VirtualWorkspaceId, WorkspaceStats, WorkspaceStore};
