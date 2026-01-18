use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::selection::*;
use crate::model::tree::{NodeId, NodeMap, Tree};

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize, Clone, Copy)]
struct LayoutState {
    root: NodeId,
}

#[derive(Serialize, Deserialize)]
pub struct BspLayoutSystem {
    layouts: slotmap::SlotMap<crate::layout_engine::LayoutId, LayoutState>,
    tree: Tree<Components>,
    kind: slotmap::SecondaryMap<NodeId, NodeKind>,
    window_to_node: HashMap<WindowId, NodeId>,
}

impl BspLayoutSystem {
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
            let orientation = direction.orientation();

            let existing_node = self.make_leaf(window);
            let new_node = self.make_leaf(Some(new_window));

            if let Some(w) = window {
                self.window_to_node.insert(w, existing_node);
            }
            self.window_to_node.insert(new_window, new_node);

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
            window_to_node: Default::default(),
        }
    }
}

impl BspLayoutSystem {
    fn make_leaf(&mut self, window: Option<WindowId>) -> NodeId {
        let id = self.tree.mk_node().into_id();
        self.kind.insert(id, NodeKind::Leaf {
            window,
            fullscreen: false,
            fullscreen_within_gaps: false,
            preselected: None,
        });
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
        let sibling = if children[0] == node {
            children[1]
        } else {
            children[0]
        };

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
                preselected,
            } => {
                if let Some(w) = window {
                    self.window_to_node.insert(w, parent_id);
                }
                self.kind.insert(parent_id, NodeKind::Leaf {
                    window,
                    fullscreen,
                    fullscreen_within_gaps,
                    preselected,
                });
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
                    self.window_to_node.insert(wid, sel);
                } else {
                    let existing = *window;
                    let left = self.make_leaf(existing);
                    let right = self.make_leaf(Some(wid));
                    self.window_to_node.insert(wid, right);
                    if let Some(w) = existing {
                        self.window_to_node.insert(w, left);
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
                ..
            }) => {
                if let Some(w) = window {
                    let target = if *fullscreen {
                        screen
                    } else if *fullscreen_within_gaps {
                        Self::apply_outer_gaps(screen, gaps)
                    } else {
                        rect
                    };
                    out.push((*w, target));
                }
            }
            Some(NodeKind::Split { orientation, ratio }) => match orientation {
                Orientation::Horizontal => {
                    let gap = gaps.inner.horizontal as f64;
                    let total = rect.size.width;
                    let available = (total - gap).max(0.0);
                    let first_w_f = available * (*ratio as f64);
                    let first_w = first_w_f.max(0.0);
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
                    let first_h_f = available * (*ratio as f64);
                    let first_h = first_h_f.max(0.0);
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

    fn selection_window(&self, state: &LayoutState) -> Option<WindowId> {
        let sel = self.tree.data.selection.current_selection(state.root);
        match self.kind.get(sel) {
            Some(NodeKind::Leaf { window, .. }) => *window,
            _ => None,
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

    #[test]
    fn fibonacci_spiral_alternates_split_orientation() {
        let mut system = BspLayoutSystem::default();
        let layout = system.create_layout();

        // Add first window - it takes the full layout
        system.add_window_after_selection(layout, w(1));

        // Add second window - should split horizontally (depth 0)
        system.add_window_after_selection(layout, w(2));
        let tree = system.draw_tree(layout);
        assert!(
            tree.contains("Horizontal"),
            "Second window should create horizontal split at depth 0"
        );

        // Add third window - should split vertically (depth 1)
        system.add_window_after_selection(layout, w(3));
        let tree = system.draw_tree(layout);
        let horizontal_count = tree.matches("Horizontal").count();
        let vertical_count = tree.matches("Vertical").count();
        assert_eq!(horizontal_count, 1, "Should have 1 horizontal split");
        assert_eq!(vertical_count, 1, "Should have 1 vertical split");

        // Add fourth window - should split horizontally (depth 2)
        system.add_window_after_selection(layout, w(4));
        let tree = system.draw_tree(layout);
        let horizontal_count = tree.matches("Horizontal").count();
        let vertical_count = tree.matches("Vertical").count();
        assert_eq!(horizontal_count, 2, "Should have 2 horizontal splits");
        assert_eq!(vertical_count, 1, "Should have 1 vertical split");

        // Add fifth window - should split vertically (depth 3)
        system.add_window_after_selection(layout, w(5));
        let tree = system.draw_tree(layout);
        let horizontal_count = tree.matches("Horizontal").count();
        let vertical_count = tree.matches("Vertical").count();
        assert_eq!(horizontal_count, 2, "Should have 2 horizontal splits");
        assert_eq!(vertical_count, 2, "Should have 2 vertical splits");
    }
}

impl LayoutSystem for BspLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let leaf = self.make_leaf(None);
        let state = LayoutState { root: leaf };
        self.layouts.insert(state)
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
            }
            let ids: Vec<_> = state.root.traverse_preorder(&self.tree.map).collect();
            for id in ids {
                self.kind.remove(id);
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
            let rect = Self::apply_outer_gaps(screen, gaps);
            self.calculate_layout_recursive(state.root, rect, screen, gaps, &mut out);
        }
        out
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
            // Try smart insertion first (with preselection support)
            if !self.smart_insert_window(layout, wid) {
                // Fall back to default insertion
                self.insert_window_at_selection(layout, wid);
            }
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if self.kind.get(node_id).is_none() {
                self.window_to_node.remove(&wid);
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
                    ..
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
        let sel_snapshot = self.selection_of_layout(layout);
        let Some(mut node) = sel_snapshot else {
            return;
        };

        while let Some(parent) = node.parent(&self.tree.map) {
            if let Some(NodeKind::Split { ratio, .. }) = self.kind.get_mut(parent) {
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
}
