use objc2_core_foundation::CGRect;

use crate::actor::app::WindowId;
use crate::layout_engine::WindowDropAction;
use crate::sys::geometry::SameAs;
use crate::sys::screen::SpaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    NativeMove,
    NativeResize,
    ModifierMove,
}

impl DragKind {
    pub fn is_move(self) -> bool { matches!(self, Self::NativeMove | Self::ModifierMove) }

    /// A native drag that keeps the window's size moves it; one that changes it resizes it.
    pub fn native(origin_frame: CGRect, frame: CGRect) -> Self {
        if origin_frame.size.same_as(frame.size) {
            Self::NativeMove
        } else {
            Self::NativeResize
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DragSceneTarget {
    pub window: WindowId,
    pub space: SpaceId,
    pub frame: CGRect,
}

#[derive(Debug, Clone, Default)]
pub struct DragScene {
    /// Layout-wide semantic override. Stack layouts use Swap for every zone.
    pub action_override: Option<WindowDropAction>,
    /// Whether each source-tile edge points toward a logical tiled neighbor.
    pub source_neighbors: [bool; 4],
    pub targets: Vec<DragSceneTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Center,
    West,
    East,
    North,
    South,
}

#[derive(Debug, Clone, Copy)]
pub struct DropIntent {
    pub window: WindowId,
    pub space: SpaceId,
    pub frame: CGRect,
    pub zone: DropZone,
    pub action: WindowDropAction,
}

#[derive(Debug, Clone, Copy)]
pub struct DropTarget {
    pub intent: DropIntent,
    pub preview_area: CGRect,
}

#[derive(Debug, Clone, Copy)]
pub struct DragSource {
    pub window: WindowId,
    pub origin_frame: CGRect,
    pub last_frame: CGRect,
    pub origin_space: Option<SpaceId>,
    pub current_space: Option<SpaceId>,
    pub tiled: bool,
}

#[derive(Debug, Clone)]
pub struct DragCommit {
    pub source: DragSource,
    pub target: Option<DropTarget>,
}

#[derive(Debug, Clone, Copy)]
pub struct DragCancel {
    pub source: DragSource,
    pub kind: DragKind,
}
