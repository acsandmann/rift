use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
use crate::layout_engine::systems::constraints::{AxisConstraints, solve_axis_lengths};
use crate::layout_engine::systems::{LayoutSystem, WindowLayoutConstraints};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::selection::*;
use crate::model::tree::{self, NodeId, NodeMap, OwnedNode, Tree};
use crate::sys::geometry::Round;

#[derive(Serialize, Deserialize, Debug)]
pub struct TraditionalLayoutSystem {
    pub(crate) tree: Tree<Components>,
    pub(crate) layout_roots: slotmap::SlotMap<LayoutId, OwnedNode>,
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
    fn find_best_focus_target(&self, node: NodeId) -> Option<(NodeId, WindowId)> {
        if let Some(wid) = self.tree.data.window.at(node) {
            return Some((node, wid));
        }

        let children: Vec<_> = node.children(self.map()).collect();
        if children.is_empty() {
            return None;
        }

        if let Some(selected) = self
            .tree
            .data
            .selection
            .local_selection(self.map(), node)
            .or_else(|| self.tree.data.selection.last_selection(self.map(), node))
        {
            if let Some(target) = self.find_best_focus_target(selected) {
                return Some(target);
            }
        }

        for &child in &children {
            if let Some(target) = self.find_best_focus_target(child) {
                return Some(target);
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
                self.split_new_sibling_from_selection(selection, node);
                self.tree.data.window.set_window(layout, node, wid);
                return node;
            }
        }

        let node = self.tree.mk_node().insert_after(selection);
        self.split_new_sibling_from_selection(selection, node);
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

    pub(crate) fn root(&self, layout: LayoutId) -> NodeId { self.layout_roots[layout].id() }

    fn selection(&self, layout: LayoutId) -> NodeId {
        self.tree.data.selection.current_selection(self.root(layout))
    }

    pub(crate) fn map(&self) -> &NodeMap { &self.tree.map }

    pub(crate) fn local_selection(&self, node: NodeId) -> Option<NodeId> {
        self.tree
            .data
            .selection
            .local_selection(self.map(), node)
            .or_else(|| self.tree.data.selection.last_selection(self.map(), node))
    }

    pub(crate) fn layout(&self, node: NodeId) -> LayoutKind { self.tree.data.layout.kind(node) }

    pub(crate) fn layouts_for_window(&self, wid: WindowId) -> Vec<LayoutId> {
        self.tree.data.window.layouts_for(wid)
    }

    pub(crate) fn set_layout(&mut self, node: NodeId, kind: LayoutKind) {
        self.tree.data.layout.set_kind(node, kind);
    }

    pub(crate) fn calculate_layout_for_node(
        &self,
        node: NodeId,
        screen: CGRect,
        rect: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let mut sizes = vec![];
        self.tree.data.layout.apply_with_gaps(
            &self.tree.map,
            &self.tree.data.window,
            &self.tree.data.selection,
            node,
            rect,
            screen,
            &mut sizes,
            stack_offset,
            constraints,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        );
        sizes
    }

    fn find_natural_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        if let Some(parent) = from.parent(self.map()) {
            let parent_layout = self.layout(parent);
            if parent_layout.orientation() == direction.orientation() && !parent_layout.is_group() {
                let child_count = parent.children(self.map()).count();
                if child_count == 2 {
                    if let Some(neighbor) = match direction {
                        Direction::Right | Direction::Down => parent.next_sibling(self.map()),
                        Direction::Left | Direction::Up => parent.prev_sibling(self.map()),
                    } {
                        return Some(neighbor);
                    }
                }
                let is_edge = match direction {
                    Direction::Right | Direction::Down => from.next_sibling(self.map()).is_none(),
                    Direction::Left | Direction::Up => from.prev_sibling(self.map()).is_none(),
                };
                if is_edge {
                    let neighbor = match direction {
                        Direction::Right | Direction::Down => parent.next_sibling(self.map()),
                        Direction::Left | Direction::Up => parent.prev_sibling(self.map()),
                    };
                    if let Some(neighbor) = neighbor {
                        return Some(neighbor);
                    }
                }
            }
        }

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

        let selection_stack_parent =
            selection_parent.filter(|&parent| self.layout(parent).is_stacked());
        let target_stack_parent = target_parent.filter(|&parent| self.layout(parent).is_stacked());

        match (selection_stack_parent, target_stack_parent) {
            (Some(stack_parent), None) => {
                let first_child = stack_parent.first_child(self.map());
                let detached = target.detach(&mut self.tree);
                match direction {
                    Direction::Left | Direction::Up => {
                        if let Some(first_child) = first_child {
                            detached.insert_before(first_child);
                        } else {
                            detached.push_back(stack_parent);
                        }
                    }
                    Direction::Right | Direction::Down => {
                        detached.push_back(stack_parent);
                    }
                }
                self.select(stack_parent);
                return;
            }
            (None, Some(stack_parent)) => {
                let first_child = stack_parent.first_child(self.map());
                let detached = selection.detach(&mut self.tree);
                match direction {
                    Direction::Left | Direction::Up => {
                        detached.push_back(stack_parent);
                    }
                    Direction::Right | Direction::Down => {
                        if let Some(first_child) = first_child {
                            detached.insert_before(first_child);
                        } else {
                            detached.push_back(stack_parent);
                        }
                    }
                }
                self.select(stack_parent);
                return;
            }
            _ => {}
        }

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
                let new_layout = if self.layout(common_parent).is_stacked() {
                    self.layout(common_parent)
                } else {
                    container_layout
                };
                self.set_layout(common_parent, new_layout);
                self.select(common_parent);
            }

            (Some(sp), Some(tp)) if self.are_containers_mergeable(sp, tp, direction) => {
                self.merge_compatible_containers(layout, sp, tp, direction);
            }

            _ => {
                let common_parent =
                    self.find_or_create_smart_common_parent(layout, selection, target, direction);
                let container_layout = LayoutKind::from(direction.orientation());
                let new_layout = if self.layout(common_parent).is_stacked() {
                    self.layout(common_parent)
                } else {
                    container_layout
                };
                self.set_layout(common_parent, new_layout);
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
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
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
            &self.tree.data.selection,
            self.root(layout),
            tiling_area,
            screen,
            &mut sizes,
            stack_offset,
            constraints,
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
            let focus_target = self.find_best_focus_target(new_node);
            let Some((focus_node, focus_window)) = focus_target else {
                return (None, vec![]);
            };
            let map = &self.tree.map;
            let mut highest_revealed = focus_node;

            for (node, parent) in focus_node.ancestors_with_parent(map) {
                let Some(parent) = parent else { break };
                let parent_layout = self.layout(parent);
                if self.tree.data.selection.select_locally(map, node) {
                    if parent_layout.is_group() {
                        highest_revealed = node;
                    }
                }
            }
            let raise_windows = self.visible_windows_under_internal(highest_revealed);
            (Some(focus_window), raise_windows)
        } else {
            (None, vec![])
        }
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.window_in_direction_from(self.root(layout), direction)
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let selection = self.selection(layout);
        let node = if selection.parent(self.map()).is_none() {
            // If the root is selected but it already has children, split relative to the
            // root's active child instead of appending a fresh full-weight sibling.
            if let Some(anchor) =
                self.local_selection(selection).or_else(|| selection.last_child(self.map()))
            {
                self.smart_window_insertion(layout, anchor, wid)
            } else {
                self.add_window_under(layout, selection, wid)
            }
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

    fn windows_for_app(&self, layout: LayoutId, pid: pid_t) -> Vec<WindowId> {
        self.root(layout)
            .traverse_postorder(self.map())
            .filter_map(|node| self.window_at(node))
            .filter(|wid| wid.pid == pid)
            .collect()
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
                    if self.tree.data.layout.info[*node].is_fullscreen {
                        current.next();
                    } else {
                        node.detach(&mut self.tree).remove();
                        current.next();
                    }
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
        gaps: &crate::common::config::GapSettings,
    ) {
        if let Some(node) = self.tree.data.window.node_for(layout, wid) {
            if new_frame == screen {
                self.tree.data.layout.set_fullscreen(node, true);
            } else if old_frame == screen {
                self.tree.data.layout.set_fullscreen(node, false);
            } else {
                let tiling = compute_tiling_area(screen, gaps);
                if new_frame == tiling {
                    self.tree.data.layout.set_fullscreen_within_gaps(node, true);
                } else if old_frame == tiling {
                    self.tree.data.layout.set_fullscreen_within_gaps(node, false);
                } else {
                    self.set_frame_from_resize(node, old_frame, new_frame, screen);
                }
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

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        let node = self.selection(layout);
        if self.tree.data.layout.toggle_fullscreen_within_gaps(node) {
            self.visible_windows_under_internal(node)
        } else {
            vec![]
        }
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        let root = self.root(layout);
        root.traverse_preorder(&self.tree.map)
            .any(|node| self.tree.data.layout.is_effectively_fullscreen(node))
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        let mut selection = self.selection(layout);

        if let Some(target) = self.find_natural_join_target(selection, direction) {
            let map = self.map();

            // If the selection is a leaf at the edge of a container that matches the direction,
            // lift the selection to that container so we can merge whole groups.
            if let Some(parent) = selection.parent(map) {
                let parent_layout = self.layout(parent);
                let is_edge = match direction {
                    Direction::Right | Direction::Down => selection.next_sibling(map).is_none(),
                    Direction::Left | Direction::Up => selection.prev_sibling(map).is_none(),
                };

                if parent_layout.orientation() == direction.orientation()
                    && !parent_layout.is_group()
                    && (is_edge || parent.children(map).count() == 2)
                    && target.parent(map) != Some(parent)
                    && !target.ancestors(map).any(|a| a == parent)
                {
                    selection = parent;
                }
            }

            // If the selection is now a container that matches the join orientation,
            // absorb the target into it to avoid creating an extra nesting layer.
            let selection_layout = self.layout(selection);
            let target_is_ancestor = target.ancestors(map).any(|a| a == selection);
            let selection_is_ancestor = selection.ancestors(map).any(|a| a == target);
            if self.window_at(selection).is_none()
                && selection_layout.orientation() == direction.orientation()
                && !selection_layout.is_group()
                && !target_is_ancestor
                && !selection_is_ancestor
                && target.parent(map) != Some(selection)
            {
                match direction {
                    Direction::Right | Direction::Down => {
                        target.detach(&mut self.tree).push_back(selection);
                    }
                    Direction::Left | Direction::Up => {
                        if let Some(first) = selection.first_child(map) {
                            target.detach(&mut self.tree).insert_before(first);
                        } else {
                            target.detach(&mut self.tree).push_back(selection);
                        }
                    }
                }
                self.select(selection);
                return;
            }

            self.perform_natural_join(layout, selection, target, direction);
            if self.tree.data.window.at(selection).is_some() {
                self.select(selection);
            } else {
                let _ = self.descend_selection(layout);
            }
        }
    }

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let selection = self.selection(layout);

        let target_container = if self.tree.data.window.at(selection).is_some() {
            selection.parent(self.map())
        } else {
            Some(selection)
        };

        if let Some(container) = target_container {
            let current_layout = self.layout(container);

            let new_layout = match current_layout {
                LayoutKind::HorizontalStack => Some(LayoutKind::VerticalStack),
                LayoutKind::VerticalStack => Some(LayoutKind::HorizontalStack),
                LayoutKind::Horizontal => match default_orientation {
                    crate::common::config::StackDefaultOrientation::Perpendicular => {
                        Some(LayoutKind::VerticalStack)
                    }
                    crate::common::config::StackDefaultOrientation::Same => {
                        Some(LayoutKind::HorizontalStack)
                    }
                    crate::common::config::StackDefaultOrientation::Horizontal => {
                        Some(LayoutKind::HorizontalStack)
                    }
                    crate::common::config::StackDefaultOrientation::Vertical => {
                        Some(LayoutKind::VerticalStack)
                    }
                },
                LayoutKind::Vertical => match default_orientation {
                    crate::common::config::StackDefaultOrientation::Perpendicular => {
                        Some(LayoutKind::HorizontalStack)
                    }
                    crate::common::config::StackDefaultOrientation::Same => {
                        Some(LayoutKind::VerticalStack)
                    }
                    crate::common::config::StackDefaultOrientation::Horizontal => {
                        Some(LayoutKind::HorizontalStack)
                    }
                    crate::common::config::StackDefaultOrientation::Vertical => {
                        Some(LayoutKind::VerticalStack)
                    }
                },
            };

            if let Some(nl) = new_layout {
                self.set_layout(container, nl);

                if nl.is_stacked() {
                    if let Some(first_child) = container.first_child(self.map()) {
                        self.select(first_child);
                    }
                }

                return self.visible_windows_under_internal(container);
            }
        }

        vec![]
    }

    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
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
                LayoutKind::HorizontalStack => match default_orientation {
                    crate::common::config::StackDefaultOrientation::Perpendicular => {
                        Some(LayoutKind::Vertical)
                    }
                    crate::common::config::StackDefaultOrientation::Same => {
                        Some(LayoutKind::Horizontal)
                    }
                    crate::common::config::StackDefaultOrientation::Horizontal => {
                        Some(LayoutKind::Horizontal)
                    }
                    crate::common::config::StackDefaultOrientation::Vertical => {
                        Some(LayoutKind::Vertical)
                    }
                },
                LayoutKind::VerticalStack => match default_orientation {
                    crate::common::config::StackDefaultOrientation::Perpendicular => {
                        Some(LayoutKind::Horizontal)
                    }
                    crate::common::config::StackDefaultOrientation::Same => {
                        Some(LayoutKind::Vertical)
                    }
                    crate::common::config::StackDefaultOrientation::Horizontal => {
                        Some(LayoutKind::Horizontal)
                    }
                    crate::common::config::StackDefaultOrientation::Vertical => {
                        Some(LayoutKind::Vertical)
                    }
                },
                _ => None,
            };

            if let Some(nl) = new_layout {
                self.set_layout(container, nl);
                return self.visible_windows_under_internal(container);
            }
        }

        vec![]
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        let selection = self.selection(layout);

        if self.tree.data.window.at(selection).is_some() {
            let map = self.map();
            return selection
                .ancestors(map)
                .skip(1)
                .any(|ancestor| self.layout(ancestor).is_stacked());
        }

        if self.layout(selection).is_stacked() {
            return true;
        }

        let map = self.map();
        selection.children(map).any(|child| self.layout(child).is_stacked())
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        let selection = self.selection(layout);

        if let Some(parent) = selection.parent(&self.tree.map) {
            if let Some(grandparent) = parent.parent(&self.tree.map) {
                let children: Vec<_> = parent.children(&self.tree.map).collect();
                if children.is_empty() {
                    return;
                }

                let local_selected_child =
                    self.tree.data.selection.local_selection(&self.tree.map, parent);
                let next_sibling = parent.next_sibling(&self.tree.map);

                for child in children.iter() {
                    let detached = child.detach(&mut self.tree);
                    if let Some(next_sibling) = next_sibling {
                        detached.insert_before(next_sibling);
                    } else {
                        detached.push_back(grandparent);
                    }
                }

                parent.detach(&mut self.tree).remove();

                if let Some(sel_child) = local_selected_child {
                    self.select(sel_child);
                } else if let Some(first_child) = grandparent.first_child(&self.tree.map) {
                    self.select(first_child);
                }
            } else {
                let children: Vec<_> = parent.children(&self.tree.map).collect();
                if children.len() == 2 {
                    self.remove_unnecessary_container_internal(parent);
                }
            }
        }
    }

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        if amount == 0.0 {
            return;
        }
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

            for direction in [
                crate::layout_engine::Direction::Right,
                crate::layout_engine::Direction::Down,
                crate::layout_engine::Direction::Left,
                crate::layout_engine::Direction::Up,
            ] {
                if candidates.iter().any(|&node| self.resize_internal(node, amount, direction)) {
                    break;
                }
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

    fn toggle_tile_orientation(&mut self, layout: LayoutId) {
        use crate::layout_engine::LayoutKind;

        let map = self.map();
        let selection_node = self.selection(layout);

        let target_node = match selection_node.parent(map) {
            Some(p) => p,
            None => self.root(layout),
        };

        let current_kind = self.layout(target_node);

        if current_kind.is_group() {
            return;
        }

        let new_kind = match current_kind {
            LayoutKind::Horizontal => LayoutKind::Vertical,
            LayoutKind::Vertical => LayoutKind::Horizontal,
            other => other,
        };

        self.set_layout(target_node, new_kind);

        self.rebalance(layout);
    }
}

impl TraditionalLayoutSystem {
    fn split_new_sibling_from_selection(&mut self, selection: NodeId, new_sibling: NodeId) {
        let map = &self.tree.map;
        let Some(parent) = selection.parent(map) else {
            return;
        };
        if new_sibling.parent(map) != Some(parent) {
            return;
        }

        let selected_size = self.tree.data.layout.info[selection].size.max(0.0);
        if selected_size <= f32::EPSILON {
            // Recover from stale/uninitialized size metadata by forcing an even split.
            self.tree.data.layout.info[selection].size = 1.0;
            self.tree.data.layout.info[new_sibling].size = 1.0;
        } else {
            self.tree.data.layout.info[selection].size = selected_size * 0.5;
            self.tree.data.layout.info[new_sibling].size = selected_size * 0.5;
        }

        let total: f32 = parent
            .children(map)
            .map(|child| self.tree.data.layout.info[child].size.max(0.0))
            .sum();
        self.tree.data.layout.info[parent].total = total;
    }

    fn stack_group_container_info(
        &self,
        node: NodeId,
        kind: crate::layout_engine::LayoutKind,
        rect: CGRect,
        children: &[NodeId],
        selected_index: usize,
    ) -> crate::layout_engine::engine::GroupContainerInfo {
        let ui_selected_index = if matches!(kind, crate::layout_engine::LayoutKind::VerticalStack) {
            children.len().saturating_sub(1).saturating_sub(selected_index)
        } else {
            selected_index
        };

        let mut window_ids =
            children.iter().filter_map(|&child| self.window_at(child)).collect::<Vec<_>>();
        if matches!(kind, crate::layout_engine::LayoutKind::VerticalStack) {
            window_ids.reverse();
        }

        crate::layout_engine::engine::GroupContainerInfo {
            node_id: node,
            container_kind: kind,
            frame: rect,
            total_count: children.len(),
            selected_index: ui_selected_index,
            window_ids,
        }
    }

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
        use crate::layout_engine::LayoutKind::*;

        let mut out = Vec::new();
        let map = &self.tree.map;

        let tiling_area = compute_tiling_area(screen, gaps);

        let mut node = self.root(layout);
        let mut rect = tiling_area;

        loop {
            if self.tree.data.layout.is_effectively_fullscreen(node) {
                out.clear();
                break;
            }

            let kind = self.tree.data.layout.kind(node);
            let children: Vec<_> = node.children(map).collect();

            if matches!(kind, HorizontalStack | VerticalStack) {
                if children.is_empty() {
                    break;
                }

                let local_sel =
                    self.tree.data.selection.local_selection(map, node).unwrap_or(children[0]);
                let selected_index = children.iter().position(|&c| c == local_sel).unwrap_or(0);

                if self.tree.data.layout.is_effectively_fullscreen(local_sel) {
                    out.clear();
                    break;
                }

                let is_horizontal = matches!(kind, HorizontalStack);
                out.push(self.stack_group_container_info(
                    node,
                    kind,
                    rect,
                    &children,
                    selected_index,
                ));

                let layout_res = stack_layout_result(
                    rect,
                    children.len(),
                    stack_offset,
                    is_horizontal,
                    stack_line_thickness,
                    stack_line_horiz,
                    stack_line_vert,
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
                rect = self.calculate_child_frame_in_container(node, next, rect, gaps);
                node = next;
                continue;
            }
            break;
        }

        out
    }

    pub(crate) fn collect_group_containers(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        use crate::layout_engine::LayoutKind::*;

        let mut out = Vec::new();
        let map = &self.tree.map;

        let tiling_area = compute_tiling_area(screen, gaps);

        let mut stack: Vec<(NodeId, CGRect)> = vec![(self.root(layout), tiling_area)];

        while let Some((node, rect)) = stack.pop() {
            if self.tree.data.layout.is_effectively_fullscreen(node) {
                continue;
            }

            let kind = self.tree.data.layout.kind(node);
            let children: Vec<_> = node.children(map).collect();

            if matches!(kind, HorizontalStack | VerticalStack) {
                if children.is_empty() {
                    continue;
                }

                let local_sel =
                    self.tree.data.selection.local_selection(map, node).unwrap_or(children[0]);
                let selected_index = children.iter().position(|&c| c == local_sel).unwrap_or(0);

                let is_horizontal = matches!(kind, HorizontalStack);
                out.push(self.stack_group_container_info(
                    node,
                    kind,
                    rect,
                    &children,
                    selected_index,
                ));

                let layout_res = stack_layout_result(
                    rect,
                    children.len(),
                    stack_offset,
                    is_horizontal,
                    stack_line_thickness,
                    stack_line_horiz,
                    stack_line_vert,
                );

                for (i, &child) in children.iter().enumerate().rev() {
                    if self.tree.data.layout.is_effectively_fullscreen(child) {
                        continue;
                    }
                    let child_rect = layout_res.get_focused_frame_for_index(i, i);
                    stack.push((child, child_rect));
                }

                continue;
            }

            if !children.is_empty() {
                for &child in children.iter().rev() {
                    let child_rect =
                        self.calculate_child_frame_in_container(node, child, rect, gaps);
                    stack.push((child, child_rect));
                }
            }
        }

        out
    }

    fn calculate_child_frame_in_axis(
        &self,
        parent_rect: CGRect,
        siblings: &[NodeId],
        child_index: usize,
        horizontal: bool,
        gaps: &crate::common::config::GapSettings,
    ) -> CGRect {
        use objc2_core_foundation::{CGPoint, CGSize};

        if siblings.is_empty() || child_index >= siblings.len() {
            return parent_rect;
        }

        let total: f32 = siblings.iter().map(|&child| self.tree.data.layout.info[child].size).sum();
        let inner_gap = if horizontal {
            gaps.inner.horizontal
        } else {
            gaps.inner.vertical
        };

        let axis_len = if horizontal {
            parent_rect.size.width
        } else {
            parent_rect.size.height
        };

        let total_gap = (siblings.len().saturating_sub(1)) as f64 * inner_gap;
        let usable_axis = if inner_gap == 0.0 {
            axis_len
        } else {
            (axis_len - total_gap).max(0.0)
        };

        let mut offset = if horizontal {
            parent_rect.origin.x
        } else {
            parent_rect.origin.y
        };

        for i in 0..child_index {
            let ratio = f64::from(self.tree.data.layout.info[siblings[i]].size) / f64::from(total);
            let seg_len = usable_axis * ratio;
            offset += seg_len;
            if i < siblings.len() - 1 {
                offset += inner_gap;
            }
        }

        let ratio =
            f64::from(self.tree.data.layout.info[siblings[child_index]].size) / f64::from(total);
        let seg_len = usable_axis * ratio;

        if horizontal {
            CGRect::new(
                CGPoint::new(offset, parent_rect.origin.y),
                CGSize::new(seg_len, parent_rect.size.height),
            )
        } else {
            CGRect::new(
                CGPoint::new(parent_rect.origin.x, offset),
                CGSize::new(parent_rect.size.width, seg_len),
            )
        }
    }

    fn calculate_child_frame_in_container(
        &self,
        parent_node: NodeId,
        child_node: NodeId,
        parent_rect: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> CGRect {
        let parent_kind = self.tree.data.layout.kind(parent_node);
        let map = &self.tree.map;
        let siblings: Vec<_> = parent_node.children(map).collect();
        let child_index = siblings.iter().position(|&n| n == child_node).unwrap_or(0);

        match parent_kind {
            crate::layout_engine::LayoutKind::Horizontal => {
                self.calculate_child_frame_in_axis(parent_rect, &siblings, child_index, true, gaps)
            }
            crate::layout_engine::LayoutKind::Vertical => {
                self.calculate_child_frame_in_axis(parent_rect, &siblings, child_index, false, gaps)
            }
            crate::layout_engine::LayoutKind::HorizontalStack
            | crate::layout_engine::LayoutKind::VerticalStack => parent_rect,
        }
    }
}

impl TraditionalLayoutSystem {
    fn get_ascii_tree(&self, node: NodeId) -> ascii_tree::Tree {
        self.get_ascii_tree_with_labels(node, None)
    }

    fn get_ascii_tree_with_labels(
        &self,
        node: NodeId,
        labels: Option<&std::collections::HashMap<NodeId, &'static str>>,
    ) -> ascii_tree::Tree {
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
        let desc = if let Some(label) = labels.and_then(|labels| labels.get(&node).copied()) {
            format!("{desc} [{label}]")
        } else {
            desc
        };
        let children: Vec<_> = node
            .children(&self.tree.map)
            .map(|c| self.get_ascii_tree_with_labels(c, labels))
            .collect();
        if children.is_empty() {
            ascii_tree::Tree::Leaf(vec![desc])
        } else {
            ascii_tree::Tree::Node(desc, children)
        }
    }

    pub(crate) fn add_window_under(
        &mut self,
        layout: LayoutId,
        parent: NodeId,
        wid: WindowId,
    ) -> NodeId {
        let node = self.tree.mk_node().push_back(parent);
        self.tree.data.window.set_window(layout, node, wid);
        node
    }

    pub(crate) fn window_at(&self, node: NodeId) -> Option<WindowId> {
        self.tree.data.window.at(node)
    }

    pub(crate) fn visible_windows_in_subtree(&self, node: NodeId) -> Vec<WindowId> {
        self.visible_windows_under_internal(node)
    }

    pub(crate) fn draw_tree_with_labels(
        &self,
        layout: LayoutId,
        labels: &std::collections::HashMap<NodeId, &'static str>,
    ) -> String {
        let tree = self.get_ascii_tree_with_labels(self.root(layout), Some(labels));
        let mut out = String::new();
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
    }

    fn window_in_direction_from(&self, node: NodeId, direction: Direction) -> Option<WindowId> {
        if let Some(window) = self.window_at(node) {
            return Some(window);
        }

        let mut children: Vec<_> = node.children(self.map()).collect();
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

    fn rebalance_node(&mut self, node: NodeId) {
        let map = &self.tree.map;
        let children: Vec<_> = node.children(map).collect();
        let count = children.len() as f32;
        if count == 0.0 {
            return;
        }
        self.tree.data.layout.info[node].total = count;
        for &child in &children {
            self.tree.data.layout.info[child].size = 1.0;
        }
        for child in children {
            self.rebalance_node(child);
        }
    }

    pub(crate) fn select(&mut self, selection: NodeId) {
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
                (LayoutKind::Horizontal, Direction::Left) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.first().copied()),
                (LayoutKind::Horizontal, Direction::Right) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.last().copied()),
                (LayoutKind::Horizontal, Direction::Up) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.first().copied()),
                (LayoutKind::Horizontal, Direction::Down) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.last().copied()),
                (LayoutKind::Vertical, Direction::Up) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.first().copied()),
                (LayoutKind::Vertical, Direction::Down) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.last().copied()),
                (LayoutKind::Vertical, Direction::Left) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.first().copied()),
                (LayoutKind::Vertical, Direction::Right) => self
                    .tree
                    .data
                    .selection
                    .local_selection(map, current)
                    .or(children.last().copied()),
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
                let children: Vec<_> = node.children(self.map()).collect();
                for child in children.into_iter().rev() {
                    stack.push(child);
                }
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
            None
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

    fn can_resize_towards(&self, node: NodeId, direction: Direction) -> bool {
        let can_resize = |candidate: NodeId| -> bool {
            let Some(parent) = candidate.parent(&self.tree.map) else {
                return false;
            };
            !self.tree.data.layout.kind(parent).is_group()
                && self.move_over(candidate, direction).is_some()
        };
        node.ancestors(&self.tree.map).any(can_resize)
    }

    fn set_frame_from_resize(
        &mut self,
        node: NodeId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
    ) {
        const RESIZE_DELTA_EPSILON: f64 = 1.0;
        let mut check_or_resize = |resize: bool| {
            let mut count = 0;
            let mut first_direction: Option<Direction> = None;
            let mut good = true;
            let mut left_delta = old_frame.min().x - new_frame.min().x;
            let mut right_delta = new_frame.max().x - old_frame.max().x;
            let mut up_delta = old_frame.min().y - new_frame.min().y;
            let mut down_delta = new_frame.max().y - old_frame.max().y;

            if left_delta.abs() < RESIZE_DELTA_EPSILON {
                left_delta = 0.0;
            }
            if right_delta.abs() < RESIZE_DELTA_EPSILON {
                right_delta = 0.0;
            }
            if up_delta.abs() < RESIZE_DELTA_EPSILON {
                up_delta = 0.0;
            }
            if down_delta.abs() < RESIZE_DELTA_EPSILON {
                down_delta = 0.0;
            }

            let mut effective = Vec::new();

            if left_delta != 0.0 || right_delta != 0.0 {
                let (primary_dir, primary_delta, secondary_dir, secondary_delta) =
                    if left_delta.abs() >= right_delta.abs() {
                        (Direction::Left, left_delta, Direction::Right, right_delta)
                    } else {
                        (Direction::Right, right_delta, Direction::Left, left_delta)
                    };
                let chosen = if primary_delta != 0.0 && self.can_resize_towards(node, primary_dir) {
                    Some((primary_dir, primary_delta))
                } else if secondary_delta != 0.0 && self.can_resize_towards(node, secondary_dir) {
                    Some((secondary_dir, secondary_delta))
                } else {
                    None
                };
                if let Some((direction, delta)) = chosen {
                    effective.push((direction, delta, screen.size.width));
                }
            }

            if up_delta != 0.0 || down_delta != 0.0 {
                let (primary_dir, primary_delta, secondary_dir, secondary_delta) =
                    if up_delta.abs() >= down_delta.abs() {
                        (Direction::Up, up_delta, Direction::Down, down_delta)
                    } else {
                        (Direction::Down, down_delta, Direction::Up, up_delta)
                    };
                let chosen = if primary_delta != 0.0 && self.can_resize_towards(node, primary_dir) {
                    Some((primary_dir, primary_delta))
                } else if secondary_delta != 0.0 && self.can_resize_towards(node, secondary_dir) {
                    Some((secondary_dir, secondary_delta))
                } else {
                    None
                };
                if let Some((direction, delta)) = chosen {
                    effective.push((direction, delta, screen.size.height));
                }
            }

            for (direction, delta, whole) in effective {
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
                    let ratio = f64::from(delta) / f64::from(whole);
                    let _ = self.resize_internal(node, ratio, direction);
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
        let map = self.map();

        if node1 == node2 {
            return node1;
        }

        if node1.ancestors(map).any(|ancestor| ancestor == node2) {
            return node2;
        }

        if node2.ancestors(map).any(|ancestor| ancestor == node1) {
            return node1;
        }

        let parent1 = node1.parent(self.map());
        let parent2 = node2.parent(self.map());
        if let (Some(p1), Some(p2)) = (parent1, parent2) {
            if p1 == p2 {
                let size1 = self.tree.data.layout.info[node1].size.max(0.0);
                let size2 = self.tree.data.layout.info[node2].size.max(0.0);
                let new_container = self.tree.mk_node().insert_before(node1);
                node1.detach(&mut self.tree).push_back(new_container).with(|child, tree| {
                    tree.data.layout.info[child].size = size1;
                });
                node2.detach(&mut self.tree).push_back(new_container).with(|child, tree| {
                    tree.data.layout.info[child].size = size2;
                });
                self.tree.data.layout.info[new_container].size = size1 + size2;
                self.tree.data.layout.info[new_container].total = size1 + size2;
                self.tree.data.layout.info[p1].total = p1
                    .children(&self.tree.map)
                    .map(|child| self.tree.data.layout.info[child].size.max(0.0))
                    .sum();
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
        match children.as_slice() {
            [] => {
                if container.parent(self.map()).is_some() {
                    container.detach(&mut self.tree).remove();
                }
            }
            [child] => {
                if container.parent(self.map()).is_some() {
                    child.detach(&mut self.tree).insert_after(container).with(|child_id, tree| {
                        tree.data.layout.assume_size_of(child_id, container, &tree.map)
                    });
                }
            }
            _ => {}
        }
    }
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub(crate) struct Components {
    selection: Selection,
    pub(crate) layout: Layout,
    pub(crate) window: WindowIndex,
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

#[derive(Default, Serialize, Deserialize, Debug)]
pub(crate) struct WindowIndex {
    windows: slotmap::SecondaryMap<NodeId, WindowId>,
    window_nodes: crate::common::collections::BTreeMap<WindowId, WindowNodeInfoVec>,
}

#[derive(Serialize, Deserialize, Debug)]
struct WindowNodeInfo {
    layout: LayoutId,
    node: NodeId,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct WindowNodeInfoVec(Vec<WindowNodeInfo>);

impl WindowIndex {
    pub(crate) fn at(&self, node: NodeId) -> Option<WindowId> { self.windows.get(node).copied() }

    fn layouts_for(&self, wid: WindowId) -> Vec<LayoutId> {
        self.window_nodes
            .get(&wid)
            .map(|nodes| nodes.0.iter().map(|info| info.layout).collect())
            .unwrap_or_default()
    }

    pub(crate) fn node_for(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.window_nodes.get(&wid).and_then(|nodes| {
            nodes.0.iter().find(|info| info.layout == layout).map(|info| info.node)
        })
    }

    pub(crate) fn set_window(&mut self, layout: LayoutId, node: NodeId, wid: WindowId) {
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
        let container = &self.container_rect;
        let width = self.window_width.min(container.size.width);
        let height = self.window_height.min(container.size.height);
        let min_x = container.origin.x;
        let max_x = (container.origin.x + container.size.width - width).max(min_x);
        let min_y = container.origin.y;
        let max_y = (container.origin.y + container.size.height - height).max(min_y);
        CGRect {
            origin: CGPoint {
                x: (container.origin.x + x_offset).clamp(min_x, max_x),
                y: (container.origin.y + y_offset).clamp(min_y, max_y),
            },
            size: CGSize { width, height },
        }
        .round()
    }

    fn get_focused_frame_for_index(&self, index: usize, _focused_idx: usize) -> CGRect {
        self.get_frame_for_index(index)
    }
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub(crate) struct Layout {
    pub(crate) info: slotmap::SecondaryMap<NodeId, LayoutInfo>,
}

#[allow(unused)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LayoutInfo {
    pub(crate) size: f32,
    pub(crate) total: f32,
    kind: LayoutKind,
    last_ungrouped_kind: LayoutKind,
    #[serde(default)]
    pub(crate) is_fullscreen: bool,
    #[serde(default)]
    is_fullscreen_within_gaps: bool,
}

impl Layout {
    fn effective_leaf_axis_constraints(
        &self,
        c: WindowLayoutConstraints,
        horizontal: bool,
    ) -> (f64, Option<f64>, Option<f64>, bool) {
        let c = c.normalized();
        let min = c.min_for_axis(horizontal);
        let max = (c.max_for_axis(horizontal) > 0.0).then(|| c.max_for_axis(horizontal));
        if c.is_resizable {
            if let Some(fixed) = c.fixed_for_axis(horizontal) {
                return (min, Some(fixed), Some(fixed), false);
            }
            // Some apps report transient or overly conservative min/max bounds while still
            // being user-resizable. In traditional tiling, honoring those bounds at split
            // time causes visibly uneven insertion (e.g. 2:1 right after a 50/50 split).
            return (0.0, None, max, true);
        }
        let fixed = c.fixed_for_axis(horizontal).filter(|v| *v > 0.0);
        let can_grow = fixed.is_none();
        (min, fixed, max.or(fixed), can_grow)
    }

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
        const MIN_NODE_SIZE: f32 = 0.05;
        let share = share.min(self.info[from].size - MIN_NODE_SIZE);
        let share = share.max(MIN_NODE_SIZE - self.info[node].size);
        self.info[from].size -= share;
        self.info[node].size += share;
        let parent = node.parent(map).unwrap();
        let children: Vec<_> = parent.children(map).collect();
        let total: f32 = children.iter().map(|&child| self.info[child].size.max(0.0)).sum();
        if total > f32::EPSILON && total.is_finite() {
            let target_total = children.len() as f32;
            let scale = target_total / total;
            for child in children {
                self.info[child].size *= scale;
            }
            self.info[parent].total = target_total;
        }
    }

    fn set_fullscreen(&mut self, node: NodeId, is_fullscreen: bool) {
        self.info[node].is_fullscreen = is_fullscreen;
        if is_fullscreen {
            self.info[node].is_fullscreen_within_gaps = false;
        }
    }

    fn set_fullscreen_within_gaps(&mut self, node: NodeId, within: bool) {
        self.info[node].is_fullscreen_within_gaps = within;
        if within {
            self.info[node].is_fullscreen = false;
        }
    }

    fn toggle_fullscreen(&mut self, node: NodeId) -> bool {
        self.info[node].is_fullscreen = !self.info[node].is_fullscreen;
        if self.info[node].is_fullscreen {
            self.info[node].is_fullscreen_within_gaps = false;
        }
        self.info[node].is_fullscreen
    }

    fn toggle_fullscreen_within_gaps(&mut self, node: NodeId) -> bool {
        self.info[node].is_fullscreen_within_gaps = !self.info[node].is_fullscreen_within_gaps;
        if self.info[node].is_fullscreen_within_gaps {
            self.info[node].is_fullscreen = false;
        }
        self.info[node].is_fullscreen_within_gaps
    }

    fn is_effectively_fullscreen(&self, node: NodeId) -> bool {
        let info = &self.info[node];
        info.is_fullscreen || info.is_fullscreen_within_gaps
    }

    fn debug(&self, node: NodeId, is_container: bool) -> String {
        let info = &self.info[node];
        if is_container {
            format!("{:?} [size {} total={}]", info.kind, info.size, info.total)
        } else {
            format!("[size {}]", info.size)
        }
    }

    fn node_axis_constraints(
        &self,
        map: &NodeMap,
        window: &WindowIndex,
        selection: &Selection,
        node: NodeId,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        stack_offset: f64,
        stack_line_thickness: f64,
        gaps: &crate::common::config::GapSettings,
        horizontal: bool,
    ) -> (f64, Option<f64>, Option<f64>, bool) {
        if let Some(wid) = window.at(node) {
            if let Some(c) = constraints.get(&wid).copied() {
                return self.effective_leaf_axis_constraints(c, horizontal);
            }
            return (0.0, None, None, true);
        }

        let children: Vec<_> = node.children(map).collect();
        if children.is_empty() {
            return (0.0, None, None, true);
        }

        let kind = self.info[node].kind;
        let axis_aligned = matches!(
            (kind, horizontal),
            (LayoutKind::Horizontal, true)
                | (LayoutKind::HorizontalStack, true)
                | (LayoutKind::Vertical, false)
                | (LayoutKind::VerticalStack, false)
        );
        let inner_gap = if horizontal {
            gaps.inner.horizontal
        } else {
            gaps.inner.vertical
        };

        let mut mins = Vec::with_capacity(children.len());
        let mut fixed_parts = Vec::with_capacity(children.len());
        let mut max_parts = Vec::with_capacity(children.len());
        let mut grows = Vec::with_capacity(children.len());
        let mut any_grow = false;
        for child in &children {
            let (min, fixed, max, can_grow) = self.node_axis_constraints(
                map,
                window,
                selection,
                *child,
                constraints,
                stack_offset,
                stack_line_thickness,
                gaps,
                horizontal,
            );
            mins.push(min.max(0.0));
            fixed_parts.push(fixed.map(|v| v.max(0.0)));
            max_parts.push(max.map(|v| v.max(0.0)));
            grows.push(can_grow);
            any_grow |= can_grow;
        }

        if children.len() == 1 {
            let reserve = if matches!(kind, LayoutKind::HorizontalStack | LayoutKind::VerticalStack)
                && matches!(
                    (kind, horizontal),
                    (LayoutKind::HorizontalStack, false) | (LayoutKind::VerticalStack, true)
                ) {
                stack_line_thickness.max(0.0)
            } else {
                0.0
            };
            return (
                mins[0] + reserve,
                fixed_parts[0].map(|value| value + reserve),
                max_parts[0].map(|value| value + reserve),
                grows.first().copied().unwrap_or(true),
            );
        }

        let stack_span = stack_offset.max(0.0) * (children.len().saturating_sub(1) as f64);
        let min_max = mins.iter().copied().fold(0.0_f64, |acc, value| acc.max(value));

        if matches!(kind, LayoutKind::HorizontalStack | LayoutKind::VerticalStack) {
            let stacked_on_axis = matches!(
                (kind, horizontal),
                (LayoutKind::HorizontalStack, true) | (LayoutKind::VerticalStack, false)
            );
            if stacked_on_axis {
                let required_focus = mins
                    .iter()
                    .copied()
                    .zip(fixed_parts.iter().copied())
                    .fold(0.0_f64, |acc, (min, fixed)| acc.max(fixed.unwrap_or(min)));
                let max_focus =
                    max_parts.iter().copied().try_fold(0.0_f64, |acc, part| match part {
                        Some(value) => Some(acc.max(value)),
                        None => None,
                    });
                return (
                    required_focus + stack_span,
                    None,
                    max_focus.map(|value| value + stack_span),
                    any_grow,
                );
            }
            let reserve = stack_line_thickness.max(0.0);
            let fixed_max = fixed_parts.iter().copied().try_fold(0.0, |acc, part| match part {
                Some(value) => Some(if value > acc { value } else { acc }),
                None => None,
            });
            let max_max = max_parts.iter().copied().try_fold(0.0_f64, |acc, part| match part {
                Some(value) => Some(acc.max(value)),
                None => None,
            });
            return (
                min_max + reserve,
                fixed_max.map(|value| value + reserve),
                max_max.map(|value| value + reserve),
                any_grow,
            );
        }

        if axis_aligned {
            let gap_total = inner_gap * (children.len().saturating_sub(1) as f64);
            let min_total = mins.iter().sum::<f64>() + gap_total;
            let fixed_total =
                fixed_parts.iter().copied().try_fold(0.0, |acc, part| part.map(|p| acc + p));
            let max_total =
                max_parts.iter().copied().try_fold(0.0, |acc, part| part.map(|p| acc + p));
            (
                min_total,
                fixed_total.map(|v| v + gap_total),
                max_total.map(|v| v + gap_total),
                any_grow,
            )
        } else {
            let fixed_max = fixed_parts.into_iter().try_fold(0.0, |acc, part| match part {
                Some(value) => Some(if value > acc { value } else { acc }),
                None => None,
            });
            let max_max = max_parts.into_iter().try_fold(0.0_f64, |acc, part| match part {
                Some(value) => Some(acc.max(value)),
                None => None,
            });
            (min_max, fixed_max, max_max, any_grow)
        }
    }

    fn apply_with_gaps(
        &self,
        map: &NodeMap,
        window: &WindowIndex,
        selection: &Selection,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) {
        let info = &self.info[node];
        let rect = if info.is_fullscreen {
            screen
        } else if info.is_fullscreen_within_gaps {
            compute_tiling_area(screen, gaps)
        } else {
            rect
        };
        if let Some(wid) = window.at(node) {
            debug_assert!(
                node.children(map).next().is_none(),
                "non-leaf node with window id"
            );
            let mut final_rect = rect;
            if let Some(c) = constraints.get(&wid).copied() {
                let (min_w, fixed_w, max_w, _) = self.effective_leaf_axis_constraints(c, true);
                let (min_h, fixed_h, max_h, _) = self.effective_leaf_axis_constraints(c, false);
                let desired_w = fixed_w.unwrap_or(rect.size.width).max(min_w);
                let desired_h = fixed_h.unwrap_or(rect.size.height).max(min_h);
                let desired_w = max_w.map_or(desired_w, |m| desired_w.min(m));
                let desired_h = max_h.map_or(desired_h, |m| desired_h.min(m));
                final_rect.size.width = desired_w.min(rect.size.width).max(0.0);
                final_rect.size.height = desired_h.min(rect.size.height).max(0.0);
            }
            sizes.push((wid, final_rect));
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
                let focused_child =
                    selection.local_selection(map, node).unwrap_or_else(|| children[0]);
                let focused_idx = children.iter().position(|&c| c == focused_child).unwrap_or(0);
                let effective_stack_offset = if children.len() > 1 {
                    let focused_child = children[focused_idx];
                    let (focus_min, focus_fixed, _focus_max, _) = self.node_axis_constraints(
                        map,
                        window,
                        selection,
                        focused_child,
                        constraints,
                        stack_offset,
                        stack_line_thickness,
                        gaps,
                        is_horizontal,
                    );
                    let axis_len = if is_horizontal {
                        rect.size.width
                    } else {
                        rect.size.height
                    };
                    // Stack offset capping exists to preserve required focused size.
                    // A max-only cap is not a required reservation and should not shrink
                    // the stack slot or reduce offset budget.
                    let desired = focus_fixed.unwrap_or(focus_min).clamp(0.0, axis_len.max(0.0));
                    let max_offset = (axis_len - desired).max(0.0) / (children.len() - 1) as f64;
                    stack_offset.min(max_offset)
                } else {
                    stack_offset
                };
                let layout = stack_layout_result(
                    rect,
                    children.len(),
                    effective_stack_offset,
                    is_horizontal,
                    stack_line_thickness,
                    stack_line_horiz,
                    stack_line_vert,
                );
                for (idx, &child) in children.iter().enumerate() {
                    let frame = if idx == focused_idx {
                        layout.get_focused_frame_for_index(idx, focused_idx)
                    } else {
                        layout.get_frame_for_index(idx)
                    };
                    self.apply_with_gaps(
                        map,
                        window,
                        selection,
                        child,
                        frame,
                        screen,
                        sizes,
                        stack_offset,
                        constraints,
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
                selection,
                node,
                rect,
                screen,
                sizes,
                stack_offset,
                constraints,
                gaps,
                true,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
            Vertical => self.layout_axis(
                map,
                window,
                selection,
                node,
                rect,
                screen,
                sizes,
                stack_offset,
                constraints,
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
        window: &WindowIndex,
        selection: &Selection,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
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
            if !sz.is_finite() || sz < min_size - f32::EPSILON {
                needs_normalization = true;
            }
        }
        if !actual_total.is_finite()
            || actual_total <= f32::EPSILON
            || needs_normalization
            || (actual_total - expected_total).abs() > 0.01
        {
            unsafe {
                let info = &mut *(&self.info as *const _
                    as *mut slotmap::SecondaryMap<NodeId, LayoutInfo>);
                for &child in &children {
                    info[child].size = 1.0;
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
        let axis_constraints: Vec<AxisConstraints> = children
            .iter()
            .map(|&child| {
                let (min, fixed, max, can_grow) = self.node_axis_constraints(
                    map,
                    window,
                    selection,
                    child,
                    constraints,
                    stack_offset,
                    stack_line_thickness,
                    gaps,
                    horizontal,
                );
                AxisConstraints {
                    min,
                    fixed,
                    max,
                    weight: f64::from(self.info[child].size.max(0.0)),
                    can_grow,
                }
            })
            .collect();
        let seg_lens = solve_axis_lengths(&axis_constraints, usable_axis);
        for (i, &child) in children.iter().enumerate() {
            let fallback = {
                let ratio = f64::from(self.info[child].size) / f64::from(total);
                usable_axis * ratio
            };
            let seg_len = seg_lens.get(i).copied().unwrap_or(fallback.max(0.0));
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
                selection,
                child,
                child_rect,
                screen,
                sizes,
                stack_offset,
                constraints,
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

fn stack_layout_result(
    rect: CGRect,
    child_count: usize,
    stack_offset: f64,
    is_horizontal: bool,
    stack_line_thickness: f64,
    stack_line_horiz: crate::common::config::HorizontalPlacement,
    stack_line_vert: crate::common::config::VerticalPlacement,
) -> StackLayoutResult {
    let reserve = stack_line_thickness.max(0.0);
    let container_rect = adjust_stack_container_rect(
        rect,
        is_horizontal,
        reserve,
        stack_line_horiz,
        stack_line_vert,
    );
    StackLayoutResult::new(container_rect, child_count, stack_offset, is_horizontal)
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

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::layout_engine::{Direction, LayoutKind};

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn window_in_direction_prefers_leftmost_when_moving_right() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));

        assert_eq!(system.window_in_direction(layout, Direction::Right), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Left), Some(w(2)));
    }

    #[test]
    fn window_in_direction_prefers_top_for_down_direction_after_orientation_toggle() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.toggle_tile_orientation(layout);

        assert_eq!(system.window_in_direction(layout, Direction::Down), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Up), Some(w(2)));
    }

    struct TestTraditionalLayoutSystem {
        system: TraditionalLayoutSystem,
        _root: OwnedNode,
        root_id: NodeId,
    }

    impl TestTraditionalLayoutSystem {
        fn new() -> Self {
            let mut system = TraditionalLayoutSystem::default();
            let root = OwnedNode::new_root_in(&mut system.tree, "test_root");
            let root_id = *root;
            system.tree.data.layout.set_kind(root_id, LayoutKind::Horizontal);
            Self { system, _root: root, root_id }
        }

        fn add_child(&mut self, parent: NodeId, kind: LayoutKind) -> NodeId {
            let child = self.system.tree.mk_node().push_back(parent);
            self.system.tree.data.layout.set_kind(child, kind);
            child
        }

        fn move_over(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
            self.system.move_over(from, direction)
        }
    }

    impl Drop for TestTraditionalLayoutSystem {
        fn drop(&mut self) { self._root.remove(&mut self.system.tree); }
    }

    #[test]
    fn test_move_over_no_parent() {
        let system = TestTraditionalLayoutSystem::new();
        // Root has no parent
        assert_eq!(system.move_over(system.root_id, Direction::Right), None);
    }

    #[test]
    fn test_move_over_matching_orientation() {
        let mut system = TestTraditionalLayoutSystem::new();
        // Root is Horizontal
        let child1 = system.add_child(system.root_id, LayoutKind::Horizontal);
        let child2 = system.add_child(system.root_id, LayoutKind::Horizontal);
        let child3 = system.add_child(system.root_id, LayoutKind::Horizontal);

        // Direction Right (Horizontal), should move to next sibling
        assert_eq!(system.move_over(child1, Direction::Right), Some(child2));
        assert_eq!(system.move_over(child2, Direction::Right), Some(child3));
        assert_eq!(system.move_over(child3, Direction::Right), None);

        // Direction Left
        assert_eq!(system.move_over(child3, Direction::Left), Some(child2));
        assert_eq!(system.move_over(child2, Direction::Left), Some(child1));
        assert_eq!(system.move_over(child1, Direction::Left), None);
    }

    #[test]
    fn test_move_over_non_matching_non_stacked() {
        let mut system = TestTraditionalLayoutSystem::new();
        // Root is Horizontal
        let child1 = system.add_child(system.root_id, LayoutKind::Vertical);
        let _child2 = system.add_child(system.root_id, LayoutKind::Vertical);

        // Direction Up (Vertical), root is Horizontal, not matching, and not stacked
        assert_eq!(system.move_over(child1, Direction::Up), None);
    }

    #[test]
    fn test_move_over_non_matching_stacked() {
        let mut system = TestTraditionalLayoutSystem::new();
        // Create a stacked parent
        let stacked_parent = system.add_child(system.root_id, LayoutKind::HorizontalStack);
        let child1 = system.add_child(stacked_parent, LayoutKind::Horizontal);
        let child2 = system.add_child(stacked_parent, LayoutKind::Horizontal);
        let child3 = system.add_child(stacked_parent, LayoutKind::Horizontal);

        // Direction Up (Vertical), parent is HorizontalStack (Horizontal), orientations don't match
        // Should not move within stack, return None
        assert_eq!(system.move_over(child2, Direction::Up), None);
        assert_eq!(system.move_over(child3, Direction::Up), None);
        assert_eq!(system.move_over(child1, Direction::Up), None);

        // Direction Down -> also None
        assert_eq!(system.move_over(child1, Direction::Down), None);
        assert_eq!(system.move_over(child2, Direction::Down), None);
        assert_eq!(system.move_over(child3, Direction::Down), None);
    }

    #[test]
    fn test_move_over_matching_stacked() {
        let mut system = TestTraditionalLayoutSystem::new();
        // Create a stacked parent
        let stacked_parent = system.add_child(system.root_id, LayoutKind::HorizontalStack);
        let child1 = system.add_child(stacked_parent, LayoutKind::Horizontal);
        let child2 = system.add_child(stacked_parent, LayoutKind::Horizontal);
        let child3 = system.add_child(stacked_parent, LayoutKind::Horizontal);

        // Direction Left (Horizontal), parent is HorizontalStack (Horizontal), orientations match
        // Should move in siblings list: Left -> prev
        assert_eq!(system.move_over(child2, Direction::Left), Some(child1));
        assert_eq!(system.move_over(child3, Direction::Left), Some(child2));
        assert_eq!(system.move_over(child1, Direction::Left), None);

        // Direction Right -> next
        assert_eq!(system.move_over(child1, Direction::Right), Some(child2));
        assert_eq!(system.move_over(child2, Direction::Right), Some(child3));
        assert_eq!(system.move_over(child3, Direction::Right), None);
    }

    #[test]
    fn test_unstack_default_orientation_behavior() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TestTraditionalLayoutSystem::new();
        let layout = system.system.create_layout();
        let root_node = system.system.root(layout);

        let horizontal_stack_container = system.add_child(root_node, LayoutKind::HorizontalStack);
        system
            .system
            .tree
            .data
            .selection
            .select(&system.system.tree.map, horizontal_stack_container);
        let _ = crate::layout_engine::systems::LayoutSystem::unstack_parent_of_selection(
            &mut system.system,
            layout,
            StackDefaultOrientation::Perpendicular,
        );
        assert_eq!(
            system.system.layout(horizontal_stack_container),
            LayoutKind::Vertical
        );

        let vertical_stack_container = system.add_child(root_node, LayoutKind::VerticalStack);
        system
            .system
            .tree
            .data
            .selection
            .select(&system.system.tree.map, vertical_stack_container);
        let _ = crate::layout_engine::systems::LayoutSystem::unstack_parent_of_selection(
            &mut system.system,
            layout,
            StackDefaultOrientation::Perpendicular,
        );
        assert_eq!(
            system.system.layout(vertical_stack_container),
            LayoutKind::Horizontal
        );

        let horizontal_stack_container2 = system.add_child(root_node, LayoutKind::HorizontalStack);
        system
            .system
            .tree
            .data
            .selection
            .select(&system.system.tree.map, horizontal_stack_container2);
        let _ = crate::layout_engine::systems::LayoutSystem::unstack_parent_of_selection(
            &mut system.system,
            layout,
            StackDefaultOrientation::Same,
        );
        assert_eq!(
            system.system.layout(horizontal_stack_container2),
            LayoutKind::Horizontal
        );

        let vertical_stack_container2 = system.add_child(root_node, LayoutKind::VerticalStack);
        system
            .system
            .tree
            .data
            .selection
            .select(&system.system.tree.map, vertical_stack_container2);
        let _ = crate::layout_engine::systems::LayoutSystem::unstack_parent_of_selection(
            &mut system.system,
            layout,
            StackDefaultOrientation::Same,
        );
        assert_eq!(
            system.system.layout(vertical_stack_container2),
            LayoutKind::Vertical
        );
    }

    #[test]
    fn test_stack_default_orientation_behavior() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TestTraditionalLayoutSystem::new();
        let layout = system.system.create_layout();
        let root_node = system.system.root(layout);

        for &parent_kind in &[LayoutKind::Horizontal, LayoutKind::Vertical] {
            let container = system.add_child(root_node, parent_kind);
            system.system.tree.data.selection.select(&system.system.tree.map, container);
            let _ =
                crate::layout_engine::systems::LayoutSystem::apply_stacking_to_parent_of_selection(
                    &mut system.system,
                    layout,
                    StackDefaultOrientation::Perpendicular,
                );
            let expected_perp = match parent_kind {
                LayoutKind::Horizontal => LayoutKind::VerticalStack,
                LayoutKind::Vertical => LayoutKind::HorizontalStack,
                _ => unreachable!(),
            };
            assert_eq!(system.system.layout(container), expected_perp);

            let container = system.add_child(root_node, parent_kind);
            system.system.tree.data.selection.select(&system.system.tree.map, container);
            let _ =
                crate::layout_engine::systems::LayoutSystem::apply_stacking_to_parent_of_selection(
                    &mut system.system,
                    layout,
                    StackDefaultOrientation::Same,
                );
            let expected_same = match parent_kind {
                LayoutKind::Horizontal => LayoutKind::HorizontalStack,
                LayoutKind::Vertical => LayoutKind::VerticalStack,
                _ => unreachable!(),
            };
            assert_eq!(system.system.layout(container), expected_same);

            let container = system.add_child(root_node, parent_kind);
            system.system.tree.data.selection.select(&system.system.tree.map, container);
            let _ =
                crate::layout_engine::systems::LayoutSystem::apply_stacking_to_parent_of_selection(
                    &mut system.system,
                    layout,
                    StackDefaultOrientation::Horizontal,
                );
            assert_eq!(system.system.layout(container), LayoutKind::HorizontalStack);

            let container = system.add_child(root_node, parent_kind);
            system.system.tree.data.selection.select(&system.system.tree.map, container);
            let _ =
                crate::layout_engine::systems::LayoutSystem::apply_stacking_to_parent_of_selection(
                    &mut system.system,
                    layout,
                    StackDefaultOrientation::Vertical,
                );
            assert_eq!(system.system.layout(container), LayoutKind::VerticalStack);
        }
    }

    #[test]
    fn stacked_container_survives_new_additions() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));

        system.select_window(layout, w(1));
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);

        let stacked_child = system.selection(layout);
        let stacked_container = stacked_child.parent(system.map()).unwrap();
        assert!(system.layout(stacked_container).is_stacked());

        system.add_window_after_selection(layout, w(4));
        assert!(
            system.layout(stacked_container).is_stacked(),
            "joined container lost stack while still focused"
        );

        system.select_window(layout, w(3));
        system.add_window_after_selection(layout, w(5));

        assert!(
            system.layout(stacked_container).is_stacked(),
            "the joined container lost its stacked layout after adding another window"
        );
    }

    #[test]
    fn joining_into_existing_stack_keeps_it_stacked() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));

        system.select_window(layout, w(1));
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);

        let stacked_child = system.selection(layout);
        let stacked_container = stacked_child.parent(system.map()).unwrap();
        assert!(system.layout(stacked_container).is_stacked());

        system.add_window_after_selection(layout, w(3));
        system.select_window(layout, w(3));
        system.join_selection_with_direction(layout, Direction::Left);

        assert!(system.layout(stacked_container).is_stacked());
        assert_eq!(
            stacked_container.children(system.map()).count(),
            3,
            "expected the joined stack to grow instead of being replaced"
        );
    }

    #[test]
    fn joining_right_into_existing_stack_prepends_left_neighbor() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left = w(145);
        let a = w(146);
        let b = w(147);
        system.add_window_after_selection(layout, left);
        system.add_window_after_selection(layout, a);
        system.add_window_after_selection(layout, b);

        assert!(system.select_window(layout, a));
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);

        assert!(system.select_window(layout, a));
        system.join_selection_with_direction(layout, Direction::Left);

        let stacked_container = system
            .tree
            .data
            .window
            .node_for(layout, a)
            .and_then(|node| node.parent(system.map()))
            .expect("stacked container");
        let child_windows: Vec<_> = stacked_container
            .children(system.map())
            .filter_map(|child| system.window_at(child))
            .collect();
        assert_eq!(child_windows, vec![left, a, b], "{}", system.draw_tree(layout));
    }

    #[test]
    fn visible_windows_follow_tree_order() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(148);
        let w2 = w(149);
        let w3 = w(150);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        assert_eq!(system.visible_windows_in_layout(layout), vec![w1, w2, w3]);
    }

    #[test]
    fn joining_siblings_preserves_parent_size_invariants() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(150);
        let w2 = w(151);
        let w3 = w(152);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        assert!(system.select_window(layout, w1));
        system.join_selection_with_direction(layout, Direction::Right);

        let children: Vec<_> = root.children(system.map()).collect();
        assert_eq!(
            children.len(),
            2,
            "join should group two siblings under one container"
        );

        let total = system.tree.data.layout.info[root].total;
        let sum_children: f32 =
            children.iter().map(|&child| system.tree.data.layout.info[child].size).sum();
        assert!(
            (sum_children - total).abs() < 0.0001,
            "parent total should remain equal to the sum of child sizes after joining siblings"
        );
    }

    #[test]
    fn joining_siblings_keeps_combined_outer_share() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(153);
        let w2 = w(154);
        let w3 = w(155);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        let n1 = system.tree.data.window.node_for(layout, w1).expect("w1 node");
        let n2 = system.tree.data.window.node_for(layout, w2).expect("w2 node");
        let n3 = system.tree.data.window.node_for(layout, w3).expect("w3 node");
        system.tree.data.layout.info[n1].size = 3.0;
        system.tree.data.layout.info[n2].size = 2.0;
        system.tree.data.layout.info[n3].size = 1.0;
        system.tree.data.layout.info[root].total = 6.0;

        assert!(system.select_window(layout, w1));
        system.join_selection_with_direction(layout, Direction::Right);

        let children: Vec<_> = root.children(system.map()).collect();
        let grouped = children[0];
        let sibling = children[1];

        assert!(
            (system.tree.data.layout.info[grouped].size - 5.0).abs() < 0.0001,
            "newly grouped siblings should keep their combined outer share"
        );
        assert!(
            (system.tree.data.layout.info[sibling].size - 1.0).abs() < 0.0001,
            "unrelated sibling share should be preserved when grouping neighbors"
        );
    }

    #[test]
    fn unjoin_preserves_window_order() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(160);
        let w2 = w(161);
        let w3 = w(162);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        assert!(system.select_window(layout, w1));
        system.join_selection_with_direction(layout, Direction::Right);
        assert!(system.select_window(layout, w1));
        system.unjoin_selection(layout);

        assert_eq!(
            system.visible_windows_in_layout(layout),
            vec![w1, w2, w3],
            "{}",
            system.draw_tree(layout)
        );
    }

    #[test]
    fn rebalance_evenly_resets_skewed_sibling_sizes() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(170);
        let w2 = w(171);
        let w3 = w(172);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        let n1 = system.tree.data.window.node_for(layout, w1).expect("w1 node");
        let n2 = system.tree.data.window.node_for(layout, w2).expect("w2 node");
        let n3 = system.tree.data.window.node_for(layout, w3).expect("w3 node");
        system.tree.data.layout.info[n1].size = 5.0;
        system.tree.data.layout.info[n2].size = 2.0;
        system.tree.data.layout.info[n3].size = 1.0;
        system.tree.data.layout.info[root].total = 8.0;

        system.rebalance(layout);

        assert!((system.tree.data.layout.info[n1].size - 1.0).abs() < 0.0001);
        assert!((system.tree.data.layout.info[n2].size - 1.0).abs() < 0.0001);
        assert!((system.tree.data.layout.info[n3].size - 1.0).abs() < 0.0001);
        assert!((system.tree.data.layout.info[root].total - 3.0).abs() < 0.0001);
    }

    #[test]
    fn stacked_locked_windows_do_not_consume_entire_parent_axis() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(1);
        let w2 = w(2);
        let w3 = w(3);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        system.select_window(layout, w1);
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);

        let mut constraints = HashMap::default();
        for wid in [w1, w2] {
            constraints.insert(
                wid,
                WindowLayoutConstraints {
                    is_resizable: false,
                    locked_width: 280.0,
                    locked_height: 120.0,
                    min_width: 280.0,
                    min_height: 120.0,
                    max_width: 280.0,
                    max_height: 120.0,
                                wrap_size: false,
            }
                .normalized(),
            );
        }

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(500.0, 300.0));
        let positions = system.calculate_layout(
            layout,
            screen,
            40.0,
            &constraints,
            &Default::default(),
            0.0,
            Default::default(),
            Default::default(),
        );
        let frames: HashMap<WindowId, CGRect> = positions.into_iter().collect();
        let stacked_w1 = frames.get(&w1).copied().expect("stacked window missing");
        let sibling_w3 = frames.get(&w3).copied().expect("sibling window missing");

        assert!(
            stacked_w1.size.width >= 279.0,
            "stacked locked width should be preserved after stack offset reservation"
        );
        assert!(
            sibling_w3.size.width >= 100.0,
            "sibling outside stack should still get meaningful width"
        );
    }

    #[test]
    fn focused_locked_child_in_mixed_stack_limits_stack_growth() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let settings = w(11);
        let normal = w(12);
        let sibling = w(13);
        system.add_window_after_selection(layout, settings);
        system.add_window_after_selection(layout, normal);
        system.add_window_after_selection(layout, sibling);

        system.select_window(layout, settings);
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);

        let mut constraints = HashMap::default();
        constraints.insert(
            settings,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 320.0,
                locked_height: 200.0,
                min_width: 320.0,
                min_height: 200.0,
                max_width: 320.0,
                max_height: 200.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        assert!(system.select_window(layout, settings));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let settings_frame = frames.get(&settings).copied().expect("settings frame missing");
        let sibling_frame = frames.get(&sibling).copied().expect("sibling frame missing");
        assert!(
            settings_frame.size.width <= 321.0,
            "focused fixed-size settings window should not keep an oversized stack slot"
        );
        assert!(
            sibling_frame.size.width >= 399.0,
            "sibling outside mixed stack should keep its normal split share"
        );
    }

    #[test]
    fn focused_locked_child_does_not_shrink_parent_stack_container() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let constrained = w(14);
        let normal = w(15);
        let sibling = w(16);
        system.add_window_after_selection(layout, constrained);
        system.add_window_after_selection(layout, normal);
        system.add_window_after_selection(layout, sibling);

        system.select_window(layout, constrained);
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);

        let mut constraints = HashMap::default();
        constraints.insert(
            constrained,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 320.0,
                locked_height: 200.0,
                min_width: 320.0,
                min_height: 200.0,
                max_width: 320.0,
                max_height: 200.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));

        assert!(system.select_window(layout, normal));
        let normal_frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        assert!(system.select_window(layout, constrained));
        let constrained_frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let normal_sibling = normal_frames
            .get(&sibling)
            .copied()
            .expect("sibling frame with unconstrained focus");
        let constrained_sibling = constrained_frames
            .get(&sibling)
            .copied()
            .expect("sibling frame with constrained focus");

        assert!(
            (normal_sibling.size.width - constrained_sibling.size.width).abs() < 1.0,
            "switching focus to a constrained stack child should not shrink the parent split"
        );
    }

    #[test]
    fn stack_offset_is_capped_to_respect_focused_fixed_width() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(21);
        let w2 = w(22);
        let w3 = w(23);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        system.select_window(layout, w1);
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);
        system.select_window(layout, w1);

        let mut constraints = HashMap::default();
        constraints.insert(
            w1,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 360.0,
                locked_height: 200.0,
                min_width: 360.0,
                min_height: 200.0,
                max_width: 360.0,
                max_height: 200.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(420.0, 600.0));
        let frames = system.calculate_layout(
            layout,
            screen,
            250.0,
            &constraints,
            &Default::default(),
            0.0,
            Default::default(),
            Default::default(),
        );
        let frames: HashMap<WindowId, CGRect> = frames.into_iter().collect();
        let focused = frames.get(&w1).copied().expect("focused frame missing");

        assert!(
            focused.size.width >= 359.0,
            "focused fixed window width should be preserved by reducing stack offset"
        );
    }

    #[test]
    fn stack_offset_is_capped_to_respect_focused_fixed_height() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Vertical);

        let w1 = w(31);
        let w2 = w(32);
        let w3 = w(33);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        system.select_window(layout, w1);
        system.join_selection_with_direction(layout, Direction::Down);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);
        system.select_window(layout, w1);

        let mut constraints = HashMap::default();
        constraints.insert(
            w1,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 200.0,
                locked_height: 300.0,
                min_width: 200.0,
                min_height: 300.0,
                max_width: 200.0,
                max_height: 300.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(600.0, 360.0));
        let frames = system.calculate_layout(
            layout,
            screen,
            220.0,
            &constraints,
            &Default::default(),
            0.0,
            Default::default(),
            Default::default(),
        );
        let frames: HashMap<WindowId, CGRect> = frames.into_iter().collect();
        let focused = frames.get(&w1).copied().expect("focused frame missing");

        assert!(
            focused.size.height >= 299.0,
            "focused fixed window height should be preserved by reducing stack offset"
        );
    }

    #[test]
    fn stack_offset_is_not_capped_by_focused_max_only_width() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(34);
        let w2 = w(35);
        let w3 = w(36);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        system.select_window(layout, w1);
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);
        system.select_window(layout, w1);

        let mut constraints = HashMap::default();
        constraints.insert(
            w1,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 360.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(420.0, 600.0));
        let frames = system.calculate_layout(
            layout,
            screen,
            250.0,
            &constraints,
            &Default::default(),
            0.0,
            Default::default(),
            Default::default(),
        );
        let frames: HashMap<WindowId, CGRect> = frames.into_iter().collect();
        let focused = frames.get(&w1).copied().expect("focused frame missing");

        // Max-only should clamp the focused frame itself, but not reduce stack offset budget.
        // With offset 250 and 3 children in a 420px container, focused width stays near 100px.
        assert!(focused.size.width <= 360.0);
        assert!(focused.size.width <= 200.0);
    }

    #[test]
    fn focused_max_only_child_does_not_shrink_mixed_stack_container() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let constrained = w(46);
        let normal = w(47);
        let sibling = w(48);
        system.add_window_after_selection(layout, constrained);
        system.add_window_after_selection(layout, normal);
        system.add_window_after_selection(layout, sibling);

        system.select_window(layout, constrained);
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);
        assert!(system.select_window(layout, constrained));

        let mut constraints = HashMap::default();
        constraints.insert(
            constrained,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 320.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let constrained_frame = frames.get(&constrained).copied().expect("constrained frame");
        let sibling_frame = frames.get(&sibling).copied().expect("sibling frame");

        assert!(
            constrained_frame.size.width <= 321.0,
            "focused max-only child should still be clamped at the leaf"
        );
        assert!(
            sibling_frame.size.width <= 601.0,
            "max-only focused child should not reclaim parent split space"
        );
    }

    #[test]
    fn selecting_non_first_stack_child_does_not_resize_windows() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let first = w(130);
        let second = w(131);
        let sibling = w(132);
        system.add_window_after_selection(layout, first);
        system.add_window_after_selection(layout, second);
        system.add_window_after_selection(layout, sibling);

        assert!(system.select_window(layout, first));
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        assert!(system.select_window(layout, first));
        let first_selected: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &Default::default(),
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();
        assert!(system.select_window(layout, second));
        let second_selected: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &Default::default(),
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        assert_eq!(first_selected, second_selected);
    }

    #[test]
    fn max_only_height_does_not_cap_plain_row_cross_axis() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Vertical);

        let top_row = system.tree.mk_node().push_back(root);
        system.tree.data.layout.set_kind(top_row, LayoutKind::Horizontal);

        let constrained = w(37);
        let unconstrained = w(38);
        let sibling = w(39);
        let constrained_node = system.add_window_under(layout, top_row, constrained);
        let _ = system.add_window_under(layout, top_row, unconstrained);
        let _ = system.add_window_under(layout, root, sibling);
        system.select(constrained_node);

        let mut constraints = HashMap::default();
        constraints.insert(
            constrained,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 200.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                0.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let constrained_frame = frames.get(&constrained).copied().expect("constrained frame");
        let unconstrained_frame = frames.get(&unconstrained).copied().expect("unconstrained frame");
        let sibling_frame = frames.get(&sibling).copied().expect("sibling frame");

        assert!(
            constrained_frame.size.height <= 201.0,
            "constrained leaf should still honor its own max height"
        );
        assert!(
            unconstrained_frame.size.height >= 399.0,
            "unconstrained sibling in the row should keep the row's full height"
        );
        assert!(
            (sibling_frame.size.height - 400.0).abs() < 1.0,
            "cross-axis max-only constraint should not change the parent split allocation"
        );
    }

    #[test]
    fn single_child_column_propagates_max_width_to_parent_split() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left_column = system.tree.mk_node().push_back(root);
        system.tree.data.layout.set_kind(left_column, LayoutKind::Vertical);

        let constrained = w(140);
        let sibling = w(141);
        let constrained_node = system.add_window_under(layout, left_column, constrained);
        let _ = system.add_window_under(layout, root, sibling);
        system.select(constrained_node);

        let mut constraints = HashMap::default();
        constraints.insert(
            constrained,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 320.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                0.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let constrained_frame = frames.get(&constrained).copied().expect("constrained frame");
        let sibling_frame = frames.get(&sibling).copied().expect("sibling frame");

        assert!(
            constrained_frame.size.width <= 321.0,
            "single-child wrapper should still clamp the constrained leaf"
        );
        assert!(
            sibling_frame.size.width >= 879.0,
            "parent split should reclaim width instead of leaving dead space beside a wrapped constrained window"
        );
    }

    #[test]
    fn multi_child_column_propagates_collective_max_width_to_parent_split() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left_column = system.tree.mk_node().push_back(root);
        system.tree.data.layout.set_kind(left_column, LayoutKind::Vertical);

        let top = w(151);
        let bottom = w(152);
        let sibling = w(153);
        let _ = system.add_window_under(layout, left_column, top);
        let _ = system.add_window_under(layout, left_column, bottom);
        let _ = system.add_window_under(layout, root, sibling);

        let mut constraints = HashMap::default();
        for wid in [top, bottom] {
            constraints.insert(
                wid,
                WindowLayoutConstraints {
                    is_resizable: true,
                    locked_width: 0.0,
                    locked_height: 0.0,
                    min_width: 0.0,
                    min_height: 0.0,
                    max_width: 320.0,
                    max_height: 0.0,
                                wrap_size: false,
            }
                .normalized(),
            );
        }

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                0.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let top_frame = frames.get(&top).copied().expect("top frame");
        let bottom_frame = frames.get(&bottom).copied().expect("bottom frame");
        let sibling_frame = frames.get(&sibling).copied().expect("sibling frame");

        assert!(top_frame.size.width <= 321.0);
        assert!(bottom_frame.size.width <= 321.0);
        assert!(
            sibling_frame.size.width >= 879.0,
            "shared column max width should be reclaimed by the sibling split"
        );
    }

    #[test]
    fn stack_line_reservation_counts_toward_cross_axis_constraints() {
        use crate::common::config::StackDefaultOrientation;

        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Vertical);

        let top_a = w(154);
        let top_b = w(155);
        let bottom = w(156);
        system.add_window_after_selection(layout, top_a);
        system.add_window_after_selection(layout, top_b);
        system.add_window_after_selection(layout, bottom);

        assert!(system.select_window(layout, top_a));
        system.join_selection_with_direction(layout, Direction::Right);
        let _ = system.apply_stacking_to_parent_of_selection(layout, StackDefaultOrientation::Same);

        let mut constraints = HashMap::default();
        for wid in [top_a, top_b] {
            constraints.insert(
                wid,
                WindowLayoutConstraints {
                    is_resizable: false,
                    locked_width: 0.0,
                    locked_height: 300.0,
                    min_width: 0.0,
                    min_height: 0.0,
                    max_width: 0.0,
                    max_height: 0.0,
                                wrap_size: false,
            }
                .normalized(),
            );
        }

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 600.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                20.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        for wid in [top_a, top_b] {
            let frame = frames.get(&wid).copied().expect("stack child frame");
            assert!(
                frame.size.height >= 299.0,
                "stack line reservation should be included before satisfying fixed child heights"
            );
        }
    }

    #[test]
    fn single_child_stack_reserves_stack_line_on_cross_axis() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Vertical);

        let top_stack = system.tree.mk_node().push_back(root);
        system.tree.data.layout.set_kind(top_stack, LayoutKind::HorizontalStack);

        let stacked = w(163);
        let sibling = w(164);
        let _ = system.add_window_under(layout, top_stack, stacked);
        let _ = system.add_window_under(layout, root, sibling);

        let mut constraints = HashMap::default();
        constraints.insert(
            stacked,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 0.0,
                locked_height: 300.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 600.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                20.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let stacked_frame = frames.get(&stacked).copied().expect("stacked frame");
        let sibling_frame = frames.get(&sibling).copied().expect("sibling frame");
        assert!(
            stacked_frame.size.height >= 299.0,
            "single-child stacks should reserve stack-line thickness before satisfying fixed heights"
        );
        assert!(
            sibling_frame.size.height <= 281.0,
            "the sibling split should not steal the stack-line reservation from a one-window stack"
        );
    }

    #[test]
    fn non_focused_stack_windows_stay_inside_small_container() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::HorizontalStack);

        let w1 = w(157);
        let w2 = w(158);
        let w3 = w(159);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(60.0, 90.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &Default::default(),
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        for wid in [w1, w2, w3] {
            let frame = frames.get(&wid).copied().expect("stack frame");
            assert!(frame.origin.x >= -0.5);
            assert!(frame.origin.y >= -0.5);
            assert!(frame.max().x <= 60.5, "frame spilled horizontally: {frame:?}");
            assert!(frame.max().y <= 90.5, "frame spilled vertically: {frame:?}");
        }
    }

    #[test]
    fn max_only_height_does_not_cap_stack_cross_axis() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Vertical);

        let top_stack = system.tree.mk_node().push_back(root);
        system.tree.data.layout.set_kind(top_stack, LayoutKind::HorizontalStack);

        let constrained = w(40);
        let unconstrained = w(41);
        let sibling = w(42);
        let constrained_node = system.add_window_under(layout, top_stack, constrained);
        let _ = system.add_window_under(layout, top_stack, unconstrained);
        let _ = system.add_window_under(layout, root, sibling);
        system.select(constrained_node);

        let mut constraints = HashMap::default();
        constraints.insert(
            constrained,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 200.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let constrained_frame = frames.get(&constrained).copied().expect("constrained frame");
        let unconstrained_frame = frames.get(&unconstrained).copied().expect("unconstrained frame");
        let sibling_frame = frames.get(&sibling).copied().expect("sibling frame");

        assert!(
            constrained_frame.size.height <= 201.0,
            "focused constrained stack child should still honor its own max height"
        );
        assert!(
            unconstrained_frame.size.height >= 399.0,
            "other stacked windows should not inherit the focused window's cross-axis max"
        );
        assert!(
            (sibling_frame.size.height - 400.0).abs() < 1.0,
            "stack container should not shrink just because the selected child has a max-only cap"
        );
    }

    #[test]
    fn stack_cross_axis_size_does_not_change_with_focus() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Vertical);

        let top_stack = system.tree.mk_node().push_back(root);
        system.tree.data.layout.set_kind(top_stack, LayoutKind::HorizontalStack);

        let constrained = w(43);
        let unconstrained = w(44);
        let sibling = w(45);
        let constrained_node = system.add_window_under(layout, top_stack, constrained);
        let _unconstrained_node = system.add_window_under(layout, top_stack, unconstrained);
        let _ = system.add_window_under(layout, root, sibling);

        let mut constraints = HashMap::default();
        constraints.insert(
            constrained,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 280.0,
                locked_height: 200.0,
                min_width: 280.0,
                min_height: 200.0,
                max_width: 280.0,
                max_height: 200.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));

        system.select(constrained_node);
        let constrained_frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        assert!(system.select_window(layout, unconstrained));
        let unconstrained_frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                40.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let constrained_sibling = constrained_frames
            .get(&sibling)
            .copied()
            .expect("sibling frame when constrained child focused");
        let unconstrained_sibling = unconstrained_frames
            .get(&sibling)
            .copied()
            .expect("sibling frame when unconstrained child focused");
        let unconstrained_frame =
            unconstrained_frames.get(&unconstrained).copied().expect("unconstrained frame");

        assert!(
            (constrained_sibling.size.height - unconstrained_sibling.size.height).abs() < 1.0,
            "switching focus inside a stack should not resize sibling containers"
        );
        assert!(
            unconstrained_frame.size.height >= 399.0,
            "an unconstrained focused child should still be allowed to use the stack's full height"
        );
    }

    // Focused stack frames should be identical to normal stack frames.
    #[test]
    fn test_get_focused_frame_for_index_horizontal_index_zero() {
        let container_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let stack_result = StackLayoutResult::new(container_rect, 3, 50.0, true);
        let frame = stack_result.get_focused_frame_for_index(0, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(0));
    }

    #[test]
    fn test_get_focused_frame_for_index_vertical_index_zero() {
        let container_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let stack_result = StackLayoutResult::new(container_rect, 3, 50.0, false);
        let frame = stack_result.get_focused_frame_for_index(0, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(0));
    }

    #[test]
    fn test_get_focused_frame_for_index_horizontal_index_one() {
        let container_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let stack_result = StackLayoutResult::new(container_rect, 3, 50.0, true);
        let frame = stack_result.get_focused_frame_for_index(1, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(1));
    }

    #[test]
    fn test_get_focused_frame_for_index_vertical_index_one() {
        let container_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let stack_result = StackLayoutResult::new(container_rect, 3, 50.0, false);
        let frame = stack_result.get_focused_frame_for_index(1, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(1));
    }

    #[test]
    fn test_get_focused_frame_for_index_window_larger_than_container() {
        let container_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(100.0, 100.0));
        let stack_result = StackLayoutResult::new(container_rect, 1, 0.0, true);
        let frame = stack_result.get_focused_frame_for_index(0, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(0));
    }

    #[test]
    fn test_get_focused_frame_for_index_zero_stack_offset() {
        let container_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let stack_result = StackLayoutResult::new(container_rect, 3, 0.0, true);
        let frame = stack_result.get_focused_frame_for_index(1, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(1));
    }

    #[test]
    fn test_get_focused_frame_for_index_floating_point_precision() {
        // Test case that could cause min > max due to precision
        let container_rect = CGRect::new(
            CGPoint::new(1726.5118132741347, 1726.5118132741347),
            CGSize::new(1.0, 1.0),
        );
        let stack_result = StackLayoutResult::new(container_rect, 1, 0.0, true);
        let frame = stack_result.get_focused_frame_for_index(0, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(0));
    }

    #[test]
    fn test_get_focused_frame_for_index_small_container() {
        let container_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(10.0, 10.0));
        let stack_result = StackLayoutResult::new(container_rect, 1, 0.0, true);
        let frame = stack_result.get_focused_frame_for_index(0, 0);
        assert_eq!(frame, stack_result.get_frame_for_index(0));
    }

    #[test]
    fn adding_window_after_selection_splits_selected_share() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(71);
        let w2 = w(72);
        let w3 = w(73);
        let w4 = w(74);

        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);

        let n1 = system.tree.data.window.node_for(layout, w1).expect("w1 node missing");
        let n2 = system.tree.data.window.node_for(layout, w2).expect("w2 node missing");
        let n3 = system.tree.data.window.node_for(layout, w3).expect("w3 node missing");

        system.tree.data.layout.info[n1].size = 3.0;
        system.tree.data.layout.info[n2].size = 1.0;
        system.tree.data.layout.info[n3].size = 1.0;
        system.tree.data.layout.info[root].total = 5.0;

        system.select_window(layout, w2);
        system.add_window_after_selection(layout, w4);

        let n4 = system.tree.data.window.node_for(layout, w4).expect("w4 node missing");
        let size2 = system.tree.data.layout.info[n2].size;
        let size4 = system.tree.data.layout.info[n4].size;
        let total = system.tree.data.layout.info[root].total;

        assert!((size2 - 0.5).abs() < 0.0001, "selected child should be halved");
        assert!((size4 - 0.5).abs() < 0.0001, "new sibling should get half");
        assert!((total - 5.0).abs() < 0.0001, "parent total should be preserved");
    }

    #[test]
    fn manual_resize_uses_dominant_edge_delta_when_edges_disagree() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left = w(81);
        let right = w(82);
        system.add_window_after_selection(layout, left);
        system.add_window_after_selection(layout, right);

        let right_node = system
            .tree
            .data
            .window
            .node_for(layout, right)
            .expect("right window node missing");

        let old_frame = CGRect::new(CGPoint::new(500.0, 0.0), CGSize::new(500.0, 800.0));
        // Left edge expands by 20px while right edge retracts by 10px.
        // We should use the dominant edge (20px), not the net (10px).
        let new_frame = CGRect::new(CGPoint::new(480.0, 0.0), CGSize::new(510.0, 800.0));
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));

        system.set_frame_from_resize(right_node, old_frame, new_frame, screen);

        let after = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");
        assert!(
            after > 0.515,
            "dominant-edge resize should apply more than the 10px net delta"
        );
    }

    #[test]
    fn manual_resize_keeps_small_node_size_across_layout_pass() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left = w(83);
        let right = w(84);
        system.add_window_after_selection(layout, left);
        system.add_window_after_selection(layout, right);

        let right_node = system
            .tree
            .data
            .window
            .node_for(layout, right)
            .expect("right window node missing");

        let old_frame = CGRect::new(CGPoint::new(500.0, 0.0), CGSize::new(500.0, 800.0));
        let new_frame = CGRect::new(CGPoint::new(950.0, 0.0), CGSize::new(50.0, 800.0));
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));

        system.set_frame_from_resize(right_node, old_frame, new_frame, screen);
        let before = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");

        let gaps = crate::common::config::GapSettings::default();
        let _ = system.calculate_layout_for_node(
            root,
            screen,
            screen,
            0.0,
            &HashMap::default(),
            &gaps,
            0.0,
            crate::common::config::HorizontalPlacement::Top,
            crate::common::config::VerticalPlacement::Left,
        );

        let after = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");
        assert!(
            before <= 0.051,
            "manual resize should reach the small-node clamp, got {before}"
        );
        assert!(
            (after - before).abs() < 0.0001,
            "layout pass should not rebalance to 50/50"
        );
    }

    #[test]
    fn adding_with_root_selected_splits_active_child_share() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(91);
        let w2 = w(92);
        let w3 = w(93);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);

        let n1 = system.tree.data.window.node_for(layout, w1).expect("w1 node missing");
        let n2 = system.tree.data.window.node_for(layout, w2).expect("w2 node missing");
        system.tree.data.layout.info[n1].size = 3.0;
        system.tree.data.layout.info[n2].size = 1.0;
        system.tree.data.layout.info[root].total = 4.0;

        system.select(root);
        system.add_window_after_selection(layout, w3);

        let n3 = system.tree.data.window.node_for(layout, w3).expect("w3 node missing");
        assert!((system.tree.data.layout.info[n2].size - 0.5).abs() < 0.0001);
        assert!((system.tree.data.layout.info[n3].size - 0.5).abs() < 0.0001);
        assert!((system.tree.data.layout.info[root].total - 4.0).abs() < 0.0001);
    }

    #[test]
    fn adding_second_window_forces_even_split_when_selected_size_is_zero() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(94);
        let w2 = w(95);
        system.add_window_after_selection(layout, w1);

        let n1 = system.tree.data.window.node_for(layout, w1).expect("w1 node missing");
        system.tree.data.layout.info[n1].size = 0.0;
        system.tree.data.layout.info[root].total = 0.0;

        system.select_window(layout, w1);
        system.add_window_after_selection(layout, w2);

        let n2 = system.tree.data.window.node_for(layout, w2).expect("w2 node missing");
        let s1 = system.tree.data.layout.info[n1].size;
        let s2 = system.tree.data.layout.info[n2].size;
        let total = system.tree.data.layout.info[root].total;

        assert!((s1 - s2).abs() < 0.0001, "second insert should split 50/50");
        assert!(
            (total - (s1 + s2)).abs() < 0.0001,
            "parent total should be recomputed"
        );
        assert!(
            (s1 / total - 0.5).abs() < 0.0001,
            "resulting proportion should be 50%"
        );
    }

    #[test]
    fn insertion_split_respects_non_resizable_locked_size_hints() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(96);
        let w2 = w(97);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);

        let mut constraints = HashMap::default();
        constraints.insert(
            w1,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 1000.0,
                locked_height: 800.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );
        constraints.insert(
            w2,
            WindowLayoutConstraints {
                is_resizable: false,
                locked_width: 500.0,
                locked_height: 800.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1500.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                0.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let f1 = frames.get(&w1).copied().expect("w1 frame missing");
        let f2 = frames.get(&w2).copied().expect("w2 frame missing");
        assert!(
            f1.size.width >= 999.0,
            "non-resizable w1 should keep locked width"
        );
        assert!(
            f2.size.width >= 499.0,
            "non-resizable w2 should keep locked width"
        );
    }

    #[test]
    fn insertion_split_ignores_bounds_for_resizable_windows() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(98);
        let w2 = w(99);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);

        let mut constraints = HashMap::default();
        constraints.insert(
            w1,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 1000.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );
        constraints.insert(
            w2,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1500.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                0.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let f1 = frames.get(&w1).copied().expect("w1 frame missing");
        let f2 = frames.get(&w2).copied().expect("w2 frame missing");
        assert!(
            (f1.size.width - f2.size.width).abs() < 1.0,
            "new split should remain even"
        );
    }

    #[test]
    fn insertion_split_respects_axis_locked_width_for_resizable_window() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(106);
        let w2 = w(107);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);

        let mut constraints = HashMap::default();
        constraints.insert(
            w1,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 1000.0,
                min_height: 0.0,
                max_width: 1000.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );
        constraints.insert(
            w2,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 0.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1500.0, 800.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                0.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let f1 = frames.get(&w1).copied().expect("w1 frame missing");
        let f2 = frames.get(&w2).copied().expect("w2 frame missing");
        assert!(f1.size.width >= 999.0, "axis-locked width should be preserved");
        assert!(
            f2.size.width >= 499.0,
            "remaining sibling should get leftover width"
        );
    }

    #[test]
    fn max_only_width_cap_reclaims_space_for_sibling() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(108);
        let w2 = w(109);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);

        let mut constraints = HashMap::default();
        constraints.insert(
            w1,
            WindowLayoutConstraints {
                is_resizable: true,
                locked_width: 0.0,
                locked_height: 0.0,
                min_width: 0.0,
                min_height: 0.0,
                max_width: 600.0,
                max_height: 0.0,
                            wrap_size: false,
            }
            .normalized(),
        );

        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1600.0, 900.0));
        let frames: HashMap<WindowId, CGRect> = system
            .calculate_layout(
                layout,
                screen,
                0.0,
                &constraints,
                &Default::default(),
                0.0,
                Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect();

        let f1 = frames.get(&w1).copied().expect("w1 frame missing");
        let f2 = frames.get(&w2).copied().expect("w2 frame missing");
        assert!((f1.size.width - 600.0).abs() < 1.0);
        assert!((f2.size.width - 1000.0).abs() < 1.0);
        assert!((f2.origin.x - 600.0).abs() < 1.0);
    }

    #[test]
    fn adding_with_many_siblings_splits_inside_new_subcontainer() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(101);
        let w2 = w(102);
        let w3 = w(103);
        let w4 = w(104);
        let w5 = w(105);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);
        system.add_window_after_selection(layout, w4);

        let n2 = system.tree.data.window.node_for(layout, w2).expect("w2 node missing");
        system.tree.data.layout.info[n2].size = 2.0;
        system.tree.data.layout.info[root].total = root
            .children(system.map())
            .map(|child| system.tree.data.layout.info[child].size.max(0.0))
            .sum();

        system.select_window(layout, w2);
        system.add_window_after_selection(layout, w5);

        let n5 = system.tree.data.window.node_for(layout, w5).expect("w5 node missing");
        let parent2 = n2.parent(system.map()).expect("w2 parent missing");
        let parent5 = n5.parent(system.map()).expect("w5 parent missing");
        assert_eq!(parent2, parent5, "w2 and w5 should share the new container");
        let size2 = system.tree.data.layout.info[n2].size;
        let size5 = system.tree.data.layout.info[n5].size;
        assert!((size2 - size5).abs() < 0.0001, "new smart split should be 50/50");
    }

    #[test]
    fn manual_resize_at_outer_edge_is_noop_instead_of_reversing() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left = w(41);
        let right = w(42);
        system.add_window_after_selection(layout, left);
        system.add_window_after_selection(layout, right);

        let right_node = system
            .tree
            .data
            .window
            .node_for(layout, right)
            .expect("right window node missing");
        let before = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");

        // Simulate dragging the right edge outward on the rightmost tile.
        // There is no neighbor to the right, so this should no-op.
        let old_frame = CGRect::new(CGPoint::new(500.0, 0.0), CGSize::new(500.0, 800.0));
        let new_frame = CGRect::new(CGPoint::new(500.0, 0.0), CGSize::new(550.0, 800.0));
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));

        system.set_frame_from_resize(right_node, old_frame, new_frame, screen);

        let after = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");
        assert_eq!(before, after);
    }

    #[test]
    fn command_resize_at_outer_edge_can_still_resize_using_available_neighbor() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left = w(51);
        let right = w(52);
        system.add_window_after_selection(layout, left);
        system.add_window_after_selection(layout, right);
        system.select_window(layout, right);

        let right_node = system
            .tree
            .data
            .window
            .node_for(layout, right)
            .expect("right window node missing");
        let before = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");

        system.resize_selection_by(layout, 0.10);

        let after = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");
        assert!(after > before);
    }

    #[test]
    fn manual_resize_ignores_cross_axis_jitter_when_hitting_edge() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let left = w(61);
        let right = w(62);
        system.add_window_after_selection(layout, left);
        system.add_window_after_selection(layout, right);

        let right_node = system
            .tree
            .data
            .window
            .node_for(layout, right)
            .expect("right window node missing");
        let before = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");

        // Horizontal outward drag with small vertical jitter should stay horizontal-only;
        // since we are at the right edge, this should no-op.
        let old_frame = CGRect::new(CGPoint::new(500.0, 0.0), CGSize::new(500.0, 800.0));
        let new_frame = CGRect::new(CGPoint::new(500.0, 0.4), CGSize::new(550.0, 799.6));
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));

        system.set_frame_from_resize(right_node, old_frame, new_frame, screen);

        let after = system
            .tree
            .data
            .layout
            .proportion(&system.tree.map, right_node)
            .expect("right node proportion missing");
        assert_eq!(before, after);
    }
}
