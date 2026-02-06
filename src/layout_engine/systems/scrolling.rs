use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU64, Ordering};

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashSet;
use crate::common::config::{ScrollingFocusNavigationStyle, ScrollingLayoutSettings};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Column {
    windows: Vec<WindowId>,
    width_offset: f64,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct LayoutState {
    columns: Vec<Column>,
    selected: Option<WindowId>,
    column_width_ratio: f64,
    #[serde(skip, default = "default_atomic")]
    scroll_offset_px: AtomicU64,
    #[serde(skip, default = "default_atomic_bool")]
    pending_align: AtomicBool,
    #[serde(skip, default = "default_atomic_bool")]
    pending_center_align: AtomicBool,
    #[serde(skip, default = "default_atomic_i8")]
    pending_reveal_direction: AtomicI8,
    center_override_window: Option<WindowId>,
    #[serde(skip, default = "default_atomic")]
    last_screen_width: AtomicU64,
    #[serde(skip, default = "default_atomic")]
    last_gap_x: AtomicU64,
    #[serde(skip, default = "default_atomic")]
    last_step_px: AtomicU64,
    #[serde(skip, default = "default_atomic")]
    last_center_offset_delta_px: AtomicU64,
    #[serde(skip, default = "default_atomic")]
    overscroll_accumulation: AtomicU64,
    fullscreen: HashSet<WindowId>,
    fullscreen_within_gaps: HashSet<WindowId>,
}

impl LayoutState {
    fn new(column_width_ratio: f64) -> Self {
        Self {
            columns: Vec::new(),
            selected: None,
            column_width_ratio,
            scroll_offset_px: AtomicU64::new(0.0f64.to_bits()),
            pending_align: AtomicBool::new(false),
            pending_center_align: AtomicBool::new(false),
            pending_reveal_direction: AtomicI8::new(0),
            center_override_window: None,
            last_screen_width: AtomicU64::new(0.0f64.to_bits()),
            last_gap_x: AtomicU64::new(0.0f64.to_bits()),
            last_step_px: AtomicU64::new(0.0f64.to_bits()),
            last_center_offset_delta_px: AtomicU64::new(0.0f64.to_bits()),
            overscroll_accumulation: AtomicU64::new(0.0f64.to_bits()),
            fullscreen: HashSet::default(),
            fullscreen_within_gaps: HashSet::default(),
        }
    }

    fn first_window(&self) -> Option<WindowId> {
        self.columns.first().and_then(|c| c.windows.first()).copied()
    }

    fn locate(&self, wid: WindowId) -> Option<(usize, usize)> {
        for (col_idx, col) in self.columns.iter().enumerate() {
            for (row_idx, w) in col.windows.iter().enumerate() {
                if *w == wid {
                    return Some((col_idx, row_idx));
                }
            }
        }
        None
    }

    fn selected_location(&self) -> Option<(usize, usize)> {
        self.selected.and_then(|wid| self.locate(wid))
    }

    fn selected_or_first(&self) -> Option<WindowId> {
        self.selected.or_else(|| self.first_window())
    }

    fn align_scroll_to_selected(&mut self) {
        // Keep centered alignment only while the same selection remains focused.
        if self.center_override_window.is_some() && self.center_override_window == self.selected {
            self.pending_center_align.store(true, Ordering::Relaxed);
            self.pending_reveal_direction.store(0, Ordering::Relaxed);
            self.pending_align.store(false, Ordering::Relaxed);
            return;
        }
        self.center_override_window = None;
        self.pending_center_align.store(false, Ordering::Relaxed);
        self.pending_reveal_direction.store(0, Ordering::Relaxed);
        let Some((_col_idx, _)) = self.selected_location() else {
            self.scroll_offset_px.store(0.0f64.to_bits(), Ordering::Relaxed);
            return;
        };
        self.pending_align.store(true, Ordering::Relaxed);
    }

    fn request_center_on_selected(&mut self) {
        if self.selected_location().is_none() {
            return;
        }
        if self.center_override_window.is_some() && self.center_override_window == self.selected {
            // Toggle off when already centered on the same selection.
            self.center_override_window = None;
            self.pending_center_align.store(false, Ordering::Relaxed);
            self.pending_align.store(true, Ordering::Relaxed);
            self.pending_reveal_direction.store(0, Ordering::Relaxed);
        } else {
            self.center_override_window = self.selected;
            self.pending_center_align.store(true, Ordering::Relaxed);
            self.pending_align.store(false, Ordering::Relaxed);
            self.pending_reveal_direction.store(0, Ordering::Relaxed);
        }
    }

    fn reveal_selected_in_direction(&mut self, direction: Direction) {
        self.center_override_window = None;
        self.pending_center_align.store(false, Ordering::Relaxed);
        self.pending_align.store(false, Ordering::Relaxed);
        let dir_code = match direction {
            Direction::Left => -1,
            Direction::Right => 1,
            _ => 0,
        };
        self.pending_reveal_direction.store(dir_code, Ordering::Relaxed);
    }

    fn reveal_selected_without_direction(&mut self) {
        self.center_override_window = None;
        self.pending_center_align.store(false, Ordering::Relaxed);
        self.pending_align.store(false, Ordering::Relaxed);
        // 2 = neutral reveal: keep current offset unless selected would be clipped.
        self.pending_reveal_direction.store(2, Ordering::Relaxed);
    }

    fn clamp_scroll_offset(&mut self) {
        if self.columns.is_empty() {
            self.scroll_offset_px.store(0.0f64.to_bits(), Ordering::Relaxed);
            return;
        }
        // Keep the user's current strip position; final bounds clamping happens in
        // `calculate_layout` where full column geometry is available.
        self.pending_align.store(false, Ordering::Relaxed);
    }

    fn remove_window(&mut self, wid: WindowId) -> Option<WindowId> {
        let (col_idx, row_idx) = self.locate(wid)?;
        let col = &mut self.columns[col_idx];
        col.windows.remove(row_idx);
        if col.windows.is_empty() {
            self.columns.remove(col_idx);
        }
        self.fullscreen.remove(&wid);
        self.fullscreen_within_gaps.remove(&wid);

        if self.selected == Some(wid) {
            self.selected = None;
            if col_idx < self.columns.len() {
                let col = &self.columns[col_idx];
                if let Some(new_sel) = col.windows.get(row_idx).copied() {
                    self.selected = Some(new_sel);
                } else if let Some(new_sel) = col.windows.last().copied() {
                    self.selected = Some(new_sel);
                }
            }
            if self.selected.is_none() && col_idx > 0 {
                if let Some(new_sel) = self.columns[col_idx - 1].windows.last().copied() {
                    self.selected = Some(new_sel);
                }
            }
            if self.selected.is_none() {
                self.selected = self.first_window();
            }
        }
        if self.center_override_window == Some(wid) {
            self.center_override_window = None;
        }

        self.clamp_scroll_offset();
        self.selected
    }

    fn insert_column_after(&mut self, index: usize, wid: WindowId) {
        let column = Column {
            windows: vec![wid],
            width_offset: 0.0,
        };
        let insert_at = (index + 1).min(self.columns.len());
        self.columns.insert(insert_at, column);
        self.selected = Some(wid);
        self.align_scroll_to_selected();
    }

    fn insert_column_at_end(&mut self, wid: WindowId) {
        self.columns.push(Column {
            windows: vec![wid],
            width_offset: 0.0,
        });
        self.selected = Some(wid);
        self.align_scroll_to_selected();
    }

    fn move_window_to_column_end(&mut self, wid: WindowId, target_col: usize) {
        if let Some((col_idx, row_idx)) = self.locate(wid) {
            if col_idx == target_col {
                return;
            }
            let window = self.columns[col_idx].windows.remove(row_idx);
            let removed_column = self.columns[col_idx].windows.is_empty();
            if removed_column {
                self.columns.remove(col_idx);
            }
            let mut target = target_col;
            if removed_column && col_idx < target {
                target = target.saturating_sub(1);
            }
            target = target.min(self.columns.len());
            if target >= self.columns.len() {
                self.columns.push(Column {
                    windows: vec![window],
                    width_offset: 0.0,
                });
            } else {
                self.columns[target].windows.push(window);
            }
            self.selected = Some(window);
            self.align_scroll_to_selected();
        }
    }
}

impl Clone for LayoutState {
    fn clone(&self) -> Self {
        Self {
            columns: self.columns.clone(),
            selected: self.selected,
            column_width_ratio: self.column_width_ratio,
            scroll_offset_px: AtomicU64::new(self.scroll_offset_px.load(Ordering::Relaxed)),
            pending_align: AtomicBool::new(self.pending_align.load(Ordering::Relaxed)),
            pending_center_align: AtomicBool::new(
                self.pending_center_align.load(Ordering::Relaxed),
            ),
            pending_reveal_direction: AtomicI8::new(
                self.pending_reveal_direction.load(Ordering::Relaxed),
            ),
            center_override_window: self.center_override_window,
            last_screen_width: AtomicU64::new(self.last_screen_width.load(Ordering::Relaxed)),
            last_gap_x: AtomicU64::new(self.last_gap_x.load(Ordering::Relaxed)),
            last_step_px: AtomicU64::new(self.last_step_px.load(Ordering::Relaxed)),
            last_center_offset_delta_px: AtomicU64::new(
                self.last_center_offset_delta_px.load(Ordering::Relaxed),
            ),
            overscroll_accumulation: AtomicU64::new(
                self.overscroll_accumulation.load(Ordering::Relaxed),
            ),
            fullscreen: self.fullscreen.clone(),
            fullscreen_within_gaps: self.fullscreen_within_gaps.clone(),
        }
    }
}

fn default_atomic_bool() -> AtomicBool { AtomicBool::new(false) }
fn default_atomic_i8() -> AtomicI8 { AtomicI8::new(0) }
fn default_atomic() -> AtomicU64 { AtomicU64::new(0.0f64.to_bits()) }

#[derive(Serialize, Deserialize, Debug)]
pub struct ScrollingLayoutSystem {
    layouts: slotmap::SlotMap<LayoutId, LayoutState>,
    #[serde(skip, default = "default_scrolling_settings")]
    settings: ScrollingLayoutSettings,
}

fn default_scrolling_settings() -> ScrollingLayoutSettings { ScrollingLayoutSettings::default() }

impl Default for ScrollingLayoutSystem {
    fn default() -> Self {
        Self {
            layouts: Default::default(),
            settings: ScrollingLayoutSettings::default(),
        }
    }
}

impl ScrollingLayoutSystem {
    pub fn new(settings: &ScrollingLayoutSettings) -> Self {
        Self {
            layouts: Default::default(),
            settings: settings.clone(),
        }
    }

    pub fn update_settings(&mut self, settings: &ScrollingLayoutSettings) {
        self.settings = settings.clone();
    }

    fn clamp_ratio(&self, ratio: f64) -> f64 {
        ratio
            .clamp(
                self.settings.min_column_width_ratio,
                self.settings.max_column_width_ratio,
            )
            .max(0.05)
            .min(0.98)
    }

    fn clamp_ratio_with_bounds(ratio: f64, min_ratio: f64, max_ratio: f64) -> f64 {
        ratio.clamp(min_ratio, max_ratio).max(0.05).min(0.98)
    }

    fn column_widths_and_starts(
        state: &LayoutState,
        screen_width: f64,
        gap_x: f64,
        min_ratio: f64,
        max_ratio: f64,
    ) -> (Vec<f64>, Vec<f64>) {
        let base_ratio =
            Self::clamp_ratio_with_bounds(state.column_width_ratio, min_ratio, max_ratio);
        let mut widths = Vec::with_capacity(state.columns.len());
        let mut starts = Vec::with_capacity(state.columns.len());
        let mut cursor = 0.0;
        for col in &state.columns {
            starts.push(cursor);
            let ratio =
                Self::clamp_ratio_with_bounds(base_ratio + col.width_offset, min_ratio, max_ratio);
            let width = (screen_width * ratio).max(1.0);
            widths.push(width);
            cursor += width + gap_x;
        }
        (widths, starts)
    }

    pub fn scroll_by_delta(&mut self, layout: LayoutId, delta: f64) -> Option<Direction> {
        let min_ratio = self.settings.min_column_width_ratio;
        let max_ratio = self.settings.max_column_width_ratio;
        let threshold = self.settings.gestures.workspace_switch_threshold;
        let Some(state) = self.layout_state_mut(layout) else {
            return None;
        };
        let screen_width = f64::from_bits(state.last_screen_width.load(Ordering::Relaxed));
        let gap_x = f64::from_bits(state.last_gap_x.load(Ordering::Relaxed));
        if screen_width <= 0.0 {
            return None;
        }
        let (widths, starts) =
            Self::column_widths_and_starts(state, screen_width, gap_x, min_ratio, max_ratio);
        if starts.is_empty() {
            return None;
        }
        let selected_idx = state.selected_location().map(|(idx, _)| idx).unwrap_or(0);
        let step = widths.get(selected_idx).copied().unwrap_or(1.0) + gap_x;
        if step <= 0.0 {
            return None;
        }
        let base_max_offset = starts.last().copied().unwrap_or(0.0);
        let center_offset_delta =
            f64::from_bits(state.last_center_offset_delta_px.load(Ordering::Relaxed));
        let (min_offset, max_offset) = if state.center_override_window.is_some() {
            (center_offset_delta, base_max_offset + center_offset_delta)
        } else {
            (0.0, base_max_offset)
        };
        let current = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
        let next_raw = current + delta * step;
        let next = next_raw.clamp(min_offset, max_offset);
        state.scroll_offset_px.store(next.to_bits(), Ordering::Relaxed);

        if next_raw < min_offset && delta < 0.0 {
            let overscroll = (min_offset - next_raw) / step;
            let accum =
                f64::from_bits(state.overscroll_accumulation.load(Ordering::Relaxed)) + overscroll;
            if accum >= threshold {
                state.overscroll_accumulation.store(0.0f64.to_bits(), Ordering::Relaxed);
                Some(Direction::Left)
            } else {
                state.overscroll_accumulation.store(accum.to_bits(), Ordering::Relaxed);
                None
            }
        } else if next_raw > max_offset && delta > 0.0 {
            let overscroll = (next_raw - max_offset) / step;
            let accum =
                f64::from_bits(state.overscroll_accumulation.load(Ordering::Relaxed)) + overscroll;
            if accum >= threshold {
                state.overscroll_accumulation.store(0.0f64.to_bits(), Ordering::Relaxed);
                Some(Direction::Right)
            } else {
                state.overscroll_accumulation.store(accum.to_bits(), Ordering::Relaxed);
                None
            }
        } else {
            state.overscroll_accumulation.store(0.0f64.to_bits(), Ordering::Relaxed);
            None
        }
    }

    pub fn snap_to_nearest_column(&mut self, layout: LayoutId) {
        let min_ratio = self.settings.min_column_width_ratio;
        let max_ratio = self.settings.max_column_width_ratio;
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let screen_width = f64::from_bits(state.last_screen_width.load(Ordering::Relaxed));
        let gap_x = f64::from_bits(state.last_gap_x.load(Ordering::Relaxed));
        if screen_width <= 0.0 {
            return;
        }
        let (_widths, starts) =
            Self::column_widths_and_starts(state, screen_width, gap_x, min_ratio, max_ratio);
        if starts.is_empty() {
            return;
        }
        let base_max_offset = starts.last().copied().unwrap_or(0.0);
        let center_offset_delta =
            f64::from_bits(state.last_center_offset_delta_px.load(Ordering::Relaxed));
        let (min_offset, max_offset, baseline) = if state.center_override_window.is_some() {
            (
                center_offset_delta,
                base_max_offset + center_offset_delta,
                center_offset_delta,
            )
        } else {
            (0.0, base_max_offset, 0.0)
        };
        let current = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
        let strip_offset = current - baseline;
        let target = starts
            .iter()
            .min_by(|a, b| {
                let da = (*a - strip_offset).abs();
                let db = (*b - strip_offset).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
            .unwrap_or(0.0);
        let next = (baseline + target).clamp(min_offset, max_offset);
        state.scroll_offset_px.store(next.to_bits(), Ordering::Relaxed);
    }

    pub fn center_selected_column(&mut self, layout: LayoutId) {
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        state.request_center_on_selected();
    }

    fn layout_state(&self, layout: LayoutId) -> Option<&LayoutState> { self.layouts.get(layout) }

    fn layout_state_mut(&mut self, layout: LayoutId) -> Option<&mut LayoutState> {
        self.layouts.get_mut(layout)
    }

    fn move_focus_vertical(state: &mut LayoutState, dir: Direction) -> Option<WindowId> {
        let (col_idx, row_idx) = state.selected_location()?;
        let column = &state.columns[col_idx];
        if column.windows.is_empty() {
            return None;
        }
        let new_idx = match dir {
            Direction::Up => row_idx.checked_sub(1)?,
            Direction::Down => (row_idx + 1 < column.windows.len()).then_some(row_idx + 1)?,
            _ => return None,
        };
        let new_sel = column.windows[new_idx];
        state.selected = Some(new_sel);
        Some(new_sel)
    }

    fn move_focus_horizontal(state: &mut LayoutState, dir: Direction) -> Option<WindowId> {
        let (col_idx, row_idx) = state.selected_location()?;
        let target_col = match dir {
            Direction::Left => col_idx.checked_sub(1)?,
            Direction::Right => (col_idx + 1 < state.columns.len()).then_some(col_idx + 1)?,
            _ => return None,
        };
        let target_column = &state.columns[target_col];
        if target_column.windows.is_empty() {
            return None;
        }
        let target_row = row_idx.min(target_column.windows.len() - 1);
        let new_sel = target_column.windows[target_row];
        state.selected = Some(new_sel);
        Some(new_sel)
    }

    fn move_selected_window_vertical(state: &mut LayoutState, dir: Direction) -> bool {
        let (col_idx, row_idx) = match state.selected_location() {
            Some(loc) => loc,
            None => return false,
        };
        let column = &mut state.columns[col_idx];
        let target_idx = match dir {
            Direction::Up => row_idx.checked_sub(1),
            Direction::Down => (row_idx + 1 < column.windows.len()).then_some(row_idx + 1),
            _ => None,
        };
        let Some(target_idx) = target_idx else { return false };
        column.windows.swap(row_idx, target_idx);
        state.selected = Some(column.windows[target_idx]);
        true
    }

    fn move_selected_window_horizontal(state: &mut LayoutState, dir: Direction) -> bool {
        let (col_idx, row_idx) = match state.selected_location() {
            Some(loc) => loc,
            None => return false,
        };
        // If the current column is stacked, horizontal move should extract the selected
        // window into its own neighbor column. This is a faster way to undo accidental stacks.
        if state.columns[col_idx].windows.len() > 1 {
            let wid = state.columns[col_idx].windows.remove(row_idx);
            let insert_at = match dir {
                Direction::Left => col_idx,
                Direction::Right => (col_idx + 1).min(state.columns.len()),
                _ => return false,
            };
            state.columns.insert(insert_at, Column {
                windows: vec![wid],
                width_offset: 0.0,
            });
            state.selected = Some(wid);
            return true;
        }

        let target_col = match dir {
            Direction::Left => col_idx.checked_sub(1),
            Direction::Right => (col_idx + 1 < state.columns.len()).then_some(col_idx + 1),
            _ => None,
        };
        let Some(target_col) = target_col else { return false };
        state.columns.swap(col_idx, target_col);
        let Some(selected) = state.selected else { return false };
        state.selected = Some(selected);
        true
    }

    fn all_windows(state: &LayoutState) -> Vec<WindowId> {
        state.columns.iter().flat_map(|c| c.windows.iter().copied()).collect()
    }
}

impl LayoutSystem for ScrollingLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        self.layouts.insert(LayoutState::new(self.settings.column_width_ratio))
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let cloned = self
            .layouts
            .get(layout)
            .cloned()
            .unwrap_or_else(|| LayoutState::new(self.settings.column_width_ratio));
        self.layouts.insert(cloned)
    }

    fn remove_layout(&mut self, layout: LayoutId) { self.layouts.remove(layout); }

    fn draw_tree(&self, layout: LayoutId) -> String {
        let Some(state) = self.layouts.get(layout) else {
            return String::new();
        };
        let mut out = String::new();
        for (idx, col) in state.columns.iter().enumerate() {
            out.push_str(&format!("Column {idx}:"));
            for wid in &col.windows {
                if Some(*wid) == state.selected {
                    out.push_str(&format!(" [*{:?}]", wid));
                } else {
                    out.push_str(&format!(" [{:?}]", wid));
                }
            }
            out.push('\n');
        }
        out
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        _stack_line_thickness: f64,
        _stack_line_horiz: crate::common::config::HorizontalPlacement,
        _stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let Some(state) = self.layouts.get(layout) else {
            return Vec::new();
        };
        let tiling = compute_tiling_area(screen, gaps);
        let gap_x = gaps.inner.horizontal;
        let gap_y = gaps.inner.vertical;
        let base_ratio = self.clamp_ratio(state.column_width_ratio);

        let mut column_ratios = Vec::with_capacity(state.columns.len());
        let mut column_widths = Vec::with_capacity(state.columns.len());
        for col in state.columns.iter() {
            let ratio = self.clamp_ratio(base_ratio + col.width_offset);
            column_ratios.push(ratio);
            column_widths.push((tiling.size.width * ratio).max(1.0));
        }

        let mut column_starts = Vec::with_capacity(state.columns.len());
        let mut strip_cursor = 0.0;
        for width in &column_widths {
            column_starts.push(strip_cursor);
            strip_cursor += *width + gap_x;
        }
        let strip_max_offset = column_starts.last().copied().unwrap_or(0.0);
        let selected_col_idx = state.selected_location().map(|(idx, _)| idx).unwrap_or(0);
        let selected_width = column_widths
            .get(selected_col_idx)
            .copied()
            .unwrap_or((tiling.size.width * base_ratio).max(1.0));
        let step = selected_width + gap_x;
        state.last_screen_width.store(tiling.size.width.to_bits(), Ordering::Relaxed);
        state.last_gap_x.store(gap_x.to_bits(), Ordering::Relaxed);
        state.last_step_px.store(step.to_bits(), Ordering::Relaxed);

        let niri_navigation = matches!(
            self.settings.focus_navigation_style,
            ScrollingFocusNavigationStyle::Niri
        );
        let anchor_x = if niri_navigation && state.center_override_window.is_none() {
            // Keep strip anchoring stable in niri mode so focus changes do not
            // shift unrelated columns when selected widths differ.
            tiling.origin.x
        } else {
            match self.settings.alignment {
                crate::common::config::ScrollingAlignment::Left => tiling.origin.x,
                crate::common::config::ScrollingAlignment::Center => {
                    tiling.origin.x + (tiling.size.width - selected_width) / 2.0
                }
                crate::common::config::ScrollingAlignment::Right => {
                    tiling.origin.x + tiling.size.width - selected_width
                }
            }
        };
        let center_anchor_x = tiling.origin.x + (tiling.size.width - selected_width) / 2.0;
        let center_offset_delta = anchor_x - center_anchor_x;
        state
            .last_center_offset_delta_px
            .store(center_offset_delta.to_bits(), Ordering::Relaxed);

        if state.pending_center_align.load(Ordering::Relaxed) {
            let offset = state
                .selected_location()
                .map(|(col_idx, _)| {
                    center_offset_delta + column_starts.get(col_idx).copied().unwrap_or(0.0)
                })
                .unwrap_or(0.0);
            state.scroll_offset_px.store(offset.to_bits(), Ordering::Relaxed);
            state.pending_center_align.store(false, Ordering::Relaxed);
            state.pending_align.store(false, Ordering::Relaxed);
        } else if state.pending_align.load(Ordering::Relaxed) {
            let offset = state
                .selected_location()
                .map(|(col_idx, _)| column_starts.get(col_idx).copied().unwrap_or(0.0))
                .unwrap_or(0.0);
            state.scroll_offset_px.store(offset.to_bits(), Ordering::Relaxed);
            state.pending_align.store(false, Ordering::Relaxed);
        }
        let reveal_direction = state.pending_reveal_direction.swap(0, Ordering::Relaxed);
        if reveal_direction != 0 {
            if let Some((selected_col_idx, _)) = state.selected_location() {
                let selected_width = column_widths
                    .get(selected_col_idx)
                    .copied()
                    .unwrap_or((tiling.size.width * base_ratio).max(1.0));
                let mut offset = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
                let selected_start = column_starts.get(selected_col_idx).copied().unwrap_or(0.0);
                let selected_x = anchor_x + selected_start - offset;
                let visible_left = tiling.origin.x;
                let visible_right = tiling.origin.x + tiling.size.width;

                match reveal_direction {
                    -1 => {
                        if selected_x < visible_left {
                            offset = anchor_x + selected_start - visible_left;
                        } else if selected_x + selected_width > visible_right {
                            offset = anchor_x + selected_start + selected_width - visible_right;
                        }
                    }
                    1 => {
                        if selected_x + selected_width > visible_right {
                            offset = anchor_x + selected_start + selected_width - visible_right;
                        } else if selected_x < visible_left {
                            offset = anchor_x + selected_start - visible_left;
                        }
                    }
                    2 => {
                        if selected_x < visible_left {
                            offset = anchor_x + selected_start - visible_left;
                        } else if selected_x + selected_width > visible_right {
                            offset = anchor_x + selected_start + selected_width - visible_right;
                        }
                    }
                    _ => {}
                }
                state.scroll_offset_px.store(offset.to_bits(), Ordering::Relaxed);
            }
        }
        let current = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
        let base_max_offset = strip_max_offset;
        let (min_offset, max_offset) = if state.center_override_window.is_some() {
            (center_offset_delta, base_max_offset + center_offset_delta)
        } else {
            (0.0, base_max_offset)
        };
        let clamped = current.clamp(min_offset, max_offset);
        state.scroll_offset_px.store(clamped.to_bits(), Ordering::Relaxed);

        let mut out = Vec::new();
        for (col_idx, col) in state.columns.iter().enumerate() {
            let offset = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
            let ratio = column_ratios.get(col_idx).copied().unwrap_or(base_ratio);
            let column_width = (tiling.size.width * ratio).max(1.0);
            let start = column_starts.get(col_idx).copied().unwrap_or(0.0);
            let x = anchor_x + start - offset;
            if col.windows.is_empty() {
                continue;
            }
            let total_gap = gap_y * (col.windows.len().saturating_sub(1) as f64);
            let available_height = (tiling.size.height - total_gap).max(0.0);
            let row_height = if col.windows.is_empty() {
                0.0
            } else {
                (available_height / col.windows.len() as f64).max(1.0)
            };

            for (row_idx, wid) in col.windows.iter().enumerate() {
                let y = tiling.origin.y + (row_idx as f64) * (row_height + gap_y);
                // round position and size independently to avoid size jitter from min/max rounding.
                let mut frame = CGRect::new(
                    CGPoint::new(x.round(), y.round()),
                    CGSize::new(column_width.round(), row_height.round()),
                );
                if state.fullscreen.contains(wid) {
                    frame = screen;
                } else if state.fullscreen_within_gaps.contains(wid) {
                    frame = tiling;
                }
                out.push((*wid, frame));
            }
        }
        out
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.layout_state(layout).and_then(|state| state.selected_or_first())
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layout_state(layout).map(Self::all_windows).unwrap_or_default()
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        let Some(state) = self.layout_state(layout) else {
            return Vec::new();
        };
        let Some((col_idx, _)) = state.selected_location() else {
            return Vec::new();
        };
        state.columns[col_idx].windows.clone()
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool {
        let Some(state) = self.layout_state_mut(layout) else {
            return false;
        };
        Self::move_focus_vertical(state, Direction::Up).is_some()
    }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        let Some(state) = self.layout_state_mut(layout) else {
            return false;
        };
        Self::move_focus_vertical(state, Direction::Down).is_some()
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let niri_navigation = matches!(
            self.settings.focus_navigation_style,
            ScrollingFocusNavigationStyle::Niri
        );
        let Some(state) = self.layout_state_mut(layout) else {
            return (None, vec![]);
        };
        let new_sel = match direction {
            Direction::Left | Direction::Right => Self::move_focus_horizontal(state, direction),
            Direction::Up | Direction::Down => Self::move_focus_vertical(state, direction),
        };
        if new_sel.is_some() && niri_navigation {
            if matches!(direction, Direction::Left | Direction::Right) {
                state.reveal_selected_in_direction(direction);
            } else {
                state.reveal_selected_without_direction();
            }
        } else {
            state.align_scroll_to_selected();
        }
        let raise = state
            .selected_location()
            .map(|(col_idx, _)| state.columns[col_idx].windows.clone())
            .unwrap_or_default();
        (new_sel, raise)
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        let state = self.layout_state(layout)?;
        let (col_idx, row_idx) = state.selected_location()?;
        match direction {
            Direction::Left => {
                let target = col_idx.checked_sub(1)?;
                state.columns.get(target).and_then(|col| {
                    col.windows.get(row_idx.min(col.windows.len().saturating_sub(1))).copied()
                })
            }
            Direction::Right => {
                let target = col_idx + 1;
                state.columns.get(target).and_then(|col| {
                    col.windows.get(row_idx.min(col.windows.len().saturating_sub(1))).copied()
                })
            }
            Direction::Up => {
                state.columns.get(col_idx)?.windows.get(row_idx.checked_sub(1)?).copied()
            }
            Direction::Down => state.columns.get(col_idx)?.windows.get(row_idx + 1).copied(),
        }
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        if let Some((col_idx, _)) = state.selected_location() {
            state.insert_column_after(col_idx, wid);
        } else if !state.columns.is_empty() {
            state.insert_column_after(0, wid);
        } else {
            state.insert_column_at_end(wid);
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        for state in self.layouts.values_mut() {
            let _ = state.remove_window(wid);
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        for state in self.layouts.values_mut() {
            let windows: Vec<_> = state
                .columns
                .iter()
                .flat_map(|c| c.windows.iter().copied())
                .filter(|w| w.pid == pid)
                .collect();
            for wid in windows {
                let _ = state.remove_window(wid);
            }
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let mut desired = desired;
        desired.sort_unstable();
        let current: Vec<_> = state
            .columns
            .iter()
            .flat_map(|c| c.windows.iter().copied())
            .filter(|w| w.pid == pid)
            .collect();
        let mut current = current;
        current.sort_unstable();
        let mut desired_iter = desired.iter().peekable();
        let mut current_iter = current.iter().peekable();
        loop {
            match (desired_iter.peek(), current_iter.peek()) {
                (Some(des), Some(cur)) if des == cur => {
                    desired_iter.next();
                    current_iter.next();
                }
                (Some(des), None) => {
                    state.insert_column_at_end(**des);
                    desired_iter.next();
                }
                (Some(des), Some(cur)) if des < cur => {
                    state.insert_column_at_end(**des);
                    desired_iter.next();
                }
                (_, Some(cur)) => {
                    let _ = state.remove_window(**cur);
                    current_iter.next();
                }
                (None, None) => break,
            }
        }
    }

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.layout_state(layout)
            .map(|state| state.columns.iter().flat_map(|c| c.windows.iter()).any(|w| w.pid == pid))
            .unwrap_or(false)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.layout_state(layout)
            .map(|state| state.locate(wid).is_some())
            .unwrap_or(false)
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        let niri_navigation = matches!(
            self.settings.focus_navigation_style,
            ScrollingFocusNavigationStyle::Niri
        );
        let Some(state) = self.layout_state_mut(layout) else {
            return false;
        };
        if state.locate(wid).is_some() {
            // refocusing the same centered window should keep the center override
            if state.selected == Some(wid) && state.center_override_window == Some(wid) {
                return true;
            }
            state.selected = Some(wid);
            if niri_navigation {
                state.reveal_selected_without_direction();
            } else {
                state.align_scroll_to_selected();
            }
            true
        } else {
            false
        }
    }

    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        _old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) {
        let min_ratio = self.settings.min_column_width_ratio;
        let max_ratio = self.settings.max_column_width_ratio;
        let niri_navigation = matches!(
            self.settings.focus_navigation_style,
            ScrollingFocusNavigationStyle::Niri
        );

        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        if state.selected != Some(wid) {
            return;
        }
        let tiling = compute_tiling_area(screen, gaps);
        if tiling.size.width <= 0.0 {
            return;
        }
        let ratio = new_frame.size.width / tiling.size.width;
        let clamped = ratio.clamp(min_ratio, max_ratio).max(0.05).min(0.98);

        let base_ratio = state.column_width_ratio;
        if let Some((col_idx, _)) = state.selected_location() {
            state.columns[col_idx].width_offset = clamped - base_ratio;
            if niri_navigation {
                state.reveal_selected_without_direction();
            } else {
                state.align_scroll_to_selected();
            }
        } else {
            state.column_width_ratio = clamped;
        }
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        let Some(state) = self.layout_state_mut(layout) else {
            return false;
        };
        let (a_col, a_row) = match state.locate(a) {
            Some(loc) => loc,
            None => return false,
        };
        let (b_col, b_row) = match state.locate(b) {
            Some(loc) => loc,
            None => return false,
        };
        if a_col == b_col {
            state.columns[a_col].windows.swap(a_row, b_row);
        } else {
            let a_window = state.columns[a_col].windows[a_row];
            let b_window = state.columns[b_col].windows[b_row];
            state.columns[a_col].windows[a_row] = b_window;
            state.columns[b_col].windows[b_row] = a_window;
        }
        true
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(state) = self.layout_state_mut(layout) else {
            return false;
        };
        let moved = match direction {
            Direction::Left | Direction::Right => {
                Self::move_selected_window_horizontal(state, direction)
            }
            Direction::Up | Direction::Down => {
                Self::move_selected_window_vertical(state, direction)
            }
        };
        if moved {
            state.align_scroll_to_selected();
        }
        moved
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        let Some(selected) = self.selected_window(from_layout) else {
            return;
        };
        if let Some(state) = self.layout_state_mut(from_layout) {
            state.remove_window(selected);
            state.align_scroll_to_selected();
        }
        if let Some(state) = self.layout_state_mut(to_layout) {
            if let Some((col_idx, _)) = state.selected_location() {
                state.insert_column_after(col_idx, selected);
            } else {
                state.insert_column_at_end(selected);
            }
            state.align_scroll_to_selected();
        }
    }

    fn split_selection(&mut self, _layout: LayoutId, _kind: LayoutKind) {
        // Not applicable for scrolling layout.
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        let Some(state) = self.layout_state_mut(layout) else {
            return Vec::new();
        };
        let Some(selected) = state.selected_or_first() else {
            return Vec::new();
        };
        if state.fullscreen.remove(&selected) {
            return vec![selected];
        }
        state.fullscreen_within_gaps.remove(&selected);
        state.fullscreen.insert(selected);
        vec![selected]
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        let Some(state) = self.layout_state_mut(layout) else {
            return Vec::new();
        };
        let Some(selected) = state.selected_or_first() else {
            return Vec::new();
        };
        if state.fullscreen_within_gaps.remove(&selected) {
            return vec![selected];
        }
        state.fullscreen.remove(&selected);
        state.fullscreen_within_gaps.insert(selected);
        vec![selected]
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        let Some(state) = self.layout_state(layout) else {
            return false;
        };
        !state.fullscreen.is_empty() || !state.fullscreen_within_gaps.is_empty()
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let Some(selected) = state.selected else { return };
        let (col_idx, _) = match state.selected_location() {
            Some(loc) => loc,
            None => return,
        };
        let target_col = match direction {
            Direction::Left => col_idx.checked_sub(1),
            Direction::Right => (col_idx + 1 < state.columns.len()).then_some(col_idx + 1),
            _ => None,
        };
        let Some(target_col) = target_col else { return };
        state.move_window_to_column_end(selected, target_col);
    }

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let Some(state) = self.layout_state_mut(layout) else {
            return Vec::new();
        };
        let (col_idx, _) = match state.selected_location() {
            Some(loc) => loc,
            None => return Vec::new(),
        };
        let target_col = if col_idx + 1 < state.columns.len() {
            col_idx + 1
        } else if col_idx > 0 {
            col_idx - 1
        } else {
            return Vec::new();
        };
        let moved_windows = state.columns[target_col].windows.clone();
        if moved_windows.is_empty() {
            return Vec::new();
        }
        for wid in moved_windows.iter().copied() {
            state.move_window_to_column_end(wid, col_idx);
        }
        moved_windows
    }

    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let Some(state) = self.layout_state_mut(layout) else {
            return Vec::new();
        };
        let (col_idx, row_idx) = match state.selected_location() {
            Some(loc) => loc,
            None => return Vec::new(),
        };
        if state.columns[col_idx].windows.len() <= 1 {
            return Vec::new();
        }
        let selected = state.columns[col_idx].windows[row_idx];
        let mut moved = Vec::new();
        let mut remaining = Vec::new();
        for wid in state.columns[col_idx].windows.drain(..) {
            if wid == selected {
                remaining.push(wid);
            } else {
                moved.push(wid);
            }
        }
        state.columns[col_idx].windows = remaining;
        let mut insert_at = col_idx + 1;
        for wid in moved.iter().copied() {
            state.columns.insert(insert_at, Column {
                windows: vec![wid],
                width_offset: 0.0,
            });
            insert_at += 1;
        }
        moved
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        let Some(state) = self.layout_state(layout) else {
            return false;
        };
        let Some((col_idx, _)) = state.selected_location() else {
            return false;
        };
        state.columns[col_idx].windows.len() > 1
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let (col_idx, row_idx) = match state.selected_location() {
            Some(loc) => loc,
            None => return,
        };
        if state.columns[col_idx].windows.len() <= 1 {
            return;
        }
        let wid = state.columns[col_idx].windows.remove(row_idx);
        let insert_at = (col_idx + 1).min(state.columns.len());
        state.columns.insert(insert_at, Column {
            windows: vec![wid],
            width_offset: 0.0,
        });
        state.selected = Some(wid);
        state.align_scroll_to_selected();
        state.clamp_scroll_offset();
    }

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        let min_ratio = self.settings.min_column_width_ratio;
        let max_ratio = self.settings.max_column_width_ratio;
        let niri_navigation = matches!(
            self.settings.focus_navigation_style,
            ScrollingFocusNavigationStyle::Niri
        );
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let base_ratio = state.column_width_ratio;

        let Some((col_idx, _)) = state.selected_location() else {
            let ratio = base_ratio + amount;
            state.column_width_ratio = ratio.clamp(min_ratio, max_ratio).max(0.05).min(0.98);
            return;
        };

        let current = base_ratio + state.columns[col_idx].width_offset;
        let next = current + amount;
        let clamped = next.clamp(min_ratio, max_ratio).max(0.05).min(0.98);
        state.columns[col_idx].width_offset = clamped - base_ratio;
        if niri_navigation {
            state.reveal_selected_without_direction();
        } else {
            state.align_scroll_to_selected();
        }
    }

    fn rebalance(&mut self, _layout: LayoutId) {}

    fn toggle_tile_orientation(&mut self, _layout: LayoutId) {}
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::ScrollingLayoutSystem;
    use crate::actor::app::{WindowId, pid_t};
    use crate::common::config::{GapSettings, ScrollingLayoutSettings};
    use crate::layout_engine::systems::LayoutSystem;
    use crate::layout_engine::utils::compute_tiling_area;
    use crate::layout_engine::{Direction, LayoutId};

    fn wid(pid: pid_t, idx: u32) -> WindowId {
        WindowId {
            pid,
            idx: std::num::NonZeroU32::new(idx).unwrap(),
        }
    }

    fn screen(width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, height))
    }

    fn render(
        system: &ScrollingLayoutSystem,
        layout: LayoutId,
        screen: CGRect,
        gaps: &GapSettings,
    ) -> Vec<(WindowId, CGRect)> {
        system.calculate_layout(
            layout,
            screen,
            0.0,
            gaps,
            0.0,
            Default::default(),
            Default::default(),
        )
    }

    fn frame_for(frames: &[(WindowId, CGRect)], wid: WindowId) -> CGRect {
        frames
            .iter()
            .find(|(id, _)| *id == wid)
            .map(|(_, frame)| *frame)
            .expect("missing frame")
    }

    fn scroll_offset(system: &ScrollingLayoutSystem, layout: LayoutId) -> f64 {
        f64::from_bits(
            system
                .layouts
                .get(layout)
                .expect("layout state missing")
                .scroll_offset_px
                .load(Ordering::Relaxed),
        )
    }

    fn setup_two_windows(
        settings: ScrollingLayoutSettings,
    ) -> (ScrollingLayoutSystem, LayoutId, WindowId, WindowId) {
        let mut system = ScrollingLayoutSystem::new(&settings);
        let layout = system.create_layout();
        let w1 = wid(1, 1);
        let w2 = wid(1, 2);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        (system, layout, w1, w2)
    }

    #[test]
    fn creates_columns_and_moves_focus() {
        let mut system = ScrollingLayoutSystem::new(&ScrollingLayoutSettings::default());
        let layout = system.create_layout();
        let w1 = wid(1, 1);
        let w2 = wid(1, 2);
        let w3 = wid(1, 3);

        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        assert_eq!(system.visible_windows_in_layout(layout).len(), 3);
        assert_eq!(system.selected_window(layout), Some(w3));

        let (focus, _) = system.move_focus(layout, Direction::Left);
        assert_eq!(focus, Some(w2));
    }

    #[test]
    fn move_selection_swaps_columns_horizontally() {
        let mut system = ScrollingLayoutSystem::new(&ScrollingLayoutSettings::default());
        let layout = system.create_layout();
        let w1 = wid(1, 1);
        let w2 = wid(1, 2);
        let w3 = wid(1, 3);

        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        assert!(system.move_selection(layout, Direction::Left));

        let state = system.layouts.get(layout).expect("layout state missing");
        assert_eq!(state.columns.len(), 3);
        assert_eq!(state.columns[1].windows, vec![w3]);
        assert_eq!(state.columns[2].windows, vec![w2]);
    }

    #[test]
    fn calculates_centered_columns() {
        let (system, layout, _, _) = setup_two_windows(ScrollingLayoutSettings::default());
        let frames = render(&system, layout, screen(1000.0, 800.0), &GapSettings::default());

        assert_eq!(frames.len(), 2);
        let width0 = frames[0].1.size.width;
        let width1 = frames[1].1.size.width;
        assert!(
            width0 > 1.0 && width1 > 1.0 && (width0 - width1).abs() < 1.0,
            "expected equal non-zero widths, got w0={}, w1={}",
            width0,
            width1
        );
    }

    #[test]
    fn centers_selected_column_without_changing_alignment() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Left;
        let (mut system, layout, _, w2) = setup_two_windows(settings);
        system.center_selected_column(layout);

        let screen = screen(1000.0, 800.0);
        let gaps = GapSettings::default();
        let frames = render(&system, layout, screen, &gaps);

        let tiling = compute_tiling_area(screen, &gaps);
        let selected_frame = frame_for(&frames, w2);

        let column_width = selected_frame.size.width;
        let expected_x = tiling.origin.x + (tiling.size.width - column_width) / 2.0;

        assert!(
            (selected_frame.origin.x - expected_x.round()).abs() < 1.0,
            "expected centered x={}, got x={}",
            expected_x.round(),
            selected_frame.origin.x
        );
    }

    #[test]
    fn center_selection_clears_when_focus_moves() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Left;
        let (mut system, layout, _, _) = setup_two_windows(settings);
        system.center_selected_column(layout);
        let _ = system.move_focus(layout, Direction::Left);

        let state = system.layouts.get(layout).expect("layout state missing");
        assert_eq!(state.center_override_window, None);
    }

    #[test]
    fn center_selection_toggles_back_to_layout_alignment() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Left;
        let (mut system, layout, _, w2) = setup_two_windows(settings);

        // First call centers the current selection.
        system.center_selected_column(layout);
        // Second call on the same selection toggles centering off.
        system.center_selected_column(layout);

        let screen = screen(1000.0, 800.0);
        let gaps = GapSettings::default();
        let frames = render(&system, layout, screen, &gaps);
        let tiling = compute_tiling_area(screen, &gaps);
        let selected_frame = frame_for(&frames, w2);
        assert!(
            (selected_frame.origin.x - tiling.origin.x.round()).abs() < 1.0,
            "expected left-aligned x={}, got x={}",
            tiling.origin.x.round(),
            selected_frame.origin.x
        );
    }

    #[test]
    fn horizontal_focus_keeps_side_by_side_columns_visible_without_anchor_snapping() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Left;
        settings.focus_navigation_style =
            crate::common::config::ScrollingFocusNavigationStyle::Niri;
        settings.column_width_ratio = 0.45;
        settings.min_column_width_ratio = 0.2;
        settings.max_column_width_ratio = 0.9;
        let (mut system, layout, w1, w2) = setup_two_windows(settings);

        let screen = screen(1000.0, 800.0);
        let gaps = GapSettings::default();

        // Apply the default initial alignment (selected = w2) so w1 starts off-screen.
        let _ = render(&system, layout, screen, &gaps);

        let _ = system.move_focus(layout, Direction::Left);
        let left_frames = render(&system, layout, screen, &gaps);
        let offset_after_left = scroll_offset(&system, layout);

        let _ = system.move_focus(layout, Direction::Right);
        let right_frames = render(&system, layout, screen, &gaps);
        let offset_after_right = scroll_offset(&system, layout);

        let w1_x_after_left = frame_for(&left_frames, w1).origin.x;
        let w2_x_after_right = frame_for(&right_frames, w2).origin.x;

        assert!(
            (offset_after_left - offset_after_right).abs() < 1.0,
            "expected no snap when toggling focus between visible columns, got offsets {} -> {}",
            offset_after_left,
            offset_after_right
        );
        assert!(
            w1_x_after_left >= -1.0 && w2_x_after_right >= -1.0,
            "expected side-by-side visibility, got x positions w1={}, w2={}",
            w1_x_after_left,
            w2_x_after_right
        );
    }

    #[test]
    fn horizontal_focus_anchored_snaps_to_alignment() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Left;
        settings.focus_navigation_style =
            crate::common::config::ScrollingFocusNavigationStyle::Anchored;
        settings.column_width_ratio = 0.45;
        settings.min_column_width_ratio = 0.2;
        settings.max_column_width_ratio = 0.9;
        let (mut system, layout, _, _) = setup_two_windows(settings);

        let screen = screen(1000.0, 800.0);
        let gaps = GapSettings::default();
        let _ = render(&system, layout, screen, &gaps);

        let _ = system.move_focus(layout, Direction::Left);
        let _ = render(&system, layout, screen, &gaps);
        let offset_after_left = scroll_offset(&system, layout);

        let _ = system.move_focus(layout, Direction::Right);
        let _ = render(&system, layout, screen, &gaps);
        let offset_after_right = scroll_offset(&system, layout);

        assert!(
            (offset_after_left - offset_after_right).abs() > 1.0,
            "expected anchored mode to snap offset on focus changes, got offsets {} -> {}",
            offset_after_left,
            offset_after_right
        );
    }

    #[test]
    fn resized_columns_remain_contiguous_without_horizontal_holes() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Left;
        settings.focus_navigation_style =
            crate::common::config::ScrollingFocusNavigationStyle::Anchored;
        let (mut system, layout, w1, w2) = setup_two_windows(settings);
        let _ = system.move_focus(layout, Direction::Left);
        system.resize_selection_by(layout, 0.12);

        let gaps = GapSettings::default();
        let frames = render(&system, layout, screen(1000.0, 800.0), &gaps);

        let w1_frame = frame_for(&frames, w1);
        let w2_frame = frame_for(&frames, w2);

        let expected_w2_x = w1_frame.origin.x + w1_frame.size.width + gaps.inner.horizontal;
        assert!(
            (w2_frame.origin.x - expected_w2_x).abs() < 1.0,
            "expected contiguous columns, got w1 right+gap={} and w2 x={}",
            expected_w2_x,
            w2_frame.origin.x
        );
    }

    #[test]
    fn selecting_column_in_niri_mode_reveals_without_centering() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Center;
        settings.focus_navigation_style =
            crate::common::config::ScrollingFocusNavigationStyle::Niri;
        settings.column_width_ratio = 0.45;
        settings.min_column_width_ratio = 0.2;
        settings.max_column_width_ratio = 0.9;
        let (mut system, layout, w1, _) = setup_two_windows(settings);

        let screen = screen(1000.0, 800.0);
        let gaps = GapSettings::default();
        let _ = render(&system, layout, screen, &gaps);

        assert!(system.select_window(layout, w1));
        let frames = render(&system, layout, screen, &gaps);
        let w1_frame = frame_for(&frames, w1);
        let center_x = (screen.size.width - w1_frame.size.width) / 2.0;
        assert!(
            (w1_frame.origin.x - center_x).abs() > 5.0,
            "expected niri mode select to avoid centering, got centered x={} (center x={})",
            w1_frame.origin.x,
            center_x
        );
    }

    #[test]
    fn niri_focus_between_different_width_columns_keeps_strip_stable() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Center;
        settings.focus_navigation_style =
            crate::common::config::ScrollingFocusNavigationStyle::Niri;
        settings.column_width_ratio = 0.42;
        settings.min_column_width_ratio = 0.2;
        settings.max_column_width_ratio = 0.9;
        let (mut system, layout, w1, _) = setup_two_windows(settings);

        // Make focused-left column wider so selected widths differ across focus moves.
        let _ = system.move_focus(layout, Direction::Left);
        system.resize_selection_by(layout, 0.15);

        let screen = screen(1200.0, 800.0);
        let gaps = GapSettings::default();

        let frames_left = render(&system, layout, screen, &gaps);
        let w1_x_left = frame_for(&frames_left, w1).origin.x;

        let _ = system.move_focus(layout, Direction::Right);
        let frames_right = render(&system, layout, screen, &gaps);
        let w1_x_right = frame_for(&frames_right, w1).origin.x;

        assert!(
            (w1_x_left - w1_x_right).abs() < 1.0,
            "expected stable strip position in niri mode, got x shift {} -> {}",
            w1_x_left,
            w1_x_right
        );
    }

    #[test]
    fn move_selection_right_extracts_selected_from_stacked_column() {
        let (mut system, layout, w1, w2) = setup_two_windows(ScrollingLayoutSettings::default());
        system.join_selection_with_direction(layout, Direction::Left);

        assert!(system.move_selection(layout, Direction::Right));
        let state = system.layouts.get(layout).expect("layout state missing");
        assert_eq!(state.columns.len(), 2);
        assert_eq!(state.columns[0].windows, vec![w1]);
        assert_eq!(state.columns[1].windows, vec![w2]);
        assert_eq!(state.selected, Some(w2));
    }

    #[test]
    fn move_selection_left_extracts_selected_from_stacked_column_at_edge() {
        let (mut system, layout, w1, w2) = setup_two_windows(ScrollingLayoutSettings::default());
        system.join_selection_with_direction(layout, Direction::Left);

        assert!(system.move_selection(layout, Direction::Left));
        let state = system.layouts.get(layout).expect("layout state missing");
        assert_eq!(state.columns.len(), 2);
        assert_eq!(state.columns[0].windows, vec![w2]);
        assert_eq!(state.columns[1].windows, vec![w1]);
        assert_eq!(state.selected, Some(w2));
    }

    #[test]
    fn niri_rightmost_resize_grow_increases_visible_width() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Center;
        settings.focus_navigation_style =
            crate::common::config::ScrollingFocusNavigationStyle::Niri;
        settings.column_width_ratio = 0.45;
        settings.min_column_width_ratio = 0.2;
        settings.max_column_width_ratio = 0.95;
        let (mut system, layout, _, w2) = setup_two_windows(settings); // selected rightmost

        let screen = screen(1000.0, 800.0);
        let gaps = GapSettings::default();

        let before = render(&system, layout, screen, &gaps);
        let before_frame = frame_for(&before, w2);

        system.resize_selection_by(layout, 0.08);

        let after = render(&system, layout, screen, &gaps);
        let after_frame = frame_for(&after, w2);

        let visible_width = |frame: CGRect| {
            let left = frame.origin.x.max(screen.origin.x);
            let right =
                (frame.origin.x + frame.size.width).min(screen.origin.x + screen.size.width);
            (right - left).max(0.0)
        };
        let before_visible = visible_width(before_frame);
        let after_visible = visible_width(after_frame);
        assert!(
            after_visible > before_visible + 1.0,
            "expected visible width to grow, before={} after={}",
            before_visible,
            after_visible
        );
    }

    #[test]
    fn center_override_persists_on_refocus_of_same_window_in_niri_mode() {
        let mut settings = ScrollingLayoutSettings::default();
        settings.alignment = crate::common::config::ScrollingAlignment::Left;
        settings.focus_navigation_style =
            crate::common::config::ScrollingFocusNavigationStyle::Niri;
        let (mut system, layout, _, w2) = setup_two_windows(settings);

        system.center_selected_column(layout);

        let screen = screen(1000.0, 800.0);
        let gaps = GapSettings::default();
        let before = frame_for(&render(&system, layout, screen, &gaps), w2);

        assert!(system.select_window(layout, w2));
        assert!(system.select_window(layout, w2));

        let after = frame_for(&render(&system, layout, screen, &gaps), w2);

        assert!(
            (before.origin.x - after.origin.x).abs() < 1.0,
            "expected centered x to persist, got {} -> {}",
            before.origin.x,
            after.origin.x
        );
    }
}
