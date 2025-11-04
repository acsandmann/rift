use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::layout_engine::systems::{LayoutSystem, ToggleAction};
use crate::layout_engine::{Direction, LayoutId, LayoutKind};

const MIN_WINDOW_DIMENSION: f64 = 32.0;
const MIN_WIDTH_UNITS: f64 = 0.2;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ScrollLayoutState {
    windows: Vec<WindowId>,
    selected: Option<WindowId>,
    widths: Vec<f64>,
    scroll_offset: f64,
    fullscreen: Option<WindowId>,
    fullscreen_within_gaps: bool,
    full_width: Option<WindowId>,
}

impl Default for ScrollLayoutState {
    fn default() -> Self {
        Self {
            windows: Vec::new(),
            selected: None,
            scroll_offset: 0.0,
            widths: Vec::new(),
            fullscreen: None,
            fullscreen_within_gaps: false,
            full_width: None,
        }
    }
}

impl ScrollLayoutState {
    fn viewport_units(&self) -> f64 { 1.0 }

    fn width_unit(&self, idx: usize) -> f64 {
        self.widths.get(idx).copied().unwrap_or(1.0).max(MIN_WIDTH_UNITS)
    }

    fn total_units(&self) -> f64 {
        if self.windows.is_empty() {
            0.0
        } else {
            self.windows.iter().enumerate().map(|(idx, _)| self.width_unit(idx)).sum()
        }
    }

    fn max_offset(&self) -> f64 {
        let viewport = self.viewport_units();
        let total = self.total_units();
        if total <= viewport {
            0.0
        } else {
            total - viewport
        }
    }

    fn clamp_offset(&mut self) {
        self.ensure_widths();
        if !self.scroll_offset.is_finite() {
            self.scroll_offset = 0.0;
        }
        let max = self.max_offset();
        self.scroll_offset = self.scroll_offset.clamp(0.0, max);
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected?;
        self.windows.iter().position(|w| *w == selected)
    }

    fn ensure_visible_index(&mut self, idx: usize) {
        self.ensure_widths();
        self.clamp_offset();

        let viewport = self.viewport_units();
        let mut start = 0.0;
        for current in 0..self.windows.len() {
            let width = self.width_unit(current);
            let end = start + width;
            if current == idx {
                let view_start = self.scroll_offset;
                let view_end = view_start + viewport;
                if start < view_start {
                    self.scroll_offset = start.max(0.0);
                } else if end > view_end {
                    self.scroll_offset = (end - viewport).max(0.0);
                }
                break;
            }
            start = end;
        }

        self.clamp_offset();
    }

    fn ensure_selected_visible(&mut self) {
        if let Some(idx) = self.selected_index() {
            self.ensure_visible_index(idx);
        } else {
            self.clamp_offset();
        }
    }

    fn focus_point(&self) -> f64 { self.scroll_offset + self.viewport_units() * 0.5 }

    fn index_nearest_focus(&self) -> Option<usize> {
        if self.windows.is_empty() {
            return None;
        }

        let focus = self.focus_point();
        let mut acc = 0.0;
        let mut best_idx = 0;
        let mut best_dist = f64::MAX;

        for (idx, _) in self.windows.iter().enumerate() {
            let width = self.width_unit(idx);
            let center = acc + width * 0.5;
            let dist = (center - focus).abs();
            if dist < best_dist {
                best_dist = dist;
                best_idx = idx;
            }
            acc += width;
        }

        Some(best_idx)
    }

    fn ensure_selection(&mut self) {
        self.ensure_widths();
        if self.windows.is_empty() {
            self.selected = None;
            self.scroll_offset = 0.0;
            return;
        }

        if self.selected_index().is_none() {
            self.selected = Some(self.windows[0]);
            self.scroll_offset = 0.0;
        }

        self.clamp_offset();
        self.ensure_selected_visible();
    }

    fn remove_window(&mut self, wid: WindowId) -> bool {
        if let Some(idx) = self.windows.iter().position(|w| *w == wid) {
            self.windows.remove(idx);
            if idx < self.widths.len() {
                self.widths.remove(idx);
            }
            if self.windows.is_empty() {
                self.selected = None;
                self.scroll_offset = 0.0;
            } else if self.selected == Some(wid) {
                let new_idx = if idx >= self.windows.len() {
                    self.windows.len() - 1
                } else {
                    idx
                };
                self.selected = Some(self.windows[new_idx]);
            }

            if self.fullscreen == Some(wid) {
                self.fullscreen = None;
                self.fullscreen_within_gaps = false;
            }
            if self.full_width == Some(wid) {
                self.full_width = None;
            }
            self.ensure_widths();
            self.clamp_offset();
            self.ensure_selected_visible();
            true
        } else {
            false
        }
    }

    fn ensure_widths(&mut self) {
        if self.widths.len() != self.windows.len() {
            self.widths.resize(self.windows.len(), 1.0);
        }
        for width in &mut self.widths {
            if *width < MIN_WIDTH_UNITS {
                *width = MIN_WIDTH_UNITS;
            }
        }
        if self.widths.iter().all(|w| *w <= 0.0) {
            for w in &mut self.widths {
                *w = 1.0;
            }
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct ScrollLayoutSystem {
    layouts: slotmap::SlotMap<LayoutId, ScrollLayoutState>,
}

impl ScrollLayoutSystem {
    pub fn scroll_by(&mut self, layout: LayoutId, delta: f64) -> Option<WindowId> {
        let state = self.layouts.get_mut(layout)?;
        if state.windows.is_empty() {
            state.selected = None;
            state.scroll_offset = 0.0;
            return None;
        }

        state.ensure_selection();

        let prev_index = state.selected_index().unwrap_or(0);

        state.scroll_offset = (state.scroll_offset + delta).clamp(0.0, state.max_offset());
        let target_idx = state.index_nearest_focus().unwrap_or(prev_index);

        if target_idx != prev_index {
            let wid = state.windows[target_idx];
            state.selected = Some(wid);
            state.ensure_visible_index(target_idx);
            Some(wid)
        } else {
            None
        }
    }

    pub fn finalize_scroll(&mut self, layout: LayoutId) -> Option<WindowId> {
        let state = self.layouts.get_mut(layout)?;
        if state.windows.is_empty() {
            state.selected = None;
            state.scroll_offset = 0.0;
            return None;
        }

        state.ensure_selection();
        state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());

        if let Some(idx) = state.index_nearest_focus() {
            if idx < state.windows.len() {
                let wid = state.windows[idx];
                state.selected = Some(wid);
                state.ensure_visible_index(idx);
                return Some(wid);
            }
        }

        state.ensure_selected_visible();
        state.selected
    }

    fn layout_state(&mut self, layout: LayoutId) -> Option<&mut ScrollLayoutState> {
        self.layouts.get_mut(layout)
    }

    fn layout_state_ref(&self, layout: LayoutId) -> Option<&ScrollLayoutState> {
        self.layouts.get(layout)
    }

    pub fn shift_view_by(&mut self, layout: LayoutId, delta: f64) {
        if let Some(state) = self.layouts.get_mut(layout) {
            if state.windows.is_empty() {
                state.selected = None;
                state.scroll_offset = 0.0;
                return;
            }
            state.ensure_selection();
            state.scroll_offset = (state.scroll_offset + delta).clamp(0.0, state.max_offset());
        }
    }
}

impl LayoutSystem for ScrollLayoutSystem {
    fn create_layout(&mut self) -> LayoutId { self.layouts.insert(ScrollLayoutState::default()) }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let state = self.layouts.get(layout).cloned().unwrap_or_default();
        self.layouts.insert(state)
    }

    fn remove_layout(&mut self, layout: LayoutId) { self.layouts.remove(layout); }

    fn draw_tree(&self, layout: LayoutId) -> String {
        match self.layouts.get(layout) {
            Some(state) => {
                let mut buf = String::from("scroll\n");
                for (idx, wid) in state.windows.iter().enumerate() {
                    let sel_marker = if state.selected == Some(*wid) {
                        '>'
                    } else {
                        ' '
                    };
                    let fs_marker = if state.fullscreen == Some(*wid) {
                        'F'
                    } else {
                        ' '
                    };
                    let fw_marker = if state.full_width == Some(*wid) {
                        'W'
                    } else {
                        ' '
                    };
                    buf.push_str(&format!(
                        "{}{}{} [{idx}] {wid:?}\n",
                        sel_marker, fs_marker, fw_marker
                    ));
                }
                buf
            }
            None => "scroll <missing layout>".to_string(),
        }
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
        if state.windows.is_empty() {
            return Vec::new();
        }

        if let Some(fs_wid) = state.fullscreen {
            if state.windows.iter().any(|w| *w == fs_wid) {
                if state.fullscreen_within_gaps {
                    let outer = &gaps.outer;
                    let available_width =
                        (screen.size.width - outer.left - outer.right).max(MIN_WINDOW_DIMENSION);
                    let available_height =
                        (screen.size.height - outer.top - outer.bottom).max(MIN_WINDOW_DIMENSION);
                    let base_x = screen.origin.x + outer.left;
                    let base_y = screen.origin.y + outer.top;
                    let frame = CGRect::new(
                        CGPoint::new(base_x, base_y),
                        CGSize::new(available_width, available_height),
                    );
                    return vec![(fs_wid, frame)];
                } else {
                    let frame = CGRect::new(screen.origin, screen.size);
                    return vec![(fs_wid, frame)];
                }
            }
        }

        let outer = &gaps.outer;
        let inner = &gaps.inner;
        let gap = inner.horizontal;
        let len = state.windows.len();

        let available_width =
            (screen.size.width - outer.left - outer.right).max(MIN_WINDOW_DIMENSION);
        let available_height =
            (screen.size.height - outer.top - outer.bottom).max(MIN_WINDOW_DIMENSION);
        let width_units: Vec<f64> =
            state.windows.iter().enumerate().map(|(idx, _)| state.width_unit(idx)).collect();

        let window_height = (available_height - inner.vertical).max(MIN_WINDOW_DIMENSION);
        let base_unit = window_height.max(MIN_WINDOW_DIMENSION);

        let mut pixel_widths: Vec<f64> = width_units
            .iter()
            .map(|units| (*units * base_unit).max(MIN_WINDOW_DIMENSION))
            .collect();

        if let Some(fw) = state.full_width {
            if let Some(idx) = state.windows.iter().position(|w| *w == fw) {
                if idx < pixel_widths.len() {
                    pixel_widths[idx] = available_width;
                }
            }
        }

        let mut prefix = Vec::with_capacity(len);
        let mut acc = 0.0;
        for width in &pixel_widths {
            prefix.push(acc);
            acc += *width + gap;
        }

        let base_x = screen.origin.x + outer.left;
        let base_y =
            screen.origin.y + outer.top + (available_height - window_height).max(0.0) / 2.0;

        let offset_units = state.scroll_offset.clamp(0.0, state.max_offset());
        let mut remaining = offset_units;
        let mut shift = 0.0;
        for (idx, width_unit) in width_units.iter().enumerate() {
            if remaining <= 0.0 {
                break;
            }

            let width_px = pixel_widths[idx];
            if remaining < *width_unit {
                if *width_unit > f64::EPSILON {
                    let fraction = remaining / *width_unit;
                    let slot_gap = if idx + 1 < len { gap } else { 0.0 };
                    let slot_width = width_px + slot_gap;
                    shift += fraction * slot_width;
                }
                break;
            } else {
                shift += width_px;
                remaining -= *width_unit;
                if idx + 1 < len {
                    shift += gap;
                }
            }
        }

        state
            .windows
            .iter()
            .enumerate()
            .map(|(idx, wid)| {
                let x = base_x + prefix[idx] - shift;
                let frame = CGRect::new(
                    CGPoint::new(x, base_y),
                    CGSize::new(pixel_widths[idx], window_height),
                );
                (*wid, frame)
            })
            .collect()
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.layout_state_ref(layout).and_then(|state| state.selected)
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layout_state_ref(layout)
            .map(|state| state.windows.clone())
            .unwrap_or_default()
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        self.selected_window(layout).into_iter().collect()
    }

    fn ascend_selection(&mut self, _layout: LayoutId) -> bool { false }

    fn descend_selection(&mut self, _layout: LayoutId) -> bool { false }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let state = match self.layout_state(layout) {
            Some(state) => state,
            None => return (None, Vec::new()),
        };

        if state.windows.is_empty() {
            state.selected = None;
            state.scroll_offset = 0.0;
            return (None, Vec::new());
        }

        state.ensure_selection();
        let current = state.selected_index().unwrap_or(0);

        let target = match direction {
            Direction::Left | Direction::Up => current.saturating_sub(1),
            Direction::Right | Direction::Down => (current + 1).min(state.windows.len() - 1),
        };

        if target == current {
            (state.selected, Vec::new())
        } else {
            let wid = state.windows[target];
            state.selected = Some(wid);
            state.ensure_visible_index(target);
            (Some(wid), vec![wid])
        }
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let Some(state) = self.layout_state(layout) else { return };

        let insert_idx = state.selected_index().map(|idx| idx + 1).unwrap_or(state.windows.len());
        state.windows.insert(insert_idx, wid);
        state.widths.insert(insert_idx, 1.0);
        state.selected = Some(wid);
        state.ensure_widths();
        state.clamp_offset();
        state.ensure_selected_visible();
    }

    fn remove_window(&mut self, wid: WindowId) {
        for state in self.layouts.values_mut() {
            if state.remove_window(wid) {
                state.ensure_selection();
            }
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        for state in self.layouts.values_mut() {
            let mut removed_selected = false;
            let mut idx = 0;
            while idx < state.windows.len() {
                if state.windows[idx].pid == pid {
                    if state.selected == Some(state.windows[idx]) {
                        removed_selected = true;
                    }
                    if state.fullscreen == Some(state.windows[idx]) {
                        state.fullscreen = None;
                        state.fullscreen_within_gaps = false;
                    }
                    if state.full_width == Some(state.windows[idx]) {
                        state.full_width = None;
                    }
                    state.windows.remove(idx);
                    if idx < state.widths.len() {
                        state.widths.remove(idx);
                    }
                } else {
                    idx += 1;
                }
            }
            state.ensure_widths();
            if removed_selected {
                state.ensure_selection();
            } else {
                state.clamp_offset();
                state.ensure_selected_visible();
            }
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let Some(state) = self.layout_state(layout) else { return };

        let mut first_index = None;
        let mut removed_selected = false;

        let mut i = 0;
        while i < state.windows.len() {
            if state.windows[i].pid == pid {
                if first_index.is_none() {
                    first_index = Some(i);
                }
                if state.selected == Some(state.windows[i]) {
                    removed_selected = true;
                }
                if state.fullscreen == Some(state.windows[i]) {
                    state.fullscreen = None;
                    state.fullscreen_within_gaps = false;
                }
                if state.full_width == Some(state.windows[i]) {
                    state.full_width = None;
                }
                state.windows.remove(i);
                if i < state.widths.len() {
                    state.widths.remove(i);
                }
            } else {
                i += 1;
            }
        }

        if desired.is_empty() {
            state.ensure_widths();
            if removed_selected {
                state.ensure_selection();
            } else {
                state.clamp_offset();
                state.ensure_selected_visible();
            }
            return;
        }

        let insert_idx = first_index.unwrap_or(state.windows.len());
        for (offset, wid) in desired.iter().enumerate() {
            state.windows.insert(insert_idx + offset, *wid);
            state.widths.insert(insert_idx + offset, 1.0);
        }

        if removed_selected {
            state.selected = Some(desired[0]);
        }

        state.ensure_selection();
    }

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.layout_state_ref(layout)
            .map(|state| state.windows.iter().any(|wid| wid.pid == pid))
            .unwrap_or(false)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.layout_state_ref(layout)
            .map(|state| state.windows.contains(&wid))
            .unwrap_or(false)
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(state) = self.layout_state(layout) else {
            return false;
        };
        if !state.windows.iter().any(|w| *w == wid) {
            return false;
        }

        state.selected = Some(wid);
        state.clamp_offset();
        state.ensure_selected_visible();
        true
    }

    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        _old_frame: CGRect,
        new_frame: CGRect,
        _screen: CGRect,
        _gaps: &crate::common::config::GapSettings,
    ) {
        let Some(state) = self.layout_state(layout) else { return };
        let Some(idx) = state.windows.iter().position(|w| *w == wid) else {
            return;
        };

        if state.fullscreen == Some(wid) || state.full_width == Some(wid) {
            return;
        }

        state.ensure_widths();

        let base_unit = new_frame.size.height.max(MIN_WINDOW_DIMENSION);
        if base_unit <= f64::EPSILON {
            return;
        }

        let mut new_units = new_frame.size.width / base_unit;
        if !new_units.is_finite() {
            return;
        }
        new_units = new_units.max(MIN_WIDTH_UNITS);

        if idx < state.widths.len() && (state.widths[idx] - new_units).abs() > f64::EPSILON {
            state.widths[idx] = new_units;
            state.clamp_offset();
            state.ensure_selected_visible();
        }
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        let Some(state) = self.layout_state(layout) else {
            return false;
        };
        let Some(a_idx) = state.windows.iter().position(|w| *w == a) else {
            return false;
        };
        let Some(b_idx) = state.windows.iter().position(|w| *w == b) else {
            return false;
        };
        state.windows.swap(a_idx, b_idx);
        if a_idx < state.widths.len() && b_idx < state.widths.len() {
            state.widths.swap(a_idx, b_idx);
        }
        state.ensure_selected_visible();
        true
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(state) = self.layout_state(layout) else {
            return false;
        };
        state.ensure_selection();
        let Some(idx) = state.selected_index() else {
            return false;
        };
        let len = state.windows.len();
        if len <= 1 {
            return false;
        }

        let target = match direction {
            Direction::Left | Direction::Up => idx.checked_sub(1),
            Direction::Right | Direction::Down => {
                if idx + 1 < len {
                    Some(idx + 1)
                } else {
                    None
                }
            }
        };

        if let Some(target_idx) = target {
            state.windows.swap(idx, target_idx);
            if idx < state.widths.len() && target_idx < state.widths.len() {
                state.widths.swap(idx, target_idx);
            }
            state.ensure_selected_visible();
            true
        } else {
            false
        }
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        let wid_opt = {
            let Some(from_state) = self.layout_state(from_layout) else {
                return;
            };
            from_state.ensure_selection();
            let Some(idx) = from_state.selected_index() else { return };
            let wid = from_state.windows.remove(idx);
            let width = if idx < from_state.widths.len() {
                from_state.widths.remove(idx)
            } else {
                1.0
            };
            if from_state.fullscreen == Some(wid) {
                from_state.fullscreen = None;
                from_state.fullscreen_within_gaps = false;
            }
            if from_state.full_width == Some(wid) {
                from_state.full_width = None;
            }

            if from_state.windows.is_empty() {
                from_state.selected = None;
                from_state.scroll_offset = 0.0;
            } else {
                let new_idx = idx.min(from_state.windows.len() - 1);
                from_state.selected = Some(from_state.windows[new_idx]);
            }
            from_state.ensure_widths();
            from_state.clamp_offset();
            from_state.ensure_selected_visible();
            Some((wid, width))
        };

        if let Some((wid, width)) = wid_opt {
            let Some(to_state) = self.layout_state(to_layout) else {
                return;
            };
            let insert_idx =
                to_state.selected_index().map(|idx| idx + 1).unwrap_or(to_state.windows.len());
            to_state.windows.insert(insert_idx, wid);
            to_state.widths.insert(insert_idx, width.max(MIN_WIDTH_UNITS));
            to_state.selected = Some(wid);
            to_state.ensure_widths();
            to_state.clamp_offset();
            to_state.ensure_selected_visible();
        }
    }

    fn split_selection(&mut self, _layout: LayoutId, _kind: LayoutKind) {}

    fn toggle_action(&mut self, layout: LayoutId, action: ToggleAction) -> Vec<WindowId> {
        let Some(state) = self.layout_state(layout) else {
            return vec![];
        };

        match action {
            ToggleAction::Fullscreen { within_gaps } => {
                state.ensure_selection();
                let Some(wid) = state.selected else {
                    return vec![];
                };

                if within_gaps {
                    if state.fullscreen == Some(wid) {
                        state.fullscreen_within_gaps = !state.fullscreen_within_gaps;
                        vec![wid]
                    } else {
                        state.fullscreen = Some(wid);
                        state.fullscreen_within_gaps = true;
                        state.full_width = None;
                        vec![wid]
                    }
                } else {
                    if state.fullscreen == Some(wid) {
                        state.fullscreen = None;
                        state.fullscreen_within_gaps = false;
                        Vec::new()
                    } else {
                        state.fullscreen = Some(wid);
                        state.fullscreen_within_gaps = false;
                        state.full_width = None;
                        vec![wid]
                    }
                }
            }
            ToggleAction::FullWidth => {
                state.ensure_selection();
                let Some(idx) = state.selected_index() else {
                    return vec![];
                };
                let wid = state.windows[idx];

                if state.full_width == Some(wid) {
                    state.full_width = None;
                    Vec::new()
                } else {
                    state.full_width = Some(wid);
                    state.fullscreen = None;
                    state.fullscreen_within_gaps = false;
                    vec![wid]
                }
            }
        }
    }

    fn join_selection_with_direction(&mut self, _layout: LayoutId, _direction: Direction) {}

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        _: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    fn unstack_parent_of_selection(
        &mut self,
        _layout: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    fn unjoin_selection(&mut self, _layout: LayoutId) {}

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        if amount.abs() < f64::EPSILON {
            return;
        }
        let Some(state) = self.layout_state(layout) else { return };
        if state.windows.is_empty() {
            return;
        }

        state.ensure_selection();
        let Some(idx) = state.selected_index() else { return };

        state.widths[idx] = (state.widths[idx] + amount).max(MIN_WIDTH_UNITS);
        state.ensure_widths();
        state.clamp_offset();
        state.ensure_selected_visible();
    }

    fn rebalance(&mut self, layout: LayoutId) {
        if let Some(state) = self.layout_state(layout) {
            state.ensure_selection();
        }
    }

    fn toggle_tile_orientation(&mut self, _layout: LayoutId) {}

    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { false }
}
