use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::selection::*;
use crate::model::tree::{NodeId, NodeMap, Tree};
use crate::sys::event::current_cursor_location;

#[derive(Serialize, Deserialize, Clone)]
enum NodeKind {
    Split { orientation: Orientation, ratio: f32 },
    Leaf {
        window: Option<WindowId>,
        fullscreen: bool,
        fullscreen_within_gaps: bool,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct LayoutState {
    root: NodeId,
    preselection: Option<Direction>,
}

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
    layouts: slotmap::SlotMap<crate::layout_engine::LayoutId, LayoutState>,
    tree: Tree<Components>,
    kind: slotmap::SecondaryMap<NodeId, NodeKind>,
    window_to_node: HashMap<WindowId, NodeId>,
    settings: crate::common::config::DwindleSettings,
    #[serde(skip)]
    insertion_hints: HashMap<LayoutId, InsertionHint>,
    #[serde(skip)]
    last_frames: std::cell::RefCell<HashMap<LayoutId, LastFrame>>,
    #[serde(skip)]
    pseudo_sizes: HashMap<WindowId, CGSize>,
}

impl Default for DwindleLayoutSystem {
    fn default() -> Self {
        Self {
            layouts: Default::default(),
            tree: Tree::with_observer(Components::default()),
            kind: Default::default(),
            window_to_node: Default::default(),
            settings: crate::common::config::DwindleSettings::default(),
            insertion_hints: Default::default(),
            last_frames: Default::default(),
            pseudo_sizes: Default::default(),
        }
    }
}

impl DwindleLayoutSystem {
    fn clamp_ratio(ratio: f32) -> f32 { ratio.clamp(0.1, 1.9) }

    fn make_leaf(&mut self, window: Option<WindowId>) -> NodeId {
        let id = self.tree.mk_node().into_id();
        self.kind.insert(
            id,
            NodeKind::Leaf {
                window,
                fullscreen: false,
                fullscreen_within_gaps: false,
            },
        );
        if let Some(w) = window {
            self.window_to_node.insert(w, id);
        }
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

    fn collect_windows_under(&self, node: NodeId, out: &mut Vec<WindowId>) {
        match self.kind.get(node) {
            Some(NodeKind::Leaf { window, .. }) => {
                if let Some(w) = window {
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

    fn find_layout_root(&self, mut node: NodeId) -> NodeId {
        while let Some(p) = node.parent(&self.tree.map) {
            node = p;
        }
        node
    }

    fn belongs_to_layout(&self, layout: LayoutState, node: NodeId) -> bool {
        if self.kind.get(node).is_none() {
            return false;
        }
        self.find_layout_root(node) == layout.root
    }

    fn cleanup_after_removal(&mut self, node: NodeId) -> NodeId {
        let Some(parent_id) = node.parent(&self.tree.map) else {
            return node;
        };

        if let Some(NodeKind::Split { .. }) = self.kind.get(parent_id) {
        } else {
            return parent_id;
        }

        let children: Vec<_> = parent_id.children(&self.tree.map).collect();
        if children.len() != 2 {
            return parent_id;
        }
        let sibling = if children[0] == node { children[1] } else { children[0] };

        let sibling_kind = match self.kind.get(sibling) {
            Some(k) => k.clone(),
            None => return parent_id,
        };

        self.kind.insert(parent_id, sibling_kind.clone());
        match sibling_kind {
            NodeKind::Split { .. } => {
                let sib_children: Vec<_> = sibling.children(&self.tree.map).collect();
                for c in sib_children {
                    c.detach(&mut self.tree).push_back(parent_id);
                }
            }
            NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
            } => {
                if let Some(w) = window {
                    self.window_to_node.insert(w, parent_id);
                }
                self.kind.insert(
                    parent_id,
                    NodeKind::Leaf {
                        window,
                        fullscreen,
                        fullscreen_within_gaps,
                    },
                );
            }
        }

        node.detach(&mut self.tree).remove();
        sibling.detach(&mut self.tree).remove();
        self.kind.remove(node);
        self.kind.remove(sibling);
        parent_id
    }

    fn selection_of_layout(&self, layout: crate::layout_engine::LayoutId) -> Option<NodeId> {
        self.layouts
            .get(layout)
            .map(|s| self.tree.data.selection.current_selection(s.root))
    }

    fn ratio_to_fraction(ratio: f32) -> f64 {
        // Hyprland uses ratio in 0.1..1.9 with base of half the container.
        (ratio as f64 / 2.0).clamp(0.05, 0.95)
    }

    fn calculate_layout_recursive(
        &self,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        match self.kind.get(node) {
            Some(NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
            }) => {
                if let Some(w) = window {
                    let mut target = if *fullscreen {
                        screen
                    } else if *fullscreen_within_gaps {
                        Self::apply_outer_gaps(screen, gaps)
                    } else {
                        rect
                    };
                    if self.settings.pseudotile {
                        if let Some(size) = self.pseudo_sizes.get(w) {
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
                    out.push((*w, target));
                }
            }
            Some(NodeKind::Split { orientation, ratio }) => match orientation {
                Orientation::Horizontal => {
                    let gap = gaps.inner.horizontal as f64;
                    let total = rect.size.width;
                    let available = (total - gap).max(0.0);
                    let first_w = available * Self::ratio_to_fraction(*ratio);
                    let second_w = (available - first_w).max(0.0);
                    let r1 = CGRect::new(rect.origin, CGSize::new(first_w, rect.size.height));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x + first_w + gap, rect.origin.y),
                        CGSize::new(second_w, rect.size.height),
                    );
                    let mut it = node.children(&self.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, screen, gaps, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, screen, gaps, out);
                    }
                }
                Orientation::Vertical => {
                    let gap = gaps.inner.vertical as f64;
                    let total = rect.size.height;
                    let available = (total - gap).max(0.0);
                    let first_h = available * Self::ratio_to_fraction(*ratio);
                    let second_h = (available - first_h).max(0.0);
                    let r1 = CGRect::new(rect.origin, CGSize::new(rect.size.width, first_h));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x, rect.origin.y + first_h + gap),
                        CGSize::new(rect.size.width, second_h),
                    );
                    let mut it = node.children(&self.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, screen, gaps, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, screen, gaps, out);
                    }
                }
            },
            None => {}
        }
    }

    fn apply_outer_gaps(screen: CGRect, gaps: &crate::common::config::GapSettings) -> CGRect {
        compute_tiling_area(screen, gaps)
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
        let sel = self.tree.data.selection.current_selection(state.root);
        match self.kind.get(sel) {
            Some(NodeKind::Leaf { window, .. }) => *window,
            _ => None,
        }
    }

    fn find_neighbor_leaf(&self, from_leaf: NodeId, direction: Direction) -> Option<NodeId> {
        let mut current = from_leaf;

        while let Some(parent) = current.parent(&self.tree.map) {
            if let Some(NodeKind::Split { orientation, .. }) = self.kind.get(parent) {
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
                            return Some(self.find_closest_leaf_in_direction(target, direction));
                        }
                    }
                }
            }
            current = parent;
        }

        None
    }

    fn find_closest_leaf_in_direction(&self, root: NodeId, direction: Direction) -> NodeId {
        match self.kind.get(root) {
            Some(NodeKind::Leaf { .. }) => root,
            Some(NodeKind::Split { orientation, .. }) => {
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
                    children.first().copied()
                };

                if let Some(child) = target_child {
                    self.find_closest_leaf_in_direction(child, direction)
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

    fn store_last_frame(&self, layout: LayoutId, frame: LastFrame) {
        if let Ok(mut map) = self.last_frames.try_borrow_mut() {
            map.insert(layout, frame);
        }
    }

    fn rects_for_layout(&self, layout: LayoutId) -> Option<HashMap<NodeId, CGRect>> {
        let state = self.layouts.get(layout)?;
        let frame = self.last_frames.try_borrow().ok()?.get(&layout)?.clone();
        let mut rects = HashMap::default();
        let root_rect = Self::apply_outer_gaps(frame.screen, &frame.gaps);
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
        out.insert(node, rect);
        match self.kind.get(node) {
            Some(NodeKind::Split { orientation, ratio }) => {
                match orientation {
                    Orientation::Horizontal => {
                        let gap = gaps.inner.horizontal as f64;
                        let available = (rect.size.width - gap).max(0.0);
                        let first_w = available * Self::ratio_to_fraction(*ratio);
                        let second_w = (available - first_w).max(0.0);
                        let r1 = CGRect::new(rect.origin, CGSize::new(first_w, rect.size.height));
                        let r2 = CGRect::new(
                            CGPoint::new(rect.origin.x + first_w + gap, rect.origin.y),
                            CGSize::new(second_w, rect.size.height),
                        );
                        let mut it = node.children(&self.tree.map);
                        if let Some(first) = it.next() {
                            self.populate_rects(first, r1, screen, gaps, out);
                        }
                        if let Some(second) = it.next() {
                            self.populate_rects(second, r2, screen, gaps, out);
                        }
                    }
                    Orientation::Vertical => {
                        let gap = gaps.inner.vertical as f64;
                        let available = (rect.size.height - gap).max(0.0);
                        let first_h = available * Self::ratio_to_fraction(*ratio);
                        let second_h = (available - first_h).max(0.0);
                        let r1 = CGRect::new(rect.origin, CGSize::new(rect.size.width, first_h));
                        let r2 = CGRect::new(
                            CGPoint::new(rect.origin.x, rect.origin.y + first_h + gap),
                            CGSize::new(rect.size.width, second_h),
                        );
                        let mut it = node.children(&self.tree.map);
                        if let Some(first) = it.next() {
                            self.populate_rects(first, r1, screen, gaps, out);
                        }
                        if let Some(second) = it.next() {
                            self.populate_rects(second, r2, screen, gaps, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn choose_target_leaf(&self, layout: LayoutId) -> Option<NodeId> {
        let settings = &self.settings;
        // Prefer active selection
        if settings.use_active_for_splits {
            if let Some(sel) = self.selection_of_layout(layout) {
                return Some(self.descend_to_leaf(sel));
            }
        }

        // Try cursor-based hit test using last frame
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

        // Fallback to selection
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

    fn plan_split(
        &mut self,
        layout: LayoutId,
        target: NodeId,
        target_rect: Option<CGRect>,
    ) -> (Orientation, bool) {
        if self.settings.preserve_split {
            if let Some(parent) = target.parent(&self.tree.map) {
                if let Some(NodeKind::Split { orientation, .. }) = self.kind.get(parent) {
                    return (*orientation, false);
                }
            }
        }
        let mut new_first = false;
        if let Some(state) = self.layouts.get_mut(layout) {
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
                        let center =
                            CGPoint::new(rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
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
                    let center =
                        CGPoint::new(rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0);
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
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get(leaf).cloned() {
            let (orientation, new_first) = self.plan_split(layout, leaf, target_rect);
            let mut ratio = Self::clamp_ratio(self.settings.default_split_ratio);

            let existing_node = self.make_leaf(window);
            let new_node = self.make_leaf(Some(new_window));

            if let Some(w) = window {
                self.window_to_node.insert(w, existing_node);
            }
            self.window_to_node.insert(new_window, new_node);

            if self.settings.split_bias && new_first {
                ratio = Self::clamp_ratio(2.0 - ratio);
            }

            self.kind.insert(leaf, NodeKind::Split { orientation, ratio });

            let (first_child, second_child) = if new_first {
                (new_node, existing_node)
            } else {
                (existing_node, new_node)
            };

            first_child.detach(&mut self.tree).push_back(leaf);
            second_child.detach(&mut self.tree).push_back(leaf);

            self.tree.data.selection.select(&self.tree.map, new_node);
        }
    }

    fn insert_window_at_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let Some(state) = self.layouts.get(layout).copied() else {
            return;
        };
        let sel = self.tree.data.selection.current_selection(state.root);
        match self.kind.get_mut(sel) {
            Some(NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
            }) => {
                if window.is_none() {
                    *window = Some(wid);
                    *fullscreen = false;
                    *fullscreen_within_gaps = false;
                    self.window_to_node.insert(wid, sel);
                } else {
                    let existing = *window;
                    let left = self.make_leaf(existing);
                    let right = self.make_leaf(Some(wid));
                    self.window_to_node.insert(wid, right);
                    if let Some(w) = existing {
                        self.window_to_node.insert(w, left);
                    }
                    let ratio = Self::clamp_ratio(self.settings.default_split_ratio);
                    self.kind.insert(
                        sel,
                        NodeKind::Split {
                            orientation: Orientation::Horizontal,
                            ratio,
                        },
                    );
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

    fn remove_window_internal(&mut self, layout: crate::layout_engine::LayoutId, wid: WindowId) {
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                if !self.belongs_to_layout(state, node_id) {
                    return;
                }
            }
            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(node_id) {
                *window = None;
            }
            self.window_to_node.remove(&wid);
            self.pseudo_sizes.remove(&wid);
            let fallback = self.cleanup_after_removal(node_id);

            let sel_snapshot = self
                .layouts
                .get(layout)
                .map(|s| self.tree.data.selection.current_selection(s.root));
            let new_sel = match sel_snapshot {
                Some(sel) if self.kind.get(sel).is_some() => self.descend_to_leaf(sel),
                _ => self.descend_to_leaf(fallback),
            };
            self.tree.data.selection.select(&self.tree.map, new_sel);
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Components {
    selection: Selection,
}

impl crate::model::tree::Observer for Components {
    fn added_to_forest(&mut self, map: &NodeMap, node: NodeId) {
        self.dispatch_event(map, TreeEvent::AddedToForest(node))
    }

    fn added_to_parent(&mut self, map: &NodeMap, node: NodeId) {
        self.dispatch_event(map, TreeEvent::AddedToParent(node))
    }

    fn removing_from_parent(&mut self, map: &NodeMap, node: NodeId) {
        self.dispatch_event(map, TreeEvent::RemovingFromParent(node))
    }

    fn removed_child(_tree: &mut Tree<Self>, _parent: NodeId) {}

    fn removed_from_forest(&mut self, map: &NodeMap, node: NodeId) {
        self.dispatch_event(map, TreeEvent::RemovedFromForest(node))
    }
}

impl Components {
    fn dispatch_event(&mut self, map: &NodeMap, event: TreeEvent) {
        self.selection.handle_event(map, event);
    }
}

impl LayoutSystem for DwindleLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let leaf = self.make_leaf(None);
        let state = LayoutState {
            root: leaf,
            preselection: None,
        };
        let id = self.layouts.insert(state);
        self.insertion_hints.insert(id, InsertionHint::default());
        id
    }

    /// shallow
    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let mut windows = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            self.collect_windows_under(state.root, &mut windows);
        }
        let new_layout = self.create_layout();
        for w in windows {
            self.add_window_after_selection(new_layout, w);
        }
        new_layout
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.remove(layout) {
            let mut windows = Vec::new();
            self.collect_windows_under(state.root, &mut windows);
            for w in windows {
                self.window_to_node.remove(&w);
                self.pseudo_sizes.remove(&w);
            }
            let ids: Vec<_> = state.root.traverse_preorder(&self.tree.map).collect();
            for id in ids {
                self.kind.remove(id);
            }
            state.root.remove_root(&mut self.tree);
        }
        self.insertion_hints.remove(&layout);
        if let Ok(mut map) = self.last_frames.try_borrow_mut() {
            map.remove(&layout);
        }
    }

    fn draw_tree(&self, layout: LayoutId) -> String {
        fn write_node(this: &DwindleLayoutSystem, node: NodeId, out: &mut String, indent: usize) {
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
            "<empty dwindle>".to_string()
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
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut rect = Self::apply_outer_gaps(screen, gaps);
            if self.visible_windows_in_layout(layout).len() == 1 {
                rect = self.single_window_rect(rect);
            }
            self.calculate_layout_recursive(state.root, rect, screen, gaps, &mut out);
        }
        self.store_last_frame(
            layout,
            LastFrame {
                screen,
                gaps: gaps.clone(),
            },
        );
        out
    }

    fn update_settings(&mut self, settings: &crate::common::config::LayoutSettings) {
        self.settings = settings.dwindle.clone();
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.layouts.get(layout).and_then(|s| self.selection_window(s))
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            self.collect_windows_under(state.root, &mut out);
        }
        out
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(sel) = self.selection_of_layout(layout) {
            if self.kind.get(sel).is_some() {
                let leaf = self.descend_to_leaf(sel);
                self.collect_windows_under(leaf, &mut out);
            }
        }

        out
    }

    fn set_insertion_point(&mut self, layout: LayoutId, point: Option<CGPoint>) {
        self.insertion_hints.entry(layout).or_default().cursor = point;
    }

    fn set_preselection(&mut self, layout: LayoutId, direction: Option<Direction>) {
        if let Some(state) = self.layouts.get_mut(layout) {
            state.preselection = direction;
        }
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
        let raise_windows = self.visible_windows_in_layout(layout);
        if raise_windows.is_empty() {
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
        (focus, raise_windows)
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.layouts
            .get(layout)
            .and_then(|state| self.window_in_direction_from(state.root, direction))
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.layouts.get(layout).is_some() {
            let target = self.choose_target_leaf(layout);

            if let Some(target_leaf) = target {
                match self.kind.get(target_leaf) {
                    Some(NodeKind::Leaf { window, .. }) => {
                        if window.is_none() {
                            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(target_leaf) {
                                *window = Some(wid);
                                self.window_to_node.insert(wid, target_leaf);
                            }
                            self.tree.data.selection.select(&self.tree.map, target_leaf);
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
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if self.kind.get(node_id).is_none() {
                self.window_to_node.remove(&wid);
                self.pseudo_sizes.remove(&wid);
                return;
            }
            let root = self.find_layout_root(node_id);
            let layout = self
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
            self.window_to_node.keys().copied().filter(|w| w.pid == pid).collect();
        for w in windows {
            self.remove_window(w);
        }
        self.pseudo_sizes.retain(|wid, _| wid.pid != pid);
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let desired_set: HashSet<WindowId> = desired.iter().copied().collect();
        let mut current_set: HashSet<WindowId> = HashSet::default();
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut under: Vec<WindowId> = Vec::new();
            self.collect_windows_under(state.root, &mut under);
            for w in under.into_iter().filter(|w| w.pid == pid) {
                current_set.insert(w);
                if !desired_set.contains(&w) {
                    if let Some(&node) = self.window_to_node.get(&w) {
                        if let Some(NodeKind::Leaf {
                            fullscreen,
                            fullscreen_within_gaps,
                            ..
                        }) = self.kind.get(node)
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
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut under = Vec::new();
            self.collect_windows_under(state.root, &mut under);
            under.into_iter().any(|w| w.pid == pid)
        } else {
            false
        }
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        if let Some(&node) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                return self.belongs_to_layout(state, node);
            }
        }
        false
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        if let Some(&node) = self.window_to_node.get(&wid) {
            if self.kind.get(node).is_none() {
                self.window_to_node.remove(&wid);
                return false;
            }
            if let Some(state) = self.layouts.get(layout).copied() {
                let belongs = self.belongs_to_layout(state, node);
                if belongs {
                    self.tree.data.selection.select(&self.tree.map, node);
                    return true;
                }
            }
        }
        false
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
        if let Some(&node) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                if !self.belongs_to_layout(state, node) {
                    return;
                }
                if let Some(NodeKind::Leaf {
                    window: _,
                    fullscreen,
                    fullscreen_within_gaps,
                }) = self.kind.get_mut(node)
                {
                    if new_frame == screen {
                        *fullscreen = true;
                        *fullscreen_within_gaps = false;
                    } else if old_frame == screen {
                        *fullscreen = false;
                    } else {
                        let tiling = Self::apply_outer_gaps(screen, gaps);
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
        let sel_snapshot = self.selection_of_layout(layout);
        let Some(sel) = sel_snapshot else {
            return false;
        };
        let sel_leaf = self.descend_to_leaf(sel);
        let Some(neighbor_leaf) = self.find_neighbor_leaf(sel_leaf, direction) else {
            return false;
        };
        let (mut a_window, mut b_window) = (None, None);
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(sel_leaf) {
            a_window = *window;
        }
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(neighbor_leaf) {
            b_window = *window;
        }
        if a_window.is_none() && b_window.is_none() {
            return false;
        }
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(sel_leaf) {
            *window = b_window;
        }
        if let Some(NodeKind::Leaf { window, .. }) = self.kind.get_mut(neighbor_leaf) {
            *window = a_window;
        }
        if let Some(w) = a_window {
            self.window_to_node.insert(w, neighbor_leaf);
        }
        if let Some(w) = b_window {
            self.window_to_node.insert(w, sel_leaf);
        }
        self.tree.data.selection.select(&self.tree.map, neighbor_leaf);
        true
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        let Some(&node_a) = self.window_to_node.get(&a) else {
            return false;
        };
        let Some(&node_b) = self.window_to_node.get(&b) else {
            return false;
        };
        if node_a == node_b {
            return false;
        }

        if let Some(state) = self.layouts.get(layout).copied() {
            if !self.belongs_to_layout(state, node_a) || !self.belongs_to_layout(state, node_b) {
                return false;
            }
        } else {
            return false;
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

        if let Some(w) = a_window {
            self.window_to_node.insert(w, node_b);
        }
        if let Some(w) = b_window {
            self.window_to_node.insert(w, node_a);
        }

        true
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
                if let Some(w) = window {
                    self.window_to_node.insert(w, left);
                }
                let ratio = Self::clamp_ratio(self.settings.default_split_ratio);
                self.kind.insert(target, NodeKind::Split { orientation, ratio });
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
                if let Some(grandparent) = parent.parent(&self.tree.map) {
                    let mut windows = Vec::new();
                    self.collect_windows_under(parent, &mut windows);

                    let _ = parent.detach(&mut self.tree);
                    self.kind.remove(parent);

                    if let Some(first_window) = windows.first() {
                        let new_leaf = self.make_leaf(Some(*first_window));
                        new_leaf.detach(&mut self.tree).push_back(grandparent);

                        for window in windows {
                            self.window_to_node.insert(window, new_leaf);
                        }

                        self.tree.data.selection.select(&self.tree.map, new_leaf);
                    }
                }
                break;
            }
            current = parent;
        }
    }

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        _: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { false }

    fn unstack_parent_of_selection(
        &mut self,
        _: LayoutId,
        _: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        vec![]
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        let Some(sel) = self.selection_of_layout(layout) else {
            return;
        };
        let sel_leaf = self.descend_to_leaf(sel);
        let map = &self.tree.map;

        let Some(parent) = sel_leaf.parent(map) else {
            return;
        };

        let Some(grandparent) = parent.parent(map) else {
            return;
        };

        let mut windows: Vec<WindowId> = Vec::new();
        self.collect_windows_under(parent, &mut windows);
        if windows.is_empty() {
            return;
        }

        let _ = parent.detach(&mut self.tree);

        let ids: Vec<_> = parent.traverse_preorder(&self.tree.map).collect();
        for id in ids {
            self.kind.remove(id);
        }

        let mut first_new_leaf: Option<NodeId> = None;
        for w in windows {
            let new_leaf = self.make_leaf(Some(w));
            new_leaf.detach(&mut self.tree).push_back(grandparent);
            self.window_to_node.insert(w, new_leaf);
            if first_new_leaf.is_none() {
                first_new_leaf = Some(new_leaf);
            }
        }

        if let Some(n) = first_new_leaf {
            self.tree.data.selection.select(&self.tree.map, n);
        }
    }

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
                        let (orientation, dir_is_first_side) = if dist_left.min(dist_right)
                            < dist_top.min(dist_bottom)
                        {
                            // horizontal axis
                            let dir_first = cursor.x <= (min_x + max_x) / 2.0;
                            (Orientation::Horizontal, dir_first)
                        } else {
                            let dir_first = cursor.y <= (min_y + max_y) / 2.0;
                            (Orientation::Vertical, dir_first)
                        };

                        while let Some(parent) = node.parent(&self.tree.map) {
                            if let Some(NodeKind::Split { ratio, orientation: o }) =
                                self.kind.get_mut(parent)
                            {
                                if *o == orientation {
                                    let is_first =
                                        Some(node) == parent.first_child(&self.tree.map);
                                    let delta = amount as f32;
                                    match orientation {
                                        Orientation::Horizontal => {
                                            if dir_is_first_side {
                                                if is_first {
                                                    *ratio = Self::clamp_ratio(*ratio - delta);
                                                } else {
                                                    *ratio = Self::clamp_ratio(*ratio + delta);
                                                }
                                            } else if is_first {
                                                *ratio = Self::clamp_ratio(*ratio + delta);
                                            } else {
                                                *ratio = Self::clamp_ratio(*ratio - delta);
                                            }
                                        }
                                        Orientation::Vertical => {
                                            if dir_is_first_side {
                                                if is_first {
                                                    *ratio = Self::clamp_ratio(*ratio - delta);
                                                } else {
                                                    *ratio = Self::clamp_ratio(*ratio + delta);
                                                }
                                            } else if is_first {
                                                *ratio = Self::clamp_ratio(*ratio + delta);
                                            } else {
                                                *ratio = Self::clamp_ratio(*ratio - delta);
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

        while let Some(parent) = node.parent(&self.tree.map) {
            if let Some(NodeKind::Split { ratio, .. }) = self.kind.get_mut(parent) {
                let is_first = Some(node) == parent.first_child(&self.tree.map);
                let delta = amount as f32;
                if is_first {
                    let new_ratio = Self::clamp_ratio(*ratio - delta);
                    *ratio = new_ratio;
                } else {
                    let new_ratio = Self::clamp_ratio(*ratio + delta);
                    *ratio = new_ratio;
                }
                break;
            }
            node = parent;
        }
    }

    fn rebalance(&mut self, _layout: LayoutId) {}

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

    fn toggle_split_of_selection(&mut self, layout: LayoutId) {
        if let Some(sel) = self.selection_of_layout(layout) {
            let sel_leaf = self.descend_to_leaf(sel);
            if let Some(parent) = sel_leaf.parent(&self.tree.map) {
                if let Some(NodeKind::Split { orientation, .. }) = self.kind.get_mut(parent) {
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
            if let Some(parent) = sel_leaf.parent(&self.tree.map) {
                let children: Vec<_> = parent.children(&self.tree.map).collect();
                if children.len() == 2 {
                    let first_id = children[0];
                    let second_id = children[1];
                    let detached_second = second_id.detach(&mut self.tree);
                    detached_second.insert_before(first_id).finish();
                }
            }
        }
    }

    fn move_selection_to_root(&mut self, layout: LayoutId, stable: bool) {
        let Some(sel) = self.selection_of_layout(layout) else {
            return;
        };
        let leaf = self.descend_to_leaf(sel);
        let root = self.find_layout_root(leaf);
        if leaf == root {
            return;
        }
        let Some(mut ancestor) = leaf.parent(&self.tree.map) else { return };
        while let Some(parent) = ancestor.parent(&self.tree.map) {
            if parent == root {
                break;
            }
            ancestor = parent;
        }
        if ancestor.parent(&self.tree.map) != Some(root) {
            return;
        }
        let children: Vec<_> = root.children(&self.tree.map).collect();
        if children.len() != 2 {
            return;
        }
        let ancestor_is_first = children.first().copied() == Some(ancestor);
        let swap_node = if ancestor_is_first {
            children.get(1).copied()
        } else {
            children.get(0).copied()
        };
        let Some(swap_node) = swap_node else { return };

        if ancestor_is_first {
            if !stable {
                let detached = ancestor.detach(&mut self.tree);
                detached.insert_after(swap_node).finish();
            }
        } else if stable {
            // keep ancestor on the second side; do nothing
        } else {
            let detached = ancestor.detach(&mut self.tree);
            detached.insert_before(swap_node).finish();
        }
    }
}
