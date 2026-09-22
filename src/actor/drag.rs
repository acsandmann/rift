//! Single-owner drag interaction primitives.
//!
//! The event tap only publishes the latest pointer value. Geometry and drag
//! semantics are consumed by the drag actor/reactor bridge, never in the tap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use objc2_core_foundation::{CGPoint, CGRect};

use crate::actor::app::WindowId;
use crate::common::config::{MouseAction, MouseDropAction, MouseSettings};
use crate::layout_engine::{Direction, WindowDropAction};
pub use crate::model::drag::{
    DragCancel, DragCommit, DragKind, DragScene, DragSceneTarget, DragSource, DropIntent,
    DropTarget, DropZone,
};
use crate::sys::geometry::{CGRectExt, SameAs};
use crate::sys::screen::SpaceId;

const HYSTERESIS_POINTS: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
pub struct DragMotion {
    pub point: CGPoint,
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
    pub source: DragSource,
    button: MouseButton,
    pub pointer: CGPoint,
    pub anchor_point: CGPoint,
    pub scene: DragScene,
    target: TargetState,
    unavailable: Vec<(WindowId, DropZone, WindowDropAction)>,
    pub kind: DragKind,
}

#[derive(Debug, Clone, Copy, Default)]
enum TargetState {
    #[default]
    None,
    Candidate(DropIntent),
    Validated(DropTarget),
}

impl TargetState {
    fn intent(self) -> Option<DropIntent> {
        match self {
            Self::Candidate(intent) => Some(intent),
            Self::Validated(target) => Some(target.intent),
            Self::None => None,
        }
    }

    fn validated(self) -> Option<DropTarget> {
        match self {
            Self::Validated(target) => Some(target),
            Self::None | Self::Candidate(_) => None,
        }
    }
}

impl Session {
    fn new(
        source: DragSource,
        button: MouseButton,
        pointer: CGPoint,
        scene: DragScene,
        kind: DragKind,
    ) -> Self {
        Self {
            source,
            button,
            pointer,
            anchor_point: pointer,
            scene,
            target: TargetState::None,
            unavailable: Vec::new(),
            kind,
        }
    }

    fn invalidate_drop(&mut self) {
        self.target = TargetState::None;
        self.unavailable.clear();
    }
}

#[derive(Debug, Clone, Copy)]
pub enum StartKind {
    Native {
        window: WindowId,
    },
    Modifier {
        button: MouseButton,
        point: CGPoint,
        action: MouseAction,
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
    Dragging(Session),
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

    fn session(&self) -> Option<&Session> {
        match &self.state {
            State::Dragging(session) => Some(session),
            _ => None,
        }
    }

    pub fn source(&self) -> Option<DragSource> { self.session().map(|session| session.source) }

    pub fn target(&self) -> Option<DropTarget> { self.session()?.target.validated() }

    pub fn intent(&self) -> Option<DropIntent> { self.session()?.target.intent() }

    pub fn kind(&self) -> Option<DragKind> { self.session().map(|session| session.kind) }

    /// Update reactor-resolved space membership and invalidate destination state.
    pub fn update_current_space(&mut self, current_space: Option<SpaceId>) -> bool {
        let State::Dragging(session) = &mut self.state else {
            return false;
        };
        if session.source.current_space == current_space {
            return false;
        }
        session.source.current_space = current_space;
        session.invalidate_drop();
        true
    }

    pub fn replace_scene(&mut self, scene: DragScene) {
        let State::Dragging(session) = &mut self.state else {
            return;
        };
        let previous = session.target.intent();
        session.scene = scene;
        session.invalidate_drop();
        if session.source.tiled
            && matches!(session.kind, DragKind::NativeMove | DragKind::ModifierMove)
            && session.source.origin_space == session.source.current_space
        {
            session.target = hit_test_available(
                &session.scene,
                session.pointer,
                self.settings.drop_zone_fraction,
                self.settings.drop_action,
                previous,
                &session.unavailable,
                Some(session.source),
            )
            .map_or(TargetState::None, TargetState::Candidate);
        }
    }

    pub fn set_preview(&mut self, intent: DropIntent, preview_area: Option<CGRect>) -> bool {
        let State::Dragging(session) = &mut self.state else {
            return false;
        };
        if session.target.intent() != Some(intent) {
            return false;
        }
        if let Some(preview_area) = preview_area {
            session.target = TargetState::Validated(DropTarget { intent, preview_area });
            return false;
        }
        session.unavailable.push((intent.window, intent.zone, intent.action));
        session.target = hit_test_available(
            &session.scene,
            session.pointer,
            self.settings.drop_zone_fraction,
            self.settings.drop_action,
            None,
            &session.unavailable,
            Some(session.source),
        )
        .map_or(TargetState::None, TargetState::Candidate);
        !matches!(session.target, TargetState::None)
    }

    pub fn update_config(&mut self, settings: MouseSettings) {
        let semantics_changed = self.settings.drop_action != settings.drop_action
            || self.settings.drop_zone_fraction != settings.drop_zone_fraction;
        self.settings = settings;
        if !settings.enabled {
            self.cancel();
        } else if semantics_changed && let State::Dragging(session) = &mut self.state {
            session.invalidate_drop();
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
        action: MouseAction,
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
                self.start_native_session(source, scene);
                true
            }
            StartKind::Modifier { button, point, action } => {
                self.start_modifier_session(source, button, point, action, scene);
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

    fn start_native_session(&mut self, source: DragSource, scene: DragScene) {
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
        self.state = State::Dragging(Session::new(source, MouseButton::Left, pointer, scene, kind));
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
        if session.source.current_space != current_space {
            session.source.current_space = current_space;
            session.invalidate_drop();
        }
        true
    }

    #[cfg(test)]
    pub fn begin_modifier(
        &mut self,
        source: DragSource,
        point: CGPoint,
        action: MouseAction,
        scene: DragScene,
    ) {
        let id = self.await_modifier(MouseButton::Left, point, action);
        let _ = self.resolve_start(id, Some(source), scene);
    }

    fn start_modifier_session(
        &mut self,
        source: DragSource,
        button: MouseButton,
        point: CGPoint,
        action: MouseAction,
        scene: DragScene,
    ) {
        let kind = match action {
            MouseAction::Move => DragKind::ModifierMove,
            MouseAction::None => return,
        };
        self.state = State::Dragging(Session::new(source, button, point, scene, kind));
    }

    pub fn motion(&mut self, motion: DragMotion) -> bool {
        let State::Dragging(session) = &mut self.state else {
            return false;
        };
        session.pointer = motion.point;
        if session.kind == DragKind::ModifierMove {
            let dx = motion.point.x - session.anchor_point.x;
            let dy = motion.point.y - session.anchor_point.y;
            session.source.last_frame = match session.kind {
                DragKind::ModifierMove => CGRect::new(
                    CGPoint::new(
                        session.source.origin_frame.origin.x + dx,
                        session.source.origin_frame.origin.y + dy,
                    ),
                    session.source.origin_frame.size,
                ),
                DragKind::NativeMove | DragKind::NativeResize => unreachable!(),
            };
        }
        let next = if session.kind == DragKind::NativeResize
            || !session.source.tiled
            || session.source.origin_space != session.source.current_space
        {
            None
        } else {
            hit_test_available(
                &session.scene,
                motion.point,
                self.settings.drop_zone_fraction,
                self.settings.drop_action,
                session.target.intent(),
                &session.unavailable,
                Some(session.source),
            )
        };
        let changed = next != session.target.intent();
        if changed {
            session.target = next.map_or(TargetState::None, TargetState::Candidate);
        }
        changed
    }

    pub fn interactive_update(&mut self) -> Option<(WindowId, CGRect)> {
        let State::Dragging(session) = &mut self.state else {
            return None;
        };
        (session.kind == DragKind::ModifierMove)
            .then_some((session.source.window, session.source.last_frame))
    }

    pub fn finish(&mut self, button: MouseButton) -> Option<DragCommit> {
        if !matches!(&self.state, State::Dragging(session) if session.button == button) {
            return None;
        }
        let State::Dragging(session) = std::mem::take(&mut self.state) else {
            return None;
        };
        Some(DragCommit {
            source: session.source,
            target: session.target.validated(),
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
            kind: session.kind,
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
        if session.target.intent().is_some_and(|intent| intent.window == window) {
            session.target = TargetState::None;
        }
        false
    }
}

impl PartialEq for DropTarget {
    fn eq(&self, other: &Self) -> bool {
        self.intent.window == other.intent.window
            && self.intent.space == other.intent.space
            && self.intent.zone == other.intent.zone
    }
}

impl PartialEq for DropIntent {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window && self.space == other.space && self.action == other.action
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
    let mut nearest = (left, DropZone::West);
    for candidate in [
        (right, DropZone::East),
        (top, DropZone::North),
        (bottom, DropZone::South),
    ] {
        if candidate.0 < nearest.0 {
            nearest = candidate;
        }
    }
    Some(nearest.1)
}

fn zone_frame(frame: CGRect, zone: DropZone, fraction: f64) -> CGRect {
    let (x, y, width, height) = match zone {
        DropZone::Center => (fraction, fraction, 1.0 - 2.0 * fraction, 1.0 - 2.0 * fraction),
        DropZone::West => (0.0, 0.0, fraction, 1.0),
        DropZone::East => (1.0 - fraction, 0.0, fraction, 1.0),
        DropZone::North => (0.0, 0.0, 1.0, fraction),
        DropZone::South => (0.0, 1.0 - fraction, 1.0, fraction),
    };
    CGRect::new(
        CGPoint::new(
            frame.origin.x + frame.size.width * x,
            frame.origin.y + frame.size.height * y,
        ),
        objc2_core_foundation::CGSize::new(frame.size.width * width, frame.size.height * height),
    )
}

pub fn resolve_action(zone: DropZone, center: MouseDropAction) -> WindowDropAction {
    match zone {
        DropZone::Center => center.into(),
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
    previous: Option<DropIntent>,
) -> Option<DropIntent> {
    hit_test_available(scene, point, fraction, center, previous, &[], None)
}

fn hit_test_available(
    scene: &DragScene,
    point: CGPoint,
    fraction: f64,
    center: MouseDropAction,
    previous: Option<DropIntent>,
    unavailable: &[(WindowId, DropZone, WindowDropAction)],
    source: Option<DragSource>,
) -> Option<DropIntent> {
    if let Some(previous) = previous
        && let Some(target) = scene.targets.iter().find(|target| target.window == previous.window)
    {
        let retained = zone_frame(target.frame, previous.zone, fraction);
        if contains(retained, point, HYSTERESIS_POINTS) {
            let intent = DropIntent {
                frame: target.frame,
                action: scene
                    .action_override
                    .unwrap_or_else(|| resolve_action(previous.zone, center)),
                ..previous
            };
            if !unavailable.contains(&(intent.window, intent.zone, intent.action)) {
                return Some(intent);
            }
        }
    }
    let intent_for = |target: &DragSceneTarget, point: CGPoint| {
        let zone = classify_zone(target.frame, point, fraction)?;
        Some(DropIntent {
            window: target.window,
            space: target.space,
            frame: target.frame,
            zone,
            action: scene.action_override.unwrap_or_else(|| resolve_action(zone, center)),
        })
    };

    for target in &scene.targets {
        if contains(target.frame, point, 0.0)
            && let Some(intent) = intent_for(target, point)
            && !unavailable.contains(&(intent.window, intent.zone, intent.action))
        {
            return Some(intent);
        }
    }

    if scene.action_override.is_none()
        && let Some(source) = source
        && let Some(space) = source.current_space
        && let Some(zone) = classify_zone(source.origin_frame, point, fraction)
        && let WindowDropAction::Insert(direction) = resolve_action(zone, center)
        && !scene
            .targets
            .iter()
            .any(|target| source.origin_frame.intersection(&target.frame).area() > 0.0)
    {
        let intent = DropIntent {
            window: source.window,
            space,
            frame: source.origin_frame,
            zone,
            action: WindowDropAction::Move(direction),
        };
        if !unavailable.contains(&(intent.window, intent.zone, intent.action)) {
            return Some(intent);
        }
    }

    let nearest = scene
        .targets
        .iter()
        .enumerate()
        .filter(|(_, target)| target.frame.size.width > 0.0 && target.frame.size.height > 0.0);
    if unavailable.is_empty() {
        let (_, target) = nearest.min_by(|(a_order, a), (b_order, b)| {
            distance_to_rect_squared(a.frame, point)
                .total_cmp(&distance_to_rect_squared(b.frame, point))
                .then_with(|| a_order.cmp(b_order))
        })?;
        let clamped = CGPoint::new(
            point.x.clamp(
                target.frame.origin.x,
                target.frame.origin.x + target.frame.size.width,
            ),
            point.y.clamp(
                target.frame.origin.y,
                target.frame.origin.y + target.frame.size.height,
            ),
        );
        return intent_for(target, clamped);
    }
    nearest
        .filter_map(|(order, target)| {
            let clamped = CGPoint::new(
                point.x.clamp(
                    target.frame.origin.x,
                    target.frame.origin.x + target.frame.size.width,
                ),
                point.y.clamp(
                    target.frame.origin.y,
                    target.frame.origin.y + target.frame.size.height,
                ),
            );
            let intent = intent_for(target, clamped)?;
            (!unavailable.contains(&(intent.window, intent.zone, intent.action))).then_some((
                distance_to_rect_squared(target.frame, point),
                order,
                intent,
            ))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, _, intent)| intent)
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

    fn frame(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
    }
    fn point(x: f64, y: f64) -> CGPoint { CGPoint::new(x, y) }
    fn rect() -> CGRect { frame(0.0, 0.0, 200.0, 100.0) }
    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }
    fn space() -> SpaceId { SpaceId::new(1) }
    fn target(idx: u32, frame: CGRect) -> DragSceneTarget {
        DragSceneTarget {
            window: w(idx),
            space: space(),
            frame,
        }
    }
    fn source(tiled: bool) -> DragSource {
        DragSource {
            window: w(1),
            origin_frame: rect(),
            last_frame: rect(),
            origin_space: Some(space()),
            current_space: Some(space()),
            tiled,
        }
    }
    fn scene(frame: CGRect) -> DragScene {
        DragScene {
            action_override: None,
            targets: vec![target(2, frame)],
        }
    }
    fn scene_with(targets: Vec<DragSceneTarget>) -> DragScene {
        DragScene { action_override: None, targets }
    }
    fn motion(actor: &mut DragActor, x: f64, y: f64) -> bool {
        actor.motion(DragMotion { point: point(x, y) })
    }
    fn native(tiled: bool, last_frame: CGRect, scene: DragScene) -> DragActor {
        let mut source = source(tiled);
        source.last_frame = last_frame;
        let mut actor = DragActor::new(MouseSettings::default());
        actor.begin_native(source, scene);
        actor
    }
    fn modifier(action: MouseAction, tiled: bool) -> DragActor {
        let mut actor = DragActor::new(MouseSettings::default());
        actor.begin_modifier(source(tiled), point(100.0, 50.0), action, scene(rect()));
        actor
    }

    #[test]
    fn classifies_target_local_center_and_edges() {
        for (coords, zone) in [
            ((100., 50.), DropZone::Center),
            ((1., 50.), DropZone::West),
            ((199., 50.), DropZone::East),
            ((100., 1.), DropZone::North),
            ((100., 99.), DropZone::South),
            ((0., 0.), DropZone::West),
            ((200., 0.), DropZone::East),
            ((0., 100.), DropZone::West),
            ((200., 100.), DropZone::East),
        ] {
            assert_eq!(
                classify_zone(rect(), point(coords.0, coords.1), 0.25),
                Some(zone)
            );
        }
        assert_eq!(
            resolve_action(DropZone::South, MouseDropAction::Swap),
            WindowDropAction::Insert(Direction::Down),
        );
        assert_eq!(classify_zone(rect(), point(-1.0, 50.0), 0.25), None);
    }

    #[test]
    fn target_zones_keep_their_local_frames() {
        let target = frame(10.0, 20.0, 200.0, 100.0);
        for (zone, expected) in [
            (DropZone::Center, frame(60.0, 45.0, 100.0, 50.0)),
            (DropZone::West, frame(10.0, 20.0, 50.0, 100.0)),
            (DropZone::East, frame(160.0, 20.0, 50.0, 100.0)),
            (DropZone::North, frame(10.0, 20.0, 200.0, 25.0)),
            (DropZone::South, frame(10.0, 95.0, 200.0, 25.0)),
        ] {
            assert_eq!(zone_frame(target, zone, 0.25), expected);
        }
    }

    #[test]
    fn preview_inset_clamps_small_tiles() {
        let intent = hit_test(
            &scene(rect()),
            point(100.0, 50.0),
            0.25,
            MouseDropAction::Swap,
            None,
        )
        .unwrap();
        for (area, expected) in [
            (rect(), frame(4.0, 4.0, 192.0, 92.0)),
            (frame(10.0, 20.0, 6.0, 6.0), frame(14.0, 24.0, 0.0, 0.0)),
        ] {
            assert_eq!(
                preview_frame(DropTarget { intent, preview_area: area }),
                expected
            );
        }
    }

    #[test]
    fn publisher_coalesces_to_newest_motion() {
        let publisher = DragMotionPublisher::default();
        for x in 0..1_000 {
            assert_eq!(
                publisher.publish(DragMotion {
                    point: CGPoint::new(x as f64, 0.0),
                }),
                x == 0
            );
        }
        assert!(publisher.wake_queued());
        assert_eq!(publisher.take_latest().unwrap().point.x, 999.0);
        assert!(!publisher.wake_queued());
        assert!(publisher.publish(DragMotion {
            point: CGPoint::new(1_000.0, 0.0),
        }));
    }

    #[test]
    fn native_tiled_move_targets_pointer_zone_and_commits_once() {
        let target_frame = frame(100.0, 0.0, 100.0, 100.0);
        let moved = frame(10.0, 0.0, 200.0, 100.0);
        let mut actor = native(true, moved, scene(target_frame));
        assert!(motion(&mut actor, 1.0, 50.0));
        let intent = actor.intent().unwrap();
        assert_eq!(intent.action, WindowDropAction::Insert(Direction::Left));
        actor.set_preview(intent, Some(target_frame));
        let preview = preview_frame(actor.target().unwrap());
        assert_eq!(preview.origin, CGPoint::new(104.0, 4.0));
        assert_eq!(preview.size, CGSize::new(92.0, 92.0));
        let commit = actor.finish(MouseButton::Left).unwrap();
        assert_eq!(commit.source.window, w(1));
        assert_eq!(commit.target.unwrap().intent.window, w(2));
        assert!(actor.finish(MouseButton::Left).is_none());
    }

    #[test]
    fn original_tile_edges_offer_keyboard_equivalent_moves() {
        let scene = scene(frame(220.0, 0.0, 200.0, 100.0));
        for ((x, y), direction) in [
            ((1.0, 50.0), Direction::Left),
            ((199.0, 50.0), Direction::Right),
            ((100.0, 1.0), Direction::Up),
            ((100.0, 99.0), Direction::Down),
        ] {
            let mut actor = native(true, rect(), scene.clone());
            motion(&mut actor, x, y);
            let intent = actor.intent().unwrap();
            assert_eq!(intent.window, w(1));
            assert_eq!(intent.action, WindowDropAction::Move(direction));
            actor.set_preview(intent, Some(rect()));
            assert_eq!(
                actor.finish(MouseButton::Left).unwrap().target.unwrap().intent.action,
                WindowDropAction::Move(direction)
            );
        }
    }

    #[test]
    fn unavailable_preview_does_not_expose_a_target() {
        let mut actor = native(true, rect(), scene(rect()));
        motion(&mut actor, 1.0, 50.0);
        let intent = actor.intent().unwrap();
        actor.set_preview(intent, None);
        assert!(actor.target().is_none());
        assert!(actor.finish(MouseButton::Left).unwrap().target.is_none());
    }

    #[test]
    fn unavailable_nearest_target_falls_back_to_the_next_target() {
        let nearest = w(2);
        let fallback = w(3);
        let scene = scene_with(vec![
            target(2, rect()),
            target(3, frame(220.0, 0.0, 200.0, 100.0)),
        ]);
        let mut actor = native(true, rect(), scene);
        motion(&mut actor, 100.0, 50.0);
        let intent = actor.intent().unwrap();
        assert_eq!(intent.window, nearest);
        assert!(actor.set_preview(intent, None));
        assert_eq!(actor.intent().unwrap().window, fallback);
    }

    #[test]
    fn equidistant_outside_targets_keep_scene_order() {
        let scene = scene_with(vec![
            target(2, frame(0.0, 0.0, 100.0, 100.0)),
            target(3, frame(200.0, 0.0, 100.0, 100.0)),
        ]);
        let point = point(150.0, 50.0);
        assert_eq!(
            hit_test(&scene, point, 0.25, MouseDropAction::Swap, None).unwrap().window,
            w(2)
        );
    }

    #[test]
    fn overlapping_targets_prefer_scene_order() {
        let visible = w(2);
        let scene = scene_with(vec![target(2, rect()), target(3, rect())]);
        let intent =
            hit_test(&scene, point(100.0, 50.0), 0.25, MouseDropAction::Swap, None).unwrap();
        assert_eq!(intent.window, visible);
    }

    #[test]
    fn replacing_scene_invalidates_a_validated_preview() {
        let mut actor = native(true, rect(), scene(rect()));
        motion(&mut actor, 100.0, 50.0);
        let intent = actor.intent().unwrap();
        actor.set_preview(intent, Some(rect()));
        assert!(actor.target().is_some());

        let moved = frame(300.0, 0.0, 200.0, 100.0);
        actor.replace_scene(scene(moved));
        assert!(actor.target().is_none());
        let moved_intent = actor.intent().unwrap();
        assert_eq!(moved_intent.frame, moved);
        actor.set_preview(moved_intent, Some(moved));
        let mut settings = MouseSettings::default();
        settings.drop_action = MouseDropAction::Stack;
        actor.update_config(settings);
        assert!(actor.intent().is_none());
        assert!(actor.target().is_none());
    }

    #[test]
    fn floating_move_never_builds_a_drop_target() {
        let moved = frame(10.0, 0.0, 200.0, 100.0);
        let mut actor = native(false, moved, scene(rect()));
        motion(&mut actor, 100.0, 50.0);
        assert!(actor.target().is_none());
    }

    #[test]
    fn native_resize_never_uses_drop_targets() {
        let mut resized = rect();
        resized.size.width += 10.0;
        let mut actor = native(true, resized, scene(rect()));
        motion(&mut actor, 100.0, 50.0);
        assert_eq!(actor.kind(), Some(DragKind::NativeResize));
        assert!(actor.intent().is_none());
        assert!(actor.target().is_none());
    }

    #[test]
    fn modifier_space_change_invalidates_origin_drop_state() {
        let destination = SpaceId::new(2);
        let mut actor = modifier(MouseAction::Move, true);
        motion(&mut actor, 100.0, 50.0);
        let intent = actor.intent().unwrap();
        actor.set_preview(intent, Some(rect()));
        assert!(actor.target().is_some());

        assert!(actor.update_current_space(Some(destination)));
        assert_eq!(actor.source().unwrap().current_space, Some(destination));
        assert!(actor.intent().is_none());
        assert!(actor.target().is_none());
        motion(&mut actor, 100.0, 50.0);
        assert!(actor.intent().is_none());
        assert!(actor.finish(MouseButton::Left).unwrap().target.is_none());
    }

    #[test]
    fn stack_scene_resolves_every_zone_to_swap() {
        let mut scene = scene(rect());
        scene.action_override = Some(WindowDropAction::Swap);
        for point in [point(1.0, 50.0), point(100.0, 50.0)] {
            let intent = hit_test(&scene, point, 0.25, MouseDropAction::Stack, None).unwrap();
            assert_eq!(intent.action, WindowDropAction::Swap);
        }
    }

    #[test]
    fn cancellation_identifies_native_and_modifier_sessions() {
        let mut moved = source(false);
        moved.last_frame = frame(20.0, 20.0, 200.0, 100.0);
        let mut actor = native(false, moved.last_frame, DragScene::default());
        assert_eq!(actor.cancel().unwrap().kind, DragKind::NativeMove);

        actor.begin_modifier(moved, point(10.0, 10.0), MouseAction::Move, DragScene::default());
        assert_eq!(actor.cancel().unwrap().kind, DragKind::ModifierMove);
    }

    #[test]
    fn only_the_owning_button_finishes_a_modifier_drag() {
        let mut actor = DragActor::new(MouseSettings::default());
        let id = actor.await_modifier(MouseButton::Left, point(10.0, 10.0), MouseAction::Move);
        assert!(actor.resolve_start(id, Some(source(true)), DragScene::default()));

        assert!(actor.finish(MouseButton::Right).is_none());
        assert!(actor.is_active());
        assert!(actor.finish(MouseButton::Left).is_some());
        assert!(!actor.is_active());
    }

    #[test]
    fn stale_source_resolution_is_ignored() {
        let mut actor = DragActor::new(MouseSettings::default());
        let stale = actor.await_native(w(1));
        let current = actor.await_native(w(2));
        assert!(!actor.resolve_start(stale, None, DragScene::default()));
        assert!(actor.is_active());
        assert!(actor.resolve_start(current, None, DragScene::default()));
        assert!(!actor.is_active());
    }
}
