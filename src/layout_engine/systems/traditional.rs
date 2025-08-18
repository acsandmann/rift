use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use tracing::warn;

mod layout;
mod stack;
mod window;
use layout::*;
use window::*;

use super::LayoutSystem;
use crate::actor::app::{WindowId, pid_t};
use crate::layout_engine::{Direction, LayoutId, LayoutKind};
use crate::model::selection::*;
use crate::model::tree::{self, NodeId, NodeMap, OwnedNode, Tree};

#[derive(Serialize, Deserialize)]
pub struct TraditionalLayoutSystem {
    tree: Tree<Components>,
    layout_roots: slotmap::SlotMap<LayoutId, OwnedNode>,
}

impl Default for TraditionalLayoutSystem {
    fn default() -> Self {
        Self {
            tree: Tree::with_observer(Components::default()),
            layout_roots: Default::default(),
        }
    }
}

impl TraditionalLayoutSystem {
    pub fn new() -> Self { Self::default() }

    fn root(&self, layout: LayoutId) -> NodeId { self.layout_roots[layout].id() }

    fn selection(&self, layout: LayoutId) -> NodeId {
        self.tree.data.selection.current_selection(self.root(layout))
    }

    fn map(&self) -> &NodeMap { &self.tree.map }

    fn layout(&self, node: NodeId) -> LayoutKind { self.tree.data.layout.kind(node) }

    fn set_layout(&mut self, node: NodeId, kind: LayoutKind) {
        self.tree.data.layout.set_kind(node, kind);
    }
}

impl Drop for TraditionalLayoutSystem {
    fn drop(&mut self) {
        for (_, node) in self.layout_roots.drain() {
            std::mem::forget(node);
        }
    }
}

impl LayoutSystem for TraditionalLayoutSystem {
    type LayoutId = crate::layout_engine::LayoutId;

    fn create_layout(&mut self) -> Self::LayoutId {
        let root = OwnedNode::new_root_in(&mut self.tree, "layout_root");
        self.layout_roots.insert(root)
    }

    fn clone_layout(&mut self, layout: Self::LayoutId) -> Self::LayoutId {
        let source_root = self.layout_roots[layout].id();
        let cloned = source_root.deep_copy(&mut self.tree).make_root("layout_root");
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
        dest_layout
    }

    fn remove_layout(&mut self, layout: Self::LayoutId) {
        self.layout_roots.remove(layout).unwrap().remove(&mut self.tree)
    }

    fn draw_tree(&self, layout: Self::LayoutId) -> String { self.draw_tree_internal(layout) }

    fn calculate_layout(
        &self,
        layout: Self::LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        self.tree.data.layout.get_sizes_with_gaps(
            &self.tree.map,
            &self.tree.data.window,
            self.root(layout),
            screen,
            stack_offset,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        )
    }

    fn selected_window(&self, layout: Self::LayoutId) -> Option<WindowId> {
        let selection = self.selection(layout);
        self.tree.data.window.at(selection)
    }

    fn visible_windows_in_layout(&self, layout: Self::LayoutId) -> Vec<WindowId> {
        let root = self.root(layout);
        self.visible_windows_under(root)
    }

    fn visible_windows_under_selection(&self, layout: Self::LayoutId) -> Vec<WindowId> {
        let selection = self.selection(layout);
        self.visible_windows_under(selection)
    }

    fn ascend_selection(&mut self, layout: Self::LayoutId) -> bool {
        if let Some(parent) = self.selection(layout).parent(self.map()) {
            self.select(parent);
            return true;
        }
        false
    }

    fn descend_selection(&mut self, layout: Self::LayoutId) -> bool {
        if let Some(child) =
            self.tree.data.selection.last_selection(self.map(), self.selection(layout))
        {
            self.select(child);
            return true;
        }
        false
    }

    fn move_focus(
        &mut self,
        layout: Self::LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let selection = self.selection(layout);
        if let Some(new_node) = self.traverse_internal(selection, direction) {
            let focus_window = self
                .tree
                .data
                .window
                .at(new_node)
                .or_else(|| self.visible_windows_under(new_node).into_iter().next());
            let raise_windows = self.select_returning_surfaced_windows_internal(new_node);
            (focus_window, raise_windows)
        } else {
            (None, vec![])
        }
    }

    fn add_window_after_selection(&mut self, layout: Self::LayoutId, wid: WindowId) {
        let selection = self.selection(layout);
        let node = self.add_window_after_internal(layout, selection, wid);
        self.select_internal(node);
    }

    fn remove_window(&mut self, wid: WindowId) { self.remove_window_impl(wid) }

    fn remove_windows_for_app(&mut self, pid: pid_t) { self.remove_windows_for_app_impl(pid) }

    fn set_windows_for_app(&mut self, layout: Self::LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        self.set_windows_for_app_impl(layout, pid, desired)
    }

    fn has_windows_for_app(&self, layout: Self::LayoutId, pid: pid_t) -> bool {
        self.has_windows_for_app_impl(layout, pid)
    }

    fn contains_window(&self, layout: Self::LayoutId, wid: WindowId) -> bool {
        self.window_node(layout, wid).is_some()
    }

    fn select_window(&mut self, layout: Self::LayoutId, wid: WindowId) -> bool {
        if let Some(node) = self.window_node(layout, wid) {
            self.select(node);
            true
        } else {
            false
        }
    }

    fn on_window_resized(
        &mut self,
        layout: Self::LayoutId,
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
    ) {
        if let Some(node) = self.window_node(layout, wid) {
            if new_frame == screen {
                self.set_fullscreen(node, true);
            } else if old_frame == screen {
                self.set_fullscreen(node, false);
            } else {
                self.set_frame_from_resize(node, old_frame, new_frame, screen);
            }
        }
    }

    fn move_selection(&mut self, layout: Self::LayoutId, direction: Direction) -> bool {
        let selection = self.selection(layout);
        self.move_node(layout, selection, direction)
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: Self::LayoutId,
        to_layout: Self::LayoutId,
    ) {
        let from_sel = self.selection(from_layout);
        let to_sel = self.selection(to_layout);
        self.move_node_after_internal(to_sel, from_sel);
    }

    fn split_selection(&mut self, layout: Self::LayoutId, kind: LayoutKind) {
        let selection = self.selection(layout);
        self.nest_in_container_internal(layout, selection, kind);
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId> {
        let node = self.selection(layout);
        if self.toggle_fullscreen_internal(node) {
            self.visible_windows_under_internal(node)
        } else {
            vec![]
        }
    }

    fn join_selection_with_direction(&mut self, layout: Self::LayoutId, direction: Direction) {
        let selection = self.selection(layout);
        if let Some(target) = self.traverse_internal(selection, direction) {
            let common_parent =
                self.find_or_create_common_parent_internal(layout, selection, target);
            let container_layout = LayoutKind::from(direction.orientation());
            self.set_layout(common_parent, container_layout);
            self.select_internal(common_parent);
        }
    }

    fn apply_stacking_to_parent_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId> {
        let selection = self.selection(layout);
        if let Some(parent) = selection.parent(self.map()) {
            let current_layout = self.layout(parent);
            let new_layout = match current_layout {
                LayoutKind::Horizontal => LayoutKind::HorizontalStack,
                LayoutKind::Vertical => LayoutKind::VerticalStack,
                LayoutKind::HorizontalStack => LayoutKind::VerticalStack,
                LayoutKind::VerticalStack => LayoutKind::HorizontalStack,
            };
            self.set_layout(parent, new_layout);
            self.visible_windows_under_internal(parent)
        } else {
            vec![]
        }
    }

    fn unstack_parent_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId> {
        let selection = self.selection(layout);
        if let Some(parent) = selection.parent(self.map()) {
            match self.layout(parent) {
                LayoutKind::HorizontalStack => {
                    self.set_layout(parent, LayoutKind::Horizontal);
                    self.visible_windows_under_internal(parent)
                }
                LayoutKind::VerticalStack => {
                    self.set_layout(parent, LayoutKind::Vertical);
                    self.visible_windows_under_internal(parent)
                }
                _ => vec![],
            }
        } else {
            vec![]
        }
    }

    fn unjoin_selection(&mut self, layout: Self::LayoutId) {
        let selection = self.selection(layout);
        if let Some(parent) = selection.parent(self.map()) {
            let children: Vec<_> = parent.children(self.map()).collect();
            if children.len() == 2 {
                self.remove_unnecessary_container_internal(parent);
            }
        }
    }

    fn resize_selection_by(&mut self, layout: Self::LayoutId, amount: f64) {
        let selection = self.selection(layout);
        if let Some(_focused_window) = self.window_at_internal(selection) {
            let candidates = selection
                .ancestors(self.map())
                .filter(|&node| {
                    if let Some(parent) = node.parent(self.map()) {
                        !self.layout(parent).is_group()
                    } else {
                        false
                    }
                })
                .collect::<Vec<_>>();

            let resized = candidates.iter().any(|&node| {
                self.resize_internal(node, amount, crate::layout_engine::Direction::Right)
            }) || candidates.iter().any(|&node| {
                self.resize_internal(node, amount, crate::layout_engine::Direction::Down)
            });

            if !resized {
                let _ = candidates.iter().any(|&node| {
                    self.resize_internal(node, amount, crate::layout_engine::Direction::Left)
                }) || candidates.iter().any(|&node| {
                    self.resize_internal(node, amount, crate::layout_engine::Direction::Up)
                });
            }
        }
    }

    fn rebalance(&mut self, layout: Self::LayoutId) {
        let root = self.root(layout);
        self.rebalance_node(root)
    }
}

impl TraditionalLayoutSystem {
    pub(crate) fn collect_group_containers_in_selection_path(
        &self,
        layout: <TraditionalLayoutSystem as LayoutSystem>::LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        use objc2_core_foundation::{CGPoint, CGSize};

        use crate::layout_engine::LayoutKind::*;
        use crate::layout_engine::systems::traditional::stack::StackLayoutResult;
        use crate::sys::geometry::Round;

        let mut out = Vec::new();
        let map = &self.tree.map;

        let tiling_area = if gaps.outer.top == 0.0
            && gaps.outer.left == 0.0
            && gaps.outer.bottom == 0.0
            && gaps.outer.right == 0.0
        {
            screen
        } else {
            CGRect {
                origin: CGPoint {
                    x: screen.origin.x + gaps.outer.left,
                    y: screen.origin.y + gaps.outer.top,
                },
                size: CGSize {
                    width: (screen.size.width - gaps.outer.left - gaps.outer.right).max(0.0),
                    height: (screen.size.height - gaps.outer.top - gaps.outer.bottom).max(0.0),
                },
            }
            .round()
        };

        let mut node = self.root(layout);
        let mut rect = tiling_area;

        loop {
            let kind = self.tree.data.layout.kind(node);
            let children: Vec<_> = node.children(map).collect();

            if matches!(kind, HorizontalStack | VerticalStack) {
                if children.is_empty() {
                    break;
                }

                let local_sel =
                    self.tree.data.selection.local_selection(map, node).unwrap_or(children[0]);
                let selected_index = children.iter().position(|&c| c == local_sel).unwrap_or(0);

                out.push(crate::layout_engine::engine::GroupContainerInfo {
                    node_id: node,
                    container_kind: kind,
                    frame: rect,
                    total_count: children.len(),
                    selected_index,
                });

                let mut container_rect = rect;
                let reserve = stack_line_thickness.max(0.0);
                let is_horizontal = matches!(kind, HorizontalStack);
                if reserve > 0.0 {
                    if is_horizontal {
                        match stack_line_horiz {
                            crate::common::config::HorizontalPlacement::Top => {
                                let new_h = (container_rect.size.height - reserve).max(0.0);
                                container_rect = CGRect {
                                    origin: CGPoint {
                                        x: container_rect.origin.x,
                                        y: container_rect.origin.y + reserve,
                                    },
                                    size: CGSize {
                                        width: container_rect.size.width,
                                        height: new_h,
                                    },
                                };
                            }
                            crate::common::config::HorizontalPlacement::Bottom => {
                                let new_h = (container_rect.size.height - reserve).max(0.0);
                                container_rect = CGRect {
                                    origin: CGPoint {
                                        x: container_rect.origin.x,
                                        y: container_rect.origin.y,
                                    },
                                    size: CGSize {
                                        width: container_rect.size.width,
                                        height: new_h,
                                    },
                                };
                            }
                        }
                    } else {
                        match stack_line_vert {
                            crate::common::config::VerticalPlacement::Right => {
                                let new_w = (container_rect.size.width - reserve).max(0.0);
                                container_rect = CGRect {
                                    origin: CGPoint {
                                        x: container_rect.origin.x,
                                        y: container_rect.origin.y,
                                    },
                                    size: CGSize {
                                        width: new_w,
                                        height: container_rect.size.height,
                                    },
                                };
                            }
                            crate::common::config::VerticalPlacement::Left => {
                                let new_w = (container_rect.size.width - reserve).max(0.0);
                                container_rect = CGRect {
                                    origin: CGPoint {
                                        x: container_rect.origin.x + reserve,
                                        y: container_rect.origin.y,
                                    },
                                    size: CGSize {
                                        width: new_w,
                                        height: container_rect.size.height,
                                    },
                                };
                            }
                        }
                    }
                }

                let layout_res = StackLayoutResult::new(
                    container_rect,
                    children.len(),
                    stack_offset,
                    is_horizontal,
                );
                rect = layout_res.get_focused_frame_for_index(selected_index, selected_index);

                node = local_sel;
                continue;
            }

            if let Some(next) = self
                .tree
                .data
                .selection
                .local_selection(map, node)
                .or_else(|| node.first_child(map))
            {
                node = next;
                continue;
            }
            break;
        }

        out
    }
}

impl TraditionalLayoutSystem {
    fn draw_tree_internal(&self, layout: LayoutId) -> String {
        let tree = self.get_ascii_tree(self.root(layout));
        let mut out = String::new();
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
    }

    fn get_ascii_tree(&self, node: NodeId) -> ascii_tree::Tree {
        let status = match node.parent(&self.tree.map) {
            None => "",
            Some(parent)
                if self.tree.data.selection.local_selection(&self.tree.map, parent)
                    == Some(node) =>
            {
                "☒ "
            }
            _ => "☐ ",
        };
        let desc = format!("{status}{node:?}");
        let desc = match self.window_at(node) {
            Some(wid) => format!("{desc} {:?} {}", wid, self.tree.data.layout.debug(node, false)),
            None => format!("{desc} {}", self.tree.data.layout.debug(node, true)),
        };
        let children: Vec<_> =
            node.children(&self.tree.map).map(|c| self.get_ascii_tree(c)).collect();
        if children.is_empty() {
            ascii_tree::Tree::Leaf(vec![desc])
        } else {
            ascii_tree::Tree::Node(desc, children)
        }
    }

    fn add_window_under(&mut self, layout: LayoutId, parent: NodeId, wid: WindowId) -> NodeId {
        let node = self.tree.mk_node().push_back(parent);
        self.tree.data.window.set_window(layout, node, wid);
        node
    }

    fn add_window_after_internal(
        &mut self,
        layout: LayoutId,
        sibling: NodeId,
        wid: WindowId,
    ) -> NodeId {
        if sibling.parent(self.map()).is_none() {
            return self.add_window_under(layout, sibling, wid);
        }
        let node = self.tree.mk_node().insert_after(sibling);
        self.tree.data.window.set_window(layout, node, wid);
        node
    }

    fn move_node_after_internal(&mut self, sibling: NodeId, moving_node: NodeId) {
        let map = &self.tree.map;
        let Some(old_parent) = moving_node.parent(map) else {
            return;
        };
        let is_selection =
            self.tree.data.selection.local_selection(map, old_parent) == Some(moving_node);
        if sibling.parent(self.map()).is_none() {
            moving_node.detach(&mut self.tree).push_back(sibling);
        } else {
            moving_node.detach(&mut self.tree).insert_after(sibling);
        }
        if is_selection {
            for node in moving_node.ancestors(&self.tree.map) {
                if node == old_parent {
                    break;
                }
                self.tree.data.selection.select_locally(&self.tree.map, node);
            }
        }
    }

    fn remove_window_impl(&mut self, wid: WindowId) {
        let nodes: Vec<_> =
            self.tree.data.window.take_nodes_for(wid).map(|(_, node)| node).collect();
        for node in nodes {
            node.detach(&mut self.tree).remove();
        }
    }

    fn remove_windows_for_app_impl(&mut self, pid: pid_t) {
        let nodes: Vec<_> =
            self.tree.data.window.take_nodes_for_app(pid).map(|(_, _, node)| node).collect();
        for node in nodes {
            node.detach(&mut self.tree).remove();
        }
    }

    fn set_windows_for_app_impl(
        &mut self,
        layout: LayoutId,
        app: pid_t,
        mut desired: Vec<WindowId>,
    ) {
        let root = self.root(layout);
        let mut current = root
            .traverse_postorder(self.map())
            .filter_map(|node| self.window_at(node).map(|wid| (wid, node)))
            .filter(|(wid, _)| wid.pid == app)
            .collect::<Vec<_>>();
        desired.sort_unstable();
        current.sort_unstable();
        debug_assert!(desired.iter().all(|wid| wid.pid == app));
        let mut desired = desired.into_iter().peekable();
        let mut current = current.into_iter().peekable();
        loop {
            match (desired.peek(), current.peek()) {
                (Some(des), Some((cur, _))) if des == cur => {
                    desired.next();
                    current.next();
                }
                (Some(des), None) => {
                    self.add_window_under(layout, root, *des);
                    desired.next();
                }
                (Some(des), Some((cur, _))) if des < cur => {
                    self.add_window_under(layout, root, *des);
                    desired.next();
                }
                (_, Some((_, node))) => {
                    node.detach(&mut self.tree).remove();
                    current.next();
                }
                (None, None) => break,
            }
        }
    }

    fn window_node(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.tree.data.window.node_for(layout, wid)
    }

    fn window_at(&self, node: NodeId) -> Option<WindowId> { self.tree.data.window.at(node) }

    fn window_at_internal(&self, node: NodeId) -> Option<WindowId> { self.window_at(node) }

    fn has_windows_for_app_impl(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.root(layout)
            .traverse_postorder(self.map())
            .filter_map(|node| self.window_at(node))
            .any(|wid| wid.pid == pid)
    }

    fn rebalance_node(&mut self, node: NodeId) {
        let map = &self.tree.map;
        let children: Vec<_> = node.children(map).collect();
        let count = children.len() as f32;
        if count == 0.0 {
            return;
        }
        self.tree.data.layout.info[node].total = count;
        for &child in &children {
            if child.children(map).next().is_none() || self.tree.data.layout.info[child].size == 0.0
            {
                self.tree.data.layout.info[child].size = 1.0;
            }
        }
        for child in children {
            self.rebalance_node(child);
        }
    }

    fn add_container(&mut self, parent: NodeId, kind: LayoutKind) -> NodeId {
        let node = self.tree.mk_node().push_back(parent);
        self.tree.data.layout.set_kind(node, kind);
        node
    }

    fn select(&mut self, selection: NodeId) {
        self.tree.data.selection.select(&self.tree.map, selection)
    }

    fn select_internal(&mut self, node: NodeId) { self.select(node) }

    fn set_fullscreen(&mut self, node: NodeId, is_fullscreen: bool) {
        self.tree.data.layout.set_fullscreen(node, is_fullscreen)
    }

    fn toggle_fullscreen_internal(&mut self, node: NodeId) -> bool {
        self.tree.data.layout.toggle_fullscreen(node)
    }

    fn traverse_internal(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let map = &self.tree.map;
        if let Some(sibling) = self.move_over(from, direction) {
            return Some(sibling);
        }
        let node = from.ancestors(map).skip(1).find_map(|ancestor| {
            if let Some(target) = self.move_over(ancestor, direction) {
                Some(self.descend_into_target(target, direction, map))
            } else {
                None
            }
        });
        node.flatten()
    }

    fn descend_into_target(
        &self,
        target: NodeId,
        direction: Direction,
        map: &NodeMap,
    ) -> Option<NodeId> {
        let mut current = target;
        loop {
            let children: Vec<_> = current.children(map).collect();
            if children.is_empty() {
                return Some(current);
            }
            let layout_kind = self.tree.data.layout.kind(current);
            if let Some(selected) = self.tree.data.selection.local_selection(map, current) {
                match (layout_kind, direction) {
                    (LayoutKind::Horizontal, Direction::Up | Direction::Down)
                    | (LayoutKind::Vertical, Direction::Left | Direction::Right) => {
                        current = selected;
                        continue;
                    }
                    _ if layout_kind.is_stacked() => {
                        current = selected;
                        continue;
                    }
                    _ => {}
                }
            }
            let next_child = match (layout_kind, direction) {
                (LayoutKind::Horizontal, Direction::Left) => children.first().copied(),
                (LayoutKind::Horizontal, Direction::Right) => children.last().copied(),
                (LayoutKind::Horizontal, Direction::Up | Direction::Down) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or_else(|| children.first().copied()),
                (LayoutKind::Vertical, Direction::Up) => children.first().copied(),
                (LayoutKind::Vertical, Direction::Down) => children.last().copied(),
                (LayoutKind::Vertical, Direction::Left | Direction::Right) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or_else(|| children.first().copied()),
                _ if layout_kind.is_stacked() => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or_else(|| children.first().copied()),
                _ => None,
            };
            match next_child {
                Some(child) => current = child,
                None => return Some(current),
            }
        }
    }

    fn select_returning_surfaced_windows_internal(&mut self, selection: NodeId) -> Vec<WindowId> {
        let map = &self.tree.map;
        let mut highest_revealed = selection;
        for (node, parent) in selection.ancestors_with_parent(map) {
            let Some(parent) = parent else { break };
            if self.tree.data.selection.select_locally(map, node) {
                if self.layout(parent).is_group() {
                    highest_revealed = node;
                }
            }
        }
        self.visible_windows_under_internal(highest_revealed)
    }

    fn visible_windows_under_internal(&self, node: NodeId) -> Vec<WindowId> {
        let mut stack = vec![node];
        let mut windows = vec![];
        while let Some(node) = stack.pop() {
            if self.layout(node).is_group() {
                stack.extend(self.tree.data.selection.local_selection(self.map(), node));
            } else {
                stack.extend(node.children(self.map()));
            }
            windows.extend(self.window_at(node));
        }
        windows
    }

    fn visible_windows_under(&self, node: NodeId) -> Vec<WindowId> {
        self.visible_windows_under_internal(node)
    }

    fn move_over(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let Some(parent) = from.parent(&self.tree.map) else {
            return None;
        };
        if self.tree.data.layout.kind(parent).orientation() == direction.orientation() {
            match direction {
                Direction::Left | Direction::Up => from.prev_sibling(&self.tree.map),
                Direction::Right | Direction::Down => from.next_sibling(&self.tree.map),
            }
        } else {
            let parent_layout = self.tree.data.layout.kind(parent);
            let siblings: Vec<_> = parent.children(&self.tree.map).collect();
            let current_position = siblings.iter().position(|&s| s == from)?;
            match (parent_layout, direction) {
                (LayoutKind::Vertical, Direction::Left)
                | (LayoutKind::Vertical, Direction::Right)
                | (LayoutKind::Horizontal, Direction::Up)
                | (LayoutKind::Horizontal, Direction::Down) => None,
                _ if parent_layout.is_stacked() => match direction {
                    Direction::Left | Direction::Up => {
                        if current_position > 0 {
                            Some(siblings[current_position - 1])
                        } else {
                            None
                        }
                    }
                    Direction::Right | Direction::Down => {
                        if current_position < siblings.len() - 1 {
                            Some(siblings[current_position + 1])
                        } else {
                            None
                        }
                    }
                },
                _ => None,
            }
        }
    }

    fn move_node(&mut self, layout: LayoutId, moving_node: NodeId, direction: Direction) -> bool {
        let map = &self.tree.map;
        let Some(old_parent) = moving_node.parent(map) else {
            return false;
        };
        let is_selection =
            self.tree.data.selection.local_selection(map, old_parent) == Some(moving_node);
        let moved = self.move_node_inner(layout, moving_node, direction);
        if moved && is_selection {
            for node in moving_node.ancestors(&self.tree.map) {
                if node == old_parent {
                    break;
                }
                self.tree.data.selection.select_locally(&self.tree.map, node);
            }
        }
        moved
    }

    fn move_node_inner(
        &mut self,
        layout: LayoutId,
        moving_node: NodeId,
        direction: Direction,
    ) -> bool {
        enum Destination {
            Ahead(NodeId),
            Behind(NodeId),
        }
        let map = &self.tree.map;
        let destination;
        if let Some(sibling) = self.move_over(moving_node, direction) {
            let mut node = sibling;
            let target = loop {
                let Some(next) =
                    self.tree.data.selection.local_selection(map, node).or(node.first_child(map))
                else {
                    break node;
                };
                if self.tree.data.layout.kind(node).orientation() == direction.orientation() {
                    break next;
                }
                node = next;
            };
            if target == sibling {
                destination = Destination::Ahead(sibling);
            } else {
                destination = Destination::Behind(target);
            }
        } else {
            let target_ancestor = moving_node.ancestors_with_parent(&self.tree.map).skip(1).find(
                |(_node, parent)| {
                    parent
                        .map(|p| self.layout(p).orientation() == direction.orientation())
                        .unwrap_or(false)
                },
            );
            if let Some((target, _parent)) = target_ancestor {
                destination = Destination::Ahead(target);
            } else {
                let old_root = moving_node.ancestors(map).last().unwrap();
                if self.tree.data.layout.kind(old_root).orientation() == direction.orientation() {
                    let is_edge_move = match direction {
                        Direction::Left | Direction::Up => moving_node.prev_sibling(map).is_none(),
                        Direction::Right | Direction::Down => {
                            moving_node.next_sibling(map).is_none()
                        }
                    };
                    if !is_edge_move {
                        return false;
                    }
                }
                let new_container_kind = LayoutKind::from(direction.orientation());
                self.nest_in_container_internal(layout, old_root, new_container_kind);
                destination = Destination::Ahead(old_root);
            }
        }
        match (destination, direction) {
            (Destination::Ahead(target), Direction::Right | Direction::Down) => {
                moving_node.detach(&mut self.tree).insert_after(target);
            }
            (Destination::Behind(target), Direction::Right | Direction::Down) => {
                moving_node.detach(&mut self.tree).insert_before(target);
            }
            (Destination::Ahead(target), Direction::Left | Direction::Up) => {
                moving_node.detach(&mut self.tree).insert_before(target);
            }
            (Destination::Behind(target), Direction::Left | Direction::Up) => {
                moving_node.detach(&mut self.tree).insert_after(target);
            }
        }
        true
    }

    fn resize_internal(&mut self, node: NodeId, screen_ratio: f64, direction: Direction) -> bool {
        let can_resize = |&node: &NodeId| -> bool {
            let Some(parent) = node.parent(&self.tree.map) else {
                return false;
            };
            !self.tree.data.layout.kind(parent).is_group()
                && self.move_over(node, direction).is_some()
        };
        let Some(resizing_node) = node.ancestors(&self.tree.map).filter(can_resize).next() else {
            return false;
        };
        let sibling = self.move_over(resizing_node, direction).unwrap();
        let exchange_rate = resizing_node
            .ancestors(&self.tree.map)
            .skip(1)
            .try_fold(1.0, |r, node| match node.parent(&self.tree.map) {
                Some(parent)
                    if self.tree.data.layout.kind(parent).orientation()
                        == direction.orientation()
                        && !self.tree.data.layout.kind(parent).is_group() =>
                {
                    self.tree.data.layout.proportion(&self.tree.map, node).map(|p| r * p)
                }
                _ => Some(r),
            })
            .unwrap_or(1.0);
        let local_ratio = f64::from(screen_ratio)
            * f64::from(
                self.tree.data.layout.info[resizing_node.parent(&self.tree.map).unwrap()].total,
            )
            / exchange_rate;
        self.tree.data.layout.take_share(
            &self.tree.map,
            resizing_node,
            sibling,
            local_ratio as f32,
        );
        true
    }

    fn resize(&mut self, node: NodeId, screen_ratio: f64, direction: Direction) -> bool {
        self.resize_internal(node, screen_ratio, direction)
    }

    fn set_frame_from_resize(
        &mut self,
        node: NodeId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
    ) {
        let mut check_or_resize = |resize: bool| {
            let mut count = 0;
            let mut first_direction: Option<Direction> = None;
            let mut good = true;
            let mut check_and_resize = |direction: Direction, delta, whole| {
                if delta != 0.0 {
                    count += 1;
                    if count > 2 {
                        good = false;
                    }
                    if let Some(first) = first_direction {
                        if first.orientation() == direction.orientation() {
                            good = false;
                        }
                    } else {
                        first_direction = Some(direction);
                    }
                    if resize {
                        self.resize(node, f64::from(delta) / f64::from(whole), direction);
                    }
                }
            };
            check_and_resize(
                Direction::Left,
                old_frame.min().x - new_frame.min().x,
                screen.size.width,
            );
            check_and_resize(
                Direction::Right,
                new_frame.max().x - old_frame.max().x,
                screen.size.width,
            );
            check_and_resize(
                Direction::Up,
                old_frame.min().y - new_frame.min().y,
                screen.size.height,
            );
            check_and_resize(
                Direction::Down,
                new_frame.max().y - old_frame.max().y,
                screen.size.height,
            );
            good
        };
        if !check_or_resize(false) {
            warn!(
                "Only resizing in 2 directions is supported, but was asked to resize from {old_frame:?} to {new_frame:?}"
            );
            return;
        }
        check_or_resize(true);
    }

    fn nest_in_container_internal(
        &mut self,
        layout: LayoutId,
        node: NodeId,
        kind: LayoutKind,
    ) -> NodeId {
        let old_parent = node.parent(&self.tree.map);
        let parent = if node.prev_sibling(&self.tree.map).is_none()
            && node.next_sibling(&self.tree.map).is_none()
            && old_parent.is_some()
        {
            old_parent.unwrap()
        } else {
            let new_parent = if let Some(old_parent) = old_parent {
                let is_selection =
                    self.tree.data.selection.local_selection(self.map(), old_parent) == Some(node);
                let new_parent = self.tree.mk_node().insert_before(node);
                self.tree.data.layout.assume_size_of(new_parent, node, &self.tree.map);
                node.detach(&mut self.tree).push_back(new_parent);
                if is_selection {
                    self.tree.data.selection.select_locally(&self.tree.map, new_parent);
                }
                new_parent
            } else {
                let layout_root = self.layout_roots.get_mut(layout).unwrap();
                layout_root.replace(self.tree.mk_node()).push_back(layout_root.id());
                layout_root.id()
            };
            self.tree.data.selection.select_locally(&self.tree.map, node);
            new_parent
        };
        self.tree.data.layout.set_kind(parent, kind);
        parent
    }

    fn find_or_create_common_parent_internal(
        &mut self,
        _layout: LayoutId,
        node1: NodeId,
        node2: NodeId,
    ) -> NodeId {
        let parent1 = node1.parent(self.map());
        let parent2 = node2.parent(self.map());
        if let (Some(p1), Some(p2)) = (parent1, parent2) {
            if p1 == p2 {
                let new_container = self.tree.mk_node().insert_before(node1);
                self.tree.data.layout.assume_size_of(new_container, node1, &self.tree.map);
                self.tree.data.layout.assume_size_of(new_container, node2, &self.tree.map);
                node1.detach(&mut self.tree).push_back(new_container);
                node2.detach(&mut self.tree).push_back(new_container);
                return new_container;
            }
        }
        let ancestors1: Vec<_> = node1.ancestors(self.map()).collect();
        let ancestors2: Vec<_> = node2.ancestors(self.map()).collect();
        for &ancestor in &ancestors1 {
            if ancestors2.contains(&ancestor) {
                let container = self.add_container(ancestor, LayoutKind::Horizontal);
                node1.detach(&mut self.tree).push_back(container);
                node2.detach(&mut self.tree).push_back(container);
                return container;
            }
        }
        panic!("Nodes are not in the same tree, cannot find common parent");
    }

    fn remove_unnecessary_container_internal(&mut self, container: NodeId) {
        let children: Vec<_> = container.children(self.map()).collect();
        if children.len() <= 1 {
            let parent = container.parent(self.map());
            for child in children {
                let detached = child.detach(&mut self.tree);
                if let Some(parent) = parent {
                    detached.push_back(parent);
                } else {
                    detached.remove();
                }
            }
            container.detach(&mut self.tree).remove();
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Components {
    selection: Selection,
    layout: Layout,
    window: Window,
}

impl tree::Observer for Components {
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
            child
                .detach(tree)
                .insert_after(parent)
                .with(|child_id, tree| tree.data.layout.assume_size_of(child_id, parent, &tree.map))
                .finish();
        }
    }

    fn removed_from_forest(&mut self, map: &NodeMap, node: NodeId) {
        self.dispatch_event(map, TreeEvent::RemovedFromForest(node))
    }
}

impl Components {
    fn dispatch_event(&mut self, map: &NodeMap, event: TreeEvent) {
        self.selection.handle_event(map, event);
        self.layout.handle_event(map, event);
        self.window.handle_event(map, event);
    }
}
