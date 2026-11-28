use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::groups::{GroupLayouts, Seat};
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
    pub(super) tiles: GroupLayouts,
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
            tiles: GroupLayouts::default(),
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
        let mut placement = Placement::After(selection);
        if let Seat::Pane { split, orientation } = self.tiles.seat(selection)
            && split.children(self.tiles.map()).nth(3).is_some()
        {
            let kind = LayoutKind::from(orientation);
            placement =
                Placement::Back(self.tiles.nest_in_container_internal(layout, selection, kind));
        }
        let node = self.tiles.tree.insert(placement);
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
        if let (Seat::Pane { split, orientation }, Seat::Pane { split: other, .. }) =
            (self.tiles.seat(node1), self.tiles.seat(node2))
            && split == other
            && orientation == direction.orientation()
            && split.children(self.tiles.map()).count() == 2
        {
            return split;
        }

        self.find_or_create_common_parent_internal(node1, node2)
    }

    fn find_natural_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let sibling = self.find_direct_sibling_target(from, direction);
        if let Seat::Pane { split, orientation } = self.tiles.seat(from)
            && orientation == direction.orientation()
            && (sibling.is_none() || split.children(self.tiles.map()).count() == 2)
            && let Some(neighbor) = self.find_direct_sibling_target(split, direction)
        {
            return Some(neighbor);
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
        // Joining toward a stack joins it, whether the target is one of its tabs or the stack
        // itself.
        let selection_seat = self.tiles.seat(selection);
        let target_seat = self.tiles.seat(target);
        // A side's stack: the node itself if it is one, else its parent if that is a stack. Unlike
        // own_or_enclosing_stack this never looks past the parent, on purpose: it is the join's
        // own rule.
        let own_or_parent_stack = |node: NodeId, seat: Seat| {
            if self.tiles.is_stack(node) {
                Some(node)
            } else {
                seat.stack()
            }
        };
        let backwards = matches!(direction, Direction::Left | Direction::Up);
        let join = match (
            own_or_parent_stack(selection, selection_seat),
            own_or_parent_stack(target, target_seat),
        ) {
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

        let axis = direction.orientation();
        let kind = LayoutKind::from(axis);
        match (selection_seat, target_seat) {
            // Two tabs of one stack: lay the stack out as a split.
            (Seat::Tab { stack, .. }, Seat::Tab { stack: other, .. }) if stack == other => {
                self.tiles.set_layout(stack, kind);
                self.tiles.select(stack);
                return;
            }
            // Panes of two splits along the join's axis: merge the splits.
            (
                Seat::Pane { split, orientation },
                Seat::Pane {
                    split: other,
                    orientation: other_orientation,
                },
            ) if split != other && orientation == axis && other_orientation == axis => {
                let parent = self.find_or_create_smart_common_parent(split, other, direction);
                self.tiles.set_layout(parent, kind);
                self.tiles.select(parent);
                return;
            }
            _ => {}
        }
        let parent = self.find_or_create_smart_common_parent(selection, target, direction);
        if !self.tiles.layout(parent).is_stacked() {
            self.tiles.set_layout(parent, kind);
        }
        self.tiles.select(parent);
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
        let root = self.tiles.root(layout);
        super::tiles::project(
            &self.tiles,
            root,
            |_| root,
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
            true,
            None,
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
            WindowChange::Add(layout, window, direction) => {
                if self.tiles.contains_window(layout, window) {
                    return;
                }
                let Some(direction) = direction else {
                    self.add_window_after_selection(layout, window);
                    return;
                };
                let mut root = self.tiles.root(layout);
                let kind = LayoutKind::from(direction.orientation());
                let current = self.tiles.layout(root);
                if !root.is_empty(self.tiles.map())
                    && (current.is_stacked() || current.orientation() != kind.orientation())
                {
                    root = self.tiles.nest_in_container_internal(layout, root, kind);
                }
                self.tiles.set_layout(root, kind);
                let node = self.tiles.add_window_under(layout, root, window);
                if matches!(direction, Direction::Right | Direction::Down)
                    && let Some(first) = root.first_child(self.tiles.map())
                {
                    self.tiles.tree.move_node(node, Placement::Before(first));
                }
                self.tiles.select(node);
            }
            WindowChange::Remove(window) => {
                self.tiles.remove_window(window);
            }
            WindowChange::RemoveAndRebalance(window) => {
                super::tiles::remove_and_rebalance(&mut self.tiles, window)
            }
            WindowChange::Replace(from, to) => self.tiles.replace_window(from, to),
            WindowChange::CloseApp(pid) => self.tiles.close_app(pid),
            WindowChange::Resize {
                layout,
                window,
                old,
                new,
                screen,
                gaps,
            } => {
                super::tiles::resize_window(
                    &mut self.tiles,
                    layout,
                    window,
                    old,
                    new,
                    screen,
                    gaps,
                );
            }
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

            LayoutEdit::Split(kind) => self.tiles.split_selection(layout, kind),
            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.tiles.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::ExitFullscreen(window) => self.tiles.exit_fullscreen(layout, window),
            LayoutEdit::Swap(a, b) => EditResult::from(self.tiles.swap_windows(layout, a, b)),
            LayoutEdit::ReorderTab { window, target } => {
                EditResult::from(self.tiles.move_stack_tab(layout, window, target))
            }
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(super::tiles::apply_explicit_window_drop(
                    &mut self.tiles,
                    layout,
                    source,
                    target,
                    action,
                ))
            }
            LayoutEdit::Group(GroupEdit::Join(direction)) => {
                EditResult::from(self.join_selection_with_direction(layout, direction))
            }
            LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)) => {
                let selection = self.tiles.selection(layout);
                let is_joined = selection
                    .parent(self.tiles.map())
                    .and_then(|parent| parent.parent(self.tiles.map()))
                    .is_some();

                EditResult::from(if is_joined {
                    self.unjoin_selection(layout)
                } else {
                    self.join_selection_with_direction(layout, direction)
                })
            }
            LayoutEdit::Group(GroupEdit::Stack(orientation)) => EditResult::Raise(
                self.tiles.apply_stacking_to_parent_of_selection(layout, orientation),
            ),
            LayoutEdit::Group(GroupEdit::Unstack(orientation)) => {
                EditResult::Raise(self.tiles.unstack_parent_of_selection(layout, orientation))
            }
            LayoutEdit::Group(GroupEdit::Unjoin) => EditResult::from(self.unjoin_selection(layout)),
            LayoutEdit::Resize { amount, orientation } => {
                super::tiles::resize_selection(&mut self.tiles, layout, amount, orientation)
            }
            LayoutEdit::Rebalance => {
                let root = self.tiles.root(layout);
                EditResult::from(super::tiles::rebalance(&mut self.tiles.tree.map, root))
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
            self.smart_window_insertion(layout, selection, wid)
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
        if !self.tiles.inactive.is_empty() && self.tiles.focused_leaf(layout).is_none() {
            return false;
        }
        self.move_node(layout, selection, direction)
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) -> bool {
        if !self.tiles.inactive.is_empty()
            && self.tiles.find_best_focus_target(self.tiles.root(layout)).is_none()
        {
            return false;
        }
        let mut selection = self.tiles.selection(layout);

        let Some(target) = self.find_natural_join_target(selection, direction) else {
            return false;
        };
        let map = self.tiles.map();

        // If the selection is a leaf at the edge of a container that matches the direction,
        // lift the selection to that container so we can merge whole groups.
        if let Seat::Pane { split: parent, orientation } = self.tiles.seat(selection) {
            let is_edge = match direction {
                Direction::Right | Direction::Down => selection.next_sibling(map).is_none(),
                Direction::Left | Direction::Up => selection.prev_sibling(map).is_none(),
            };

            if orientation == direction.orientation()
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
            return true;
        }

        self.perform_natural_join(selection, target, direction);
        if self.tiles.tree.map.window(selection).is_some() {
            self.tiles.select(selection);
        } else {
            let _ = self.tiles.descend_selection(layout);
        }
        true
    }

    fn unjoin_selection(&mut self, layout: LayoutId) -> bool {
        if !self.tiles.inactive.is_empty()
            && self.tiles.find_best_focus_target(self.tiles.root(layout)).is_none()
        {
            return false;
        }
        let map = &self.tiles.tree.map;
        let Some(parent) = self.tiles.selection(layout).parent(map) else {
            return false;
        };
        let Some(grandparent) = parent.parent(map) else {
            return false;
        };

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
        true
    }

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
    pub(in crate::layout_engine) fn can_move_window(
        &self,
        layout: LayoutId,
        window: WindowId,
        direction: Direction,
    ) -> bool {
        self.window_participates(window)
            && self
                .tiles
                .window_node(layout, window)
                .is_some_and(|node| self.move_destination(node, direction).is_some())
    }

    fn move_node(&mut self, layout: LayoutId, moving_node: NodeId, direction: Direction) -> bool {
        let map = &self.tiles.tree.map;
        let Some(old_parent) = moving_node.parent(map) else {
            return false;
        };
        let is_selection = self.tiles.tree.map.local_selection(old_parent) == Some(moving_node);
        let Some((placement, wrap_root)) = self.move_destination(moving_node, direction) else {
            return false;
        };
        if let Some(root) = wrap_root {
            self.tiles.nest_in_container_internal(
                layout,
                root,
                LayoutKind::from(direction.orientation()),
            );
        }
        let moved = self.tiles.tree.move_node(moving_node, placement);
        if moved && is_selection {
            let mut current = Some(moving_node);
            while let Some(node) = current.filter(|&node| node != old_parent) {
                current = node.parent(&self.tiles.tree.map);
                self.tiles.tree.map.select_locally(node);
            }
        }
        moved
    }

    fn move_destination(
        &self,
        moving_node: NodeId,
        direction: Direction,
    ) -> Option<(Placement, Option<NodeId>)> {
        let map = self.tiles.map();
        moving_node.parent(map)?;
        let mut wrap_root = None;
        let (target, behind) = if let Some(sibling) = self.tiles.move_over(moving_node, direction) {
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
            (target, target != sibling)
        } else {
            // A stack's axis counts here as a split's does.
            let target_ancestor = moving_node
                .ancestors_with_parent(&self.tiles.tree.map)
                .skip(1)
                .find(|&(_, parent)| {
                    self.tiles.seat_under(parent).axis() == Some(direction.orientation())
                });
            if let Some((target, _parent)) = target_ancestor {
                (target, false)
            } else {
                let old_root = moving_node.ancestors(map).last().unwrap();
                let root_kind = self.tiles.tree.map.kind(old_root);
                // Leaving the edge of a root group splits the window out of it; at the edge
                // of a same-oriented split there is nowhere further to go.
                if root_kind.orientation() == direction.orientation() && !root_kind.is_group() {
                    return None;
                }
                wrap_root = Some(old_root);
                (old_root, false)
            }
        };
        let before = matches!(direction, Direction::Left | Direction::Up) != behind;
        let placement = if before {
            Placement::Before(target)
        } else {
            Placement::After(target)
        };
        Some((placement, wrap_root))
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
            let tree = &mut self.tiles.tree;
            let new_container = tree.insert(Placement::Before(node1));
            // Put both shares into the replacement before the moves normalize
            // the source siblings. All surviving weights then scale together.
            tree.map[new_container].layout.size = size1 + size2;
            tree.map[node1].layout.size = 0.0;
            tree.map[node2].layout.size = 0.0;
            tree.move_node(node1, Placement::Back(new_container));
            tree.move_node(node2, Placement::Back(new_container));
            tree.map[node1].layout.size = size1;
            tree.map[node2].layout.size = size2;
            return new_container;
        }
        let container = self.tiles.tree.insert(Placement::Back(ancestor));
        self.tiles.tree.map.set_kind(container, LayoutKind::Horizontal);
        self.tiles.tree.move_node(node1, Placement::Back(container));
        self.tiles.tree.move_node(node2, Placement::Back(container));
        container
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::common::config::StackDefaultOrientation;
    use crate::layout_engine::{Direction, LayoutKind, Orientation};

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    /// A layout whose root lies along `axis`, holding `order`'s windows, with window 1 and the
    /// window after it joined and stacked.
    fn stacked_pair(axis: Orientation, order: &[WindowId]) -> (TraditionalLayoutSystem, LayoutId) {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.tiles.root(layout);
        system.tiles.set_layout(root, LayoutKind::from(axis));
        for &window in order {
            system.add_window_after_selection(layout, window);
        }
        let forward = match axis {
            Orientation::Horizontal => Direction::Right,
            Orientation::Vertical => Direction::Down,
        };
        system.focus(layout, Focus::Select(w(1)));
        system.edit(layout, LayoutEdit::Group(GroupEdit::Join(forward)));
        let _ = system.edit(
            layout,
            LayoutEdit::Group(GroupEdit::Stack(StackDefaultOrientation::Same)),
        );
        assert_eq!(system.windows(layout, WindowQuery::Stack(w(1))), [w(1), w(2)]);
        (system, layout)
    }

    fn parent_of(
        system: &TraditionalLayoutSystem,
        layout: LayoutId,
        window: WindowId,
    ) -> Option<NodeId> {
        let tiles = &system.tiles;
        tiles.window_node(layout, window).and_then(|node| node.parent(tiles.map()))
    }

    #[test]
    fn splitting_a_narrow_tile_preserves_the_other_tiles_width() {
        let mut system = TraditionalLayoutSystem::new(WindowInsertionPoint::NextToSelection, false);
        let layout = system.create_layout();
        for window in [w(1), w(2)] {
            system.update(WindowChange::Add(layout, window, None));
        }
        for amount in [-0.25, -0.9] {
            assert!(
                system
                    .edit(layout, LayoutEdit::Resize {
                        amount,
                        orientation: crate::layout_engine::ResizeOrientation::Horizontal,
                    })
                    .changed()
            );
        }

        let widths = |system: &TraditionalLayoutSystem| {
            system
                .calculate_layout(
                    layout,
                    CGRect::new(CGPoint::ZERO, CGSize::new(4000., 800.)),
                    0.0,
                    &Default::default(),
                    &Default::default(),
                    Default::default(),
                )
                .into_iter()
                .map(|(window, frame)| (window, frame.size.width))
                .collect::<HashMap<_, _>>()
        };
        let before = widths(&system);
        assert_eq!(before[&w(2)], 100.);
        system.update(WindowChange::Add(layout, w(3), None));
        let after = widths(&system);
        assert_eq!(after[&w(1)], before[&w(1)]);
        assert_eq!(after[&w(2)], 50.);
        assert_eq!(after[&w(3)], 50.);
    }

    #[test]
    fn joining_resized_neighbors_preserves_the_unrelated_tiles_width() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3)] {
            system.update(WindowChange::Add(layout, window, None));
        }
        assert!(
            system
                .edit(layout, LayoutEdit::Resize {
                    amount: 0.2,
                    orientation: crate::layout_engine::ResizeOrientation::Horizontal,
                })
                .changed()
        );
        let frame = |system: &TraditionalLayoutSystem| {
            system
                .calculate_layout(
                    layout,
                    CGRect::new(CGPoint::ZERO, CGSize::new(1200., 800.)),
                    0.0,
                    &Default::default(),
                    &Default::default(),
                    Default::default(),
                )
                .into_iter()
                .find(|(window, _)| *window == w(3))
                .unwrap()
                .1
        };
        let before = frame(&system);
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        assert!(
            system
                .edit(layout, LayoutEdit::Group(GroupEdit::Join(Direction::Right)))
                .changed()
        );
        assert_eq!(frame(&system), before);
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

    #[test]
    fn join_adds_a_window_to_the_neighboring_stack_in_every_direction() {
        // The stack holds windows 1 and 2; window 3 stands alone on one side of it.
        let build = |axis, lone_first| {
            let order = if lone_first {
                [w(3), w(1), w(2)]
            } else {
                [w(1), w(2), w(3)]
            };
            stacked_pair(axis, &order)
        };

        // (direction, lone window first, window selected, stack after the join)
        let beside_the_stack = [
            (Direction::Left, false, w(3), [w(1), w(2), w(3)]),
            (Direction::Up, false, w(3), [w(1), w(2), w(3)]),
            (Direction::Right, true, w(3), [w(3), w(1), w(2)]),
            (Direction::Down, true, w(3), [w(3), w(1), w(2)]),
        ];
        let from_the_stack = [
            (Direction::Right, false, w(2), [w(1), w(2), w(3)]),
            (Direction::Down, false, w(2), [w(1), w(2), w(3)]),
            (Direction::Left, true, w(1), [w(3), w(1), w(2)]),
            (Direction::Up, true, w(1), [w(3), w(1), w(2)]),
        ];
        for (direction, lone_first, selected, expected) in
            beside_the_stack.into_iter().chain(from_the_stack)
        {
            let (mut system, layout) = build(direction.orientation(), lone_first);
            system.focus(layout, Focus::Select(selected));
            system.edit(layout, LayoutEdit::Group(GroupEdit::Join(direction)));
            let case = format!("join {direction:?} with window {selected:?} selected");
            assert_eq!(
                system.windows(layout, WindowQuery::Stack(w(3))),
                expected,
                "{case}: stack order"
            );
            assert_eq!(
                system.selected_window(layout),
                Some(selected),
                "{case}: selection"
            );
            assert_eq!(
                system.windows(layout, WindowQuery::Visible),
                [selected],
                "{case}: visible tab"
            );
        }
    }

    #[test]
    fn joining_two_tabs_of_one_stack_lays_the_stack_out_as_a_split() {
        let (mut system, layout) = stacked_pair(Orientation::Horizontal, &[w(1), w(2), w(3)]);
        system.focus(layout, Focus::Select(w(1)));
        system.edit(layout, LayoutEdit::Group(GroupEdit::Join(Direction::Right)));

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(1))), []);
        assert_eq!(system.windows(layout, WindowQuery::Visible), [w(1), w(2), w(3)]);
    }

    #[test]
    fn a_new_window_nests_beside_the_selection_in_a_crowded_split_but_not_in_a_stack() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3), w(4)] {
            system.add_window_after_selection(layout, window);
        }
        let mut stacked = system.clone();

        // A split already holding four panes: the new window nests with the selection.
        system.add_window_after_selection(layout, w(5));
        let parent = |window| parent_of(&system, layout, window);
        assert_eq!(parent(w(4)), parent(w(5)));
        assert_ne!(parent(w(5)), Some(system.tiles.root(layout)));

        // A stack of four tabs: the new window is a fifth tab.
        let _ = stacked.edit(
            layout,
            LayoutEdit::Group(GroupEdit::Stack(StackDefaultOrientation::Same)),
        );
        stacked.focus(layout, Focus::Select(w(4)));
        stacked.add_window_after_selection(layout, w(5));
        assert_eq!(stacked.windows(layout, WindowQuery::Stack(w(5))), [
            w(1),
            w(2),
            w(3),
            w(4),
            w(5)
        ]);
    }

    #[test]
    fn toggling_orientation_rotates_a_split_but_leaves_a_stack_alone() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2)] {
            system.add_window_after_selection(layout, window);
        }
        let root = system.tiles.root(layout);

        assert!(system.edit(layout, LayoutEdit::ToggleOrientation).changed());
        assert_eq!(system.tiles.layout(root), LayoutKind::Vertical);

        let _ = system.edit(
            layout,
            LayoutEdit::Group(GroupEdit::Stack(StackDefaultOrientation::Same)),
        );
        let stacked = system.tiles.layout(root);
        assert!(stacked.is_stacked());
        assert!(!system.edit(layout, LayoutEdit::ToggleOrientation).changed());
        assert_eq!(system.tiles.layout(root), stacked);
    }

    #[test]
    fn a_stacks_members_are_its_own_windows_not_those_nested_in_a_tab() {
        // Window 3 sits in a split that is itself a tab of the stack.
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3)] {
            system.add_window_after_selection(layout, window);
        }
        system.focus(layout, Focus::Select(w(2)));
        system.edit(layout, LayoutEdit::Group(GroupEdit::Join(Direction::Right)));
        system.focus(layout, Focus::Select(w(1)));
        let _ = system.edit(
            layout,
            LayoutEdit::Group(GroupEdit::Stack(StackDefaultOrientation::Same)),
        );

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(1))), [w(1)]);
        assert_eq!(system.windows(layout, WindowQuery::Stack(w(3))), []);
    }

    #[test]
    fn dropping_onto_a_stacked_window_targets_its_stack_and_onto_a_pane_targets_the_pane() {
        use crate::layout_engine::WindowDropAction::{Insert, Stack};

        // Windows 1 and 2 are a stack; windows 3 and 4 are panes of the root split beside it.
        let build = || stacked_pair(Orientation::Horizontal, &[w(1), w(2), w(3), w(4)]);
        let drop = |system: &mut TraditionalLayoutSystem, layout, source, target, action| {
            let edit = LayoutEdit::Drop { source, target, action };
            assert!(system.edit(layout, edit).changed());
        };

        // Stacking onto a tab adds a tab right after it.
        let (mut system, layout) = build();
        drop(&mut system, layout, w(4), w(1), Stack);
        assert_eq!(system.windows(layout, WindowQuery::Stack(w(1))), [
            w(1),
            w(4),
            w(2)
        ]);

        // Stacking onto a pane of a horizontal split makes a new horizontal stack of the two.
        let (mut system, layout) = build();
        drop(&mut system, layout, w(4), w(3), Stack);
        assert_eq!(system.windows(layout, WindowQuery::Stack(w(3))), [w(3), w(4)]);
        let new_stack = parent_of(&system, layout, w(3)).unwrap();
        assert_eq!(system.tiles.layout(new_stack), LayoutKind::HorizontalStack);

        // Inserting beside a tab, along the split's axis, inserts beside its whole stack.
        let (mut system, layout) = build();
        drop(&mut system, layout, w(4), w(2), Insert(Direction::Left));
        let root = Some(system.tiles.root(layout));
        assert_eq!(parent_of(&system, layout, w(4)), root);
        assert_eq!(system.windows(layout, WindowQuery::Stack(w(1))), [w(1), w(2)]);
        let stack = parent_of(&system, layout, w(1)).unwrap();
        let window_4 = system.tiles.window_node(layout, w(4)).unwrap();
        assert_eq!(window_4.next_sibling(system.tiles.map()), Some(stack));
    }
}
