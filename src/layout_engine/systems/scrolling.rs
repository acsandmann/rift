//! A strip of persistent columns in world coordinates, translated by one camera.
//! View targets follow niri's fit/center and gesture snap semantics. Ordinary layout
//! animation remains in the reactor; touchpad release belongs to the camera.
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{EditResult, Focus, FocusResult, GroupEdit, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{
    GapSettings, ScrollingAlignment, ScrollingFocusNavigationStyle, ScrollingLayoutSettings,
    WindowInsertionPoint,
};
use crate::layout_engine::systems::constraints::{AxisConstraints, solve_axis_lengths};
use crate::layout_engine::systems::{LayoutSystem, WindowLayoutConstraints};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, ResizeOrientation, WindowDropAction};
use crate::model::hidden_window_placement::HiddenWindowPlacement;
use crate::sys::geometry::Round;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(transparent)]
struct ColumnId(u64);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
enum ColumnWidth {
    #[default]
    Default,
    Proportion(f64),
    Fixed(f64),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct Row {
    window: WindowId,
    height: f64,
}

#[derive(Serialize, Clone, Debug)]
struct Column {
    id: ColumnId,
    rows: Vec<Row>,
    active_window: usize,
    width: ColumnWidth,
}

impl Column {
    fn windows(&self) -> impl DoubleEndedIterator<Item = &WindowId> + ExactSizeIterator + Clone {
        self.rows.iter().map(|row| &row.window)
    }

    fn resolved_width(
        &self,
        view: f64,
        gap: f64,
        settings: &ScrollingLayoutSettings,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        inactive: &HashSet<WindowId>,
    ) -> f64 {
        let ratio = match self.width {
            ColumnWidth::Fixed(width) => return width.max(1.0),
            ColumnWidth::Proportion(ratio) => ratio,
            ColumnWidth::Default => self
                .windows()
                .find(|wid| !inactive.contains(*wid))
                .and_then(|wid| constraints.get(wid))
                .map(|c| c.locked_width)
                .filter(|w| settings.preserve_window_sizes && w.is_finite() && *w > 0.0)
                .map_or(settings.column_width_ratio, |w| width_ratio(view, gap, w)),
        };
        proportional_width(view, gap, clamp_ratio(ratio, settings))
    }

    fn selected(&self) -> Option<WindowId> {
        self.rows.get(self.active_window).map(|row| row.window)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ViewBookmark {
    column: ColumnId,
    // Relative to the bookmarked column so edits to preceding columns reconcile naturally.
    relative_offset: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(from = "f64")]
enum Viewport {
    #[default]
    Uninitialized,
    Static(f64),
    // Keep the previous position until the presenter starts the new target.
    Pending {
        from: f64,
        target: f64,
    },
    Gesture(f64),
    Animation(CameraSpring),
}

impl Viewport {
    // Layout queries use the requested offset; the presenter also needs the
    // previous offset until it starts the animation.
    fn offset(&self) -> f64 {
        match self {
            Self::Uninitialized => 0.0,
            Self::Static(offset) | Self::Gesture(offset) => *offset,
            Self::Pending { target, .. } => *target,
            Self::Animation(spring) => spring.current,
        }
    }

    fn start_offset(&self) -> f64 {
        match self {
            Self::Pending { from, .. } => *from,
            _ => self.offset(),
        }
    }

    fn target(&self) -> f64 {
        match self {
            Self::Animation(spring) => spring.target,
            _ => self.offset(),
        }
    }

    fn finish(&mut self) {
        if !matches!(self, Self::Gesture(_)) {
            *self = Self::Static(self.target());
        }
    }

    fn set_target(&mut self, target: f64) {
        let from = self.start_offset();
        *self = if from == target {
            Self::Static(target)
        } else {
            Self::Pending { from, target }
        };
    }

    fn sample(&mut self, now: Instant, scale: f64) -> Option<bool> {
        if let Self::Pending { target, .. } = *self {
            *self = Self::Static(target);
            return Some(false);
        }
        let Self::Animation(spring) = self else {
            return None;
        };
        // Bounds constrain the destination. After a column closes, the previous
        // position can lie outside them and must remain visible while returning.
        if spring.sample(now, scale) {
            *self = Self::Static(spring.target);
            Some(false)
        } else {
            Some(true)
        }
    }

    fn rebase(&mut self, delta: f64) {
        match self {
            Self::Uninitialized => {}
            Self::Static(offset) | Self::Gesture(offset) => *offset += delta,
            Self::Pending { from, target } => {
                *from += delta;
                *target += delta;
            }
            Self::Animation(spring) => {
                spring.from += delta;
                spring.target += delta;
                spring.current += delta;
            }
        }
    }
}

/// Niri's default horizontal-view-movement: mass 1, critical damping, stiffness 800.
#[derive(Clone, Debug)]
pub(crate) struct CameraSpring {
    from: f64,
    target: f64,
    velocity: f64,
    current: f64,
    current_velocity: f64,
    started: Instant,
    sampled: Instant,
}
impl CameraSpring {
    pub(crate) fn new(from: f64, target: f64, velocity: f64, started: Instant) -> Self {
        Self {
            from,
            target,
            velocity,
            current: from,
            current_velocity: velocity,
            started,
            sampled: started,
        }
    }

    pub(crate) fn current(&self) -> f64 { self.current }

    pub(crate) fn position_velocity(&self, now: Instant) -> (f64, f64) {
        let t = now.saturating_duration_since(self.started).as_secs_f64();
        let omega = 800.0_f64.sqrt();
        let x0 = self.from - self.target;
        let b = omega * x0 + self.velocity;
        let envelope = (-omega * t).exp();
        (
            self.target + envelope * (x0 + b * t),
            envelope * (self.velocity - omega * b * t),
        )
    }

    pub(crate) fn sample(&mut self, now: Instant, scale: f64) -> bool {
        let now = now.max(self.sampled);
        let (position, velocity) = self.position_velocity(now);
        self.current = position;
        self.current_velocity = velocity;
        self.sampled = now;
        // Check both position and speed so crossing the target cannot finish early.
        (self.current - self.target).abs() <= 0.25 / scale
            && velocity.abs() / 800.0_f64.sqrt() <= 0.25 / scale
    }
}

impl From<f64> for Viewport {
    fn from(offset: f64) -> Self { Self::Static(if offset.is_finite() { offset } else { 0.0 }) }
}

/// Semantic release result; animation, when enabled, belongs to the viewport.
#[derive(Clone, Copy, Debug)]
pub struct ViewportRelease {
    pub window: WindowId,
    pub offset: f64,
    pub from_offset: f64,
    pub velocity: f64,
}

#[derive(Clone, Copy, Debug)]
struct ColumnGeometry {
    id: ColumnId,
    world_x: f64,
    width: f64,
}

#[derive(Clone, Copy, Debug)]
struct SnapPoint {
    offset: f64,
    column: usize,
}

#[derive(Clone, Debug)]
struct Geometry {
    screen: CGRect,
    tiling: CGRect,
    gaps: GapSettings,
    constraints: HashMap<WindowId, WindowLayoutConstraints>,
    constraint_revision: Option<u64>,
    columns: Vec<ColumnGeometry>,
    frames: Vec<(WindowId, CGRect)>,
    bounds: (f64, f64),
    snaps: Vec<SnapPoint>,
}

/// Prepared world frames and temporary camera motion. Built only at semantic
/// boundaries; sampling does not enter the layout engine.
#[derive(Debug)]
pub(crate) struct ViewportPresentation {
    viewport: Viewport,
    motion: Arc<Mutex<MotionHistory>>,
    bounds: (f64, f64),
    width: f64,
    tiling: CGRect,
    screen: CGRect,
}

/// Rendered scalar state, read only at semantic boundaries. Motion samples stay
/// shared so release keeps the exact touchpad velocity history without cloning
/// a camera or copying history on each display refresh.
#[derive(Clone, Debug)]
pub(crate) struct PresentedViewport {
    pub offset: f64,
    pub velocity: f64,
    pub target: f64,
    pub gesturing: bool,
    pub timestamp: Instant,
    motion: Arc<Mutex<MotionHistory>>,
}

impl ViewportPresentation {
    pub fn offset(&self) -> f64 { self.viewport.start_offset() }

    pub fn gesturing(&self) -> bool { matches!(self.viewport, Viewport::Gesture(_)) }

    pub fn animated(&self) -> bool { matches!(self.viewport, Viewport::Animation(_)) }

    pub fn finish(&mut self) { self.viewport.finish(); }

    pub fn target(&self) -> f64 { self.viewport.target() }

    pub fn retarget(&mut self, from: f64, velocity: f64, now: Instant) {
        let target = self.target();
        self.viewport = Viewport::Animation(CameraSpring::new(from, target, velocity, now));
    }

    pub fn position_velocity(&self, now: Instant) -> (f64, f64) {
        match &self.viewport {
            Viewport::Animation(s) => s.position_velocity(s.sampled.max(now)),
            _ => (self.offset(), 0.0),
        }
    }

    pub fn update(&mut self, delta: f64, time: Duration) {
        if let Viewport::Gesture(offset) = &mut self.viewport {
            self.motion.lock().update(offset, self.bounds, delta * self.width, time);
        }
    }

    pub fn sample(&mut self, now: Instant, scale: f64) -> bool {
        self.viewport.sample(now, scale).unwrap_or(self.gesturing())
    }

    pub fn snapshot(&self, now: Instant) -> PresentedViewport {
        PresentedViewport {
            offset: self.offset(),
            velocity: match &self.viewport {
                Viewport::Animation(s) => s.current_velocity,
                _ => 0.0,
            },
            target: self.target(),
            gesturing: self.gesturing(),
            timestamp: now,
            motion: self.motion.clone(),
        }
    }

    pub fn target_frame(&self, frame: CGRect, fixed: bool, scale: f64) -> CGRect {
        self.frame_at_offset(frame, fixed, scale, self.target())
    }

    pub fn frame_at_offset(
        &self,
        mut frame: CGRect,
        fixed: bool,
        scale: f64,
        offset: f64,
    ) -> CGRect {
        if !fixed {
            frame = translate_frame(frame, offset, self.tiling, self.screen, true);
        }
        frame.origin.x = (frame.origin.x * scale).round() / scale;
        frame.origin.y = (frame.origin.y * scale).round() / scale;
        frame
    }
}

fn translate_frame(
    mut frame: CGRect,
    offset: f64,
    tiling: CGRect,
    screen: CGRect,
    park: bool,
) -> CGRect {
    frame.origin.x += tiling.origin.x - offset;
    if park {
        if frame.max().x <= tiling.origin.x {
            frame.origin.x = screen.origin.x - frame.size.width + HiddenWindowPlacement::REVEAL_PX;
        } else if frame.origin.x >= tiling.max().x {
            frame.origin.x = screen.max().x - HiddenWindowPlacement::REVEAL_PX;
        }
    }
    frame
}

fn proportional_width(view: f64, gap: f64, ratio: f64) -> f64 {
    ((view + gap) * ratio - gap).max(1.0)
}

fn width_ratio(view: f64, gap: f64, width: f64) -> f64 { (width + gap) / (view + gap).max(1.0) }

fn clamp_ratio(ratio: f64, settings: &ScrollingLayoutSettings) -> f64 {
    let min = settings.min_column_width_ratio.max(0.05);
    let max = settings.max_column_width_ratio.max(min);
    if ratio.is_finite() {
        ratio.clamp(min, max)
    } else {
        min
    }
}

// Adapted from niri's compute_new_view_offset, using an absolute camera.
// Rift's outer gaps already reserve padding, including at both strip boundaries.
fn fit_offset(current: f64, view: f64, column: ColumnGeometry) -> f64 {
    let left = column.world_x;
    if column.width >= view {
        return left;
    }
    current.clamp(left + column.width - view, left)
}

fn centered_offset(view: f64, column: ColumnGeometry) -> f64 {
    column.world_x - (view - column.width).max(0.0) / 2.0
}

impl Geometry {
    fn build(
        state: &LayoutState,
        settings: &ScrollingLayoutSettings,
        inactive: &HashSet<WindowId>,
        screen: CGRect,
        constraints: HashMap<WindowId, WindowLayoutConstraints>,
        constraint_revision: Option<u64>,
        gaps: GapSettings,
    ) -> Self {
        let tiling = compute_tiling_area(screen, &gaps);
        let mut columns = Vec::with_capacity(state.arrangement.columns.len());
        let mut frames =
            Vec::with_capacity(state.arrangement.columns.iter().map(|c| c.rows.len()).sum());
        let constraint = |wid: &WindowId| {
            constraints
                .get(wid)
                .copied()
                .unwrap_or(WindowLayoutConstraints {
                    is_resizable: true,
                    ..Default::default()
                })
                .normalized()
        };
        let mut x = 0.0;
        let active_columns = if inactive.is_empty() {
            state.arrangement.columns.len()
        } else {
            state
                .arrangement
                .columns
                .iter()
                .filter(|c| c.windows().any(|w| !inactive.contains(w)))
                .count()
        };
        for column in &state.arrangement.columns {
            let active_rows = column.rows.iter().filter(|row| !inactive.contains(&row.window));
            let mut rows = Vec::with_capacity(column.rows.len());
            let mut min: f64 = 1.0;
            let mut fixed: f64 = 0.0;
            let mut max = f64::INFINITY;
            let mut flexible = false;
            for row in active_rows.clone() {
                let c = constraint(&row.window);
                let fixed_width = c.fixed_for_axis(true);
                min = min.max(c.min_width);
                fixed = fixed.max(fixed_width.unwrap_or(0.0));
                flexible |= fixed_width.is_none();
                if c.max_width > 0.0 {
                    max = max.min(c.max_width);
                }
                let fixed_height = c.fixed_for_axis(false);
                rows.push(AxisConstraints {
                    min: c.min_height,
                    fixed: fixed_height,
                    max: (c.max_height > 0.0).then_some(c.max_height),
                    weight: (row.height - c.min_height).max(0.001),
                    can_grow: fixed_height.is_none(),
                });
            }
            if rows.is_empty() {
                continue;
            }
            let base = if settings.expand_single_column && active_columns == 1 {
                tiling.size.width
            } else {
                column.resolved_width(
                    tiling.size.width,
                    gaps.inner.horizontal,
                    settings,
                    &constraints,
                    inactive,
                )
            };
            let width = if flexible {
                base.min(max).max(min).max(fixed)
            } else {
                min.max(fixed)
            };
            columns.push(ColumnGeometry {
                id: column.id,
                world_x: x,
                width,
            });
            let available =
                (tiling.size.height - gaps.inner.vertical * (rows.len() - 1) as f64).max(0.0);
            let heights = solve_axis_lengths(rows, available);
            let mut y = tiling.origin.y;
            for (row, height) in active_rows.zip(heights) {
                let wid = row.window;
                let mut size = CGSize::new(width, height);
                let c = constraint(&wid);
                size.width = c.fixed_for_axis(true).unwrap_or(width).max(c.min_width).min(width);
                size.height =
                    c.fixed_for_axis(false).unwrap_or(height).max(c.min_height).min(height);
                if c.max_width > 0.0 {
                    size.width = size.width.min(c.max_width);
                }
                if c.max_height > 0.0 {
                    size.height = size.height.min(c.max_height);
                }
                frames.push((wid, CGRect::new(CGPoint::new(x, y), size)));
                y += height + gaps.inner.vertical;
            }
            x += width + gaps.inner.horizontal;
        }
        let mut geometry = Self {
            screen,
            tiling,
            gaps,
            constraints,
            constraint_revision,
            columns,
            frames,
            bounds: (0.0, 0.0),
            snaps: Vec::new(),
        };
        // The bounds are resting positions, not topology-derived column indices.
        geometry.snaps = geometry.snap_points(settings);
        if !geometry.snaps.is_empty() {
            geometry.bounds = geometry
                .snaps
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), snap| {
                    (min.min(snap.offset), max.max(snap.offset))
                });
        }
        geometry
    }

    fn same_projected_constraints(
        &self,
        current: &HashMap<WindowId, WindowLayoutConstraints>,
    ) -> bool {
        self.frames
            .iter()
            .all(|(window, _)| self.constraints.get(window) == current.get(window))
    }

    fn has_constraints(
        &self,
        current: &HashMap<WindowId, WindowLayoutConstraints>,
        revision: Option<u64>,
    ) -> bool {
        match (self.constraint_revision, revision) {
            (Some(saved), Some(current)) => saved == current,
            _ => self.constraints == *current,
        }
    }

    fn column(&self, id: ColumnId) -> Option<ColumnGeometry> {
        self.columns.iter().find(|c| c.id == id).copied()
    }

    fn snap_points(&self, settings: &ScrollingLayoutSettings) -> Vec<SnapPoint> {
        let Some(first) = self.columns.first() else {
            return Vec::new();
        };
        let view = self.tiling.size.width;
        let anchored = settings.focus_navigation_style == ScrollingFocusNavigationStyle::Anchored;
        if anchored {
            return self
                .columns
                .iter()
                .enumerate()
                .map(|(index, _)| SnapPoint {
                    offset: self.anchor_offset(index, settings.alignment),
                    column: index,
                })
                .collect();
        }
        let last = self.columns.last().unwrap();
        let left = first.world_x;
        let right = last.world_x + last.width - view;
        let mut points = vec![SnapPoint { offset: left, column: 0 }, SnapPoint {
            offset: right,
            column: self.columns.len() - 1,
        }];
        for (index, column) in self.columns.iter().enumerate() {
            for offset in [column.world_x, column.world_x + column.width - view] {
                if left < offset && offset < right {
                    points.push(SnapPoint { offset, column: index });
                }
            }
        }
        points
    }

    fn anchor_offset(&self, index: usize, alignment: ScrollingAlignment) -> f64 {
        let column = self.columns[index];
        if column.width >= self.tiling.size.width {
            return column.world_x;
        }
        // Preserve Rift's anchored first/last-column policy.
        if self.columns.len() > 1 {
            if index == 0 {
                return column.world_x;
            }
            if index + 1 == self.columns.len() {
                return column.world_x + column.width - self.tiling.size.width;
            }
        }
        match alignment {
            ScrollingAlignment::Left => column.world_x,
            ScrollingAlignment::Center => centered_offset(self.tiling.size.width, column),
            ScrollingAlignment::Right => column.world_x + column.width - self.tiling.size.width,
        }
    }
}

/// Recent camera deltas in pixels, independent of viewport rebasing.
#[derive(Clone, Debug, Default)]
struct MotionHistory {
    samples: VecDeque<(Duration, f64)>,
    overscroll: f64,
}
impl MotionHistory {
    fn push(&mut self, delta: f64, time: Duration) -> bool {
        if self.samples.back().is_some_and(|(last, _)| time < *last) {
            return false;
        }
        // macOS reports stationary contacts continuously; retain one fresh idle
        // anchor so that a pause before a new flick does not dilute that flick.
        if delta == 0.0
            && let Some((last, 0.0)) = self.samples.back_mut()
        {
            *last = time;
        } else {
            if self.samples.len() == 64 {
                self.samples.pop_front();
            }
            self.samples.push_back((time, delta));
        }
        while self
            .samples
            .front()
            .is_some_and(|(first, _)| time.saturating_sub(*first) > Duration::from_millis(150))
        {
            self.samples.pop_front();
        }
        true
    }

    fn update(
        &mut self,
        offset: &mut f64,
        bounds: (f64, f64),
        delta: f64,
        time: Duration,
    ) -> Option<f64> {
        if !delta.is_finite() {
            return None;
        }
        let raw = *offset + self.overscroll + delta;
        let bounded = raw.clamp(bounds.0, bounds.1);
        let moved = bounded - *offset;
        if !self.push(moved, time) {
            return None;
        }
        self.overscroll = raw - bounded;
        *offset = bounded;
        Some(moved)
    }

    fn velocity(&self) -> f64 {
        let (Some(&(first, _)), Some(&(last, _))) = (self.samples.front(), self.samples.back())
        else {
            return 0.0;
        };
        let dt = last.saturating_sub(first).as_secs_f64();
        if dt > 0.0 {
            self.samples.iter().map(|(_, delta)| delta).sum::<f64>() / dt
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum FullscreenMode {
    Screen,
    WithinGaps,
}

#[derive(Clone, Debug, Default)]
struct Arrangement {
    columns: Vec<Column>,
    active_column: usize,
    next_column_id: u64,
    fullscreen: HashMap<WindowId, FullscreenMode>,
}

enum MoveDestination {
    NewColumn(usize),
    Column(usize),
    Row(usize),
}

#[derive(Clone, Debug, Default)]
struct Camera {
    viewport: Viewport,
    motion: MotionHistory,
    transient_restore: Option<ViewBookmark>,
    fullscreen_restore: Option<ViewBookmark>,
    restored_view: Option<ViewBookmark>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(from = "StoredLayoutState")]
struct LayoutState {
    arrangement: Arrangement,
    camera: Camera,
    geometry: Option<Geometry>,
}

// Store the camera relative to its active column as well as its absolute
// position. On another display, the first preparation rebases it using the new
// geometry. Serialize borrowed fields so saving does not clone geometry/maps.
impl Serialize for LayoutState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut stored = serializer.serialize_struct("LayoutState", 8)?;
        stored.serialize_field("columns", &self.arrangement.columns)?;
        stored.serialize_field("active_column", &self.arrangement.active_column)?;
        stored.serialize_field("next_column_id", &self.arrangement.next_column_id)?;
        let current = self.camera.viewport.offset();
        let offset = if matches!(
            self.camera.viewport,
            Viewport::Gesture(_) | Viewport::Animation(_)
        ) {
            self.geometry
                .as_ref()
                .map_or(current, |g| current.clamp(g.bounds.0, g.bounds.1))
        } else {
            current
        };
        stored.serialize_field("viewport", &offset)?;
        let anchor =
            self.bookmark().or_else(|| self.camera.restored_view.clone()).map(|mut anchor| {
                anchor.relative_offset += offset - current;
                anchor
            });
        stored.serialize_field("view_anchor", &anchor)?;
        stored.serialize_field("transient_restore", &self.camera.transient_restore)?;
        stored.serialize_field("fullscreen_restore", &self.camera.fullscreen_restore)?;
        stored.serialize_field("fullscreen_modes", &self.arrangement.fullscreen)?;
        stored.end()
    }
}

// Released snapshots on main store selected, column_width_ratio, node_id,
// width_offset and width_overridden. Keep their migration at this boundary;
// current snapshots use the same flat representation.
#[derive(Deserialize, Default)]
#[serde(default)]
struct StoredLayoutState {
    columns: Vec<StoredColumn>,
    active_column: usize,
    next_column_id: u64,
    viewport: Viewport,
    view_anchor: Option<ViewBookmark>,
    transient_restore: Option<ViewBookmark>,
    fullscreen_restore: Option<ViewBookmark>,
    fullscreen_modes: HashMap<WindowId, FullscreenMode>,
    fullscreen: HashSet<WindowId>,
    fullscreen_within_gaps: HashSet<WindowId>,
    selected: Option<WindowId>,
    column_width_ratio: f64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StoredColumn {
    #[serde(alias = "node_id")]
    id: u64,
    windows: Vec<WindowId>,
    rows: Vec<Row>,
    active_window: usize,
    width: ColumnWidth,
    height_weights: Vec<f64>,
    width_offset: f64,
    width_overridden: bool,
}

impl From<StoredLayoutState> for LayoutState {
    fn from(stored: StoredLayoutState) -> Self {
        let mut next = stored.next_column_id.max(2);
        for column in &stored.columns {
            next = next.max(column.id.saturating_add(2));
        }
        let base = if stored.column_width_ratio > 0.0 {
            stored.column_width_ratio
        } else {
            0.7
        };
        let columns = stored
            .columns
            .into_iter()
            .filter_map(|mut c| {
                if c.rows.is_empty() {
                    c.rows = c
                        .windows
                        .into_iter()
                        .enumerate()
                        .map(|(i, window)| Row {
                            window,
                            height: c.height_weights.get(i).copied().unwrap_or(1.0),
                        })
                        .collect();
                }
                if c.rows.is_empty() {
                    return None;
                }
                if c.id == 0 {
                    c.id = next;
                    next += 2;
                }
                let width = if c.width_overridden || c.width_offset != 0.0 {
                    ColumnWidth::Proportion(base + c.width_offset)
                } else {
                    c.width
                };
                Some(Column {
                    id: ColumnId(c.id),
                    active_window: c.active_window.min(c.rows.len() - 1),
                    rows: c.rows,
                    width,
                })
            })
            .collect::<Vec<_>>();
        let active_column = stored.active_column.min(columns.len().saturating_sub(1));
        let mut fullscreen = stored.fullscreen_modes;
        fullscreen.extend(
            stored
                .fullscreen_within_gaps
                .into_iter()
                .map(|w| (w, FullscreenMode::WithinGaps)),
        );
        fullscreen.extend(stored.fullscreen.into_iter().map(|w| (w, FullscreenMode::Screen)));
        let mut state = Self {
            arrangement: Arrangement {
                columns,
                active_column,
                next_column_id: next,
                fullscreen,
            },
            camera: Camera {
                viewport: stored.viewport,
                transient_restore: stored.transient_restore,
                fullscreen_restore: stored.fullscreen_restore,
                restored_view: stored.view_anchor,
                ..Camera::default()
            },
            geometry: None,
        };
        if let Some(window) = stored.selected {
            state.arrangement.activate(window);
        }
        state
    }
}

impl Arrangement {
    fn selected(&self, inactive: &HashSet<WindowId>) -> Option<WindowId> {
        let (col, row) = self.selected_location(inactive)?;
        Some(self.columns[col].rows[row].window)
    }

    fn selected_location(&self, inactive: &HashSet<WindowId>) -> Option<(usize, usize)> {
        let column = self.columns.get(self.active_column)?;
        let row = if column.selected().is_some_and(|w| !inactive.contains(&w)) {
            column.active_window
        } else {
            column.windows().position(|w| !inactive.contains(w))?
        };
        Some((self.active_column, row))
    }

    fn select_available_column(
        &mut self,
        preferred: Option<ColumnId>,
        inactive: &HashSet<WindowId>,
    ) -> Option<ColumnId> {
        let bookmarked =
            preferred.and_then(|id| self.columns.iter().position(|column| column.id == id));
        let candidate = bookmarked
            .into_iter()
            .chain(self.active_column..self.columns.len())
            .chain((0..self.active_column).rev())
            .find(|&index| self.columns[index].windows().any(|w| !inactive.contains(w)))?;
        self.active_column = candidate;
        Some(self.columns[candidate].id)
    }

    fn locate(&self, wid: WindowId) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(col, column)| {
            column.windows().position(|&w| w == wid).map(|row| (col, row))
        })
    }

    fn move_destination(
        &self,
        (col, row): (usize, usize),
        direction: Direction,
        inactive: &HashSet<WindowId>,
    ) -> Option<MoveDestination> {
        let horizontal = matches!(direction, Direction::Left | Direction::Right);
        let rows = &self.columns[col].rows;
        if horizontal && rows.len() > 1 {
            return Some(MoveDestination::NewColumn(
                col + usize::from(direction == Direction::Right),
            ));
        }
        let (index, len) = if horizontal {
            (col, self.columns.len())
        } else {
            (row, rows.len())
        };
        let participates = |candidate: &usize| {
            if horizontal {
                self.columns[*candidate].windows().any(|w| !inactive.contains(w))
            } else {
                !inactive.contains(&rows[*candidate].window)
            }
        };
        let target = match direction {
            Direction::Left | Direction::Up => (0..index).rev().find(participates),
            Direction::Right | Direction::Down => (index + 1..len).find(participates),
        }?;
        Some(if horizontal {
            MoveDestination::Column(target)
        } else {
            MoveDestination::Row(target)
        })
    }

    fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.columns.iter().flat_map(|c| c.windows().copied())
    }

    fn activate(&mut self, wid: WindowId) -> bool {
        let Some((col, row)) = self.locate(wid) else {
            return false;
        };
        self.active_column = col;
        self.columns[col].active_window = row;
        true
    }

    fn new_column(&mut self, index: usize, wid: WindowId, width: ColumnWidth, weight: f64) {
        self.next_column_id = self.next_column_id.max(2);
        let id = ColumnId(self.next_column_id);
        self.next_column_id += 2; // Even container IDs never collide with odd window IDs.
        self.columns.insert(index.min(self.columns.len()), Column {
            id,
            rows: vec![Row { window: wid, height: weight }],
            active_window: 0,
            width,
        });
        self.activate(wid);
    }

    /// Detach without destroying sizing/fullscreen metadata; shared by every transfer.
    fn detach(
        &mut self,
        wid: WindowId,
        inactive: &HashSet<WindowId>,
    ) -> Option<(ColumnWidth, f64)> {
        let (col, row) = self.locate(wid)?;
        let selected = self.selected(inactive);
        let column = &mut self.columns[col];
        let width = column.width;
        let weight = column.rows.remove(row).height;
        if row < column.active_window {
            column.active_window -= 1;
        }
        column.active_window = column.active_window.min(column.rows.len().saturating_sub(1));
        if column.rows.is_empty() {
            self.columns.remove(col);
            if col < self.active_column {
                self.active_column -= 1;
            }
        }
        self.active_column = self.active_column.min(self.columns.len().saturating_sub(1));
        if let Some(selected) = selected.filter(|&w| w != wid) {
            self.activate(selected);
        }
        Some((width, weight))
    }

    fn transfer(
        &mut self,
        source: WindowId,
        target: WindowId,
        action: WindowDropAction,
        inactive: &HashSet<WindowId>,
    ) -> bool {
        if source == target || self.locate(source).is_none() || self.locate(target).is_none() {
            return false;
        }
        let (width, weight) = self.detach(source, inactive).unwrap();
        let (col, row) = self.locate(target).unwrap();
        match action {
            WindowDropAction::Stack | WindowDropAction::Insert(Direction::Up | Direction::Down) => {
                let row = row + usize::from(action != WindowDropAction::Insert(Direction::Up));
                let column = &mut self.columns[col];
                if row <= column.active_window {
                    column.active_window += 1;
                }
                column.rows.insert(row, Row { window: source, height: weight });
            }
            WindowDropAction::Insert(direction @ (Direction::Left | Direction::Right)) => {
                self.new_column(
                    col + usize::from(direction == Direction::Right),
                    source,
                    width,
                    weight,
                );
            }
            _ => unreachable!("move and swap handled before transfer"),
        }
        self.activate(source);
        true
    }
}

impl LayoutState {
    fn fullscreen_frame(&self, geometry: &Geometry, window: WindowId) -> Option<CGRect> {
        Some(match self.arrangement.fullscreen.get(&window)? {
            FullscreenMode::Screen => geometry.screen,
            FullscreenMode::WithinGaps => geometry.tiling,
        })
    }

    fn restore_fullscreen_view(&mut self) -> bool {
        let fullscreen =
            self.arrangement
                .columns
                .get(self.arrangement.active_column)
                .is_some_and(|column| {
                    column.windows().any(|wid| self.arrangement.fullscreen.contains_key(wid))
                });
        !fullscreen
            && self
                .camera
                .fullscreen_restore
                .take()
                .is_some_and(|bookmark| self.restore(bookmark))
    }

    fn prepare_geometry(
        &mut self,
        settings: &ScrollingLayoutSettings,
        inactive: &HashSet<WindowId>,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        revision: Option<u64>,
        gaps: &GapSettings,
    ) {
        let same_constraints =
            self.geometry.as_ref().is_some_and(|g| g.has_constraints(constraints, revision));
        if let Some(g) = self.geometry.as_mut()
            && g.screen == screen
            && g.gaps == *gaps
            && (same_constraints
                || (!settings.preserve_window_sizes && g.same_projected_constraints(constraints)))
        {
            if !same_constraints {
                // Edits may introduce a member before the next engine projection.
                g.constraints.clone_from(constraints);
            }
            g.constraint_revision = revision;
            return;
        }
        if self.camera.restored_view.is_none() {
            self.camera.restored_view = self.bookmark();
        }
        let initial = matches!(self.camera.viewport, Viewport::Uninitialized);
        if settings.preserve_window_sizes {
            let tiling = compute_tiling_area(screen, gaps);
            for column in &mut self.arrangement.columns {
                if matches!(column.width, ColumnWidth::Default)
                    && column.rows.len() == 1
                    && !inactive.contains(&column.rows[0].window)
                    && constraints
                        .get(&column.rows[0].window)
                        .map(|c| c.locked_width)
                        .is_some_and(|w| w.is_finite() && w > 0.0)
                {
                    column.width = ColumnWidth::Fixed(column.resolved_width(
                        tiling.size.width,
                        gaps.inner.horizontal,
                        settings,
                        constraints,
                        inactive,
                    ));
                }
            }
        }
        let mut snapshot = self.geometry.take().map(|g| g.constraints).unwrap_or_default();
        if !same_constraints {
            snapshot.clone_from(constraints);
        }
        self.geometry = Some(Geometry::build(
            self,
            settings,
            inactive,
            screen,
            snapshot,
            revision,
            gaps.clone(),
        ));
        self.apply_restored_view();
        self.reconcile_camera_bounds();
        if initial {
            self.camera.viewport = Viewport::Static(0.0);
            if self.geometry.as_ref().unwrap().columns.len() == 1 {
                let g = self.geometry.as_ref().unwrap();
                self.camera.viewport = Viewport::Static(g.anchor_offset(0, settings.alignment));
            } else {
                self.reveal(settings);
            }
            self.camera.viewport = Viewport::Static(self.camera.viewport.offset());
        }
    }

    fn apply_restored_view(&mut self) {
        let (Some(bookmark), Some(geometry)) = (&self.camera.restored_view, &self.geometry) else {
            return;
        };
        if let Some(column) = geometry.column(bookmark.column) {
            let offset = self.camera.viewport.offset();
            self.camera.viewport.rebase(column.world_x + bookmark.relative_offset - offset);
            self.camera.restored_view = None;
        } else {
            // A dormant column has no geometry. Keep its bookmark for readmission,
            // and use the available strip bounds for the current projection.
            let offset = self.camera.viewport.offset().clamp(geometry.bounds.0, geometry.bounds.1);
            self.camera.viewport = Viewport::Static(offset);
            if !self.arrangement.columns.iter().any(|column| column.id == bookmark.column) {
                self.camera.restored_view = None;
            }
        }
    }

    fn reconcile_camera_bounds(&mut self) {
        if let (Viewport::Animation(spring), Some(g)) = (&mut self.camera.viewport, &self.geometry)
        {
            let target = spring.target.clamp(g.bounds.0, g.bounds.1);
            if target != spring.target {
                let now = Instant::now().max(spring.sampled);
                let (_, velocity) = spring.position_velocity(now);
                spring.from = spring.current;
                spring.velocity = velocity;
                spring.target = target;
                spring.started = now;
                spring.sampled = now;
            }
        }
    }

    fn bookmark(&self) -> Option<ViewBookmark> {
        let id = self.arrangement.columns.get(self.arrangement.active_column)?.id;
        let x = self.geometry.as_ref()?.column(id)?.world_x;
        Some(ViewBookmark {
            column: id,
            relative_offset: self.camera.viewport.offset() - x,
        })
    }

    fn restore(&mut self, bookmark: ViewBookmark) -> bool {
        let Some(index) = self.arrangement.columns.iter().position(|c| c.id == bookmark.column)
        else {
            return false;
        };
        self.arrangement.active_column = index;
        if let Some(g) = &self.geometry
            && let Some(column) = g.column(bookmark.column)
        {
            let view = g.tiling.size.width;
            self.camera.viewport.set_target(fit_offset(
                column.world_x + bookmark.relative_offset,
                view,
                column,
            ));
        }
        true
    }

    fn reveal(&mut self, settings: &ScrollingLayoutSettings) {
        let Some(g) = &self.geometry else {
            return;
        };
        let Some(column) = self
            .arrangement
            .columns
            .get(self.arrangement.active_column)
            .and_then(|c| g.column(c.id))
        else {
            return;
        };
        let offset = if settings.focus_navigation_style == ScrollingFocusNavigationStyle::Anchored {
            g.anchor_offset(
                g.columns.iter().position(|c| c.id == column.id).unwrap(),
                settings.alignment,
            )
        } else {
            let target = match &self.camera.viewport {
                Viewport::Animation(s) => s.target,
                _ => self.camera.viewport.offset(),
            };
            fit_offset(target, g.tiling.size.width, column)
        };
        if matches!(&self.camera.viewport, Viewport::Animation(s) if s.target == offset) {
            return;
        }
        self.camera.viewport.set_target(offset);
    }

    /// All topology/sizing edits use this transaction. With screen_x = world_x -
    /// camera, the camera changes by NEW world_x - OLD world_x. Rebase the free
    /// gesture as well, without losing its recent velocity samples.
    fn mutate<R>(
        &mut self,
        settings: &ScrollingLayoutSettings,
        inactive: &HashSet<WindowId>,
        edit: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let window = self.arrangement.selected(inactive);
        let id = self.arrangement.columns.get(self.arrangement.active_column).map(|c| c.id);
        let old_x = id.and_then(|id| self.geometry.as_ref()?.column(id)).map(|c| c.world_x);
        let result = edit(self);
        if let Some(old) = self.geometry.take() {
            self.geometry = Some(Geometry::build(
                self,
                settings,
                inactive,
                old.screen,
                old.constraints,
                old.constraint_revision,
                old.gaps,
            ));
            let anchor = window
                .and_then(|wid| self.arrangement.locate(wid))
                .map(|(col, _)| self.arrangement.columns[col].id)
                .or_else(|| id.filter(|id| self.arrangement.columns.iter().any(|c| c.id == *id)));
            if let (Some(old_x), Some(new)) =
                (old_x, anchor.and_then(|id| self.geometry.as_ref()?.column(id)))
            {
                self.camera.viewport.rebase(new.world_x - old_x);
            }
            self.apply_restored_view();
        }
        self.reconcile_camera_bounds();
        if self.arrangement.columns.is_empty() {
            self.camera.viewport = Viewport::Static(0.0);
        }
        result
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ScrollingLayoutSystem {
    #[serde(skip)]
    inactive: HashSet<WindowId>,
    layouts: slotmap::SlotMap<LayoutId, LayoutState>,
    #[serde(skip)]
    pub(super) settings: ScrollingLayoutSettings,
}

impl ScrollingLayoutSystem {
    pub(super) fn initialize_dormant_windows(&mut self, windows: Vec<WindowId>) {
        self.inactive.extend(windows);
    }

    fn selected_mut<'a>(
        layouts: &'a mut slotmap::SlotMap<LayoutId, LayoutState>,
        layout: LayoutId,
        inactive: &HashSet<WindowId>,
    ) -> Option<(&'a mut LayoutState, usize, usize)> {
        let state = layouts.get_mut(layout)?;
        let (column, row) = state.arrangement.selected_location(inactive)?;
        Some((state, column, row))
    }

    pub fn new(settings: &ScrollingLayoutSettings) -> Self {
        let mut system = Self::default();
        system.update_settings(settings);
        system
    }

    pub fn update_settings(&mut self, settings: &ScrollingLayoutSettings) {
        let mut settings = settings.clone();
        settings.per_display.clear();
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        for state in self.layouts.values_mut() {
            state.mutate(&self.settings, &self.inactive, |_| {});
        }
    }

    pub fn update_width_settings(&mut self, (ratio, min, max): (f64, f64, f64)) {
        if (ratio, min, max) == self.configured_widths() {
            return;
        }
        self.settings.column_width_ratio = ratio;
        self.settings.min_column_width_ratio = min;
        self.settings.max_column_width_ratio = max;
        for state in self.layouts.values_mut() {
            state.mutate(&self.settings, &self.inactive, |_| {});
        }
    }

    pub(crate) fn configured_widths(&self) -> (f64, f64, f64) {
        (
            self.settings.column_width_ratio,
            self.settings.min_column_width_ratio,
            self.settings.max_column_width_ratio,
        )
    }

    fn insert(&mut self, layout: LayoutId, wid: WindowId) {
        let Some(state) = self.layouts.get_mut(layout) else {
            return;
        };
        if state.arrangement.locate(wid).is_some() {
            return;
        }
        let bookmark = state.bookmark();
        state.camera.restored_view = None;
        state.mutate(&self.settings, &self.inactive, |state| {
            let index = if self.settings.base.window_insertion_point.unwrap_or_default()
                == WindowInsertionPoint::EndOfTree
            {
                state.arrangement.columns.len()
            } else {
                state
                    .arrangement
                    .selected_location(&self.inactive)
                    .map_or(0, |(col, _)| col + 1)
            };
            state.arrangement.new_column(index, wid, ColumnWidth::Default, 1.0);
        });
        state.camera.transient_restore = bookmark;
        state.reveal(&self.settings);
    }

    fn remove_from_state(
        state: &mut LayoutState,
        settings: &ScrollingLayoutSettings,
        wid: WindowId,
        inactive: &HashSet<WindowId>,
    ) {
        let Some((col, _)) = state.arrangement.locate(wid) else {
            return;
        };
        let active_removed = state.arrangement.selected(inactive) == Some(wid);
        let column_removed = state.arrangement.columns[col].rows.len() == 1;
        let restore = if active_removed && column_removed {
            state.camera.transient_restore.take()
        } else {
            None
        };
        let was_fullscreen = state.arrangement.fullscreen.remove(&wid).is_some();
        let selected_column = state.mutate(settings, inactive, |state| {
            state.arrangement.detach(wid, inactive);
            if active_removed {
                state.arrangement.select_available_column(
                    restore.as_ref().map(|bookmark| bookmark.column),
                    inactive,
                )
            } else {
                None
            }
        });
        let restored = restore
            .filter(|bookmark| Some(bookmark.column) == selected_column)
            .is_some_and(|bookmark| state.restore(bookmark));
        let restored_fullscreen = was_fullscreen && state.restore_fullscreen_view();
        if active_removed && !restored && !restored_fullscreen {
            state.reveal(settings);
        }
        if state
            .camera
            .transient_restore
            .as_ref()
            .is_some_and(|b| !state.arrangement.columns.iter().any(|c| c.id == b.column))
        {
            state.camera.transient_restore = None;
        }
    }

    pub(crate) fn prepare(
        &mut self,
        layout: LayoutId,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        revision: Option<u64>,
        gaps: &GapSettings,
    ) {
        if let Some(state) = self.layouts.get_mut(layout) {
            state.prepare_geometry(
                &self.settings,
                &self.inactive,
                screen,
                constraints,
                revision,
                gaps,
            );
        }
    }

    pub(crate) fn calculate_frames(
        &self,
        layout: LayoutId,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        revision: Option<u64>,
        gaps: &GapSettings,
        park: bool,
    ) -> Vec<(WindowId, CGRect)> {
        let Some(state) = self.layouts.get(layout) else {
            return Vec::new();
        };
        if let Some(g) = &state.geometry
            && g.screen == screen
            && g.gaps == *gaps
            && (g.has_constraints(constraints, revision)
                || (!self.settings.preserve_window_sizes
                    && g.same_projected_constraints(constraints)))
        {
            return Self::translate_frames(state, g, park).collect();
        }
        let mut camera = state.camera.clone();
        camera.restored_view = camera.restored_view.or_else(|| state.bookmark());
        let mut fallback = LayoutState {
            arrangement: state.arrangement.clone(),
            camera,
            geometry: None,
        };
        fallback.prepare_geometry(
            &self.settings,
            &self.inactive,
            screen,
            constraints,
            revision,
            gaps,
        );
        Self::translate_frames(&fallback, fallback.geometry.as_ref().unwrap(), park).collect()
    }

    fn translate_frames<'a>(
        state: &'a LayoutState,
        g: &'a Geometry,
        park: bool,
    ) -> impl Iterator<Item = (WindowId, CGRect)> + 'a {
        let offset = state.camera.viewport.offset();
        g.frames.iter().map(move |&(wid, frame)| {
            let frame = state.fullscreen_frame(g, wid).unwrap_or_else(|| {
                translate_frame(frame, offset, g.tiling, g.screen, park).round()
            });
            (wid, frame)
        })
    }

    pub(crate) fn finish_viewport(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.get_mut(layout)
            && state.geometry.is_some()
        {
            state.camera.viewport.finish();
        }
    }

    pub(crate) fn presentation(
        &self,
        layout: LayoutId,
    ) -> Option<(ViewportPresentation, Vec<(WindowId, CGRect, bool)>)> {
        let state = self.layouts.get(layout)?;
        let g = state.geometry.as_ref()?;
        let mut motion = state.camera.motion.clone();
        motion.samples.reserve(64_usize.saturating_sub(motion.samples.len()));
        let camera = ViewportPresentation {
            viewport: state.camera.viewport.clone(),
            motion: Arc::new(Mutex::new(motion)),
            bounds: g.bounds,
            width: g.tiling.size.width,
            tiling: g.tiling,
            screen: g.screen,
        };
        let frames = g
            .frames
            .iter()
            .map(|&(wid, frame)| {
                state
                    .fullscreen_frame(g, wid)
                    .map_or_else(|| (wid, frame.round(), false), |frame| (wid, frame, true))
            })
            .collect();
        Some((camera, frames))
    }

    pub(crate) fn commit_presented_viewport(&mut self, layout: LayoutId, p: &PresentedViewport) {
        if let Some(state) = self.layouts.get_mut(layout) {
            state.camera.viewport = if p.gesturing {
                Viewport::Gesture(p.offset)
            } else if p.offset != p.target {
                Viewport::Animation(CameraSpring::new(p.offset, p.target, p.velocity, p.timestamp))
            } else {
                Viewport::Static(p.offset)
            };
            state.camera.motion = p.motion.lock().clone();
        }
    }

    /// Legacy normalized strip delta; boundary recognition is owned by the caller.
    pub fn scroll_by_delta(&mut self, layout: LayoutId, delta: f64) -> Option<(Direction, f64)> {
        if !delta.is_finite() {
            return None;
        }
        let state = self.layouts.get_mut(layout)?;
        let g = state.geometry.as_ref()?;
        let column = state
            .arrangement
            .columns
            .get(state.arrangement.active_column)
            .and_then(|c| g.column(c.id))?;
        let step = column.width + g.gaps.inner.horizontal;
        let raw = state.camera.viewport.offset() + delta * step;
        state.camera.viewport.set_target(raw.clamp(g.bounds.0, g.bounds.1));
        if raw < g.bounds.0 {
            Some((Direction::Left, (g.bounds.0 - raw) / step))
        } else if raw > g.bounds.1 {
            Some((Direction::Right, (raw - g.bounds.1) / step))
        } else {
            None
        }
    }

    pub fn viewport_gesture_available(&self, layout: LayoutId) -> bool {
        self.layouts.get(layout).is_some_and(|state| {
            state.geometry.is_some()
                && state
                    .arrangement
                    .selected(&self.inactive)
                    .is_some_and(|wid| !state.arrangement.fullscreen.contains_key(&wid))
        })
    }

    pub fn begin_viewport_gesture(&mut self, layout: LayoutId, now: Instant) -> bool {
        if !self.viewport_gesture_available(layout) {
            return false;
        }
        let state = &mut self.layouts[layout];
        state.camera.transient_restore = None;
        if matches!(state.camera.viewport, Viewport::Gesture(_)) {
            return false;
        }
        state.camera.motion.overscroll = 0.0;
        state.camera.motion.samples.clear();
        state.camera.motion.samples.reserve(64);
        state.camera.viewport.sample(now, 1.0);
        state.camera.viewport = Viewport::Gesture(state.camera.viewport.offset());
        true
    }

    /// Pixel deltas, independent of any native input source; focus stays fixed.
    pub fn update_viewport_gesture(
        &mut self,
        layout: LayoutId,
        delta: f64,
        timestamp: Duration,
    ) -> Option<f64> {
        if !delta.is_finite() {
            return None;
        }
        let state = self.layouts.get_mut(layout)?;
        let Viewport::Gesture(offset) = &mut state.camera.viewport else {
            return None;
        };
        let g = state.geometry.as_ref()?;
        state.camera.motion.update(offset, g.bounds, delta, timestamp)
    }

    /// Normalize only at the layout boundary, using cached working geometry.
    pub fn update_viewport_gesture_normalized(
        &mut self,
        layout: LayoutId,
        delta: f64,
        timestamp: Duration,
    ) -> Option<f64> {
        let width = self.layouts.get(layout)?.geometry.as_ref()?.tiling.size.width;
        self.update_viewport_gesture(layout, delta * width, timestamp)
    }

    /// Release projection is niri's exponential touchpad deceleration, in camera pixels.
    pub fn end_viewport_gesture(
        &mut self,
        layout: LayoutId,
        timestamp: Duration,
        animate: bool,
    ) -> Option<ViewportRelease> {
        let state = self.layouts.get_mut(layout)?;
        let Viewport::Gesture(offset) = state.camera.viewport else {
            return None;
        };
        if !state.camera.motion.push(0.0, timestamp) {
            return None;
        }
        let velocity = state.camera.motion.velocity();
        let projected = offset - velocity / (1000.0 * 0.997_f64.ln());
        let release = self.settle(layout, projected, velocity, true)?;
        if animate {
            self.layouts[layout].camera.viewport = Viewport::Animation(CameraSpring::new(
                release.from_offset,
                release.offset,
                release.velocity,
                Instant::now(),
            ));
        }
        Some(release)
    }

    /// Settle where cancellation occurred, with no artificial fling or focus change.
    pub fn cancel_viewport_gesture(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.get_mut(layout)
            && matches!(
                state.camera.viewport,
                Viewport::Gesture(_) | Viewport::Animation(_)
            )
        {
            let offset = state.camera.viewport.offset();
            let offset =
                state.geometry.as_ref().map_or(offset, |g| offset.clamp(g.bounds.0, g.bounds.1));
            state.camera.viewport = Viewport::Static(offset);
            state.camera.motion = MotionHistory::default();
        }
    }

    /// Signed travel beyond resting bounds, in working-area widths.
    pub fn gesture_overscroll(&self, layout: LayoutId) -> f64 {
        self.layouts
            .get(layout)
            .and_then(|state| {
                let g = state.geometry.as_ref()?;
                Some(state.camera.motion.overscroll / g.tiling.size.width)
            })
            .unwrap_or(0.0)
    }

    fn settle(
        &mut self,
        layout: LayoutId,
        projected: f64,
        velocity: f64,
        gesture_release: bool,
    ) -> Option<ViewportRelease> {
        let state = self.layouts.get_mut(layout)?;
        let g = state.geometry.as_ref()?;
        let from_offset = state.camera.viewport.offset();
        let niri = self.settings.focus_navigation_style == ScrollingFocusNavigationStyle::Niri;
        let snap = g.snaps.iter().min_by(|a, b| {
            (a.offset - projected)
                .abs()
                .total_cmp(&(b.offset - projected).abs())
                .then_with(|| {
                    if niri {
                        // Niri sorts snaps by position before nearest-point selection.
                        a.offset.total_cmp(&b.offset)
                    } else {
                        (gesture_release
                            && g.columns[a.column].id
                                != state.arrangement.columns[state.arrangement.active_column].id)
                            .cmp(
                                &(gesture_release
                                    && g.columns[b.column].id
                                        != state.arrangement.columns
                                            [state.arrangement.active_column]
                                            .id),
                            )
                    }
                })
        })?;
        let mut index = snap.column;
        if niri {
            // Choose the furthest fully visible column
            // in the direction of travel.
            index = if projected >= from_offset {
                g.columns
                    .iter()
                    .enumerate()
                    .skip(index + 1)
                    .take_while(|(_, column)| {
                        column.world_x + column.width <= snap.offset + g.tiling.size.width
                    })
                    .last()
                    .map_or(index, |(next, _)| next)
            } else {
                (0..index)
                    .rev()
                    .take_while(|&next| g.columns[next].world_x >= snap.offset)
                    .last()
                    .unwrap_or(index)
            };
        }
        let offset = snap.offset;
        let canonical_index =
            state.arrangement.columns.iter().position(|c| c.id == g.columns[index].id)?;
        if canonical_index != state.arrangement.active_column {
            state.camera.fullscreen_restore = None;
        }
        state.arrangement.active_column = canonical_index;
        state.camera.transient_restore = None;
        state.camera.viewport.set_target(offset);
        Some(ViewportRelease {
            window: state.arrangement.selected(&self.inactive)?,
            offset,
            from_offset,
            velocity,
        })
    }

    pub fn snap_to_nearest_column(&mut self, layout: LayoutId) -> Option<WindowId> {
        let offset = self.layouts.get(layout)?.camera.viewport.offset();
        self.settle(layout, offset, 0.0, false).map(|release| release.window)
    }

    pub fn center_selected_column(&mut self, layout: LayoutId) {
        let Some(state) = self.layouts.get_mut(layout) else {
            return;
        };
        if let Some(g) = &state.geometry
            && let Some(column) = state
                .arrangement
                .columns
                .get(state.arrangement.active_column)
                .and_then(|c| g.column(c.id))
        {
            state.camera.viewport.set_target(centered_offset(g.tiling.size.width, column));
        }
    }

    pub fn switch_preset_column_width(&mut self, layout: LayoutId, backwards: bool) -> bool {
        let Some((state, col, _)) = Self::selected_mut(&mut self.layouts, layout, &self.inactive)
        else {
            return false;
        };
        let presets = self
            .settings
            .preset_column_widths
            .iter()
            .copied()
            .filter(|r| r.is_finite() && *r > 0.0);
        let Some(first) = presets.clone().next() else {
            return false;
        };
        // The requested proportion remembers the preset even when geometry is constrained.
        let known = match state.arrangement.columns[col].width {
            ColumnWidth::Proportion(ratio) => {
                presets.clone().position(|preset| (ratio - preset).abs() <= 1e-6)
            }
            _ => None,
        };
        let target = if let Some(index) = known {
            let len = presets.clone().count();
            let next = (index + if backwards { len - 1 } else { 1 }) % len;
            presets.clone().nth(next).unwrap()
        } else {
            let Some(g) = &state.geometry else {
                return false;
            };
            let Some(column) = g.column(state.arrangement.columns[col].id) else {
                return false;
            };
            let resolved = |ratio| {
                proportional_width(
                    g.tiling.size.width,
                    g.gaps.inner.horizontal,
                    clamp_ratio(ratio, &self.settings),
                )
            };
            // One logical pixel of allowance, separate from semantic ratio matching.
            if backwards {
                presets
                    .clone()
                    .rev()
                    .find(|&r| resolved(r) + 1.0 < column.width)
                    .unwrap_or_else(|| presets.clone().next_back().unwrap())
            } else {
                presets.clone().find(|&r| column.width + 1.0 < resolved(r)).unwrap_or(first)
            }
        };
        if matches!(state.arrangement.columns[col].width, ColumnWidth::Proportion(r) if r == target)
        {
            return false;
        }
        state.mutate(&self.settings, &self.inactive, |state| {
            state.arrangement.columns[col].width = ColumnWidth::Proportion(target);
        });
        state.reveal(&self.settings);
        true
    }

    fn toggle_fullscreen(&mut self, layout: LayoutId, within_gaps: bool) -> Vec<WindowId> {
        let Some(state) = self.layouts.get_mut(layout) else {
            return Vec::new();
        };
        let Some(wid) = state.arrangement.selected(&self.inactive) else {
            return Vec::new();
        };
        let mode = if within_gaps {
            FullscreenMode::WithinGaps
        } else {
            FullscreenMode::Screen
        };
        if state.arrangement.fullscreen.remove(&wid) == Some(mode) {
            state.restore_fullscreen_view();
        } else {
            if state.camera.fullscreen_restore.is_none() {
                state.camera.fullscreen_restore = state.bookmark();
            }
            state.arrangement.fullscreen.insert(wid, mode);
            if let Some(column) = state.geometry.as_ref().and_then(|g| {
                state
                    .arrangement
                    .columns
                    .get(state.arrangement.active_column)
                    .and_then(|c| g.column(c.id))
            }) {
                state.camera.viewport.set_target(column.world_x);
            }
        }
        vec![wid]
    }
}

impl LayoutSystem for ScrollingLayoutSystem {
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.layouts.get(layout).is_some_and(|state| {
            state
                .arrangement
                .columns
                .iter()
                .any(|column| column.windows().any(|window| window.pid == pid))
        })
    }

    fn window_participates(&self, window: WindowId) -> bool { !self.inactive.contains(&window) }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        let changed = super::update_participation(&mut self.inactive, window, participates);
        if !changed {
            return false;
        }
        // Before the first projection, discovery order must not replace the saved selection.
        for state in self.layouts.values_mut().filter(|state| {
            (!participates || state.geometry.is_some())
                && state.arrangement.locate(window).is_some()
        }) {
            let previous_column = state.arrangement.active_column;
            state.mutate(&self.settings, &self.inactive, |state| {
                if state.camera.restored_view.is_none()
                    && state.arrangement.selected(&self.inactive).is_none()
                {
                    state.arrangement.select_available_column(
                        state.camera.transient_restore.as_ref().map(|bookmark| bookmark.column),
                        &self.inactive,
                    );
                }
            });
            if state.arrangement.active_column != previous_column {
                let selected_column = state.arrangement.columns[state.arrangement.active_column].id;
                let restored = state
                    .camera
                    .transient_restore
                    .take()
                    .filter(|bookmark| bookmark.column == selected_column)
                    .is_some_and(|bookmark| state.restore(bookmark));
                if !restored {
                    state.reveal(&self.settings);
                }
            }
        }
        true
    }

    fn create_layout(&mut self) -> LayoutId { self.layouts.insert(LayoutState::default()) }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.layouts.contains_key(layout) }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let mut state = self.layouts.get(layout).cloned().unwrap_or_default();
        if let Viewport::Pending { target, .. } = state.camera.viewport {
            state.camera.viewport = Viewport::Static(target);
        } else if matches!(state.camera.viewport, Viewport::Animation(_)) {
            let offset = state.camera.viewport.offset();
            state.camera.viewport = Viewport::Static(
                state.geometry.as_ref().map_or(offset, |g| offset.clamp(g.bounds.0, g.bounds.1)),
            );
        }
        self.layouts.insert(state)
    }

    fn remove_layout(&mut self, layout: LayoutId) { self.layouts.remove(layout); }

    fn prepare_layout(
        &mut self,
        layout: LayoutId,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
    ) {
        self.prepare(layout, screen, constraints, None, gaps);
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
        _stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        self.calculate_frames(layout, screen, constraints, None, gaps, true)
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.layouts.get(layout)?.arrangement.selected(&self.inactive)
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        let state = self.layouts.get(layout)?;
        let (col, row) = state.arrangement.selected_location(&self.inactive)?;
        let selected = |column: &Column| {
            column
                .selected()
                .filter(|w| !self.inactive.contains(w))
                .or_else(|| column.windows().copied().find(|w| !self.inactive.contains(w)))
        };
        match direction {
            Direction::Left => state.arrangement.columns[..col].iter().rev().find_map(selected),
            Direction::Right => state.arrangement.columns[col + 1..].iter().find_map(selected),
            Direction::Up => state.arrangement.columns[col].rows[..row]
                .iter()
                .map(|row| &row.window)
                .rev()
                .copied()
                .find(|w| !self.inactive.contains(w)),
            Direction::Down => state.arrangement.columns[col].rows[row + 1..]
                .iter()
                .map(|row| &row.window)
                .copied()
                .find(|w| !self.inactive.contains(w)),
        }
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.layouts
            .get(layout)
            .is_some_and(|state| state.arrangement.locate(wid).is_some())
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.layouts
            .get(layout)
            .is_some_and(|s| s.arrangement.fullscreen.keys().any(|w| !self.inactive.contains(w)))
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.layouts
            .get(layout)
            .and_then(|s| s.arrangement.columns.get(s.arrangement.active_column))
            .is_some_and(|c| c.rows.len() > 1)
    }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        match query {
            WindowQuery::All | WindowQuery::Visible => self
                .layouts
                .get(layout)
                .into_iter()
                .flat_map(|state| state.arrangement.windows())
                .filter(|&window| {
                    matches!(query, WindowQuery::All) || self.window_participates(window)
                })
                .collect(),
            WindowQuery::Selection => self.visible_windows_under_selection(layout),
            WindowQuery::Stack(_) => Vec::new(),
        }
    }

    fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        match focus {
            Focus::Select(window) => FocusResult::selection(self.select_window(layout, window)),
            Focus::Ascend => {
                FocusResult::selection({ self.move_focus(layout, Direction::Up).0.is_some() })
            }
            Focus::Descend => {
                FocusResult::selection({ self.move_focus(layout, Direction::Down).0.is_some() })
            }
            Focus::Direction(direction) => {
                FocusResult::movement(self.move_focus(layout, direction))
            }
        }
    }

    fn update(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Add(layout, window, _) => self.insert(layout, window),
            WindowChange::Remove(window) | WindowChange::RemoveAndRebalance(window) => {
                self.inactive.remove(&window);
                for state in self.layouts.values_mut() {
                    Self::remove_from_state(state, &self.settings, window, &self.inactive);
                }
            }
            WindowChange::Replace(from, to) => {
                super::replace_participation(&mut self.inactive, from, to);
                for state in self
                    .layouts
                    .values_mut()
                    .filter(|state| state.arrangement.locate(from).is_some())
                {
                    state.mutate(&self.settings, &self.inactive, |state| {
                        for column in &mut state.arrangement.columns {
                            for window in column.rows.iter_mut().map(|row| &mut row.window) {
                                if *window == from {
                                    *window = to;
                                }
                            }
                        }
                        if let Some(mode) = state.arrangement.fullscreen.remove(&from) {
                            state.arrangement.fullscreen.insert(to, mode);
                        }
                    });
                }
            }
            WindowChange::CloseApp(pid) => {
                self.inactive.retain(|w| w.pid != pid);
                for state in self.layouts.values_mut() {
                    let closing: Vec<_> =
                        state.arrangement.windows().filter(|w| w.pid == pid).collect();
                    for wid in closing {
                        Self::remove_from_state(state, &self.settings, wid, &self.inactive);
                    }
                }
            }
            WindowChange::Resize {
                layout,
                window,
                old,
                new,
                screen,
                gaps,
            } => {
                let wid = window;
                let old_frame = old;
                let new_frame = new;

                if !self.window_participates(wid) {
                    return;
                }
                let Some(state) = self.layouts.get_mut(layout) else {
                    return;
                };
                let Some((col, row)) = state.arrangement.locate(wid) else {
                    return;
                };
                let width_changed = (new_frame.size.width - old_frame.size.width).abs() > 0.5;
                let height_changed = (new_frame.size.height - old_frame.size.height).abs() > 0.5;
                if !width_changed && !height_changed {
                    return;
                }
                let tiling = compute_tiling_area(screen, gaps);
                state.mutate(&self.settings, &self.inactive, |state| {
                    // Display moves can resize only the height. Keep the requested width
                    // (including preserved pixel widths) unless the width actually changed.
                    if width_changed {
                        state.arrangement.columns[col].width =
                            ColumnWidth::Proportion(clamp_ratio(
                                width_ratio(
                                    tiling.size.width,
                                    gaps.inner.horizontal,
                                    new_frame.size.width,
                                ),
                                &self.settings,
                            ));
                    }
                    let column = &mut state.arrangement.columns[col];
                    if height_changed && column.rows.len() > 1 {
                        let total = (tiling.size.height
                            - gaps.inner.vertical * column.rows.len().saturating_sub(1) as f64)
                            .max(1.0);
                        set_height_share(
                            column,
                            row,
                            (new_frame.size.height / total).clamp(0.05, 0.95),
                            total,
                        );
                    }
                });
            }
        }
    }

    fn edit_layout(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult {
        match edit {
            LayoutEdit::Move {
                direction,
                has_display_neighbor: _,
            } => EditResult::from(self.move_selection(layout, direction)),
            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::Swap(a, b) => EditResult::from(self.swap_windows(layout, a, b)),
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(self.apply_target_drop(layout, source, target, action))
            }
            LayoutEdit::Group(GroupEdit::Join(direction)) => {
                EditResult::from(self.join_selection_with_direction(layout, direction))
            }
            LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)) => {
                if !matches!(direction, Direction::Left | Direction::Right) {
                    return EditResult::Unchanged;
                }
                EditResult::from(if self.parent_of_selection_is_stacked(layout) {
                    self.move_selection(layout, direction)
                } else {
                    self.join_selection_with_direction(layout, direction)
                })
            }
            LayoutEdit::Group(GroupEdit::Stack(orientation)) => EditResult::Raise({
                let _orientation = orientation;

                let Some((state, col, _)) =
                    Self::selected_mut(&mut self.layouts, layout, &self.inactive)
                else {
                    return EditResult::Raise(Vec::new());
                };
                let target = if col + 1 < state.arrangement.columns.len() {
                    col + 1
                } else if col > 0 {
                    col - 1
                } else {
                    return EditResult::Raise(Vec::new());
                };
                let moved =
                    state.arrangement.columns[target].windows().copied().collect::<Vec<_>>();
                let selected = state.arrangement.selected(&self.inactive).unwrap();
                state.camera.transient_restore = None;
                state.mutate(&self.settings, &self.inactive, |state| {
                    let mut neighbor = state.arrangement.columns.remove(target);
                    let column = &mut state.arrangement.columns[col - usize::from(target < col)];
                    column.rows.append(&mut neighbor.rows);
                    state.arrangement.activate(selected);
                });
                moved
            }),
            LayoutEdit::Group(GroupEdit::Unstack(orientation)) => EditResult::Raise({
                let _orientation = orientation;

                let Some((state, col, _)) =
                    Self::selected_mut(&mut self.layouts, layout, &self.inactive)
                else {
                    return EditResult::Raise(Vec::new());
                };
                let selected = state.arrangement.selected(&self.inactive).unwrap();
                let moved: Vec<_> = state.arrangement.columns[col]
                    .windows()
                    .copied()
                    .filter(|w| *w != selected)
                    .collect();
                state.camera.transient_restore = None;
                state.mutate(&self.settings, &self.inactive, |state| {
                    for (index, &wid) in moved.iter().enumerate() {
                        let (width, weight) =
                            state.arrangement.detach(wid, &self.inactive).unwrap();
                        state.arrangement.new_column(col + 1 + index, wid, width, weight);
                    }
                    state.arrangement.activate(selected);
                });
                moved
            }),
            LayoutEdit::Group(GroupEdit::Unjoin) => EditResult::from(
                self.parent_of_selection_is_stacked(layout)
                    && self.move_selection(layout, Direction::Right),
            ),
            LayoutEdit::Resize { amount, orientation } => {
                if !amount.is_finite() {
                    return EditResult::Unchanged;
                }
                let Some((state, col, row)) =
                    Self::selected_mut(&mut self.layouts, layout, &self.inactive)
                else {
                    return EditResult::Unchanged;
                };
                let vertical = orientation == ResizeOrientation::Vertical
                    || (orientation == ResizeOrientation::Smart
                        && state.arrangement.columns[col].rows.len() > 1);
                state.mutate(&self.settings, &self.inactive, |state| {
                    let column = &mut state.arrangement.columns[col];
                    if vertical {
                        if column.rows.len() > 1 {
                            let total: f64 = column.rows.iter().map(|row| row.height).sum();
                            let share =
                                (column.rows[row].height / total + amount).clamp(0.05, 0.95);
                            set_height_share(column, row, share, total.max(10_000.0));
                        }
                    } else {
                        let ratio = state
                            .geometry
                            .as_ref()
                            .and_then(|g| {
                                g.column(column.id).map(|c| {
                                    width_ratio(
                                        g.tiling.size.width,
                                        g.gaps.inner.horizontal,
                                        c.width,
                                    )
                                })
                            })
                            .unwrap_or(match column.width {
                                ColumnWidth::Proportion(r) => r,
                                _ => self.settings.column_width_ratio,
                            });
                        column.width =
                            ColumnWidth::Proportion(clamp_ratio(ratio + amount, &self.settings));
                    }
                });
                if !vertical {
                    state.reveal(&self.settings);
                };
                EditResult::Changed
            }
            _ => EditResult::Unsupported,
        }
    }
}

impl ScrollingLayoutSystem {
    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layouts
            .get(layout)
            .and_then(|s| s.arrangement.columns.get(s.arrangement.active_column))
            .map(|c| c.windows().copied().filter(|w| self.window_participates(*w)).collect())
            .unwrap_or_default()
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let Some(wid) = self.window_in_direction(layout, direction) else {
            return (None, Vec::new());
        };
        self.select_window(layout, wid);
        (Some(wid), self.visible_windows_under_selection(layout))
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        if !self.window_participates(wid) {
            return false;
        }
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        if state.arrangement.selected(&self.inactive) == Some(wid) {
            return true;
        }
        if !state.arrangement.activate(wid) {
            return false;
        }
        state.camera.restored_view = None;
        state.camera.transient_restore = None;
        if state.camera.fullscreen_restore.as_ref().is_some_and(|b| {
            b.column != state.arrangement.columns[state.arrangement.active_column].id
        }) {
            state.camera.fullscreen_restore = None;
        }
        state.reveal(&self.settings);
        true
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        if !self.window_participates(a) || !self.window_participates(b) {
            return false;
        }
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        let (Some((ac, ar)), Some((bc, br))) =
            (state.arrangement.locate(a), state.arrangement.locate(b))
        else {
            return false;
        };
        state.mutate(&self.settings, &self.inactive, |state| {
            let selected = state.arrangement.selected(&self.inactive);
            let a = state.arrangement.columns[ac].rows[ar];
            let b = state.arrangement.columns[bc].rows[br];
            state.arrangement.columns[ac].rows[ar] = b;
            state.arrangement.columns[bc].rows[br] = a;
            if let Some(wid) = selected {
                state.arrangement.activate(wid);
            }
        });
        true
    }

    fn apply_target_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: WindowDropAction,
    ) -> bool {
        if !self.window_participates(source) || !self.window_participates(target) {
            return false;
        }
        if let WindowDropAction::Move(direction) = action {
            return self.select_window(layout, source) && self.move_selection(layout, direction);
        }
        if action == WindowDropAction::Swap {
            return self.swap_windows(layout, source, target);
        }
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        state.camera.transient_restore = None;
        state.mutate(&self.settings, &self.inactive, |s| {
            s.arrangement.transfer(source, target, action, &self.inactive)
        })
    }

    pub(in crate::layout_engine) fn can_move_window(
        &self,
        layout: LayoutId,
        window: WindowId,
        direction: Direction,
    ) -> bool {
        self.window_participates(window)
            && self.layouts.get(layout).is_some_and(|state| {
                state.arrangement.locate(window).is_some_and(|slot| {
                    state.arrangement.move_destination(slot, direction, &self.inactive).is_some()
                })
            })
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        let Some((col, row)) = state.arrangement.selected_location(&self.inactive) else {
            return false;
        };
        let Some(destination) =
            state.arrangement.move_destination((col, row), direction, &self.inactive)
        else {
            return false;
        };
        state.camera.transient_restore = None;
        let viewport = (!matches!(destination, MoveDestination::Row(_)))
            .then(|| state.camera.viewport.clone());
        state.mutate(&self.settings, &self.inactive, |state| {
            let arrangement = &mut state.arrangement;
            match destination {
                MoveDestination::NewColumn(target) => {
                    let window = arrangement.columns[col].rows[row].window;
                    let (width, weight) = arrangement.detach(window, &self.inactive).unwrap();
                    arrangement.new_column(target, window, width, weight);
                }
                MoveDestination::Column(target) => {
                    arrangement.columns.swap(col, target);
                    arrangement.active_column = target;
                }
                MoveDestination::Row(target) => {
                    let column = &mut arrangement.columns[col];
                    column.rows.swap(row, target);
                    column.active_window = target;
                }
            }
        });
        if let Some(viewport) = viewport {
            state.camera.viewport = viewport;
            state.reconcile_camera_bounds();
            state.reveal(&self.settings);
        }
        true
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) -> bool {
        if !matches!(direction, Direction::Left | Direction::Right) {
            return false;
        }
        let Some(target) = self.window_in_direction(layout, direction) else {
            return false;
        };
        let Some(source) = self.selected_window(layout) else {
            return false;
        };
        self.apply_target_drop(layout, source, target, WindowDropAction::Stack)
    }
}

fn set_height_share(column: &mut Column, row: usize, share: f64, scale: f64) {
    let other: f64 = column
        .rows
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != row)
        .map(|(_, r)| r.height)
        .sum();
    let others = column.rows.len().saturating_sub(1).max(1) as f64;
    for (index, r) in column.rows.iter_mut().enumerate() {
        r.height = if index == row {
            share * scale
        } else if other > 0.0 {
            (1.0 - share) * scale * r.height / other
        } else {
            (1.0 - share) * scale / others
        };
    }
}

impl ScrollingLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        let Some(state) = self.layouts.get(layout) else {
            return String::new();
        };
        state
            .arrangement
            .columns
            .iter()
            .enumerate()
            .map(|(index, col)| {
                format!("Column {index}: {:?} active={}\n", col.rows, col.active_window)
            })
            .collect()
    }

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        let state = self.layouts.get(layout).expect("unknown scrolling layout");
        let selected = state.arrangement.selected(&self.inactive);
        let children = state
            .arrangement
            .columns
            .iter()
            .map(|column| {
                let windows = column
                    .rows
                    .iter()
                    .map(|&Row { window, height }| {
                        let fullscreen = state.arrangement.fullscreen.get(&window);
                        rift_protocol::ContainerTreeNode {
                            node_id: (((window.pid as u32 as u64) << 32)
                                | u64::from(window.idx.get()))
                            .rotate_left(1)
                                | 1,
                            node_type: rift_protocol::ContainerNodeType::Window,
                            weight: Some(height),
                            window_id: Some(window.into()),
                            is_selected: selected == Some(window),
                            is_fullscreen: fullscreen == Some(&FullscreenMode::Screen),
                            is_fullscreen_within_gaps: fullscreen
                                == Some(&FullscreenMode::WithinGaps),
                            ..Default::default()
                        }
                    })
                    .collect();
                rift_protocol::ContainerTreeNode {
                    node_id: column.id.0,
                    layout_kind: Some(rift_protocol::LayoutKind::Vertical),
                    role: Some("column".to_owned()),
                    children: windows,
                    ..Default::default()
                }
            })
            .collect();

        rift_protocol::ContainerTreeNode {
            layout_kind: Some(rift_protocol::LayoutKind::Horizontal),
            children,
            ..Default::default()
        }
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        let (col, row) = self.layouts.get(layout)?.arrangement.locate(window)?;
        Some(vec![col, row])
    }
}
#[cfg(test)]
pub(crate) mod tests {
    pub(crate) fn presented_frames(
        system: &super::ScrollingLayoutSystem,
        layout: crate::layout_engine::LayoutId,
    ) -> std::vec::IntoIter<(crate::actor::app::WindowId, objc2_core_foundation::CGRect)> {
        system
            .presentation(layout)
            .map(|(camera, frames)| {
                frames
                    .into_iter()
                    .map(|(wid, frame, fixed)| {
                        (wid, camera.frame_at_offset(frame, fixed, 1.0, camera.offset()))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
    }
    use super::*;

    fn wid(index: u32) -> WindowId { WindowId::new(1, index) }

    struct Fixture {
        system: ScrollingLayoutSystem,
        layout: LayoutId,
        screen: CGRect,
        gaps: GapSettings,
        constraints: HashMap<WindowId, WindowLayoutConstraints>,
    }

    impl Fixture {
        fn new(count: u32) -> Self {
            let settings = ScrollingLayoutSettings {
                column_width_ratio: 0.5,
                min_column_width_ratio: 0.1,
                alignment: ScrollingAlignment::Left,
                preserve_window_sizes: false,
                ..Default::default()
            };
            let mut system = ScrollingLayoutSystem::new(&settings);
            let layout = system.create_layout();
            for index in 1..=count {
                system.update(WindowChange::Add(layout, wid(index), None));
            }
            if count > 0 {
                system.select_window(layout, wid(1));
            }
            let mut fixture = Self {
                system,
                layout,
                screen: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0)),
                gaps: GapSettings::default(),
                constraints: HashMap::default(),
            };
            fixture.prepare();
            fixture
        }

        fn prepare(&mut self) {
            self.system
                .prepare_layout(self.layout, self.screen, &self.constraints, &self.gaps);
        }

        fn frames(&mut self) -> Vec<(WindowId, CGRect)> {
            self.prepare();
            self.system.calculate_frames(
                self.layout,
                self.screen,
                &self.constraints,
                None,
                &self.gaps,
                false,
            )
        }

        fn frame(&mut self, index: u32) -> CGRect {
            self.frames().into_iter().find(|(w, _)| *w == wid(index)).unwrap().1
        }

        fn select(&mut self, index: u32) {
            assert!(self.system.select_window(self.layout, wid(index)));
        }

        fn selected(&self) -> Option<WindowId> { self.system.selected_window(self.layout) }

        fn drop(&mut self, source: u32, target: u32, action: WindowDropAction) {
            assert!(
                self.system
                    .edit(self.layout, LayoutEdit::Drop {
                        source: wid(source),
                        target: wid(target),
                        action: action
                    })
                    .changed()
            );
        }
    }

    #[test]
    fn consume_expel_and_horizontal_extraction_preserve_identity_sizing_and_focus() {
        for direction in [Direction::Left, Direction::Right] {
            let mut f = Fixture::new(2);
            f.select(1);
            f.system
                .edit(f.layout, LayoutEdit::Resize {
                    amount: -0.1,
                    orientation: ResizeOrientation::Horizontal,
                })
                .discard();
            let destination_id = f.system.container_tree(f.layout).children[0].node_id;
            f.select(2);
            f.system
                .edit(
                    f.layout,
                    LayoutEdit::Group(GroupEdit::ConsumeOrExpel(Direction::Left)),
                )
                .discard();
            assert_eq!(f.selected(), Some(wid(2)));
            assert_eq!(f.frame(2).size.width, 400.0);
            assert_eq!(
                f.system.container_tree(f.layout).children[0].node_id,
                destination_id
            );
            f.system
                .edit(f.layout, LayoutEdit::Resize {
                    amount: 0.2,
                    orientation: ResizeOrientation::Vertical,
                })
                .discard();
            let offset = f.system.layouts[f.layout].camera.viewport.start_offset();
            f.system
                .edit(f.layout, LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)))
                .discard();
            assert_eq!(f.selected(), Some(wid(2)));
            assert_eq!(f.system.layouts[f.layout].camera.viewport.start_offset(), offset);
            assert_eq!(f.frame(2).size.width, 400.0);
            let tree = f.system.container_tree(f.layout);
            let expelled = tree
                .children
                .iter()
                .find(|c| c.children[0].window_id == Some(wid(2).into()))
                .unwrap();
            assert_ne!(expelled.node_id, destination_id);
            f.system.join_selection_with_direction(
                f.layout,
                if direction == Direction::Left {
                    Direction::Right
                } else {
                    Direction::Left
                },
            );
            assert_eq!(f.selected(), Some(wid(2)));
            assert_eq!(f.frame(2).size.height, 560.0);
            assert!(f.system.move_selection(f.layout, direction));
            assert_eq!(f.selected(), Some(wid(2)));
            assert_eq!(
                f.system.window_slot(f.layout, wid(2)),
                Some(vec![usize::from(direction == Direction::Right), 0])
            );
        }
    }

    #[test]
    fn removing_selected_column_skips_suspended_neighbor() {
        let mut f = Fixture::new(3);
        f.select(3);
        f.system.set_window_participation(wid(2), false);

        f.system.update(WindowChange::Remove(wid(3)));

        assert_eq!(f.selected(), Some(wid(1)));
        assert_eq!(f.frame(1).origin.x, f.screen.origin.x);
    }

    #[test]
    fn structural_edits_rebase_an_ongoing_gesture_without_changing_focus() {
        let mut f = Fixture::new(4);
        f.select(3);
        f.system.begin_viewport_gesture(f.layout, Instant::now());
        f.system.update_viewport_gesture(f.layout, 30.0, Duration::from_millis(10));
        let x = f.frame(3).origin.x;
        f.system.update(WindowChange::Remove(wid(1)));
        assert_eq!(f.frame(3).origin.x, x);
        assert_eq!(f.selected(), Some(wid(3)));
        f.system.update_viewport_gesture(f.layout, 20.0, Duration::from_millis(20));
        assert_eq!(f.frame(3).origin.x, x - 20.0);
        assert_eq!(f.selected(), Some(wid(3)));
        let release = f
            .system
            .end_viewport_gesture(f.layout, Duration::from_millis(220), false)
            .unwrap();
        assert_eq!(release.from_offset, 50.0);
        assert_eq!(release.offset, 0.0);
        assert_eq!(release.window, wid(3));
        assert_eq!(f.frame(3).origin.x, 500.0);
    }

    #[test]
    fn persistence_and_clone_preserve_column_state() {
        let mut f = Fixture::new(3);
        f.drop(2, 1, WindowDropAction::Stack);
        f.select(2);
        f.system.center_selected_column(f.layout);
        let tree = f.system.container_tree(f.layout);
        let frames = f.frames();
        let clone = f.system.clone_layout(f.layout);
        assert_eq!(f.system.container_tree(clone), tree);
        assert_eq!(presented_frames(&f.system, clone).collect::<Vec<_>>(), frames);
        let text = ron::to_string(&f.system).unwrap();
        let mut restored: ScrollingLayoutSystem = ron::from_str(&text).unwrap();
        restored.update_settings(&f.system.settings);
        restored.prepare_layout(f.layout, f.screen, &f.constraints, &f.gaps);
        assert_eq!(restored.container_tree(f.layout), tree);
        assert_eq!(presented_frames(&restored, f.layout).collect::<Vec<_>>(), frames);
        f.system.select_window(clone, wid(3));
        f.system.center_selected_column(clone);
        let old_x = f
            .system
            .calculate_frames(clone, f.screen, &f.constraints, None, &f.gaps, false)
            .into_iter()
            .find(|(w, _)| *w == wid(3))
            .unwrap()
            .1
            .origin
            .x;
        let mut resized_restore: ScrollingLayoutSystem =
            ron::from_str(&ron::to_string(&f.system).unwrap()).unwrap();
        resized_restore.update_settings(&f.system.settings);
        let wider = CGRect::new(CGPoint::ZERO, CGSize::new(1600.0, 800.0));
        resized_restore.prepare_layout(clone, wider, &f.constraints, &f.gaps);
        let new_x = resized_restore
            .calculate_frames(clone, wider, &f.constraints, None, &f.gaps, false)
            .into_iter()
            .find(|(w, _)| *w == wid(3))
            .unwrap()
            .1
            .origin
            .x;
        assert_eq!(new_x, old_x);

        f.system.update(WindowChange::Add(f.layout, wid(4), None));
        let ids: Vec<_> =
            f.system.container_tree(f.layout).children.iter().map(|c| c.node_id).collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn old_scrolling_snapshots_migrate_identity_sizing_and_selected_row() {
        let first = ron::to_string(&wid(1)).unwrap();
        let second = ron::to_string(&wid(2)).unwrap();
        let text = format!(
            "(columns:[(node_id:42,windows:[{first},{second}],width_offset:0.1,width_overridden:true,height_weights:[1.0,1.0])],selected:Some({second}),column_width_ratio:0.5,fullscreen:[],fullscreen_within_gaps:[])"
        );
        let state: LayoutState = ron::from_str(&text).unwrap();
        let mut system = ScrollingLayoutSystem::default();
        let layout = system.layouts.insert(state);
        let screen = CGRect::new(CGPoint::ZERO, CGSize::new(1000.0, 800.0));
        system.prepare_layout(layout, screen, &HashMap::default(), &GapSettings::default());
        assert_eq!(system.selected_window(layout), Some(wid(2)));
        assert_eq!(system.container_tree(layout).children[0].node_id, 42);
        assert_eq!(
            presented_frames(&system, layout).next().unwrap().1.size.width,
            600.0
        );
        system.update(WindowChange::Add(layout, wid(3), None));
        assert_ne!(system.container_tree(layout).children[1].node_id, 42);
    }
}
