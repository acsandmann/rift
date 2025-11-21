use ascii_tree::Tree as AsciiTree;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{BTreeExt, BTreeMap, HashSet};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId};
use crate::model::selection::{Selection, TreeEvent};
use crate::model::tree::{self, NodeId, NodeMap, OwnedNode, Tree};
use crate::sys::geometry::Round;

const MAX_ROWS_PER_COLUMN: usize = 3;

#[derive(Default, Clone, Copy, Serialize, Deserialize)]
struct ScrollLayoutState {
    first_visible_column: usize,
}

#[derive(Clone, Copy)]
enum ScrollRevealEdge {
    Left,
    Right,
}

#[derive(Serialize, Deserialize)]
pub struct ScrollLayoutSystem {
    tree: Tree<ScrollComponents>,
    layout_roots: slotmap::SlotMap<LayoutId, OwnedNode>,
    layout_state: slotmap::SecondaryMap<LayoutId, ScrollLayoutState>,
    max_visible_columns: usize,
    #[serde(default)]
    infinite_loop: bool,
}

impl ScrollLayoutSystem {
    pub fn new(max_visible_columns: usize, infinite_loop: bool) -> Self {
        let clamped = max_visible_columns.clamp(1, 5);
        Self {
            tree: Tree::with_observer(ScrollComponents::default()),
            layout_roots: Default::default(),
            layout_state: Default::default(),
            max_visible_columns: clamped,
            infinite_loop,
        }
    }

    pub fn apply_settings(&mut self, settings: &crate::common::config::ScrollSettings) {
        self.max_visible_columns = settings.visible_columns.clamp(1, 5);
        self.infinite_loop = settings.infinite_loop;
    }

    fn root(&self, layout: LayoutId) -> NodeId { self.layout_roots[layout].id() }

    fn map(&self) -> &NodeMap { &self.tree.map }

    fn selection(&self, layout: LayoutId) -> NodeId {
        self.tree.data.selection.current_selection(self.root(layout))
    }

    fn ensure_state(&mut self, layout: LayoutId) -> &mut ScrollLayoutState {
        if !self.layout_state.contains_key(layout) {
            self.layout_state.insert(layout, ScrollLayoutState::default());
        }
        self.layout_state.get_mut(layout).expect("layout state must exist")
    }

    fn columns(&self, layout: LayoutId) -> Vec<NodeId> {
        self.root(layout).children(self.map()).collect()
    }

    fn ensure_column(&mut self, layout: LayoutId) -> NodeId {
        let root = self.root(layout);
        if let Some(column) = root.first_child(self.map()) {
            column
        } else {
            self.tree.mk_node().push_back(root)
        }
    }

    fn insert_column_after(&mut self, layout: LayoutId, anchor: NodeId) -> NodeId {
        let root = self.root(layout);
        if anchor == root {
            return self.tree.mk_node().push_back(root);
        }
        self.tree.mk_node().insert_after(anchor)
    }

    fn insert_column_before(&mut self, layout: LayoutId, anchor: NodeId) -> NodeId {
        let root = self.root(layout);
        if anchor == root {
            return self.tree.mk_node().push_back(root);
        }
        self.tree.mk_node().insert_before(anchor)
    }

    fn column_of(&self, layout: LayoutId, node: NodeId) -> Option<NodeId> {
        let map = self.map();
        let root = self.root(layout);
        node.ancestors(map).find(|ancestor| ancestor.parent(map) == Some(root))
    }

    fn column_index(&self, layout: LayoutId, column: NodeId) -> Option<usize> {
        self.columns(layout).iter().position(|&c| c == column)
    }

    fn column_row_count(&self, column: NodeId) -> usize { column.children(self.map()).count() }

    fn normalized_first_visible(
        &self,
        state_first: usize,
        total: usize,
        visible_cap: usize,
    ) -> usize {
        if total == 0 || visible_cap >= total {
            0
        } else if self.infinite_loop {
            state_first % total
        } else {
            let max_start = total.saturating_sub(visible_cap);
            state_first.min(max_start)
        }
    }

    fn ensure_selection_visible(&mut self, layout: LayoutId) {
        self.ensure_selection_visible_with(layout, ScrollRevealEdge::Left);
    }

    fn ensure_selection_visible_with(&mut self, layout: LayoutId, edge: ScrollRevealEdge) {
        let total_columns = self.columns(layout);
        if total_columns.is_empty() {
            if let Some(state) = self.layout_state.get_mut(layout) {
                state.first_visible_column = 0;
            }
            return;
        }

        let visible_cap = self.max_visible_columns.min(total_columns.len());
        if visible_cap >= total_columns.len() {
            if let Some(state) = self.layout_state.get_mut(layout) {
                state.first_visible_column = 0;
            }
            return;
        }

        let selection = self.selection(layout);
        let target_idx =
            self.column_of(layout, selection).and_then(|col| self.column_index(layout, col));
        let Some(target_idx) = target_idx else { return };

        let len = total_columns.len();
        let current_state = self
            .layout_state
            .get(layout)
            .map(|state| state.first_visible_column)
            .unwrap_or(0);
        let current_start = self.normalized_first_visible(current_state, len, visible_cap);

        let is_visible = if self.infinite_loop {
            let visible: HashSet<usize> =
                (0..visible_cap).map(|offset| (current_start + offset) % len).collect();
            visible.contains(&target_idx)
        } else {
            target_idx >= current_start && target_idx < current_start + visible_cap
        };

        if is_visible {
            return;
        }

        let new_first = if self.infinite_loop {
            match edge {
                ScrollRevealEdge::Left => target_idx,
                ScrollRevealEdge::Right => {
                    let trailing = visible_cap.saturating_sub(1);
                    if target_idx >= trailing {
                        target_idx - trailing
                    } else {
                        len - (trailing - target_idx)
                    }
                }
            }
        } else {
            let max_start = len - visible_cap;
            match edge {
                ScrollRevealEdge::Left => target_idx.min(max_start),
                ScrollRevealEdge::Right => {
                    let trailing = visible_cap.saturating_sub(1);
                    let start = target_idx.saturating_sub(trailing);
                    start.min(max_start)
                }
            }
        };
        self.ensure_state(layout).first_visible_column = new_first;
    }

    fn window_at(&self, node: NodeId) -> Option<WindowId> { self.tree.data.window.at(node) }

    fn visible_window_ids(&self, layout: LayoutId) -> Vec<WindowId> {
        let mut result = Vec::new();
        for node in self.root(layout).traverse_preorder(self.map()) {
            if let Some(wid) = self.window_at(node) {
                result.push(wid);
            }
        }
        result
    }

    fn visible_under_node(&self, node: NodeId) -> Vec<WindowId> {
        let mut result = Vec::new();
        for child in node.traverse_preorder(self.map()) {
            if let Some(wid) = self.window_at(child) {
                result.push(wid);
            }
        }
        result
    }

    fn focus_window_in_column(&mut self, layout: LayoutId, column_idx: usize) -> Option<NodeId> {
        let columns = self.columns(layout);
        if columns.is_empty() {
            return None;
        }
        let idx = if self.infinite_loop {
            column_idx % columns.len()
        } else if column_idx < columns.len() {
            column_idx
        } else {
            return None;
        };
        let column = columns[idx];
        if let Some(first_row) = column.first_child(self.map()) {
            self.tree.data.selection.select(&self.tree.map, first_row);
            return Some(first_row);
        }
        None
    }

    fn ascii_tree(&self, node: NodeId) -> AsciiTree {
        let mut desc = format!("{:?}", node);
        if let Some(wid) = self.window_at(node) {
            desc = format!("{desc} {:?}", wid);
        }
        let children: Vec<_> = node.children(self.map()).map(|c| self.ascii_tree(c)).collect();
        if children.is_empty() {
            AsciiTree::Leaf(vec![desc])
        } else {
            AsciiTree::Node(desc, children)
        }
    }

    fn layout_columns(
        &self,
        layout: LayoutId,
        screen: CGRect,
        tiling_area: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> Vec<(WindowId, CGRect)> {
        let columns = self.columns(layout);
        if columns.is_empty() {
            return Vec::new();
        }
        let total = columns.len();
        let visible_cap = self.max_visible_columns.min(total).max(1);
        let state = self.layout_state.get(layout).copied().unwrap_or_default();
        let first = self.normalized_first_visible(state.first_visible_column, total, visible_cap);
        let visible_set: HashSet<usize> = if self.infinite_loop {
            (0..visible_cap).map(|offset| (first + offset) % total).collect()
        } else {
            HashSet::default()
        };

        let mut result = Vec::new();
        let horiz_gap = gaps.inner.horizontal;
        let total_gap = (visible_cap.saturating_sub(1)) as f64 * horiz_gap;
        let usable_width = if horiz_gap == 0.0 {
            tiling_area.size.width
        } else {
            (tiling_area.size.width - total_gap).max(0.0)
        };
        let column_width = usable_width / visible_cap as f64;
        let mut x = tiling_area.origin.x;
        for offset in 0..visible_cap {
            let idx = if self.infinite_loop {
                (first + offset) % total
            } else {
                first + offset
            };
            if let Some(column) = columns.get(idx) {
                let rect = CGRect {
                    origin: CGPoint { x, y: tiling_area.origin.y },
                    size: CGSize {
                        width: column_width,
                        height: tiling_area.size.height,
                    },
                }
                .round();
                self.layout_column(*column, rect, gaps.inner.vertical, &mut result);
            }
            x += column_width;
            if offset < visible_cap - 1 {
                x += horiz_gap;
            }
        }

        if total > visible_cap {
            for (idx, column) in columns.iter().enumerate() {
                let is_visible = if self.infinite_loop {
                    visible_set.contains(&idx)
                } else {
                    idx >= first && idx < first + visible_cap
                };
                if !is_visible {
                    let hide_rect =
                        Self::hidden_column_rect(screen, column_width, tiling_area.size.height);
                    self.layout_column(*column, hide_rect, gaps.inner.vertical, &mut result);
                }
            }
        }

        result
    }

    fn layout_column(
        &self,
        column: NodeId,
        rect: CGRect,
        vertical_gap: f64,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        let rows: Vec<_> = column.children(self.map()).collect();
        if rows.is_empty() {
            return;
        }
        let count = rows.len();
        let total_gap = (count.saturating_sub(1)) as f64 * vertical_gap;
        let usable_height = if vertical_gap == 0.0 {
            rect.size.height
        } else {
            (rect.size.height - total_gap).max(0.0)
        };
        let row_height = if count == 0 {
            rect.size.height
        } else {
            usable_height / count as f64
        };
        let mut y = rect.origin.y;
        for (idx, row) in rows.iter().enumerate() {
            if let Some(wid) = self.window_at(*row) {
                let frame = CGRect {
                    origin: CGPoint { x: rect.origin.x, y },
                    size: CGSize {
                        width: rect.size.width,
                        height: row_height,
                    },
                }
                .round();
                out.push((wid, frame));
            }
            y += row_height;
            if idx < count - 1 {
                y += vertical_gap;
            }
        }
    }

    fn ensure_column_constraints(&mut self, layout: LayoutId, column: NodeId) {
        if self.column_row_count(column) > 0 {
            return;
        }
        let root = self.root(layout);
        column.detach(&mut self.tree).remove();
        if root.first_child(self.map()).is_none() {
            self.ensure_column(layout);
        }
    }

    fn hidden_column_rect(screen: CGRect, width: f64, height: f64) -> CGRect {
        let bottom_right = CGPoint::new(screen.max().x, screen.max().y);
        let origin = CGPoint::new(bottom_right.x - 2.0, bottom_right.y - 2.0);
        CGRect::new(origin, CGSize::new(width.max(1.0), height))
    }
}

impl LayoutSystem for ScrollLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let root = OwnedNode::new_root_in(&mut self.tree, "scroll_layout_root");
        let id = self.layout_roots.insert(root);
        self.layout_state.insert(id, ScrollLayoutState::default());
        id
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let source_root = self.layout_roots[layout].id();
        let cloned = source_root.deep_copy(&mut self.tree).make_root("scroll_layout_root");
        let cloned_root = cloned.id();
        let dest_layout = self.layout_roots.insert(cloned);
        for (src, dest) in std::iter::zip(
            source_root.traverse_preorder(&self.tree.map),
            cloned_root.traverse_preorder(&self.tree.map),
        ) {
            self.tree.data.dispatch_event(&self.tree.map, TreeEvent::Copied {
                src,
                dest,
                dest_layout,
            });
        }
        if let Some(state) = self.layout_state.get(layout).copied() {
            self.layout_state.insert(dest_layout, state);
        }
        dest_layout
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        if let Some(mut root) = self.layout_roots.remove(layout) {
            root.remove(&mut self.tree);
        }
        self.layout_state.remove(layout);
    }

    fn draw_tree(&self, layout: LayoutId) -> String {
        let mut out = String::new();
        let tree = self.ascii_tree(self.root(layout));
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
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
        let tiling_area = compute_tiling_area(screen, gaps);
        self.layout_columns(layout, screen, tiling_area, gaps)
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.window_at(self.selection(layout))
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.visible_window_ids(layout)
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        let selection = self.selection(layout);
        self.visible_under_node(selection)
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool {
        if let Some(parent) = self.selection(layout).parent(self.map()) {
            self.tree.data.selection.select(&self.tree.map, parent);
            self.ensure_selection_visible(layout);
            true
        } else {
            false
        }
    }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        let selection = self.selection(layout);
        if let Some(child) = selection.first_child(self.map()) {
            self.tree.data.selection.select(&self.tree.map, child);
            self.ensure_selection_visible(layout);
            true
        } else {
            false
        }
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let current = self.selection(layout);
        let target = match direction {
            Direction::Up => current.prev_sibling(self.map()),
            Direction::Down => current.next_sibling(self.map()),
            Direction::Left | Direction::Right => {
                let columns = self.columns(layout);
                if columns.is_empty() {
                    None
                } else if let Some(column) = self.column_of(layout, current) {
                    let idx = self.column_index(layout, column).unwrap_or(0);
                    let len = columns.len();
                    let next_idx = match direction {
                        Direction::Right => {
                            if idx + 1 < len {
                                Some(idx + 1)
                            } else if self.infinite_loop {
                                Some(0)
                            } else {
                                None
                            }
                        }
                        Direction::Left => {
                            if idx > 0 {
                                Some(idx - 1)
                            } else if self.infinite_loop {
                                Some(len - 1)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    next_idx.and_then(|i| self.focus_window_in_column(layout, i))
                } else {
                    None
                }
            }
        };

        if let Some(target) = target {
            self.tree.data.selection.select(&self.tree.map, target);
            match direction {
                Direction::Left => {
                    self.ensure_selection_visible_with(layout, ScrollRevealEdge::Left);
                }
                Direction::Right => {
                    self.ensure_selection_visible_with(layout, ScrollRevealEdge::Right);
                }
                _ => self.ensure_selection_visible(layout),
            }
            let wid = self.window_at(target);
            let raise = self.visible_windows_under_selection(layout);
            (wid, raise)
        } else {
            (self.window_at(current), Vec::new())
        }
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        let current = self.selection(layout);
        match direction {
            Direction::Up => current.prev_sibling(self.map()).and_then(|n| self.window_at(n)),
            Direction::Down => current.next_sibling(self.map()).and_then(|n| self.window_at(n)),
            Direction::Left | Direction::Right => {
                let columns = self.columns(layout);
                let column = self.column_of(layout, current)?;
                let idx = self.column_index(layout, column)?;
                if columns.is_empty() {
                    return None;
                }
                let len = columns.len();
                let next_idx = match direction {
                    Direction::Right => {
                        if idx + 1 < len {
                            Some(idx + 1)
                        } else if self.infinite_loop {
                            Some(0)
                        } else {
                            None
                        }
                    }
                    Direction::Left => {
                        if idx > 0 {
                            Some(idx - 1)
                        } else if self.infinite_loop {
                            Some(len - 1)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let target_column = next_idx.and_then(|i| columns.get(i)).copied();
                target_column
                    .and_then(|col| col.first_child(self.map()))
                    .and_then(|node| self.window_at(node))
            }
        }
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let root = self.root(layout);
        let selection = self.selection(layout);
        let reference_column = if self.window_at(selection).is_some() {
            self.column_of(layout, selection)
        } else if selection.parent(self.map()) == Some(root) {
            Some(selection)
        } else {
            None
        };

        let new_column = match reference_column {
            Some(column) => self.tree.mk_node().insert_after(column),
            None => match root.last_child(self.map()) {
                Some(last) => self.tree.mk_node().insert_after(last),
                None => self.tree.mk_node().push_back(root),
            },
        };

        let node = self.tree.mk_node().push_back(new_column);
        self.tree.data.window.set_window(layout, node, wid);
        self.tree.data.selection.select(&self.tree.map, node);
        self.ensure_selection_visible(layout);
    }

    fn remove_window(&mut self, wid: WindowId) {
        let nodes: Vec<_> = self.tree.data.window.take_nodes_for(wid).collect();
        for (layout, node) in nodes {
            let column = self.column_of(layout, node);
            node.detach(&mut self.tree).remove();
            if let Some(column) = column {
                self.ensure_column_constraints(layout, column);
            }
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        let nodes: Vec<_> = self
            .tree
            .data
            .window
            .take_nodes_for_app(pid)
            .map(|(_, layout, node)| (layout, node))
            .collect();
        for (layout, node) in nodes {
            let column = self.column_of(layout, node);
            node.detach(&mut self.tree).remove();
            if let Some(col) = column {
                self.ensure_column_constraints(layout, col);
            }
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, mut desired: Vec<WindowId>) {
        let root = self.root(layout);
        let mut current = root
            .traverse_postorder(self.map())
            .filter_map(|node| self.window_at(node).map(|wid| (wid, node)))
            .filter(|(wid, _)| wid.pid == pid)
            .collect::<Vec<_>>();
        desired.sort_unstable();
        current.sort_unstable();
        debug_assert!(desired.iter().all(|wid| wid.pid == pid));
        let mut desired = desired.into_iter().peekable();
        let mut current = current.into_iter().peekable();
        loop {
            match (desired.peek(), current.peek()) {
                (Some(des), Some((cur, _))) if des == cur => {
                    desired.next();
                    current.next();
                }
                (Some(des), None) => {
                    self.add_window_after_selection(layout, *des);
                    desired.next();
                }
                (Some(des), Some((cur, _))) if des < cur => {
                    self.add_window_after_selection(layout, *des);
                    desired.next();
                }
                (_, Some((_, node))) => {
                    let column = self.column_of(layout, *node);
                    node.detach(&mut self.tree).remove();
                    if let Some(col) = column {
                        self.ensure_column_constraints(layout, col);
                    }
                    current.next();
                }
                (None, None) => break,
            }
        }
    }

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.root(layout)
            .traverse_postorder(self.map())
            .filter_map(|node| self.window_at(node))
            .any(|wid| wid.pid == pid)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.tree.data.window.node_for(layout, wid).is_some()
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        if let Some(node) = self.tree.data.window.node_for(layout, wid) {
            self.tree.data.selection.select(&self.tree.map, node);
            self.ensure_selection_visible(layout);
            true
        } else {
            false
        }
    }

    fn on_window_resized(
        &mut self,
        _layout: LayoutId,
        _wid: WindowId,
        _old_frame: CGRect,
        _new_frame: CGRect,
        _screen: CGRect,
        _gaps: &crate::common::config::GapSettings,
    ) {
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        let node_a = match self.tree.data.window.node_for(layout, a) {
            Some(n) => n,
            None => return false,
        };
        let node_b = match self.tree.data.window.node_for(layout, b) {
            Some(n) => n,
            None => return false,
        };
        if node_a == node_b {
            return true;
        }
        self.tree.data.window.swap_nodes(layout, node_a, node_b);
        true
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let selection = self.selection(layout);
        if self.window_at(selection).is_none() {
            return false;
        }
        match direction {
            Direction::Up => {
                if let Some(prev) = selection.prev_sibling(self.map()) {
                    selection.detach(&mut self.tree).insert_before(prev);
                    self.tree.data.selection.select(&self.tree.map, selection);
                    true
                } else {
                    false
                }
            }
            Direction::Down => {
                if let Some(next) = selection.next_sibling(self.map()) {
                    selection.detach(&mut self.tree).insert_after(next);
                    self.tree.data.selection.select(&self.tree.map, selection);
                    true
                } else {
                    false
                }
            }
            Direction::Left | Direction::Right => {
                let columns = self.columns(layout);
                if columns.is_empty() {
                    return false;
                }
                let column = match self.column_of(layout, selection) {
                    Some(col) => col,
                    None => return false,
                };
                let original_column = column;
                let idx = match self.column_index(layout, column) {
                    Some(i) => i,
                    None => return false,
                };
                let len = columns.len();
                let reveal_edge = if matches!(direction, Direction::Right) {
                    ScrollRevealEdge::Right
                } else {
                    ScrollRevealEdge::Left
                };
                let target_idx = if self.infinite_loop {
                    Some(if matches!(direction, Direction::Right) {
                        (idx + 1) % len
                    } else {
                        (idx + len - 1) % len
                    })
                } else if matches!(direction, Direction::Right) {
                    if idx + 1 < len { Some(idx + 1) } else { None }
                } else if idx > 0 {
                    Some(idx - 1)
                } else {
                    None
                };

                if let Some(target_idx) = target_idx {
                    let target_column = columns[target_idx];
                    if self.column_row_count(target_column) < MAX_ROWS_PER_COLUMN {
                        selection.detach(&mut self.tree).push_back(target_column);
                        self.tree.data.selection.select(&self.tree.map, selection);
                        self.ensure_column_constraints(layout, original_column);
                        self.ensure_selection_visible_with(layout, reveal_edge);
                        true
                    } else if self.column_row_count(column) > 1 {
                        let new_column = if matches!(direction, Direction::Right) {
                            self.insert_column_after(layout, column)
                        } else {
                            self.insert_column_before(layout, column)
                        };
                        selection.detach(&mut self.tree).push_back(new_column);
                        self.tree.data.selection.select(&self.tree.map, selection);
                        self.ensure_column_constraints(layout, original_column);
                        self.ensure_selection_visible_with(layout, reveal_edge);
                        true
                    } else {
                        false
                    }
                } else if self.column_row_count(column) > 1 {
                    let new_column = if matches!(direction, Direction::Right) {
                        self.insert_column_after(layout, column)
                    } else {
                        self.insert_column_before(layout, column)
                    };
                    selection.detach(&mut self.tree).push_back(new_column);
                    self.tree.data.selection.select(&self.tree.map, selection);
                    self.ensure_column_constraints(layout, original_column);
                    self.ensure_selection_visible_with(layout, reveal_edge);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn move_column(&mut self, layout: LayoutId, direction: Direction) -> bool {
        if !matches!(direction, Direction::Left | Direction::Right) {
            return false;
        }

        let selection = self.selection(layout);
        if self.window_at(selection).is_none() {
            return false;
        }

        let columns = self.columns(layout);
        if columns.len() <= 1 {
            return false;
        }

        let column = match self.column_of(layout, selection) {
            Some(col) => col,
            None => return false,
        };
        let idx = match self.column_index(layout, column) {
            Some(i) => i,
            None => return false,
        };
        let len = columns.len();

        let target_idx = match direction {
            Direction::Left => {
                if idx > 0 {
                    Some(idx - 1)
                } else if self.infinite_loop {
                    Some(len - 1)
                } else {
                    None
                }
            }
            Direction::Right => {
                if idx + 1 < len {
                    Some(idx + 1)
                } else if self.infinite_loop {
                    Some(0)
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(target_idx) = target_idx else { return false };
        let target_column = columns[target_idx];

        let detacher = column.detach(&mut self.tree);
        if matches!(direction, Direction::Left) {
            if target_idx < idx {
                detacher.insert_before(target_column);
            } else {
                detacher.insert_after(target_column);
            }
        } else if target_idx > idx {
            detacher.insert_after(target_column);
        } else {
            detacher.insert_before(target_column);
        }

        self.tree.data.selection.select(&self.tree.map, selection);
        let reveal_edge = if matches!(direction, Direction::Right) {
            ScrollRevealEdge::Right
        } else {
            ScrollRevealEdge::Left
        };
        self.ensure_selection_visible_with(layout, reveal_edge);
        true
    }

    fn consume_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let selection = self.selection(layout);
        if self.window_at(selection).is_none()
            || !matches!(direction, Direction::Left | Direction::Right)
        {
            return false;
        }

        let Some(column) = self.column_of(layout, selection) else { return false };
        let columns = self.columns(layout);
        let Some(idx) = self.column_index(layout, column) else { return false };

        let target_idx = match direction {
            Direction::Left if idx > 0 => Some(idx - 1),
            Direction::Right if idx + 1 < columns.len() => Some(idx + 1),
            _ => None,
        };
        let Some(target_idx) = target_idx else { return false };

        let target_column = columns[target_idx];
        if self.column_row_count(target_column) >= MAX_ROWS_PER_COLUMN {
            return false;
        }

        selection.detach(&mut self.tree).push_back(target_column);
        self.tree.data.selection.select(&self.tree.map, selection);
        self.ensure_column_constraints(layout, column);

        let edge = if matches!(direction, Direction::Right) {
            ScrollRevealEdge::Right
        } else {
            ScrollRevealEdge::Left
        };
        self.ensure_selection_visible_with(layout, edge);
        true
    }

    fn expel_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let selection = self.selection(layout);
        if self.window_at(selection).is_none()
            || !matches!(direction, Direction::Left | Direction::Right)
        {
            return false;
        }

        let Some(column) = self.column_of(layout, selection) else { return false };
        if self.column_row_count(column) <= 1 {
            return false;
        }

        let new_column = if matches!(direction, Direction::Right) {
            self.insert_column_after(layout, column)
        } else {
            self.insert_column_before(layout, column)
        };

        selection.detach(&mut self.tree).push_back(new_column);
        self.tree.data.selection.select(&self.tree.map, selection);

        let edge = if matches!(direction, Direction::Right) {
            ScrollRevealEdge::Right
        } else {
            ScrollRevealEdge::Left
        };
        self.ensure_selection_visible_with(layout, edge);
        true
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        let from_sel = self.selection(from_layout);
        let to_sel = self.selection(to_layout);
        if to_sel.parent(self.map()).is_none() {
            from_sel.detach(&mut self.tree).push_back(to_sel);
        } else {
            from_sel.detach(&mut self.tree).insert_after(to_sel);
        }
    }

    fn split_selection(&mut self, _layout: LayoutId, _kind: crate::layout_engine::LayoutKind) {}

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
        Vec::new()
    }

    fn unstack_parent_of_selection(
        &mut self,
        _layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        Vec::new()
    }

    fn parent_of_selection_is_stacked(&self, _layout: LayoutId) -> bool { false }

    fn unjoin_selection(&mut self, _layout: LayoutId) {}

    fn resize_selection_by(&mut self, _layout: LayoutId, _amount: f64) {}

    fn rebalance(&mut self, _layout: LayoutId) {}

    fn toggle_tile_orientation(&mut self, _layout: LayoutId) {}
}

#[derive(Default, Serialize, Deserialize)]
struct ScrollComponents {
    selection: Selection,
    window: Window,
}

impl ScrollComponents {
    fn dispatch_event(&mut self, map: &NodeMap, event: TreeEvent) {
        self.selection.handle_event(map, event);
        self.window.handle_event(map, event);
    }
}

impl tree::Observer for ScrollComponents {
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

#[derive(Default, Serialize, Deserialize)]
struct Window {
    windows: slotmap::SecondaryMap<NodeId, WindowId>,
    window_nodes: BTreeMap<WindowId, WindowNodeInfoVec>,
}

#[derive(Serialize, Deserialize)]
struct WindowNodeInfo {
    layout: LayoutId,
    node: NodeId,
}

#[derive(Serialize, Deserialize, Default)]
struct WindowNodeInfoVec(Vec<WindowNodeInfo>);

impl Window {
    fn at(&self, node: NodeId) -> Option<WindowId> { self.windows.get(node).copied() }

    fn node_for(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.window_nodes.get(&wid).and_then(|nodes| {
            nodes.0.iter().find(|info| info.layout == layout).map(|info| info.node)
        })
    }

    fn set_window(&mut self, layout: LayoutId, node: NodeId, wid: WindowId) {
        let existing = self.windows.insert(node, wid);
        assert!(
            existing.is_none(),
            "Attempted to overwrite window for node {node:?} from {existing:?} to {wid:?}"
        );
        self.window_nodes
            .entry(wid)
            .or_default()
            .0
            .push(WindowNodeInfo { layout, node });
    }

    fn take_nodes_for(&mut self, wid: WindowId) -> impl Iterator<Item = (LayoutId, NodeId)> {
        self.window_nodes
            .remove(&wid)
            .unwrap_or_default()
            .0
            .into_iter()
            .map(|info| (info.layout, info.node))
    }

    fn take_nodes_for_app(
        &mut self,
        pid: pid_t,
    ) -> impl Iterator<Item = (WindowId, LayoutId, NodeId)> {
        let removed = self.window_nodes.remove_all_for_pid(pid);
        removed.into_iter().flat_map(|(wid, infos)| {
            infos.0.into_iter().map(move |info| (wid, info.layout, info.node))
        })
    }

    fn swap_nodes(&mut self, layout: LayoutId, a: NodeId, b: NodeId) {
        if let (Some(info_a), Some(info_b)) =
            (self.windows.get(a).copied(), self.windows.get(b).copied())
        {
            self.windows.insert(a, info_b);
            self.windows.insert(b, info_a);
            if let Some(entries) = self.window_nodes.get_mut(&info_a) {
                if let Some(entry) =
                    entries.0.iter_mut().find(|entry| entry.layout == layout && entry.node == a)
                {
                    entry.node = b;
                }
            }
            if let Some(entries) = self.window_nodes.get_mut(&info_b) {
                if let Some(entry) =
                    entries.0.iter_mut().find(|entry| entry.layout == layout && entry.node == b)
                {
                    entry.node = a;
                }
            }
        }
    }

    fn handle_event(&mut self, map: &NodeMap, event: TreeEvent) {
        match event {
            TreeEvent::AddedToForest(_) => (),
            TreeEvent::AddedToParent(node) => debug_assert!(
                self.windows.get(node.parent(map).unwrap()).is_none(),
                "Window nodes are not allowed to have children: {:?}/{:?}",
                node.parent(map).unwrap(),
                node
            ),
            TreeEvent::Copied { src, dest, dest_layout } => {
                if let Some(&wid) = self.windows.get(src) {
                    self.set_window(dest_layout, dest, wid);
                }
            }
            TreeEvent::RemovingFromParent(_) => (),
            TreeEvent::RemovedFromForest(node) => {
                if let Some(wid) = self.windows.remove(node) {
                    if let Some(window_nodes) = self.window_nodes.get_mut(&wid) {
                        window_nodes.0.retain(|info| info.node != node);
                        if window_nodes.0.is_empty() {
                            self.window_nodes.remove(&wid);
                        }
                    }
                }
            }
        }
    }
}
