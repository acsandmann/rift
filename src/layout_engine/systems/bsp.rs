use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::binary_tree::{BinaryTreeLayout, LayoutState, NodeKind, RatioPolicy};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::tree::NodeId;

struct BspRatioPolicy;

impl RatioPolicy for BspRatioPolicy {
    fn ratio_to_fraction(&self, ratio: f32) -> f64 { ratio as f64 }

    fn default_ratio(&self) -> f32 { 0.5 }
}

#[derive(Serialize, Deserialize)]
pub struct BspLayoutSystem {
    #[serde(flatten)]
    core: BinaryTreeLayout,
}

impl Default for BspLayoutSystem {
    fn default() -> Self {
        Self {
            core: BinaryTreeLayout::default(),
        }
    }
}

impl BspLayoutSystem {
    fn policy(&self) -> BspRatioPolicy { BspRatioPolicy }

    fn calculate_layout_recursive(
        &self,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        policy: &impl RatioPolicy,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        match self.core.kind.get(node) {
            Some(NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) => {
                if let Some(w) = window {
                    let target = if *fullscreen {
                        screen
                    } else if *fullscreen_within_gaps {
                        BinaryTreeLayout::apply_outer_gaps(screen, gaps)
                    } else {
                        rect
                    };
                    out.push((*w, target));
                }
            }
            Some(NodeKind::Split { orientation, ratio }) => match orientation {
                Orientation::Horizontal => {
                    let gap = gaps.inner.horizontal();
                    let total = rect.size.width;
                    let available = (total - gap).max(0.0);
                    let first_w_f = available * policy.ratio_to_fraction(*ratio);
                    let first_w = first_w_f.max(0.0);
                    let second_w = (available - first_w).max(0.0);
                    let r1 = CGRect::new(rect.origin, CGSize::new(first_w, rect.size.height));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x + first_w + gap, rect.origin.y),
                        CGSize::new(second_w, rect.size.height),
                    );
                    let mut it = node.children(&self.core.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, screen, gaps, policy, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, screen, gaps, policy, out);
                    }
                }
                Orientation::Vertical => {
                    let gap = gaps.inner.vertical();
                    let total = rect.size.height;
                    let available = (total - gap).max(0.0);
                    let first_h_f = available * policy.ratio_to_fraction(*ratio);
                    let first_h = first_h_f.max(0.0);
                    let second_h = (available - first_h).max(0.0);
                    let r1 = CGRect::new(rect.origin, CGSize::new(rect.size.width, first_h));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x, rect.origin.y + first_h + gap),
                        CGSize::new(rect.size.width, second_h),
                    );
                    let mut it = node.children(&self.core.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, screen, gaps, policy, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, screen, gaps, policy, out);
                    }
                }
            },
            None => {}
        }
    }

    fn smart_insert_window(&mut self, layout: LayoutId, window: WindowId) -> bool {
        if let Some(sel) = self.core.selection_of_layout(layout) {
            let leaf = self.core.descend_to_leaf(sel);

            if let Some(NodeKind::Leaf {
                preselected: Some(direction), ..
            }) = self.core.kind.get(leaf).cloned()
            {
                self.split_leaf_in_direction(leaf, direction, window);

                if let Some(NodeKind::Leaf { preselected, .. }) = self.core.kind.get_mut(leaf) {
                    *preselected = None;
                }
                return true;
            }
        }
        false
    }

    fn split_leaf_in_direction(
        &mut self,
        leaf: NodeId,
        direction: Direction,
        new_window: WindowId,
    ) {
        if let Some(NodeKind::Leaf { window, .. }) = self.core.kind.get(leaf).cloned() {
            let orientation = direction.orientation();

            let existing_node = self.core.make_leaf(window);
            let new_node = self.core.make_leaf(Some(new_window));

            if let Some(w) = window {
                self.core.window_to_node.insert(w, existing_node);
            }
            self.core.window_to_node.insert(new_window, new_node);

            self.core.kind.insert(leaf, NodeKind::Split { orientation, ratio: 0.5 });

            let (first_child, second_child) = match direction {
                Direction::Left | Direction::Up => (new_node, existing_node),
                Direction::Right | Direction::Down => (existing_node, new_node),
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
                    let left = self.core.make_leaf(existing);
                    let right = self.core.make_leaf(Some(wid));
                    self.core.window_to_node.insert(wid, right);
                    if let Some(w) = existing {
                        self.core.window_to_node.insert(w, left);
                    }
                    self.core.kind.insert(sel, NodeKind::Split {
                        orientation: Orientation::Horizontal,
                        ratio: 0.5,
                    });
                    left.detach(&mut self.core.tree).push_back(sel);
                    right.detach(&mut self.core.tree).push_back(sel);
                    self.core.tree.data.selection.select(&self.core.tree.map, right);
                }
            }
            Some(NodeKind::Split { .. }) => {
                let leaf = self.core.descend_to_leaf(sel);
                self.core.tree.data.selection.select(&self.core.tree.map, leaf);
                self.insert_window_at_selection(layout, wid);
            }
            None => {}
        }
    }

    fn remove_window_internal(&mut self, layout: LayoutId, wid: WindowId) {
        if let Some(&node_id) = self.core.window_to_node.get(&wid) {
            if let Some(state) = self.core.layouts.get(layout).copied() {
                if !self.core.belongs_to_layout(state, node_id) {
                    return;
                }
            }
            if let Some(NodeKind::Leaf { window, .. }) = self.core.kind.get_mut(node_id) {
                *window = None;
            }
            self.core.window_to_node.remove(&wid);
            let fallback = self.core.cleanup_after_removal(node_id);

            let sel_snapshot = self
                .core
                .layouts
                .get(layout)
                .map(|s| self.core.tree.data.selection.current_selection(s.root));
            let new_sel = match sel_snapshot {
                Some(sel) if self.core.kind.get(sel).is_some() => self.core.descend_to_leaf(sel),
                _ => self.core.descend_to_leaf(fallback),
            };
            self.core.tree.data.selection.select(&self.core.tree.map, new_sel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn window_in_direction_prefers_leftmost_when_moving_right() {
        let mut system = BspLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));

        assert_eq!(system.window_in_direction(layout, Direction::Right), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Left), Some(w(2)));
    }

    #[test]
    fn window_in_direction_prefers_top_for_down_direction_after_orientation_toggle() {
        let mut system = BspLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.toggle_tile_orientation(layout);

        assert_eq!(system.window_in_direction(layout, Direction::Down), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Up), Some(w(2)));
    }
}

impl LayoutSystem for BspLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let leaf = self.core.make_leaf(None);
        let state = LayoutState { root: leaf, preselection: None };
        self.core.layouts.insert(state)
    }

    /// shallow
    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let mut windows = Vec::new();
        if let Some(state) = self.core.layouts.get(layout).copied() {
            self.core.collect_windows_under(state.root, &mut windows);
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
            self.core.collect_windows_under(state.root, &mut windows);
            for w in windows {
                self.core.window_to_node.remove(&w);
            }
            let ids: Vec<_> = state.root.traverse_preorder(&self.core.tree.map).collect();
            for id in ids {
                self.core.kind.remove(id);
            }
            state.root.remove_root(&mut self.core.tree);
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
            let rect = BinaryTreeLayout::apply_outer_gaps(screen, gaps);
            self.calculate_layout_recursive(
                state.root,
                rect,
                screen,
                gaps,
                &self.policy(),
                &mut out,
            );
        }
        out
    }

    fn update_settings(&mut self, _settings: &crate::common::config::LayoutSettings) {}

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.core.layouts.get(layout).and_then(|s| self.core.selection_window(s))
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.core.visible_windows_in_layout(layout)
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        self.core.visible_windows_under_selection(layout)
    }

    fn set_insertion_point(&mut self, _layout: LayoutId, _point: Option<CGPoint>) {}

    fn set_preselection(&mut self, _layout: LayoutId, _direction: Option<Direction>) {}

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
            if !self.smart_insert_window(layout, wid) {
                self.insert_window_at_selection(layout, wid);
            }
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        if let Some(&node_id) = self.core.window_to_node.get(&wid) {
            if self.core.kind.get(node_id).is_none() {
                self.core.window_to_node.remove(&wid);
                return;
            }
            let root = self.core.find_layout_root(node_id);
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
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let desired_set: HashSet<WindowId> = desired.iter().copied().collect();
        let mut current_set: HashSet<WindowId> = HashSet::default();
        if let Some(state) = self.core.layouts.get(layout).copied() {
            let mut under: Vec<WindowId> = Vec::new();
            self.core.collect_windows_under(state.root, &mut under);
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
                                continue;
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
            self.core.collect_windows_under(state.root, &mut under);
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
                if !self.core.belongs_to_layout(state, node) {
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
        let target = self.core.descend_to_leaf(sel);
        match self.core.kind.get(target).cloned() {
            Some(NodeKind::Leaf { window, .. }) => {
                let left = self.core.make_leaf(window);
                let right = self.core.make_leaf(None);
                if let Some(w) = window {
                    self.core.window_to_node.insert(w, left);
                }
                self.core.kind.insert(target, NodeKind::Split {
                    orientation,
                    ratio: self.policy().default_ratio(),
                });
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

    // Stacking is not supported in BSP layout
    fn apply_stacking_to_parent_of_selection(
        &mut self,
        _: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    // Stacking is not supported in BSP layout
    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { false }

    // Stacking is not supported in BSP layout
    fn unstack_parent_of_selection(
        &mut self,
        _: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    fn unjoin_selection(&mut self, layout: LayoutId) { self.core.unjoin_selection(layout) }

    fn resize_active(
        &mut self,
        layout: LayoutId,
        delta_x: f64,
        delta_y: f64,
        corner: crate::layout_engine::ResizeCorner,
        frame: Option<&crate::layout_engine::LayoutFrame>,
        cursor: Option<CGPoint>,
    ) {
        let sel_snapshot = self.core.selection_of_layout(layout);
        let Some(node) = sel_snapshot else {
            return;
        };

        let (pinned_h, pinned_v) = (false, false);

        let rects_cache: Option<HashMap<NodeId, CGRect>> = None;
        let fallback_size = frame.map(|f| f.screen.size);

        let effective_corner = if matches!(corner, crate::layout_engine::ResizeCorner::None) {
            if let (Some(cursor), Some(rects)) = (cursor, rects_cache.as_ref()) {
                if let Some(rect) = rects.get(&node) {
                    let center = CGPoint::new(
                        rect.origin.x + rect.size.width / 2.0,
                        rect.origin.y + rect.size.height / 2.0,
                    );
                    crate::layout_engine::ResizeCorner::from_cursor_position(cursor, center)
                } else {
                    crate::layout_engine::ResizeCorner::BottomRight
                }
            } else {
                crate::layout_engine::ResizeCorner::BottomRight
            }
        } else {
            corner
        };

        if delta_x.abs() > 0.001 && !pinned_h {
            if let Some((parent, is_first_child)) = self.find_parent_split(node, Orientation::Horizontal) {
                self.apply_split_resize(
                    parent,
                    delta_x,
                    is_first_child,
                    effective_corner.affects_left(),
                    rects_cache.as_ref(),
                    fallback_size,
                );
            }
        }

        if delta_y.abs() > 0.001 && !pinned_v {
            if let Some((parent, is_first_child)) = self.find_parent_split(node, Orientation::Vertical) {
                self.apply_split_resize(
                    parent,
                    delta_y,
                    is_first_child,
                    effective_corner.affects_top(),
                    rects_cache.as_ref(),
                    fallback_size,
                );
            }
        }
    }

    // Rebalancing is not supported in BSP layout
    fn rebalance(&mut self, _layout: LayoutId) {}

    fn toggle_tile_orientation(&mut self, layout: LayoutId) {
        self.core.toggle_tile_orientation(layout)
    }

    fn toggle_split_of_selection(&mut self, _layout: LayoutId) {}

    fn swap_split_of_selection(&mut self, _layout: LayoutId) {}

    fn move_selection_to_root(&mut self, layout: LayoutId, stable: bool) {
        self.core.move_selection_to_root(layout, stable)
    }
}

impl BspLayoutSystem {
    fn find_parent_split(
        &self,
        start: NodeId,
        target_orientation: Orientation,
    ) -> Option<(NodeId, bool)> {
        let mut current = start;
        while let Some(parent) = current.parent(&self.core.tree.map) {
            if let Some(NodeKind::Split { orientation, .. }) = self.core.kind.get(parent) {
                if *orientation == target_orientation {
                    let is_first = Some(current) == parent.first_child(&self.core.tree.map);
                    return Some((parent, is_first));
                }
            }
            current = parent;
        }
        None
    }

    fn apply_split_resize(
        &mut self,
        split_node: NodeId,
        delta: f64,
        is_first_child: bool,
        affects_first_edge: bool,
        rects: Option<&HashMap<NodeId, CGRect>>,
        fallback_size: Option<CGSize>,
    ) {
        if let Some(NodeKind::Split { ratio, orientation, .. }) =
            self.core.kind.get_mut(split_node)
        {
            let parent_rect = rects.and_then(|r| r.get(&split_node).copied());
            let denom = match (orientation, parent_rect) {
                (Orientation::Horizontal, Some(r)) => r.size.width,
                (Orientation::Vertical, Some(r)) => r.size.height,
                (Orientation::Horizontal, None) => {
                    fallback_size.map(|s| s.width).unwrap_or(1000.0)
                }
                (Orientation::Vertical, None) => fallback_size.map(|s| s.height).unwrap_or(1000.0),
            };
            let ratio_delta = (delta * 2.0 / denom).clamp(-1.0, 1.0) as f32;
            let increase_ratio = if affects_first_edge { !is_first_child } else { is_first_child };
            if increase_ratio {
                *ratio = (*ratio + ratio_delta).clamp(0.05, 0.95);
            } else {
                *ratio = (*ratio - ratio_delta).clamp(0.05, 0.95);
            }
        }
    }
}
