use objc2_core_foundation::{CGPoint, CGRect};

use crate::actor::app::WindowId;
use crate::layout_engine::WindowDropAction;
use crate::sys::screen::SpaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    NativeMove,
    NativeResize,
    ModifierMove,
    ModifierResize,
}

#[derive(Debug, Clone, Copy)]
pub struct DragSceneTarget {
    pub window: WindowId,
    pub space: SpaceId,
    pub frame: CGRect,
    /// Whether this layout can represent directional insertion around this tile.
    pub directional: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DragScene {
    /// Effective screen area available to the active tiled layout.
    pub tiling_area: Option<CGRect>,
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
pub struct DropTarget {
    pub window: WindowId,
    pub space: SpaceId,
    pub frame: CGRect,
    pub tiling_area: CGRect,
    pub zone: DropZone,
    pub action: WindowDropAction,
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
    pub pointer: CGPoint,
    pub kind: DragKind,
}

#[derive(Debug, Clone, Copy)]
pub struct DragCancel {
    pub source: DragSource,
    /// Rift must restore only frames that it moved itself.
    pub restore_origin: bool,
}
