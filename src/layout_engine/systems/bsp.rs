use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use super::LayoutSystem;
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
use crate::layout_engine::{Direction, LayoutKind, Orientation};

slotmap::new_key_type! { pub struct BspNodeId; }

#[derive(Serialize, Deserialize, Clone)]
enum NodeKind {
    Split {
        orientation: Orientation,
        ratio: f32,
        first: BspNodeId,
        second: BspNodeId,
    },
    Leaf {
        window: Option<WindowId>,
        fullscreen: bool,
    },
}

#[derive(Serialize, Deserialize, Clone)]
struct Node {
    parent: Option<BspNodeId>,
    kind: NodeKind,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct LayoutState {
    root: BspNodeId,
    selection: BspNodeId,
}

#[derive(Serialize, Deserialize, Default)]
pub struct BspLayoutSystem {
    layouts: slotmap::SlotMap<crate::layout_engine::LayoutId, LayoutState>,
    nodes: slotmap::SlotMap<BspNodeId, Node>,
    window_to_node: HashMap<WindowId, BspNodeId>,
}

impl BspLayoutSystem {
    pub fn new() -> Self { Self::default() }
}

impl BspLayoutSystem {
    fn make_leaf(&mut self, window: Option<WindowId>) -> BspNodeId {
        self.nodes.insert(Node {
            parent: None,
            kind: NodeKind::Leaf { window, fullscreen: false },
        })
    }

    fn descend_to_leaf(&self, mut node: BspNodeId) -> BspNodeId {
        while let Some(n) = self.nodes.get(node) {
            match &n.kind {
                NodeKind::Leaf { .. } => return node,
                NodeKind::Split { first, .. } => {
                    node = *first;
                }
            }
        }
        node
    }

    fn collect_windows_under(&self, node: BspNodeId, out: &mut Vec<WindowId>) {
        let Some(n) = self.nodes.get(node) else {
            return;
        };
        match &n.kind {
            NodeKind::Leaf { window, .. } => {
                if let Some(w) = window {
                    out.push(*w);
                }
            }
            NodeKind::Split { first, second, .. } => {
                self.collect_windows_under(*first, out);
                self.collect_windows_under(*second, out);
            }
        }
    }

    fn find_layout_root(&self, mut node: BspNodeId) -> BspNodeId {
        while let Some(n) = self.nodes.get(node) {
            if let Some(p) = n.parent {
                node = p;
            } else {
                break;
            }
        }
        node
    }

    fn belongs_to_layout(&self, layout: LayoutState, node: BspNodeId) -> bool {
        if self.nodes.get(node).is_none() {
            return false;
        }
        self.find_layout_root(node) == layout.root
    }

    fn cleanup_after_removal(&mut self, node: BspNodeId) -> BspNodeId {
        let Some(parent_id) = self.nodes[node].parent else {
            return node;
        };
        let (first, second, _orientation, _ratio) = match self.nodes[parent_id].kind.clone() {
            NodeKind::Split {
                first,
                second,
                orientation,
                ratio,
            } => (first, second, orientation, ratio),
            NodeKind::Leaf { .. } => return parent_id,
        };
        let sibling = if node == first { second } else { first };
        let sibling_kind = self.nodes[sibling].kind.clone();
        self.nodes[parent_id].kind = sibling_kind.clone();
        match sibling_kind {
            NodeKind::Split {
                first: s_first,
                second: s_second,
                ..
            } => {
                self.nodes[s_first].parent = Some(parent_id);
                self.nodes[s_second].parent = Some(parent_id);
            }
            NodeKind::Leaf { window, .. } => {
                if let Some(w) = window {
                    self.window_to_node.insert(w, parent_id);
                }
            }
        }

        self.nodes.remove(node);
        self.nodes.remove(sibling);
        parent_id
    }

    fn descend_to_edge_leaf(&self, mut node: BspNodeId, direction: Direction) -> BspNodeId {
        loop {
            let Some(n) = self.nodes.get(node) else {
                return node;
            };
            match &n.kind {
                NodeKind::Leaf { .. } => return node,
                NodeKind::Split { orientation, first, second, .. } => {
                    let go_second = match (orientation, direction) {
                        (Orientation::Horizontal, Direction::Left) => true, // rightmost of left subtree
                        (Orientation::Horizontal, Direction::Right) => false, // leftmost of right subtree
                        (Orientation::Vertical, Direction::Up) => true, // bottommost of upper subtree
                        (Orientation::Vertical, Direction::Down) => false, // topmost of lower subtree
                        _ => false, // orientation not aligned with direction: descend first child deterministically
                    };
                    node = if go_second { *second } else { *first };
                }
            }
        }
    }

    fn find_neighbor_leaf(&self, from_leaf: BspNodeId, direction: Direction) -> Option<BspNodeId> {
        let mut child = from_leaf;
        let mut parent_opt = self.nodes[child].parent;
        while let Some(parent) = parent_opt {
            let Some(n) = self.nodes.get(parent) else {
                break;
            };
            if let NodeKind::Split { orientation, first, second, .. } = n.kind {
                if orientation == direction.orientation() {
                    let is_first = child == first;
                    let can_move = match direction {
                        Direction::Left | Direction::Up => !is_first, // must be second child to move toward first
                        Direction::Right | Direction::Down => is_first, // must be first child to move toward second
                    };
                    if can_move {
                        let target_subtree = match direction {
                            Direction::Left | Direction::Up => first,
                            Direction::Right | Direction::Down => second,
                        };
                        let leaf = self.descend_to_edge_leaf(target_subtree, direction);
                        return Some(leaf);
                    }
                }
            }
            child = parent;
            parent_opt = self.nodes.get(child).and_then(|n| n.parent);
        }
        None
    }

    fn insert_window_at_selection(&mut self, state: &mut LayoutState, wid: WindowId) {
        let sel = state.selection;
        match &mut self.nodes[sel].kind {
            NodeKind::Leaf { window, fullscreen } => {
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
                    self.nodes[sel].kind = NodeKind::Split {
                        orientation: Orientation::Horizontal,
                        ratio: 0.5,
                        first: left,
                        second: right,
                    };
                    self.nodes[left].parent = Some(sel);
                    self.nodes[right].parent = Some(sel);
                    state.selection = right;
                }
            }
            NodeKind::Split { .. } => {
                let leaf = self.descend_to_leaf(sel);
                state.selection = leaf;
                self.insert_window_at_selection(state, wid);
            }
        }
    }

    fn remove_window_internal(&mut self, layout: crate::layout_engine::LayoutId, wid: WindowId) {
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                if !self.belongs_to_layout(state, node_id) {
                    return;
                }
            }

            match &mut self.nodes[node_id].kind {
                NodeKind::Leaf { window, .. } => {
                    *window = None;
                }
                NodeKind::Split { .. } => {}
            }
            self.window_to_node.remove(&wid);
            let fallback = self.cleanup_after_removal(node_id);

            let sel_snapshot = self.layouts.get(layout).map(|s| s.selection);
            let needs_reset = sel_snapshot.and_then(|sel| self.nodes.get(sel)).is_none();
            let new_sel = if needs_reset {
                self.descend_to_leaf(fallback)
            } else {
                self.descend_to_leaf(sel_snapshot.unwrap())
            };
            if let Some(state) = self.layouts.get_mut(layout) {
                state.selection = new_sel;
            }
        }
    }

    fn calculate_layout_recursive(
        &self,
        node: BspNodeId,
        rect: CGRect,
        gaps: &crate::common::config::GapSettings,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        match &self.nodes[node].kind {
            NodeKind::Leaf { window, fullscreen } => {
                if let Some(w) = window {
                    let r = if *fullscreen { rect } else { rect };
                    out.push((*w, r));
                }
            }
            NodeKind::Split {
                orientation,
                ratio,
                first,
                second,
            } => match orientation {
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
                    self.calculate_layout_recursive(*first, r1, gaps, out);
                    self.calculate_layout_recursive(*second, r2, gaps, out);
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
                    self.calculate_layout_recursive(*first, r1, gaps, out);
                    self.calculate_layout_recursive(*second, r2, gaps, out);
                }
            },
        }
    }

    fn apply_outer_gaps(screen: CGRect, gaps: &crate::common::config::GapSettings) -> CGRect {
        let x = screen.origin.x + gaps.outer.left;
        let y = screen.origin.y + gaps.outer.top;
        let w = (screen.size.width - gaps.outer.left - gaps.outer.right).max(0.0);
        let h = (screen.size.height - gaps.outer.top - gaps.outer.bottom).max(0.0);
        CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
    }

    fn selection_window(&self, state: &LayoutState) -> Option<WindowId> {
        if let Some(node) = self.nodes.get(state.selection) {
            match &node.kind {
                NodeKind::Leaf { window, .. } => *window,
                _ => None,
            }
        } else {
            None
        }
    }
}

impl LayoutSystem for BspLayoutSystem {
    type LayoutId = crate::layout_engine::LayoutId;

    fn create_layout(&mut self) -> Self::LayoutId {
        let leaf = self.make_leaf(None);
        let state = LayoutState { root: leaf, selection: leaf };
        self.layouts.insert(state)
    }

    /// shallow
    fn clone_layout(&mut self, layout: Self::LayoutId) -> Self::LayoutId {
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

    fn remove_layout(&mut self, layout: Self::LayoutId) {
        if let Some(state) = self.layouts.remove(layout) {
            // Remove windows mappings
            let mut windows = Vec::new();
            self.collect_windows_under(state.root, &mut windows);
            for w in windows {
                self.window_to_node.remove(&w);
            }
        }
    }

    fn draw_tree(&self, layout: Self::LayoutId) -> String {
        fn write_node(this: &BspLayoutSystem, node: BspNodeId, out: &mut String, indent: usize) {
            for _ in 0..indent {
                out.push_str("  ");
            }
            let Some(n) = this.nodes.get(node) else {
                return;
            };
            match &n.kind {
                NodeKind::Leaf { window, .. } => {
                    out.push_str(&format!("Leaf {:?}\n", window));
                }
                NodeKind::Split {
                    orientation,
                    ratio,
                    first,
                    second,
                } => {
                    out.push_str(&format!("Split {:?} {:.2}\n", orientation, ratio));
                    write_node(this, *first, out, indent + 1);
                    write_node(this, *second, out, indent + 1);
                }
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
        layout: Self::LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
    ) -> Vec<(WindowId, CGRect)> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            let rect = Self::apply_outer_gaps(screen, gaps);
            self.calculate_layout_recursive(state.root, rect, gaps, &mut out);
        }
        out
    }

    fn selected_window(&self, layout: Self::LayoutId) -> Option<WindowId> {
        self.layouts.get(layout).and_then(|s| self.selection_window(s))
    }

    fn visible_windows_in_layout(&self, layout: Self::LayoutId) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            self.collect_windows_under(state.root, &mut out);
        }
        out
    }

    fn visible_windows_under_selection(&self, layout: Self::LayoutId) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            if self.nodes.get(state.selection).is_some() {
                let leaf = self.descend_to_leaf(state.selection);
                self.collect_windows_under(leaf, &mut out);
            }
        }
        out
    }

    fn ascend_selection(&mut self, layout: Self::LayoutId) -> bool {
        if let Some(sel) = self.layouts.get(layout).map(|s| s.selection) {
            if self.nodes.get(sel).is_none() {
                return false;
            }
            let parent_opt = self.nodes[sel].parent;
            if let Some(parent) = parent_opt {
                let new_sel = self.descend_to_leaf(parent);
                if let Some(state) = self.layouts.get_mut(layout) {
                    state.selection = new_sel;
                }
                return true;
            }
        }
        false
    }

    fn descend_selection(&mut self, layout: Self::LayoutId) -> bool {
        let sel_snapshot = self.layouts.get(layout).map(|s| s.selection);
        if let Some(sel) = sel_snapshot {
            let new_sel = self.descend_to_leaf(sel);
            if new_sel != sel {
                if let Some(state) = self.layouts.get_mut(layout) {
                    state.selection = new_sel;
                }
                return true;
            }
        }
        false
    }

    fn move_focus(
        &mut self,
        layout: Self::LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let raise_windows = self.visible_windows_in_layout(layout);
        if raise_windows.is_empty() {
            return (None, vec![]);
        }
        let sel_snapshot = self.layouts.get(layout).map(|s| s.selection);
        let Some(current_sel) = sel_snapshot else {
            return (None, vec![]);
        };
        let current_leaf = self.descend_to_leaf(current_sel);
        let Some(next_leaf) = self.find_neighbor_leaf(current_leaf, direction) else {
            return (None, vec![]);
        };
        if let Some(state) = self.layouts.get_mut(layout) {
            state.selection = next_leaf;
        }
        let focus = match &self.nodes[next_leaf].kind {
            NodeKind::Leaf { window, .. } => *window,
            _ => None,
        };
        (focus, raise_windows)
    }

    fn add_window_after_selection(&mut self, layout: Self::LayoutId, wid: WindowId) {
        if let Some(state_sel) = self.layouts.get(layout).copied() {
            let mut tmp = state_sel;
            self.insert_window_at_selection(&mut tmp, wid);
            if let Some(state_mut) = self.layouts.get_mut(layout) {
                *state_mut = tmp;
            }
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        if let Some(&node_id) = self.window_to_node.get(&wid) {
            if self.nodes.get(node_id).is_none() {
                self.window_to_node.remove(&wid);
                return;
            }
            let root = self.find_layout_root(node_id);
            let layout = self
                .layouts
                .iter()
                .find_map(|(lid, st)| if st.root == root { Some(lid) } else { None });
            if let Some(l) = layout {
                self.remove_window_internal(l, wid);
            } else {
                self.window_to_node.remove(&wid);
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

    fn set_windows_for_app(&mut self, layout: Self::LayoutId, pid: pid_t, desired: Vec<WindowId>) {
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

    fn has_windows_for_app(&self, layout: Self::LayoutId, pid: pid_t) -> bool {
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut under = Vec::new();
            self.collect_windows_under(state.root, &mut under);
            under.into_iter().any(|w| w.pid == pid)
        } else {
            false
        }
    }

    fn contains_window(&self, layout: Self::LayoutId, wid: WindowId) -> bool {
        if let Some(&node) = self.window_to_node.get(&wid) {
            if let Some(state) = self.layouts.get(layout).copied() {
                return self.belongs_to_layout(state, node);
            }
        }
        false
    }

    fn select_window(&mut self, layout: Self::LayoutId, wid: WindowId) -> bool {
        if let Some(&node) = self.window_to_node.get(&wid) {
            if self.nodes.get(node).is_none() {
                self.window_to_node.remove(&wid);
                return false;
            }
            if let Some(state) = self.layouts.get(layout).copied() {
                let belongs = self.belongs_to_layout(state, node);
                if let Some(state_mut) = self.layouts.get_mut(layout) {
                    if belongs {
                        state_mut.selection = node;
                        return true;
                    }
                }
            }
        }
        false
    }

    fn on_window_resized(
        &mut self,
        layout: Self::LayoutId,
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
                if let NodeKind::Leaf { window: _, fullscreen } = &mut self.nodes[node].kind {
                    if new_frame == screen {
                        *fullscreen = true;
                    } else if old_frame == screen {
                        *fullscreen = false;
                    }
                }
            }
        }
    }

    fn move_selection(&mut self, layout: Self::LayoutId, direction: Direction) -> bool {
        let sel_snapshot = self.layouts.get(layout).map(|s| s.selection);
        let Some(sel) = sel_snapshot else {
            return false;
        };
        let sel_leaf = self.descend_to_leaf(sel);
        let Some(neighbor_leaf) = self.find_neighbor_leaf(sel_leaf, direction) else {
            return false;
        };
        let (mut a_window, mut b_window) = (None, None);
        if let NodeKind::Leaf { window, .. } = &mut self.nodes[sel_leaf].kind {
            a_window = *window;
        }
        if let NodeKind::Leaf { window, .. } = &mut self.nodes[neighbor_leaf].kind {
            b_window = *window;
        }
        if a_window.is_none() && b_window.is_none() {
            return false;
        }
        // Swap mapping
        if let NodeKind::Leaf { window, .. } = &mut self.nodes[sel_leaf].kind {
            *window = b_window;
        }
        if let NodeKind::Leaf { window, .. } = &mut self.nodes[neighbor_leaf].kind {
            *window = a_window;
        }
        if let Some(w) = a_window {
            self.window_to_node.insert(w, neighbor_leaf);
        }
        if let Some(w) = b_window {
            self.window_to_node.insert(w, sel_leaf);
        }
        // Keep selection on destination leaf
        if let Some(state) = self.layouts.get_mut(layout) {
            state.selection = neighbor_leaf;
        }
        true
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: Self::LayoutId,
        to_layout: Self::LayoutId,
    ) {
        let sel = self.selected_window(from_layout);
        if let Some(w) = sel {
            self.remove_window_internal(from_layout, w);
            self.add_window_after_selection(to_layout, w);
        }
    }

    fn split_selection(&mut self, layout: Self::LayoutId, kind: LayoutKind) {
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

        let target = self.descend_to_leaf(state.selection);
        match self.nodes[target].kind.clone() {
            NodeKind::Leaf { window, .. } => {
                let left = self.make_leaf(window);
                let right = self.make_leaf(None);
                if let Some(w) = window {
                    self.window_to_node.insert(w, left);
                }
                self.nodes[target].kind = NodeKind::Split {
                    orientation,
                    ratio: 0.5,
                    first: left,
                    second: right,
                };
                self.nodes[left].parent = Some(target);
                self.nodes[right].parent = Some(target);
                if let Some(st) = self.layouts.get_mut(layout) {
                    st.selection = right;
                }
            }
            NodeKind::Split { .. } => {}
        }
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId> {
        let sel_snapshot = self.layouts.get(layout).map(|s| s.selection);
        if let Some(sel) = sel_snapshot {
            let sel_leaf = self.descend_to_leaf(sel);
            if let NodeKind::Leaf { window: Some(w), fullscreen } = &mut self.nodes[sel_leaf].kind {
                *fullscreen = !*fullscreen;
                return vec![*w];
            }
        }
        vec![]
    }

    fn join_selection_with_direction(&mut self, _layout: Self::LayoutId, _direction: Direction) {}

    fn apply_stacking_to_parent_of_selection(&mut self, _layout: Self::LayoutId) -> Vec<WindowId> {
        vec![]
    }

    fn unstack_parent_of_selection(&mut self, _layout: Self::LayoutId) -> Vec<WindowId> { vec![] }

    fn unjoin_selection(&mut self, _layout: Self::LayoutId) {}

    fn resize_selection_by(&mut self, layout: Self::LayoutId, amount: f64) {
        let sel_snapshot = self.layouts.get(layout).map(|s| s.selection);
        let Some(mut node) = sel_snapshot else {
            return;
        };

        while let Some(parent) = self.nodes[node].parent {
            if let NodeKind::Split {
                orientation: _,
                ratio,
                first,
                second: _,
            } = &mut self.nodes[parent].kind
            {
                let is_first = node == *first;
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

    fn rebalance(&mut self, layout: Self::LayoutId) {
        if let Some(state) = self.layouts.get(layout).copied() {
            let mut stack = vec![state.root];
            while let Some(n) = stack.pop() {
                match &mut self.nodes[n].kind {
                    NodeKind::Split { ratio, first, second, .. } => {
                        *ratio = (*ratio).clamp(0.05, 0.95);
                        stack.push(*first);
                        stack.push(*second);
                    }
                    NodeKind::Leaf { .. } => {}
                }
            }
        }
    }
}
