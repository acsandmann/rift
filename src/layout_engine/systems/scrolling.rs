//! A strip of persistent columns in world coordinates, translated by one camera.
//! View targets follow niri's fit/center and gesture snap semantics. Ordinary layout
//! animation remains in the reactor; touchpad release belongs to the camera.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{
    GapSettings, ScrollingAlignment, ScrollingFocusNavigationStyle, ScrollingLayoutSettings,
    WindowInsertionPoint,
};
use crate::layout_engine::systems::constraints::{AxisConstraints, solve_axis_lengths};
use crate::layout_engine::systems::{
    LayoutSystem, WindowLayoutConstraints, reconcile_app_membership,
};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, ResizeOrientation, WindowDropAction};
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

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Column {
    id: ColumnId,
    windows: Vec<WindowId>,
    active_window: usize,
    width: ColumnWidth,
    height_weights: Vec<f64>,
}

impl Column {
    fn resolved_width(
        &self,
        view: f64,
        gap: f64,
        settings: &ScrollingLayoutSettings,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
    ) -> f64 {
        let ratio = match self.width {
            ColumnWidth::Fixed(width) => return width.max(1.0),
            ColumnWidth::Proportion(ratio) => ratio,
            ColumnWidth::Default => self
                .windows
                .first()
                .and_then(|wid| constraints.get(wid))
                .map(|c| c.locked_width)
                .filter(|w| settings.preserve_window_sizes && w.is_finite() && *w > 0.0)
                .map_or(settings.column_width_ratio, |w| width_ratio(view, gap, w)),
        };
        proportional_width(view, gap, clamp_ratio(ratio, settings))
    }

    fn selected(&self) -> Option<WindowId> { self.windows.get(self.active_window).copied() }
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
    Gesture(f64),
    Animation(CameraSpring),
}

impl Viewport {
    fn offset(&self) -> f64 {
        match self {
            Self::Uninitialized => 0.0,
            Self::Static(offset) | Self::Gesture(offset) => *offset,
            Self::Animation(spring) => spring.current,
        }
    }

    fn rebase(&mut self, delta: f64) {
        match self {
            Self::Uninitialized => {}
            Self::Static(offset) | Self::Gesture(offset) => *offset += delta,
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
struct CameraSpring {
    from: f64,
    target: f64,
    velocity: f64,
    current: f64,
    started: Instant,
}
impl CameraSpring {
    fn sample(&mut self, now: Instant) -> bool {
        let t = now.saturating_duration_since(self.started).as_secs_f64();
        let omega = 800.0_f64.sqrt();
        let x0 = self.from - self.target;
        let b = omega * x0 + self.velocity;
        let envelope = (-omega * t).exp();
        self.current = self.target + envelope * (x0 + b * t);
        let velocity = envelope * (self.velocity - omega * b * t);
        // Check both position and speed so crossing the target cannot finish early.
        (self.current - self.target).abs() <= 0.0001 && velocity.abs() <= 0.0001
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
    columns: Vec<ColumnGeometry>,
    frames: Vec<(WindowId, CGRect)>,
    bounds: (f64, f64),
    snaps: Vec<SnapPoint>,
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
        screen: CGRect,
        constraints: HashMap<WindowId, WindowLayoutConstraints>,
        gaps: GapSettings,
    ) -> Self {
        let tiling = compute_tiling_area(screen, &gaps);
        let mut columns = Vec::with_capacity(state.columns.len());
        let mut frames = Vec::with_capacity(state.columns.iter().map(|c| c.windows.len()).sum());
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
        for column in &state.columns {
            let base = column.resolved_width(
                tiling.size.width,
                gaps.inner.horizontal,
                settings,
                &constraints,
            );
            let mut min: f64 = 1.0;
            let mut fixed: f64 = 0.0;
            let mut max = f64::INFINITY;
            let mut flexible = false;
            for wid in &column.windows {
                let c = constraint(wid);
                min = min.max(c.min_width);
                fixed = fixed.max(c.fixed_for_axis(true).unwrap_or(0.0));
                flexible |= c.resizable_for_axis(true);
                if c.max_width > 0.0 {
                    max = max.min(c.max_width);
                }
            }
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
            let available = (tiling.size.height
                - gaps.inner.vertical * column.windows.len().saturating_sub(1) as f64)
                .max(0.0);
            let rows: Vec<_> = column
                .windows
                .iter()
                .enumerate()
                .map(|(row, wid)| {
                    let c = constraint(wid);
                    AxisConstraints {
                        min: c.min_height,
                        fixed: c.fixed_for_axis(false),
                        max: (c.max_height > 0.0).then_some(c.max_height),
                        weight: (column.height_weights.get(row).copied().unwrap_or(1.0)
                            - c.min_height)
                            .max(0.001),
                        can_grow: c.resizable_for_axis(false),
                    }
                })
                .collect();
            let heights = solve_axis_lengths(&rows, available);
            let mut y = tiling.origin.y;
            for (row, &wid) in column.windows.iter().enumerate() {
                let height = heights[row];
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

    fn matches(
        &self,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
    ) -> bool {
        self.screen == screen && self.constraints == *constraints && self.gaps == *gaps
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

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(from = "StoredLayoutState")]
struct LayoutState {
    columns: Vec<Column>,
    active_column: usize,
    next_column_id: u64,
    #[serde(default)]
    viewport: Viewport,
    #[serde(skip)]
    geometry: Option<Geometry>,
    #[serde(skip)]
    motion: MotionHistory,
    // Only semantic restoration survives operations; no pending render-time actions.
    transient_restore: Option<ViewBookmark>,
    fullscreen_restore: Option<ViewBookmark>,
    #[serde(skip)]
    restored_view: Option<ViewBookmark>,
    fullscreen: HashSet<WindowId>,
    fullscreen_within_gaps: HashSet<WindowId>,
}

// Store the camera relative to its active column as well as its absolute
// position. On another display, the first preparation rebases it using the new
// geometry. Serialize borrowed fields so saving does not clone geometry/maps.
impl Serialize for LayoutState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut stored = serializer.serialize_struct("LayoutState", 9)?;
        stored.serialize_field("columns", &self.columns)?;
        stored.serialize_field("active_column", &self.active_column)?;
        stored.serialize_field("next_column_id", &self.next_column_id)?;
        let current = self.viewport.offset();
        let offset = if matches!(self.viewport, Viewport::Gesture(_) | Viewport::Animation(_)) {
            self.geometry
                .as_ref()
                .map_or(current, |g| current.clamp(g.bounds.0, g.bounds.1))
        } else {
            current
        };
        stored.serialize_field("viewport", &offset)?;
        let anchor = self.bookmark().or_else(|| self.restored_view.clone()).map(|mut anchor| {
            anchor.relative_offset += offset - current;
            anchor
        });
        stored.serialize_field("view_anchor", &anchor)?;
        stored.serialize_field("transient_restore", &self.transient_restore)?;
        stored.serialize_field("fullscreen_restore", &self.fullscreen_restore)?;
        stored.serialize_field("fullscreen", &self.fullscreen)?;
        stored.serialize_field("fullscreen_within_gaps", &self.fullscreen_within_gaps)?;
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
            .filter(|c| !c.windows.is_empty())
            .map(|mut c| {
                if c.id == 0 {
                    c.id = next;
                    next += 2;
                }
                c.height_weights.resize(c.windows.len(), 1.0);
                let width = if c.width_overridden || c.width_offset != 0.0 {
                    ColumnWidth::Proportion(base + c.width_offset)
                } else {
                    c.width
                };
                Column {
                    id: ColumnId(c.id),
                    active_window: c.active_window.min(c.windows.len() - 1),
                    windows: c.windows,
                    width,
                    height_weights: c.height_weights,
                }
            })
            .collect::<Vec<_>>();
        let active_column = stored.active_column.min(columns.len().saturating_sub(1));
        let mut state = Self {
            columns,
            active_column,
            next_column_id: next,
            viewport: stored.viewport,
            geometry: None,
            motion: MotionHistory::default(),
            transient_restore: stored.transient_restore,
            fullscreen_restore: stored.fullscreen_restore,
            restored_view: stored.view_anchor,
            fullscreen: stored.fullscreen,
            fullscreen_within_gaps: stored.fullscreen_within_gaps,
        };
        if let Some(window) = stored.selected {
            state.activate(window);
        }
        state
    }
}

impl LayoutState {
    fn selected(&self) -> Option<WindowId> { self.columns.get(self.active_column)?.selected() }

    fn selected_location(&self) -> Option<(usize, usize)> {
        self.columns
            .get(self.active_column)
            .map(|c| (self.active_column, c.active_window))
    }

    fn locate(&self, wid: WindowId) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(col, column)| {
            column.windows.iter().position(|&w| w == wid).map(|row| (col, row))
        })
    }

    fn all_windows(&self) -> Vec<WindowId> {
        self.columns.iter().flat_map(|c| c.windows.iter().copied()).collect()
    }

    fn activate(&mut self, wid: WindowId) -> bool {
        let Some((col, row)) = self.locate(wid) else {
            return false;
        };
        self.active_column = col;
        self.columns[col].active_window = row;
        true
    }

    fn restore_fullscreen_view(&mut self) -> bool {
        let fullscreen = self.columns.get(self.active_column).is_some_and(|column| {
            column.windows.iter().any(|wid| {
                self.fullscreen.contains(wid) || self.fullscreen_within_gaps.contains(wid)
            })
        });
        !fullscreen && self.fullscreen_restore.take().is_some_and(|bookmark| self.restore(bookmark))
    }

    fn bookmark(&self) -> Option<ViewBookmark> {
        let id = self.columns.get(self.active_column)?.id;
        let x = self.geometry.as_ref()?.column(id)?.world_x;
        Some(ViewBookmark {
            column: id,
            relative_offset: self.viewport.offset() - x,
        })
    }

    fn restore(&mut self, bookmark: ViewBookmark) -> bool {
        let Some(index) = self.columns.iter().position(|c| c.id == bookmark.column) else {
            return false;
        };
        self.active_column = index;
        if let Some(g) = &self.geometry
            && let Some(column) = g.column(bookmark.column)
        {
            let view = g.tiling.size.width;
            self.viewport = Viewport::Static(fit_offset(
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
        let Some(&column) = g.columns.get(self.active_column) else {
            return;
        };
        let offset = if settings.focus_navigation_style == ScrollingFocusNavigationStyle::Anchored {
            g.anchor_offset(self.active_column, settings.alignment)
        } else {
            fit_offset(self.viewport.offset(), g.tiling.size.width, column)
        };
        self.viewport = Viewport::Static(offset);
    }

    /// All topology/sizing edits use this transaction. With screen_x = world_x -
    /// camera, the camera changes by NEW world_x - OLD world_x. Rebase the free
    /// gesture as well, without losing its recent velocity samples.
    fn mutate<R>(
        &mut self,
        settings: &ScrollingLayoutSettings,
        edit: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let interrupted = matches!(self.viewport, Viewport::Animation(_));
        if interrupted {
            self.viewport = Viewport::Static(self.viewport.offset());
        }
        let window = self.selected();
        let id = self.columns.get(self.active_column).map(|c| c.id);
        let old_x = id.and_then(|id| self.geometry.as_ref()?.column(id)).map(|c| c.world_x);
        let result = edit(self);
        if let Some(old) = self.geometry.take() {
            self.geometry = Some(Geometry::build(
                self,
                settings,
                old.screen,
                old.constraints,
                old.gaps,
            ));
            let anchor = window
                .and_then(|wid| self.locate(wid))
                .map(|(col, _)| self.columns[col].id)
                .or_else(|| id.filter(|id| self.columns.iter().any(|c| c.id == *id)));
            if let (Some(old_x), Some(new)) =
                (old_x, anchor.and_then(|id| self.geometry.as_ref()?.column(id)))
            {
                self.viewport.rebase(new.world_x - old_x);
            }
        }
        if interrupted
            && let (Viewport::Static(offset), Some(g)) = (&mut self.viewport, &self.geometry)
        {
            *offset = offset.clamp(g.bounds.0, g.bounds.1);
        }
        if self.columns.is_empty() {
            self.viewport = Viewport::Static(0.0);
        }
        result
    }

    fn new_column(&mut self, index: usize, wid: WindowId, width: ColumnWidth, weight: f64) {
        self.next_column_id = self.next_column_id.max(2);
        let id = ColumnId(self.next_column_id);
        self.next_column_id += 2; // Even container IDs never collide with odd window IDs.
        self.columns.insert(index.min(self.columns.len()), Column {
            id,
            windows: vec![wid],
            active_window: 0,
            width,
            height_weights: vec![weight],
        });
        self.activate(wid);
    }

    /// Detach without destroying sizing/fullscreen metadata; shared by every transfer.
    fn detach(&mut self, wid: WindowId) -> Option<(ColumnWidth, f64)> {
        let (col, row) = self.locate(wid)?;
        let selected = self.selected();
        let column = &mut self.columns[col];
        let width = column.width;
        column.windows.remove(row);
        let weight = column.height_weights.remove(row);
        if row < column.active_window {
            column.active_window -= 1;
        }
        column.active_window = column.active_window.min(column.windows.len().saturating_sub(1));
        if column.windows.is_empty() {
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

    fn transfer(&mut self, source: WindowId, target: WindowId, action: WindowDropAction) -> bool {
        if source == target || self.locate(source).is_none() || self.locate(target).is_none() {
            return false;
        }
        let (width, weight) = self.detach(source).unwrap();
        let (col, row) = self.locate(target).unwrap();
        match action {
            WindowDropAction::Stack | WindowDropAction::Insert(Direction::Up | Direction::Down) => {
                let row = row + usize::from(action != WindowDropAction::Insert(Direction::Up));
                let column = &mut self.columns[col];
                if row <= column.active_window {
                    column.active_window += 1;
                }
                column.windows.insert(row, source);
                column.height_weights.insert(row, weight);
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

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ScrollingLayoutSystem {
    layouts: slotmap::SlotMap<LayoutId, LayoutState>,
    #[serde(skip)]
    pub(super) settings: ScrollingLayoutSettings,
}

impl ScrollingLayoutSystem {
    fn selected_mut(
        layouts: &mut slotmap::SlotMap<LayoutId, LayoutState>,
        layout: LayoutId,
    ) -> Option<(&mut LayoutState, usize, usize)> {
        let state = layouts.get_mut(layout)?;
        let (column, row) = state.selected_location()?;
        Some((state, column, row))
    }

    pub fn new(settings: &ScrollingLayoutSettings) -> Self {
        let mut system = Self::default();
        system.update_settings(settings);
        system
    }

    pub fn update_settings(&mut self, settings: &ScrollingLayoutSettings) {
        self.settings = settings.clone();
        self.settings.per_display.clear();
        for state in self.layouts.values_mut() {
            state.mutate(&self.settings, |_| {});
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
            state.mutate(&self.settings, |_| {});
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
        if state.locate(wid).is_some() {
            return;
        }
        let bookmark = state.bookmark();
        state.mutate(&self.settings, |state| {
            let index = if self.settings.base.window_insertion_point.unwrap_or_default()
                == WindowInsertionPoint::EndOfTree
            {
                state.columns.len()
            } else {
                state.selected_location().map_or(0, |(col, _)| col + 1)
            };
            state.new_column(index, wid, ColumnWidth::Default, 1.0);
        });
        state.transient_restore = bookmark;
        state.reveal(&self.settings);
    }

    fn remove_from_state(
        state: &mut LayoutState,
        settings: &ScrollingLayoutSettings,
        wid: WindowId,
    ) {
        let Some((col, _)) = state.locate(wid) else {
            return;
        };
        let active_removed = state.selected() == Some(wid);
        let column_removed = state.columns[col].windows.len() == 1;
        let restore = if active_removed && column_removed {
            state.transient_restore.take()
        } else {
            None
        };
        let was_fullscreen =
            state.fullscreen.remove(&wid) | state.fullscreen_within_gaps.remove(&wid);
        state.mutate(settings, |state| {
            state.detach(wid);
        });
        let restored = restore.is_some_and(|bookmark| state.restore(bookmark));
        let restored_fullscreen = was_fullscreen && state.restore_fullscreen_view();
        if active_removed && !restored && !restored_fullscreen {
            state.reveal(settings);
        }
        if state
            .transient_restore
            .as_ref()
            .is_some_and(|b| !state.columns.iter().any(|c| c.id == b.column))
        {
            state.transient_restore = None;
        }
    }

    pub(crate) fn calculate_frames(
        &self,
        layout: LayoutId,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
        park: bool,
    ) -> Vec<(WindowId, CGRect)> {
        let Some(state) = self.layouts.get(layout) else {
            return Vec::new();
        };
        let fallback;
        let g = match state.geometry.as_ref().filter(|g| g.matches(screen, constraints, gaps)) {
            Some(g) => g,
            None => {
                fallback = Geometry::build(
                    state,
                    &self.settings,
                    screen,
                    constraints.clone(),
                    gaps.clone(),
                );
                &fallback
            }
        };
        Self::translate_frames(state, g, park).collect()
    }

    fn translate_frames<'a>(
        state: &'a LayoutState,
        g: &'a Geometry,
        park: bool,
    ) -> impl Iterator<Item = (WindowId, CGRect)> + 'a {
        let offset = state.viewport.offset();
        g.frames.iter().map(move |&(wid, mut frame)| {
            frame.origin.x += g.tiling.origin.x - offset;
            if park {
                if frame.max().x <= g.tiling.origin.x {
                    frame.origin.x = g.screen.origin.x - frame.size.width;
                } else if frame.origin.x >= g.tiling.max().x {
                    frame.origin.x = g.screen.max().x;
                }
            }
            frame = frame.round();
            if state.fullscreen.contains(&wid) {
                frame = g.screen;
            } else if state.fullscreen_within_gaps.contains(&wid) {
                frame = g.tiling;
            }
            (wid, frame)
        })
    }

    /// Cached-geometry fast path; no constraint solving or topology rebuilding.
    pub fn viewport_frames(
        &self,
        layout: LayoutId,
    ) -> impl Iterator<Item = (WindowId, CGRect)> + '_ {
        self.layouts
            .get(layout)
            .into_iter()
            .filter_map(|state| Some((state, state.geometry.as_ref()?)))
            .flat_map(|(state, geometry)| Self::translate_frames(state, geometry, true))
    }

    /// Legacy normalized strip delta; boundary recognition is owned by the caller.
    pub fn scroll_by_delta(&mut self, layout: LayoutId, delta: f64) -> Option<(Direction, f64)> {
        if !delta.is_finite() {
            return None;
        }
        let state = self.layouts.get_mut(layout)?;
        let g = state.geometry.as_ref()?;
        let column = g.columns.get(state.active_column)?;
        let step = column.width + g.gaps.inner.horizontal;
        let raw = state.viewport.offset() + delta * step;
        state.viewport = Viewport::Static(raw.clamp(g.bounds.0, g.bounds.1));
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
                && state.selected().is_some_and(|wid| {
                    !state.fullscreen.contains(&wid) && !state.fullscreen_within_gaps.contains(&wid)
                })
        })
    }

    pub fn begin_viewport_gesture(&mut self, layout: LayoutId, now: Instant) -> bool {
        if !self.viewport_gesture_available(layout) {
            return false;
        }
        let state = &mut self.layouts[layout];
        state.transient_restore = None;
        if matches!(state.viewport, Viewport::Gesture(_)) {
            return false;
        }
        state.motion.overscroll = 0.0;
        state.motion.samples.clear();
        state.motion.samples.reserve(64);
        Self::advance_camera(state, now);
        state.viewport = Viewport::Gesture(state.viewport.offset());
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
        let Viewport::Gesture(offset) = &mut state.viewport else {
            return None;
        };
        let g = state.geometry.as_ref()?;
        let raw = *offset + delta;
        let bounded = raw.clamp(g.bounds.0, g.bounds.1);
        let moved = bounded - *offset;
        if !state.motion.push(moved, timestamp) {
            return None;
        }
        if delta != 0.0 {
            let excess = raw - bounded;
            // Keep the strip at its edge while accumulating workspace intent.
            // Reversing moves inward immediately and clears the edge intent.
            state.motion.overscroll = if excess == 0.0 {
                0.0
            } else if excess.signum() == state.motion.overscroll.signum() {
                state.motion.overscroll + excess
            } else {
                excess
            };
        }
        *offset = bounded;
        Some(moved)
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
        let Viewport::Gesture(offset) = state.viewport else {
            return None;
        };
        if !state.motion.push(0.0, timestamp) {
            return None;
        }
        let velocity = state.motion.velocity();
        let projected = offset - velocity / (1000.0 * 0.997_f64.ln());
        let release = self.settle(layout, projected, velocity, true)?;
        if animate {
            self.layouts[layout].viewport = Viewport::Animation(CameraSpring {
                from: release.from_offset,
                target: release.offset,
                velocity: release.velocity,
                current: release.from_offset,
                started: Instant::now(),
            });
        }
        Some(release)
    }

    fn advance_camera(state: &mut LayoutState, now: Instant) -> Option<bool> {
        let Viewport::Animation(spring) = &mut state.viewport else {
            return None;
        };
        let complete = spring.sample(now);
        if let Some(g) = &state.geometry {
            spring.current = spring.current.clamp(g.bounds.0, g.bounds.1);
        }
        if complete {
            state.viewport = Viewport::Static(spring.target);
            Some(false)
        } else {
            Some(true)
        }
    }

    pub fn advance_viewport_animation(&mut self, layout: LayoutId, now: Instant) -> Option<bool> {
        self.layouts.get_mut(layout).and_then(|state| Self::advance_camera(state, now))
    }

    /// Settle where cancellation occurred, with no artificial fling or focus change.
    pub fn cancel_viewport_gesture(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.get_mut(layout)
            && matches!(state.viewport, Viewport::Gesture(_) | Viewport::Animation(_))
        {
            let offset = state.viewport.offset();
            let offset =
                state.geometry.as_ref().map_or(offset, |g| offset.clamp(g.bounds.0, g.bounds.1));
            state.viewport = Viewport::Static(offset);
            state.motion = MotionHistory::default();
        }
    }

    /// Signed travel beyond resting bounds, in working-area widths.
    pub fn gesture_overscroll(&self, layout: LayoutId) -> f64 {
        self.layouts
            .get(layout)
            .and_then(|state| {
                let g = state.geometry.as_ref()?;
                Some(state.motion.overscroll / g.tiling.size.width)
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
        let from_offset = state.viewport.offset();
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
                        (gesture_release && a.column != state.active_column)
                            .cmp(&(gesture_release && b.column != state.active_column))
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
        if index != state.active_column {
            state.fullscreen_restore = None;
        }
        state.active_column = index;
        state.transient_restore = None;
        state.viewport = Viewport::Static(offset);
        Some(ViewportRelease {
            window: state.selected()?,
            offset,
            from_offset,
            velocity,
        })
    }

    pub fn snap_to_nearest_column(&mut self, layout: LayoutId) -> Option<WindowId> {
        let offset = self.layouts.get(layout)?.viewport.offset();
        self.settle(layout, offset, 0.0, false).map(|release| release.window)
    }

    pub fn center_selected_column(&mut self, layout: LayoutId) {
        let Some(state) = self.layouts.get_mut(layout) else {
            return;
        };
        if let Some(g) = &state.geometry
            && let Some(column) = g.columns.get(state.active_column)
        {
            state.viewport = Viewport::Static(centered_offset(g.tiling.size.width, *column));
        }
    }

    pub fn switch_preset_column_width(&mut self, layout: LayoutId, backwards: bool) -> bool {
        let Some((state, col, _)) = Self::selected_mut(&mut self.layouts, layout) else {
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
        let known = match state.columns[col].width {
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
            let Some(column) = g.columns.get(col) else {
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
        if matches!(state.columns[col].width, ColumnWidth::Proportion(r) if r == target) {
            return false;
        }
        state.mutate(&self.settings, |state| {
            state.columns[col].width = ColumnWidth::Proportion(target);
        });
        state.reveal(&self.settings);
        true
    }

    fn toggle_fullscreen(&mut self, layout: LayoutId, within_gaps: bool) -> Vec<WindowId> {
        let Some(state) = self.layouts.get_mut(layout) else {
            return Vec::new();
        };
        let Some(wid) = state.selected() else {
            return Vec::new();
        };
        let fullscreen = state.fullscreen.remove(&wid);
        let gaps = state.fullscreen_within_gaps.remove(&wid);
        let was_same = if within_gaps { gaps } else { fullscreen };
        if was_same {
            state.restore_fullscreen_view();
        } else {
            if state.fullscreen_restore.is_none() {
                state.fullscreen_restore = state.bookmark();
            }
            if within_gaps {
                state.fullscreen_within_gaps.insert(wid);
            } else {
                state.fullscreen.insert(wid);
            }
            if let Some(column) =
                state.geometry.as_ref().and_then(|g| g.columns.get(state.active_column))
            {
                state.viewport = Viewport::Static(column.world_x);
            }
        }
        vec![wid]
    }
}

impl LayoutSystem for ScrollingLayoutSystem {
    fn create_layout(&mut self) -> LayoutId { self.layouts.insert(LayoutState::default()) }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.layouts.contains_key(layout) }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let mut state = self.layouts.get(layout).cloned().unwrap_or_default();
        if matches!(state.viewport, Viewport::Animation(_)) {
            let offset = state.viewport.offset();
            state.viewport = Viewport::Static(
                state.geometry.as_ref().map_or(offset, |g| offset.clamp(g.bounds.0, g.bounds.1)),
            );
        }
        self.layouts.insert(state)
    }

    fn remove_layout(&mut self, layout: LayoutId) { self.layouts.remove(layout); }

    fn draw_tree(&self, layout: LayoutId) -> String {
        let Some(state) = self.layouts.get(layout) else {
            return String::new();
        };
        state
            .columns
            .iter()
            .enumerate()
            .map(|(index, col)| {
                format!(
                    "Column {index}: {:?} active={}\n",
                    col.windows, col.active_window
                )
            })
            .collect()
    }

    fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        let state = self.layouts.get(layout).expect("unknown scrolling layout");
        let children = state
            .columns
            .iter()
            .map(|column| {
                let windows = column
                    .windows
                    .iter()
                    .enumerate()
                    .map(|(index, &window)| rift_protocol::ContainerTreeNode {
                        node_id: (((window.pid as u32 as u64) << 32) | u64::from(window.idx.get()))
                            .rotate_left(1)
                            | 1,
                        node_type: rift_protocol::ContainerNodeType::Window,
                        weight: Some(column.height_weights.get(index).copied().unwrap_or(1.0)),
                        window_id: Some(window.into()),
                        is_selected: state.selected() == Some(window),
                        is_fullscreen: state.fullscreen.contains(&window),
                        is_fullscreen_within_gaps: state.fullscreen_within_gaps.contains(&window),
                        ..Default::default()
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

    /// Environmental changes are explicit mutations, never side effects of frame reads.
    fn prepare_layout(
        &mut self,
        layout: LayoutId,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
    ) {
        let Some(state) = self.layouts.get_mut(layout) else {
            return;
        };
        if state.geometry.as_ref().is_some_and(|g| g.matches(screen, constraints, gaps)) {
            return;
        }
        let interrupted = matches!(state.viewport, Viewport::Animation(_));
        if interrupted {
            state.viewport = Viewport::Static(state.viewport.offset());
        }
        let bookmark = state.restored_view.take().or_else(|| state.bookmark());
        let initial = matches!(state.viewport, Viewport::Uninitialized);
        if self.settings.preserve_window_sizes {
            let tiling = compute_tiling_area(screen, gaps);
            for column in &mut state.columns {
                if matches!(column.width, ColumnWidth::Default)
                    && column.windows.len() == 1
                    && constraints
                        .get(&column.windows[0])
                        .map(|c| c.locked_width)
                        .is_some_and(|w| w.is_finite() && w > 0.0)
                {
                    column.width = ColumnWidth::Fixed(column.resolved_width(
                        tiling.size.width,
                        gaps.inner.horizontal,
                        &self.settings,
                        constraints,
                    ));
                }
            }
        }
        state.geometry = Some(Geometry::build(
            state,
            &self.settings,
            screen,
            constraints.clone(),
            gaps.clone(),
        ));
        if let Some(bookmark) = bookmark
            && let Some(column) = state.geometry.as_ref().unwrap().column(bookmark.column)
        {
            let old = state.viewport.offset();
            state.viewport.rebase(column.world_x + bookmark.relative_offset - old);
        }
        if interrupted && let Viewport::Static(offset) = &mut state.viewport {
            let g = state.geometry.as_ref().unwrap();
            *offset = offset.clamp(g.bounds.0, g.bounds.1);
        }
        if initial {
            state.viewport = Viewport::Static(0.0);
            if state.columns.len() == 1 {
                let g = state.geometry.as_ref().unwrap();
                state.viewport = Viewport::Static(g.anchor_offset(0, self.settings.alignment));
            } else {
                state.reveal(&self.settings);
            }
        }
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
        _stack_line_thickness: f64,
        _stack_line_horiz: crate::common::config::HorizontalPlacement,
        _stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        self.calculate_frames(layout, screen, constraints, gaps, true)
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.layouts.get(layout)?.selected()
    }

    fn all_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layouts.get(layout).map(LayoutState::all_windows).unwrap_or_default()
    }

    fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        let (col, row) = self.layouts.get(layout)?.locate(window)?;
        Some(vec![col, row])
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.all_windows_in_layout(layout)
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layouts
            .get(layout)
            .and_then(|s| s.columns.get(s.active_column))
            .map(|c| c.windows.clone())
            .unwrap_or_default()
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool {
        self.move_focus(layout, Direction::Up).0.is_some()
    }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        self.move_focus(layout, Direction::Down).0.is_some()
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

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        let state = self.layouts.get(layout)?;
        let (col, row) = state.selected_location()?;
        match direction {
            Direction::Left => state.columns.get(col.checked_sub(1)?)?.selected(),
            Direction::Right => state.columns.get(col + 1)?.selected(),
            Direction::Up => state.columns[col].windows.get(row.checked_sub(1)?).copied(),
            Direction::Down => state.columns[col].windows.get(row + 1).copied(),
        }
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        self.insert(layout, wid);
    }

    fn replace_window(&mut self, from: WindowId, to: WindowId) {
        for state in self.layouts.values_mut() {
            state.mutate(&self.settings, |state| {
                for column in &mut state.columns {
                    for window in &mut column.windows {
                        if *window == from {
                            *window = to;
                        }
                    }
                }
                if state.fullscreen.remove(&from) {
                    state.fullscreen.insert(to);
                }
                if state.fullscreen_within_gaps.remove(&from) {
                    state.fullscreen_within_gaps.insert(to);
                }
            });
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        for state in self.layouts.values_mut() {
            Self::remove_from_state(state, &self.settings, wid);
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        for state in self.layouts.values_mut() {
            for wid in state.all_windows().into_iter().filter(|w| w.pid == pid) {
                Self::remove_from_state(state, &self.settings, wid);
            }
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let current = self.windows_for_app(layout, pid);
        let delta = reconcile_app_membership(pid, current, desired);
        if let Some(state) = self.layouts.get_mut(layout) {
            for wid in delta.removals {
                Self::remove_from_state(state, &self.settings, wid);
            }
        }
        for wid in delta.additions {
            self.insert(layout, wid);
        }
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.layouts.get(layout).is_some_and(|state| state.locate(wid).is_some())
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        if state.selected() == Some(wid) {
            return true;
        }
        if !state.activate(wid) {
            return false;
        }
        state.transient_restore = None;
        if state
            .fullscreen_restore
            .as_ref()
            .is_some_and(|b| b.column != state.columns[state.active_column].id)
        {
            state.fullscreen_restore = None;
        }
        state.reveal(&self.settings);
        true
    }

    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        _old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
        gaps: &GapSettings,
    ) {
        let Some(state) = self.layouts.get_mut(layout) else {
            return;
        };
        let Some((col, row)) = state.locate(wid) else {
            return;
        };
        let tiling = compute_tiling_area(screen, gaps);
        state.mutate(&self.settings, |state| {
            state.columns[col].width = ColumnWidth::Proportion(clamp_ratio(
                width_ratio(tiling.size.width, gaps.inner.horizontal, new_frame.size.width),
                &self.settings,
            ));
            let column = &mut state.columns[col];
            if column.windows.len() > 1 {
                let total = (tiling.size.height
                    - gaps.inner.vertical * column.windows.len().saturating_sub(1) as f64)
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

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        let (Some((ac, ar)), Some((bc, br))) = (state.locate(a), state.locate(b)) else {
            return false;
        };
        state.mutate(&self.settings, |state| {
            let selected = state.selected();
            let aw = state.columns[ac].height_weights[ar];
            let bw = state.columns[bc].height_weights[br];
            state.columns[ac].windows[ar] = b;
            state.columns[bc].windows[br] = a;
            state.columns[ac].height_weights[ar] = bw;
            state.columns[bc].height_weights[br] = aw;
            if let Some(wid) = selected {
                state.activate(wid);
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
        if let WindowDropAction::Move(direction) = action {
            return self.select_window(layout, source) && self.move_selection(layout, direction);
        }
        if action == WindowDropAction::Swap {
            return self.swap_windows(layout, source, target);
        }
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        state.transient_restore = None;
        state.mutate(&self.settings, |s| s.transfer(source, target, action))
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        state.transient_restore = None;
        state.mutate(&self.settings, |state| {
            let Some((col, row)) = state.selected_location() else {
                return false;
            };
            let horizontal = matches!(direction, Direction::Left | Direction::Right);
            if horizontal && state.columns[col].windows.len() > 1 {
                let wid = state.selected().unwrap();
                let (width, weight) = state.detach(wid).unwrap();
                state.new_column(
                    col + usize::from(direction == Direction::Right),
                    wid,
                    width,
                    weight,
                );
                return true;
            }
            let (index, len) = if horizontal {
                (col, state.columns.len())
            } else {
                (row, state.columns[col].windows.len())
            };
            let target = match direction {
                Direction::Left | Direction::Up => index.checked_sub(1),
                Direction::Right | Direction::Down => (index + 1 < len).then_some(index + 1),
            };
            let Some(target) = target else {
                return false;
            };
            if horizontal {
                state.columns.swap(col, target);
                state.active_column = target;
            } else {
                let column = &mut state.columns[col];
                column.windows.swap(row, target);
                column.height_weights.swap(row, target);
                column.active_window = target;
            }
            true
        })
    }

    fn move_selection_to_layout_after_selection(&mut self, from: LayoutId, to: LayoutId) {
        if from == to || !self.layouts.contains_key(to) {
            return;
        }
        let Some(wid) = self.selected_window(from) else {
            return;
        };
        let fullscreen = self.layouts[from].fullscreen.remove(&wid);
        let within_gaps = self.layouts[from].fullscreen_within_gaps.remove(&wid);
        self.layouts[from].transient_restore = None;
        let fullscreen_restore = self.layouts[from].fullscreen_restore.clone();
        let (width, weight) = self.layouts[from].mutate(&self.settings, |s| s.detach(wid)).unwrap();
        if !self.layouts[from].restore_fullscreen_view() {
            self.layouts[from].reveal(&self.settings);
        }
        self.layouts[to].mutate(&self.settings, |s| {
            let index = s.selected_location().map_or(0, |(col, _)| col + 1);
            s.new_column(index, wid, width, weight);
            if fullscreen {
                s.fullscreen.insert(wid);
            }
            if within_gaps {
                s.fullscreen_within_gaps.insert(wid);
            }
            s.fullscreen_restore =
                fullscreen_restore.filter(|_| fullscreen || within_gaps).map(|bookmark| {
                    ViewBookmark {
                        column: s.columns[s.active_column].id,
                        relative_offset: bookmark.relative_offset,
                    }
                });
        });
        self.layouts[to].reveal(&self.settings);
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.toggle_fullscreen(layout, false)
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.toggle_fullscreen(layout, true)
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.layouts
            .get(layout)
            .is_some_and(|s| !s.fullscreen.is_empty() || !s.fullscreen_within_gaps.is_empty())
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        if !matches!(direction, Direction::Left | Direction::Right) {
            return;
        }
        let Some(target) = self.window_in_direction(layout, direction) else {
            return;
        };
        let Some(source) = self.selected_window(layout) else {
            return;
        };
        self.apply_target_drop(layout, source, target, WindowDropAction::Stack);
    }

    fn consume_or_expel_selection(&mut self, layout: LayoutId, direction: Direction) {
        if !matches!(direction, Direction::Left | Direction::Right) {
            return;
        }
        if self.parent_of_selection_is_stacked(layout) {
            self.move_selection(layout, direction);
        } else {
            self.join_selection_with_direction(layout, direction);
        }
    }

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let Some((state, col, _)) = Self::selected_mut(&mut self.layouts, layout) else {
            return Vec::new();
        };
        let target = if col + 1 < state.columns.len() {
            col + 1
        } else if col > 0 {
            col - 1
        } else {
            return Vec::new();
        };
        let moved = state.columns[target].windows.clone();
        let selected = state.selected().unwrap();
        state.transient_restore = None;
        state.mutate(&self.settings, |state| {
            let mut neighbor = state.columns.remove(target);
            let column = &mut state.columns[col - usize::from(target < col)];
            column.windows.append(&mut neighbor.windows);
            column.height_weights.append(&mut neighbor.height_weights);
            state.activate(selected);
        });
        moved
    }

    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let Some((state, col, _)) = Self::selected_mut(&mut self.layouts, layout) else {
            return Vec::new();
        };
        let selected = state.selected().unwrap();
        let moved: Vec<_> =
            state.columns[col].windows.iter().copied().filter(|w| *w != selected).collect();
        state.transient_restore = None;
        state.mutate(&self.settings, |state| {
            for (index, &wid) in moved.iter().enumerate() {
                let (width, weight) = state.detach(wid).unwrap();
                state.new_column(col + 1 + index, wid, width, weight);
            }
            state.activate(selected);
        });
        moved
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.layouts
            .get(layout)
            .and_then(|s| s.columns.get(s.active_column))
            .is_some_and(|c| c.windows.len() > 1)
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        if self.parent_of_selection_is_stacked(layout) {
            self.move_selection(layout, Direction::Right);
        }
    }

    fn resize_selection_by(
        &mut self,
        layout: LayoutId,
        amount: f64,
        orientation: ResizeOrientation,
    ) {
        if !amount.is_finite() {
            return;
        }
        let Some((state, col, row)) = Self::selected_mut(&mut self.layouts, layout) else {
            return;
        };
        let vertical = orientation == ResizeOrientation::Vertical
            || (orientation == ResizeOrientation::Smart && state.columns[col].windows.len() > 1);
        state.mutate(&self.settings, |state| {
            let column = &mut state.columns[col];
            if vertical {
                if column.windows.len() > 1 {
                    let total: f64 = column.height_weights.iter().sum();
                    let share = (column.height_weights[row] / total + amount).clamp(0.05, 0.95);
                    set_height_share(column, row, share, total.max(10_000.0));
                }
            } else {
                let ratio = state
                    .geometry
                    .as_ref()
                    .and_then(|g| {
                        g.columns.get(col).map(|c| {
                            width_ratio(g.tiling.size.width, g.gaps.inner.horizontal, c.width)
                        })
                    })
                    .unwrap_or(match column.width {
                        ColumnWidth::Proportion(r) => r,
                        _ => self.settings.column_width_ratio,
                    });
                column.width = ColumnWidth::Proportion(clamp_ratio(ratio + amount, &self.settings));
            }
        });
        if !vertical {
            state.reveal(&self.settings);
        }
    }
}

fn set_height_share(column: &mut Column, row: usize, share: f64, scale: f64) {
    let other: f64 = column
        .height_weights
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != row)
        .map(|(_, w)| w)
        .sum();
    for (index, weight) in column.height_weights.iter_mut().enumerate() {
        *weight = if index == row {
            share * scale
        } else if other > 0.0 {
            (1.0 - share) * scale * *weight / other
        } else {
            (1.0 - share) * scale / column.windows.len().saturating_sub(1).max(1) as f64
        };
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::config::{ScrollingWidthOverride, StackDefaultOrientation};

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
                system.add_window_after_selection(layout, wid(index));
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

        fn focus(&mut self, direction: Direction) -> Option<WindowId> {
            self.system.move_focus(self.layout, direction).0
        }

        fn drop(&mut self, source: u32, target: u32, action: WindowDropAction) {
            assert!(self.system.apply_window_drop(self.layout, wid(source), wid(target), action));
        }
    }

    #[test]
    fn preset_width_cycles_and_effective_width_search() {
        for (width, backwards, expected) in [
            (
                ColumnWidth::Proportion(1.0 / 3.0),
                false,
                &[500.0, 667.0, 333.0][..],
            ),
            (ColumnWidth::Default, false, &[667.0][..]),
            (ColumnWidth::Proportion(0.25), false, &[333.0][..]),
            (ColumnWidth::Proportion(0.42), false, &[500.0][..]),
            (ColumnWidth::Proportion(0.60), false, &[667.0][..]),
            (ColumnWidth::Fixed(420.0), false, &[500.0][..]),
            (ColumnWidth::Fixed(800.0), false, &[333.0][..]),
            (ColumnWidth::Fixed(499.5), false, &[667.0][..]),
            (ColumnWidth::Fixed(498.9), false, &[500.0][..]),
            (
                ColumnWidth::Proportion(1.0 / 3.0),
                true,
                &[667.0, 500.0, 333.0][..],
            ),
            (ColumnWidth::Fixed(420.0), true, &[333.0][..]),
            (ColumnWidth::Fixed(200.0), true, &[667.0][..]),
        ] {
            let mut f = Fixture::new(1);
            f.system.layouts[f.layout].mutate(&f.system.settings, |s| s.columns[0].width = width);
            for &pixels in expected {
                assert!(f.system.switch_preset_column_width(f.layout, backwards));
                assert_eq!(f.frame(1).size.width, pixels);
            }
        }
        let mut f = Fixture::new(1);
        f.system.settings.preset_column_widths = vec![0.7, f64::NAN, 0.3, 0.5, 0.0];
        for pixels in [700.0, 300.0, 500.0] {
            assert!(f.system.switch_preset_column_width(f.layout, false));
            assert_eq!(f.frame(1).size.width, pixels);
        }
        f.system.settings.preset_column_widths = vec![0.33333, 0.66667];
        for pixels in [667.0, 333.0, 667.0, 333.0] {
            assert!(f.system.switch_preset_column_width(f.layout, false));
            assert_eq!(f.frame(1).size.width, pixels);
        }
        f.system.settings.preset_column_widths = vec![-1.0, f64::INFINITY];
        assert!(!f.system.switch_preset_column_width(f.layout, false));
    }

    #[test]
    fn semantic_presets_advance_despite_constrained_geometry() {
        let mut f = Fixture::new(1);
        f.constraints.insert(wid(1), WindowLayoutConstraints {
            is_resizable: true,
            max_width: 450.0,
            ..Default::default()
        });
        f.system.resize_selection_by(f.layout, -0.08, ResizeOrientation::Horizontal);
        f.prepare();
        for ratio in [0.5, 2.0 / 3.0] {
            assert!(f.system.switch_preset_column_width(f.layout, false));
            assert!(matches!(f.system.layouts[f.layout].columns[0].width,
                ColumnWidth::Proportion(r) if (r - ratio).abs() < 1e-6));
            assert_eq!(f.frame(1).size.width, 450.0);
        }
        for (start, target) in [(0.5000005, 2.0 / 3.0), (0.500002, 0.5)] {
            f.system.layouts[f.layout].mutate(&f.system.settings, |s| {
                s.columns[0].width = ColumnWidth::Proportion(start);
            });
            assert!(f.system.switch_preset_column_width(f.layout, false));
            assert!(matches!(f.system.layouts[f.layout].columns[0].width,
                ColumnWidth::Proportion(r) if (r - target).abs() < 1e-6));
        }
    }

    #[test]
    fn preset_width_preserves_stack_selection_and_fullscreen_restore() {
        let mut f = Fixture::new(2);
        f.drop(2, 1, WindowDropAction::Stack);
        f.system.resize_selection_by(f.layout, 0.1, ResizeOrientation::Vertical);
        f.gaps.inner.horizontal = 20.0;
        let before = f.frames();
        let weights = f.system.layouts[f.layout].columns[0].height_weights.clone();
        for _ in 0..3 {
            assert!(f.system.switch_preset_column_width(f.layout, false));
        }
        assert_eq!(f.frames(), before); // Same membership, row heights and gap-aware half width.
        assert_eq!(f.frame(1).size.width, 490.0);
        assert_eq!(f.selected(), Some(wid(2)));
        assert_eq!(f.system.layouts[f.layout].columns[0].height_weights, weights);
        for within_gaps in [false, true] {
            f.system.toggle_fullscreen(f.layout, within_gaps);
            assert!(f.system.switch_preset_column_width(f.layout, false));
            assert_eq!(f.frame(2), f.screen);
            f.system.toggle_fullscreen(f.layout, within_gaps);
            assert_eq!(f.frame(2).size.width, 660.0);
            assert_eq!(f.selected(), Some(wid(2)));
            for _ in 0..2 {
                assert!(f.system.switch_preset_column_width(f.layout, false));
            }
        }
        assert!(f.system.begin_viewport_gesture(f.layout, Instant::now()));
        assert!(f.system.switch_preset_column_width(f.layout, false));
        assert_eq!(f.selected(), Some(wid(2)));
        assert_eq!(
            f.system.update_viewport_gesture(f.layout, 10.0, Duration::ZERO),
            None
        );
    }

    #[test]
    fn half_width_single_column_and_visible_focus_do_not_expand_or_shift() {
        let mut f = Fixture::new(1);
        assert_eq!(f.frame(1).size.width, 500.0);
        f.system.add_window_after_selection(f.layout, wid(2));
        let before = f.frames();
        f.focus(Direction::Left);
        assert_eq!(f.frames(), before);
        f.focus(Direction::Right);
        assert_eq!(f.frames(), before);
        assert_eq!(f.frame(1).origin.x, 0.0);
        assert_eq!(f.frame(2).origin.x, 500.0);
    }

    #[test]
    fn half_width_columns_include_inner_gaps_in_their_share() {
        let mut f = Fixture::new(2);
        f.gaps.inner.horizontal = 20.0;
        let before = f.frames();
        assert_eq!(f.frame(1).size.width, 490.0);
        assert_eq!(f.frame(2).origin.x, 510.0);
        f.focus(Direction::Right);
        assert_eq!(f.frames(), before);
        f.focus(Direction::Left);
        assert_eq!(f.frames(), before);
    }

    #[test]
    fn unequal_and_clipped_columns_reveal_with_minimum_camera_motion() {
        let mut f = Fixture::new(3);
        f.select(2);
        f.system.resize_selection_by(f.layout, -0.1, ResizeOrientation::Horizontal);
        let before = f.frames();
        f.select(1);
        f.select(2);
        assert_eq!(f.frames(), before);
        assert_eq!(f.frame(3).origin.x, 900.0);
        f.focus(Direction::Right);
        assert_eq!(f.frame(3).origin.x, 500.0);
        assert_eq!(f.frame(1).origin.x, -400.0);
        f.focus(Direction::Left);
        assert_eq!(f.frame(1).origin.x, -400.0);
        f.focus(Direction::Left);
        assert_eq!(f.frame(1).origin.x, 0.0);
    }

    #[test]
    fn columns_remember_independent_active_rows() {
        let mut f = Fixture::new(6);
        for (source, target) in [(2, 1), (4, 3), (5, 4)] {
            f.drop(source, target, WindowDropAction::Stack);
        }
        f.select(2);
        assert_eq!(f.focus(Direction::Right), Some(wid(5)));
        assert_eq!(f.focus(Direction::Right), Some(wid(6)));
        assert_eq!(f.focus(Direction::Left), Some(wid(5)));
        assert_eq!(f.focus(Direction::Up), Some(wid(4)));
        assert_eq!(f.focus(Direction::Left), Some(wid(2)));
        assert_eq!(f.focus(Direction::Right), Some(wid(4)));
        f.system.remove_window(wid(3));
        assert_eq!(f.selected(), Some(wid(4)));
        f.system.remove_window(wid(4));
        assert_eq!(f.selected(), Some(wid(5)));
    }

    #[test]
    fn reorder_insert_remove_and_preceding_resize_rebase_the_active_column() {
        let mut f = Fixture::new(4);
        f.select(3);
        let x = f.frame(3).origin.x;
        for direction in [Direction::Left, Direction::Right] {
            assert!(f.system.move_selection(f.layout, direction));
            assert_eq!(f.frame(3).origin.x, x);
            assert_eq!(f.selected(), Some(wid(3)));
        }
        // Move an inactive window before the active one via the shared drop path.
        f.drop(4, 1, WindowDropAction::Insert(Direction::Left));
        assert_eq!(f.frame(3).origin.x, x);
        f.select(3);
        f.system.on_window_resized(
            f.layout,
            wid(4),
            CGRect::ZERO,
            CGRect::new(CGPoint::ZERO, CGSize::new(700.0, 800.0)),
            f.screen,
            &f.gaps,
        );
        assert_eq!(f.frame(3).origin.x, x);
        f.system.remove_window(wid(4));
        assert_eq!(f.frame(3).origin.x, x);
    }

    #[test]
    fn removing_a_transient_active_column_restores_the_previous_view() {
        let mut f = Fixture::new(3);
        f.select(2);
        f.system.center_selected_column(f.layout);
        let before = f.frames();
        f.system.add_window_after_selection(f.layout, wid(4));
        assert_eq!(f.selected(), Some(wid(4)));
        f.system.remove_window(wid(4));
        assert_eq!(f.selected(), Some(wid(2)));
        assert_eq!(f.frames(), before);
        f.system.add_window_after_selection(f.layout, wid(5));
        f.select(3); // Intentional focus change cancels transient restoration.
        let before = f.frame(3);
        f.system.remove_window(wid(5));
        assert_eq!(f.selected(), Some(wid(3)));
        assert_eq!(f.frame(3), before);
    }

    #[test]
    fn consume_expel_and_horizontal_extraction_preserve_identity_sizing_and_focus() {
        for direction in [Direction::Left, Direction::Right] {
            let mut f = Fixture::new(2);
            f.select(1);
            f.system.resize_selection_by(f.layout, -0.1, ResizeOrientation::Horizontal);
            let destination_id = f.system.container_tree(f.layout).children[0].node_id;
            f.select(2);
            f.system.consume_or_expel_selection(f.layout, Direction::Left);
            assert_eq!(f.selected(), Some(wid(2)));
            assert_eq!(f.frame(2).size.width, 400.0);
            assert_eq!(
                f.system.container_tree(f.layout).children[0].node_id,
                destination_id
            );
            f.system.resize_selection_by(f.layout, 0.2, ResizeOrientation::Vertical);
            let x = f.frame(2).origin.x;
            f.system.consume_or_expel_selection(f.layout, direction);
            assert_eq!(f.selected(), Some(wid(2)));
            assert_eq!(f.frame(2).origin.x, x);
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
    fn removing_the_original_window_does_not_change_column_identity() {
        let mut f = Fixture::new(2);
        let id = f.system.container_tree(f.layout).children[0].node_id;
        f.drop(2, 1, WindowDropAction::Stack);
        f.system.remove_window(wid(1));
        assert_eq!(f.system.container_tree(f.layout).children[0].node_id, id);
        assert_eq!(f.selected(), Some(wid(2)));
        f.system.replace_window(wid(2), wid(3));
        assert_eq!(f.selected(), Some(wid(3)));
        assert_eq!(f.system.container_tree(f.layout).children[0].node_id, id);
    }

    #[test]
    fn resize_preserves_its_left_edge_when_selection_still_fits() {
        let mut f = Fixture::new(3);
        f.select(2);
        f.system.center_selected_column(f.layout);
        let before = f.frames();
        f.system.resize_selection_by(f.layout, 0.1, ResizeOrientation::Horizontal);
        assert_eq!(f.frame(1), before[0].1);
        assert_eq!(f.frame(2).origin.x, before[1].1.origin.x);
        assert_eq!(f.frame(2).size.width, 600.0);
        assert_eq!(f.frame(3).origin.x, 850.0);
    }

    #[test]
    fn center_is_a_camera_operation_and_ordinary_focus_resumes() {
        let mut f = Fixture::new(3);
        f.select(2);
        f.system.center_selected_column(f.layout);
        assert_eq!(f.frame(2).origin.x, 250.0);
        f.select(2);
        assert_eq!(f.frame(2).origin.x, 250.0);
        f.system.center_selected_column(f.layout); // Idempotent, never toggles a mode.
        assert_eq!(f.frame(2).origin.x, 250.0);
        f.focus(Direction::Right);
        assert_eq!(f.frame(3).origin.x, 500.0);
        f.focus(Direction::Left);
        assert_eq!(f.frame(2).origin.x, 0.0);
    }

    #[test]
    fn fullscreen_restores_camera_and_reconciles_preceding_removal() {
        for within_gaps in [false, true] {
            let mut f = Fixture::new(3);
            f.gaps.outer.left = 10.0;
            f.gaps.outer.right = 10.0;
            f.prepare();
            f.select(2);
            f.system.center_selected_column(f.layout);
            let before = f.frame(2);
            if within_gaps {
                f.system.toggle_fullscreen_within_gaps_of_selection(f.layout);
            } else {
                f.system.toggle_fullscreen_of_selection(f.layout);
            }
            assert_eq!(
                f.frame(2),
                if within_gaps {
                    compute_tiling_area(f.screen, &f.gaps)
                } else {
                    f.screen
                }
            );
            f.system.remove_window(wid(1));
            if within_gaps {
                f.system.toggle_fullscreen_within_gaps_of_selection(f.layout);
            } else {
                f.system.toggle_fullscreen_of_selection(f.layout);
            }
            assert_eq!(f.frame(2), before);
        }

        let mut f = Fixture::new(2);
        f.drop(2, 1, WindowDropAction::Stack);
        f.system.center_selected_column(f.layout);
        let x = f.frame(2).origin.x;
        f.system.toggle_fullscreen_of_selection(f.layout);
        f.select(1);
        f.system.toggle_fullscreen_of_selection(f.layout);
        f.system.remove_window(wid(2)); // Another fullscreen tile is still in the column.
        f.system.toggle_fullscreen_of_selection(f.layout);
        assert_eq!(f.frame(1).origin.x, x);

        f.system.toggle_fullscreen_of_selection(f.layout);
        f.system.resize_selection_by(f.layout, 0.3, ResizeOrientation::Horizontal);
        f.system.toggle_fullscreen_of_selection(f.layout);
        assert_eq!(f.frame(1).origin.x, 200.0); // Reconcile the saved view with the enlarged tile.
    }

    #[test]
    fn width_height_and_axis_specific_constraints_remain_independent() {
        // Primary owner for the previous separate min/max/fixed-axis regressions.
        for (width, height, constraints) in [
            (500.0, 800.0, WindowLayoutConstraints {
                is_resizable: true,
                min_width: 500.0,
                min_height: 350.0,
                ..Default::default()
            }),
            (300.0, 800.0, WindowLayoutConstraints {
                is_resizable: true,
                max_width: 300.0,
                ..Default::default()
            }),
            (1400.0, 400.0, WindowLayoutConstraints {
                locked_width: 1400.0,
                locked_height: 400.0,
                ..Default::default()
            }),
            (1400.0, 800.0, WindowLayoutConstraints {
                is_resizable: true,
                min_width: 1400.0,
                ..Default::default()
            }),
        ] {
            let mut f = Fixture::new(1);
            f.constraints.insert(wid(1), constraints);
            assert_eq!(f.frame(1).size, CGSize::new(width, height));
        }
        let mut f = Fixture::new(2);
        f.drop(2, 1, WindowDropAction::Stack);
        f.constraints.insert(wid(1), WindowLayoutConstraints {
            locked_width: 700.0,
            locked_height: 400.0,
            min_width: 700.0,
            max_width: 700.0,
            ..Default::default()
        });
        f.constraints.insert(wid(2), WindowLayoutConstraints {
            is_resizable: true,
            max_width: 500.0,
            min_height: 350.0,
            ..Default::default()
        });
        assert_eq!(f.frame(1).size, CGSize::new(700.0, 400.0));
        assert_eq!(f.frame(2).size, CGSize::new(500.0, 400.0));

        let mut f = Fixture::new(2);
        f.constraints.insert(wid(1), WindowLayoutConstraints {
            locked_width: 350.0,
            is_resizable: false,
            ..Default::default()
        });
        let before = f.frames();
        assert_eq!(f.frame(2).origin.x, 350.0);
        f.focus(Direction::Right);
        assert_eq!(f.frames(), before);
    }

    #[test]
    fn preserved_width_and_display_defaults_obey_explicit_resize_bounds() {
        let mut f = Fixture::new(2);
        let mut settings = f.system.settings.clone();
        settings.preserve_window_sizes = true;
        settings.per_display.insert("display".into(), ScrollingWidthOverride {
            column_width_ratio: Some(0.6),
            min_column_width_ratio: Some(0.3),
            max_column_width_ratio: Some(0.7),
        });
        f.system.update_settings(&settings);
        f.constraints.insert(wid(1), WindowLayoutConstraints {
            is_resizable: true,
            locked_width: 400.0,
            ..Default::default()
        });
        assert_eq!(f.frame(1).size.width, 400.0);
        f.system.update_width_settings(settings.widths_for_display(Some("display")));
        assert_eq!(f.frame(1).size.width, 400.0);
        assert_eq!(f.frame(2).size.width, 600.0);
        f.select(1);
        for (delta, expected) in [(0.1, 500.0), (1.0, 700.0), (-1.0, 300.0)] {
            f.system.resize_selection_by(f.layout, delta, ResizeOrientation::Horizontal);
            assert_eq!(f.frame(1).size.width, expected);
        }
    }

    #[test]
    fn smart_and_vertical_resize_preserve_width_and_actual_row_heights() {
        for orientation in [ResizeOrientation::Vertical, ResizeOrientation::Smart] {
            let mut f = Fixture::new(2);
            f.drop(2, 1, WindowDropAction::Stack);
            f.system.resize_selection_by(f.layout, 0.1, orientation);
            assert_eq!(f.frame(2).size, CGSize::new(500.0, 480.0));
            assert_eq!(f.frame(1).size.height, 320.0);
            let new = CGRect::new(CGPoint::ZERO, CGSize::new(500.0, 600.0));
            let old = f.frame(2);
            f.system.on_window_resized(f.layout, wid(2), old, new, f.screen, &f.gaps);
            assert_eq!(f.frame(2).size.height, 600.0);
            assert_eq!(f.frame(1).size.height, 200.0);
        }
    }

    #[test]
    fn anchored_alignments_and_single_column_placement_remain_supported() {
        for (alignment, middle, single) in [
            (ScrollingAlignment::Left, 0.0, 0.0),
            (ScrollingAlignment::Center, 250.0, 250.0),
            (ScrollingAlignment::Right, 500.0, 500.0),
        ] {
            let mut f = Fixture::new(3);
            let mut settings = f.system.settings.clone();
            settings.alignment = alignment;
            settings.focus_navigation_style = ScrollingFocusNavigationStyle::Anchored;
            f.system.update_settings(&settings);
            for (index, x) in [(2, middle), (3, 500.0), (1, 0.0)] {
                f.select(index);
                assert_eq!(f.frame(index).origin.x, x);
                f.system.begin_viewport_gesture(f.layout, Instant::now());
                f.system.update_viewport_gesture(f.layout, 20.0, Duration::from_millis(10));
                let release = f
                    .system
                    .end_viewport_gesture(f.layout, Duration::from_millis(210), false)
                    .unwrap();
                assert_eq!(release.window, wid(index), "alignment={alignment:?}");
                assert_eq!(f.frame(index).origin.x, x);
            }
            let mut system = ScrollingLayoutSystem::new(&settings);
            let layout = system.create_layout();
            system.add_window_after_selection(layout, wid(1));
            system.prepare_layout(layout, f.screen, &f.constraints, &f.gaps);
            assert_eq!(system.viewport_frames(layout).next().unwrap().1.origin.x, single);
        }
    }

    #[test]
    fn gesture_changes_focus_only_once_after_lift() {
        for (start, travel, target, window) in [
            (1, 150.0, 0.0, 2),
            (2, 250.0, 0.0, 2), // Equidistant snaps choose the earlier position, even with active focus.
            (1, 900.0, 1000.0, 4),
            (1, 2000.0, 1000.0, 4),
        ] {
            let mut f = Fixture::new(4);
            f.select(start);
            f.prepare();
            assert!(f.system.begin_viewport_gesture(f.layout, Instant::now()));
            f.system.update_viewport_gesture(f.layout, travel, Duration::from_millis(10));
            assert_eq!(f.selected(), Some(wid(start)));
            let release = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(210), false)
                .unwrap();
            assert_eq!(release.offset, target);
            assert_eq!(release.window, wid(window));
            assert_eq!(f.selected(), Some(wid(window)));
            assert!(
                f.system
                    .end_viewport_gesture(f.layout, Duration::from_millis(220), false)
                    .is_none()
            );
        }
    }

    #[test]
    fn release_velocity_projects_momentum_and_idle_release_stays_nearby() {
        let mut offsets = Vec::new();
        for (drag_ms, idle_ms, pause_ms) in [(30, 0, 0), (300, 0, 0), (30, 200, 0), (10, 0, 300)] {
            let mut f = Fixture::new(4);
            f.system.begin_viewport_gesture(f.layout, Instant::now());
            f.system.update_viewport_gesture(f.layout, 0.0, Duration::from_millis(pause_ms));
            for i in 1..=6 {
                f.system.update_viewport_gesture(
                    f.layout,
                    200.0 / 6.0,
                    Duration::from_millis(pause_ms + i * drag_ms / 6),
                );
                assert_eq!(f.selected(), Some(wid(1)));
            }
            let release = f
                .system
                .end_viewport_gesture(
                    f.layout,
                    Duration::from_millis(pause_ms + drag_ms + idle_ms),
                    false,
                )
                .unwrap();
            offsets.push(release.offset);
            if idle_ms > 0 {
                assert_eq!(release.velocity, 0.0);
                assert_eq!(release.offset, 0.0);
            }
        }
        assert!(offsets[0] > offsets[1]);
        assert!(offsets[0] > offsets[2]);
        assert!(
            offsets[3] > offsets[1],
            "a pause before a flick must not dilute its velocity"
        );
    }

    #[test]
    fn release_respects_navigation_style_and_preserves_flick_momentum() {
        for style in [
            ScrollingFocusNavigationStyle::Niri,
            ScrollingFocusNavigationStyle::Anchored,
        ] {
            for ((distance, speed), target) in [
                (240.0, 75.0),
                (300.0, 75.0),
                (240.0, 400.0),
                (300.0, 400.0),
                (500.0, 1000.0),
                (240.0, 3000.0),
            ]
            .into_iter()
            .zip([500.0, 500.0, 500.0, 500.0, 1000.0, 1000.0])
            {
                let mut f = Fixture::new(4);
                let mut settings = f.system.settings.clone();
                settings.focus_navigation_style = style;
                f.system.update_settings(&settings);
                f.system.begin_viewport_gesture(f.layout, Instant::now());
                f.system.update_viewport_gesture(f.layout, 0.0, Duration::ZERO);
                let steps = (distance / (speed * 0.01)) as u64;
                for i in 1..=steps {
                    f.system.update_viewport_gesture(
                        f.layout,
                        speed * 0.01,
                        Duration::from_millis(i * 10),
                    );
                }
                let release = f
                    .system
                    .end_viewport_gesture(f.layout, Duration::from_millis(steps * 10), false)
                    .unwrap();
                assert!(
                    (release.offset - target).abs() < 0.001,
                    "style={style:?}, distance={distance}, speed={speed}, offset={}",
                    release.offset
                );
                assert!(release.velocity > 0.0, "slow release must not discard velocity");
            }
        }
    }

    #[test]
    fn niri_release_snaps_variable_width_columns_and_both_oversized_edges() {
        for (widths, travel, target, selected) in [
            ([300.0, 700.0, 400.0], 260.0, 300.0, 2),
            ([300.0, 700.0, 400.0], 370.0, 400.0, 3),
            ([500.0, 1400.0, 500.0], 540.0, 500.0, 2),
            ([500.0, 1400.0, 500.0], 860.0, 900.0, 2),
        ] {
            let mut f = Fixture::new(3);
            for (i, width) in widths.into_iter().enumerate() {
                f.constraints.insert(wid(i as u32 + 1), WindowLayoutConstraints {
                    locked_width: width,
                    ..Default::default()
                });
            }
            f.prepare();
            assert!(f.system.begin_viewport_gesture(f.layout, Instant::now()));
            f.system.update_viewport_gesture(f.layout, travel, Duration::from_millis(10));
            assert_eq!(f.selected(), Some(wid(1)));
            let release = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(210), false)
                .unwrap();
            assert_eq!(release.from_offset, travel);
            assert_eq!(release.offset, target);
            assert_eq!(release.window, wid(selected));
        }
    }

    #[test]
    fn niri_flick_selects_the_furthest_visible_column_in_each_direction() {
        for (start, delta, target, selected) in [(1, 20.0, 1000.0, 4), (4, -20.0, 0.0, 1)] {
            let mut f = Fixture::new(4);
            f.select(start);
            f.prepare();
            assert!(f.system.begin_viewport_gesture(f.layout, Instant::now()));
            f.system.update_viewport_gesture(f.layout, 0.0, Duration::ZERO);
            for i in 1..=6 {
                f.system.update_viewport_gesture(f.layout, delta, Duration::from_millis(i * 10));
                assert_eq!(f.selected(), Some(wid(start)));
            }
            let release = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(60), false)
                .unwrap();
            assert_eq!(release.offset, target);
            assert_eq!(release.window, wid(selected));
            assert_eq!(release.velocity.signum(), delta.signum());
        }
    }

    #[test]
    fn structural_edits_rebase_an_ongoing_gesture_without_changing_focus() {
        let mut f = Fixture::new(4);
        f.select(3);
        f.system.begin_viewport_gesture(f.layout, Instant::now());
        f.system.update_viewport_gesture(f.layout, 30.0, Duration::from_millis(10));
        let x = f.frame(3).origin.x;
        f.system.remove_window(wid(1));
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
    fn native_parking_uses_physical_edges_and_logical_geometry_keeps_rows() {
        let mut f = Fixture::new(5);
        f.screen.origin = CGPoint::new(1200.0, -200.0);
        f.gaps.outer.left = 20.0;
        f.gaps.outer.right = 20.0;
        f.gaps.inner.horizontal = 17.0;
        f.gaps.inner.vertical = 11.0;
        f.drop(5, 4, WindowDropAction::Stack);
        f.select(1);
        let logical = f.frames();
        let native: Vec<_> = f.system.viewport_frames(f.layout).collect();
        for index in 0..3 {
            assert!((logical[index + 1].1.origin.x - logical[index].1.max().x - 17.0).abs() <= 1.0);
        }
        assert_eq!(logical[3].1.origin.x, logical[4].1.origin.x);
        assert!((logical[4].1.origin.y - logical[3].1.max().y - 11.0).abs() <= 1.0);
        assert_eq!(native[3].1.origin.x, f.screen.max().x);
        assert!(logical[3].1.origin.x > native[3].1.origin.x);
        assert_eq!(f.system.viewport_frames(f.layout).collect::<Vec<_>>(), native);
    }

    #[test]
    fn drop_swap_stack_and_unstack_share_the_same_structural_operations() {
        let mut f = Fixture::new(3);
        f.drop(2, 1, WindowDropAction::Insert(Direction::Down));
        assert_eq!(f.system.window_slot(f.layout, wid(2)), Some(vec![0, 1]));
        f.drop(3, 2, WindowDropAction::Swap);
        assert_eq!(f.selected(), Some(wid(2)));
        assert_eq!(f.system.window_slot(f.layout, wid(2)), Some(vec![1, 0]));
        let moved = f
            .system
            .apply_stacking_to_parent_of_selection(f.layout, StackDefaultOrientation::Vertical);
        assert_eq!(moved, vec![wid(1), wid(3)]);
        assert_eq!(f.selected(), Some(wid(2)));
        let x = f.frame(2).origin.x;
        f.system
            .unstack_parent_of_selection(f.layout, StackDefaultOrientation::Vertical);
        assert_eq!(f.frame(2).origin.x, x);
        assert_eq!(f.system.all_windows_in_layout(f.layout), vec![
            wid(2),
            wid(1),
            wid(3)
        ]);
    }

    #[test]
    fn app_reconciliation_honors_reloaded_insertion_policy() {
        for insertion in [
            WindowInsertionPoint::NextToSelection,
            WindowInsertionPoint::EndOfTree,
        ] {
            let mut f = Fixture::new(2);
            let mut settings = f.system.settings.clone();
            settings.base.window_insertion_point = Some(insertion);
            f.system.update_settings(&settings);
            f.system.set_windows_for_app(f.layout, 1, vec![wid(1), wid(2), wid(3)]);
            let expected = if insertion == WindowInsertionPoint::EndOfTree {
                vec![wid(1), wid(2), wid(3)]
            } else {
                vec![wid(1), wid(3), wid(2)]
            };
            assert_eq!(f.system.all_windows_in_layout(f.layout), expected);
        }
    }

    #[test]
    fn persistence_clone_and_cross_layout_transfer_preserve_column_state() {
        let mut f = Fixture::new(3);
        f.drop(2, 1, WindowDropAction::Stack);
        f.select(2);
        f.system.center_selected_column(f.layout);
        let tree = f.system.container_tree(f.layout);
        let frames = f.frames();
        let clone = f.system.clone_layout(f.layout);
        assert_eq!(f.system.container_tree(clone), tree);
        assert_eq!(f.system.viewport_frames(clone).collect::<Vec<_>>(), frames);
        let text = ron::to_string(&f.system).unwrap();
        let mut restored: ScrollingLayoutSystem = ron::from_str(&text).unwrap();
        restored.update_settings(&f.system.settings);
        restored.prepare_layout(f.layout, f.screen, &f.constraints, &f.gaps);
        assert_eq!(restored.container_tree(f.layout), tree);
        assert_eq!(restored.viewport_frames(f.layout).collect::<Vec<_>>(), frames);
        f.system.select_window(clone, wid(3));
        f.system.center_selected_column(clone);
        let old_x = f
            .system
            .calculate_frames(clone, f.screen, &f.constraints, &f.gaps, false)
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
            .calculate_frames(clone, wider, &f.constraints, &f.gaps, false)
            .into_iter()
            .find(|(w, _)| *w == wid(3))
            .unwrap()
            .1
            .origin
            .x;
        assert_eq!(new_x, old_x);

        let destination = f.system.create_layout();
        f.system.move_selection_to_layout_after_selection(f.layout, destination);
        assert_eq!(f.system.selected_window(destination), Some(wid(2)));
        assert_eq!(f.selected(), Some(wid(1)));
        assert!(!f.system.contains_window(f.layout, wid(2)));
        f.system.add_window_after_selection(f.layout, wid(4));
        let ids: Vec<_> =
            f.system.container_tree(f.layout).children.iter().map(|c| c.node_id).collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn empty_short_strip_and_invalid_input_never_produce_invalid_frames() {
        for count in 0..=2 {
            let mut f = Fixture::new(count);
            f.system.scroll_by_delta(f.layout, f64::NAN);
            f.system.scroll_by_delta(f.layout, 100.0);
            f.system.snap_to_nearest_column(f.layout);
            for (_, frame) in f.frames() {
                assert!(frame.origin.x.is_finite());
                assert!(frame.size.width > 0.0);
            }
            for index in 1..=count {
                f.system.remove_window(wid(index));
            }
            assert_eq!(f.selected(), None);
            assert!(f.system.viewport_frames(f.layout).next().is_none());
            assert!(!f.system.begin_viewport_gesture(f.layout, Instant::now()));
        }
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
            system.viewport_frames(layout).next().unwrap().1.size.width,
            600.0
        );
        system.add_window_after_selection(layout, wid(3));
        assert_ne!(system.container_tree(layout).children[1].node_id, 42);
    }
    #[test]
    fn gesture_edges_stay_bounded_and_reverse_immediately() {
        for direction in [-1.0, 1.0] {
            let mut f = Fixture::new(4);
            let bounds = f.system.layouts[f.layout].geometry.as_ref().unwrap().bounds;
            let edge = if direction < 0.0 { bounds.0 } else { bounds.1 };
            f.system.layouts[f.layout].viewport = Viewport::Static(edge);
            assert!(f.system.begin_viewport_gesture(f.layout, Instant::now()));
            let before = f.frame(1).origin.x;
            for (time, delta, excess) in [(10, 200.0, 0.2), (15, 0.0, 0.2), (20, 100.0, 0.3)] {
                f.system.update_viewport_gesture(
                    f.layout,
                    direction * delta,
                    Duration::from_millis(time),
                );
                assert_eq!(
                    f.frame(1).origin.x,
                    before,
                    "outward motion keeps the strip at its edge"
                );
                assert!((f.system.gesture_overscroll(f.layout) - direction * excess).abs() < 1e-9);
            }
            let release = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(20), true)
                .unwrap();
            assert_eq!(
                release.velocity, 0.0,
                "blocked edge travel cannot fling the camera"
            );
            assert_eq!(release.from_offset, edge);
            assert_eq!(release.offset, edge);
            f.system.begin_viewport_gesture(f.layout, Instant::now());
            f.system.update_viewport_gesture(
                f.layout,
                direction * 200.0,
                Duration::from_millis(30),
            );
            f.system.update_viewport_gesture(
                f.layout,
                -direction * 75.0,
                Duration::from_millis(40),
            );
            assert_eq!(f.frame(1).origin.x, before + direction * 75.0);
            assert_eq!(
                f.system.gesture_overscroll(f.layout),
                0.0,
                "reversal cancels workspace intent immediately"
            );
            f.system.cancel_viewport_gesture(f.layout);
            f.system.begin_viewport_gesture(f.layout, Instant::now());
            f.system.update_viewport_gesture(f.layout, 0.0, Duration::from_millis(40));
            f.system
                .update_viewport_gesture(f.layout, direction * 75.0, Duration::from_millis(50));
            let release = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(50), true)
                .unwrap();
            assert_eq!(release.offset, edge);
            assert_eq!(release.velocity.signum(), direction);
            f.system
                .advance_viewport_animation(f.layout, Instant::now() + Duration::from_millis(10));
            assert_eq!(
                f.frame(1).origin.x,
                before,
                "release cannot bounce past the workspace edge"
            );
        }
    }
    #[test]
    fn niri_projection_selects_near_medium_and_far_snaps_in_both_directions() {
        for direction in [-1.0, 1.0] {
            for (speed, extra) in [(75.0_f64, 0.0), (400.0, 500.0), (2000.0, 1000.0)] {
                let mut f = Fixture::new(8);
                f.system.layouts[f.layout].viewport = Viewport::Static(1500.0);
                f.system.begin_viewport_gesture(f.layout, Instant::now());
                f.system.update_viewport_gesture(f.layout, 0.0, Duration::ZERO);
                let steps = (200.0 / (speed * 0.01)).ceil() as u64;
                for i in 1..=steps {
                    f.system.update_viewport_gesture(
                        f.layout,
                        direction * 200.0 / steps as f64,
                        Duration::from_millis(i * 10),
                    );
                }
                let release = f
                    .system
                    .end_viewport_gesture(f.layout, Duration::from_millis(steps * 10), false)
                    .unwrap();
                assert_eq!(release.offset, 1500.0 + direction * extra);
                assert_eq!(release.velocity.signum(), direction);
                assert!((release.from_offset - (1500.0 + direction * 200.0)).abs() < 1e-8);
            }
        }
    }

    #[test]
    fn recent_motion_discards_idle_time_and_rejects_backwards_timestamps() {
        let mut history = MotionHistory::default();
        history.push(0.0, Duration::ZERO);
        history.push(10.0, Duration::from_millis(10));
        history.push(10.0, Duration::from_millis(20));
        assert_eq!(history.velocity(), 1000.0);
        assert!(!history.push(900.0, Duration::from_millis(19)));
        assert_eq!(history.velocity(), 1000.0);
        history.push(0.0, Duration::from_millis(171));
        assert_eq!(history.velocity(), 0.0);
        for i in 18..=30 {
            history.push(0.0, Duration::from_millis(i * 10));
        }
        history.push(10.0, Duration::from_millis(310));
        history.push(10.0, Duration::from_millis(320));
        history.push(0.0, Duration::from_millis(320));
        assert_eq!(
            history.velocity(),
            1000.0,
            "fresh flick excludes preceding stationary time"
        );

        let mut f = Fixture::new(4);
        f.system.begin_viewport_gesture(f.layout, Instant::now());
        f.system.update_viewport_gesture(f.layout, 20.0, Duration::from_millis(20));
        assert!(
            f.system
                .update_viewport_gesture(f.layout, 900.0, Duration::from_millis(19))
                .is_none()
        );
        assert_eq!(f.system.layouts[f.layout].viewport.offset(), 20.0);
    }

    #[test]
    fn normalized_motion_and_overscroll_use_working_width_with_outer_gaps() {
        let mut f = Fixture::new(4);
        f.gaps.outer.left = 100.0;
        f.gaps.outer.right = 50.0;
        f.prepare();
        f.system.begin_viewport_gesture(f.layout, Instant::now());
        let from = f.system.layouts[f.layout].viewport.offset();
        // The input actor applies sensitivity 4 to quarter-pad travel.
        assert_eq!(
            f.system.update_viewport_gesture_normalized(
                f.layout,
                0.25 * 4.0,
                Duration::from_millis(10)
            ),
            Some(850.0)
        );
        assert_eq!(f.system.layouts[f.layout].viewport.offset(), from + 850.0);
        assert_eq!(
            f.system
                .update_viewport_gesture_normalized(f.layout, 0.25, Duration::from_millis(20)),
            Some(0.0)
        );
        assert_eq!(
            f.system.gesture_overscroll(f.layout),
            0.25,
            "workspace handoff uses working widths even with outer gaps"
        );
    }

    #[test]
    fn release_camera_spring_interrupts_continuously_and_finishes_exactly() {
        for (animate, direction) in [(false, -1.0), (false, 1.0), (true, -1.0), (true, 1.0)] {
            let mut f = Fixture::new(8);
            let from = if direction > 0.0 { 0.0 } else { 3000.0 };
            f.system.layouts[f.layout].viewport = Viewport::Static(from);
            f.system.begin_viewport_gesture(f.layout, Instant::now());
            f.system.update_viewport_gesture(f.layout, 0.0, Duration::ZERO);
            f.system.update_viewport_gesture(
                f.layout,
                direction * 150.0,
                Duration::from_millis(50),
            );
            f.system.update_viewport_gesture(
                f.layout,
                direction * 150.0,
                Duration::from_millis(100),
            );
            let release = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(100), animate)
                .unwrap();
            assert_eq!(release.offset, 1500.0);
            if !animate {
                assert_eq!(f.system.layouts[f.layout].viewport.offset(), 1500.0);
                assert_eq!(
                    f.system.advance_viewport_animation(f.layout, Instant::now()),
                    None
                );
                continue;
            }
            assert_eq!(
                f.system.layouts[f.layout].viewport.offset(),
                from + direction * 300.0
            );
            let Viewport::Animation(spring) = &f.system.layouts[f.layout].viewport else {
                panic!("release spring")
            };
            let started = spring.started;
            let mut previous = from + direction * 300.0;
            for ms in [1, 10, 40, 80] {
                assert_eq!(
                    f.system
                        .advance_viewport_animation(f.layout, started + Duration::from_millis(ms)),
                    Some(true)
                );
                let current = f.system.layouts[f.layout].viewport.offset();
                assert!(
                    (current - previous) * direction > 0.0
                        && (release.offset - current) * direction > 0.0
                );
                if ms == 1 {
                    assert!((current - previous - direction * 3.0).abs() < 0.5);
                }
                previous = current;
            }
            let now = started + Duration::from_millis(80);
            f.system.begin_viewport_gesture(f.layout, now);
            assert_eq!(f.system.layouts[f.layout].viewport.offset(), previous);
            f.system.update_viewport_gesture(
                f.layout,
                direction * 20.0,
                Duration::from_millis(110),
            );
            assert_eq!(
                f.system.layouts[f.layout].viewport.offset(),
                previous + direction * 20.0
            );
            let second = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(310), true)
                .unwrap();
            assert_eq!(second.from_offset, previous + direction * 20.0);
            assert_eq!(
                f.system
                    .advance_viewport_animation(f.layout, Instant::now() + Duration::from_secs(2)),
                Some(false)
            );
            assert!(
                matches!(f.system.layouts[f.layout].viewport, Viewport::Static(x) if x == second.offset)
            );
        }
    }

    #[test]
    fn structural_and_environmental_changes_cancel_release_without_stale_frames() {
        for edit in [0, 1, 2] {
            let mut f = Fixture::new(4);
            f.system.begin_viewport_gesture(f.layout, Instant::now());
            f.system.update_viewport_gesture(f.layout, 200.0, Duration::from_millis(10));
            f.system
                .end_viewport_gesture(f.layout, Duration::from_millis(210), true)
                .unwrap();
            match edit {
                0 => f.system.remove_window(wid(4)),
                1 => {
                    f.gaps.outer.left = 30.0;
                    f.prepare();
                }
                _ => {
                    f.layout = f.system.clone_layout(f.layout);
                }
            }
            let frames = f.frames();
            assert_eq!(
                f.system
                    .advance_viewport_animation(f.layout, Instant::now() + Duration::from_secs(1)),
                None
            );
            assert_eq!(f.frames(), frames);
            let state = &f.system.layouts[f.layout];
            let g = state.geometry.as_ref().unwrap();
            assert!(
                matches!(state.viewport, Viewport::Static(x) if (g.bounds.0..=g.bounds.1).contains(&x))
            );
        }
    }
    #[test]
    fn saving_interactive_camera_restores_current_static_position() {
        for released in [false, true] {
            let mut f = Fixture::new(4);
            f.system.begin_viewport_gesture(f.layout, Instant::now());
            f.system.update_viewport_gesture(f.layout, 200.0, Duration::from_millis(10));
            if released {
                f.system
                    .end_viewport_gesture(f.layout, Duration::from_millis(210), true)
                    .unwrap();
            }
            let snapshot = serde_json::to_string(&f.system).unwrap();
            let settings = f.system.settings.clone();
            f.system = serde_json::from_str(&snapshot).unwrap();
            f.system.update_settings(&settings);
            f.prepare();
            assert_eq!(f.system.layouts[f.layout].viewport.offset(), 200.0);
            assert!(matches!(
                f.system.layouts[f.layout].viewport,
                Viewport::Static(_)
            ));
        }
    }
}
