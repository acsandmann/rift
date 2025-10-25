use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::config::ScrollLayoutSettings;
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::{Direction, LayoutId, LayoutKind};

const MIN_WINDOW_DIMENSION: f64 = 32.0;
const MIN_WIDTH_UNITS: f64 = 0.2;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollDirection {
    #[serde(rename = "forward")]
    Forward,
    #[serde(rename = "reverse")]
    Reverse,
}

impl ScrollDirection {
    fn toggle(self) -> Self {
        match self {
            ScrollDirection::Forward => ScrollDirection::Reverse,
            ScrollDirection::Reverse => ScrollDirection::Forward,
        }
    }

    #[inline]
    fn is_reverse(self) -> bool { matches!(self, ScrollDirection::Reverse) }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ScrollLayoutState {
    windows: Vec<WindowId>,
    selected: Option<WindowId>,
    widths: Vec<f64>,
    scroll_offset: f64,
    direction: ScrollDirection,
}

impl Default for ScrollLayoutState {
    fn default() -> Self {
        Self {
            windows: Vec::new(),
            selected: None,
            scroll_offset: 0.0,
            widths: Vec::new(),
            direction: ScrollDirection::Forward,
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

    fn ensure_selection(&mut self, default_ratio: f64) {
        self.ensure_widths(default_ratio);
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

    fn remove_window(&mut self, wid: WindowId, default_ratio: f64) -> bool {
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
            self.ensure_widths(default_ratio);
            true
        } else {
            false
        }
    }

    fn ensure_widths(&mut self, default_ratio: f64) {
        let fallback = default_ratio.max(MIN_WIDTH_UNITS);
        if self.widths.len() != self.windows.len() {
            self.widths.resize(self.windows.len(), fallback);
        }
        for width in &mut self.widths {
            if !width.is_finite() || *width < MIN_WIDTH_UNITS {
                *width = fallback;
            }
        }
        if self.widths.iter().all(|w| *w <= 0.0) {
            for w in &mut self.widths {
                *w = fallback;
            }
        }
    }
}

#[derive(Clone, Debug)]
struct ScrollRuntimeConfig {
    default_window_ratio: f64,
    center_bias: f64,
    snap_threshold: f64,
}

impl ScrollRuntimeConfig {
    fn from_settings(settings: &ScrollLayoutSettings) -> Self {
        let default_ratio = settings.window_fraction.max(MIN_WIDTH_UNITS);
        let center_bias = settings.center_bias.clamp(-0.49, 0.49);
        let snap_threshold = settings.snap_threshold.clamp(0.05, 0.95);
        Self {
            default_window_ratio: default_ratio,
            center_bias,
            snap_threshold,
        }
    }

    fn center_factor(&self) -> f64 { (0.5 + self.center_bias).clamp(0.0, 1.0) }

    fn for_serde() -> Self { Self::from_settings(&ScrollLayoutSettings::default()) }
}

impl Default for ScrollRuntimeConfig {
    fn default() -> Self { Self::from_settings(&ScrollLayoutSettings::default()) }
}

#[derive(Serialize, Deserialize)]
pub struct ScrollLayoutSystem {
    layouts: slotmap::SlotMap<LayoutId, ScrollLayoutState>,
    #[serde(skip)]
    #[serde(default = "ScrollRuntimeConfig::for_serde")]
    settings: ScrollRuntimeConfig,
}

impl ScrollLayoutSystem {
    pub fn from_settings(settings: &ScrollLayoutSettings) -> Self {
        Self {
            layouts: slotmap::SlotMap::default(),
            settings: ScrollRuntimeConfig::from_settings(settings),
        }
    }

    pub fn update_settings(&mut self, settings: &ScrollLayoutSettings) {
        self.settings = ScrollRuntimeConfig::from_settings(settings);
        for state in self.layouts.values_mut() {
            state.ensure_widths(self.settings.default_window_ratio);
        }
    }

    pub fn scroll_by(&mut self, layout: LayoutId, delta: f64) -> Option<WindowId> {
        let default_ratio = self.settings.default_window_ratio;
        let snap_threshold = self.settings.snap_threshold;
        let state = self.layouts.get_mut(layout)?;
        if state.windows.is_empty() {
            state.selected = None;
            state.scroll_offset = 0.0;
            return None;
        }

        state.ensure_selection(default_ratio);

        let prev_index = state.selected_index().unwrap_or(0);

        state.scroll_offset = (state.scroll_offset + delta).clamp(0.0, state.max_offset());

        let base = state.scroll_offset.floor().clamp(0.0, state.max_offset());
        let frac = state.scroll_offset - base;
        let mut target_idx = base as usize;
        if frac >= snap_threshold && target_idx + 1 < state.windows.len() {
            target_idx += 1;
        }

        if target_idx != prev_index {
            let wid = state.windows[target_idx];
            state.selected = Some(wid);
            state.scroll_offset = target_idx as f64;
            Some(wid)
        } else {
            None
        }
    }

    pub fn finalize_scroll(&mut self, layout: LayoutId) -> Option<WindowId> {
        let default_ratio = self.settings.default_window_ratio;
        let snap_threshold = self.settings.snap_threshold;
        let state = self.layouts.get_mut(layout)?;
        state.ensure_selection(default_ratio);
        state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());

        let base = state.scroll_offset.floor().clamp(0.0, state.max_offset());
        let frac = state.scroll_offset - base;
        let mut target_idx = base as usize;
        if frac >= snap_threshold && target_idx + 1 < state.windows.len() {
            target_idx += 1;
        }
        if let Some(&wid) = state.windows.get(target_idx) {
            state.scroll_offset = target_idx as f64;
            state.selected = Some(wid);
            Some(wid)
        } else {
            None
        }
    }

    fn layout_state(&mut self, layout: LayoutId) -> Option<&mut ScrollLayoutState> {
        self.layouts.get_mut(layout)
    }

    fn layout_state_ref(&self, layout: LayoutId) -> Option<&ScrollLayoutState> {
        self.layouts.get(layout)
    }
}

impl Default for ScrollLayoutSystem {
    fn default() -> Self { Self::from_settings(&ScrollLayoutSettings::default()) }
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
        let mut pixel_widths = Vec::with_capacity(len);
        let width_scale = available_width.max(MIN_WINDOW_DIMENSION);
        let default_width =
            (self.settings.default_window_ratio * width_scale).max(MIN_WINDOW_DIMENSION);
        for w in state.widths.iter().take(len) {
            let ratio = w.max(MIN_WIDTH_UNITS);
            pixel_widths.push((ratio * width_scale).max(MIN_WINDOW_DIMENSION));
        }
        if pixel_widths.len() < len {
            let missing = len - pixel_widths.len();
            pixel_widths.extend(std::iter::repeat(default_width).take(missing));
        }

        let mut left_offsets = Vec::with_capacity(len);
        let mut centers = Vec::with_capacity(len);
        let mut acc = 0.0;
        for width in &pixel_widths {
            left_offsets.push(acc);
            centers.push(acc + width / 2.0);
            acc += *width + gap;
        }

        let window_height = (available_height - inner.vertical).max(MIN_WINDOW_DIMENSION);
        let base_x = screen.origin.x + outer.left;
        let base_y =
            screen.origin.y + outer.top + (available_height - window_height).max(0.0) / 2.0;

        let offset = state.scroll_offset.clamp(0.0, state.max_offset());
        let (focus_index, frac) = if len <= 1 {
            (0usize, 0.0f64)
        } else {
            let max_index = len - 1;
            let clamped = offset.clamp(0.0, max_index as f64);
            let idx_floor = clamped.floor() as usize;
            let frac = (clamped - idx_floor as f64).clamp(0.0, 1.0);
            (idx_floor.min(max_index), frac)
        };

        let focus_center_rel = if frac > f64::EPSILON && focus_index + 1 < len {
            let current = centers[focus_index];
            let next = centers[focus_index + 1];
            current * (1.0 - frac) + next * frac
        } else {
            centers[focus_index]
        };

        let viewport_center = base_x + available_width * self.settings.center_factor();
        let center_adjust = viewport_center - (base_x + focus_center_rel);

        state
            .windows
            .iter()
            .enumerate()
            .map(|(idx, wid)| {
                let x_base = base_x + left_offsets[idx] + center_adjust;
                let frame = if state.direction.is_reverse() {
                    let mirrored_x =
                        base_x + available_width - (x_base - base_x) - pixel_widths[idx];
                    CGRect::new(
                        CGPoint::new(mirrored_x, base_y),
                        CGSize::new(pixel_widths[idx], window_height),
                    )
                } else {
                    CGRect::new(
                        CGPoint::new(x_base, base_y),
                        CGSize::new(pixel_widths[idx], window_height),
                    )
                };

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
        let default_ratio = self.settings.default_window_ratio;
        let state = match self.layout_state(layout) {
            Some(state) => state,
            None => return (None, Vec::new()),
        };

        if state.windows.is_empty() {
            state.selected = None;
            state.scroll_offset = 0.0;
            return (None, Vec::new());
        }

        state.ensure_selection(default_ratio);
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
        let default_ratio = self.settings.default_window_ratio;
        let Some(state) = self.layout_state(layout) else { return };

        let insert_idx = state.selected_index().map(|idx| idx + 1).unwrap_or(state.windows.len());
        state.windows.insert(insert_idx, wid);
        state.widths.insert(insert_idx, default_ratio);
        state.selected = Some(wid);
        state.scroll_offset = (insert_idx as f64).min(state.max_offset());
        state.ensure_widths(default_ratio);
    }

    fn remove_window(&mut self, wid: WindowId) {
        let default_ratio = self.settings.default_window_ratio;
        for state in self.layouts.values_mut() {
            if state.remove_window(wid, default_ratio) {
                state.ensure_selection(default_ratio);
                if let Some(idx) = state.selected_index() {
                    state.scroll_offset = idx as f64;
                } else {
                    state.scroll_offset = 0.0;
                }
            }
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        let default_ratio = self.settings.default_window_ratio;
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
            state.ensure_widths(default_ratio);
            if removed_selected {
                state.ensure_selection(default_ratio);
            } else {
                state.clamp_offset();
            }
            if let Some(sel_idx) = state.selected_index() {
                state.scroll_offset = sel_idx as f64;
            } else if state.windows.is_empty() {
                state.scroll_offset = 0.0;
            }
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let default_ratio = self.settings.default_window_ratio;
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
            state.ensure_widths(default_ratio);
            if removed_selected {
                state.ensure_selection(default_ratio);
            } else {
                state.clamp_offset();
            }
            if let Some(idx) = state.selected_index() {
                state.scroll_offset = idx as f64;
            } else {
                state.scroll_offset = 0.0;
            }
            return;
        }

        let insert_idx = first_index.unwrap_or(state.windows.len());
        for (offset, wid) in desired.iter().enumerate() {
            state.windows.insert(insert_idx + offset, *wid);
            state.widths.insert(insert_idx + offset, default_ratio);
        }

        if removed_selected {
            state.selected = Some(desired[0]);
            state.scroll_offset = (insert_idx as f64).min(state.max_offset());
        }

        state.ensure_selection(default_ratio);
        if let Some(idx) = state.selected_index() {
            state.scroll_offset = idx as f64;
        } else {
            state.scroll_offset = 0.0;
        }
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
        if let Some(idx) = state.selected_index() {
            state.scroll_offset = idx as f64;
        } else {
            state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
        }
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
        let _ = (screen, gaps);
        let default_ratio = self.settings.default_window_ratio;
        let Some(state) = self.layout_state(layout) else { return };
        let Some(idx) = state.windows.iter().position(|w| *w == wid) else {
            return;
        };
        if idx >= state.widths.len() {
            return;
        }

        state.ensure_widths(default_ratio);

        let old_span = old_frame.size.width.max(f64::EPSILON);
        let new_span = new_frame.size.width.max(MIN_WINDOW_DIMENSION);

        if old_span <= f64::EPSILON {
            return;
        }

        let ratio = (new_span / old_span).clamp(0.05, 20.0);
        state.widths[idx] = (state.widths[idx] * ratio).max(MIN_WIDTH_UNITS);
        state.ensure_widths(default_ratio);

        if let Some(sel_idx) = state.selected_index() {
            state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
            if sel_idx == idx {
                state.scroll_offset = sel_idx as f64;
            }
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
        if state.selected == Some(a) {
            state.scroll_offset = b_idx as f64;
        } else if state.selected == Some(b) {
            state.scroll_offset = a_idx as f64;
        }
        true
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let default_ratio = self.settings.default_window_ratio;
        let Some(state) = self.layout_state(layout) else {
            return false;
        };
        state.ensure_selection(default_ratio);
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
        let default_ratio = self.settings.default_window_ratio;
        let wid_opt = {
            let Some(from_state) = self.layout_state(from_layout) else {
                return;
            };
            from_state.ensure_selection(default_ratio);
            let Some(idx) = from_state.selected_index() else { return };
            let wid = from_state.windows.remove(idx);
            let width = if idx < from_state.widths.len() {
                from_state.widths.remove(idx)
            } else {
                default_ratio
            };
            if from_state.windows.is_empty() {
                from_state.selected = None;
                from_state.scroll_offset = 0.0;
            } else {
                let new_idx = idx.min(from_state.windows.len() - 1);
                from_state.selected = Some(from_state.windows[new_idx]);
                from_state.scroll_offset = new_idx as f64;
            }
            from_state.ensure_widths(default_ratio);
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
            to_state.ensure_widths(default_ratio);
            if let Some(idx) = to_state.selected_index() {
                to_state.scroll_offset = idx as f64;
            } else {
                to_state.scroll_offset = 0.0;
            }
        }
    }

    fn split_selection(&mut self, _layout: LayoutId, _kind: LayoutKind) {}

    fn toggle_tile_orientation(&mut self, layout: LayoutId) {
        let Some(state) = self.layout_state(layout) else { return };
        state.direction = state.direction.toggle();
        if let Some(idx) = state.selected_index() {
            state.scroll_offset = idx as f64;
        } else {
            state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
        }
    }

    fn toggle_fullscreen_of_selection(&mut self, _layout: LayoutId) -> Vec<WindowId> { Vec::new() }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, _layout: LayoutId) -> Vec<WindowId> {
        Vec::new()
    }

    fn join_selection_with_direction(&mut self, _layout: LayoutId, _direction: Direction) {}

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        _layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    fn unstack_parent_of_selection(&mut self, _layout: LayoutId) -> Vec<WindowId> { Vec::new() }

    fn unjoin_selection(&mut self, _layout: LayoutId) {}

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        if amount.abs() < f64::EPSILON {
            return;
        }
        let default_ratio = self.settings.default_window_ratio;
        let Some(state) = self.layout_state(layout) else { return };
        if state.windows.is_empty() {
            return;
        }

        state.ensure_selection(default_ratio);
        let Some(idx) = state.selected_index() else { return };

        state.widths[idx] = (state.widths[idx] + amount).max(MIN_WIDTH_UNITS);
        state.ensure_widths(default_ratio);
        state.scroll_offset = state.scroll_offset.clamp(0.0, state.max_offset());
    }

    fn rebalance(&mut self, layout: LayoutId) {
        let default_ratio = self.settings.default_window_ratio;
        if let Some(state) = self.layout_state(layout) {
            if !state.windows.is_empty() {
                state.widths.resize(state.windows.len(), default_ratio);
                for width in &mut state.widths {
                    *width = default_ratio;
                }
                state.ensure_selection(default_ratio);
                if let Some(idx) = state.selected_index() {
                    state.scroll_offset = idx as f64;
                } else {
                    state.scroll_offset = 0.0;
                }
            }
        }
    }
}
