use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
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

    fn equalize_tree(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.get(layout).copied() {
            self.equalize_node_recursive(state.root);
        }
    }

    fn equalize_node_recursive(&mut self, node: NodeId) {
        match self.kind.get_mut(node) {
            Some(NodeKind::Split { ratio, .. }) => {
                *ratio = 0.5;
                let children: Vec<_> = node.children(&self.tree.map).collect();
                for child in children {
                    self.equalize_node_recursive(child);
                }
            }
            _ => {}
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

            let left = self.make_leaf(window);
            let right = self.make_leaf(Some(new_window));

            if let Some(w) = window {
                self.window_to_node.insert(w, left);
            }
            self.window_to_node.insert(new_window, right);

            self.kind.insert(leaf, NodeKind::Split { orientation, ratio: 0.5 });

            match direction {
                Direction::Left | Direction::Up => {
                    right.detach(&mut self.tree).push_back(leaf);
                    left.detach(&mut self.tree).push_back(leaf);
                }
                Direction::Right | Direction::Down => {
                    left.detach(&mut self.tree).push_back(leaf);
                    right.detach(&mut self.tree).push_back(leaf);
                }
            }

            self.tree.data.selection.select(&self.tree.map, right);
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
        let NodeKind::Split { .. } = self.kind[parent_id] else {
            return parent_id;
        };

        let children: Vec<_> = parent_id.children(&self.tree.map).collect();
        if children.len() != 2 {
            return parent_id;
        }
        let sibling = if children[0] == node {
            children[1]
        } else {
            children[0]
        };

        let sibling_kind = self.kind[sibling].clone();
        self.kind.insert(parent_id, sibling_kind.clone());
        match sibling_kind {
            NodeKind::Split { .. } => {
                let sib_children: Vec<_> = sibling.children(&self.tree.map).collect();
                for c in sib_children {
                    c.detach(&mut self.tree).push_back(parent_id);
                }
            }
            NodeKind::Leaf { window, .. } => {
                if let Some(w) = window {
                    self.window_to_node.insert(w, parent_id);
                }
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
            Some(NodeKind::Leaf { window, fullscreen, .. }) => {
                if window.is_none() {
                    *window = Some(wid);
                    *fullscreen = false;
                    self.window_to_node.insert(wid, sel);
                } else {
                    let existing = *window;
                    let left = self.make_leaf(existing);
                    let right = self.make_leaf(Some(wid));
                    self.window_to_node.insert(wid, right);
                    if let Some(w) = existing {
                        self.window_to_node.insert(w, left);
                    }
                    self.kind.insert(sel, NodeKind::Split {
                        orientation: Orientation::Horizontal,
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
            let needs_reset = sel_snapshot.and_then(|sel| self.kind.get(sel)).is_none();
            let new_sel = if needs_reset {
                self.descend_to_leaf(fallback)
            } else {
                self.descend_to_leaf(sel_snapshot.unwrap())
            };
            self.tree.data.selection.select(&self.tree.map, new_sel);
        }
    }

    fn calculate_layout_recursive(
        &self,
        node: NodeId,
        rect: CGRect,
        gaps: &crate::common::config::GapSettings,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        match &self.kind[node] {
            NodeKind::Leaf { window, fullscreen, .. } => {
                if let Some(w) = window {
                    let r = if *fullscreen { rect } else { rect };
                    out.push((*w, r));
                }
            }
            NodeKind::Split { orientation, ratio } => match orientation {
                Orientation::Horizontal => {
                    let gap = gaps.inner.horizontal;
                    let total = rect.size.width;
                    let first_w = (total - gap) as f32 * *ratio;
                    let second_w = (total - gap) - f64::from(first_w);
                    let r1 =
                        CGRect::new(rect.origin, CGSize::new(first_w as f64, rect.size.height));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x + first_w as f64 + gap, rect.origin.y),
                        CGSize::new(second_w.max(0.0), rect.size.height),
                    );
                    let mut it = node.children(&self.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, gaps, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, gaps, out);
                    }
                }
                Orientation::Vertical => {
                    let gap = gaps.inner.vertical;
                    let total = rect.size.height;
                    let first_h = (total - gap) as f32 * *ratio;
                    let second_h = (total - gap) - f64::from(first_h);
                    let r1 = CGRect::new(rect.origin, CGSize::new(rect.size.width, first_h as f64));
                    let r2 = CGRect::new(
                        CGPoint::new(rect.origin.x, rect.origin.y + first_h as f64 + gap),
                        CGSize::new(rect.size.width, second_h.max(0.0)),
                    );
                    let mut it = node.children(&self.tree.map);
                    if let Some(first) = it.next() {
                        self.calculate_layout_recursive(first, r1, gaps, out);
                    }
                    if let Some(second) = it.next() {
                        self.calculate_layout_recursive(second, r2, gaps, out);
                    }
                }
            },
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

    fn removed_child(tree: &mut Tree<Self>, parent: NodeId) {
        if parent.parent(&tree.map).is_none() {
            return;
        }
        if parent.is_empty(&tree.map) {
            parent.detach(tree).remove();
        } else if parent.first_child(&tree.map) == parent.last_child(&tree.map) {
            let child = parent.first_child(&tree.map).unwrap();
            child.detach(tree).insert_after(parent).finish();
        }
    }

    fn removed_from_forest(&mut self, map: &NodeMap, node: NodeId) {
        self.dispatch_event(map, TreeEvent::RemovedFromForest(node))
    }
}

impl Components {
    fn dispatch_event(&mut self, map: &NodeMap, event: TreeEvent) {
        self.selection.handle_event(map, event);
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
            self.calculate_layout_recursive(state.root, rect, gaps, &mut out);
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
        let focus = match &self.kind[next_leaf] {
            NodeKind::Leaf { window, .. } => *window,
            _ => None,
        };
        (focus, raise_windows)
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
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut under = Vec::new();
            self.collect_windows_under(state.root, &mut under);
            for w in under.into_iter().filter(|w| w.pid == pid) {
                self.remove_window_internal(layout, w);
            }
        }
        for w in desired {
            self.add_window_after_selection(layout, w);
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
    ) {
        if let Some(&node) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                if !self.belongs_to_layout(state, node) {
                    return;
                }
                if let Some(NodeKind::Leaf { window: _, fullscreen, .. }) = self.kind.get_mut(node)
                {
                    if new_frame == screen {
                        *fullscreen = true;
                    } else if old_frame == screen {
                        *fullscreen = false;
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
                window: Some(w), fullscreen, ..
            }) = self.kind.get_mut(sel_leaf)
            {
                *fullscreen = !*fullscreen;
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

    fn apply_stacking_to_parent_of_selection(&mut self, _layout: LayoutId) -> Vec<WindowId> {
        vec![]
    }

    fn unstack_parent_of_selection(&mut self, _layout: LayoutId) -> Vec<WindowId> { vec![] }

    fn unjoin_selection(&mut self, _layout: LayoutId) {}

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
                    *ratio = (*ratio - delta).clamp(0.05, 0.95);
                } else {
                    *ratio = (*ratio + delta).clamp(0.05, 0.95);
                }
                break;
            }
            node = parent;
        }
    }

    fn rebalance(&mut self, layout: LayoutId) { self.equalize_tree(layout); }
}
