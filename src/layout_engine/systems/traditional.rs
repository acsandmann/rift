use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::tiles::TileLayouts;
use super::{EditResult, Focus, FocusResult, GroupEdit, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
use crate::common::config::WindowInsertionPoint;
use crate::layout_engine::systems::{LayoutSystem, WindowLayoutConstraints};
use crate::layout_engine::{Direction, LayoutId, LayoutKind};
use crate::model::tree::{NodeId, Placement};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(transparent)]
pub struct TraditionalLayoutSystem {
    pub(super) tiles: TileLayouts,
    #[serde(skip)]
    window_insertion_point: WindowInsertionPoint,
    #[serde(skip)]
    equalize_nodes: bool,
}

impl Default for TraditionalLayoutSystem {
    fn default() -> Self { Self::new(WindowInsertionPoint::default(), true) }
}

impl TraditionalLayoutSystem {
    pub fn new(window_insertion_point: WindowInsertionPoint, equalize_nodes: bool) -> Self {
        Self {
            tiles: TileLayouts::default(),
            window_insertion_point,
            equalize_nodes,
        }
    }

    pub fn set_window_insertion_point(&mut self, value: WindowInsertionPoint) {
        self.window_insertion_point = value;
    }

    pub fn set_equalize_nodes(&mut self, value: bool) { self.equalize_nodes = value; }

    fn smart_window_insertion(
        &mut self,
        layout: LayoutId,
        selection: NodeId,
        wid: WindowId,
    ) -> NodeId {
        let parent = selection.parent(self.tiles.map());

        if let Some(parent) = parent {
            let parent_layout = self.tiles.layout(parent);
            let sibling_count = parent.children(self.tiles.map()).count();

            if sibling_count >= 4 && !parent_layout.is_group() {
                let sub_container =
                    self.tiles.nest_in_container_internal(layout, selection, parent_layout);
                let node = self.tiles.tree.insert(Placement::Back(sub_container));
                self.split_new_sibling_from_selection(selection, node);
                self.tiles.tree.window.set_window(&mut self.tiles.tree.map, layout, node, wid);
                return node;
            }
        }

        let node = self.tiles.tree.insert(Placement::After(selection));
        self.split_new_sibling_from_selection(selection, node);
        self.tiles.tree.window.set_window(&mut self.tiles.tree.map, layout, node, wid);
        node
    }

    fn find_or_create_smart_common_parent(
        &mut self,
        node1: NodeId,
        node2: NodeId,
        direction: Direction,
    ) -> NodeId {
        let parent1 = node1.parent(self.tiles.map());
        let parent2 = node2.parent(self.tiles.map());

        if let (Some(p1), Some(p2)) = (parent1, parent2) {
            if p1 == p2 {
                let parent_layout = self.tiles.layout(p1);
                let sibling_count = p1.children(self.tiles.map()).count();

                if parent_layout.orientation() == direction.orientation()
                    && !parent_layout.is_group()
                    && sibling_count == 2
                {
                    return p1;
                }
            }
        }

        self.find_or_create_common_parent_internal(node1, node2)
    }

    fn find_natural_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let sibling = self.find_direct_sibling_target(from, direction);
        if let Some(parent) = from.parent(self.tiles.map()) {
            let kind = self.tiles.layout(parent);
            if !kind.is_group()
                && kind.orientation() == direction.orientation()
                && (sibling.is_none() || parent.children(self.tiles.map()).count() == 2)
                && let Some(neighbor) = self.find_direct_sibling_target(parent, direction)
            {
                return Some(neighbor);
            }
        }
        sibling
            .or_else(|| {
                self.tiles.traverse_internal(from, direction).map(|target| {
                    if self.tiles.window_at(target).is_some() {
                        target
                    } else {
                        self.find_best_container_child_for_joining(target, direction)
                            .unwrap_or(target)
                    }
                })
            })
            .or_else(|| self.find_hierarchical_join_target(from, direction))
    }

    fn find_direct_sibling_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let _parent = from.parent(self.tiles.map())?;

        match direction {
            Direction::Right | Direction::Down => from.next_sibling(self.tiles.map()),
            Direction::Left | Direction::Up => from.prev_sibling(self.tiles.map()),
        }
    }

    fn find_best_container_child_for_joining(
        &self,
        container: NodeId,
        direction: Direction,
    ) -> Option<NodeId> {
        let first = container.first_child(self.tiles.map())?;
        if self.tiles.layout(container).orientation() != direction.orientation() {
            return self.tiles.local_selection(container).or(Some(first));
        }
        match direction {
            Direction::Left | Direction::Up => Some(first),
            Direction::Right | Direction::Down => container.last_child(self.tiles.map()),
        }
    }

    fn find_hierarchical_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        for ancestor in from.ancestors(self.tiles.map()).skip(1) {
            if let Some(target) = self.find_direct_sibling_target(ancestor, direction) {
                return self.find_best_container_child_for_joining(target, direction.opposite());
            }
        }
        None
    }

    fn perform_natural_join(&mut self, selection: NodeId, target: NodeId, direction: Direction) {
        let selection_parent = selection.parent(self.tiles.map());
        let target_parent = target.parent(self.tiles.map());

        let selection_stack_parent =
            selection_parent.filter(|&parent| self.tiles.layout(parent).is_stacked());
        let target_stack_parent =
            target_parent.filter(|&parent| self.tiles.layout(parent).is_stacked());

        let backwards = matches!(direction, Direction::Left | Direction::Up);
        let join = match (selection_stack_parent, target_stack_parent) {
            (Some(stack), None) => Some((stack, target, backwards)),
            (None, Some(stack)) => Some((stack, selection, !backwards)),
            _ => None,
        };
        if let Some((stack, moving, prepend)) = join {
            let first = stack.first_child(self.tiles.map()).filter(|_| prepend);

            if let Some(first) = first {
                self.tiles.tree.move_node(moving, Placement::Before(first));
            } else {
                self.tiles.tree.move_node(moving, Placement::Back(stack));
            }
            self.tiles.select(stack);
            return;
        }

        let kind = LayoutKind::from(direction.orientation());
        if let (Some(sp), Some(tp)) = (selection_parent, target_parent) {
            if sp == tp && self.tiles.layout(sp).is_stacked() {
                self.tiles.set_layout(sp, kind);
                self.tiles.select(sp);
                return;
            }
            if sp != tp && self.are_containers_mergeable(sp, tp, direction) {
                let parent = self.find_or_create_smart_common_parent(sp, tp, direction);
                self.tiles.set_layout(parent, kind);
                self.tiles.select(parent);
                return;
            }
        }
        let parent = self.find_or_create_smart_common_parent(selection, target, direction);
        if !self.tiles.layout(parent).is_stacked() {
            self.tiles.set_layout(parent, kind);
        }
        self.tiles.select(parent);
    }

    fn are_containers_mergeable(
        &self,
        container1: NodeId,
        container2: NodeId,
        direction: Direction,
    ) -> bool {
        let layout1 = self.tiles.layout(container1);
        let layout2 = self.tiles.layout(container2);

        layout1.orientation() == direction.orientation()
            && layout2.orientation() == direction.orientation()
            && !layout1.is_group()
            && !layout2.is_group()
    }
}

impl LayoutSystem for TraditionalLayoutSystem {
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.tiles.has_windows_for_app(layout, pid)
    }

    fn window_participates(&self, window: WindowId) -> bool {
        self.tiles.window_participates(window)
    }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        self.tiles.set_window_participation(window, participates)
    }

    fn create_layout(&mut self) -> LayoutId { self.tiles.create_layout() }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.tiles.contains_layout(layout) }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId { self.tiles.clone_layout(layout) }

    fn remove_layout(&mut self, layout: LayoutId) { self.tiles.remove_layout(layout) }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        self.tiles.project(
            self.tiles.root(layout),
            None,
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
        )
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.tiles.selected_window(layout)
    }

    fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        self.tiles.selection_contains_window(layout, window)
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.tiles.window_in_direction(layout, direction)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.tiles.contains_window(layout, wid)
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.tiles.has_any_fullscreen_node(layout)
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.tiles.parent_of_selection_is_stacked(layout)
    }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        self.tiles.windows(layout, query)
    }

    fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        self.tiles.focus(layout, focus)
    }

    fn update(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Add(layout, window, Some(direction)) => {
                let wid = window;

                let mut root = self.tiles.root(layout);
                if root.children(self.tiles.map()).next().is_some()
                    && (self.tiles.layout(root).is_stacked()
                        || self.tiles.layout(root).orientation() != direction.orientation())
                {
                    root = self.tiles.nest_in_container_internal(
                        layout,
                        root,
                        LayoutKind::from(direction.orientation()),
                    );
                }
                self.tiles.tree.map.set_kind(root, LayoutKind::from(direction.orientation()));
                let node = self.tiles.add_window_under(layout, root, wid);
                if matches!(direction, Direction::Right | Direction::Down) {
                    if let Some(first) =
                        root.first_child(self.tiles.map()).filter(|&first| first != node)
                    {
                        self.tiles.tree.move_node(node, Placement::Before(first));
                    }
                }
                self.tiles.select(node);
            }
            WindowChange::Add(layout, window, None) => {
                self.add_window_after_selection(layout, window)
            }
            WindowChange::Reconcile(layout, pid, desired) => {
                for wid in self.tiles.reconcile_windows(layout, pid, desired) {
                    self.add_window_after_selection(layout, wid);
                }
            }
            _ => self.tiles.update_existing(change),
        }
    }

    fn edit_layout(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult {
        match edit {
            LayoutEdit::Move {
                direction,
                has_display_neighbor,
            } => EditResult::from(self.move_selection_with_display_neighbor(
                layout,
                direction,
                has_display_neighbor,
            )),
            LayoutEdit::Transfer(to) => self.tiles.transfer_selection(layout, to),
            LayoutEdit::Split(kind) => self.tiles.split_selection(layout, kind),
            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.tiles.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::Swap(a, b) => EditResult::from(self.tiles.swap_windows(layout, a, b)),
            LayoutEdit::ReorderTab { window, target } => {
                EditResult::from(self.tiles.move_stack_tab(layout, window, target))
            }
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(self.apply_target_drop(layout, source, target, action))
            }
            LayoutEdit::Group(GroupEdit::Join(direction)) => {
                self.join_selection_with_direction(layout, direction);
                EditResult::Changed
            }
            LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)) => {
                let selection = self.tiles.selection(layout);
                let is_joined = selection
                    .parent(self.tiles.map())
                    .and_then(|parent| parent.parent(self.tiles.map()))
                    .is_some();

                if is_joined {
                    self.unjoin_selection(layout);
                } else {
                    self.join_selection_with_direction(layout, direction);
                };
                EditResult::Changed
            }
            LayoutEdit::Group(GroupEdit::Stack(orientation)) => EditResult::Raise(
                self.tiles.apply_stacking_to_parent_of_selection(layout, orientation),
            ),
            LayoutEdit::Group(GroupEdit::Unstack(orientation)) => {
                EditResult::Raise(self.tiles.unstack_parent_of_selection(layout, orientation))
            }
            LayoutEdit::Group(GroupEdit::Unjoin) => {
                self.unjoin_selection(layout);
                EditResult::Changed
            }
            LayoutEdit::Resize { amount, orientation } => {
                self.tiles.resize_selection(layout, amount, orientation)
            }
            LayoutEdit::Rebalance => {
                let root = self.tiles.root(layout);
                self.tiles.rebalance_node(root);
                EditResult::Changed
            }
            LayoutEdit::ToggleOrientation => self.tiles.toggle_orientation(layout),
        }
    }
}

impl TraditionalLayoutSystem {
    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.window_insertion_point == WindowInsertionPoint::EndOfTree {
            let root = self.tiles.root(layout);
            let node = self.tiles.add_window_under(layout, root, wid);
            self.tiles.select(node);
            return;
        }
        let selection = self.tiles.selection(layout);
        let node = if selection.parent(self.tiles.map()).is_none() {
            // If the root is selected but it already has children, split relative to the
            // root's active child instead of appending a fresh full-weight sibling.
            if let Some(anchor) = self
                .tiles
                .local_selection(selection)
                .or_else(|| selection.last_child(self.tiles.map()))
            {
                self.smart_window_insertion(layout, anchor, wid)
            } else {
                self.tiles.add_window_under(layout, selection, wid)
            }
        } else {
            let node = self.smart_window_insertion(layout, selection, wid);
            node
        };
        self.tiles.select(node);
    }

    fn move_selection_with_display_neighbor(
        &mut self,
        layout: LayoutId,
        direction: Direction,
        has_neighbor: bool,
    ) -> bool {
        let selection = self.tiles.selection(layout);
        if has_neighbor
            && !selection
                .ancestors(self.tiles.map())
                .any(|node| self.tiles.move_over(node, direction).is_some())
        {
            return false;
        }
        {
            if !self.tiles.inactive.is_empty()
                && self.tiles.find_best_focus_target(self.tiles.root(layout)).is_none()
            {
                return false;
            }
            let selection = self.tiles.selection(layout);
            self.move_node(layout, selection, direction)
        }
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        if !self.tiles.inactive.is_empty()
            && self.tiles.find_best_focus_target(self.tiles.root(layout)).is_none()
        {
            return;
        }
        let mut selection = self.tiles.selection(layout);

        if let Some(target) = self.find_natural_join_target(selection, direction) {
            let map = self.tiles.map();

            // If the selection is a leaf at the edge of a container that matches the direction,
            // lift the selection to that container so we can merge whole groups.
            if let Some(parent) = selection.parent(map) {
                let parent_layout = self.tiles.layout(parent);
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
            let selection_layout = self.tiles.layout(selection);
            let target_is_ancestor = target.ancestors(map).any(|a| a == selection);
            let selection_is_ancestor = selection.ancestors(map).any(|a| a == target);
            if self.tiles.window_at(selection).is_none()
                && selection_layout.orientation() == direction.orientation()
                && !selection_layout.is_group()
                && !target_is_ancestor
                && !selection_is_ancestor
                && target.parent(map) != Some(selection)
            {
                match direction {
                    Direction::Right | Direction::Down => {
                        self.tiles.tree.move_node(target, Placement::Back(selection));
                    }
                    Direction::Left | Direction::Up => {
                        if let Some(first) = selection.first_child(map) {
                            self.tiles.tree.move_node(target, Placement::Before(first));
                        } else {
                            self.tiles.tree.move_node(target, Placement::Back(selection));
                        }
                    }
                }
                self.tiles.select(selection);
                return;
            }

            self.perform_natural_join(selection, target, direction);
            if self.tiles.tree.map.window(selection).is_some() {
                self.tiles.select(selection);
            } else {
                let _ = self.tiles.descend_selection(layout);
            }
        }
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        if !self.tiles.inactive.is_empty()
            && self.tiles.find_best_focus_target(self.tiles.root(layout)).is_none()
        {
            return;
        }
        let map = &self.tiles.tree.map;
        let Some(parent) = self.tiles.selection(layout).parent(map) else {
            return;
        };
        let Some(grandparent) = parent.parent(map) else { return };

        let local_selected_child = self.tiles.tree.map.local_selection(parent);
        let next_sibling = parent.next_sibling(map);
        let parent_size = map[parent].layout.size.max(0.0);
        // Save weights before detach events normalize the remaining siblings.
        let children: Vec<_> = parent
            .children(map)
            .map(|child| (child, map[child].layout.size.max(0.0)))
            .collect();
        let total: f32 = children.iter().map(|(_, weight)| weight).sum();
        let count = children.len() as f32;
        for (child, weight) in children {
            let promoted_size = if total.is_finite() && total > f32::EPSILON {
                parent_size * weight / total
            } else {
                parent_size / count
            };

            if let Some(next_sibling) = next_sibling {
                self.tiles.tree.move_node(child, Placement::Before(next_sibling));
            } else {
                self.tiles.tree.move_node(child, Placement::Back(grandparent));
            }
            self.tiles.tree.map[child].layout.size = promoted_size;
        }
        self.tiles.tree.remove(parent);
        if let Some(selected) =
            local_selected_child.or_else(|| grandparent.first_child(self.tiles.map()))
        {
            self.tiles.select(selected);
        }
    }

    fn apply_target_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
    ) -> bool {
        if !self.tiles.window_participates(source) || !self.tiles.window_participates(target) {
            return false;
        }
        self.tiles.apply_explicit_window_drop(layout, source, target, action)
    }
}

impl TraditionalLayoutSystem {
    fn split_new_sibling_from_selection(&mut self, selection: NodeId, new_sibling: NodeId) {
        let map = &self.tiles.tree.map;
        let Some(parent) = selection.parent(map) else {
            return;
        };
        if new_sibling.parent(map) != Some(parent) {
            return;
        }

        if self.equalize_nodes {
            // Sway initializes a new node to the average weight of the existing siblings,
            // then normalizes the sibling weights. Rift keeps every sibling set normalized
            // to its child count, so that average is 1.0. Leaving the observer-initialized
            // weight intact gives identical proportions while preserving manual resizes.
            return;
        }

        let selected_size = self.tiles.tree.map[selection].layout.size.max(0.0);
        if selected_size <= f32::EPSILON {
            // Recover from stale/uninitialized size metadata by forcing an even split.
            self.tiles.tree.map[selection].layout.size = 1.0;
            self.tiles.tree.map[new_sibling].layout.size = 1.0;
        } else {
            self.tiles.tree.map[selection].layout.size = selected_size * 0.5;
            self.tiles.tree.map[new_sibling].layout.size = selected_size * 0.5;
        }
    }
}

impl TraditionalLayoutSystem {
    fn move_node(&mut self, layout: LayoutId, moving_node: NodeId, direction: Direction) -> bool {
        let map = &self.tiles.tree.map;
        let Some(old_parent) = moving_node.parent(map) else {
            return false;
        };
        let is_selection = self.tiles.tree.map.local_selection(old_parent) == Some(moving_node);
        let moved = self.move_node_inner(layout, moving_node, direction);
        if moved && is_selection {
            let mut current = Some(moving_node);
            while let Some(node) = current.filter(|&node| node != old_parent) {
                current = node.parent(&self.tiles.tree.map);
                self.tiles.tree.map.select_locally(node);
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
        let map = &self.tiles.tree.map;
        let destination;
        if let Some(sibling) = self.tiles.move_over(moving_node, direction) {
            let mut node = sibling;
            let target = loop {
                let Some(next) =
                    self.tiles.tree.map.local_selection(node).or(node.first_child(map))
                else {
                    break node;
                };
                if self.tiles.tree.map.kind(node).orientation() == direction.orientation() {
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
            let target_ancestor = moving_node
                .ancestors_with_parent(&self.tiles.tree.map)
                .skip(1)
                .find(|(_node, parent)| {
                    parent
                        .map(|p| self.tiles.layout(p).orientation() == direction.orientation())
                        .unwrap_or(false)
                });
            if let Some((target, _parent)) = target_ancestor {
                destination = Destination::Ahead(target);
            } else {
                let old_root = moving_node.ancestors(map).last().unwrap();
                let root_kind = self.tiles.tree.map.kind(old_root);
                // Leaving the edge of a root group splits the window out of it; at the edge
                // of a same-oriented split there is nowhere further to go.
                if root_kind.orientation() == direction.orientation() && !root_kind.is_group() {
                    return false;
                }
                let new_container_kind = LayoutKind::from(direction.orientation());
                self.tiles.nest_in_container_internal(layout, old_root, new_container_kind);
                destination = Destination::Ahead(old_root);
            }
        }
        let (target, before) = match (destination, direction) {
            (Destination::Ahead(target), Direction::Left | Direction::Up)
            | (Destination::Behind(target), Direction::Right | Direction::Down) => (target, true),
            (Destination::Ahead(target), Direction::Right | Direction::Down)
            | (Destination::Behind(target), Direction::Left | Direction::Up) => (target, false),
        };
        if moving_node.parent(&self.tiles.tree.map) == target.parent(&self.tiles.tree.map) {
            moving_node.reorder(target, before, &mut self.tiles.tree.map);
        } else {
            if before {
                self.tiles.tree.move_node(moving_node, Placement::Before(target));
            } else {
                self.tiles.tree.move_node(moving_node, Placement::After(target));
            }
        }
        true
    }

    fn find_or_create_common_parent_internal(&mut self, node1: NodeId, node2: NodeId) -> NodeId {
        let map = self.tiles.map();
        let ancestor =
            node1.common_ancestor(node2, map).expect("nodes must belong to the same tree");
        if ancestor == node1 || ancestor == node2 {
            return ancestor;
        }

        if node1.parent(map) == Some(ancestor) && node2.parent(map) == Some(ancestor) {
            let size1 = self.tiles.tree.map[node1].layout.size.max(0.0);
            let size2 = self.tiles.tree.map[node2].layout.size.max(0.0);
            let new_container = self.tiles.tree.insert(Placement::Before(node1));
            self.tiles
                .tree
                .move_node_with(node1, Placement::Back(new_container), |child, tree| {
                    tree.map[child].layout.size = size1;
                });
            self.tiles
                .tree
                .move_node_with(node2, Placement::Back(new_container), |child, tree| {
                    tree.map[child].layout.size = size2;
                });

            // A collapsed parent already transferred its outer weight.
            if self.tiles.tree.map.contains(ancestor) {
                self.tiles.tree.map[new_container].layout.size = size1 + size2;
            }
            return new_container;
        }
        let container = self.tiles.tree.insert(Placement::Back(ancestor));
        self.tiles.tree.map.set_kind(container, LayoutKind::Horizontal);
        self.tiles.tree.move_node(node1, Placement::Back(container));
        self.tiles.tree.move_node(node2, Placement::Back(container));
        container
    }
}

impl TraditionalLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        self.tiles.draw_tree(layout, None)
    }

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        self.tiles.container_tree(layout)
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        self.tiles.window_slot(layout, window)
    }
}
#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::layout_engine::{Direction, LayoutKind};

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn same_layout_transfer_preserves_the_selected_tile() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        system.update(WindowChange::Add(layout, w(1), None));
        let before = system.container_tree(layout);
        assert!(!system.edit(layout, LayoutEdit::Transfer(layout)).changed());
        assert_eq!(system.container_tree(layout), before);
        assert_eq!(system.selected_window(layout), Some(w(1)));
    }

    #[test]
    fn unjoin_preserves_nested_and_outer_resize_ratios() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.tiles.create_layout();
        let root = system.tiles.root(layout);
        system.tiles.tree.map.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(163);
        let w2 = w(164);
        let w3 = w(165);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);
        assert!(system.tiles.focus(layout, Focus::Select(w1)).handled);
        system.join_selection_with_direction(layout, Direction::Right);

        let n1 = system.tiles.tree.window.node_for(layout, w1).unwrap();
        let n2 = system.tiles.tree.window.node_for(layout, w2).unwrap();
        let n3 = system.tiles.tree.window.node_for(layout, w3).unwrap();
        let group = n1.parent(system.tiles.map()).unwrap();
        system.tiles.tree.map[group].layout.size = 6.0;
        system.tiles.tree.map[n3].layout.size = 4.0;

        system.tiles.tree.map[n1].layout.size = 3.0;
        system.tiles.tree.map[n2].layout.size = 1.0;

        system.tiles.select(n1);
        system.unjoin_selection(layout);

        assert!((system.tiles.tree.map[n1].layout.size - 4.5).abs() < 0.0001);
        assert!((system.tiles.tree.map[n2].layout.size - 1.5).abs() < 0.0001);
        assert!((system.tiles.tree.map[n3].layout.size - 4.0).abs() < 0.0001);
        assert!((system.tiles.tree.map.children_total(root) - 10.0).abs() < 0.0001);
    }

    #[test]
    fn reordering_siblings_preserves_user_resize_ratios() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.tiles.create_layout();
        let root = system.tiles.root(layout);
        system.tiles.tree.map.set_kind(root, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(178));
        system.add_window_after_selection(layout, w(179));
        system.add_window_after_selection(layout, w(180));

        let n1 = system.tiles.tree.window.node_for(layout, w(178)).unwrap();
        let n2 = system.tiles.tree.window.node_for(layout, w(179)).unwrap();
        let n3 = system.tiles.tree.window.node_for(layout, w(180)).unwrap();
        system.tiles.tree.map[n1].layout.size = 5.0;
        system.tiles.tree.map[n2].layout.size = 2.0;
        system.tiles.tree.map[n3].layout.size = 1.0;

        assert!(system.move_node(layout, n2, Direction::Right));

        assert_eq!(root.children(system.tiles.map()).collect::<Vec<_>>(), vec![
            n1, n3, n2
        ]);
        assert!((system.tiles.tree.map[n1].layout.size - 5.0).abs() < 0.0001);
        assert!((system.tiles.tree.map[n2].layout.size - 2.0).abs() < 0.0001);
        assert!((system.tiles.tree.map[n3].layout.size - 1.0).abs() < 0.0001);
        assert!((system.tiles.tree.map.children_total(root) - 8.0).abs() < 0.0001);
    }

    #[test]
    fn native_tabs_reserve_the_configured_edge_without_changing_stack_direction() {
        use crate::common::config::{HorizontalPlacement, StackLineReserve, VerticalPlacement};
        let rect = |x, y, w, h| CGRect::new(CGPoint::new(x, y), CGSize::new(w, h));
        let cases = [
            (
                LayoutKind::HorizontalStack,
                HorizontalPlacement::Top,
                VerticalPlacement::Left,
                rect(100.0, 229.0, 400.0, 271.0),
            ),
            (
                LayoutKind::HorizontalStack,
                HorizontalPlacement::Bottom,
                VerticalPlacement::Left,
                rect(100.0, 200.0, 400.0, 271.0),
            ),
            (
                LayoutKind::VerticalStack,
                HorizontalPlacement::Top,
                VerticalPlacement::Left,
                rect(129.0, 200.0, 371.0, 300.0),
            ),
            (
                LayoutKind::VerticalStack,
                HorizontalPlacement::Top,
                VerticalPlacement::Right,
                rect(100.0, 200.0, 371.0, 300.0),
            ),
        ];
        for (kind, horiz, vert, expected) in cases {
            let mut system = TraditionalLayoutSystem::default();
            let layout = system.tiles.create_layout();
            let root = system.tiles.root(layout);
            system.tiles.tree.map.set_kind(root, kind);
            let a = w(170);
            let b = w(171);
            system.tiles.add_window_under(layout, root, a);
            system.tiles.add_window_under(layout, root, b);
            system.tiles.focus(layout, Focus::Select(b)).handled;
            let screen = rect(100.0, 200.0, 400.0, 300.0);
            let reserve = StackLineReserve {
                thickness: 28.0,
                spacing: 1.0,
                horiz,
                vert,
            };
            let frames = system.calculate_layout(
                layout,
                screen,
                0.0,
                &HashMap::default(),
                &Default::default(),
                reserve,
            );
            for (_, frame) in frames {
                assert_eq!(frame, expected, "{kind:?} {horiz:?} {vert:?}");
            }
            // The bar sits on the reserved edge, flush with the stack's outer frame.
            let bar = reserve.bar(screen, kind == LayoutKind::HorizontalStack);
            let expected_bar = match (kind, horiz, vert) {
                (LayoutKind::HorizontalStack, HorizontalPlacement::Top, _) => {
                    rect(100.0, 200.0, 400.0, 28.0)
                }
                (LayoutKind::HorizontalStack, HorizontalPlacement::Bottom, _) => {
                    rect(100.0, 472.0, 400.0, 28.0)
                }
                (_, _, VerticalPlacement::Left) => rect(100.0, 200.0, 28.0, 300.0),
                (_, _, VerticalPlacement::Right) => rect(472.0, 200.0, 28.0, 300.0),
            };
            assert_eq!(bar, expected_bar);
            assert_eq!(system.tiles.tree.map.kind(root), kind);
        }
    }
}
