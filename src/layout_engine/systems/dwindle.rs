use std::cell::RefCell;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::binary_tree::{BinaryTreeLayout, LayoutState, NodeKind, RatioPolicy};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::tree::NodeId;
use crate::sys::event::current_cursor_location;

#[derive(Clone)]
struct LastFrame {
    screen: CGRect,
    gaps: crate::common::config::GapSettings,
}

#[derive(Clone, Copy, Default)]
struct InsertionHint {
    cursor: Option<CGPoint>,
}

#[derive(Serialize, Deserialize)]
pub struct DwindleLayoutSystem {
    #[serde(flatten)]
    core: BinaryTreeLayout,
    settings: crate::common::config::DwindleSettings,
    #[serde(skip)]
    insertion_hints: HashMap<LayoutId, InsertionHint>,
    #[serde(skip)]
    last_frames: RefCell<HashMap<LayoutId, LastFrame>>,
    #[serde(skip)]
    pseudo_sizes: HashMap<WindowId, CGSize>,
}

impl Default for DwindleLayoutSystem {
    fn default() -> Self {
        Self {
            core: BinaryTreeLayout::default(),
            settings: crate::common::config::DwindleSettings::default(),
            insertion_hints: Default::default(),
            last_frames: Default::default(),
            pseudo_sizes: Default::default(),
        }
    }
}

struct DwindleRatioPolicy<'a> {
    settings: &'a crate::common::config::DwindleSettings,
}

impl RatioPolicy for DwindleRatioPolicy<'_> {
    fn ratio_to_fraction(&self, ratio: f32) -> f64 { (ratio as f64 / 2.0).clamp(0.05, 0.95) }

    fn default_ratio(&self) -> f32 { self.settings.default_split_ratio }
}

impl DwindleLayoutSystem {
    fn policy(&self) -> DwindleRatioPolicy<'_> { DwindleRatioPolicy { settings: &self.settings } }

    fn clamp_ratio(ratio: f32) -> f32 { ratio.clamp(0.1, 1.9) }

    /// Splits a rectangle into two parts WITHOUT subtracting gaps.
    /// This matches Hyprland's behavior where gaps are applied per-window at leaf level.
    fn compute_split_rects_no_gaps(
        &self,
        rect: CGRect,
        orientation: Orientation,
        ratio: f32,
    ) -> (CGRect, CGRect) {
        match orientation {
            Orientation::Horizontal => {
                let first_w = rect.size.width * self.policy().ratio_to_fraction(ratio);
                let second_w = rect.size.width - first_w;
                let r1 = CGRect::new(rect.origin, CGSize::new(first_w, rect.size.height));
                let r2 = CGRect::new(
                    CGPoint::new(rect.origin.x + first_w, rect.origin.y),
                    CGSize::new(second_w, rect.size.height),
                );
                (r1, r2)
            }
            Orientation::Vertical => {
                let first_h = rect.size.height * self.policy().ratio_to_fraction(ratio);
                let second_h = rect.size.height - first_h;
                let r1 = CGRect::new(rect.origin, CGSize::new(rect.size.width, first_h));
                let r2 = CGRect::new(
                    CGPoint::new(rect.origin.x, rect.origin.y + first_h),
                    CGSize::new(rect.size.width, second_h),
                );
                (r1, r2)
            }
        }
    }

    /// Applies gaps to a window rect with Hyprland-style edge detection.
    ///
    /// When a window is at a screen edge, outer gaps are used; otherwise inner gaps.
    /// This matches Hyprland's STICKS() macro behavior.
    fn apply_gaps_to_window(
        node_rect: CGRect,
        tiling_area: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> CGRect {
        const STICKS_TOLERANCE: f64 = 2.0;

        // Detect if window is at screen edges
        let at_left = (node_rect.origin.x - tiling_area.origin.x).abs() < STICKS_TOLERANCE;
        let at_right = ((node_rect.origin.x + node_rect.size.width)
            - (tiling_area.origin.x + tiling_area.size.width))
            .abs()
            < STICKS_TOLERANCE;
        let at_top = (node_rect.origin.y - tiling_area.origin.y).abs() < STICKS_TOLERANCE;
        let at_bottom = ((node_rect.origin.y + node_rect.size.height)
            - (tiling_area.origin.y + tiling_area.size.height))
            .abs()
            < STICKS_TOLERANCE;

        // Use outer gaps at edges, inner gaps between windows
        let left_gap = if at_left { gaps.outer.left } else { gaps.inner.left };
        let right_gap = if at_right { gaps.outer.right } else { gaps.inner.right };
        let top_gap = if at_top { gaps.outer.top } else { gaps.inner.top };
        let bottom_gap = if at_bottom { gaps.outer.bottom } else { gaps.inner.bottom };

        CGRect::new(
            CGPoint::new(node_rect.origin.x + left_gap, node_rect.origin.y + top_gap),
            CGSize::new(
                (node_rect.size.width - left_gap - right_gap).max(1.0),
                (node_rect.size.height - top_gap - bottom_gap).max(1.0),
            ),
        )
    }

    fn make_leaf(&mut self, window: Option<WindowId>) -> NodeId { self.core.make_leaf(window) }

    fn descend_to_leaf(&self, node: NodeId) -> NodeId { self.core.descend_to_leaf(node) }

    fn collect_windows_under(&self, node: NodeId, out: &mut Vec<WindowId>) {
        self.core.collect_windows_under(node, out);
    }

    fn find_layout_root(&self, node: NodeId) -> NodeId { self.core.find_layout_root(node) }

    fn belongs_to_layout(&self, layout: LayoutState, node: NodeId) -> bool {
        self.core.belongs_to_layout(layout, node)
    }

    fn cleanup_after_removal(&mut self, node: NodeId) -> NodeId {
        self.core.cleanup_after_removal(node)
    }

    fn selection_of_layout(&self, layout: crate::layout_engine::LayoutId) -> Option<NodeId> {
        self.core.selection_of_layout(layout)
    }

    /// Recursively calculates window positions using Hyprland-style gap handling.
    ///
    /// Key differences from the old approach:
    /// - Splits are calculated WITHOUT subtracting gaps
    /// - Gaps are applied per-window at leaf level based on edge detection
    /// - Orientation is mutated when preserve_split=false and smart_split=false
    fn calculate_layout_recursive(
        &mut self,
        node: NodeId,
        rect: CGRect,
        tiling_area: CGRect,
        gaps: &crate::common::config::GapSettings,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        match self.core.kind.get(node).cloned() {
            Some(NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) => {
                if let Some(w) = window {
                    let mut target = if fullscreen {
                        tiling_area
                    } else if fullscreen_within_gaps {
                        BinaryTreeLayout::apply_outer_gaps(tiling_area, gaps)
                    } else {
                        // Apply Hyprland-style edge-aware gaps at leaf level
                        Self::apply_gaps_to_window(rect, tiling_area, gaps)
                    };
                    if self.settings.pseudotile {
                        if let Some(size) = self.pseudo_sizes.get(&w) {
                            let mut desired_w = size.width;
                            let mut desired_h = size.height;
                            if desired_w <= 0.0 || desired_h <= 0.0 {
                                desired_w = target.size.width;
                                desired_h = target.size.height;
                            }
                            let scale = (target.size.width / desired_w)
                                .min(target.size.height / desired_h)
                                .min(1.0);
                            let new_w = (desired_w * scale).max(0.0);
                            let new_h = (desired_h * scale).max(0.0);
                            let origin_x =
                                target.origin.x + (target.size.width - new_w).max(0.0) / 2.0;
                            let origin_y =
                                target.origin.y + (target.size.height - new_h).max(0.0) / 2.0;
                            target = CGRect::new(
                                CGPoint::new(origin_x, origin_y),
                                CGSize::new(new_w, new_h),
                            );
                        }
                    }
                    out.push((w, target));
                }
            }
            Some(NodeKind::Split { orientation, ratio }) => {
                // HYPRLAND BEHAVIOR: Mutate orientation when preserve_split=false and smart_split=false
                let effective_orientation =
                    if !self.settings.preserve_split && !self.settings.smart_split {
                        let new_orientation = self.aspect_orientation(Some(rect));
                        // Mutate the stored orientation
                        if let Some(NodeKind::Split {
                            orientation: stored, ..
                        }) = self.core.kind.get_mut(node)
                        {
                            *stored = new_orientation;
                        }
                        new_orientation
                    } else {
                        orientation
                    };

                // Split WITHOUT gaps - gaps are applied at leaf level
                let (r1, r2) = self.compute_split_rects_no_gaps(rect, effective_orientation, ratio);
                let children: Vec<_> = node.children(&self.core.tree.map).collect();
                if let Some(&first) = children.first() {
                    self.calculate_layout_recursive(first, r1, tiling_area, gaps, out);
                }
                if let Some(&second) = children.get(1) {
                    self.calculate_layout_recursive(second, r2, tiling_area, gaps, out);
                }
            }
            None => {}
        }
    }

    fn single_window_rect(&self, rect: CGRect) -> CGRect {
        let (x, y) = self.settings.single_window_aspect_ratio;
        if y == 0.0 {
            return rect;
        }
        let requested_ratio = f64::from(x) / f64::from(y);
        let current_ratio = rect.size.width / rect.size.height;
        let tolerance = f64::from(self.settings.single_window_aspect_ratio_tolerance);
        if requested_ratio <= 0.0 || tolerance < 0.0 {
            return rect;
        }
        let mut new_rect = rect;
        if (requested_ratio - current_ratio).abs() / current_ratio < tolerance {
            return rect;
        }
        if requested_ratio > current_ratio {
            // need to reduce height
            let desired_h = rect.size.width / requested_ratio;
            let pad = (rect.size.height - desired_h).max(0.0) / 2.0;
            new_rect.origin.y += pad;
            new_rect.size.height -= 2.0 * pad;
        } else {
            // need to reduce width
            let desired_w = rect.size.height * requested_ratio;
            let pad = (rect.size.width - desired_w).max(0.0) / 2.0;
            new_rect.origin.x += pad;
            new_rect.size.width -= 2.0 * pad;
        }
        new_rect
    }

    fn selection_window(&self, state: &LayoutState) -> Option<WindowId> {
        let sel = self.core.tree.data.selection.current_selection(state.root);
        match self.core.kind.get(sel) {
            Some(NodeKind::Leaf { window, .. }) => *window,
            _ => None,
        }
    }

    fn store_last_frame(&self, layout: LayoutId, frame: LastFrame) {
        if let Ok(mut map) = self.last_frames.try_borrow_mut() {
            map.insert(layout, frame);
        }
    }

    fn rects_for_layout(&self, layout: LayoutId) -> Option<HashMap<NodeId, CGRect>> {
        let state = self.core.layouts.get(layout)?;
        let frame = self.last_frames.try_borrow().ok()?.get(&layout)?.clone();
        let mut rects = HashMap::default();
        let root_rect = BinaryTreeLayout::apply_outer_gaps(frame.screen, &frame.gaps);
        self.populate_rects(state.root, root_rect, frame.screen, &frame.gaps, &mut rects);
        Some(rects)
    }

    fn populate_rects(
        &self,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        out: &mut HashMap<NodeId, CGRect>,
    ) {
        // For hit testing, store rects without gaps (Hyprland-style)
        out.insert(node, rect);
        if let Some(NodeKind::Split { orientation, ratio }) = self.core.kind.get(node) {
            let effective_orientation = self.effective_orientation(rect, *orientation);
            // Use no-gaps split for consistency with Hyprland
            let (r1, r2) = self.compute_split_rects_no_gaps(rect, effective_orientation, *ratio);
            let mut it = node.children(&self.core.tree.map);
            if let Some(first) = it.next() {
                self.populate_rects(first, r1, screen, gaps, out);
            }
            if let Some(second) = it.next() {
                self.populate_rects(second, r2, screen, gaps, out);
            }
        }
    }

    fn choose_target_leaf(&self, layout: LayoutId) -> Option<NodeId> {
        let settings = &self.settings;

        // Prefer active selection if enabled
        if settings.use_active_for_splits {
            if let Some(sel) = self.selection_of_layout(layout) {
                return Some(self.descend_to_leaf(sel));
            }
        }

        // Try cursor-based hit test
        if let Some(hint) = self.insertion_hints.get(&layout) {
            if let Some(cursor) = hint.cursor {
                if let Some(rects) = self.rects_for_layout(layout) {
                    let mut best = None;
                    let mut best_dist = f64::MAX;
                    for (node, rect) in rects {
                        let min_x = rect.origin.x;
                        let max_x = rect.origin.x + rect.size.width;
                        let min_y = rect.origin.y;
                        let max_y = rect.origin.y + rect.size.height;
                        let inside = cursor.x >= min_x
                            && cursor.x <= max_x
                            && cursor.y >= min_y
                            && cursor.y <= max_y;
                        if inside {
                            return Some(self.descend_to_leaf(node));
                        }
                        let dx = if cursor.x < min_x {
                            min_x - cursor.x
                        } else if cursor.x > max_x {
                            cursor.x - max_x
                        } else {
                            0.0
                        };
                        let dy = if cursor.y < min_y {
                            min_y - cursor.y
                        } else if cursor.y > max_y {
                            cursor.y - max_y
                        } else {
                            0.0
                        };
                        let dist = dx * dx + dy * dy;
                        if dist < best_dist {
                            best_dist = dist;
                            best = Some(node);
                        }
                    }
                    if let Some(node) = best {
                        return Some(self.descend_to_leaf(node));
                    }
                }
            }
        }

        // Final fallback to selection
        self.selection_of_layout(layout).map(|sel| self.descend_to_leaf(sel))
    }

    fn aspect_orientation(&self, rect: Option<CGRect>) -> Orientation {
        if let Some(r) = rect {
            if r.size.width > r.size.height * self.settings.split_width_multiplier as f64 {
                Orientation::Horizontal
            } else {
                Orientation::Vertical
            }
        } else {
            Orientation::Horizontal
        }
    }

    fn effective_orientation(&self, rect: CGRect, stored: Orientation) -> Orientation {
        if self.settings.preserve_split || self.settings.smart_split {
            stored
        } else {
            self.aspect_orientation(Some(rect))
        }
    }

    fn plan_split(
        &mut self,
        layout: LayoutId,
        target: NodeId,
        target_rect: Option<CGRect>,
    ) -> (Orientation, bool) {
        if self.settings.preserve_split {
            if let Some(parent) = target.parent(&self.core.tree.map) {
                if let Some(NodeKind::Split { orientation, .. }) = self.core.kind.get(parent) {
                    return (*orientation, false);
                }
            }
        }
        let mut new_first = false;
        if let Some(state) = self.core.layouts.get_mut(layout) {
            if let Some(dir) = state.preselection {
                let orientation = dir.orientation();
                new_first = matches!(dir, Direction::Left | Direction::Up);
                if !self.settings.permanent_direction_override {
                    state.preselection = None;
                }
                return (orientation, new_first);
            }
        }

        if let Some(hint) = self.insertion_hints.get(&layout) {
            if let Some(cursor) = hint.cursor {
                if self.settings.smart_split {
                    if let Some(rect) = target_rect {
                        let center = CGPoint::new(
                            rect.origin.x + rect.size.width / 2.0,
                            rect.origin.y + rect.size.height / 2.0,
                        );
                        let delta_x = cursor.x - center.x;
                        let delta_y = cursor.y - center.y;
                        let slope = if delta_x.abs() < f64::EPSILON {
                            f64::INFINITY
                        } else {
                            delta_y / delta_x
                        };
                        let aspect = if rect.size.width.abs() < f64::EPSILON {
                            f64::INFINITY
                        } else {
                            rect.size.height / rect.size.width
                        };
                        if slope.abs() < aspect {
                            new_first = delta_x < 0.0;
                            return (Orientation::Horizontal, new_first);
                        } else {
                            new_first = delta_y < 0.0;
                            return (Orientation::Vertical, new_first);
                        }
                    }
                } else if let Some(rect) = target_rect {
                    let center = CGPoint::new(
                        rect.origin.x + rect.size.width / 2.0,
                        rect.origin.y + rect.size.height / 2.0,
                    );
                    let orientation = self.aspect_orientation(target_rect);
                    new_first = match orientation {
                        Orientation::Horizontal => cursor.x <= center.x,
                        Orientation::Vertical => cursor.y <= center.y,
                    };
                    return (orientation, new_first);
                }
            }
        }

        if self.settings.force_split == 1 {
            new_first = true;
        } else if self.settings.force_split == 2 {
            new_first = false;
        }

        let orientation = self.aspect_orientation(target_rect);
        (orientation, new_first)
    }

    fn split_leaf(
        &mut self,
        layout: LayoutId,
        leaf: NodeId,
        new_window: WindowId,
        target_rect: Option<CGRect>,
    ) {
        if let Some(NodeKind::Leaf { window, .. }) = self.core.kind.get(leaf).cloned() {
            let (orientation, new_first) = self.plan_split(layout, leaf, target_rect);
            let mut ratio = Self::clamp_ratio(self.settings.default_split_ratio);

            let existing_node = self.make_leaf(window);
            let new_node = self.make_leaf(Some(new_window));

            if let Some(w) = window {
                self.core.window_to_node.insert(w, existing_node);
            }
            self.core.window_to_node.insert(new_window, new_node);

            if self.settings.split_bias && new_first {
                ratio = Self::clamp_ratio(2.0 - ratio);
            }

            self.core.kind.insert(leaf, NodeKind::Split { orientation, ratio });

            let (first_child, second_child) = if new_first {
                (new_node, existing_node)
            } else {
                (existing_node, new_node)
            };

            first_child.detach(&mut self.core.tree).push_back(leaf);
            second_child.detach(&mut self.core.tree).push_back(leaf);

            self.core.tree.data.selection.select(&self.core.tree.map, new_node);
        }
    }

    fn insert_window_at_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let Some(state) = self.core.layouts.get(layout).copied() else {
            return;
        };
        let sel = self.core.tree.data.selection.current_selection(state.root);
        match self.core.kind.get_mut(sel) {
            Some(NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) => {
                if window.is_none() {
                    *window = Some(wid);
                    *fullscreen = false;
                    *fullscreen_within_gaps = false;
                    self.core.window_to_node.insert(wid, sel);
                } else {
                    let existing = *window;
                    let left = self.make_leaf(existing);
                    let right = self.make_leaf(Some(wid));
                    self.core.window_to_node.insert(wid, right);
                    if let Some(w) = existing {
                        self.core.window_to_node.insert(w, left);
                    }
                    let ratio = Self::clamp_ratio(self.settings.default_split_ratio);
                    self.core.kind.insert(sel, NodeKind::Split {
                        orientation: Orientation::Horizontal,
                        ratio,
                    });
                    left.detach(&mut self.core.tree).push_back(sel);
                    right.detach(&mut self.core.tree).push_back(sel);
                    self.core.tree.data.selection.select(&self.core.tree.map, right);
                }
            }
            Some(NodeKind::Split { .. }) => {
                let leaf = self.descend_to_leaf(sel);
                self.core.tree.data.selection.select(&self.core.tree.map, leaf);
                self.insert_window_at_selection(layout, wid);
            }
            None => {}
        }
    }

    fn remove_window_internal(&mut self, layout: crate::layout_engine::LayoutId, wid: WindowId) {
        if let Some(&node_id) = self.core.window_to_node.get(&wid) {
            if let Some(state) = self.core.layouts.get(layout).copied() {
                if !self.belongs_to_layout(state, node_id) {
                    return;
                }
            }
            if let Some(NodeKind::Leaf { window, .. }) = self.core.kind.get_mut(node_id) {
                *window = None;
            }
            self.core.window_to_node.remove(&wid);
            self.pseudo_sizes.remove(&wid);
            let fallback = self.cleanup_after_removal(node_id);

            let sel_snapshot = self
                .core
                .layouts
                .get(layout)
                .map(|s| self.core.tree.data.selection.current_selection(s.root));
            let new_sel = match sel_snapshot {
                Some(sel) if self.core.kind.get(sel).is_some() => self.descend_to_leaf(sel),
                _ => self.descend_to_leaf(fallback),
            };
            self.core.tree.data.selection.select(&self.core.tree.map, new_sel);
        }
    }
}

impl LayoutSystem for DwindleLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let leaf = self.make_leaf(None);
        let state = LayoutState { root: leaf, preselection: None };
        let id = self.core.layouts.insert(state);
        self.insertion_hints.insert(id, InsertionHint::default());
        id
    }

    /// shallow
    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let mut windows = Vec::new();
        if let Some(state) = self.core.layouts.get(layout).copied() {
            self.collect_windows_under(state.root, &mut windows);
        }
        let new_layout = self.create_layout();
        for w in windows {
            self.add_window_after_selection(new_layout, w);
        }
        new_layout
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        if let Some(state) = self.core.layouts.remove(layout) {
            let mut windows = Vec::new();
            self.collect_windows_under(state.root, &mut windows);
            for w in windows {
                self.core.window_to_node.remove(&w);
                self.pseudo_sizes.remove(&w);
            }
            let ids: Vec<_> = state.root.traverse_preorder(&self.core.tree.map).collect();
            for id in ids {
                self.core.kind.remove(id);
            }
            state.root.remove_root(&mut self.core.tree);
        }
        self.insertion_hints.remove(&layout);
        if let Ok(mut map) = self.last_frames.try_borrow_mut() {
            map.remove(&layout);
        }
    }

    fn draw_tree(&self, layout: LayoutId) -> String { self.core.draw_tree(layout) }

    fn calculate_layout(
        &mut self,
        layout: LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        _stack_line_thickness: f64,
        _stack_line_horiz: crate::common::config::HorizontalPlacement,
        _stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let mut out = Vec::new();
        if let Some(state) = self.core.layouts.get(layout).copied() {
            // Hyprland-style: start with raw screen bounds
            // Single window aspect ratio is applied to screen (before gaps)
            // Gaps are applied per-window at leaf level based on edge detection
            let mut rect = screen;
            if self.visible_windows_in_layout(layout).len() == 1 {
                rect = self.single_window_rect(screen);
            }
            self.calculate_layout_recursive(state.root, rect, screen, gaps, &mut out);
        }
        self.store_last_frame(layout, LastFrame { screen, gaps: gaps.clone() });
        out
    }

    fn update_settings(&mut self, settings: &crate::common::config::LayoutSettings) {
        self.settings = settings.dwindle.clone();
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.core.layouts.get(layout).and_then(|s| self.selection_window(s))
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.core.visible_windows_in_layout(layout)
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        self.core.visible_windows_under_selection(layout)
    }

    fn set_insertion_point(&mut self, layout: LayoutId, point: Option<CGPoint>) {
        self.insertion_hints.entry(layout).or_default().cursor = point;
    }

    fn set_preselection(&mut self, layout: LayoutId, direction: Option<Direction>) {
        if let Some(state) = self.core.layouts.get_mut(layout) {
            state.preselection = direction;
        }
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool { self.core.ascend_selection(layout) }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        self.core.descend_selection(layout)
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        self.core.move_focus(layout, direction)
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.core.window_in_direction(layout, direction)
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.core.layouts.get(layout).is_some() {
            let target = self.choose_target_leaf(layout);

            if let Some(target_leaf) = target {
                match self.core.kind.get(target_leaf) {
                    Some(NodeKind::Leaf { window, .. }) => {
                        if window.is_none() {
                            if let Some(NodeKind::Leaf { window, .. }) =
                                self.core.kind.get_mut(target_leaf)
                            {
                                *window = Some(wid);
                                self.core.window_to_node.insert(wid, target_leaf);
                            }
                            self.core.tree.data.selection.select(&self.core.tree.map, target_leaf);
                        } else {
                            let rect = self
                                .rects_for_layout(layout)
                                .and_then(|m| m.get(&target_leaf).copied());
                            self.split_leaf(layout, target_leaf, wid, rect);
                        }
                    }
                    _ => {}
                }
            } else {
                self.insert_window_at_selection(layout, wid);
            }
        }
        self.insertion_hints.remove(&layout);
    }

    fn remove_window(&mut self, wid: WindowId) {
        if let Some(&node_id) = self.core.window_to_node.get(&wid) {
            if self.core.kind.get(node_id).is_none() {
                self.core.window_to_node.remove(&wid);
                self.pseudo_sizes.remove(&wid);
                return;
            }
            let root = self.find_layout_root(node_id);
            let layout = self
                .core
                .layouts
                .iter()
                .find_map(|(id, s)| if s.root == root { Some(id) } else { None });
            if let Some(l) = layout {
                self.remove_window_internal(l, wid);
            }
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        let windows: Vec<_> =
            self.core.window_to_node.keys().copied().filter(|w| w.pid == pid).collect();
        for w in windows {
            self.remove_window(w);
        }
        self.pseudo_sizes.retain(|wid, _| wid.pid != pid);
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let desired_set: HashSet<WindowId> = desired.iter().copied().collect();
        let mut current_set: HashSet<WindowId> = HashSet::default();
        if let Some(state) = self.core.layouts.get(layout).copied() {
            let mut under: Vec<WindowId> = Vec::new();
            self.collect_windows_under(state.root, &mut under);
            for w in under.into_iter().filter(|w| w.pid == pid) {
                current_set.insert(w);
                if !desired_set.contains(&w) {
                    if let Some(&node) = self.core.window_to_node.get(&w) {
                        if let Some(NodeKind::Leaf {
                            fullscreen,
                            fullscreen_within_gaps,
                            ..
                        }) = self.core.kind.get(node)
                        {
                            if *fullscreen || *fullscreen_within_gaps {
                                continue; // keep fullscreen node in tree
                            }
                        }
                    }
                    self.remove_window_internal(layout, w);
                }
            }
        }
        for w in desired {
            if !current_set.contains(&w) {
                self.add_window_after_selection(layout, w);
            }
        }
    }

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        if let Some(state) = self.core.layouts.get(layout).copied() {
            let mut under = Vec::new();
            self.collect_windows_under(state.root, &mut under);
            under.into_iter().any(|w| w.pid == pid)
        } else {
            false
        }
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.core.contains_window(layout, wid)
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        self.core.select_window(layout, wid)
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
        if let Some(&node) = self.core.window_to_node.get(&wid) {
            if let Some(state) = self.core.layouts.get(layout).copied() {
                if !self.belongs_to_layout(state, node) {
                    return;
                }
                if let Some(NodeKind::Leaf {
                    window: _,
                    fullscreen,
                    fullscreen_within_gaps,
                    ..
                }) = self.core.kind.get_mut(node)
                {
                    if new_frame == screen {
                        *fullscreen = true;
                        *fullscreen_within_gaps = false;
                    } else if old_frame == screen {
                        *fullscreen = false;
                    } else {
                        let tiling = BinaryTreeLayout::apply_outer_gaps(screen, gaps);
                        if new_frame == tiling {
                            *fullscreen_within_gaps = true;
                            *fullscreen = false;
                        } else if old_frame == tiling {
                            *fullscreen_within_gaps = false;
                        } else if self.settings.pseudotile {
                            self.pseudo_sizes.insert(wid, new_frame.size);
                        }
                    }
                }
            }
        }
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        self.core.move_selection(layout, direction)
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        self.core.swap_windows(layout, a, b)
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        let sel = self.selected_window(from_layout);
        if let Some(w) = sel {
            self.remove_window_internal(from_layout, w);
            self.add_window_after_selection(to_layout, w);
        }
    }

    fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind) {
        let orientation = match kind {
            LayoutKind::Horizontal => Orientation::Horizontal,
            LayoutKind::Vertical => Orientation::Vertical,
            _ => return,
        };
        let state = if let Some(s) = self.core.layouts.get(layout).copied() {
            s
        } else {
            return;
        };

        let sel = self.core.tree.data.selection.current_selection(state.root);
        let target = self.descend_to_leaf(sel);
        match self.core.kind.get(target).cloned() {
            Some(NodeKind::Leaf { window, .. }) => {
                let left = self.make_leaf(window);
                let right = self.make_leaf(None);
                if let Some(w) = window {
                    self.core.window_to_node.insert(w, left);
                }
                let ratio = Self::clamp_ratio(self.settings.default_split_ratio);
                self.core.kind.insert(target, NodeKind::Split { orientation, ratio });
                left.detach(&mut self.core.tree).push_back(target);
                right.detach(&mut self.core.tree).push_back(target);
                self.core.tree.data.selection.select(&self.core.tree.map, right);
            }
            _ => {}
        }
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.core.toggle_fullscreen_of_selection(layout)
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.core.toggle_fullscreen_within_gaps_of_selection(layout)
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        self.core.join_selection_with_direction(layout, direction)
    }

    // Stacking is not supported in Dwindle layout
    fn apply_stacking_to_parent_of_selection(
        &mut self,
        _: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    // Stacking is not supported in Dwindle layout
    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { false }

    // Stacking is not supported in Dwindle layout
    fn unstack_parent_of_selection(
        &mut self,
        _: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    fn unjoin_selection(&mut self, layout: LayoutId) { self.core.unjoin_selection(layout) }

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        if self.settings.smart_resizing {
            if let (Some(cursor), Some(rects)) =
                (current_cursor_location().ok(), self.rects_for_layout(layout))
            {
                let sel_snapshot = self.selection_of_layout(layout);
                if let Some(mut node) = sel_snapshot {
                    let leaf = self.descend_to_leaf(node);
                    if let Some(rect) = rects.get(&leaf).copied() {
                        let min_x = rect.origin.x;
                        let max_x = rect.origin.x + rect.size.width;
                        let min_y = rect.origin.y;
                        let max_y = rect.origin.y + rect.size.height;
                        let dist_left = (cursor.x - min_x).abs();
                        let dist_right = (cursor.x - max_x).abs();
                        let dist_top = (cursor.y - min_y).abs();
                        let dist_bottom = (cursor.y - max_y).abs();
                        let (orientation, dir_is_first_side) =
                            if dist_left.min(dist_right) < dist_top.min(dist_bottom) {
                                // horizontal axis
                                let dir_first = cursor.x <= (min_x + max_x) / 2.0;
                                (Orientation::Horizontal, dir_first)
                            } else {
                                let dir_first = cursor.y <= (min_y + max_y) / 2.0;
                                (Orientation::Vertical, dir_first)
                            };

                        while let Some(parent) = node.parent(&self.core.tree.map) {
                            let stored_orientation = match self.core.kind.get(parent) {
                                Some(NodeKind::Split { orientation: o, .. }) => Some(*o),
                                _ => None,
                            };
                            let parent_orientation = stored_orientation
                                .map(|o| {
                                    rects
                                        .get(&parent)
                                        .copied()
                                        .map(|r| self.effective_orientation(r, o))
                                        .unwrap_or(o)
                                })
                                .unwrap_or(Orientation::Horizontal);

                            if parent_orientation == orientation {
                                if let Some(NodeKind::Split { ratio, .. }) =
                                    self.core.kind.get_mut(parent)
                                {
                                    let is_first =
                                        Some(node) == parent.first_child(&self.core.tree.map);
                                    let delta = amount as f32;
                                    match orientation {
                                        Orientation::Horizontal => {
                                            if dir_is_first_side {
                                                if is_first {
                                                    *ratio = (*ratio - delta).clamp(0.1, 1.9);
                                                } else {
                                                    *ratio = (*ratio + delta).clamp(0.1, 1.9);
                                                }
                                            } else if is_first {
                                                *ratio = (*ratio + delta).clamp(0.1, 1.9);
                                            } else {
                                                *ratio = (*ratio - delta).clamp(0.1, 1.9);
                                            }
                                        }
                                        Orientation::Vertical => {
                                            if dir_is_first_side {
                                                if is_first {
                                                    *ratio = (*ratio - delta).clamp(0.1, 1.9);
                                                } else {
                                                    *ratio = (*ratio + delta).clamp(0.1, 1.9);
                                                }
                                            } else if is_first {
                                                *ratio = (*ratio + delta).clamp(0.1, 1.9);
                                            } else {
                                                *ratio = (*ratio - delta).clamp(0.1, 1.9);
                                            }
                                        }
                                    }
                                    return;
                                }
                            }
                            node = parent;
                        }
                    }
                }
            }
        }
        let sel_snapshot = self.selection_of_layout(layout);
        let Some(mut node) = sel_snapshot else {
            return;
        };

        while let Some(parent) = node.parent(&self.core.tree.map) {
            if let Some(NodeKind::Split { ratio, .. }) = self.core.kind.get_mut(parent) {
                let is_first = Some(node) == parent.first_child(&self.core.tree.map);
                let delta = amount as f32;
                if is_first {
                    let new_ratio = (*ratio - delta).clamp(0.1, 1.9);
                    *ratio = new_ratio;
                } else {
                    let new_ratio = (*ratio + delta).clamp(0.1, 1.9);
                    *ratio = new_ratio;
                }
                break;
            }
            node = parent;
        }
    }

    // Rebalancing is not supported in Dwindle layout
    fn rebalance(&mut self, _layout: LayoutId) {}

    fn toggle_tile_orientation(&mut self, layout: LayoutId) {
        self.core.toggle_tile_orientation(layout)
    }

    fn toggle_split_of_selection(&mut self, layout: LayoutId) {
        if let Some(sel) = self.selection_of_layout(layout) {
            let sel_leaf = self.descend_to_leaf(sel);
            if let Some(parent) = sel_leaf.parent(&self.core.tree.map) {
                if let Some(NodeKind::Split { orientation, .. }) = self.core.kind.get_mut(parent) {
                    *orientation = match *orientation {
                        Orientation::Horizontal => Orientation::Vertical,
                        Orientation::Vertical => Orientation::Horizontal,
                    };
                }
            }
        }
    }

    fn swap_split_of_selection(&mut self, layout: LayoutId) {
        if let Some(sel) = self.selection_of_layout(layout) {
            let sel_leaf = self.descend_to_leaf(sel);
            if let Some(parent) = sel_leaf.parent(&self.core.tree.map) {
                let children: Vec<_> = parent.children(&self.core.tree.map).collect();
                if children.len() == 2 {
                    let first_id = children[0];
                    let second_id = children[1];
                    let detached_second = second_id.detach(&mut self.core.tree);
                    detached_second.insert_before(first_id).finish();
                }
            }
        }
    }

    fn move_selection_to_root(&mut self, layout: LayoutId, stable: bool) {
        self.core.move_selection_to_root(layout, stable)
    }
}
