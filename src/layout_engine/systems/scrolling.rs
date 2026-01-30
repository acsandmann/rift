use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashSet;
use crate::common::config::ScrollingLayoutSettings;
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Column {
    windows: Vec<WindowId>,
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
    #[serde(skip, default = "default_atomic")]
    last_screen_width: AtomicU64,
    #[serde(skip, default = "default_atomic")]
    last_step_px: AtomicU64,
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
            last_screen_width: AtomicU64::new(0.0f64.to_bits()),
            last_step_px: AtomicU64::new(0.0f64.to_bits()),
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
        let Some((col_idx, _)) = self.selected_location() else {
            self.scroll_offset_px.store(0.0f64.to_bits(), Ordering::Relaxed);
            return;
        };
        let step = f64::from_bits(self.last_step_px.load(Ordering::Relaxed));
        if step > 0.0 {
            let offset = col_idx as f64 * step;
            self.scroll_offset_px.store(offset.to_bits(), Ordering::Relaxed);
            self.pending_align.store(false, Ordering::Relaxed);
        } else {
            self.pending_align.store(true, Ordering::Relaxed);
        }
    }

    fn clamp_scroll_offset(&mut self) {
        let step = f64::from_bits(self.last_step_px.load(Ordering::Relaxed));
        if step <= 0.0 {
            self.scroll_offset_px.store(0.0f64.to_bits(), Ordering::Relaxed);
            return;
        }
        let max_offset = (self.columns.len().saturating_sub(1) as f64) * step;
        let offset = f64::from_bits(self.scroll_offset_px.load(Ordering::Relaxed));
        let clamped = offset.clamp(0.0, max_offset);
        self.scroll_offset_px.store(clamped.to_bits(), Ordering::Relaxed);
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

        self.clamp_scroll_offset();
        self.selected
    }

    fn insert_column_after(&mut self, index: usize, wid: WindowId) {
        let column = Column { windows: vec![wid] };
        let insert_at = (index + 1).min(self.columns.len());
        self.columns.insert(insert_at, column);
        self.selected = Some(wid);
        self.align_scroll_to_selected();
    }

    fn insert_column_at_end(&mut self, wid: WindowId) {
        self.columns.push(Column { windows: vec![wid] });
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
                self.columns.push(Column { windows: vec![window] });
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
            last_screen_width: AtomicU64::new(self.last_screen_width.load(Ordering::Relaxed)),
            last_step_px: AtomicU64::new(self.last_step_px.load(Ordering::Relaxed)),
            fullscreen: self.fullscreen.clone(),
            fullscreen_within_gaps: self.fullscreen_within_gaps.clone(),
        }
    }
}

fn default_atomic_bool() -> AtomicBool { AtomicBool::new(false) }

fn default_atomic() -> AtomicU64 { AtomicU64::new(0.0f64.to_bits()) }

#[derive(Serialize, Deserialize)]
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

    pub fn scroll_by_delta(&mut self, layout: LayoutId, delta: f64) {
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let step = f64::from_bits(state.last_step_px.load(Ordering::Relaxed));
        if step <= 0.0 {
            return;
        }
        let max_offset = (state.columns.len().saturating_sub(1) as f64) * step;
        let current = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
        let next = (current + delta * step).clamp(0.0, max_offset);
        state.scroll_offset_px.store(next.to_bits(), Ordering::Relaxed);
    }

    pub fn snap_to_nearest_column(&mut self, layout: LayoutId) {
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let step = f64::from_bits(state.last_step_px.load(Ordering::Relaxed));
        if step <= 0.0 {
            return;
        }
        let max_offset = (state.columns.len().saturating_sub(1) as f64) * step;
        let current = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
        let target_idx = (current / step).round().max(0.0);
        let next = (target_idx * step).clamp(0.0, max_offset);
        state.scroll_offset_px.store(next.to_bits(), Ordering::Relaxed);
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
        let (col_idx, _row_idx) = match state.selected_location() {
            Some(loc) => loc,
            None => return false,
        };
        let target_col = match dir {
            Direction::Left => col_idx.checked_sub(1),
            Direction::Right => (col_idx + 1 < state.columns.len()).then_some(col_idx + 1),
            _ => None,
        };
        let Some(target_col) = target_col else { return false };
        let Some(selected) = state.selected else { return false };
        state.move_window_to_column_end(selected, target_col);
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
        let column_width =
            (tiling.size.width * self.clamp_ratio(state.column_width_ratio)).max(1.0);
        let step = column_width + gap_x;
        state.last_screen_width.store(tiling.size.width.to_bits(), Ordering::Relaxed);
        state.last_step_px.store(step.to_bits(), Ordering::Relaxed);
        if state.pending_align.load(Ordering::Relaxed) {
            let offset = state
                .selected_location()
                .map(|(col_idx, _)| col_idx as f64 * step)
                .unwrap_or(0.0);
            state.scroll_offset_px.store(offset.to_bits(), Ordering::Relaxed);
            state.pending_align.store(false, Ordering::Relaxed);
        }
        let current = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
        let max_offset = (state.columns.len().saturating_sub(1) as f64) * step;
        let clamped = current.clamp(0.0, max_offset);
        state.scroll_offset_px.store(clamped.to_bits(), Ordering::Relaxed);

        let anchor_x = match self.settings.alignment {
            crate::common::config::ScrollingAlignment::Left => tiling.origin.x,
            crate::common::config::ScrollingAlignment::Center => {
                tiling.origin.x + (tiling.size.width - column_width) / 2.0
            }
            crate::common::config::ScrollingAlignment::Right => {
                tiling.origin.x + tiling.size.width - column_width
            }
        };

        let mut out = Vec::new();
        for (col_idx, col) in state.columns.iter().enumerate() {
            let offset = f64::from_bits(state.scroll_offset_px.load(Ordering::Relaxed));
            let x = anchor_x + (col_idx as f64) * step - offset;
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
        let Some(state) = self.layout_state_mut(layout) else {
            return (None, vec![]);
        };
        let new_sel = match direction {
            Direction::Left | Direction::Right => Self::move_focus_horizontal(state, direction),
            Direction::Up | Direction::Down => Self::move_focus_vertical(state, direction),
        };
        state.align_scroll_to_selected();
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
        let Some(state) = self.layout_state_mut(layout) else {
            return false;
        };
        if state.locate(wid).is_some() {
            state.selected = Some(wid);
            state.align_scroll_to_selected();
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
        state.column_width_ratio = ratio.clamp(min_ratio, max_ratio).max(0.05).min(0.98);
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
            state.columns.insert(insert_at, Column { windows: vec![wid] });
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
        state.columns.insert(insert_at, Column { windows: vec![wid] });
        state.selected = Some(wid);
        state.align_scroll_to_selected();
        state.clamp_scroll_offset();
    }

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        let min_ratio = self.settings.min_column_width_ratio;
        let max_ratio = self.settings.max_column_width_ratio;
        let Some(state) = self.layout_state_mut(layout) else {
            return;
        };
        let ratio = state.column_width_ratio + amount;
        state.column_width_ratio = ratio.clamp(min_ratio, max_ratio).max(0.05).min(0.98);
    }

    fn rebalance(&mut self, _layout: LayoutId) {}

    fn toggle_tile_orientation(&mut self, _layout: LayoutId) {}
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::ScrollingLayoutSystem;
    use crate::actor::app::{WindowId, pid_t};
    use crate::common::config::ScrollingLayoutSettings;
    use crate::layout_engine::Direction;
    use crate::layout_engine::systems::LayoutSystem;

    fn wid(pid: pid_t, idx: u32) -> WindowId {
        WindowId {
            pid,
            idx: std::num::NonZeroU32::new(idx).unwrap(),
        }
    }

    #[test]
    fn creates_columns_and_moves_focus() {
        let settings = ScrollingLayoutSettings::default();
        let mut system = ScrollingLayoutSystem::new(&settings);
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
    fn calculates_centered_columns() {
        let settings = ScrollingLayoutSettings::default();
        let mut system = ScrollingLayoutSystem::new(&settings);
        let layout = system.create_layout();

        let w1 = wid(1, 1);
        let w2 = wid(1, 2);

        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let gaps = crate::common::config::GapSettings::default();
        let frames = system.calculate_layout(
            layout,
            screen,
            0.0,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
        );

        assert_eq!(frames.len(), 2);
        let width = frames[1].1.size.width;
        assert!((width - 700.0).abs() < 1.0);
    }
}
