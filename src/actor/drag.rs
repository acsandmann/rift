//! Single-owner drag interaction primitives.
//!
//! The event tap only publishes the latest pointer value. Geometry and drag
//! semantics are consumed by the drag actor/reactor bridge, never in the tap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use objc2_core_foundation::{CGPoint, CGRect};

use crate::actor::app::WindowId;
use crate::common::config::{MouseDropAction, MouseSettings};
use crate::layout_engine::{Direction, WindowDropAction};
pub use crate::model::drag::{
    DragCancel, DragCommit, DragKind, DragScene, DragSceneTarget, DragSource, DropTarget, DropZone,
};
use crate::sys::geometry::SameAs;
use crate::sys::screen::SpaceId;

const HYSTERESIS_POINTS: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
enum HorizontalEdge {
    West,
    East,
}

#[derive(Debug, Clone, Copy)]
enum VerticalEdge {
    North,
    South,
}

#[derive(Debug, Clone, Copy)]
struct ResizeEdges {
    horizontal: HorizontalEdge,
    vertical: VerticalEdge,
}

#[derive(Debug, Clone, Copy)]
pub struct DragMotion {
    pub point: CGPoint,
    pub button: MouseButton,
}

/// Latest-value transport with at most one outstanding actor wake.
#[derive(Debug, Default)]
struct DragMotionState {
    latest: Mutex<Option<DragMotion>>,
    wake_queued: AtomicBool,
}

#[derive(Clone, Debug, Default)]
pub struct DragMotionPublisher(Arc<DragMotionState>);

impl DragMotionPublisher {
    /// Publish a sample. Returns true exactly when the caller must enqueue a wake.
    pub fn publish(&self, motion: DragMotion) -> bool {
        *self.0.latest.lock().expect("drag motion mutex poisoned") = Some(motion);
        !self.0.wake_queued.swap(true, Ordering::AcqRel)
    }

    /// Consume the newest sample and re-arm publication atomically with respect
    /// to the value mutex, so a concurrent publication cannot be stranded.
    pub fn take_latest(&self) -> Option<DragMotion> {
        let mut latest = self.0.latest.lock().expect("drag motion mutex poisoned");
        let motion = latest.take();
        self.0.wake_queued.store(false, Ordering::Release);
        motion
    }

    pub fn wake_queued(&self) -> bool { self.0.wake_queued.load(Ordering::Acquire) }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: u64,
    pub source: DragSource,
    pub pointer: CGPoint,
    pub anchor_point: CGPoint,
    pub scene: DragScene,
    pub target: Option<DropTarget>,
    pub kind: DragKind,
    last_effect_frame: CGRect,
    resize_edges: Option<ResizeEdges>,
}

#[derive(Debug, Clone, Copy)]
pub enum StartKind {
    Native {
        window: WindowId,
    },
    Modifier {
        button: MouseButton,
        point: CGPoint,
        action: crate::common::config::MouseAction,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct PendingStart {
    pub id: u64,
    pub kind: StartKind,
}

#[derive(Debug, Clone, Default)]
pub enum State {
    #[default]
    Idle,
    AwaitingSource(PendingStart),
    Dragging(Box<Session>),
}

#[derive(Debug, Clone)]
pub struct DragActor {
    state: State,
    next_session_id: u64,
    settings: MouseSettings,
}

impl DragActor {
    pub fn new(settings: MouseSettings) -> Self {
        Self {
            state: State::Idle,
            next_session_id: 1,
            settings,
        }
    }

    pub fn is_active(&self) -> bool { !matches!(self.state, State::Idle) }

    pub fn source(&self) -> Option<DragSource> {
        match &self.state {
            State::Idle | State::AwaitingSource(_) => None,
            State::Dragging(session) => Some(session.source),
        }
    }

    pub fn target(&self) -> Option<DropTarget> {
        match &self.state {
            State::Idle | State::AwaitingSource(_) => None,
            State::Dragging(session) => session.target,
        }
    }

    pub fn kind(&self) -> Option<DragKind> {
        match &self.state {
            State::Dragging(session) => Some(session.kind),
            State::Idle | State::AwaitingSource(_) => None,
        }
    }

    pub fn replace_scene(&mut self, scene: DragScene) {
        let State::Dragging(session) = &mut self.state else {
            return;
        };
        session.scene = scene;
        if session.source.tiled
            && matches!(session.kind, DragKind::NativeMove | DragKind::ModifierMove)
            && session.source.origin_space == session.source.current_space
        {
            let previous = session.target.and_then(|previous| {
                session
                    .scene
                    .targets
                    .iter()
                    .find(|target| target.window == previous.window)
                    .filter(|target| previous.zone == DropZone::Center || target.directional)
                    .and_then(|target| {
                        Some(DropTarget {
                            frame: target.frame,
                            tiling_area: session.scene.tiling_area.unwrap_or(target.frame),
                            preview_area: target.preview_area(previous.action)?,
                            ..previous
                        })
                    })
            });
            session.target = hit_test(
                &session.scene,
                session.pointer,
                self.settings.drop_zone_fraction,
                self.settings.drop_action,
                previous,
            );
        } else {
            session.target = None;
        }
    }

    pub fn update_config(&mut self, settings: MouseSettings) {
        self.settings = settings;
        if !settings.enabled {
            self.cancel();
        }
    }

    fn await_start(&mut self, kind: StartKind) -> u64 {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.wrapping_add(1);
        self.state = State::AwaitingSource(PendingStart { id, kind });
        id
    }

    pub fn await_native(&mut self, window: WindowId) -> u64 {
        self.await_start(StartKind::Native { window })
    }

    pub fn await_modifier(
        &mut self,
        button: MouseButton,
        point: CGPoint,
        action: crate::common::config::MouseAction,
    ) -> u64 {
        self.await_start(StartKind::Modifier { button, point, action })
    }

    /// Complete source resolution. Stale results are ignored by session id.
    pub fn resolve_start(
        &mut self,
        session_id: u64,
        source: Option<DragSource>,
        scene: DragScene,
    ) -> bool {
        let State::AwaitingSource(pending) = self.state else {
            return false;
        };
        if pending.id != session_id {
            return false;
        }
        let Some(source) = source else {
            self.state = State::Idle;
            return true;
        };
        match pending.kind {
            StartKind::Native { window } if window == source.window => {
                self.start_native_session(session_id, source, scene);
                true
            }
            StartKind::Modifier { point, action, .. } => {
                self.start_modifier_session(session_id, source, point, action, scene);
                true
            }
            StartKind::Native { .. } => {
                self.state = State::Idle;
                false
            }
        }
    }

    #[cfg(test)]
    pub fn begin_native(&mut self, source: DragSource, scene: DragScene) {
        if self.update_native(source.window, source.last_frame, source.current_space) {
            return;
        }
        let id = self.await_native(source.window);
        let _ = self.resolve_start(id, Some(source), scene);
    }

    fn start_native_session(&mut self, id: u64, source: DragSource, scene: DragScene) {
        let resized = !source.origin_frame.size.same_as(source.last_frame.size);
        let kind = if resized {
            DragKind::NativeResize
        } else {
            DragKind::NativeMove
        };
        let pointer = CGPoint::new(
            source.last_frame.origin.x + source.last_frame.size.width / 2.0,
            source.last_frame.origin.y + source.last_frame.size.height / 2.0,
        );
        self.state = State::Dragging(Box::new(Session {
            id,
            source,
            pointer,
            anchor_point: pointer,
            scene,
            target: None,
            kind,
            last_effect_frame: source.last_frame,
            resize_edges: None,
        }));
    }

    /// Updates an existing native drag without rebuilding its immutable scene.
    ///
    /// Returns `true` when `window` owns the active native drag session.
    pub fn update_native(
        &mut self,
        window: WindowId,
        frame: CGRect,
        current_space: Option<SpaceId>,
    ) -> bool {
        let State::Dragging(session) = &mut self.state else {
            return false;
        };
        if session.source.window != window
            || !matches!(session.kind, DragKind::NativeMove | DragKind::NativeResize)
        {
            return false;
        }
        session.source.last_frame = frame;
        session.source.current_space = current_space;
        if session.source.origin_space != current_space {
            session.target = None;
        }
        true
    }

    #[cfg(test)]
    pub fn begin_modifier(
        &mut self,
        source: DragSource,
        point: CGPoint,
        action: crate::common::config::MouseAction,
        scene: DragScene,
    ) {
        let id = self.await_modifier(MouseButton::Left, point, action);
        let _ = self.resolve_start(id, Some(source), scene);
    }

    fn start_modifier_session(
        &mut self,
        id: u64,
        source: DragSource,
        point: CGPoint,
        action: crate::common::config::MouseAction,
        scene: DragScene,
    ) {
        let resize_edges =
            (action == crate::common::config::MouseAction::Resize).then(|| ResizeEdges {
                horizontal: if point.x
                    < source.origin_frame.origin.x + source.origin_frame.size.width / 2.0
                {
                    HorizontalEdge::West
                } else {
                    HorizontalEdge::East
                },
                vertical: if point.y
                    < source.origin_frame.origin.y + source.origin_frame.size.height / 2.0
                {
                    VerticalEdge::North
                } else {
                    VerticalEdge::South
                },
            });
        let kind = match action {
            crate::common::config::MouseAction::Move => DragKind::ModifierMove,
            crate::common::config::MouseAction::Resize => DragKind::ModifierResize,
            crate::common::config::MouseAction::None => return,
        };
        self.state = State::Dragging(Box::new(Session {
            id,
            source,
            pointer: point,
            anchor_point: point,
            scene,
            target: None,
            kind,
            last_effect_frame: source.last_frame,
            resize_edges,
        }));
    }

    pub fn motion(&mut self, motion: DragMotion) -> bool {
        let State::Dragging(session) = &mut self.state else {
            return false;
        };
        session.pointer = motion.point;
        let action = match session.kind {
            DragKind::ModifierMove => Some(crate::common::config::MouseAction::Move),
            DragKind::ModifierResize => Some(crate::common::config::MouseAction::Resize),
            DragKind::NativeMove | DragKind::NativeResize => None,
        };
        if let Some(action) = action {
            let dx = motion.point.x - session.anchor_point.x;
            let dy = motion.point.y - session.anchor_point.y;
            session.source.last_frame = match action {
                crate::common::config::MouseAction::Move => CGRect::new(
                    CGPoint::new(
                        session.source.origin_frame.origin.x + dx,
                        session.source.origin_frame.origin.y + dy,
                    ),
                    session.source.origin_frame.size,
                ),
                crate::common::config::MouseAction::Resize => {
                    let edges = session.resize_edges.expect("resize session has fixed edges");
                    let mut origin = session.source.origin_frame.origin;
                    let mut size = session.source.origin_frame.size;
                    match edges.horizontal {
                        HorizontalEdge::West => {
                            origin.x += dx;
                            size.width -= dx;
                        }
                        HorizontalEdge::East => size.width += dx,
                    }
                    match edges.vertical {
                        VerticalEdge::North => {
                            origin.y += dy;
                            size.height -= dy;
                        }
                        VerticalEdge::South => size.height += dy,
                    }
                    if size.width < 1.0 {
                        if matches!(edges.horizontal, HorizontalEdge::West) {
                            origin.x -= 1.0 - size.width;
                        }
                        size.width = 1.0;
                    }
                    if size.height < 1.0 {
                        if matches!(edges.vertical, VerticalEdge::North) {
                            origin.y -= 1.0 - size.height;
                        }
                        size.height = 1.0;
                    }
                    CGRect::new(origin, size)
                }
                crate::common::config::MouseAction::None => session.source.origin_frame,
            };
        }
        let next = if matches!(session.kind, DragKind::NativeResize | DragKind::ModifierResize)
            || !session.source.tiled
            || session.source.origin_space != session.source.current_space
        {
            None
        } else {
            hit_test(
                &session.scene,
                motion.point,
                self.settings.drop_zone_fraction,
                self.settings.drop_action,
                session.target,
            )
        };
        let changed = next != session.target;
        session.target = next;
        changed
    }

    pub fn interactive_update(
        &mut self,
    ) -> Option<(
        WindowId,
        CGRect,
        CGRect,
        crate::common::config::MouseAction,
        bool,
    )> {
        let State::Dragging(session) = &mut self.state else {
            return None;
        };
        let action = match session.kind {
            DragKind::ModifierMove => crate::common::config::MouseAction::Move,
            DragKind::ModifierResize => crate::common::config::MouseAction::Resize,
            DragKind::NativeMove | DragKind::NativeResize => return None,
        };
        if action == crate::common::config::MouseAction::Resize && session.source.tiled {
            let previous = session.last_effect_frame;
            let next = session.source.last_frame;
            let materially_changed = (previous.origin.x - next.origin.x).abs() >= 1.0
                || (previous.origin.y - next.origin.y).abs() >= 1.0
                || (previous.size.width - next.size.width).abs() >= 1.0
                || (previous.size.height - next.size.height).abs() >= 1.0;
            if !materially_changed {
                return None;
            }
            session.last_effect_frame = next;
        }
        Some((
            session.source.window,
            session.source.origin_frame,
            session.source.last_frame,
            action,
            session.source.tiled,
        ))
    }

    pub fn constrain_resize(
        &mut self,
        min_size: Option<objc2_core_foundation::CGSize>,
        max_size: Option<objc2_core_foundation::CGSize>,
    ) {
        let State::Dragging(session) = &mut self.state else {
            return;
        };
        if session.kind != DragKind::ModifierResize {
            return;
        }
        let Some(edges) = session.resize_edges else { return };
        let old = session.source.last_frame;
        let mut size = old.size;
        if let Some(min) = min_size {
            size.width = size.width.max(min.width);
            size.height = size.height.max(min.height);
        }
        if let Some(max) = max_size {
            if max.width > 0.0 {
                size.width = size.width.min(max.width);
            }
            if max.height > 0.0 {
                size.height = size.height.min(max.height);
            }
        }
        let mut origin = old.origin;
        if matches!(edges.horizontal, HorizontalEdge::West) {
            origin.x = old.origin.x + old.size.width - size.width;
        }
        if matches!(edges.vertical, VerticalEdge::North) {
            origin.y = old.origin.y + old.size.height - size.height;
        }
        session.source.last_frame = CGRect::new(origin, size);
    }

    pub fn finish(&mut self) -> Option<DragCommit> {
        let State::Dragging(session) = std::mem::take(&mut self.state) else {
            return None;
        };
        let session = *session;
        Some(DragCommit {
            source: session.source,
            target: session.target,
            pointer: session.pointer,
            kind: session.kind,
        })
    }

    pub fn cancel(&mut self) -> Option<DragCancel> {
        let State::Dragging(session) = std::mem::take(&mut self.state) else {
            return None;
        };
        Some(DragCancel {
            source: session.source,
            restore_origin: matches!(
                session.kind,
                DragKind::ModifierMove | DragKind::ModifierResize
            ),
        })
    }

    pub fn window_removed(&mut self, window: WindowId) -> bool {
        if matches!(
            self.state,
            State::AwaitingSource(PendingStart {
                kind: StartKind::Native { window: pending },
                ..
            }) if pending == window
        ) {
            self.state = State::Idle;
            return true;
        }
        let State::Dragging(session) = &mut self.state else {
            return false;
        };
        if session.source.window == window {
            self.cancel();
            return true;
        }
        session.scene.targets.retain(|target| target.window != window);
        if session.target.is_some_and(|target| target.window == window) {
            session.target = None;
        }
        false
    }
}

impl PartialEq for DropTarget {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window && self.space == other.space && self.zone == other.zone
    }
}

fn contains(rect: CGRect, point: CGPoint, margin: f64) -> bool {
    point.x >= rect.origin.x - margin
        && point.x <= rect.origin.x + rect.size.width + margin
        && point.y >= rect.origin.y - margin
        && point.y <= rect.origin.y + rect.size.height + margin
}

pub fn classify_zone(frame: CGRect, point: CGPoint, fraction: f64) -> Option<DropZone> {
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 || !contains(frame, point, 0.0) {
        return None;
    }
    let left = (point.x - frame.origin.x) / frame.size.width;
    let right = 1.0 - left;
    let top = (point.y - frame.origin.y) / frame.size.height;
    let bottom = 1.0 - top;
    if left >= fraction && right >= fraction && top >= fraction && bottom >= fraction {
        return Some(DropZone::Center);
    }

    // Strict comparisons preserve the specified West, East, North, South tie order.
    let mut best = (left, DropZone::West);
    if right < best.0 {
        best = (right, DropZone::East);
    }
    if top < best.0 {
        best = (top, DropZone::North);
    }
    if bottom < best.0 {
        best = (bottom, DropZone::South);
    }
    Some(best.1)
}

fn zone_frame(frame: CGRect, zone: DropZone, fraction: f64) -> CGRect {
    let x = frame.origin.x;
    let y = frame.origin.y;
    let w = frame.size.width;
    let h = frame.size.height;
    match zone {
        DropZone::Center => CGRect::new(
            CGPoint::new(x + w * fraction, y + h * fraction),
            objc2_core_foundation::CGSize::new(
                w * (1.0 - 2.0 * fraction),
                h * (1.0 - 2.0 * fraction),
            ),
        ),
        DropZone::West => CGRect::new(
            CGPoint::new(x, y),
            objc2_core_foundation::CGSize::new(w * fraction, h),
        ),
        DropZone::East => CGRect::new(
            CGPoint::new(x + w * (1.0 - fraction), y),
            objc2_core_foundation::CGSize::new(w * fraction, h),
        ),
        DropZone::North => CGRect::new(
            CGPoint::new(x, y),
            objc2_core_foundation::CGSize::new(w, h * fraction),
        ),
        DropZone::South => CGRect::new(
            CGPoint::new(x, y + h * (1.0 - fraction)),
            objc2_core_foundation::CGSize::new(w, h * fraction),
        ),
    }
}

pub fn resolve_action(zone: DropZone, center: MouseDropAction) -> WindowDropAction {
    match zone {
        DropZone::Center => match center {
            MouseDropAction::Swap => WindowDropAction::Swap,
            MouseDropAction::Stack => WindowDropAction::Stack,
        },
        DropZone::West => WindowDropAction::Insert(Direction::Left),
        DropZone::East => WindowDropAction::Insert(Direction::Right),
        DropZone::North => WindowDropAction::Insert(Direction::Up),
        DropZone::South => WindowDropAction::Insert(Direction::Down),
    }
}

/// Hit-test a cached immutable scene without allocating or sorting.
pub fn hit_test(
    scene: &DragScene,
    point: CGPoint,
    fraction: f64,
    center: MouseDropAction,
    previous: Option<DropTarget>,
) -> Option<DropTarget> {
    if let Some(previous) = previous {
        let base = if previous.zone == DropZone::Center {
            previous.frame
        } else {
            previous.tiling_area
        };
        let retained = zone_frame(base, previous.zone, fraction);
        if contains(retained, point, HYSTERESIS_POINTS) {
            return Some(previous);
        }
    }
    let tiling_area = scene.tiling_area.or_else(|| {
        scene
            .targets
            .iter()
            .find_map(|target| contains(target.frame, point, 0.0).then_some(target.frame))
    })?;
    let zone = classify_zone(tiling_area, point, fraction)?;
    let target = scene.targets.iter().min_by(|a, b| {
        distance_to_rect_squared(a.frame, point)
            .total_cmp(&distance_to_rect_squared(b.frame, point))
    })?;
    if zone != DropZone::Center && !target.directional {
        return None;
    }
    let action = resolve_action(zone, center);
    Some(DropTarget {
        window: target.window,
        space: target.space,
        frame: target.frame,
        tiling_area,
        preview_area: target.preview_area(action)?,
        zone,
        action,
    })
}

fn distance_to_rect_squared(frame: CGRect, point: CGPoint) -> f64 {
    let dx = (frame.origin.x - point.x)
        .max(0.0)
        .max(point.x - (frame.origin.x + frame.size.width));
    let dy = (frame.origin.y - point.y)
        .max(0.0)
        .max(point.y - (frame.origin.y + frame.size.height));
    dx * dx + dy * dy
}

impl DragSceneTarget {
    fn preview_area(self, action: WindowDropAction) -> Option<CGRect> {
        match action {
            WindowDropAction::Swap | WindowDropAction::Stack => self.center_frame,
            WindowDropAction::Insert(Direction::Left) => self.west_frame,
            WindowDropAction::Insert(Direction::Right) => self.east_frame,
            WindowDropAction::Insert(Direction::Up) => self.north_frame,
            WindowDropAction::Insert(Direction::Down) => self.south_frame,
        }
    }
}

pub fn preview_frame(target: DropTarget) -> CGRect {
    let mut frame = target.preview_area;
    frame.origin.x += 4.0;
    frame.origin.y += 4.0;
    frame.size.width = (frame.size.width - 8.0).max(0.0);
    frame.size.height = (frame.size.height - 8.0).max(0.0);
    frame
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CGSize;

    use super::*;

    fn rect() -> CGRect { CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(200.0, 100.0)) }

    #[test]
    fn classifies_center_edges_and_corner_ties() {
        assert_eq!(
            classify_zone(rect(), CGPoint::new(100.0, 50.0), 0.25),
            Some(DropZone::Center)
        );
        assert_eq!(
            classify_zone(rect(), CGPoint::new(1.0, 50.0), 0.25),
            Some(DropZone::West)
        );
        assert_eq!(
            classify_zone(rect(), CGPoint::new(199.0, 50.0), 0.25),
            Some(DropZone::East)
        );
        assert_eq!(
            classify_zone(rect(), CGPoint::new(100.0, 1.0), 0.25),
            Some(DropZone::North)
        );
        assert_eq!(
            classify_zone(rect(), CGPoint::new(100.0, 99.0), 0.25),
            Some(DropZone::South)
        );
        assert_eq!(
            classify_zone(rect(), CGPoint::new(0.0, 0.0), 0.25),
            Some(DropZone::West)
        );
        assert_eq!(classify_zone(rect(), CGPoint::new(-1.0, 50.0), 0.25), None);
    }

    #[test]
    fn publisher_coalesces_to_newest_motion() {
        let publisher = DragMotionPublisher::default();
        for x in 0..1_000 {
            assert_eq!(
                publisher.publish(DragMotion {
                    point: CGPoint::new(x as f64, 0.0),
                    button: MouseButton::Left
                }),
                x == 0
            );
        }
        assert!(publisher.wake_queued());
        assert_eq!(publisher.take_latest().unwrap().point.x, 999.0);
        assert!(!publisher.wake_queued());
        assert!(publisher.publish(DragMotion {
            point: CGPoint::new(1_000.0, 0.0),
            button: MouseButton::Left
        }));
    }

    #[test]
    fn native_tiled_move_targets_pointer_zone_and_commits_once() {
        let source = WindowId::new(1, 1);
        let target = WindowId::new(1, 2);
        let space = SpaceId::new(1);
        let target_frame = CGRect::new(CGPoint::new(100.0, 0.0), CGSize::new(100.0, 100.0));
        let mut actor = DragActor::new(MouseSettings::default());
        actor.begin_native(
            DragSource {
                window: source,
                origin_frame: rect(),
                last_frame: CGRect::new(CGPoint::new(10.0, 0.0), rect().size),
                origin_space: Some(space),
                current_space: Some(space),
                tiled: true,
            },
            DragScene {
                tiling_area: Some(rect()),
                targets: vec![DragSceneTarget {
                    window: target,
                    space,
                    frame: target_frame,
                    center_frame: Some(target_frame),
                    west_frame: Some(target_frame),
                    east_frame: Some(target_frame),
                    north_frame: Some(target_frame),
                    south_frame: Some(target_frame),
                    directional: true,
                }],
            },
        );
        assert!(actor.motion(DragMotion {
            point: CGPoint::new(1.0, 50.0),
            button: MouseButton::Left,
        }));
        assert_eq!(
            actor.target().unwrap().action,
            WindowDropAction::Insert(Direction::Left)
        );
        let preview = preview_frame(actor.target().unwrap());
        assert_eq!(preview.origin, CGPoint::new(104.0, 4.0));
        assert_eq!(preview.size, CGSize::new(92.0, 92.0));
        let commit = actor.finish().unwrap();
        assert_eq!(commit.source.window, source);
        assert_eq!(commit.target.unwrap().window, target);
        assert!(actor.finish().is_none());
    }

    #[test]
    fn layouts_without_directional_inserts_expose_only_the_center_zone() {
        let scene = DragScene {
            tiling_area: Some(rect()),
            targets: vec![DragSceneTarget {
                window: WindowId::new(1, 2),
                space: SpaceId::new(1),
                frame: rect(),
                center_frame: Some(rect()),
                west_frame: Some(rect()),
                east_frame: Some(rect()),
                north_frame: Some(rect()),
                south_frame: Some(rect()),
                directional: false,
            }],
        };
        assert!(
            hit_test(
                &scene,
                CGPoint::new(1.0, 50.0),
                0.25,
                MouseDropAction::Swap,
                None,
            )
            .is_none()
        );
        assert_eq!(
            hit_test(
                &scene,
                CGPoint::new(100.0, 50.0),
                0.25,
                MouseDropAction::Swap,
                None,
            )
            .unwrap()
            .zone,
            DropZone::Center,
        );
    }

    #[test]
    fn floating_move_never_builds_a_drop_target() {
        let source = WindowId::new(1, 1);
        let target = WindowId::new(1, 2);
        let space = SpaceId::new(1);
        let mut actor = DragActor::new(MouseSettings::default());
        actor.begin_native(
            DragSource {
                window: source,
                origin_frame: rect(),
                last_frame: CGRect::new(CGPoint::new(10.0, 0.0), rect().size),
                origin_space: Some(space),
                current_space: Some(space),
                tiled: false,
            },
            DragScene {
                tiling_area: Some(rect()),
                targets: vec![DragSceneTarget {
                    window: target,
                    space,
                    frame: rect(),
                    center_frame: Some(rect()),
                    west_frame: Some(rect()),
                    east_frame: Some(rect()),
                    north_frame: Some(rect()),
                    south_frame: Some(rect()),
                    directional: true,
                }],
            },
        );
        actor.motion(DragMotion {
            point: CGPoint::new(100.0, 50.0),
            button: MouseButton::Left,
        });
        assert!(actor.target().is_none());
    }

    #[test]
    fn modifier_resize_keeps_the_opposite_corner_fixed() {
        let source = WindowId::new(1, 1);
        let space = SpaceId::new(1);
        let origin = rect();
        let mut actor = DragActor::new(MouseSettings::default());
        actor.begin_modifier(
            DragSource {
                window: source,
                origin_frame: origin,
                last_frame: origin,
                origin_space: Some(space),
                current_space: Some(space),
                tiled: false,
            },
            CGPoint::new(10.0, 10.0),
            crate::common::config::MouseAction::Resize,
            DragScene::default(),
        );
        actor.motion(DragMotion {
            point: CGPoint::new(30.0, 20.0),
            button: MouseButton::Right,
        });
        let (_, _, resized, _, _) = actor.interactive_update().unwrap();
        assert_eq!(resized.origin, CGPoint::new(20.0, 10.0));
        assert_eq!(resized.size, CGSize::new(180.0, 90.0));
        assert_eq!(resized.origin.x + resized.size.width, 200.0);
        assert_eq!(resized.origin.y + resized.size.height, 100.0);
    }

    #[test]
    fn tiled_resize_ignores_sub_point_motion() {
        let source = WindowId::new(1, 1);
        let space = SpaceId::new(1);
        let mut actor = DragActor::new(MouseSettings::default());
        actor.begin_modifier(
            DragSource {
                window: source,
                origin_frame: rect(),
                last_frame: rect(),
                origin_space: Some(space),
                current_space: Some(space),
                tiled: true,
            },
            CGPoint::new(190.0, 90.0),
            crate::common::config::MouseAction::Resize,
            DragScene::default(),
        );
        actor.motion(DragMotion {
            point: CGPoint::new(190.5, 90.5),
            button: MouseButton::Right,
        });
        assert!(actor.interactive_update().is_none());
        actor.motion(DragMotion {
            point: CGPoint::new(191.0, 90.5),
            button: MouseButton::Right,
        });
        assert!(actor.interactive_update().is_some());
        assert!(actor.interactive_update().is_none());
    }

    #[test]
    fn cancellation_restores_only_rift_initiated_floating_frames() {
        let source = DragSource {
            window: WindowId::new(1, 1),
            origin_frame: rect(),
            last_frame: CGRect::new(CGPoint::new(20.0, 20.0), rect().size),
            origin_space: Some(SpaceId::new(1)),
            current_space: Some(SpaceId::new(1)),
            tiled: false,
        };
        let mut actor = DragActor::new(MouseSettings::default());
        actor.begin_native(source, DragScene::default());
        assert!(!actor.cancel().unwrap().restore_origin);

        actor.begin_modifier(
            source,
            CGPoint::new(10.0, 10.0),
            crate::common::config::MouseAction::Move,
            DragScene::default(),
        );
        assert!(actor.cancel().unwrap().restore_origin);
    }

    #[test]
    fn stale_source_resolution_is_ignored() {
        let mut actor = DragActor::new(MouseSettings::default());
        let stale = actor.await_native(WindowId::new(1, 1));
        let current = actor.await_native(WindowId::new(1, 2));
        assert!(!actor.resolve_start(stale, None, DragScene::default()));
        assert!(actor.is_active());
        assert!(actor.resolve_start(current, None, DragScene::default()));
        assert!(!actor.is_active());
    }
}
