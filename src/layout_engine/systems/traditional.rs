use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::actor::app::{WindowId, pid_t};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::selection::*;
use crate::model::tree::{self, NodeId, NodeMap, OwnedNode, Tree};
use crate::sys::geometry::Round;

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
    fn find_best_focus_target(&self, node: NodeId) -> Option<WindowId> {
        if let Some(wid) = self.tree.data.window.at(node) {
            return Some(wid);
        }

        let children: Vec<_> = node.children(self.map()).collect();
        if children.is_empty() {
            return None;
        }

        if let Some(selected) = self.tree.data.selection.local_selection(self.map(), node) {
            if let Some(wid) = self.find_best_focus_target(selected) {
                return Some(wid);
            }
        }

        for &child in &children {
            if let Some(wid) = self.find_best_focus_target(child) {
                return Some(wid);
            }
        }

        None
    }

    fn smart_window_insertion(
        &mut self,
        layout: LayoutId,
        selection: NodeId,
        wid: WindowId,
    ) -> NodeId {
        let parent = selection.parent(self.map());

        if let Some(parent) = parent {
            let parent_layout = self.layout(parent);
            let sibling_count = parent.children(self.map()).count();

            if sibling_count >= 4 && !parent_layout.is_group() {
                let sub_container =
                    self.nest_in_container_internal(layout, selection, parent_layout);
                let node = self.tree.mk_node().push_back(sub_container);
                self.tree.data.window.set_window(layout, node, wid);
                return node;
            }
        }

        let node = self.tree.mk_node().insert_after(selection);
        self.tree.data.window.set_window(layout, node, wid);
        node
    }

    fn find_or_create_smart_common_parent(
        &mut self,
        layout: LayoutId,
        node1: NodeId,
        node2: NodeId,
        direction: Direction,
    ) -> NodeId {
        let parent1 = node1.parent(self.map());
        let parent2 = node2.parent(self.map());

        if let (Some(p1), Some(p2)) = (parent1, parent2) {
            if p1 == p2 {
                let parent_layout = self.layout(p1);
                let sibling_count = p1.children(self.map()).count();

                if parent_layout.orientation() == direction.orientation()
                    && !parent_layout.is_group()
                    && sibling_count == 2
                {
                    return p1;
                }
            }
        }

        self.find_or_create_common_parent_internal(layout, node1, node2)
    }

    fn root(&self, layout: LayoutId) -> NodeId { self.layout_roots[layout].id() }

    fn selection(&self, layout: LayoutId) -> NodeId {
        self.tree.data.selection.current_selection(self.root(layout))
    }

    fn map(&self) -> &NodeMap { &self.tree.map }

    fn layout(&self, node: NodeId) -> LayoutKind { self.tree.data.layout.kind(node) }

    fn set_layout(&mut self, node: NodeId, kind: LayoutKind) {
        self.tree.data.layout.set_kind(node, kind);
    }

    fn find_natural_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        if let Some(sibling) = self.find_direct_sibling_target(from, direction) {
            return Some(sibling);
        }

        if let Some(stack_neighbor) = self.find_stack_neighbor_target(from, direction) {
            return Some(stack_neighbor);
        }

        if let Some(traversed) = self.traverse_internal(from, direction) {
            if self.tree.data.window.at(traversed).is_some() {
                return Some(traversed);
            }

            if let Some(target_child) =
                self.find_best_container_child_for_joining(traversed, direction)
            {
                return Some(target_child);
            }

            return Some(traversed);
        }

        self.find_hierarchical_join_target(from, direction)
    }

    fn find_stack_neighbor_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let parent = from.parent(self.map())?;
        let parent_layout = self.layout(parent);

        if !parent_layout.is_stacked() {
            return None;
        }

        let children: Vec<_> = parent.children(self.map()).collect();
        let current_idx = children.iter().position(|&c| c == from)?;

        match direction {
            Direction::Right | Direction::Down => children.get(current_idx + 1).copied(),
            Direction::Left | Direction::Up => {
                if current_idx > 0 {
                    children.get(current_idx - 1).copied()
                } else {
                    None
                }
            }
        }
    }

    fn find_direct_sibling_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let _parent = from.parent(self.map())?;

        match direction {
            Direction::Right | Direction::Down => from.next_sibling(self.map()),
            Direction::Left | Direction::Up => from.prev_sibling(self.map()),
        }
    }

    fn find_best_container_child_for_joining(
        &self,
        container: NodeId,
        direction: Direction,
    ) -> Option<NodeId> {
        let children: Vec<_> = container.children(self.map()).collect();
        if children.is_empty() {
            return None;
        }

        let container_layout = self.layout(container);

        if container_layout.orientation() == direction.orientation() {
            return match direction {
                Direction::Left | Direction::Up => children.first().copied(),
                Direction::Right | Direction::Down => children.last().copied(),
            };
        }

        if let Some(selected) = self.tree.data.selection.local_selection(self.map(), container) {
            return Some(selected);
        }

        children.first().copied()
    }

    fn find_hierarchical_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        for ancestor in from.ancestors(self.map()).skip(1) {
            if let Some(target) = self.find_direct_sibling_target(ancestor, direction) {
                return self.find_best_container_child_for_joining(target, direction.opposite());
            }
        }
        None
    }

    fn perform_natural_join(
        &mut self,
        layout: LayoutId,
        selection: NodeId,
        target: NodeId,
        direction: Direction,
    ) {
        let selection_parent = selection.parent(self.map());
        let target_parent = target.parent(self.map());

        match (selection_parent, target_parent) {
            (Some(sp), Some(tp)) if sp == tp => {
                if self.layout(sp).is_stacked() {
                    let new_layout = match direction.orientation() {
                        Orientation::Horizontal => LayoutKind::Horizontal,
                        Orientation::Vertical => LayoutKind::Vertical,
                    };
                    self.set_layout(sp, new_layout);
                    self.select(sp);
                    return;
                }

                let common_parent =
                    self.find_or_create_smart_common_parent(layout, selection, target, direction);
                let container_layout = LayoutKind::from(direction.orientation());
                self.set_layout(common_parent, container_layout);
                self.select(common_parent);
            }

            (Some(sp), Some(tp)) if self.are_containers_mergeable(sp, tp, direction) => {
                self.merge_compatible_containers(layout, sp, tp, direction);
            }

            _ => {
                let common_parent =
                    self.find_or_create_smart_common_parent(layout, selection, target, direction);
                let container_layout = LayoutKind::from(direction.orientation());
                self.set_layout(common_parent, container_layout);
                self.select(common_parent);
            }
        }
    }

    fn are_containers_mergeable(
        &self,
        container1: NodeId,
        container2: NodeId,
        direction: Direction,
    ) -> bool {
        let layout1 = self.layout(container1);
        let layout2 = self.layout(container2);

        layout1.orientation() == direction.orientation()
            && layout2.orientation() == direction.orientation()
            && !layout1.is_group()
            && !layout2.is_group()
    }

    fn merge_compatible_containers(
        &mut self,
        layout: LayoutId,
        container1: NodeId,
        container2: NodeId,
        direction: Direction,
    ) {
        // TODO: Implement intelligent container merging
        let common_parent =
            self.find_or_create_smart_common_parent(layout, container1, container2, direction);
        let container_layout = LayoutKind::from(direction.orientation());
        self.set_layout(common_parent, container_layout);
        self.select(common_parent);
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
    fn create_layout(&mut self) -> LayoutId {
        let root = OwnedNode::new_root_in(&mut self.tree, "layout_root");
        self.layout_roots.insert(root)
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
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

    fn remove_layout(&mut self, layout: LayoutId) {
        self.layout_roots.remove(layout).unwrap().remove(&mut self.tree)
    }

    fn draw_tree(&self, layout: LayoutId) -> String {
        let tree = self.get_ascii_tree(self.root(layout));
        let mut out = String::new();
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let mut sizes = vec![];
        let tiling_area = compute_tiling_area(screen, gaps);

        self.tree.data.layout.apply_with_gaps(
            &self.tree.map,
            &self.tree.data.window,
            self.root(layout),
            tiling_area,
            screen,
            &mut sizes,
            stack_offset,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        );

        sizes
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        let selection = self.selection(layout);
        self.tree.data.window.at(selection)
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        let root = self.root(layout);
        self.visible_windows_under_internal(root)
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        let selection = self.selection(layout);
        self.visible_windows_under_internal(selection)
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool {
        if let Some(parent) = self.selection(layout).parent(self.map()) {
            self.select(parent);
            return true;
        }
        false
    }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
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
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let selection = self.selection(layout);
        if let Some(new_node) = self.traverse_internal(selection, direction) {
            let focus_window = self.find_best_focus_target(new_node);
            let map = &self.tree.map;
            let mut highest_revealed = new_node;

            for (node, parent) in new_node.ancestors_with_parent(map) {
                let Some(parent) = parent else { break };
                if self.tree.data.selection.select_locally(map, node) {
                    if self.layout(parent).is_group() {
                        highest_revealed = node;
                    }
                }
            }
            let raise_windows = self.visible_windows_under_internal(highest_revealed);
            (focus_window, raise_windows)
        } else {
            (None, vec![])
        }
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let selection = self.selection(layout);
        let node = if selection.parent(self.map()).is_none() {
            self.add_window_under(layout, selection, wid)
        } else {
            let node = self.smart_window_insertion(layout, selection, wid);
            node
        };
        self.select(node);
    }

    fn remove_window(&mut self, wid: WindowId) {
        let nodes: Vec<_> =
            self.tree.data.window.take_nodes_for(wid).map(|(_, node)| node).collect();
        for node in nodes {
            node.detach(&mut self.tree).remove();
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        let nodes: Vec<_> =
            self.tree.data.window.take_nodes_for_app(pid).map(|(_, _, node)| node).collect();
        for node in nodes {
            node.detach(&mut self.tree).remove();
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
            self.select(node);
            true
        } else {
            false
        }
    }

    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
    ) {
        if let Some(node) = self.tree.data.window.node_for(layout, wid) {
            if new_frame == screen {
                self.tree.data.layout.set_fullscreen(node, true);
            } else if old_frame == screen {
                self.tree.data.layout.set_fullscreen(node, false);
            } else {
                self.set_frame_from_resize(node, old_frame, new_frame, screen);
            }
        }
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let selection = self.selection(layout);
        self.move_node(layout, selection, direction)
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        let from_sel = self.selection(from_layout);
        let to_sel = self.selection(to_layout);

        let map = &self.tree.map;
        let Some(old_parent) = from_sel.parent(map) else { return };
        let is_selection =
            self.tree.data.selection.local_selection(map, old_parent) == Some(from_sel);
        if to_sel.parent(self.map()).is_none() {
            from_sel.detach(&mut self.tree).push_back(to_sel);
        } else {
            from_sel.detach(&mut self.tree).insert_after(to_sel);
        }
        if is_selection {
            for node in from_sel.ancestors(&self.tree.map) {
                if node == old_parent {
                    break;
                }
                self.tree.data.selection.select_locally(&self.tree.map, node);
            }
        }
    }

    fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind) {
        let selection = self.selection(layout);
        self.nest_in_container_internal(layout, selection, kind);
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        let node = self.selection(layout);
        if self.tree.data.layout.toggle_fullscreen(node) {
            self.visible_windows_under_internal(node)
        } else {
            vec![]
        }
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        let selection = self.selection(layout);

        if let Some(target) = self.find_natural_join_target(selection, direction) {
            self.perform_natural_join(layout, selection, target, direction);
        }
    }

    fn apply_stacking_to_parent_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        let selection = self.selection(layout);

        let target_container = if self.tree.data.window.at(selection).is_some() {
            selection.parent(self.map())
        } else {
            Some(selection)
        };

        if let Some(container) = target_container {
            let current_layout = self.layout(container);

            let new_layout = match current_layout {
                LayoutKind::Horizontal => Some(LayoutKind::HorizontalStack),
                LayoutKind::Vertical => Some(LayoutKind::VerticalStack),
                LayoutKind::HorizontalStack => Some(LayoutKind::VerticalStack),
                LayoutKind::VerticalStack => Some(LayoutKind::HorizontalStack),
            };

            if let Some(nl) = new_layout {
                self.set_layout(container, nl);
                return self.visible_windows_under_internal(container);
            }
        }

        vec![]
    }

    fn unstack_parent_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        let selection = self.selection(layout);

        let target_container = if self.tree.data.window.at(selection).is_some() {
            let map = self.map();
            selection
                .ancestors(map)
                .skip(1)
                .find(|&ancestor| self.layout(ancestor).is_stacked())
        } else {
            let selection_layout = self.layout(selection);
            if selection_layout.is_stacked() {
                Some(selection)
            } else {
                let map = self.map();
                selection.children(map).find(|&child| self.layout(child).is_stacked())
            }
        };

        if let Some(container) = target_container {
            let new_layout = match self.layout(container) {
                LayoutKind::HorizontalStack => Some(LayoutKind::Horizontal),
                LayoutKind::VerticalStack => Some(LayoutKind::Vertical),
                _ => None,
            };

            if let Some(nl) = new_layout {
                self.set_layout(container, nl);
                return self.visible_windows_under_internal(container);
            }
        }

        vec![]
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        let selection = self.selection(layout);
        if let Some(parent) = selection.parent(self.map()) {
            let children: Vec<_> = parent.children(self.map()).collect();
            if children.len() == 2 {
                self.remove_unnecessary_container_internal(parent);
            }
        }
    }

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        let selection = self.selection(layout);
        if let Some(_focused_window) = self.window_at(selection) {
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

    fn rebalance(&mut self, layout: LayoutId) {
        let root = self.root(layout);
        self.rebalance_node(root)
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
            return false;
        }

        let wa = self.tree.data.window.at(node_a);
        let wb = self.tree.data.window.at(node_b);

        match (wa, wb) {
            (None, None) => return false,
            _ => {
                if let Some(w) = wa {
                    self.tree.data.window.windows.insert(node_b, w);
                } else {
                    self.tree.data.window.windows.remove(node_b);
                }
                if let Some(w) = wb {
                    self.tree.data.window.windows.insert(node_a, w);
                } else {
                    self.tree.data.window.windows.remove(node_a);
                }
            }
        }

        if let Some(infos) = self.tree.data.window.window_nodes.get_mut(&a) {
            for info in &mut infos.0 {
                if info.layout == layout {
                    info.node = node_b;
                }
            }
        }
        if let Some(infos) = self.tree.data.window.window_nodes.get_mut(&b) {
            for info in &mut infos.0 {
                if info.layout == layout {
                    info.node = node_a;
                }
            }
        }

        true
    }
}

impl TraditionalLayoutSystem {
    pub(crate) fn collect_group_containers_in_selection_path(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        use self::StackLayoutResult;
        use crate::layout_engine::LayoutKind::*;

        let mut out = Vec::new();
        let map = &self.tree.map;

        let tiling_area = compute_tiling_area(screen, gaps);

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
                container_rect = adjust_stack_container_rect(
                    container_rect,
                    is_horizontal,
                    reserve,
                    stack_line_horiz,
                    stack_line_vert,
                );

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

    fn window_at(&self, node: NodeId) -> Option<WindowId> { self.tree.data.window.at(node) }

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

    fn select(&mut self, selection: NodeId) {
        self.tree.data.selection.select(&self.tree.map, selection)
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
                    .or(children.first().copied()),
                (LayoutKind::Vertical, Direction::Up) => children.first().copied(),
                (LayoutKind::Vertical, Direction::Down) => children.last().copied(),
                (LayoutKind::Vertical, Direction::Left | Direction::Right) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.first().copied()),
                _ if layout_kind.is_stacked() => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.first().copied()),
                _ => None,
            };
            match next_child {
                Some(child) => current = child,
                None => return Some(current),
            }
        }
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
            if !parent_layout.is_stacked() {
                return None;
            }
            let siblings: Vec<_> = parent.children(&self.tree.map).collect();
            let current_position = siblings.iter().position(|&s| s == from)?;
            match direction {
                Direction::Left | Direction::Up => {
                    if current_position > 0 {
                        Some(siblings[current_position - 1])
                    } else {
                        None
                    }
                }
                Direction::Right | Direction::Down => siblings.get(current_position + 1).copied(),
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
        let Some(resizing_node) = node.ancestors(&self.tree.map).find(can_resize) else {
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
            let deltas = [
                (
                    Direction::Left,
                    old_frame.min().x - new_frame.min().x,
                    screen.size.width,
                ),
                (
                    Direction::Right,
                    new_frame.max().x - old_frame.max().x,
                    screen.size.width,
                ),
                (
                    Direction::Up,
                    old_frame.min().y - new_frame.min().y,
                    screen.size.height,
                ),
                (
                    Direction::Down,
                    new_frame.max().y - old_frame.max().y,
                    screen.size.height,
                ),
            ];
            for (direction, delta, whole) in deltas {
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
                        self.resize_internal(node, f64::from(delta) / f64::from(whole), direction);
                    }
                }
            }
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
                let container = {
                    let node = self.tree.mk_node().push_back(ancestor);
                    self.tree.data.layout.set_kind(node, LayoutKind::Horizontal);
                    node
                };
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

#[derive(Default, Serialize, Deserialize)]
struct Window {
    windows: slotmap::SecondaryMap<NodeId, WindowId>,
    window_nodes: crate::common::collections::BTreeMap<WindowId, WindowNodeInfoVec>,
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
        use crate::common::collections::BTreeExt;
        let removed = self.window_nodes.remove_all_for_pid(pid);
        removed.into_iter().flat_map(|(wid, infos)| {
            infos.0.into_iter().map(move |info| (wid, info.layout, info.node))
        })
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

struct StackLayoutResult {
    container_rect: CGRect,
    stack_offset: f64,
    is_horizontal: bool,
    window_width: f64,
    window_height: f64,
}

impl StackLayoutResult {
    fn new(
        container_rect: CGRect,
        window_count: usize,
        stack_offset: f64,
        is_horizontal: bool,
    ) -> Self {
        let total_offset_space = if window_count > 0 {
            (window_count - 1) as f64 * stack_offset
        } else {
            0.0
        };
        let (window_width, window_height) = if is_horizontal {
            (
                (container_rect.size.width - total_offset_space).max(100.0),
                container_rect.size.height.max(100.0),
            )
        } else {
            (
                container_rect.size.width.max(100.0),
                (container_rect.size.height - total_offset_space).max(100.0),
            )
        };
        Self {
            container_rect,
            stack_offset,
            is_horizontal,
            window_width,
            window_height,
        }
    }

    fn get_frame_for_index(&self, index: usize) -> CGRect {
        use objc2_core_foundation::{CGPoint, CGSize};
        let offset_amount = index as f64 * self.stack_offset;
        let (x_offset, y_offset) = if self.is_horizontal {
            (offset_amount, 0.0)
        } else {
            (0.0, offset_amount)
        };
        CGRect {
            origin: CGPoint {
                x: self.container_rect.origin.x + x_offset,
                y: self.container_rect.origin.y + y_offset,
            },
            size: CGSize {
                width: self.window_width,
                height: self.window_height,
            },
        }
        .round()
    }

    fn get_focused_frame_for_index(&self, index: usize, _focused_idx: usize) -> CGRect {
        use objc2_core_foundation::{CGPoint, CGSize};
        const FOCUS_SIZE_INCREASE: f64 = 10.0;
        const FOCUS_OFFSET_DECREASE: f64 = 5.0;
        let offset_amount = index as f64 * self.stack_offset;
        let (mut origin_x, mut origin_y) = if self.is_horizontal {
            (
                self.container_rect.origin.x + offset_amount - FOCUS_OFFSET_DECREASE,
                self.container_rect.origin.y - FOCUS_OFFSET_DECREASE,
            )
        } else {
            (
                self.container_rect.origin.x - FOCUS_OFFSET_DECREASE,
                self.container_rect.origin.y + offset_amount - FOCUS_OFFSET_DECREASE,
            )
        };
        if self.is_horizontal {
            if index == 0 {
                origin_x = self.container_rect.origin.x;
            }
            let max_x = self.container_rect.origin.x + self.container_rect.size.width
                - (self.window_width + FOCUS_SIZE_INCREASE);
            origin_x = origin_x.min(max_x);
        }
        if !self.is_horizontal {
            if index == 0 {
                origin_y = self.container_rect.origin.y;
            }
            let max_y = self.container_rect.origin.y + self.container_rect.size.height
                - (self.window_height + FOCUS_SIZE_INCREASE);
            origin_y = origin_y.min(max_y);
        }
        let screen_x = self.container_rect.origin.x;
        let screen_y = self.container_rect.origin.y;
        let screen_width = self.container_rect.size.width;
        let screen_height = self.container_rect.size.height;
        let width = (self.window_width + FOCUS_SIZE_INCREASE).min(screen_width);
        let height = (self.window_height + FOCUS_SIZE_INCREASE).min(screen_height);
        let x = origin_x.clamp(screen_x, screen_x + screen_width - width);
        let y = origin_y.clamp(screen_y, screen_y + screen_height - height);
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
        .round()
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Layout {
    info: slotmap::SecondaryMap<NodeId, LayoutInfo>,
}

#[allow(unused)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct LayoutInfo {
    size: f32,
    total: f32,
    kind: LayoutKind,
    last_ungrouped_kind: LayoutKind,
    #[serde(default)]
    is_fullscreen: bool,
}

impl Layout {
    fn handle_event(&mut self, map: &NodeMap, event: TreeEvent) {
        match event {
            TreeEvent::AddedToForest(node) => {
                self.info.insert(node, LayoutInfo::default());
            }
            TreeEvent::AddedToParent(node) => {
                let parent = node.parent(map).unwrap();
                self.info[node].size = 1.0;
                self.info[parent].total += 1.0;
            }
            TreeEvent::Copied { src, dest, .. } => {
                self.info.insert(dest, self.info[src]);
            }
            TreeEvent::RemovingFromParent(node) => {
                self.info[node.parent(map).unwrap()].total -= self.info[node].size;
            }
            TreeEvent::RemovedFromForest(node) => {
                self.info.remove(node);
            }
        }
    }

    fn assume_size_of(&mut self, new: NodeId, old: NodeId, map: &NodeMap) {
        assert_eq!(new.parent(map), old.parent(map));
        let parent = new.parent(map).unwrap();
        self.info[parent].total -= self.info[new].size;
        self.info[new].size = core::mem::replace(&mut self.info[old].size, 0.0);
    }

    fn set_kind(&mut self, node: NodeId, kind: LayoutKind) {
        self.info[node].kind = kind;
        if !kind.is_group() {
            self.info[node].last_ungrouped_kind = kind;
        }
    }

    fn kind(&self, node: NodeId) -> LayoutKind { self.info[node].kind }

    fn proportion(&self, map: &NodeMap, node: NodeId) -> Option<f64> {
        let Some(parent) = node.parent(map) else { return None };
        Some(f64::from(self.info[node].size) / f64::from(self.info[parent].total))
    }

    fn take_share(&mut self, map: &NodeMap, node: NodeId, from: NodeId, share: f32) {
        assert_eq!(node.parent(map), from.parent(map));
        let share = share.min(self.info[from].size);
        let share = share.max(-self.info[node].size);
        self.info[from].size -= share;
        self.info[node].size += share;
    }

    fn set_fullscreen(&mut self, node: NodeId, is_fullscreen: bool) {
        self.info[node].is_fullscreen = is_fullscreen;
    }

    fn toggle_fullscreen(&mut self, node: NodeId) -> bool {
        self.info[node].is_fullscreen = !self.info[node].is_fullscreen;
        self.info[node].is_fullscreen
    }

    fn debug(&self, node: NodeId, is_container: bool) -> String {
        let info = &self.info[node];
        if is_container {
            format!("{:?} [size {} total={}]", info.kind, info.size, info.total)
        } else {
            format!("[size {}]", info.size)
        }
    }

    fn is_focused_in_subtree(&self, map: &NodeMap, window: &Window, node: NodeId) -> bool {
        if window.at(node).is_some() {
            if let Some(parent) = node.parent(map) {
                return parent.first_child(map) == Some(node);
            }
        }
        for child in node.children(map) {
            if self.is_focused_in_subtree(map, window, child) {
                return true;
            }
        }
        false
    }

    fn apply_with_gaps(
        &self,
        map: &NodeMap,
        window: &Window,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) {
        let info = &self.info[node];
        let rect = if info.is_fullscreen { screen } else { rect };
        if let Some(wid) = window.at(node) {
            debug_assert!(
                node.children(map).next().is_none(),
                "non-leaf node with window id"
            );
            sizes.push((wid, rect));
            return;
        }
        use LayoutKind::*;
        match info.kind {
            HorizontalStack | VerticalStack => {
                let children: Vec<_> = node.children(map).collect();
                if children.is_empty() {
                    return;
                }
                let is_horizontal = matches!(info.kind, HorizontalStack);
                let reserve = stack_line_thickness.max(0.0);
                let container_rect = adjust_stack_container_rect(
                    rect,
                    is_horizontal,
                    reserve,
                    stack_line_horiz,
                    stack_line_vert,
                );
                let layout = StackLayoutResult::new(
                    container_rect,
                    children.len(),
                    stack_offset,
                    is_horizontal,
                );
                let focused_idx = children
                    .iter()
                    .position(|&c| self.is_focused_in_subtree(map, window, c))
                    .unwrap_or(0);
                for (idx, &child) in children.iter().enumerate() {
                    let frame = if idx == focused_idx {
                        layout.get_focused_frame_for_index(idx, focused_idx)
                    } else {
                        layout.get_frame_for_index(idx)
                    };
                    self.apply_with_gaps(
                        map,
                        window,
                        child,
                        frame,
                        screen,
                        sizes,
                        stack_offset,
                        gaps,
                        stack_line_thickness,
                        stack_line_horiz,
                        stack_line_vert,
                    );
                }
            }
            Horizontal => self.layout_axis(
                map,
                window,
                node,
                rect,
                screen,
                sizes,
                stack_offset,
                gaps,
                true,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
            Vertical => self.layout_axis(
                map,
                window,
                node,
                rect,
                screen,
                sizes,
                stack_offset,
                gaps,
                false,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
        }
    }

    fn layout_axis(
        &self,
        map: &NodeMap,
        window: &Window,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        horizontal: bool,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) {
        use objc2_core_foundation::{CGPoint, CGSize};
        let children: Vec<_> = node.children(map).collect();
        if children.is_empty() {
            return;
        }
        let min_size = 0.05;
        let expected_total = children.len() as f32;
        let mut needs_normalization = false;
        let mut actual_total = 0.0;
        for &child in &children {
            let sz = self.info[child].size;
            actual_total += sz;
            if sz < min_size {
                needs_normalization = true;
            }
        }
        if (actual_total - expected_total).abs() > 0.01 || needs_normalization {
            let share = 1.0;
            unsafe {
                let info = &mut *(&self.info as *const _
                    as *mut slotmap::SecondaryMap<NodeId, LayoutInfo>);
                for &child in &children {
                    info[child].size = share;
                }
                info[node].total = children.len() as f32;
            }
        }
        debug_assert!({
            let sum_children: f32 = children.iter().map(|c| self.info[*c].size).sum();
            (sum_children - self.info[node].total).abs() < 0.01
        });
        let total = self.info[node].total;
        let inner_gap = if horizontal {
            gaps.inner.horizontal
        } else {
            gaps.inner.vertical
        };
        let axis_len = if horizontal {
            rect.size.width
        } else {
            rect.size.height
        };
        let total_gap = (children.len().saturating_sub(1)) as f64 * inner_gap;
        let usable_axis = if inner_gap == 0.0 {
            axis_len
        } else {
            (axis_len - total_gap).max(0.0)
        };
        let mut offset = if horizontal {
            rect.origin.x
        } else {
            rect.origin.y
        };
        for (i, &child) in children.iter().enumerate() {
            let ratio = f64::from(self.info[child].size) / f64::from(total);
            let seg_len = usable_axis * ratio;
            let child_rect = if horizontal {
                CGRect {
                    origin: CGPoint { x: offset, y: rect.origin.y },
                    size: CGSize {
                        width: seg_len,
                        height: rect.size.height,
                    },
                }
            } else {
                CGRect {
                    origin: CGPoint { x: rect.origin.x, y: offset },
                    size: CGSize {
                        width: rect.size.width,
                        height: seg_len,
                    },
                }
            }
            .round();
            self.apply_with_gaps(
                map,
                window,
                child,
                child_rect,
                screen,
                sizes,
                stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            );
            offset += seg_len;
            if i < children.len() - 1 {
                offset += inner_gap;
            }
        }
    }
}

impl Components {
    fn dispatch_event(&mut self, map: &NodeMap, event: TreeEvent) {
        self.selection.handle_event(map, event);
        self.layout.handle_event(map, event);
        self.window.handle_event(map, event);
    }
}

fn adjust_stack_container_rect(
    mut container_rect: CGRect,
    is_horizontal: bool,
    reserve: f64,
    stack_line_horiz: crate::common::config::HorizontalPlacement,
    stack_line_vert: crate::common::config::VerticalPlacement,
) -> CGRect {
    if reserve <= 0.0 {
        return container_rect;
    }
    if is_horizontal {
        let new_h = (container_rect.size.height - reserve).max(0.0);
        if matches!(stack_line_horiz, crate::common::config::HorizontalPlacement::Top) {
            container_rect.origin.y += reserve;
        }
        container_rect.size.height = new_h;
    } else {
        let new_w = (container_rect.size.width - reserve).max(0.0);
        if matches!(stack_line_vert, crate::common::config::VerticalPlacement::Left) {
            container_rect.origin.x += reserve;
        }
        container_rect.size.width = new_w;
    }
    container_rect
}
