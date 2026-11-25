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
    pub targets: Vec<DragSceneTarget>,
    /// What the dragged window is to this layout.
    pub place: ScenePlace,
}

/// Whether the dragged window has a slot in the layout the scene is of.
#[derive(Debug, Clone)]
pub enum ScenePlace {
    /// It does (a tile over its own layout, or any scene not built for a move): a tile's slot is
    /// its home, and each entry says whether that edge (left, right, up, down) faces a logical
    /// tiled neighbour.
    Home { neighbors: [bool; 4] },
    /// It does not (torn out under the press, or from another display's layout): each drop it
    /// can make here, with the frame it lands in. The pointer picks the drop whose landing holds
    /// it, so the overlay is under the pointer.
    NewTile { slots: Vec<DragSlot> },
}

impl Default for ScenePlace {
    fn default() -> Self { Self::Home { neighbors: [false; 4] } }
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
    /// Where the drag started. A tile's home (see `ScenePlace::Home`) is there.
    pub origin_frame: CGRect,
    pub last_frame: CGRect,
    pub origin_space: Option<SpaceId>,
    pub current_space: Option<SpaceId>,
    pub origin: DragOrigin,
}

/// Where a dragged window comes from, which decides what it can drop onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOrigin {
    /// A tile: it drops onto other tiles, or back onto its own slot while over its layout.
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
