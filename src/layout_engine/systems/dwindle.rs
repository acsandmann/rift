use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::selection::*;
use crate::model::tree::{NodeId, NodeMap, Tree};

/// Node kinds for dwindle layout
/// Similar to BSP but split direction is determined dynamically based on available space
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
    },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct LayoutState {
    root: NodeId,
}

#[derive(Serialize, Deserialize)]
pub struct DwindleLayoutSystem {
    layouts: slotmap::SlotMap<crate::layout_engine::LayoutId, LayoutState>,
    tree: Tree<Components>,
    kind: slotmap::SecondaryMap<NodeId, NodeKind>,
    window_to_node: HashMap<WindowId, NodeId>,
}

impl DwindleLayoutSystem {
    /// Find a neighboring leaf in the specified direction
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

    /// Find the closest leaf in a direction by descending the tree
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

    /// Create a new leaf node
    fn make_leaf(&mut self, window: Option<WindowId>) -> NodeId {
        let id = self.tree.mk_node().into_id();
        self.kind.insert(id, NodeKind::Leaf {
            window,
            fullscreen: false,
            fullscreen_within_gaps: false,
        });
        if let Some(w) = window {
            self.window_to_node.insert(w, id);
        }
        id
    }

    /// Descend to a leaf node
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

    /// Collect all windows under a node
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

    /// Find the root of a layout tree
    fn find_layout_root(&self, mut node: NodeId) -> NodeId {
        while let Some(p) = node.parent(&self.tree.map) {
            node = p;
        }
        node
    }

    /// Check if a node belongs to a layout
    fn belongs_to_layout(&self, layout: LayoutState, node: NodeId) -> bool {
        if self.kind.get(node).is_none() {
            return false;
        }
        self.find_layout_root(node) == layout.root
    }

    /// Cleanup after removing a window
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
            NodeKind::Leaf { window, fullscreen, fullscreen_within_gaps } => {
                if let Some(w) = window {
                    self.window_to_node.insert(w, parent_id);
                }
                self.kind.insert(parent_id, NodeKind::Leaf {
                    window,
                    fullscreen,
                    fullscreen_within_gaps,
                });
            }
        }

        node.detach(&mut self.tree).remove();
        sibling.detach(&mut self.tree).remove();
        self.kind.remove(node);
        self.kind.remove(sibling);
        parent_id
    }

    /// Get the current selection for a layout
    fn selection_of_layout(&self, layout: crate::layout_engine::LayoutId) -> Option<NodeId> {
        self.layouts
            .get(layout)
            .map(|s| self.tree.data.selection.current_selection(s.root))
    }

    /// Determine split orientation based on available space (Fibonacci spiral logic)
    /// This is the key difference from BSP: orientation is determined by aspect ratio
    fn determine_split_orientation(&self, rect: CGRect) -> Orientation {
        // Hyprland's logic: splitTop = availableSize.y > availableSize.x
        // If height > width, split vertically (top/bottom)
        // If width >= height, split horizontally (left/right)
        if rect.size.height > rect.size.width {
            Orientation::Vertical
        } else {
            Orientation::Horizontal
        }
    }

    /// Insert a window at the current selection with dwindle logic
    fn insert_window_at_selection(
        &mut self,
        layout: crate::layout_engine::LayoutId,
        wid: WindowId,
        last_rect: Option<CGRect>,
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

                    // Determine orientation based on available space (dwindle spiral logic)
                    let orientation = if let Some(rect) = last_rect {
                        self.determine_split_orientation(rect)
                    } else {
                        Orientation::Horizontal // Default fallback
                    };

                    self.kind.insert(sel, NodeKind::Split {
                        orientation,
                        ratio: 0.5,
                    });
                    left.detach(&mut self.tree).push_back(sel);
                    right.detach(&mut self.tree).push_back(sel);
                    self.tree.data.selection.select(&self.tree.map, right);
                }
            }
            Some(NodeKind::Split { .. }) => {
                let leaf = self.descend_to_leaf(sel);
                self.tree.data.selection.select(&self.tree.map, leaf);
                self.insert_window_at_selection(layout, wid, last_rect);
            }
            None => {}
        }
    }

    /// Remove a window from a layout
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

    /// Calculate layout recursively with proper gap handling
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

    /// Apply outer gaps to the screen rectangle
    fn apply_outer_gaps(screen: CGRect, gaps: &crate::common::config::GapSettings) -> CGRect {
        compute_tiling_area(screen, gaps)
    }

    /// Get the window in the current selection
    fn selection_window(&self, state: &LayoutState) -> Option<WindowId> {
        let sel = self.tree.data.selection.current_selection(state.root);
        match self.kind.get(sel) {
            Some(NodeKind::Leaf { window, .. }) => *window,
            _ => None,
        }
    }

    /// Helper to get rect for node - used for determining split orientation
    fn get_node_rect(
        &self,
        layout: LayoutId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> Option<CGRect> {
        if let Some(state) = self.layouts.get(layout).copied() {
            Some(Self::apply_outer_gaps(screen, gaps))
        } else {
            None
        }
    }
}

impl Default for DwindleLayoutSystem {
    fn default() -> Self {
        Self {
            layouts: Default::default(),
            tree: Tree::with_observer(Components::default()),
            kind: Default::default(),
            window_to_node: Default::default(),
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
        let state = LayoutState { root: leaf };
        self.layouts.insert(state)
    }

    /// Shallow clone
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

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.layouts.get(layout).is_some() {
            // For dwindle, we need to pass layout context to determine split orientation
            // We'll use a placeholder rect for now - in practice this would be calculated
            // from the current layout state
            self.insert_window_at_selection(layout, wid, None);
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

        for w in desired.into_iter() {
            if !current_set.contains(&w) {
                self.add_window_after_selection(layout, w);
            }
        }
    }

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut windows = Vec::new();
            self.collect_windows_under(state.root, &mut windows);
            windows.iter().any(|w| w.pid == pid)
        } else {
            false
        }
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                return self.belongs_to_layout(state, node_id);
            }
        }
        false
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                if self.belongs_to_layout(state, node_id) {
                    self.tree.data.selection.select(&self.tree.map, node_id);
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
        _old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) {
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                if !self.belongs_to_layout(state, node_id) {
                    return;
                }
                if let Some(parent_id) = node_id.parent(&self.tree.map) {
                    if let Some(NodeKind::Split { orientation, ratio }) = self.kind.get_mut(parent_id) {
                        let children: Vec<_> = parent_id.children(&self.tree.map).collect();
                        if children.len() == 2 {
                            let is_first = children[0] == node_id;

                            // Calculate parent rect to determine new ratio
                            let mut parent_rect = Self::apply_outer_gaps(screen, gaps);
                            let mut current = parent_id;
                            while let Some(pp) = current.parent(&self.tree.map) {
                                current = pp;
                            }

                            match *orientation {
                                Orientation::Horizontal => {
                                    let gap = gaps.inner.horizontal as f64;
                                    let available = (parent_rect.size.width - gap).max(0.0);
                                    let new_width = new_frame.size.width;
                                    let new_ratio = if is_first {
                                        (new_width / available).clamp(0.1, 0.9) as f32
                                    } else {
                                        1.0 - ((new_width / available).clamp(0.1, 0.9) as f32)
                                    };
                                    *ratio = new_ratio;
                                }
                                Orientation::Vertical => {
                                    let gap = gaps.inner.vertical as f64;
                                    let available = (parent_rect.size.height - gap).max(0.0);
                                    let new_height = new_frame.size.height;
                                    let new_ratio = if is_first {
                                        (new_height / available).clamp(0.1, 0.9) as f32
                                    } else {
                                        1.0 - ((new_height / available).clamp(0.1, 0.9) as f32)
                                    };
                                    *ratio = new_ratio;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        if let (Some(&a_node), Some(&b_node)) = (self.window_to_node.get(&a), self.window_to_node.get(&b)) {
            if let Some(state) = self.layouts.get(layout).copied() {
                if self.belongs_to_layout(state, a_node) && self.belongs_to_layout(state, b_node) {
                    if let Some(NodeKind::Leaf { window: window_a, .. }) = self.kind.get_mut(a_node) {
                        *window_a = Some(b);
                    }
                    if let Some(NodeKind::Leaf { window: window_b, .. }) = self.kind.get_mut(b_node) {
                        *window_b = Some(a);
                    }
                    self.window_to_node.insert(a, b_node);
                    self.window_to_node.insert(b, a_node);
                    return true;
                }
            }
        }
        false
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(current_sel) = self.selection_of_layout(layout) else {
            return false;
        };
        let current_leaf = self.descend_to_leaf(current_sel);
        let Some(target_leaf) = self.find_neighbor_leaf(current_leaf, direction) else {
            return false;
        };

        if let (Some(NodeKind::Leaf { window: current_window, .. }), Some(NodeKind::Leaf { window: target_window, .. })) =
            (self.kind.get(current_leaf), self.kind.get(target_leaf))
        {
            if let (Some(cw), Some(tw)) = (*current_window, *target_window) {
                return self.swap_windows(layout, cw, tw);
            }
        }
        false
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        if let Some(sel) = self.selection_of_layout(from_layout) {
            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get(sel) {
                if let Some(w) = *window {
                    self.remove_window_internal(from_layout, w);
                    self.add_window_after_selection(to_layout, w);
                }
            }
        }
    }

    fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind) {
        if let Some(sel) = self.selection_of_layout(layout) {
            if let Some(NodeKind::Leaf { window, .. }) = self.kind.get(sel) {
                if window.is_some() {
                    let orientation = match kind {
                        LayoutKind::Horizontal => Orientation::Horizontal,
                        LayoutKind::Vertical => Orientation::Vertical,
                        _ => return, // Dwindle doesn't support stacking
                    };

                    let existing = *window;
                    let left = self.make_leaf(existing);
                    let right = self.make_leaf(None);

                    if let Some(w) = existing {
                        self.window_to_node.insert(w, left);
                    }

                    self.kind.insert(sel, NodeKind::Split {
                        orientation,
                        ratio: 0.5,
                    });
                    left.detach(&mut self.tree).push_back(sel);
                    right.detach(&mut self.tree).push_back(sel);
                    self.tree.data.selection.select(&self.tree.map, right);
                }
            }
        }
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        if let Some(sel) = self.selection_of_layout(layout) {
            if let Some(NodeKind::Leaf { fullscreen, fullscreen_within_gaps, .. }) = self.kind.get_mut(sel) {
                *fullscreen = !*fullscreen;
                if *fullscreen {
                    *fullscreen_within_gaps = false;
                }
            }
        }
        self.visible_windows_in_layout(layout)
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        if let Some(sel) = self.selection_of_layout(layout) {
            if let Some(NodeKind::Leaf { fullscreen, fullscreen_within_gaps, .. }) = self.kind.get_mut(sel) {
                *fullscreen_within_gaps = !*fullscreen_within_gaps;
                if *fullscreen_within_gaps {
                    *fullscreen = false;
                }
            }
        }
        self.visible_windows_in_layout(layout)
    }

    fn join_selection_with_direction(&mut self, _layout: LayoutId, _direction: Direction) {
        // Not supported in dwindle - this is a traditional layout feature
    }

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        // Not supported in dwindle
        self.visible_windows_in_layout(layout)
    }

    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        // Not supported in dwindle
        self.visible_windows_in_layout(layout)
    }

    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool {
        // Dwindle doesn't support stacking
        false
    }

    fn unjoin_selection(&mut self, _layout: LayoutId) {
        // Not supported in dwindle
    }

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        if let Some(sel) = self.selection_of_layout(layout) {
            if let Some(parent_id) = sel.parent(&self.tree.map) {
                if let Some(NodeKind::Split { ratio, .. }) = self.kind.get_mut(parent_id) {
                    let children: Vec<_> = parent_id.children(&self.tree.map).collect();
                    if children.len() == 2 {
                        let is_first = children[0] == sel;
                        let delta = (amount / 100.0) as f32;
                        let new_ratio = if is_first {
                            (*ratio + delta).clamp(0.1, 0.9)
                        } else {
                            (*ratio - delta).clamp(0.1, 0.9)
                        };
                        *ratio = new_ratio;
                    }
                }
            }
        }
    }

    fn rebalance(&mut self, layout: LayoutId) {
        fn rebalance_recursive(this: &mut DwindleLayoutSystem, node: NodeId) {
            if let Some(NodeKind::Split { ratio, .. }) = this.kind.get_mut(node) {
                *ratio = 0.5;
                for child in node.children(&this.tree.map).collect::<Vec<_>>() {
                    rebalance_recursive(this, child);
                }
            }
        }

        if let Some(state) = self.layouts.get(layout).copied() {
            rebalance_recursive(self, state.root);
        }
    }

    fn toggle_tile_orientation(&mut self, layout: LayoutId) {
        if let Some(sel) = self.selection_of_layout(layout) {
            if let Some(parent_id) = sel.parent(&self.tree.map) {
                if let Some(NodeKind::Split { orientation, .. }) = self.kind.get_mut(parent_id) {
                    *orientation = match *orientation {
                        Orientation::Horizontal => Orientation::Vertical,
                        Orientation::Vertical => Orientation::Horizontal,
                    };
                }
            }
        }
    }
}
