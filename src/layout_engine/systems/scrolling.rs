//! A strip of persistent columns in world coordinates, translated by one camera.
//! View targets follow niri's fit/center and gesture snap semantics. Native frame
//! animation remains in the reactor; this module owns the semantic destination.
use std::collections::VecDeque;
use std::time::Duration;

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
}

impl Viewport {
    fn offset(&self) -> f64 {
        match self {
            Self::Uninitialized => 0.0,
            Self::Static(offset) | Self::Gesture(offset) => *offset,
        }
    }

    fn rebase(&mut self, delta: f64) {
        match self {
            Self::Uninitialized => {}
            Self::Static(offset) | Self::Gesture(offset) => *offset += delta,
        }
    }
}

// Persist the camera, not an in-progress input session.
impl Serialize for Viewport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.offset().serialize(serializer)
    }
}
impl From<f64> for Viewport {
    fn from(offset: f64) -> Self { Self::Static(if offset.is_finite() { offset } else { 0.0 }) }
}

/// Future input/animation callers can carry release velocity into their settling animation.
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
    column: ColumnId,
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
    let right = left + column.width - view;
    if current <= left && right <= current {
        current
    } else if (current - left).abs() <= (current - right).abs() {
        left
    } else {
        right
    }
}

fn centered_offset(view: f64, column: ColumnGeometry) -> f64 {
    column.world_x - (view - column.width).max(0.0) / 2.0
}

impl Geometry {
    fn build(
        state: &LayoutState,
        settings: &ScrollingLayoutSettings,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
    ) -> Self {
        let tiling = compute_tiling_area(screen, gaps);
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
                constraints,
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
            gaps: gaps.clone(),
            constraints: constraints.clone(),
            columns,
            frames,
            bounds: (0.0, 0.0),
        };
        // The bounds are resting positions, not topology-derived column indices.
        let snaps = geometry.snap_points(settings);
        if !snaps.is_empty() {
            geometry.bounds =
                snaps.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), snap| {
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
                .map(|(index, &column)| SnapPoint {
                    offset: self.anchor_offset(index, settings.alignment),
                    column: column.id,
                })
                .collect();
        }
        let last = self.columns.last().unwrap();
        let left = first.world_x;
        let right = last.world_x + last.width - view;
        let mut points = vec![SnapPoint { offset: left, column: first.id }, SnapPoint {
            offset: right,
            column: last.id,
        }];
        for column in &self.columns {
            for offset in [column.world_x, column.world_x + column.width - view] {
                if left < offset && offset < right {
                    points.push(SnapPoint { offset, column: column.id });
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

/// Recent cumulative pixel motion, independent of viewport rebasing.
#[derive(Clone, Debug, Default)]
struct MotionHistory {
    samples: VecDeque<(Duration, f64)>,
    total: f64,
    stationary: bool,
}
impl MotionHistory {
    fn push(&mut self, delta: f64, time: Duration) {
        if self.samples.back().is_some_and(|(last, _)| time < *last) {
            return;
        }
        self.total += delta;
        // A pause needs one fresh anchor, not 120 identical positions that
        // dilute the next short flick with stationary time.
        if delta == 0.0
            && self.stationary
            && let Some(last) = self.samples.back_mut()
        {
            *last = (time, self.total);
        } else {
            if self.samples.len() == 32 {
                self.samples.pop_front();
            }
            self.samples.push_back((time, self.total));
        }
        self.stationary = delta == 0.0;
        while self
            .samples
            .front()
            .is_some_and(|(first, _)| time.saturating_sub(*first) > Duration::from_millis(150))
        {
            self.samples.pop_front();
        }
    }

    fn velocity(&self) -> f64 {
        let (Some(&(first, a)), Some(&(last, b))) = (self.samples.front(), self.samples.back())
        else {
            return 0.0;
        };
        let dt = last.saturating_sub(first).as_secs_f64();
        if dt > 0.0 { (b - a) / dt } else { 0.0 }
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
        stored.serialize_field("viewport", &self.viewport)?;
        stored.serialize_field(
            "view_anchor",
            &self.bookmark().or_else(|| self.restored_view.clone()),
        )?;
        stored.serialize_field("transient_restore", &self.transient_restore)?;
        stored.serialize_field("fullscreen_restore", &self.fullscreen_restore)?;
        stored.serialize_field("fullscreen", &self.fullscreen)?;
        stored.serialize_field("fullscreen_within_gaps", &self.fullscreen_within_gaps)?;
        stored.end()
    }
}

// Read old snapshots at the persistence boundary; obsolete fields never enter
// the live model. Current snapshots use the same flat representation.
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

    fn active_column_fullscreen(&self) -> bool {
        self.columns.get(self.active_column).is_some_and(|column| {
            column.windows.iter().any(|wid| {
                self.fullscreen.contains(wid) || self.fullscreen_within_gaps.contains(wid)
            })
        })
    }

    fn restore_fullscreen_view(&mut self) -> bool {
        if !self.active_column_fullscreen()
            && let Some(bookmark) = self.fullscreen_restore.take()
        {
            return self.restore(bookmark);
        }
        false
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

    fn render_offset(&self, g: &Geometry) -> f64 {
        let raw = self.viewport.offset();
        if !matches!(self.viewport, Viewport::Gesture(_)) {
            return raw;
        }
        let bounded = raw.clamp(g.bounds.0, g.bounds.1);
        let excess = raw - bounded;
        let limit = (g.tiling.size.width * 0.15).max(1.0);
        bounded + limit * excess / (limit + excess.abs())
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
        let window = self.selected();
        let id = self.columns.get(self.active_column).map(|c| c.id);
        let old_x = id.and_then(|id| self.geometry.as_ref()?.column(id)).map(|c| c.world_x);
        let result = edit(self);
        if let Some(old) = self.geometry.take() {
            self.geometry = Some(Geometry::build(
                self,
                settings,
                old.screen,
                &old.constraints,
                &old.gaps,
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

    fn extract(&mut self, direction: Direction) -> bool {
        let Some((col, _)) = self.selected_location() else {
            return false;
        };
        if self.columns[col].windows.len() < 2 {
            return false;
        }
        let wid = self.selected().unwrap();
        let (width, weight) = self.detach(wid).unwrap();
        self.new_column(
            col + usize::from(direction == Direction::Right),
            wid,
            width,
            weight,
        );
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

    fn calculate_frames(
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
                fallback = Geometry::build(state, &self.settings, screen, constraints, gaps);
                &fallback
            }
        };
        self.translate_frames(state, g, park)
    }

    fn translate_frames(
        &self,
        state: &LayoutState,
        g: &Geometry,
        park: bool,
    ) -> Vec<(WindowId, CGRect)> {
        let offset = state.render_offset(g);
        g.frames
            .iter()
            .map(|&(wid, mut frame)| {
                frame.origin.x += g.tiling.origin.x - offset;
                if park {
                    if frame.max().x <= g.tiling.origin.x {
                        frame.origin.x = g.screen.origin.x - frame.size.width;
                    } else if frame.origin.x >= g.tiling.max().x {
                        frame.origin.x = g.screen.max().x;
                    }
                }
                frame.origin.x = frame.origin.x.round();
                frame.origin.y = frame.origin.y.round();
                frame.size.width = frame.size.width.round();
                frame.size.height = frame.size.height.round();
                if state.fullscreen.contains(&wid) {
                    frame = g.screen;
                } else if state.fullscreen_within_gaps.contains(&wid) {
                    frame = g.tiling;
                }
                (wid, frame)
            })
            .collect()
    }

    /// Cached-geometry fast path; no constraint solving or topology rebuilding.
    pub fn viewport_frames(&self, layout: LayoutId) -> Vec<(WindowId, CGRect)> {
        let Some(state) = self.layouts.get(layout) else {
            return Vec::new();
        };
        state
            .geometry
            .as_ref()
            .map(|g| self.translate_frames(state, g, true))
            .unwrap_or_default()
    }

    pub(crate) fn logical_frames(
        &self,
        layout: LayoutId,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &GapSettings,
    ) -> Vec<(WindowId, CGRect)> {
        self.calculate_frames(layout, screen, constraints, gaps, false)
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

    pub fn begin_viewport_gesture(&mut self, layout: LayoutId) -> bool {
        if !self.viewport_gesture_available(layout) {
            return false;
        }
        let Some(state) = self.layouts.get_mut(layout) else {
            return false;
        };
        if state.columns.is_empty() || state.geometry.is_none() {
            return false;
        }
        state.transient_restore = None;
        if matches!(state.viewport, Viewport::Gesture(_)) {
            return false;
        }
        state.motion = MotionHistory {
            samples: VecDeque::with_capacity(32),
            total: 0.0,
            stationary: true,
        };
        state.viewport = Viewport::Gesture(state.viewport.offset());
        true
    }

    /// Pixel deltas, independent of any native input source; focus stays fixed.
    pub fn update_viewport_gesture(
        &mut self,
        layout: LayoutId,
        delta: f64,
        timestamp: Duration,
    ) -> Option<Direction> {
        if !delta.is_finite() {
            return None;
        }
        let state = self.layouts.get_mut(layout)?;
        let Viewport::Gesture(offset) = &mut state.viewport else {
            return None;
        };
        state.motion.push(delta, timestamp);
        *offset += delta;
        let g = state.geometry.as_ref()?;
        if *offset < g.bounds.0 {
            Some(Direction::Left)
        } else if *offset > g.bounds.1 {
            Some(Direction::Right)
        } else {
            None
        }
    }

    /// Layout owns release velocity and projection. Idle time at lift removes fling.
    pub fn end_viewport_gesture(
        &mut self,
        layout: LayoutId,
        timestamp: Duration,
        projected_offset: Option<f64>,
    ) -> Option<ViewportRelease> {
        let state = self.layouts.get_mut(layout)?;
        let Viewport::Gesture(offset) = &mut state.viewport else {
            return None;
        };
        state.motion.push(0.0, timestamp);
        let velocity = state.motion.velocity();
        let projected = projected_offset
            .filter(|x| x.is_finite())
            .unwrap_or_else(|| *offset - velocity / (1000.0 * 0.997f64.ln()));
        self.settle(layout, projected, velocity)
    }

    /// Settle where cancellation occurred, with no artificial fling or focus change.
    pub fn cancel_viewport_gesture(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.get_mut(layout) {
            if let Viewport::Gesture(offset) = state.viewport {
                let offset = state
                    .geometry
                    .as_ref()
                    .map_or(offset, |g| offset.clamp(g.bounds.0, g.bounds.1));
                state.viewport = Viewport::Static(offset);
                state.motion = MotionHistory::default();
            }
        }
    }

    pub fn gesture_overscroll(&self, layout: LayoutId) -> f64 {
        let Some(state) = self.layouts.get(layout) else {
            return 0.0;
        };
        let Some(g) = &state.geometry else {
            return 0.0;
        };
        let x = state.viewport.offset();
        x - x.clamp(g.bounds.0, g.bounds.1)
    }

    fn settle(
        &mut self,
        layout: LayoutId,
        projected: f64,
        velocity: f64,
    ) -> Option<ViewportRelease> {
        let state = self.layouts.get_mut(layout)?;
        let g = state.geometry.as_ref()?;
        let from_offset = state.render_offset(g);
        let snap = g
            .snap_points(&self.settings)
            .into_iter()
            .min_by(|a, b| (a.offset - projected).abs().total_cmp(&(b.offset - projected).abs()))?;
        let mut index = state.columns.iter().position(|c| c.id == snap.column)?;
        if self.settings.focus_navigation_style == ScrollingFocusNavigationStyle::Niri {
            // Like niri, choose the furthest fully visible column in travel direction.
            if projected >= state.viewport.offset() {
                for (next, column) in g.columns.iter().enumerate().skip(index + 1) {
                    if column.world_x + column.width > snap.offset + g.tiling.size.width {
                        break;
                    }
                    index = next;
                }
            } else {
                for next in (0..index).rev() {
                    if g.columns[next].world_x < snap.offset {
                        break;
                    }
                    index = next;
                }
            }
        }
        if index != state.active_column {
            state.fullscreen_restore = None;
        }
        state.active_column = index;
        state.transient_restore = None;
        state.viewport = Viewport::Static(snap.offset);
        Some(ViewportRelease {
            window: state.selected()?,
            offset: snap.offset,
            from_offset,
            velocity,
        })
    }

    pub fn snap_to_nearest_column(&mut self, layout: LayoutId) -> Option<WindowId> {
        let offset = self.layouts.get(layout)?.viewport.offset();
        self.settle(layout, offset, 0.0).map(|release| release.window)
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
        self.layouts.insert(self.layouts.get(layout).cloned().unwrap_or_default())
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
                        frame: Default::default(),
                        layout_kind: None,
                        weight: Some(column.height_weights.get(index).copied().unwrap_or(1.0)),
                        window_id: Some(window.into()),
                        is_selected: state.selected() == Some(window),
                        is_fullscreen: state.fullscreen.contains(&window),
                        is_fullscreen_within_gaps: state.fullscreen_within_gaps.contains(&window),
                        role: None,
                        pending_split: None,
                        children: Vec::new(),
                    })
                    .collect();
                rift_protocol::ContainerTreeNode {
                    node_id: column.id.0,
                    node_type: rift_protocol::ContainerNodeType::Container,
                    frame: Default::default(),
                    layout_kind: Some(rift_protocol::LayoutKind::Vertical),
                    weight: None,
                    window_id: None,
                    is_selected: false,
                    is_fullscreen: false,
                    is_fullscreen_within_gaps: false,
                    role: Some("column".to_owned()),
                    pending_split: None,
                    children: windows,
                }
            })
            .collect();

        rift_protocol::ContainerTreeNode {
            node_id: 0,
            node_type: rift_protocol::ContainerNodeType::Container,
            frame: Default::default(),
            layout_kind: Some(rift_protocol::LayoutKind::Horizontal),
            weight: None,
            window_id: None,
            is_selected: false,
            is_fullscreen: false,
            is_fullscreen_within_gaps: false,
            role: None,
            pending_split: None,
            children,
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
        state.geometry = Some(Geometry::build(state, &self.settings, screen, constraints, gaps));
        if let Some(bookmark) = bookmark
            && let Some(column) = state.geometry.as_ref().unwrap().column(bookmark.column)
        {
            let old = state.viewport.offset();
            state.viewport.rebase(column.world_x + bookmark.relative_offset - old);
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
        let Some((ac, ar)) = state.locate(a) else {
            return false;
        };
        let Some((bc, br)) = state.locate(b) else {
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
                return state.extract(direction);
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
        let Some(state) = self.layouts.get_mut(layout) else {
            return Vec::new();
        };
        let Some((col, _)) = state.selected_location() else {
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
        let Some(state) = self.layouts.get_mut(layout) else {
            return Vec::new();
        };
        let Some((col, _)) = state.selected_location() else {
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
        let Some(state) = self.layouts.get_mut(layout) else {
            return;
        };
        let Some((col, row)) = state.selected_location() else {
            return;
        };
        state.mutate(&self.settings, |state| {
            let column = &mut state.columns[col];
            if orientation == ResizeOrientation::Vertical
                || (orientation == ResizeOrientation::Smart && column.windows.len() > 1)
            {
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
            self.system
                .logical_frames(self.layout, self.screen, &self.constraints, &self.gaps)
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
    fn active_resize_preserves_its_left_edge_and_contiguous_strip() {
        let mut f = Fixture::new(3);
        f.select(2);
        let before = f.frames();
        f.system.resize_selection_by(f.layout, 0.1, ResizeOrientation::Horizontal);
        assert_eq!(f.frame(1), before[0].1);
        assert_eq!(f.frame(2).origin.x, before[1].1.origin.x);
        assert_eq!(f.frame(2).size.width, 600.0);
        assert_eq!(f.frame(3).origin.x, 1100.0);
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
            }
            let mut system = ScrollingLayoutSystem::new(&settings);
            let layout = system.create_layout();
            system.add_window_after_selection(layout, wid(1));
            system.prepare_layout(layout, f.screen, &f.constraints, &f.gaps);
            assert_eq!(system.viewport_frames(layout)[0].1.origin.x, single);
        }
    }

    #[test]
    fn snap_points_include_fit_edges_and_safe_short_strip_boundaries() {
        for (widths, mut expected) in [
            (vec![500.0, 500.0, 500.0], vec![0.0, 500.0]),
            (vec![400.0, 700.0, 300.0], vec![0.0, 100.0, 400.0]),
            (vec![1400.0, 500.0], vec![0.0, 400.0, 900.0]),
            (vec![400.0, 400.0], vec![-200.0, 0.0]),
            (vec![500.0], vec![-500.0, 0.0]),
            (vec![], vec![]),
        ] {
            let mut f = Fixture::new(widths.len() as u32);
            // Exercise fixed sizing through the production geometry helper.
            let settings = f.system.settings.clone();
            f.system.layouts[f.layout].mutate(&settings, |state| {
                for (column, width) in state.columns.iter_mut().zip(widths) {
                    column.width = ColumnWidth::Fixed(width);
                }
            });
            let g = f.system.layouts[f.layout].geometry.as_ref().unwrap();
            let points = g.snap_points(&settings);
            let mut offsets: Vec<_> = points.iter().map(|p| p.offset).collect();
            offsets.sort_by(f64::total_cmp);
            offsets.dedup();
            expected.sort_by(f64::total_cmp);
            assert_eq!(offsets, expected);
            assert!(points.iter().all(|p| g.column(p.column).is_some()));
        }
    }

    #[test]
    fn gesture_updates_keep_focus_and_release_uses_projected_destination_once() {
        let mut f = Fixture::new(4);
        let before = f.frame(1);
        assert!(f.system.begin_viewport_gesture(f.layout));
        for _ in 0..3 {
            f.system.update_viewport_gesture(f.layout, 50.0, Duration::from_millis(10));
            assert_eq!(f.selected(), Some(wid(1)));
        }
        assert_eq!(f.frame(1).origin.x, before.origin.x - 150.0);
        let release = f
            .system
            .end_viewport_gesture(f.layout, Duration::from_millis(20), Some(900.0))
            .unwrap();
        assert_eq!(release.offset, 1000.0);
        assert_eq!(release.window, wid(4));
        assert_eq!(f.selected(), Some(wid(4)));
        assert!(release.velocity.is_finite());
        assert!(
            f.system
                .end_viewport_gesture(f.layout, Duration::from_millis(30), None)
                .is_none()
        );
    }

    #[test]
    fn release_velocity_projects_momentum_and_idle_release_stays_nearby() {
        let mut offsets = Vec::new();
        for (drag_ms, idle_ms) in [(30, 0), (300, 0), (30, 200)] {
            let mut f = Fixture::new(4);
            f.system.begin_viewport_gesture(f.layout);
            f.system.update_viewport_gesture(f.layout, 0.0, Duration::ZERO);
            for i in 1..=6 {
                f.system.update_viewport_gesture(
                    f.layout,
                    200.0 / 6.0,
                    Duration::from_millis(i * drag_ms / 6),
                );
                assert_eq!(f.selected(), Some(wid(1)));
            }
            let release = f
                .system
                .end_viewport_gesture(f.layout, Duration::from_millis(drag_ms + idle_ms), None)
                .unwrap();
            offsets.push(release.offset);
            if idle_ms > 0 {
                assert_eq!(release.velocity, 0.0);
            }
        }
        assert!(offsets[0] > offsets[1]);
        assert!(offsets[0] > offsets[2]);
    }

    #[test]
    fn structural_edits_rebase_an_ongoing_gesture_without_changing_focus() {
        let mut f = Fixture::new(4);
        f.select(3);
        f.system.begin_viewport_gesture(f.layout);
        f.system.update_viewport_gesture(f.layout, 30.0, Duration::from_millis(10));
        let x = f.frame(3).origin.x;
        f.system.remove_window(wid(1));
        assert_eq!(f.frame(3).origin.x, x);
        assert_eq!(f.selected(), Some(wid(3)));
        f.system.update_viewport_gesture(f.layout, 20.0, Duration::from_millis(20));
        assert_eq!(f.frame(3).origin.x, x - 20.0);
        assert_eq!(f.selected(), Some(wid(3)));
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
        let native = f.system.viewport_frames(f.layout);
        for index in 0..3 {
            assert!((logical[index + 1].1.origin.x - logical[index].1.max().x - 17.0).abs() <= 1.0);
        }
        assert_eq!(logical[3].1.origin.x, logical[4].1.origin.x);
        assert!((logical[4].1.origin.y - logical[3].1.max().y - 11.0).abs() <= 1.0);
        assert_eq!(native[3].1.origin.x, f.screen.max().x);
        assert!(logical[3].1.origin.x > native[3].1.origin.x);
        assert_eq!(f.system.viewport_frames(f.layout), native);
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
        assert_eq!(f.system.viewport_frames(clone), frames);
        let text = ron::to_string(&f.system).unwrap();
        let mut restored: ScrollingLayoutSystem = ron::from_str(&text).unwrap();
        restored.update_settings(&f.system.settings);
        restored.prepare_layout(f.layout, f.screen, &f.constraints, &f.gaps);
        assert_eq!(restored.container_tree(f.layout), tree);
        assert_eq!(restored.viewport_frames(f.layout), frames);
        f.system.select_window(clone, wid(3));
        f.system.center_selected_column(clone);
        let old_x = f
            .system
            .logical_frames(clone, f.screen, &f.constraints, &f.gaps)
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
            .logical_frames(clone, wider, &f.constraints, &f.gaps)
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
            assert!(f.system.viewport_frames(f.layout).is_empty());
            assert!(!f.system.begin_viewport_gesture(f.layout));
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
        assert_eq!(system.viewport_frames(layout)[0].1.size.width, 600.0);
        system.add_window_after_selection(layout, wid(3));
        assert_ne!(system.container_tree(layout).children[1].node_id, 42);
    }
    #[test]
    fn gesture_edges_resist_without_losing_overscroll_or_reverse_motion() {
        let mut f = Fixture::new(4);
        let before = f.frame(1).origin.x;
        assert!(f.system.begin_viewport_gesture(f.layout));
        f.system.update_viewport_gesture(f.layout, -1000.0, Duration::from_millis(10));
        let dragged = f
            .system
            .viewport_frames(f.layout)
            .into_iter()
            .find(|(w, _)| *w == wid(1))
            .unwrap()
            .1
            .origin
            .x;
        assert!(
            dragged > before && dragged - before < 150.0,
            "edge motion must remain bounded: {dragged}"
        );
        assert_eq!(f.system.gesture_overscroll(f.layout), -1000.0);
        f.system.update_viewport_gesture(f.layout, 1020.0, Duration::from_millis(20));
        assert_eq!(f.system.gesture_overscroll(f.layout), 0.0);
        assert_eq!(f.frame(1).origin.x, before - 20.0);
    }
}
