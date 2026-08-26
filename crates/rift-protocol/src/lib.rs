//! Shared, platform-neutral protocol types for Rift.
//!
//! The server owns the runtime model and the client owns the Mach transport,
//! but both use these types at the wire boundary. JSON encoding remains an
//! implementation detail of the transport crates.

mod commands;
mod events;
mod layout;
mod queries;
mod selectors;
mod transport;

pub use commands::{
    AnimationEasing, ConfigCommand, FloatingWindowSize, FloatingWindowSizePreset, LayoutCommand,
    MetricsCommand, ReactorCommand, RiftCommand, ToggleWindowFloatingOptions,
};
pub use events::{EventKind, RiftEvent, StackInfo, WorkspaceId};
pub use layout::{Direction, LayoutKind, LayoutMode, Orientation, ResizeOrientation};
pub use queries::{
    ApplicationData, ContainerNodeType, ContainerTreeNode, DisplayData, LayoutStateData, Point,
    Rect, Size, WindowData, WindowId, WindowLayoutPosition, WorkspaceData, WorkspaceLayoutData,
};
pub use selectors::{DisplaySelector, RestoreScope, RestoreSource, WorkspaceSelector};
pub use transport::{JsonRiftResponse, RiftRequest, RiftResponse};
