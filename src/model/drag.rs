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
    /// Set only for a window that brings a new tile (torn out under the press, or dragged in from
    /// another display): each drop it can make here, with the frame it lands in. The pointer then
    /// picks the drop whose landing holds it, so the overlay is under the pointer.
    pub slots: Vec<DragSlot>,
}

/// A drop a window bringing a new tile can make, and the frame it lands in.
#[derive(Debug, Clone, Copy)]
pub struct DragSlot {
    pub intent: DropIntent,
    pub area: CGRect,
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
    pub origin: DragOrigin,
}

/// Where a dragged window comes from, which decides what it can drop onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOrigin {
    /// A tile: it drops onto other tiles, or back onto its own slot.
    Tiled,
    /// A floating window: it moves freely and drops onto no tile.
    Floating,
    /// A new window the held button keeps out of the layout (a tab torn out of its window):
    /// it drops onto tiles like a tile, but has no slot of its own yet.
    Arriving,
}

impl DragOrigin {
    /// Whether the drag hit-tests the tiles it passes over.
    pub fn targets_tiles(self) -> bool {
        match self {
            Self::Tiled | Self::Arriving => true,
            Self::Floating => false,
        }
    }

    /// Whether the window has a slot to drop back onto.
    pub fn has_slot(self) -> bool {
        match self {
            Self::Tiled => true,
            Self::Floating | Self::Arriving => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DragCommit {
    pub source: DragSource,
    pub kind: DragKind,
    pub target: Option<DropTarget>,
}

#[derive(Debug, Clone, Copy)]
pub struct DragCancel {
    pub source: DragSource,
    pub kind: DragKind,
}
