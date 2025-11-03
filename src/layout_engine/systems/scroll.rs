use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::{Direction, LayoutId, LayoutKind};

const MIN_WINDOW_DIMENSION: f64 = 32.0;
const MIN_WIDTH_UNITS: f64 = 0.2;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ScrollLayoutState {
    windows: Vec<WindowId>,
    selected: Option<WindowId>,
    widths: Vec<f64>,
    scroll_offset: f64,
}

impl Default for ScrollLayoutState {
    fn default() -> Self {
        Self {
            windows: Vec::new(),
            selected: None,
            scroll_offset: 0.0,
            widths: Vec::new(),
        }
    }
}

impl ScrollLayoutState {
    fn max_offset(&self) -> f64 {
        if self.windows.len() > 1 {
            (self.windows.len() - 1) as f64
        } else {
            0.0
        }
    }

    fn clamp_offset(&mut self) {
        if !self.scroll_offset.is_finite() {
            self.scroll_offset = 0.0;
        }
        let max = self.max_offset();
        if max == 0.0 {
            self.scroll_offset = 0.0;
        } else {
            self.scroll_offset = self.scroll_offset.clamp(0.0, max);
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected?;
        self.windows.iter().position(|w| *w == selected)
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
        self.scroll_offset = self.scroll_offset.clamp(0.0, self.max_offset());
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
                self.scroll_offset = new_idx as f64;
            } else if let Some(sel_idx) = self.selected_index() {
                self.scroll_offset = sel_idx as f64;
            }
            self.ensure_widths();
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

        let target_idx = state.scroll_offset.round().clamp(0.0, state.max_offset()) as usize;

        if target_idx != prev_index {
            let wid = state.windows[target_idx];
            state.selected = Some(wid);
            Some(wid)
        } else {
            None
        }
    }

    pub fn finalize_scroll(&mut self, layout: LayoutId) -> Option<WindowId> {
        let state = self.layouts.get_mut(layout)?;
        state.ensure_selection();
        state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
        None
    }

    fn layout_state(&mut self, layout: LayoutId) -> Option<&mut ScrollLayoutState> {
        self.layouts.get_mut(layout)
    }

    fn layout_state_ref(&self, layout: LayoutId) -> Option<&ScrollLayoutState> {
        self.layouts.get(layout)
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
                    let marker = if state.selected == Some(*wid) {
                        '>'
                    } else {
                        ' '
                    };
                    buf.push_str(&format!("{marker} [{idx}] {wid:?}\n"));
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

        let outer = &gaps.outer;
        let inner = &gaps.inner;
        let gap = inner.horizontal;
        let len = state.windows.len();

        let available_width =
            (screen.size.width - outer.left - outer.right).max(MIN_WINDOW_DIMENSION);
        let available_height =
            (screen.size.height - outer.top - outer.bottom).max(MIN_WINDOW_DIMENSION);
        let available_content_width =
            (available_width - gap * (len.saturating_sub(1) as f64)).max(MIN_WINDOW_DIMENSION);

        let mut weights: Vec<f64> =
            state.widths.iter().take(len).map(|w| w.max(MIN_WIDTH_UNITS)).collect();
        if weights.len() < len {
            weights.resize(len, 1.0);
        }
        let total_units = weights.iter().sum::<f64>().max(MIN_WIDTH_UNITS * len as f64);
        let unit_scale = if total_units <= f64::EPSILON {
            available_content_width / len as f64
        } else {
            available_content_width / total_units
        };

        let mut pixel_widths = Vec::with_capacity(len);
        for w in &weights {
            pixel_widths.push(w * unit_scale);
        }

        let mut prefix = Vec::with_capacity(len);
        let mut acc = 0.0;
        for width in &pixel_widths {
            prefix.push(acc);
            acc += *width + gap;
        }

        let window_height = (available_height - inner.vertical).max(MIN_WINDOW_DIMENSION);
        let base_x = screen.origin.x + outer.left;
        let base_y =
            screen.origin.y + outer.top + (available_height - window_height).max(0.0) / 2.0;

        let offset = state.scroll_offset.clamp(0.0, state.max_offset());
        let idx_floor = offset.floor() as usize;
        let frac = offset - idx_floor as f64;
        let mut shift = 0.0;
        for i in 0..idx_floor.min(len) {
            shift += pixel_widths[i] + gap;
        }
        if idx_floor < len {
            shift += frac * (pixel_widths[idx_floor] + gap);
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
            state.scroll_offset = target as f64;
            (Some(wid), vec![wid])
        }
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let Some(state) = self.layout_state(layout) else { return };

        let insert_idx = state.selected_index().map(|idx| idx + 1).unwrap_or(state.windows.len());
        state.windows.insert(insert_idx, wid);
        state.widths.insert(insert_idx, 1.0);
        state.selected = Some(wid);
        state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
        state.ensure_widths();
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
            state.scroll_offset = (insert_idx as f64).min(state.max_offset());
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
        state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
        true
    }

    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) {
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
        if state.selected == Some(a) {
            state.scroll_offset = b_idx as f64;
        } else if state.selected == Some(b) {
            state.scroll_offset = a_idx as f64;
        }
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
            state.scroll_offset = target_idx as f64;
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
            if from_state.windows.is_empty() {
                from_state.selected = None;
                from_state.scroll_offset = 0.0;
            } else {
                let new_idx = idx.min(from_state.windows.len() - 1);
                from_state.selected = Some(from_state.windows[new_idx]);
                from_state.scroll_offset = new_idx as f64;
            }
            from_state.ensure_widths();
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
            to_state.scroll_offset = to_state.scroll_offset.clamp(0.0, to_state.max_offset());
            to_state.ensure_widths();
        }
    }

    fn split_selection(&mut self, _layout: LayoutId, _kind: LayoutKind) {}

    fn toggle_fullscreen_of_selection(&mut self, _layout: LayoutId) -> Vec<WindowId> { Vec::new() }

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
        state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
    }

    fn rebalance(&mut self, layout: LayoutId) {
        if let Some(state) = self.layout_state(layout) {
            state.ensure_selection();
        }
    }
}
