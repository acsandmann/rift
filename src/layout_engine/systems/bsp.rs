use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};
use slotmap::Key;

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
use crate::common::config::WindowInsertionPoint;
use crate::layout_engine::systems::constraints::{AxisConstraints, solve_axis_lengths};
use crate::layout_engine::systems::{
    LayoutSystem, WindowLayoutConstraints, reconcile_app_membership,
};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation, ResizeOrientation};
use crate::model::selection::*;
use crate::model::tree::{NodeId, NodeMap, Tree};

#[derive(Serialize, Deserialize, Clone, Debug)]
enum NodeKind {
    Split {
        orientation: Orientation,
        ratio: f32,
    },
    Leaf {
        window: Option<WindowId>,
        fullscreen: bool,
        fullscreen_within_gaps: bool,
        preselected: Option<Direction>,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct LayoutState {
    root: NodeId,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct BspLayoutSystem {
    layouts: slotmap::SlotMap<crate::layout_engine::LayoutId, LayoutState>,
    tree: Tree<Components>,
    kind: slotmap::SecondaryMap<NodeId, NodeKind>,
    /// Ordered members for BSP leaves that have been explicitly stacked.
    #[serde(default)]
    stacks: HashMap<NodeId, Vec<WindowId>>,
    #[serde(skip, default)]
    window_insertion_point: WindowInsertionPoint,
    #[serde(skip, default)]
    single_window_aspect_ratio: Option<f64>,
}

impl BspLayoutSystem {
    fn find_neighbor_leaf(&self, from_leaf: NodeId, direction: Direction) -> Option<NodeId> {
        // Track the source center across the movement axis, relative to each ancestor.
        let mut anchor = 0.5;
        let mut current = from_leaf;

        while let Some(parent) = current.parent(&self.tree.map) {
            if let Some(NodeKind::Split { orientation, ratio }) = self.kind.get(parent) {
                if *orientation == direction.orientation() {
                    let children: Vec<_> = parent.children(&self.tree.map).collect();
                    if children.len() == 2 {
                        let is_first = children[0] == current;
                        let target_child = match direction {
                            Direction::Left | Direction::Up => {
                                if !is_first {
                                    Some(children[0])
                                } else {
                                    None
                                }
                            }
                            Direction::Right | Direction::Down => {
                                if is_first {
                                    Some(children[1])
                                } else {
                                    None
                                }
                            }
                        };

                        if let Some(target) = target_child {
                            return Some(
                                self.find_closest_leaf_in_direction(target, direction, anchor),
                            );
                        }
                    }
                } else {
                    let ratio = f64::from(*ratio);
                    anchor = if parent.first_child(&self.tree.map) == Some(current) {
                        anchor * ratio
                    } else {
                        ratio + anchor * (1.0 - ratio)
                    };
                }
            }
            current = parent;
        }

        None
    }

    fn find_closest_leaf_in_direction(
        &self,
        root: NodeId,
        direction: Direction,
        mut anchor: f64,
    ) -> NodeId {
        match self.kind.get(root) {
            Some(NodeKind::Leaf { .. }) => root,
            Some(NodeKind::Split { orientation, ratio }) => {
                let children: Vec<_> = root.children(&self.tree.map).collect();
                if children.is_empty() {
                    return root;
                }

                let target_child = if *orientation == direction.orientation() {
                    match direction {
                        Direction::Left | Direction::Up => children.last().copied(),
                        Direction::Right | Direction::Down => children.first().copied(),
                    }
                } else {
                    let ratio = f64::from(*ratio);
                    // Choose the adjacent side and express the anchor within that child.
                    if anchor <= ratio {
                        anchor /= ratio;
                        children.first().copied()
                    } else {
                        anchor = (anchor - ratio) / (1.0 - ratio);
                        children.last().copied()
                    }
                };

                if let Some(child) = target_child {
                    self.find_closest_leaf_in_direction(child, direction, anchor)
                } else {
                    root
                }
            }
            None => root,
        }
    }

    fn window_in_direction_from(&self, node: NodeId, direction: Direction) -> Option<WindowId> {
        match self.kind.get(node) {
            Some(NodeKind::Leaf { window: Some(w), .. }) => Some(*w),
            Some(NodeKind::Leaf { .. }) => None,
            Some(NodeKind::Split { .. }) => {
                let mut children: Vec<_> = node.children(&self.tree.map).collect();
                match direction {
                    Direction::Left | Direction::Up => children.reverse(),
                    Direction::Right | Direction::Down => {}
                }
                for child in children {
                    if let Some(window) = self.window_in_direction_from(child, direction) {
                        return Some(window);
                    }
                }
                None
            }
            None => None,
        }
    }

    fn smart_insert_window(&mut self, layout: LayoutId, window: WindowId) -> bool {
        if let Some(sel) = self.selection_of_layout(layout) {
            let leaf = self.descend_to_leaf(sel);

            if let Some(NodeKind::Leaf {
                preselected: Some(direction), ..
            }) = self.kind.get(leaf).cloned()
            {
                self.split_leaf_in_direction(leaf, direction, window);

                if let Some(NodeKind::Leaf { preselected, .. }) = self.kind.get_mut(leaf) {
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
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get(leaf).cloned() {
            let existing_stack = self.stacks.remove(&leaf);
            let orientation = direction.orientation();

            let existing_node = self.make_leaf(window);
            let new_node = self.make_leaf(Some(new_window));

            if let Some(stack) = existing_stack {
                self.stacks.insert(existing_node, stack);
            }

            self.kind.insert(leaf, NodeKind::Split { orientation, ratio: 0.5 });

            let (first_child, second_child) = match direction {
                Direction::Left | Direction::Up => (new_node, existing_node),
                Direction::Right | Direction::Down => (existing_node, new_node),
            };

            first_child.detach(&mut self.tree).push_back(leaf);
            second_child.detach(&mut self.tree).push_back(leaf);

            self.tree.data.selection.select(&self.tree.map, new_node);
        }
    }
}

impl Default for BspLayoutSystem {
    fn default() -> Self {
        Self {
            layouts: Default::default(),
            tree: Tree::with_observer(Components::default()),
            kind: Default::default(),
            stacks: Default::default(),
            window_insertion_point: WindowInsertionPoint::default(),
            single_window_aspect_ratio: None,
        }
    }
}

impl BspLayoutSystem {
    pub fn new(window_insertion_point: WindowInsertionPoint) -> Self {
        Self {
            window_insertion_point,
            ..Self::default()
        }
    }

    pub fn set_window_insertion_point(&mut self, value: WindowInsertionPoint) {
        self.window_insertion_point = value;
    }

    pub fn set_single_window_aspect_ratio(&mut self, ratio: Option<f64>) {
        self.single_window_aspect_ratio = ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    }

    fn node_for_window_in_layout(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        let root = self.layouts.get(layout)?.root;
        root.traverse_preorder(&self.tree.map).find(|&node| match self.kind.get(node) {
            Some(NodeKind::Leaf { window, .. }) => self
                .stacks
                .get(&node)
                .map_or(*window == Some(wid), |stack| stack.contains(&wid)),
            _ => false,
        })
    }

    fn make_leaf(&mut self, window: Option<WindowId>) -> NodeId {
        let id = self.tree.mk_node().into_id();
        self.kind.insert(id, NodeKind::Leaf {
            window,
            fullscreen: false,
            fullscreen_within_gaps: false,
            preselected: None,
        });
        id
    }

    fn descend_to_leaf(&self, mut node: NodeId) -> NodeId {
        loop {
            match self.kind.get(node) {
                Some(NodeKind::Leaf { .. }) => return node,
                Some(NodeKind::Split { .. }) => {
                    if let Some(child) = node.first_child(&self.tree.map) {
                        node = child;
                    } else {
                        return node;
                    }
                }
                None => return node,
            }
        }
    }

    /// Calculates the depth of a node in the BSP tree by counting parent levels.
    /// The root node has depth 0, its direct children have depth 1, and so on.
    fn node_depth(&self, node: NodeId) -> usize {
        let mut depth = 0;
        let mut current = node;
        while let Some(parent) = current.parent(&self.tree.map) {
            depth += 1;
            current = parent;
        }
        depth
    }

    /// Returns the orientation for a new split based on the node's depth.
    /// Creates a fibonacci spiral pattern by alternating orientations:
    /// - Even depth (0, 2, 4, ...) → Horizontal split
    /// - Odd depth (1, 3, 5, ...) → Vertical split
    fn orientation_for_depth(&self, depth: usize) -> Orientation {
        if depth % 2 == 0 {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }
    }

    fn collect_windows_under(&self, node: NodeId, out: &mut Vec<WindowId>) {
        match self.kind.get(node) {
            Some(NodeKind::Leaf { window, .. }) => {
                if let Some(stack) = self.stacks.get(&node) {
                    out.extend(stack.iter().copied());
                } else if let Some(w) = window {
                    out.push(*w);
                }
            }
            Some(NodeKind::Split { .. }) => {
                for child in node.children(&self.tree.map) {
                    self.collect_windows_under(child, out);
                }
            }
            None => {}
        }
    }

    fn collect_visible_windows_under(&self, node: NodeId, out: &mut Vec<WindowId>) {
        match self.kind.get(node) {
            Some(NodeKind::Leaf { window: Some(window), .. }) => out.push(*window),
            Some(NodeKind::Leaf { .. }) => {}
            Some(NodeKind::Split { .. }) => {
                for child in node.children(&self.tree.map) {
                    self.collect_visible_windows_under(child, out);
                }
            }
            None => {}
        }
    }

    fn has_fullscreen_in_subtree(&self, node: NodeId) -> bool {
        match self.kind.get(node) {
            Some(NodeKind::Leaf {
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) => *fullscreen || *fullscreen_within_gaps,
            Some(NodeKind::Split { .. }) => {
                node.children(&self.tree.map).any(|child| self.has_fullscreen_in_subtree(child))
            }
            None => false,
        }
    }

    // Return the number of tiles sharing the parent's axis; perpendicular splits count as one.
    fn rebalance_node(&mut self, node: NodeId, parent_axis: Orientation) -> usize {
        let Some(&NodeKind::Split { orientation, .. }) = self.kind.get(node) else {
            return 1;
        };
        let (Some(first), Some(second)) =
            (node.first_child(&self.tree.map), node.last_child(&self.tree.map))
        else {
            return 1;
        };
        let first_span = self.rebalance_node(first, orientation);
        let second_span = self.rebalance_node(second, orientation);
        let span = first_span + second_span;
        if let Some(NodeKind::Split { ratio, .. }) = self.kind.get_mut(node) {
            *ratio = (first_span as f32 / span as f32).clamp(0.05, 0.95);
        }
        if orientation == parent_axis { span } else { 1 }
    }

    fn cleanup_after_removal(&mut self, node: NodeId) -> NodeId {
        let Some(parent_id) = node.parent(&self.tree.map) else {
            return node;
        };

        if !matches!(self.kind.get(parent_id), Some(NodeKind::Split { .. })) {
            return parent_id;
        }

        let children: Vec<_> = parent_id.children(&self.tree.map).collect();
        if children.len() != 2 {
            return parent_id;
        }
        node.detach(&mut self.tree).remove();
        self.kind.remove(node);
        self.stacks.remove(&node);
        self.collapse_split(parent_id);
        parent_id
    }

    /// Promote the sole remaining child while keeping the layout's root stable.
    fn collapse_split(&mut self, parent_id: NodeId) {
        if !matches!(self.kind.get(parent_id), Some(NodeKind::Split { .. })) {
            return;
        }
        let children: Vec<_> = parent_id.children(&self.tree.map).collect();
        let [sibling] = children.as_slice() else { return };
        let sibling = *sibling;
        let sibling_kind = match self.kind.get(sibling) {
            Some(k) => k.clone(),
            None => return,
        };

        match &sibling_kind {
            NodeKind::Split { .. } => {
                let sib_children: Vec<_> = sibling.children(&self.tree.map).collect();
                for c in sib_children {
                    c.detach(&mut self.tree).push_back(parent_id);
                }
            }
            NodeKind::Leaf { .. } => {
                if let Some(stack) = self.stacks.remove(&sibling) {
                    self.stacks.insert(parent_id, stack);
                }
            }
        }
        self.kind.insert(parent_id, sibling_kind);

        sibling.detach(&mut self.tree).remove();
        self.kind.remove(sibling);
        self.stacks.remove(&sibling);
    }

    fn selection_of_layout(&self, layout: crate::layout_engine::LayoutId) -> Option<NodeId> {
        self.layouts
            .get(layout)
            .map(|s| self.tree.data.selection.current_selection(s.root))
    }

    fn insert_window_at_selection(
        &mut self,
        layout: crate::layout_engine::LayoutId,
        wid: WindowId,
    ) {
        let Some(state) = self.layouts.get(layout).copied() else {
            return;
        };
        let sel = self.tree.data.selection.current_selection(state.root);
        match self.kind.get_mut(sel) {
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
                } else {
                    let existing = *window;
                    let left = self.make_leaf(existing);
                    let right = self.make_leaf(Some(wid));
                    if let Some(stack) = self.stacks.remove(&sel) {
                        self.stacks.insert(left, stack);
                    }

                    // Use alternating orientations based on depth for fibonacci spiral
                    let depth = self.node_depth(sel);
                    let orientation = self.orientation_for_depth(depth);
                    self.kind.insert(sel, NodeKind::Split { orientation, ratio: 0.5 });
                    left.detach(&mut self.tree).push_back(sel);
                    right.detach(&mut self.tree).push_back(sel);
                    self.tree.data.selection.select(&self.tree.map, right);
                }
            }
            Some(NodeKind::Split { .. }) => {
                let leaf = self.descend_to_leaf(sel);
                self.tree.data.selection.select(&self.tree.map, leaf);
                self.insert_window_at_selection(layout, wid);
            }
            None => {}
        }
    }

    /// Remove an occurrence and report progress so callers can clear legacy duplicates.
    fn remove_window_internal(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(node) = self.node_for_window_in_layout(layout, wid) else {
            return false;
        };
        if let Some(stack) = self.stacks.get_mut(&node) {
            stack.retain(|window| *window != wid);
            if let Some(&visible) = stack.last() {
                if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node) {
                    *window = Some(visible);
                }
                self.tree.data.selection.select(&self.tree.map, node);
                return true;
            }
            self.stacks.remove(&node);
        }
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node) {
            *window = None;
        }
        let fallback = self.cleanup_after_removal(node);
        let selection = self.tree.data.selection.current_selection(self.layouts[layout].root);
        let new_selection = if self.kind.contains_key(selection) {
            self.descend_to_leaf(selection)
        } else {
            self.descend_to_leaf(fallback)
        };
        self.tree.data.selection.select(&self.tree.map, new_selection);
        true
    }

    fn calculate_layout_recursive(
        &self,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        match self.kind.get(node) {
            Some(NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) => {
                if let Some(w) = window {
                    let mut target = if *fullscreen {
                        screen
                    } else if *fullscreen_within_gaps {
                        compute_tiling_area(screen, gaps)
                    } else {
                        rect
                    };
                    if !*fullscreen
                        && !*fullscreen_within_gaps
                        && let Some(c) = constraints.get(w).copied()
                    {
                        let c = c.normalized();
                        let desired_w = c
                            .fixed_for_axis(true)
                            .unwrap_or(target.size.width)
                            .max(c.min_for_axis(true));
                        let desired_h = c
                            .fixed_for_axis(false)
                            .unwrap_or(target.size.height)
                            .max(c.min_for_axis(false));
                        let desired_w = if c.max_for_axis(true) > 0.0 {
                            desired_w.min(c.max_for_axis(true))
                        } else {
                            desired_w
                        };
                        let desired_h = if c.max_for_axis(false) > 0.0 {
                            desired_h.min(c.max_for_axis(false))
                        } else {
                            desired_h
                        };
                        target.size.width = desired_w.min(target.size.width).max(0.0);
                        target.size.height = desired_h.min(target.size.height).max(0.0);
                    }
                    out.push((*w, target));
                }
            }
            Some(NodeKind::Split { orientation, ratio }) => match orientation {
                Orientation::Horizontal => {
                    let gap = gaps.inner.horizontal as f64;
                    let total = rect.size.width;
                    let available = (total - gap).max(0.0);
                    let mut it = node.children(&self.tree.map);
                    let first = it.next();
                    let second = it.next();
                    let (first_w, second_w) =
                        if let (Some(first_node), Some(second_node)) = (first, second) {
                            let (first_min, first_fixed, first_max, first_can_grow) =
                                self.subtree_axis_constraints(first_node, true, constraints, gaps);
                            let (second_min, second_fixed, second_max, second_can_grow) =
                                self.subtree_axis_constraints(second_node, true, constraints, gaps);
                            let solved = solve_axis_lengths(
                                &[
                                    AxisConstraints {
                                        min: first_min,
                                        fixed: first_fixed,
                                        max: first_max,
                                        weight: (*ratio as f64).max(0.0),
                                        can_grow: first_can_grow,
                                    },
                                    AxisConstraints {
                                        min: second_min,
                                        fixed: second_fixed,
                                        max: second_max,
                                        weight: (1.0 - *ratio as f64).max(0.0),
                                        can_grow: second_can_grow,
                                    },
                                ],
                                available,
                            );
                            (
                                solved.first().copied().unwrap_or(available * (*ratio as f64)),
                                solved.get(1).copied().unwrap_or(0.0),
                            )
                        } else {
                            let first_w_f = available * (*ratio as f64);
                            let first_w = first_w_f.max(0.0);
                            let second_w = (available - first_w).max(0.0);
                            (first_w, second_w)
                        };
                    let r1 = CGRect::new(rect.origin, CGSize::new(first_w, rect.size.height));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x + first_w + gap, rect.origin.y),
                        CGSize::new(second_w, rect.size.height),
                    );
                    let mut it = node.children(&self.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, screen, constraints, gaps, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, screen, constraints, gaps, out);
                    }
                }
                Orientation::Vertical => {
                    let gap = gaps.inner.vertical as f64;
                    let total = rect.size.height;
                    let available = (total - gap).max(0.0);
                    let mut it = node.children(&self.tree.map);
                    let first = it.next();
                    let second = it.next();
                    let (first_h, second_h) =
                        if let (Some(first_node), Some(second_node)) = (first, second) {
                            let (first_min, first_fixed, first_max, first_can_grow) =
                                self.subtree_axis_constraints(first_node, false, constraints, gaps);
                            let (second_min, second_fixed, second_max, second_can_grow) = self
                                .subtree_axis_constraints(second_node, false, constraints, gaps);
                            let solved = solve_axis_lengths(
                                &[
                                    AxisConstraints {
                                        min: first_min,
                                        fixed: first_fixed,
                                        max: first_max,
                                        weight: (*ratio as f64).max(0.0),
                                        can_grow: first_can_grow,
                                    },
                                    AxisConstraints {
                                        min: second_min,
                                        fixed: second_fixed,
                                        max: second_max,
                                        weight: (1.0 - *ratio as f64).max(0.0),
                                        can_grow: second_can_grow,
                                    },
                                ],
                                available,
                            );
                            (
                                solved.first().copied().unwrap_or(available * (*ratio as f64)),
                                solved.get(1).copied().unwrap_or(0.0),
                            )
                        } else {
                            let first_h_f = available * (*ratio as f64);
                            let first_h = first_h_f.max(0.0);
                            let second_h = (available - first_h).max(0.0);
                            (first_h, second_h)
                        };
                    let r1 = CGRect::new(rect.origin, CGSize::new(rect.size.width, first_h));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x, rect.origin.y + first_h + gap),
                        CGSize::new(rect.size.width, second_h),
                    );
                    let mut it = node.children(&self.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, screen, constraints, gaps, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, screen, constraints, gaps, out);
                    }
                }
            },
            None => {}
        }
    }

    fn subtree_axis_constraints(
        &self,
        node: NodeId,
        horizontal: bool,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
    ) -> (f64, Option<f64>, Option<f64>, bool) {
        match self.kind.get(node) {
            Some(NodeKind::Leaf { window, .. }) => {
                if let Some(wid) = window {
                    if let Some(c) = constraints.get(wid).copied() {
                        let c = c.normalized();
                        return (
                            c.min_for_axis(horizontal),
                            c.fixed_for_axis(horizontal),
                            (c.max_for_axis(horizontal) > 0.0).then(|| c.max_for_axis(horizontal)),
                            c.resizable_for_axis(horizontal),
                        );
                    }
                }
                (0.0, None, None, true)
            }
            Some(NodeKind::Split { orientation, .. }) => {
                let children: Vec<_> = node.children(&self.tree.map).collect();
                if children.is_empty() {
                    return (0.0, None, None, true);
                }
                let axis_aligned = *orientation
                    == if horizontal {
                        Orientation::Horizontal
                    } else {
                        Orientation::Vertical
                    };
                let inner_gap = if horizontal {
                    gaps.inner.horizontal
                } else {
                    gaps.inner.vertical
                };
                let mut mins = Vec::with_capacity(children.len());
                let mut fixed_parts = Vec::with_capacity(children.len());
                let mut max_parts = Vec::with_capacity(children.len());
                let mut any_grow = false;
                for child in children {
                    let (min, fixed, max, can_grow) =
                        self.subtree_axis_constraints(child, horizontal, constraints, gaps);
                    mins.push(min.max(0.0));
                    fixed_parts.push(fixed.map(|v| v.max(0.0)));
                    max_parts.push(max.map(|v| v.max(0.0)));
                    any_grow |= can_grow;
                }
                if axis_aligned {
                    let gap_total = inner_gap * (fixed_parts.len().saturating_sub(1) as f64);
                    let min_total = mins.iter().sum::<f64>() + gap_total;
                    let fixed_total = fixed_parts
                        .iter()
                        .copied()
                        .try_fold(0.0, |acc, part| part.map(|p| acc + p));
                    let max_total =
                        max_parts.iter().copied().try_fold(0.0, |acc, part| part.map(|p| acc + p));
                    (
                        min_total,
                        fixed_total.map(|v| v + gap_total),
                        max_total.map(|v| v + gap_total),
                        any_grow,
                    )
                } else {
                    let min_max = mins
                        .into_iter()
                        .fold(0.0, |acc, value| if value > acc { value } else { acc });
                    let fixed_max = fixed_parts.into_iter().try_fold(0.0, |acc, part| match part {
                        Some(value) => Some(if value > acc { value } else { acc }),
                        None => None,
                    });
                    (min_max, fixed_max, None, any_grow)
                }
            }
            None => (0.0, None, None, true),
        }
    }

    fn selection_window(&self, state: &LayoutState) -> Option<WindowId> {
        let sel = self.tree.data.selection.current_selection(state.root);
        match self.kind.get(sel) {
            Some(NodeKind::Leaf { window, .. }) => *window,
            _ => None,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Components {
    selection: Selection,
}

impl crate::model::tree::Observer for Components {
    fn added_to_forest(&mut self, map: &NodeMap, node: NodeId) {
        self.selection.handle_event(map, TreeEvent::AddedToForest(node))
    }

    fn added_to_parent(&mut self, map: &NodeMap, node: NodeId) {
        self.selection.handle_event(map, TreeEvent::AddedToParent(node))
    }

    fn removing_from_parent(&mut self, map: &NodeMap, node: NodeId) {
        self.selection.handle_event(map, TreeEvent::RemovingFromParent(node))
    }

    fn removed_child(_tree: &mut Tree<Self>, _parent: NodeId) {}

    fn removed_from_forest(&mut self, map: &NodeMap, node: NodeId) {
        self.selection.handle_event(map, TreeEvent::RemovedFromForest(node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    fn tiled(windows: &[u32]) -> (BspLayoutSystem, LayoutId) {
        let mut system = BspLayoutSystem::default();
        let layout = system.create_layout();
        for &window in windows {
            system.add_window_after_selection(layout, w(window));
        }
        (system, layout)
    }

    fn calculate_frames(
        system: &BspLayoutSystem,
        layout: LayoutId,
        size: CGSize,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
    ) -> HashMap<WindowId, CGRect> {
        system
            .calculate_layout(
                layout,
                CGRect::new(CGPoint::new(0.0, 0.0), size),
                0.0,
                constraints,
                &Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect()
    }

    fn assert_frame(
        frames: &HashMap<WindowId, CGRect>,
        window: WindowId,
        rect: (f64, f64, f64, f64),
    ) {
        let (x, y, width, height) = rect;
        assert_eq!(
            frames[&window],
            CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
        );
    }

    #[test]
    fn app_exit_clears_saved_display_layouts_and_reclaims_frames() {
        use crate::layout_engine::workspaces::WorkspaceLayoutState;

        let mut system = BspLayoutSystem::default();
        let mut workspace = WorkspaceLayoutState::default();
        let sizes = [CGSize::new(1000.0, 800.0), CGSize::new(1600.0, 900.0)];
        workspace.ensure_active(sizes[0], &mut system);
        let original = workspace.active().unwrap();
        system.add_window_after_selection(original, w(1));
        let survivor = WindowId::new(2, 1);
        system.add_window_after_selection(original, survivor);
        workspace.ensure_active(sizes[1], &mut system);
        let cloned = workspace.active().unwrap();
        system.remove_windows_for_app(1);
        for (layout, size) in [(cloned, sizes[1]), (original, sizes[0])] {
            workspace.ensure_active(size, &mut system);
            assert_eq!(workspace.active(), Some(layout));
            assert_eq!(system.all_windows_in_layout(layout), vec![survivor]);
            assert_eq!(system.selected_window(layout), Some(survivor));
            let frames = calculate_frames(&system, layout, size, &Default::default());
            assert_frame(&frames, survivor, (0.0, 0.0, size.width, size.height));
        }
    }

    #[test]
    fn removal_clears_legacy_duplicates_and_empty_roots() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        // Simulate duplicate membership from an older saved tree.
        let duplicate = system.node_for_window_in_layout(layout, w(2)).unwrap();
        if let Some(NodeKind::Leaf { window, .. }) = system.kind.get_mut(duplicate) {
            *window = Some(w(1));
        }
        system.remove_window(w(1));
        assert_eq!(system.all_windows_in_layout(layout), vec![w(3)]);
        let frames =
            calculate_frames(&system, layout, CGSize::new(1000.0, 800.0), &Default::default());
        assert_frame(&frames, w(3), (0.0, 0.0, 1000.0, 800.0));
        let stack_layout = system.clone_layout(layout);
        let node = system.node_for_window_in_layout(stack_layout, w(3)).unwrap();
        system.stacks.insert(node, vec![w(3), w(3)]);
        system.remove_window(w(3));
        system.remove_window(w(3));
        assert!(system.all_windows_in_layout(layout).is_empty());
        assert_eq!(system.selected_window(layout), None);
        assert!(system.all_windows_in_layout(stack_layout).is_empty());
        assert!(system.visible_windows_in_layout(stack_layout).is_empty());
    }

    #[test]
    fn cloned_stacks_replace_and_remove_all_members() {
        use crate::layout_engine::WindowDropAction::Stack;

        let (mut system, original) = tiled(&[1, 2, 3]);
        assert!(system.apply_window_drop(original, w(2), w(1), Stack));
        let cloned = system.clone_layout(original);
        system.replace_window(w(1), w(4));
        system.replace_window(w(2), w(5));
        for layout in [original, cloned] {
            assert_eq!(system.stack_members(layout, w(4)), vec![w(4), w(5)]);
            assert_eq!(system.visible_windows_in_layout(layout), vec![w(5), w(3)]);
        }
        system.remove_window(w(5));
        for layout in [original, cloned] {
            assert_eq!(system.stack_members(layout, w(4)), vec![w(4)]);
            assert_eq!(system.visible_windows_in_layout(layout), vec![w(4), w(3)]);
        }
        system.replace_window(w(4), w(3));
        for layout in [original, cloned] {
            assert_eq!(system.all_windows_in_layout(layout), vec![w(3)]);
        }
        system.remove_windows_for_app(1);
        for layout in [original, cloned] {
            assert!(system.all_windows_in_layout(layout).is_empty());
            assert!(system.visible_windows_in_layout(layout).is_empty());
            assert_eq!(system.selected_window(layout), None);
        }
    }

    #[test]
    fn interactions_are_scoped_and_destroying_a_clone_preserves_original() {
        use crate::layout_engine::WindowDropAction::{Insert, Stack};

        let (mut system, original) = tiled(&[1, 2]);
        let cloned = system.clone_layout(original);
        assert!(system.swap_windows(original, w(1), w(2)));
        assert_eq!(system.all_windows_in_layout(original), vec![w(2), w(1)]);
        assert_eq!(system.all_windows_in_layout(cloned), vec![w(1), w(2)]);
        assert!(system.apply_window_drop(original, w(1), w(2), Stack));
        assert!(system.select_window(original, w(2)));
        assert_eq!(system.selected_window(original), Some(w(2)));
        assert_eq!(system.selected_window(cloned), Some(w(2)));
        assert!(system.select_window(cloned, w(1)));
        assert_eq!(system.selected_window(original), Some(w(2)));
        assert!(system.apply_window_drop(original, w(2), w(1), Insert(Direction::Left)));
        assert_eq!(system.all_windows_in_layout(original), vec![w(2), w(1)]);
        system.remove_layout(cloned);
        assert!(system.select_window(original, w(1)));
        system.remove_window(w(2));
        assert_eq!(system.all_windows_in_layout(original), vec![w(1)]);
    }

    #[test]
    fn insertion_preserves_stacks_and_rejects_existing_members() {
        use crate::layout_engine::WindowDropAction::Stack;

        for (insertion, explicit_split) in [
            (WindowInsertionPoint::default(), false),
            (WindowInsertionPoint::default(), true),
            (WindowInsertionPoint::EndOfTree, false),
        ] {
            let (mut system, layout) = tiled(&[1, 2]);
            system.set_window_insertion_point(insertion);
            assert!(system.apply_window_drop(layout, w(2), w(1), Stack));
            system.add_window_after_selection(layout, w(1));
            system.add_window_after_selection(layout, w(2));
            assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2)]);
            if explicit_split {
                system.split_selection(layout, LayoutKind::Horizontal);
            }
            system.add_window_after_selection(layout, w(3));
            assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2), w(3)]);
            assert_eq!(system.stack_members(layout, w(1)), vec![w(1), w(2)]);
            system.remove_windows_for_app(1);
            assert!(system.all_windows_in_layout(layout).is_empty());
        }
    }

    #[test]
    fn joining_preserves_membership_for_cloning_and_app_exit() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        system.join_selection_with_direction(layout, Direction::Up);
        assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2), w(3)]);
        assert_eq!(system.stack_members(layout, w(2)), vec![w(2), w(3)]);
        let cloned = system.clone_layout(layout);
        system.remove_windows_for_app(1);
        for layout in [layout, cloned] {
            assert!(system.all_windows_in_layout(layout).is_empty());
        }
    }

    #[test]
    fn unjoining_retiles_the_stack_in_place() {
        for (windows, direction, joined) in [
            // Stack directly under the root: [1 | stack(2, 3)].
            (&[1, 2, 3][..], Direction::Up, [2, 3]),
            // Deeper stack in the spiral: [1 | [2 / stack(3, 4)]].
            (&[1, 2, 3, 4][..], Direction::Left, [3, 4]),
        ] {
            let (mut system, layout) = tiled(windows);
            let tiled_tree = system.draw_tree(layout);
            let focused = w(joined[1]);

            system.join_selection_with_direction(layout, direction);

            assert_eq!(system.stack_members(layout, focused), joined.map(w));

            system.unjoin_selection(layout);

            assert_eq!(system.draw_tree(layout), tiled_tree);
            assert!(system.stack_members(layout, focused).is_empty());
            assert_eq!(system.selected_window(layout), Some(focused));
        }

        // Members keep their stack order and the active member keeps the selection.
        let (mut system, layout) = tiled(&[1, 2, 3]);
        system.join_selection_with_direction(layout, Direction::Up);
        assert!(system.select_window(layout, w(2)));

        system.unjoin_selection(layout);

        assert_eq!(system.visible_windows_in_layout(layout), vec![w(1), w(3), w(2)]);
        assert_eq!(system.selected_window(layout), Some(w(2)));
    }

    #[test]
    fn unjoining_a_window_that_is_not_stacked_is_a_noop() {
        for windows in [&[1][..], &[1, 2], &[1, 2, 3], &[1, 2, 3, 4]] {
            let (mut system, layout) = tiled(windows);
            let tree = system.draw_tree(layout);
            let selected = system.selected_window(layout);

            system.unjoin_selection(layout);

            assert_eq!(system.draw_tree(layout), tree);
            assert_eq!(system.selected_window(layout), selected);
        }
    }

    #[test]
    fn unjoining_keeps_fullscreen_on_the_visible_member() {
        let toggles: [fn(&mut BspLayoutSystem, LayoutId) -> Vec<WindowId>; 2] = [
            BspLayoutSystem::toggle_fullscreen_of_selection,
            BspLayoutSystem::toggle_fullscreen_within_gaps_of_selection,
        ];

        for toggle in toggles {
            let (mut system, layout) = tiled(&[1, 2, 3]);
            system.join_selection_with_direction(layout, Direction::Up);
            toggle(&mut system, layout);

            system.unjoin_selection(layout);

            assert_eq!(system.selected_window(layout), Some(w(3)));
            assert!(system.has_any_fullscreen_node(layout));

            let frames =
                calculate_frames(&system, layout, CGSize::new(1000.0, 800.0), &Default::default());

            assert_frame(&frames, w(2), (500.0, 0.0, 500.0, 400.0));
            assert_frame(&frames, w(3), (0.0, 0.0, 1000.0, 800.0));

            toggle(&mut system, layout);

            assert!(!system.has_any_fullscreen_node(layout));
        }
    }

    #[test]
    fn consume_or_expel_toggles_between_joined_and_unjoined() {
        for (windows, direction, joined) in [
            (&[1, 2, 3][..], Direction::Up, [2, 3]),
            (&[1, 2, 3, 4][..], Direction::Left, [3, 4]),
        ] {
            let (mut system, layout) = tiled(windows);
            let tiled_tree = system.draw_tree(layout);

            system.consume_or_expel_selection(layout, direction);

            assert_eq!(system.stack_members(layout, w(joined[0])), joined.map(w));

            system.consume_or_expel_selection(layout, direction);

            assert_eq!(system.draw_tree(layout), tiled_tree);
        }

        // A joined first child keeps its side: [[2 / 5] / [3 | 4]].
        let (mut system, layout) = tiled(&[1, 2, 3, 4]);
        system.remove_window(w(1));
        assert!(system.select_window(layout, w(2)));
        system.add_window_after_selection(layout, w(5));
        assert!(system.select_window(layout, w(5)));
        let tiled_tree = system.draw_tree(layout);

        system.consume_or_expel_selection(layout, Direction::Up);

        assert_eq!(system.stack_members(layout, w(5)), vec![w(2), w(5)]);
        assert_eq!(system.visible_windows_in_layout(layout), [5, 3, 4].map(w));

        system.consume_or_expel_selection(layout, Direction::Up);

        assert_eq!(system.draw_tree(layout), tiled_tree);
        assert_eq!(system.visible_windows_in_layout(layout), [2, 5, 3, 4].map(w));
    }

    #[test]
    fn consume_or_expel_joins_a_window_left_alone_in_its_stack() {
        // [1 | [2 / stack(3)]] once the other stacked window has closed.
        let (mut system, layout) = tiled(&[1, 2, 3, 4]);
        system.join_selection_with_direction(layout, Direction::Left);
        system.remove_window(w(4));

        assert_eq!(system.stack_members(layout, w(3)), vec![w(3)]);

        system.consume_or_expel_selection(layout, Direction::Up);

        assert_eq!(system.stack_members(layout, w(3)), vec![w(2), w(3)]);
    }

    #[test]
    fn unjoining_a_window_left_alone_in_its_stack_clears_the_stack() {
        // [1 | [2 / stack(3)]] once the other stacked window has closed.
        let (mut system, layout) = tiled(&[1, 2, 3, 4]);
        system.join_selection_with_direction(layout, Direction::Left);
        system.remove_window(w(4));
        let tree = system.draw_tree(layout);

        assert_eq!(system.stack_members(layout, w(3)), vec![w(3)]);

        system.unjoin_selection(layout);

        assert_eq!(system.draw_tree(layout), tree);
        assert!(system.stack_members(layout, w(3)).is_empty());
        assert_eq!(system.selected_window(layout), Some(w(3)));
    }

    #[test]
    fn joining_keeps_the_selected_window_visible() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.select_window(layout, w(2)));

        system.join_selection_with_direction(layout, Direction::Down);

        assert_eq!(system.stack_members(layout, w(2)), vec![w(3), w(2)]);
        assert_eq!(system.visible_windows_in_layout(layout), vec![w(1), w(2)]);
        assert_eq!(system.selected_window(layout), Some(w(2)));

        system.unjoin_selection(layout);

        let (reference, reference_layout) = tiled(&[1, 3, 2]);

        assert_eq!(system.draw_tree(layout), reference.draw_tree(reference_layout));
        assert_eq!(system.selected_window(layout), Some(w(2)));
    }

    #[test]
    fn legacy_ron_index_is_ignored_when_loading_saved_layouts() {
        let (system, original) = tiled(&[1, 2]);
        let node = system.node_for_window_in_layout(original, w(1)).unwrap();
        let obsolete_index: HashMap<_, _> = [(w(1), node)].into_iter().collect();
        let saved = ron::to_string(&system).unwrap();
        let legacy = saved.replacen(
            "stacks:",
            &format!(
                "window_to_node:{},stacks:",
                ron::to_string(&obsolete_index).unwrap()
            ),
            1,
        );
        let mut restored: BspLayoutSystem = ron::from_str(&legacy).unwrap();
        assert_eq!(restored.all_windows_in_layout(original), vec![w(1), w(2)]);
        restored.remove_window(w(1));
        assert_eq!(restored.all_windows_in_layout(original), vec![w(2)]);
    }

    #[test]
    fn insertion_alternates_splits_and_navigation_follows_orientation() {
        let (mut system, layout) = tiled(&[1, 2]);
        assert_eq!(system.window_in_direction(layout, Direction::Right), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Left), Some(w(2)));
        system.toggle_tile_orientation(layout);
        assert_eq!(system.window_in_direction(layout, Direction::Down), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Up), Some(w(2)));

        let (mut spiral, layout) = tiled(&[1]);
        for (window, horizontal, vertical) in [(2, 1, 0), (3, 1, 1), (4, 2, 1), (5, 2, 2)] {
            spiral.add_window_after_selection(layout, w(window));
            let tree = spiral.draw_tree(layout);
            assert_eq!(tree.matches("Horizontal").count(), horizontal);
            assert_eq!(tree.matches("Vertical").count(), vertical);
        }
    }

    #[test]
    fn rebalance_splits_each_axis_evenly() {
        let size = CGSize::new(1200.0, 900.0);

        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.select_window(layout, w(1)));
        system.resize_selection_by(layout, -0.4, ResizeOrientation::Smart);

        assert!(system.select_window(layout, w(2)));
        system.resize_selection_by(layout, -0.3, ResizeOrientation::Vertical);

        system.rebalance(layout);

        let frames = calculate_frames(&system, layout, size, &Default::default());
        assert_frame(&frames, w(1), (0.0, 0.0, 600.0, 900.0));
        assert_frame(&frames, w(2), (600.0, 0.0, 600.0, 450.0));
        assert_frame(&frames, w(3), (600.0, 450.0, 600.0, 450.0));
    }

    #[test]
    fn rebalance_shares_an_axis_across_nested_splits() {
        let size = CGSize::new(1200.0, 900.0);

        let (mut system, layout) = tiled(&[1]);
        system.split_selection(layout, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(2));
        system.split_selection(layout, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(3));

        let frames = calculate_frames(&system, layout, size, &Default::default());
        assert_frame(&frames, w(1), (0.0, 0.0, 600.0, 900.0));

        system.rebalance(layout);

        let frames = calculate_frames(&system, layout, size, &Default::default());
        for window in [1, 2, 3] {
            assert!((frames[&w(window)].size.width - 400.0).abs() < 1.0);
        }
    }

    #[test]
    fn rebalance_counts_a_stacked_leaf_as_one_tile() {
        let size = CGSize::new(1200.0, 900.0);

        let (mut system, layout) = tiled(&[1]);
        system.split_selection(layout, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(2));
        system.split_selection(layout, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(3));
        system.join_selection_with_direction(layout, Direction::Left);

        assert_eq!(system.stack_members(layout, w(2)), vec![w(2), w(3)]);

        assert!(system.select_window(layout, w(1)));
        system.resize_selection_by(layout, -0.4, ResizeOrientation::Smart);

        system.rebalance(layout);

        let frames = calculate_frames(&system, layout, size, &Default::default());
        assert_frame(&frames, w(1), (0.0, 0.0, 600.0, 900.0));
    }

    #[test]
    fn focus_into_perpendicular_splits_tracks_the_source_center() {
        for vertical in [false, true] {
            for (left_resize, right_resize, targets) in [
                (0.0, 0.0, [2, 4, 1, 3]),
                (-0.5, 0.5, [2, 2, 3, 3]),
                (0.5, -0.5, [4, 4, 1, 1]),
            ] {
                let (mut system, layout) = tiled(&[1, 2]);
                if vertical {
                    system.toggle_tile_orientation(layout);
                }
                for (window, new, resize) in [(1, 3, left_resize), (2, 4, right_resize)] {
                    assert!(system.select_window(layout, w(window)));
                    system.add_window_after_selection(layout, w(new));
                    if vertical {
                        system.toggle_tile_orientation(layout);
                    }
                    assert!(system.select_window(layout, w(window)));
                    system.resize_selection_by(layout, resize, ResizeOrientation::Smart);
                }
                let (forward, backward) = if vertical {
                    (Direction::Down, Direction::Up)
                } else {
                    (Direction::Right, Direction::Left)
                };
                for ((from, direction), to) in
                    [(1, forward), (3, forward), (2, backward), (4, backward)]
                        .into_iter()
                        .zip(targets)
                {
                    assert!(system.select_window(layout, w(from)));
                    assert_eq!(system.move_focus(layout, direction), (Some(w(to)), vec![w(to)]));
                }
            }
        }
    }

    #[test]
    fn focus_raises_only_its_target_and_layout_membership_is_independent() {
        let (mut system, first) = tiled(&[1, 2, 3]);
        assert_eq!(
            system.move_focus(first, Direction::Left),
            (Some(w(1)), vec![w(1)])
        );
        let second = system.create_layout();
        system.add_window_after_selection(second, w(1));
        system.set_windows_for_app(first, w(1).pid, vec![w(1)]);
        assert_eq!(system.all_windows_in_layout(first), vec![w(1)]);
        assert_eq!(system.all_windows_in_layout(second), vec![w(1)]);
    }

    #[test]
    fn directional_moves_retile_in_all_axes_and_stop_at_the_edge() {
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            let vertical = matches!(direction, Direction::Up | Direction::Down);
            let backwards = matches!(direction, Direction::Left | Direction::Up);
            let (mut system, layout) = tiled(&[1, 2]);
            if vertical {
                system.toggle_tile_orientation(layout);
            }
            system.add_window_after_selection(layout, w(3));
            if vertical {
                system.toggle_tile_orientation(layout);
            }

            let moving = if backwards { w(3) } else { w(1) };
            assert!(system.select_window(layout, moving));
            assert!(system.move_selection(layout, direction));
            assert_eq!(system.selected_window(layout), Some(moving));

            let expected = if backwards {
                [
                    (250.0, 0.0, 250.0, 1000.0),
                    (500.0, 0.0, 500.0, 1000.0),
                    (0.0, 0.0, 250.0, 1000.0),
                ]
            } else {
                [
                    (500.0, 0.0, 500.0, 500.0),
                    (0.0, 0.0, 500.0, 500.0),
                    (0.0, 500.0, 1000.0, 500.0),
                ]
            };
            let frames =
                calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default());
            for (index, (x, y, width, height)) in expected.into_iter().enumerate() {
                assert_frame(
                    &frames,
                    w(index as u32 + 1),
                    if vertical {
                        (y, x, height, width)
                    } else {
                        (x, y, width, height)
                    },
                );
            }
            assert!(!system.move_selection(layout, direction));
            assert_eq!(system.selected_window(layout), Some(moving));
        }
    }

    #[test]
    fn directional_move_fills_an_empty_slot_and_collapses_the_old_split() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.select_window(layout, w(1)));
        system.split_selection(layout, LayoutKind::Horizontal);
        assert!(!system.move_selection(layout, Direction::Right));
        assert!(system.select_window(layout, w(3)));
        assert!(system.move_selection(layout, Direction::Left));
        assert_eq!(system.selected_window(layout), Some(w(3)));

        let frames =
            calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default());
        assert_eq!(frames.len(), 3);
        for (window, x, width) in [
            (w(1), 0.0, 250.0),
            (w(3), 250.0, 250.0),
            (w(2), 500.0, 500.0),
        ] {
            assert_frame(&frames, window, (x, 0.0, width, 1000.0));
        }
    }

    #[test]
    fn moving_a_stack_keeps_its_membership_and_fullscreen_state() {
        use crate::layout_engine::WindowDropAction::Stack;

        for (windows, source) in [(&[1, 2, 3][..], w(3)), (&[1, 2][..], w(2))] {
            let (mut system, layout) = tiled(windows);
            for window in [4, 5] {
                system.add_window_after_selection(layout, w(window));
            }
            assert!(system.apply_window_drop(layout, w(4), source, Stack));
            assert!(system.apply_window_drop(layout, w(5), w(1), Stack));
            assert!(system.select_window(layout, w(4)));
            system.toggle_fullscreen_of_selection(layout);
            assert!(system.move_selection(layout, Direction::Left));

            assert_eq!(system.stack_members(layout, w(4)), vec![source, w(4)]);
            assert_eq!(system.stack_members(layout, w(5)), vec![w(1), w(5)]);
            assert!(system.has_any_fullscreen_node(layout));
            system.toggle_fullscreen_of_selection(layout);
            assert!(!system.has_any_fullscreen_node(layout));
            for window in system.all_windows_in_layout(layout) {
                system.remove_window(window);
            }
            assert!(system.all_windows_in_layout(layout).is_empty());
        }
    }

    #[test]
    fn split_constraints_stay_local_and_nonbinding_minima_keep_equal_halves() {
        let (system, layout) = tiled(&[101, 102]);
        let mut constraints = HashMap::default();
        constraints.insert(
            w(101),
            WindowLayoutConstraints {
                is_resizable: true,
                max_width: 600.0,
                ..Default::default()
            }
            .normalized(),
        );
        let frames = calculate_frames(&system, layout, CGSize::new(1600.0, 900.0), &constraints);
        assert!((frames[&w(101)].size.width - 600.0).abs() < 1.0);
        assert!((frames[&w(102)].size.width - 1000.0).abs() < 1.0);
        assert!((frames[&w(102)].origin.x - 600.0).abs() < 1.0);

        let (system, layout) = tiled(&[106, 107]);
        constraints.clear();
        constraints.insert(
            w(107),
            WindowLayoutConstraints {
                is_resizable: true,
                min_width: 400.0,
                ..Default::default()
            }
            .normalized(),
        );
        let frames = calculate_frames(&system, layout, CGSize::new(1200.0, 900.0), &constraints);
        assert!((frames[&w(106)].size.width - 600.0).abs() < 1.0);
        assert!((frames[&w(107)].size.width - 600.0).abs() < 1.0);
        assert!((frames[&w(107)].origin.x - 600.0).abs() < 1.0);

        let (mut system, layout) = tiled(&[103]);
        system.split_selection(layout, LayoutKind::Vertical);
        system.add_window_after_selection(layout, w(105));
        assert!(system.select_window(layout, w(103)));
        system.split_selection(layout, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(104));
        constraints.clear();
        constraints.insert(
            w(103),
            WindowLayoutConstraints {
                is_resizable: true,
                max_height: 200.0,
                ..Default::default()
            }
            .normalized(),
        );
        let frames = calculate_frames(&system, layout, CGSize::new(1200.0, 800.0), &constraints);
        assert!(frames[&w(103)].size.height <= 201.0);
        assert!(frames[&w(104)].size.height >= 399.0);
        assert!((frames[&w(105)].size.height - 400.0).abs() < 1.0);
    }
}

impl LayoutSystem for BspLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let leaf = self.make_leaf(None);
        let state = LayoutState { root: leaf };
        self.layouts.insert(state)
    }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.layouts.contains_key(layout) }

    /// shallow
    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let mut groups = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            for node in state.root.traverse_preorder(&self.tree.map) {
                if let Some(NodeKind::Leaf { window: Some(window), .. }) = self.kind.get(node) {
                    groups.push(self.stacks.get(&node).cloned().unwrap_or_else(|| vec![*window]));
                }
            }
        }
        let new_layout = self.create_layout();
        for group in groups {
            let mut members = group.into_iter();
            let Some(first) = members.next() else { continue };
            self.add_window_after_selection(new_layout, first);
            let mut target = first;
            for window in members {
                self.add_window_after_selection(new_layout, window);
                let _ = self.apply_window_drop(
                    new_layout,
                    window,
                    target,
                    crate::layout_engine::WindowDropAction::Stack,
                );
                target = window;
            }
        }
        new_layout
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.remove(layout) {
            let ids: Vec<_> = state.root.traverse_preorder(&self.tree.map).collect();
            for id in ids {
                self.kind.remove(id);
                self.stacks.remove(&id);
            }
            state.root.remove_root(&mut self.tree);
        }
    }

    fn draw_tree(&self, layout: LayoutId) -> String {
        fn write_node(this: &BspLayoutSystem, node: NodeId, out: &mut String, indent: usize) {
            for _ in 0..indent {
                out.push_str("  ");
            }
            match this.kind.get(node) {
                Some(NodeKind::Leaf { window, .. }) => {
                    out.push_str(&format!("Leaf {:?}\n", window));
                }
                Some(NodeKind::Split { orientation, ratio }) => {
                    out.push_str(&format!("Split {:?} {:.2}\n", orientation, ratio));
                    let mut it = node.children(&this.tree.map);
                    if let Some(first) = it.next() {
                        write_node(this, first, out, indent + 1);
                    }
                    if let Some(second) = it.next() {
                        write_node(this, second, out, indent + 1);
                    }
                }
                None => {}
            }
        }
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut s = String::new();
            write_node(self, state.root, &mut s, 0);
            s
        } else {
            "<empty bsp>".to_string()
        }
    }

    fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        fn snapshot(
            system: &BspLayoutSystem,
            node: NodeId,
            selected: NodeId,
        ) -> rift_protocol::ContainerTreeNode {
            let weight = node.parent(&system.tree.map).and_then(|parent| {
                let NodeKind::Split { ratio, .. } = system.kind.get(parent)? else {
                    return None;
                };
                let first = parent.first_child(&system.tree.map);
                Some(if first == Some(node) {
                    f64::from(*ratio)
                } else {
                    1.0 - f64::from(*ratio)
                })
            });

            match system.kind.get(node) {
                Some(NodeKind::Split { orientation, .. }) => rift_protocol::ContainerTreeNode {
                    node_id: node.data().as_ffi(),
                    node_type: rift_protocol::ContainerNodeType::Container,
                    frame: Default::default(),
                    layout_kind: Some(rift_protocol::LayoutKind::from(*orientation)),
                    weight,
                    window_id: None,
                    is_selected: node == selected,
                    is_fullscreen: false,
                    is_fullscreen_within_gaps: false,
                    role: None,
                    pending_split: None,
                    children: node
                        .children(&system.tree.map)
                        .map(|child| snapshot(system, child, selected))
                        .collect(),
                },
                Some(NodeKind::Leaf {
                    window,
                    fullscreen,
                    fullscreen_within_gaps,
                    preselected,
                }) => {
                    let members = system.stacks.get(&node);
                    let stacked = members.is_some();
                    rift_protocol::ContainerTreeNode {
                        node_id: node.data().as_ffi(),
                        node_type: if stacked {
                            rift_protocol::ContainerNodeType::Container
                        } else if window.is_some() {
                            rift_protocol::ContainerNodeType::Window
                        } else {
                            rift_protocol::ContainerNodeType::Placeholder
                        },
                        frame: Default::default(),
                        layout_kind: stacked.then_some(rift_protocol::LayoutKind::HorizontalStack),
                        weight,
                        window_id: if stacked {
                            None
                        } else {
                            window.map(Into::into)
                        },
                        is_selected: node == selected,
                        is_fullscreen: *fullscreen,
                        is_fullscreen_within_gaps: *fullscreen_within_gaps,
                        role: None,
                        pending_split: preselected.map(Into::into),
                        children: members.map_or_else(Vec::new, |members| {
                            members
                                .iter()
                                .map(|member| rift_protocol::ContainerTreeNode {
                                    node_id: (u64::from(member.pid as u32) << 32)
                                        | u64::from(member.idx.get()),
                                    node_type: rift_protocol::ContainerNodeType::Window,
                                    frame: Default::default(),
                                    layout_kind: None,
                                    weight: None,
                                    window_id: Some((*member).into()),
                                    is_selected: node == selected && *window == Some(*member),
                                    is_fullscreen: *fullscreen,
                                    is_fullscreen_within_gaps: *fullscreen_within_gaps,
                                    role: None,
                                    pending_split: None,
                                    children: Vec::new(),
                                })
                                .collect()
                        }),
                    }
                }
                None => unreachable!("BSP layout contains a node without metadata"),
            }
        }

        let state = self.layouts.get(layout).expect("unknown BSP layout");
        let selected = self.tree.data.selection.current_selection(state.root);
        snapshot(self, state.root, selected)
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        _stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            let rect = compute_tiling_area(screen, gaps);
            self.calculate_layout_recursive(state.root, rect, screen, constraints, gaps, &mut out);
            if let (Some(ratio), [(_, frame)]) =
                (self.single_window_aspect_ratio, out.as_mut_slice())
                && matches!(
                    self.kind.get(state.root),
                    Some(NodeKind::Leaf {
                        fullscreen: false,
                        fullscreen_within_gaps: false,
                        ..
                    })
                )
                && !self.stacks.contains_key(&state.root)
            {
                let (width, height) = if frame.size.width > frame.size.height * ratio {
                    (frame.size.height * ratio, frame.size.height)
                } else {
                    (frame.size.width, frame.size.width / ratio)
                };
                frame.origin.x = rect.origin.x + (rect.size.width - width) / 2.0;
                frame.origin.y = rect.origin.y + (rect.size.height - height) / 2.0;
                frame.size = CGSize::new(width, height);
            }
        }
        out
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.layouts.get(layout).and_then(|s| self.selection_window(s))
    }

    fn all_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            self.collect_windows_under(state.root, &mut out);
        }
        out
    }

    fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        let root = self.layouts.get(layout)?.root;
        let node = self.node_for_window_in_layout(layout, window)?;
        let mut slot = crate::layout_engine::systems::node_slot(node, root, &self.tree.map)?;
        if let Some(members) = self.stacks.get(&node) {
            slot.push(members.iter().position(|&member| member == window)?);
        }
        Some(slot)
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            self.collect_visible_windows_under(state.root, &mut out);
        }
        out
    }

    fn stack_members(&self, layout: LayoutId, window: WindowId) -> Vec<WindowId> {
        if !self.contains_window(layout, window) {
            return Vec::new();
        }
        self.node_for_window_in_layout(layout, window)
            .and_then(|node| self.stacks.get(&node))
            .cloned()
            .unwrap_or_default()
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(sel) = self.selection_of_layout(layout) {
            if self.kind.get(sel).is_some() {
                let leaf = self.descend_to_leaf(sel);
                self.collect_visible_windows_under(leaf, &mut out);
            }
        }

        out
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool {
        if let Some(sel) = self.selection_of_layout(layout) {
            if self.kind.get(sel).is_none() {
                return false;
            }
            let parent_opt = sel.parent(&self.tree.map);
            if let Some(parent) = parent_opt {
                let new_sel = self.descend_to_leaf(parent);
                self.tree.data.selection.select(&self.tree.map, new_sel);
                return true;
            }
        }
        false
    }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        if let Some(sel) = self.selection_of_layout(layout) {
            let new_sel = self.descend_to_leaf(sel);
            if new_sel != sel {
                self.tree.data.selection.select(&self.tree.map, new_sel);
                return true;
            }
        }
        false
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        if self.visible_windows_in_layout(layout).is_empty() {
            return (None, vec![]);
        }
        let sel_snapshot = self.selection_of_layout(layout);
        let Some(current_sel) = sel_snapshot else {
            return (None, vec![]);
        };
        let current_leaf = self.descend_to_leaf(current_sel);
        let Some(next_leaf) = self.find_neighbor_leaf(current_leaf, direction) else {
            return (None, vec![]);
        };
        self.tree.data.selection.select(&self.tree.map, next_leaf);
        let focus = match self.kind.get(next_leaf) {
            Some(NodeKind::Leaf { window, .. }) => *window,
            _ => None,
        };
        // Tiled BSP windows never overlap, so only the focus target needs to be
        // raised. Raising every visible window makes the raise manager front
        // each app in turn (via make_key_window) before the target, which
        // flickers the menu bar and any focus border on every focus move. The
        // traditional engine already raises only the revealed group.
        (focus, focus.into_iter().collect())
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.layouts
            .get(layout)
            .and_then(|state| self.window_in_direction_from(state.root, direction))
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.contains_window(layout, wid) {
            return;
        }
        if self.layouts.get(layout).is_some() {
            if self.window_insertion_point == WindowInsertionPoint::EndOfTree {
                let root = self.layouts[layout].root;
                if let Some(leaf) = root
                    .traverse_preorder(&self.tree.map)
                    .filter(|node| matches!(self.kind.get(*node), Some(NodeKind::Leaf { .. })))
                    .last()
                {
                    self.tree.data.selection.select(&self.tree.map, leaf);
                }
                self.insert_window_at_selection(layout, wid);
                return;
            }
            // Try smart insertion first (with preselection support)
            if !self.smart_insert_window(layout, wid) {
                // Fall back to default insertion
                self.insert_window_at_selection(layout, wid);
            }
        }
    }

    fn replace_window(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        let layouts: Vec<_> = self.layouts.keys().collect();
        for layout in layouts {
            if !self.contains_window(layout, from) {
                continue;
            }
            // A replacement may already be present after restore matching.
            while self.remove_window_internal(layout, to) {}
            let root = self.layouts[layout].root;
            for node in root.traverse_preorder(&self.tree.map) {
                if let Some(stack) = self.stacks.get_mut(&node) {
                    for member in stack.iter_mut().filter(|member| **member == from) {
                        *member = to;
                    }
                }
                if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node)
                    && *window == Some(from)
                {
                    *window = Some(to);
                }
            }
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        let layouts: Vec<_> = self.layouts.keys().collect();
        for layout in layouts {
            while self.remove_window_internal(layout, wid) {}
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        let mut windows = Vec::new();
        for state in self.layouts.values() {
            self.collect_windows_under(state.root, &mut windows);
        }
        windows.retain(|window| window.pid == pid);
        windows.sort_unstable();
        windows.dedup();
        for window in windows {
            self.remove_window(window);
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let current = if let Some(state) = self.layouts.get(layout).copied() {
            let mut under: Vec<WindowId> = Vec::new();
            self.collect_windows_under(state.root, &mut under);
            under.into_iter().filter(|w| w.pid == pid).collect()
        } else {
            Vec::new()
        };
        let delta = reconcile_app_membership(pid, current, desired);
        for wid in delta.removals {
            if let Some(node) = self.node_for_window_in_layout(layout, wid)
                && let Some(NodeKind::Leaf {
                    fullscreen,
                    fullscreen_within_gaps,
                    ..
                }) = self.kind.get(node)
                && (*fullscreen || *fullscreen_within_gaps)
            {
                continue;
            }
            self.remove_window_internal(layout, wid);
        }
        for wid in delta.additions {
            self.add_window_after_selection(layout, wid);
        }
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.node_for_window_in_layout(layout, wid).is_some()
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(node) = self.node_for_window_in_layout(layout, wid) else {
            return false;
        };
        if let Some(stack) = self.stacks.get_mut(&node)
            && let Some(index) = stack.iter().position(|member| *member == wid)
        {
            stack.remove(index);
            stack.push(wid);
            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node) {
                *window = Some(wid);
            }
        }
        self.tree.data.selection.select(&self.tree.map, node);
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
        if let Some(node) = self.node_for_window_in_layout(layout, wid) {
            let tiling = compute_tiling_area(screen, gaps);
            let mut fullscreen_transition = false;
            if let Some(NodeKind::Leaf {
                window: _,
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) = self.kind.get_mut(node)
            {
                if new_frame == screen {
                    *fullscreen = true;
                    *fullscreen_within_gaps = false;
                    fullscreen_transition = true;
                } else if old_frame == screen {
                    *fullscreen = false;
                    fullscreen_transition = true;
                } else {
                    if new_frame == tiling {
                        *fullscreen_within_gaps = true;
                        *fullscreen = false;
                        fullscreen_transition = true;
                    } else if old_frame == tiling {
                        *fullscreen_within_gaps = false;
                        fullscreen_transition = true;
                    }
                }
            }

            if fullscreen_transition {
                return;
            }

            let width_changed = (new_frame.size.width - old_frame.size.width).abs() > 0.5;
            let height_changed = (new_frame.size.height - old_frame.size.height).abs() > 0.5;
            if !width_changed && !height_changed {
                return;
            }

            let mut current = node;
            while let Some(parent) = current.parent(&self.tree.map) {
                let Some(NodeKind::Split { orientation, ratio }) = self.kind.get_mut(parent) else {
                    current = parent;
                    continue;
                };

                let (old_len, new_len) = match orientation {
                    Orientation::Horizontal if width_changed => {
                        (old_frame.size.width, new_frame.size.width)
                    }
                    Orientation::Vertical if height_changed => {
                        (old_frame.size.height, new_frame.size.height)
                    }
                    _ => {
                        current = parent;
                        continue;
                    }
                };

                let current_ratio = f64::from(*ratio);
                let is_first = Some(current) == parent.first_child(&self.tree.map);
                let denom = if is_first {
                    current_ratio
                } else {
                    1.0 - current_ratio
                };
                if old_len <= 0.0 || denom <= f64::EPSILON {
                    break;
                }

                let parent_len = old_len / denom;
                if parent_len <= 0.0 {
                    break;
                }

                let raw_ratio = (new_len / parent_len).clamp(0.05, 0.95);
                *ratio = if is_first {
                    raw_ratio as f32
                } else {
                    (1.0 - raw_ratio) as f32
                };
                break;
            }
        }
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(sel) = self.selection_of_layout(layout) else {
            return false;
        };
        let moving = self.descend_to_leaf(sel);
        if !matches!(
            self.kind.get(moving),
            Some(NodeKind::Leaf { window: Some(_), .. })
        ) {
            return false;
        }
        let Some(target) = self.find_neighbor_leaf(moving, direction) else {
            return false;
        };
        let Some(old_parent) = moving.parent(&self.tree.map) else {
            return false;
        };

        // Siblings already share the requested split: reorder them without
        // resetting the split ratio or exchanging their window state.
        if target.parent(&self.tree.map) == Some(old_parent) {
            match direction {
                Direction::Left | Direction::Up => {
                    moving.detach(&mut self.tree).insert_before(target);
                }
                Direction::Right | Direction::Down => {
                    moving.detach(&mut self.tree).push_back(old_parent);
                }
            }
        } else if matches!(self.kind.get(target), Some(NodeKind::Leaf { window: None, .. })) {
            // A pre-created empty slot receives the leaf directly.
            moving.detach(&mut self.tree).insert_before(target);
            target.detach(&mut self.tree).remove();
            self.kind.remove(target);
            self.collapse_split(old_parent);
        } else {
            // Transfer the entire leaf (including stacks and fullscreen state)
            // onto a new split at the directional neighbor, then reclaim its
            // old region for the sibling left behind.
            let split = self.tree.mk_node().into_id();
            self.kind.insert(split, NodeKind::Split {
                orientation: direction.orientation(),
                ratio: 0.5,
            });
            split.detach(&mut self.tree).insert_before(target);
            let (first, second) = match direction {
                Direction::Left | Direction::Up => (moving, target),
                Direction::Right | Direction::Down => (target, moving),
            };
            first.detach(&mut self.tree).push_back(split);
            second.detach(&mut self.tree).push_back(split);
            self.collapse_split(old_parent);
        }
        self.tree.data.selection.select(&self.tree.map, moving);
        true
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        let Some(node_a) = self.node_for_window_in_layout(layout, a) else {
            return false;
        };
        let Some(node_b) = self.node_for_window_in_layout(layout, b) else {
            return false;
        };
        if node_a == node_b {
            let Some(stack) = self.stacks.get_mut(&node_a) else {
                return false;
            };
            if !stack.contains(&a) || !stack.contains(&b) {
                return false;
            }
            // A BSP stack has one active leaf window. Keep the dragged source active
            // while moving the target immediately behind it in stack order.
            stack.retain(|window| *window != a && *window != b);
            stack.push(b);
            stack.push(a);
            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node_a) {
                *window = Some(a);
            }
            self.tree.data.selection.select(&self.tree.map, node_a);
            return true;
        }

        if self.stacks.contains_key(&node_a) || self.stacks.contains_key(&node_b) {
            let group_a = self.stacks.remove(&node_a).unwrap_or_else(|| vec![a]);
            let group_b = self.stacks.remove(&node_b).unwrap_or_else(|| vec![b]);
            let visible_a = *group_a.last().unwrap();
            let visible_b = *group_b.last().unwrap();
            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node_a) {
                *window = Some(visible_b);
            }
            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node_b) {
                *window = Some(visible_a);
            }
            if group_b.len() > 1 {
                self.stacks.insert(node_a, group_b);
            }
            if group_a.len() > 1 {
                self.stacks.insert(node_b, group_a);
            }
            return true;
        }

        let mut a_window = None;
        let mut b_window = None;
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get(node_a) {
            a_window = *window;
        }
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get(node_b) {
            b_window = *window;
        }

        if a_window.is_none() && b_window.is_none() {
            return false;
        }

        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node_a) {
            *window = b_window;
        }
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node_b) {
            *window = a_window;
        }

        true
    }

    fn apply_target_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
    ) -> bool {
        if source == target
            || !self.contains_window(layout, source)
            || !self.contains_window(layout, target)
        {
            return false;
        }
        if action == crate::layout_engine::WindowDropAction::Swap {
            return self.swap_windows(layout, source, target);
        }

        while self.remove_window_internal(layout, source) {}
        let Some(target_node) = self.node_for_window_in_layout(layout, target) else {
            return false;
        };
        match action {
            crate::layout_engine::WindowDropAction::Stack => {
                let stack = self.stacks.entry(target_node).or_insert_with(|| vec![target]);
                stack.retain(|window| *window != source);
                stack.push(source);
                if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(target_node) {
                    *window = Some(source);
                }

                self.tree.data.selection.select(&self.tree.map, target_node);
                true
            }
            crate::layout_engine::WindowDropAction::Insert(direction) => {
                self.split_leaf_in_direction(target_node, direction, source);
                true
            }
            crate::layout_engine::WindowDropAction::Swap
            | crate::layout_engine::WindowDropAction::Move(_) => {
                unreachable!()
            }
        }
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        let sel = self.selected_window(from_layout);
        if let Some(w) = sel {
            while self.remove_window_internal(from_layout, w) {}
            self.add_window_after_selection(to_layout, w);
        }
    }

    fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind) {
        let orientation = match kind {
            LayoutKind::Horizontal => Orientation::Horizontal,
            LayoutKind::Vertical => Orientation::Vertical,
            _ => return,
        };
        let state = if let Some(s) = self.layouts.get(layout).copied() {
            s
        } else {
            return;
        };

        let sel = self.tree.data.selection.current_selection(state.root);
        let target = self.descend_to_leaf(sel);
        match self.kind.get(target).cloned() {
            Some(NodeKind::Leaf { window, .. }) => {
                let left = self.make_leaf(window);
                let right = self.make_leaf(None);
                if let Some(stack) = self.stacks.remove(&target) {
                    self.stacks.insert(left, stack);
                }
                self.kind.insert(target, NodeKind::Split { orientation, ratio: 0.5 });
                left.detach(&mut self.tree).push_back(target);
                right.detach(&mut self.tree).push_back(target);
                self.tree.data.selection.select(&self.tree.map, right);
            }
            _ => {}
        }
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        if let Some(sel) = self.selection_of_layout(layout) {
            let sel_leaf = self.descend_to_leaf(sel);
            if let Some(NodeKind::Leaf {
                window: Some(w),
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) = self.kind.get_mut(sel_leaf)
            {
                *fullscreen = !*fullscreen;
                if *fullscreen {
                    *fullscreen_within_gaps = false;
                }
                return vec![*w];
            }
        }
        vec![]
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        if let Some(sel) = self.selection_of_layout(layout) {
            let sel_leaf = self.descend_to_leaf(sel);
            if let Some(NodeKind::Leaf {
                window: Some(w),
                fullscreen_within_gaps,
                fullscreen,
                ..
            }) = self.kind.get_mut(sel_leaf)
            {
                *fullscreen_within_gaps = !*fullscreen_within_gaps;
                if *fullscreen_within_gaps {
                    *fullscreen = false;
                }
                return vec![*w];
            }
        }
        vec![]
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        if let Some(state) = self.layouts.get(layout).copied() {
            self.has_fullscreen_in_subtree(state.root)
        } else {
            false
        }
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        let Some(sel) = self.selection_of_layout(layout) else {
            return;
        };
        let sel_leaf = self.descend_to_leaf(sel);

        let Some(neighbor) = self.find_neighbor_leaf(sel_leaf, direction) else {
            return;
        };

        let mut current = sel_leaf;
        while let Some(parent) = current.parent(&self.tree.map) {
            let children: Vec<_> = parent.children(&self.tree.map).collect();
            if children.contains(&neighbor) {
                if parent.parent(&self.tree.map).is_some() {
                    let mut windows = Vec::new();
                    self.collect_windows_under(parent, &mut windows);

                    // The last member is the visible one, so keep the selected window there.
                    let selected = self.selected_window(layout);

                    if let Some(index) = windows.iter().position(|window| Some(*window) == selected)
                    {
                        windows[index..].rotate_left(1);
                    }

                    if let Some(visible) = windows.last().copied() {
                        // Take the collapsed split's place so a first child stays first.
                        let new_leaf = self.make_leaf(Some(visible));
                        new_leaf.detach(&mut self.tree).insert_after(parent);

                        let ids: Vec<_> = parent.traverse_preorder(&self.tree.map).collect();

                        for id in ids {
                            self.kind.remove(id);
                            self.stacks.remove(&id);
                        }

                        parent.detach(&mut self.tree).remove();

                        self.stacks.insert(new_leaf, windows);
                        self.tree.data.selection.select(&self.tree.map, new_leaf);
                    }
                }
                break;
            }
            current = parent;
        }
    }

    fn consume_or_expel_selection(&mut self, layout: LayoutId, direction: Direction) {
        let is_joined = self
            .selection_of_layout(layout)
            .map(|selection| self.descend_to_leaf(selection))
            .and_then(|leaf| self.stacks.get(&leaf))
            .is_some_and(|members| members.len() > 1);

        if is_joined {
            self.unjoin_selection(layout);
        } else {
            self.join_selection_with_direction(layout, direction);
        }
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        let Some(sel) = self.selection_of_layout(layout) else {
            return;
        };

        let leaf = self.descend_to_leaf(sel);
        let Some(NodeKind::Leaf {
            window,
            fullscreen,
            fullscreen_within_gaps,
            ..
        }) = self.kind.get_mut(leaf)
        else {
            return;
        };

        let Some(members) = self.stacks.remove(&leaf) else {
            return;
        };

        let flags = (*fullscreen, *fullscreen_within_gaps);

        // Retile this stack in place using depth-based splits.
        // The visible member is last in the stack, so it ends up selected.
        *window = None;
        self.tree.data.selection.select(&self.tree.map, leaf);

        for member in members {
            self.insert_window_at_selection(layout, member);
        }

        // Insertion rebuilds leaves without fullscreen, so hand it back to the visible member.
        if let Some(visible) = self.selection_of_layout(layout)
            && let Some(NodeKind::Leaf {
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) = self.kind.get_mut(visible)
        {
            (*fullscreen, *fullscreen_within_gaps) = flags;
        }
    }

    fn resize_selection_by(
        &mut self,
        layout: LayoutId,
        amount: f64,
        orientation: ResizeOrientation,
    ) {
        let sel_snapshot = self.selection_of_layout(layout);
        let Some(mut node) = sel_snapshot else {
            return;
        };

        while let Some(parent) = node.parent(&self.tree.map) {
            if let Some(NodeKind::Split {
                orientation: split_orientation,
                ratio,
            }) = self.kind.get_mut(parent)
                && match orientation {
                    ResizeOrientation::Horizontal => *split_orientation == Orientation::Horizontal,
                    ResizeOrientation::Vertical => *split_orientation == Orientation::Vertical,
                    ResizeOrientation::Smart => true,
                }
            {
                let is_first = Some(node) == parent.first_child(&self.tree.map);
                let delta = (amount as f32) * 0.5;
                if is_first {
                    *ratio = (*ratio + delta).clamp(0.05, 0.95);
                } else {
                    *ratio = (*ratio - delta).clamp(0.05, 0.95);
                }
                break;
            }
            node = parent;
        }
    }

    fn rebalance(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.get(layout).copied() {
            self.rebalance_node(state.root, Orientation::Horizontal);
        }
    }

    fn toggle_tile_orientation(&mut self, layout: LayoutId) {
        let sel_snapshot = self.selection_of_layout(layout);

        let start_node = if let Some(sel) = sel_snapshot {
            sel
        } else {
            let Some(state) = self.layouts.get(layout) else {
                return;
            };
            state.root
        };

        let mut node_opt = Some(start_node);
        while let Some(node) = node_opt {
            if let Some(NodeKind::Split { orientation, .. }) = self.kind.get_mut(node) {
                *orientation = match *orientation {
                    Orientation::Horizontal => Orientation::Vertical,
                    Orientation::Vertical => Orientation::Horizontal,
                };
                return;
            }
            node_opt = node.parent(&self.tree.map);
        }

        if let Some(state) = self.layouts.get_mut(layout) {
            let root = state.root;
            if let Some(NodeKind::Split { orientation, .. }) = self.kind.get_mut(root) {
                *orientation = match *orientation {
                    Orientation::Horizontal => Orientation::Vertical,
                    Orientation::Vertical => Orientation::Horizontal,
                };
            }
        }
    }
}
