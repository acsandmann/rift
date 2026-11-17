use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use slotmap::Key;

use super::{EditResult, Focus, FocusResult, GroupEdit, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::WindowInsertionPoint;
use crate::layout_engine::systems::constraints::{
    AxisConstraints, ConstraintCache, constrain_frame, container_axis_constraints,
    solve_axis_lengths, traditional_leaf_axis_constraints,
};
use crate::layout_engine::systems::{
    LayoutSystem, WindowLayoutConstraints, reconcile_app_membership,
};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation, ResizeOrientation};
use crate::model::selection::*;
use crate::model::tree::{self, NodeId, NodeMap, Tree};
use crate::sys::geometry::Round;

fn perpendicular(orientation: Orientation) -> Orientation {
    match orientation {
        Orientation::Horizontal => Orientation::Vertical,
        Orientation::Vertical => Orientation::Horizontal,
    }
}

fn group_orientation(
    base: Orientation,
    configured: crate::common::config::StackDefaultOrientation,
) -> Orientation {
    use crate::common::config::StackDefaultOrientation::*;
    match configured {
        Same => base,
        Perpendicular => perpendicular(base),
        Horizontal => Orientation::Horizontal,
        Vertical => Orientation::Vertical,
    }
}

/// The forest owns roots. The saved tuple retains compatibility with named root guards.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(
    try_from = "(Option<NodeId>, String)",
    into = "(Option<NodeId>, &'static str)"
)]
pub(crate) struct LayoutRoot(pub(super) NodeId);

impl TryFrom<(Option<NodeId>, String)> for LayoutRoot {
    type Error = &'static str;

    fn try_from((root, _): (Option<NodeId>, String)) -> Result<Self, Self::Error> {
        root.map(Self).ok_or("saved layout has no root")
    }
}

impl From<LayoutRoot> for (Option<NodeId>, &'static str) {
    fn from(root: LayoutRoot) -> Self { (Some(root.0), "layout_root") }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TraditionalLayoutSystem {
    #[serde(skip)]
    inactive: HashSet<WindowId>,
    pub(crate) tree: Tree<Components>,
    pub(crate) layout_roots: slotmap::SlotMap<LayoutId, LayoutRoot>,
    #[serde(skip, default)]
    window_insertion_point: WindowInsertionPoint,
    #[serde(skip, default)]
    equalize_nodes: bool,
}

impl Default for TraditionalLayoutSystem {
    fn default() -> Self {
        Self {
            inactive: Default::default(),
            tree: Tree::with_observer(Components::default()),
            layout_roots: Default::default(),
            window_insertion_point: WindowInsertionPoint::default(),
            equalize_nodes: true,
        }
    }
}

impl TraditionalLayoutSystem {
    pub fn new(window_insertion_point: WindowInsertionPoint, equalize_nodes: bool) -> Self {
        Self {
            inactive: Default::default(),
            tree: Tree::with_observer(Components::default()),
            layout_roots: Default::default(),
            window_insertion_point,
            equalize_nodes,
        }
    }

    pub fn set_window_insertion_point(&mut self, value: WindowInsertionPoint) {
        self.window_insertion_point = value;
    }

    pub fn set_equalize_nodes(&mut self, value: bool) { self.equalize_nodes = value; }

    fn participating_nodes(&self, root: NodeId) -> HashSet<NodeId> {
        let mut active = HashSet::default();
        if self.inactive.is_empty() {
            return active;
        }
        for node in root.traverse_postorder(self.map()) {
            if self.window_at(node).is_some_and(|w| !self.inactive.contains(&w))
                || node.children(self.map()).any(|child| active.contains(&child))
            {
                active.insert(node);
            }
        }
        active
    }

    fn find_best_focus_target(&self, node: NodeId) -> Option<(NodeId, WindowId)> {
        self.tree
            .data
            .focus_target(self.map(), node, &|window| self.window_participates(window))
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

        self.find_or_create_common_parent_internal(node1, node2)
    }

    pub(crate) fn root(&self, layout: LayoutId) -> NodeId { self.layout_roots[layout].0 }

    pub(crate) fn selection(&self, layout: LayoutId) -> NodeId {
        let selection = self.tree.data.selection.current_selection(self.root(layout));
        if !self.inactive.is_empty() && self.find_best_focus_target(selection).is_none() {
            return self
                .find_best_focus_target(self.root(layout))
                .map_or(self.root(layout), |(node, _)| node);
        }
        selection
    }

    pub(crate) fn map(&self) -> &NodeMap { &self.tree.map }

    pub(crate) fn local_selection(&self, node: NodeId) -> Option<NodeId> {
        self.tree
            .data
            .selection
            .local_selection(self.map(), node)
            .or_else(|| self.tree.data.selection.last_selection(node))
    }

    pub(crate) fn layout(&self, node: NodeId) -> LayoutKind { self.tree.data.layout.kind(node) }

    pub(crate) fn layouts_for_window(&self, wid: WindowId) -> Vec<LayoutId> {
        self.tree.data.window.layouts_for(wid)
    }

    pub(crate) fn window_insertion_point(&self) -> WindowInsertionPoint {
        self.window_insertion_point
    }

    /// Indexed membership access for policies sharing this tree representation.
    pub(crate) fn window_node(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.tree.data.window.node_for(layout, wid)
    }

    pub(crate) fn windows_with_nodes(
        &self,
        layout: LayoutId,
    ) -> impl Iterator<Item = (NodeId, WindowId)> + '_ {
        self.root(layout)
            .traverse_preorder(self.map())
            .filter_map(|node| self.window_at(node).map(|window| (node, window)))
    }

    pub(super) fn windows_from(
        &self,
        region: NodeId,
        start: Option<NodeId>,
        forward: bool,
    ) -> impl Iterator<Item = WindowId> + '_ {
        let map = self.map();
        std::iter::successors(start, move |&node| {
            if forward {
                node.next_in_subtree(region, map)
            } else {
                node.prev_in_subtree(region, map)
            }
        })
        .filter_map(|node| self.window_at(node))
    }

    pub(crate) fn contains_any_window(&self, wid: WindowId) -> bool {
        self.tree.data.window.window_nodes.contains_key(&wid)
    }

    pub(crate) fn window_is_visible(&self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(node) = self.window_node(layout, wid) else {
            return false;
        };
        node.ancestors_with_parent(self.map()).all(|(child, parent)| {
            parent.is_none_or(|parent| {
                !self.layout(parent).is_stacked()
                    || self.tree.data.selection.local_selection(self.map(), parent) == Some(child)
            })
        })
    }

    pub(super) fn reconcile_windows(
        &mut self,
        layout: LayoutId,
        pid: pid_t,
        desired: Vec<WindowId>,
    ) -> Vec<WindowId> {
        let delta = reconcile_app_membership(
            pid,
            self.windows(layout, WindowQuery::App(pid)),
            desired,
            |window| self.window_participates(window),
        );
        for window in delta.removals {
            if let Some(node) = self.window_node(layout, window)
                && !self.tree.data.layout.info[node].is_fullscreen
            {
                node.detach(&mut self.tree).remove();
            }
        }
        delta.additions
    }

    pub(crate) fn fullscreen_frame(
        &self,
        node: NodeId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> Option<CGRect> {
        node.ancestors(self.map()).find_map(|node| {
            let info = &self.tree.data.layout.info[node];
            if info.is_fullscreen {
                Some(screen)
            } else if info.is_fullscreen_within_gaps {
                Some(compute_tiling_area(screen, gaps))
            } else {
                None
            }
        })
    }

    pub(crate) fn set_layout(&mut self, node: NodeId, kind: LayoutKind) {
        self.tree.data.layout.set_kind(node, kind);
    }

    /// Apply a source/target drop without using the current selection as an input.
    pub(crate) fn apply_explicit_window_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
    ) -> bool {
        if source == target {
            return false;
        }
        if action == crate::layout_engine::WindowDropAction::Swap {
            return self.swap_windows(layout, source, target);
        }
        let Some(source_node) = self.window_node(layout, source) else {
            return false;
        };
        let Some(target_node) = self.window_node(layout, target) else {
            return false;
        };

        if action == crate::layout_engine::WindowDropAction::Stack {
            if let Some(parent) = target_node.parent(self.map())
                && self.layout(parent).is_group()
            {
                source_node.detach(&mut self.tree).insert_after(target_node);
                self.select(source_node);
                return true;
            }
            let parent_kind = target_node
                .parent(self.map())
                .map(|parent| self.layout(parent))
                .unwrap_or(LayoutKind::Horizontal);
            let stack_kind = match parent_kind.orientation() {
                Orientation::Horizontal => LayoutKind::HorizontalStack,
                Orientation::Vertical => LayoutKind::VerticalStack,
            };
            let container = self.tree.mk_node().insert_before(target_node);
            self.tree.data.layout.assume_size_of(container, target_node, &self.tree.map);
            target_node.detach(&mut self.tree).push_back(container);
            source_node.detach(&mut self.tree).push_back(container);
            self.set_layout(container, stack_kind);
            self.select(source_node);
            return true;
        }

        let crate::layout_engine::WindowDropAction::Insert(direction) = action else {
            unreachable!()
        };
        let target_anchor = target_node
            .parent(self.map())
            .filter(|parent| self.layout(*parent).is_group())
            .unwrap_or(target_node);
        if target_anchor == source_node
            || target_anchor.ancestors(self.map()).any(|node| node == source_node)
        {
            return false;
        }
        let before = matches!(direction, Direction::Left | Direction::Up);
        if let Some(parent) = target_anchor.parent(self.map())
            && !self.layout(parent).is_group()
            && self.layout(parent).orientation() == direction.orientation()
        {
            if source_node.parent(self.map()) == Some(parent) {
                source_node.reorder(target_anchor, before, &mut self.tree.map);
            } else {
                let source = source_node.detach(&mut self.tree);
                if before {
                    source.insert_before(target_anchor);
                } else {
                    source.insert_after(target_anchor);
                }
            }
        } else {
            let kind = match direction.orientation() {
                Orientation::Horizontal => LayoutKind::Horizontal,
                Orientation::Vertical => LayoutKind::Vertical,
            };
            let container = if target_anchor.parent(self.map()).is_none() {
                // A root stack needs a new layout root, rather than a sibling.
                self.nest_in_container_internal(layout, target_anchor, kind)
            } else {
                let container = self.tree.mk_node().insert_before(target_anchor);
                self.tree.data.layout.assume_size_of(container, target_anchor, &self.tree.map);
                self.set_layout(container, kind);
                container
            };
            // Attach the stack first: extracting the source can collapse a
            // two-window stack and invalidate target_anchor after reattachment.
            target_anchor.detach(&mut self.tree).push_back(container);
            if before {
                source_node.detach(&mut self.tree).insert_before(target_anchor);
            } else {
                source_node.detach(&mut self.tree).push_back(container);
            }
        }
        self.select(source_node);
        true
    }

    pub(super) fn project(
        &self,
        node: NodeId,
        regions: Option<(NodeId, NodeId)>,
        screen: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let map = &self.tree.map;
        let window = &self.tree.data.window;
        let mut prepared = ConstraintCache::default();
        for child in node.traverse_postorder(map) {
            if window.at(child).is_some_and(|w| !self.inactive.contains(&w))
                || child.children(map).any(|c| prepared.contains_key(&c))
            {
                let axes = [false, true].map(|horizontal| {
                    self.tree.data.layout.node_axis_constraints(
                        &prepared,
                        map,
                        window,
                        child,
                        constraints,
                        stack_offset,
                        stack_line,
                        gaps,
                        horizontal,
                    )
                });
                prepared.insert(child, axes);
            }
        }
        // An empty secondary region gives the primary region the whole screen slot.
        let node = regions
            .filter(|(_, secondary)| !prepared.contains_key(secondary))
            .map_or(node, |(primary, _)| primary);
        let mut sizes = Vec::new();
        Projection {
            components: &self.tree.data,
            prepared: &prepared,
            map,
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
        }
        .apply(node, compute_tiling_area(screen, gaps), &mut sizes);
        sizes
    }

    fn find_natural_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let sibling = self.find_direct_sibling_target(from, direction);
        if let Some(parent) = from.parent(self.map()) {
            let kind = self.layout(parent);
            if !kind.is_group()
                && kind.orientation() == direction.orientation()
                && (sibling.is_none() || parent.children(self.map()).count() == 2)
                && let Some(neighbor) = self.find_direct_sibling_target(parent, direction)
            {
                return Some(neighbor);
            }
        }
        sibling
            .or_else(|| {
                self.traverse_internal(from, direction).map(|target| {
                    if self.window_at(target).is_some() {
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
        let first = container.first_child(self.map())?;
        if self.layout(container).orientation() != direction.orientation() {
            return self.local_selection(container).or(Some(first));
        }
        match direction {
            Direction::Left | Direction::Up => Some(first),
            Direction::Right | Direction::Down => container.last_child(self.map()),
        }
    }

    fn find_hierarchical_join_target(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        for ancestor in from.ancestors(self.map()).skip(1) {
            if let Some(target) = self.find_direct_sibling_target(ancestor, direction) {
                return self.find_best_container_child_for_joining(target, direction.opposite());
            }
        }
        None
    }

    fn perform_natural_join(&mut self, selection: NodeId, target: NodeId, direction: Direction) {
        let selection_parent = selection.parent(self.map());
        let target_parent = target.parent(self.map());

        let selection_stack_parent =
            selection_parent.filter(|&parent| self.layout(parent).is_stacked());
        let target_stack_parent = target_parent.filter(|&parent| self.layout(parent).is_stacked());

        let backwards = matches!(direction, Direction::Left | Direction::Up);
        let join = match (selection_stack_parent, target_stack_parent) {
            (Some(stack), None) => Some((stack, target, backwards)),
            (None, Some(stack)) => Some((stack, selection, !backwards)),
            _ => None,
        };
        if let Some((stack, moving, prepend)) = join {
            let first = stack.first_child(self.map()).filter(|_| prepend);
            let detached = moving.detach(&mut self.tree);
            if let Some(first) = first {
                detached.insert_before(first);
            } else {
                detached.push_back(stack);
            }
            self.select(stack);
            return;
        }

        let kind = LayoutKind::from(direction.orientation());
        if let (Some(sp), Some(tp)) = (selection_parent, target_parent) {
            if sp == tp && self.layout(sp).is_stacked() {
                self.set_layout(sp, kind);
                self.select(sp);
                return;
            }
            if sp != tp && self.are_containers_mergeable(sp, tp, direction) {
                let parent = self.find_or_create_smart_common_parent(sp, tp, direction);
                self.set_layout(parent, kind);
                self.select(parent);
                return;
            }
        }
        let parent = self.find_or_create_smart_common_parent(selection, target, direction);
        if !self.layout(parent).is_stacked() {
            self.set_layout(parent, kind);
        }
        self.select(parent);
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
}

impl LayoutSystem for TraditionalLayoutSystem {
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.tree
            .data
            .window
            .window_nodes
            .range(WindowId::new(pid, 1)..=WindowId::new(pid, u32::MAX))
            .any(|(_, nodes)| nodes.0.iter().any(|info| info.layout == layout))
    }

    fn window_participates(&self, window: WindowId) -> bool { !self.inactive.contains(&window) }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        super::update_participation(&mut self.inactive, window, participates)
    }

    fn create_layout(&mut self) -> LayoutId {
        let root = self.tree.mk_node().into_id();
        self.layout_roots.insert(LayoutRoot(root))
    }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.layout_roots.contains_key(layout) }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let source_root = self.layout_roots[layout].0;
        let cloned_root = source_root.deep_copy(&mut self.tree).into_id();
        let dest_layout = self.layout_roots.insert(LayoutRoot(cloned_root));
        for (src, dest) in std::iter::zip(
            source_root.traverse_preorder(&self.tree.map),
            cloned_root.traverse_preorder(&self.tree.map),
        ) {
            let data = &mut self.tree.data;
            data.selection.copy_branch(&self.tree.map, src, dest);
            data.layout.info.insert(dest, data.layout.info[src]);
            if let Some(wid) = data.window.at(src) {
                data.window.set_window(dest_layout, dest, wid);
            }
        }
        dest_layout
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        self.layout_roots.remove(layout).unwrap().0.remove_root(&mut self.tree)
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        self.project(
            self.root(layout),
            None,
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
        )
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        let selection = self.tree.data.selection.current_selection(self.root(layout));
        if let Some(window) = self.window_at(selection).filter(|w| !self.inactive.contains(w)) {
            return Some(window);
        }
        self.find_best_focus_target(selection)
            .or_else(|| self.find_best_focus_target(self.root(layout)))
            .map(|(_, window)| window)
    }

    fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        let selection = self.selection(layout);
        self.window_node(layout, window)
            .is_some_and(|node| node.ancestors(self.map()).any(|ancestor| ancestor == selection))
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.window_in_direction_from(self.root(layout), direction)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.tree.data.window.node_for(layout, wid).is_some()
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        let root = self.root(layout);
        let active = self.participating_nodes(root);
        root.traverse_preorder(&self.tree.map).any(|node| {
            (self.inactive.is_empty() || active.contains(&node))
                && self.tree.data.layout.is_effectively_fullscreen(node)
        })
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.stacked_selection_container(layout).is_some()
    }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        match query {
            WindowQuery::All => self.windows_with_nodes(layout).map(|(_, window)| window).collect(),
            WindowQuery::Visible => {
                let root = self.root(layout);
                self.visible_windows_under_internal(root)
            }
            WindowQuery::Selection => {
                let selection = self.selection(layout);
                self.visible_windows_under_internal(selection)
            }
            WindowQuery::Stack(window) => self.stack_members(layout, window),
            WindowQuery::App(pid) => self
                .windows_with_nodes(layout)
                .filter_map(|(_, window)| (window.pid == pid).then_some(window))
                .collect(),
        }
    }

    fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        match focus {
            Focus::Select(window) => FocusResult::selection({
                let wid = window;

                if self.inactive.contains(&wid) {
                    return FocusResult::selection(false);
                }
                if let Some(node) = self.tree.data.window.node_for(layout, wid) {
                    self.select(node);
                    true
                } else {
                    false
                }
            }),
            Focus::Ascend => FocusResult::selection({
                if !self.inactive.is_empty()
                    && self.find_best_focus_target(self.root(layout)).is_none()
                {
                    return FocusResult::selection(false);
                }
                if let Some(parent) = self.selection(layout).parent(self.map()) {
                    self.select(parent);
                    return FocusResult::selection(true);
                }
                false
            }),
            Focus::Descend => FocusResult::selection(self.descend_selection(layout)),
            Focus::Direction(direction) => {
                FocusResult::movement(self.move_focus(layout, direction))
            }
        }
    }

    fn update(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Add(layout, window, Some(direction)) => {
                let wid = window;

                let mut root = self.root(layout);
                if root.children(self.map()).next().is_some()
                    && (self.layout(root).is_stacked()
                        || self.layout(root).orientation() != direction.orientation())
                {
                    root = self.nest_in_container_internal(
                        layout,
                        root,
                        LayoutKind::from(direction.orientation()),
                    );
                }
                self.tree.data.layout.set_kind(root, LayoutKind::from(direction.orientation()));
                let node = self.add_window_under(layout, root, wid);
                if matches!(direction, Direction::Right | Direction::Down) {
                    if let Some(first) = root.first_child(self.map()).filter(|&first| first != node)
                    {
                        node.detach(&mut self.tree).insert_before(first);
                    }
                }
                self.select(node);
            }
            WindowChange::Add(layout, window, None) => {
                self.add_window_after_selection(layout, window)
            }
            WindowChange::Remove(window) | WindowChange::RemoveAndRebalance(window) => {
                let rebalance = matches!(change, WindowChange::RemoveAndRebalance(_));
                self.inactive.remove(&window);
                for node in self.tree.data.window.take_nodes_for(window) {
                    let parent = node.parent(self.map());
                    node.detach(&mut self.tree).remove();
                    if rebalance
                        && let Some(parent) = parent.filter(|&node| self.map().contains(node))
                    {
                        self.rebalance_node(parent);
                    }
                }
            }
            WindowChange::Replace(from, to) => {
                super::replace_participation(&mut self.inactive, from, to);
                self.tree.data.window.replace_window(from, to);
            }
            WindowChange::CloseApp(pid) => {
                self.inactive.retain(|w| w.pid != pid);
                for node in self.tree.data.window.take_nodes_for_app(pid) {
                    node.detach(&mut self.tree).remove();
                }
            }
            WindowChange::Reconcile(layout, pid, desired) => {
                for wid in self.reconcile_windows(layout, pid, desired) {
                    self.add_window_after_selection(layout, wid);
                }
            }
            WindowChange::Resize {
                layout,
                window,
                old,
                new,
                screen,
                gaps,
            } => {
                let wid = window;
                let old_frame = old;
                let new_frame = new;

                if !self.window_participates(wid) {
                    return;
                }
                if let Some(node) = self.tree.data.window.node_for(layout, wid) {
                    let info = &mut self.tree.data.layout.info[node];
                    if !super::track_fullscreen_resize(
                        &mut info.is_fullscreen,
                        &mut info.is_fullscreen_within_gaps,
                        old_frame,
                        new_frame,
                        screen,
                        gaps,
                    ) {
                        self.set_frame_from_resize(node, old_frame, new_frame, screen);
                    }
                }
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
            LayoutEdit::Transfer(to) => {
                let from_layout = layout;
                let to_layout = to;

                let from_sel = self.selection(from_layout);
                let to_sel = self.selection(to_layout);

                let map = &self.tree.map;
                let Some(old_parent) = from_sel.parent(map) else {
                    return EditResult::Unchanged;
                };
                let is_selection =
                    self.tree.data.selection.local_selection(map, old_parent) == Some(from_sel);
                if to_sel.parent(self.map()).is_none() {
                    from_sel.detach(&mut self.tree).push_back(to_sel);
                } else {
                    from_sel.detach(&mut self.tree).insert_after(to_sel);
                }
                self.tree.data.window.reassign_subtree(&self.tree.map, from_sel, to_layout);
                if is_selection {
                    for node in from_sel.ancestors(&self.tree.map) {
                        if node == old_parent {
                            break;
                        }
                        self.tree.data.selection.select_locally(&self.tree.map, node);
                    }
                };
                EditResult::Changed
            }
            LayoutEdit::Split(kind) => {
                if !self.inactive.is_empty()
                    && self.find_best_focus_target(self.root(layout)).is_none()
                {
                    return EditResult::Unchanged;
                }
                let selection = self.selection(layout);
                self.nest_in_container_internal(layout, selection, kind);
                EditResult::Changed
            }
            LayoutEdit::Fullscreen { within_gaps } => EditResult::Raise({
                if !self.inactive.is_empty()
                    && self.find_best_focus_target(self.root(layout)).is_none()
                {
                    return EditResult::Raise(Vec::new());
                }
                let node = self.selection(layout);
                let info = &mut self.tree.data.layout.info[node];
                if super::toggle_fullscreen_mode(
                    &mut info.is_fullscreen,
                    &mut info.is_fullscreen_within_gaps,
                    within_gaps,
                ) {
                    self.visible_windows_under_internal(node)
                } else {
                    Vec::new()
                }
            }),
            LayoutEdit::Swap(a, b) => EditResult::from(self.swap_windows(layout, a, b)),
            LayoutEdit::ReorderTab { window, target } => {
                EditResult::from(self.move_stack_tab(layout, window, target))
            }
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(self.apply_target_drop(layout, source, target, action))
            }
            LayoutEdit::Group(GroupEdit::Join(direction)) => {
                self.join_selection_with_direction(layout, direction);
                EditResult::Changed
            }
            LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)) => {
                let selection = self.selection(layout);
                let is_joined = selection
                    .parent(self.map())
                    .and_then(|parent| parent.parent(self.map()))
                    .is_some();

                if is_joined {
                    self.unjoin_selection(layout);
                } else {
                    self.join_selection_with_direction(layout, direction);
                };
                EditResult::Changed
            }
            LayoutEdit::Group(GroupEdit::Stack(orientation)) => {
                EditResult::Raise(self.apply_stacking_to_parent_of_selection(layout, orientation))
            }
            LayoutEdit::Group(GroupEdit::Unstack(orientation)) => {
                EditResult::Raise(self.unstack_parent_of_selection(layout, orientation))
            }
            LayoutEdit::Group(GroupEdit::Unjoin) => {
                self.unjoin_selection(layout);
                EditResult::Changed
            }
            LayoutEdit::Resize { amount, orientation } => {
                if !self.inactive.is_empty()
                    && self.find_best_focus_target(self.root(layout)).is_none()
                {
                    return EditResult::Unchanged;
                }
                if amount == 0.0 {
                    return EditResult::Unchanged;
                }
                let selection = self.selection(layout);
                if self.window_at(selection).is_none() {
                    return EditResult::Unchanged;
                }
                let changed = match orientation {
                    ResizeOrientation::Horizontal => [Direction::Right, Direction::Left]
                        .into_iter()
                        .any(|direction| self.resize_internal(selection, amount, direction)),
                    ResizeOrientation::Vertical => [Direction::Down, Direction::Up]
                        .into_iter()
                        .any(|direction| self.resize_internal(selection, amount, direction)),
                    ResizeOrientation::Smart => {
                        let mut node = selection;
                        while let Some(parent) = node.parent(self.map()) {
                            let kind = self.layout(parent);
                            if !kind.is_group() {
                                let directions = match kind.orientation() {
                                    Orientation::Horizontal => [Direction::Right, Direction::Left],
                                    Orientation::Vertical => [Direction::Down, Direction::Up],
                                };
                                if directions
                                    .into_iter()
                                    .any(|direction| self.resize_internal(node, amount, direction))
                                {
                                    return EditResult::Changed;
                                }
                            }
                            node = parent;
                        }
                        false
                    }
                };
                EditResult::from(changed)
            }
            LayoutEdit::Rebalance => {
                let root = self.root(layout);
                self.rebalance_node(root);
                EditResult::Changed
            }
            LayoutEdit::ToggleOrientation => {
                if !self.inactive.is_empty()
                    && self.find_best_focus_target(self.root(layout)).is_none()
                {
                    return EditResult::Unchanged;
                }
                use crate::layout_engine::LayoutKind;

                let map = self.map();
                let selection_node = self.selection(layout);

                let target_node = match selection_node.parent(map) {
                    Some(p) => p,
                    None => self.root(layout),
                };

                let current_kind = self.layout(target_node);

                if current_kind.is_group() {
                    return EditResult::Unchanged;
                }

                let new_kind = match current_kind {
                    LayoutKind::Horizontal => LayoutKind::Vertical,
                    LayoutKind::Vertical => LayoutKind::Horizontal,
                    other => other,
                };

                // Split weights are axis-independent. Rotating a container should preserve the
                // user's ratios; `rebalance` is the explicit operation that resets them.
                self.set_layout(target_node, new_kind);
                EditResult::Changed
            }
        }
    }
}

impl TraditionalLayoutSystem {
    fn stack_members(&self, layout: LayoutId, window: WindowId) -> Vec<WindowId> {
        let Some(parent) =
            self.window_node(layout, window).and_then(|node| node.parent(self.map()))
        else {
            return Vec::new();
        };
        if !self.layout(parent).is_group() {
            return Vec::new();
        }
        parent.children(self.map()).filter_map(|node| self.window_at(node)).collect()
    }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return false;
        }
        if let Some(child) = self.tree.data.selection.last_selection(self.selection(layout)) {
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

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.window_insertion_point == WindowInsertionPoint::EndOfTree {
            let root = self.root(layout);
            let node = self.add_window_under(layout, root, wid);
            self.select(node);
            return;
        }
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

    fn move_selection_with_display_neighbor(
        &mut self,
        layout: LayoutId,
        direction: Direction,
        has_neighbor: bool,
    ) -> bool {
        let selection = self.selection(layout);
        if has_neighbor
            && !selection
                .ancestors(self.map())
                .any(|node| self.move_over(node, direction).is_some())
        {
            return false;
        }
        {
            if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none()
            {
                return false;
            }
            let selection = self.selection(layout);
            self.move_node(layout, selection, direction)
        }
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return;
        }
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

            self.perform_natural_join(selection, target, direction);
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
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return Vec::new();
        }
        let selection = self.selection(layout);
        let container = if self.window_at(selection).is_some() {
            selection.parent(self.map())
        } else {
            Some(selection)
        };
        let Some(container) = container else { return Vec::new() };
        let kind = self.layout(container);
        let orientation = if kind.is_stacked() {
            perpendicular(kind.orientation())
        } else {
            group_orientation(kind.orientation(), default_orientation)
        };
        self.set_layout(container, match orientation {
            Orientation::Horizontal => LayoutKind::HorizontalStack,
            Orientation::Vertical => LayoutKind::VerticalStack,
        });
        if let Some(first) = container.first_child(self.map()) {
            self.select(first);
        }
        self.visible_windows_under_internal(container)
    }

    fn stacked_selection_container(&self, layout: LayoutId) -> Option<NodeId> {
        let selection = self.selection(layout);
        let stacked = |&node: &NodeId| self.layout(node).is_stacked();
        if self.window_at(selection).is_some() {
            selection.ancestors(self.map()).skip(1).find(stacked)
        } else if stacked(&selection) {
            Some(selection)
        } else {
            selection.children(self.map()).find(stacked)
        }
    }

    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return Vec::new();
        }
        let Some(container) = self.stacked_selection_container(layout) else {
            return Vec::new();
        };
        let orientation =
            group_orientation(self.layout(container).orientation(), default_orientation);
        self.set_layout(container, match orientation {
            Orientation::Horizontal => LayoutKind::Horizontal,
            Orientation::Vertical => LayoutKind::Vertical,
        });
        self.visible_windows_under_internal(container)
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return;
        }
        let map = &self.tree.map;
        let Some(parent) = self.selection(layout).parent(map) else {
            return;
        };
        let Some(grandparent) = parent.parent(map) else { return };

        let local_selected_child = self.tree.data.selection.local_selection(map, parent);
        let next_sibling = parent.next_sibling(map);
        let info = &self.tree.data.layout.info;
        let parent_size = info[parent].size.max(0.0);
        // Save weights before detach events normalize the remaining siblings.
        let children: Vec<_> =
            parent.children(map).map(|child| (child, info[child].size.max(0.0))).collect();
        let total: f32 = children.iter().map(|(_, weight)| weight).sum();
        let count = children.len() as f32;
        for (child, weight) in children {
            let promoted_size = if total.is_finite() && total > f32::EPSILON {
                parent_size * weight / total
            } else {
                parent_size / count
            };
            let detached = child.detach(&mut self.tree);
            if let Some(next_sibling) = next_sibling {
                detached.insert_before(next_sibling);
            } else {
                detached.push_back(grandparent);
            }
            self.tree.data.layout.info[child].size = promoted_size;
        }
        parent.detach(&mut self.tree).remove();
        if let Some(selected) = local_selected_child.or_else(|| grandparent.first_child(self.map()))
        {
            self.select(selected);
        }
    }

    fn move_stack_tab(&mut self, layout: LayoutId, window: WindowId, target: WindowId) -> bool {
        if !self.window_participates(window) || !self.window_participates(target) {
            return false;
        }
        let map = &self.tree.map;
        let (Some(source), Some(target)) = (
            self.tree.data.window.node_for(layout, window),
            self.tree.data.window.node_for(layout, target),
        ) else {
            return false;
        };
        // A tab can hold a nested container. Find the two children directly under their stack.
        let Some(stack) = source.common_ancestor(target, map) else {
            return false;
        };
        if !self.layout(stack).is_stacked() {
            return false;
        }
        let tab = |node: NodeId| node.ancestors(map).find(|node| node.parent(map) == Some(stack));
        let (Some(moving), Some(anchor)) = (tab(source), tab(target)) else {
            return false;
        };
        let forward = stack.children(map).position(|c| c == moving)
            < stack.children(map).position(|c| c == anchor);
        let detached = moving.detach(&mut self.tree);
        if forward {
            detached.insert_after(anchor).finish();
        } else {
            detached.insert_before(anchor).finish();
        }
        true
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        if !self.window_participates(a) || !self.window_participates(b) {
            return false;
        }
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

        let selection = self.selection(layout);
        // Exchange the occupants of the slots without restructuring the tree.
        // Reparenting a leaf can collapse a single-child container mid-swap.
        let index = &mut self.tree.data.window;
        index.windows.insert(node_a, b);
        index.windows.insert(node_b, a);
        for (wid, from, to) in [(a, node_a, node_b), (b, node_b, node_a)] {
            let info = index
                .window_nodes
                .get_mut(&wid)
                .unwrap()
                .0
                .iter_mut()
                .find(|info| info.layout == layout && info.node == from)
                .unwrap();
            info.node = to;
        }
        self.select(if selection == node_a {
            node_b
        } else if selection == node_b {
            node_a
        } else {
            selection
        });

        true
    }

    fn apply_target_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
    ) -> bool {
        if !self.window_participates(source) || !self.window_participates(target) {
            return false;
        }
        self.apply_explicit_window_drop(layout, source, target, action)
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

        if self.equalize_nodes {
            // Sway initializes a new node to the average weight of the existing siblings,
            // then normalizes the sibling weights. Rift keeps every sibling set normalized
            // to its child count, so that average is 1.0. Leaving the observer-initialized
            // weight intact gives identical proportions while preserving manual resizes.
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
    }

    pub(crate) fn stack_group_container_info(
        &self,
        node: NodeId,
        kind: crate::layout_engine::LayoutKind,
        rect: CGRect,
        children: &[NodeId],
        selected_index: usize,
    ) -> crate::layout_engine::engine::GroupContainerInfo {
        // One representative per non-empty child, in child order: left to right for a
        // horizontal stack, top to bottom for a vertical one.
        let mut window_ids = Vec::with_capacity(children.len());
        let mut representative_selection = 0;
        for (i, &child) in children.iter().enumerate() {
            if let Some((_, wid)) = self.find_best_focus_target(child) {
                if i == selected_index {
                    representative_selection = window_ids.len();
                }
                window_ids.push(wid);
            }
        }
        let selected_index = representative_selection;

        crate::layout_engine::engine::GroupContainerInfo {
            node_id: node,
            container_kind: kind,
            frame: rect,
            total_count: window_ids.len(),
            selected_index,
            window_ids,
        }
    }

    pub(crate) fn collect_group_containers(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        selection_path_only: bool,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        use crate::layout_engine::LayoutKind::*;

        let mut out = Vec::new();
        let map = &self.tree.map;
        let active = self.participating_nodes(self.root(layout));

        let tiling_area = compute_tiling_area(screen, gaps);

        let mut stack: Vec<(NodeId, CGRect)> = vec![(self.root(layout), tiling_area)];

        while let Some((node, rect)) = stack.pop() {
            if self.tree.data.layout.is_effectively_fullscreen(node) {
                if selection_path_only {
                    out.clear();
                    break;
                }
                continue;
            }

            let kind = self.tree.data.layout.kind(node);
            let children: Vec<_> = node
                .children(map)
                .filter(|c| self.inactive.is_empty() || active.contains(c))
                .collect();

            let selected = self
                .tree
                .data
                .selection
                .local_selection(map, node)
                .filter(|c| self.inactive.is_empty() || active.contains(c))
                .or_else(|| children.first().copied());

            if matches!(kind, HorizontalStack | VerticalStack) {
                if children.is_empty() {
                    continue;
                }

                let local_sel = selected.unwrap();
                if selection_path_only && self.tree.data.layout.is_effectively_fullscreen(local_sel)
                {
                    out.clear();
                    break;
                }
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
                    stack_line,
                );

                for (i, &child) in children.iter().enumerate().rev() {
                    if (selection_path_only && Some(child) != selected)
                        || self.tree.data.layout.is_effectively_fullscreen(child)
                    {
                        continue;
                    }
                    let child_rect = layout_res.get_frame_for_index(i);
                    stack.push((child, child_rect));
                }

                continue;
            }

            // Calculate sibling frames once in forward order, preserving the exact
            // accumulation used by selection-path geometry. Reverse only the work
            // queue so the depth-first traversal still visits children in order.
            let start = stack.len();
            stack.extend(
                children
                    .iter()
                    .copied()
                    .zip(self.child_frames_in_axis(rect, &children, kind == Horizontal, gaps))
                    .filter(|(child, _)| !selection_path_only || Some(*child) == selected),
            );
            stack[start..].reverse();
        }

        out
    }

    fn child_frames_in_axis<'a>(
        &'a self,
        parent_rect: CGRect,
        siblings: &'a [NodeId],
        horizontal: bool,
        gaps: &crate::common::config::GapSettings,
    ) -> impl Iterator<Item = CGRect> + 'a {
        use objc2_core_foundation::{CGPoint, CGSize};

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
        let total_gap = siblings.len().saturating_sub(1) as f64 * inner_gap;
        let usable_axis = if inner_gap == 0.0 {
            axis_len
        } else {
            (axis_len - total_gap).max(0.0)
        };
        let offset = if horizontal {
            parent_rect.origin.x
        } else {
            parent_rect.origin.y
        };

        siblings.iter().scan(offset, move |offset, &child| {
            let ratio = f64::from(self.tree.data.layout.info[child].size) / f64::from(total);
            let seg_len = usable_axis * ratio;
            let frame = if horizontal {
                CGRect::new(
                    CGPoint::new(*offset, parent_rect.origin.y),
                    CGSize::new(seg_len, parent_rect.size.height),
                )
            } else {
                CGRect::new(
                    CGPoint::new(parent_rect.origin.x, *offset),
                    CGSize::new(parent_rect.size.width, seg_len),
                )
            };
            *offset += seg_len;
            *offset += inner_gap;
            Some(frame)
        })
    }
}

impl TraditionalLayoutSystem {
    fn get_ascii_tree_with_labels(
        &self,
        node: NodeId,
        labels: Option<&HashMap<NodeId, &'static str>>,
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
            Some(wid) => format!(
                "{desc} {:?} {}",
                wid,
                self.tree.data.layout.debug(node, self.map(), false)
            ),
            None => format!("{desc} {}", self.tree.data.layout.debug(node, self.map(), true)),
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

    pub(crate) fn container_tree_with_roles(
        &self,
        layout: LayoutId,
        roles: &HashMap<NodeId, &'static str>,
    ) -> rift_protocol::ContainerTreeNode {
        fn snapshot(
            system: &TraditionalLayoutSystem,
            node: NodeId,
            selected: NodeId,
            roles: &HashMap<NodeId, &'static str>,
        ) -> rift_protocol::ContainerTreeNode {
            let window = system.window_at(node);
            let info = system.tree.data.layout.info[node];
            rift_protocol::ContainerTreeNode {
                node_id: node.data().as_ffi(),
                node_type: if window.is_some() {
                    rift_protocol::ContainerNodeType::Window
                } else {
                    rift_protocol::ContainerNodeType::Container
                },
                frame: Default::default(),
                layout_kind: window.is_none().then_some(info.kind),
                weight: node.parent(system.map()).map(|_| f64::from(info.size)),
                window_id: window.map(Into::into),
                is_selected: node == selected,
                is_fullscreen: info.is_fullscreen,
                is_fullscreen_within_gaps: info.is_fullscreen_within_gaps,
                role: roles.get(&node).map(|role| (*role).to_owned()),
                pending_split: None,
                children: node
                    .children(system.map())
                    .map(|child| snapshot(system, child, selected, roles))
                    .collect(),
            }
        }

        snapshot(self, self.root(layout), self.selection(layout), roles)
    }

    pub(crate) fn visible_windows_in_subtree(&self, node: NodeId) -> Vec<WindowId> {
        self.visible_windows_under_internal(node)
    }

    pub(crate) fn draw_tree_with_labels(
        &self,
        layout: LayoutId,
        labels: &HashMap<NodeId, &'static str>,
    ) -> String {
        let tree = self.get_ascii_tree_with_labels(self.root(layout), Some(labels));
        let mut out = String::new();
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
    }

    fn window_in_direction_from(&self, node: NodeId, direction: Direction) -> Option<WindowId> {
        if let Some(window) = self.window_at(node) {
            return self.window_participates(window).then_some(window);
        }
        // Along the movement axis, enter at the near edge. Across a split,
        // reuse its selected branch so a return trip retraces the focus path.
        // Stacks always present their selected member.
        let kind = self.tree.data.layout.kind(node);
        let selected = (kind.is_stacked() || kind.orientation() != direction.orientation())
            .then(|| self.tree.data.selection.local_selection(self.map(), node))
            .flatten();
        if let Some(window) =
            selected.and_then(|child| self.window_in_direction_from(child, direction))
        {
            return Some(window);
        }
        let alternative = |child: &NodeId| Some(*child) != selected;
        match direction {
            Direction::Left | Direction::Up => node
                .children_rev(self.map())
                .filter(alternative)
                .find_map(|child| self.window_in_direction_from(child, direction)),
            Direction::Right | Direction::Down => node
                .children(self.map())
                .filter(alternative)
                .find_map(|child| self.window_in_direction_from(child, direction)),
        }
    }

    fn rebalance_node(&mut self, node: NodeId) {
        let map = &self.tree.map;
        let children: Vec<_> = node.children(map).collect();
        if children.is_empty() {
            return;
        }

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
        let active = self.participating_nodes(target);
        let participates = |child: &NodeId| self.inactive.is_empty() || active.contains(child);
        let mut current = target;
        loop {
            let kind = self.tree.data.layout.kind(current);
            let selected =
                self.tree.data.selection.local_selection(map, current).filter(participates);
            let edge = || match direction {
                Direction::Left | Direction::Up => current.children(map).find(participates),
                Direction::Right | Direction::Down => current.children_rev(map).find(participates),
            };
            let next = if kind.is_stacked() {
                selected.or_else(|| current.children(map).find(participates))
            } else {
                selected.or_else(edge)
            };
            match next {
                Some(child) => current = child,
                None => return Some(current),
            }
        }
    }

    fn visible_windows_under_internal(&self, node: NodeId) -> Vec<WindowId> {
        let mut windows = Vec::new();
        self.visit_visible_windows(node, &mut |_, window| windows.push(window));
        windows
    }

    pub(super) fn visit_visible_windows(
        &self,
        node: NodeId,
        visit: &mut impl FnMut(NodeId, WindowId),
    ) -> bool {
        if let Some(window) = self.window_at(node) {
            if !self.window_participates(window) {
                return false;
            }
            visit(node, window);
            return true;
        }
        let mut children = node.children(self.map());
        if self.layout(node).is_group() {
            let selected = self.tree.data.selection.local_selection(self.map(), node);
            if self.inactive.is_empty() {
                return selected
                    .or_else(|| children.next())
                    .is_some_and(|child| self.visit_visible_windows(child, visit));
            }
            return selected.is_some_and(|child| self.visit_visible_windows(child, visit))
                || children.any(|child| {
                    Some(child) != selected && self.visit_visible_windows(child, visit)
                });
        }
        let mut visited = false;
        for child in children {
            visited |= self.visit_visible_windows(child, visit);
        }
        visited
    }

    fn move_over(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let Some(parent) = from.parent(&self.tree.map) else {
            return None;
        };
        if self.tree.data.layout.kind(parent).orientation() != direction.orientation() {
            return None;
        }
        let sibling = |node: NodeId| match direction {
            Direction::Left | Direction::Up => node.prev_sibling(&self.tree.map),
            Direction::Right | Direction::Down => node.next_sibling(&self.tree.map),
        };
        let mut next = sibling(from);
        while let Some(node) = next {
            if self.inactive.is_empty() || self.find_best_focus_target(node).is_some() {
                return Some(node);
            }
            next = sibling(node);
        }
        None
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
                let root_kind = self.tree.data.layout.kind(old_root);
                // Leaving the edge of a root group splits the window out of it; at the edge
                // of a same-oriented split there is nowhere further to go.
                if root_kind.orientation() == direction.orientation() && !root_kind.is_group() {
                    return false;
                }
                let new_container_kind = LayoutKind::from(direction.orientation());
                self.nest_in_container_internal(layout, old_root, new_container_kind);
                destination = Destination::Ahead(old_root);
            }
        }
        let (target, before) = match (destination, direction) {
            (Destination::Ahead(target), Direction::Left | Direction::Up)
            | (Destination::Behind(target), Direction::Right | Direction::Down) => (target, true),
            (Destination::Ahead(target), Direction::Right | Direction::Down)
            | (Destination::Behind(target), Direction::Left | Direction::Up) => (target, false),
        };
        if moving_node.parent(&self.tree.map) == target.parent(&self.tree.map) {
            moving_node.reorder(target, before, &mut self.tree.map);
        } else {
            let moving = moving_node.detach(&mut self.tree);
            if before {
                moving.insert_before(target);
            } else {
                moving.insert_after(target);
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
                self.tree
                    .data
                    .layout
                    .children_total(&self.tree.map, resizing_node.parent(&self.tree.map).unwrap()),
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
        let choose_edge = |mut edges: [(Direction, f64); 2]| {
            for (_, delta) in &mut edges {
                if delta.abs() < RESIZE_DELTA_EPSILON {
                    *delta = 0.0;
                }
            }
            if !(edges[0].1.abs() >= edges[1].1.abs()) {
                edges.swap(0, 1);
            }
            edges.into_iter().find(|&(direction, delta)| {
                delta != 0.0 && self.can_resize_towards(node, direction)
            })
        };
        let horizontal = choose_edge([
            (Direction::Left, old_frame.min().x - new_frame.min().x),
            (Direction::Right, new_frame.max().x - old_frame.max().x),
        ]);
        let vertical = choose_edge([
            (Direction::Up, old_frame.min().y - new_frame.min().y),
            (Direction::Down, new_frame.max().y - old_frame.max().y),
        ]);
        for (edge, extent) in [
            (horizontal, screen.size.width),
            (vertical, screen.size.height),
        ] {
            if let Some((direction, delta)) = edge {
                self.resize_internal(node, delta / extent, direction);
            }
        }
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
                let new_root = self.tree.mk_node().into_id();
                self.layout_roots[layout] = LayoutRoot(new_root);
                node.attach_root(&mut self.tree, new_root);
                new_root
            };
            self.tree.data.selection.select_locally(&self.tree.map, node);
            new_parent
        };
        self.tree.data.layout.set_kind(parent, kind);
        parent
    }

    fn find_or_create_common_parent_internal(&mut self, node1: NodeId, node2: NodeId) -> NodeId {
        let map = self.map();
        let ancestor =
            node1.common_ancestor(node2, map).expect("nodes must belong to the same tree");
        if ancestor == node1 || ancestor == node2 {
            return ancestor;
        }

        if node1.parent(map) == Some(ancestor) && node2.parent(map) == Some(ancestor) {
            let size1 = self.tree.data.layout.info[node1].size.max(0.0);
            let size2 = self.tree.data.layout.info[node2].size.max(0.0);
            let new_container = self.tree.mk_node().insert_before(node1);
            node1.detach(&mut self.tree).push_back(new_container).with(|child, tree| {
                tree.data.layout.info[child].size = size1;
            });
            node2.detach(&mut self.tree).push_back(new_container).with(|child, tree| {
                tree.data.layout.info[child].size = size2;
            });

            // A collapsed parent already transferred its outer weight.
            if self.tree.map.contains(ancestor) {
                self.tree.data.layout.info[new_container].size = size1 + size2;
            }
            return new_container;
        }
        let container = self.tree.mk_node().push_back(ancestor);
        self.tree.data.layout.set_kind(container, LayoutKind::Horizontal);
        node1.detach(&mut self.tree).push_back(container);
        node2.detach(&mut self.tree).push_back(container);
        container
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub(crate) struct Components {
    pub(super) selection: Selection,
    pub(crate) layout: Layout,
    pub(crate) window: WindowIndex,
}

impl Components {
    pub(super) fn focus_target(
        &self,
        map: &NodeMap,
        node: NodeId,
        participates: &impl Fn(WindowId) -> bool,
    ) -> Option<(NodeId, WindowId)> {
        if let Some(window) = self.window.at(node) {
            return participates(window).then_some((node, window));
        }
        let selected = self.selection.last_selection(node);
        selected
            .and_then(|child| self.focus_target(map, child, participates))
            .or_else(|| {
                node.children(map)
                    .filter(|&child| Some(child) != selected)
                    .find_map(|child| self.focus_target(map, child, participates))
            })
    }
}

impl tree::Observer for Components {
    fn added_to_forest(&mut self, _: &NodeMap, node: NodeId) {
        self.layout.info.insert(node, LayoutInfo::default());
    }

    fn added_to_parent(&mut self, map: &NodeMap, node: NodeId) {
        debug_assert!(node.parent(map).is_none_or(|parent| self.window.at(parent).is_none()));
        self.layout.info[node].size = 1.0;
    }

    fn removing_from_parent(&mut self, map: &NodeMap, node: NodeId) {
        self.selection.detach(map, node);
        if self.window.at(node).is_some() {
            self.layout.normalize_after_removal(map, node);
        }
    }

    fn removed_child(tree: &mut Tree<Self>, parent: NodeId) {
        if parent.parent(&tree.map).is_none() || tree.data.layout.info[parent].fixed_container {
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

    fn removed_from_forest(&mut self, _: &NodeMap, node: NodeId) {
        self.selection.remove(node);
        self.layout.info.remove(node);
        self.window.remove_node(node);
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub(crate) struct WindowIndex {
    windows: slotmap::SecondaryMap<NodeId, WindowId>,
    window_nodes: crate::common::collections::BTreeMap<WindowId, WindowNodeInfoVec>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct WindowNodeInfo {
    layout: LayoutId,
    node: NodeId,
}

#[derive(Clone, Serialize, Deserialize, Default, Debug)]
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

    pub(crate) fn reassign_subtree(&mut self, map: &NodeMap, node: NodeId, layout: LayoutId) {
        for node in node.traverse_preorder(map) {
            if let Some(window) = self.at(node) {
                for info in &mut self.window_nodes.get_mut(&window).unwrap().0 {
                    if info.node == node {
                        info.layout = layout;
                    }
                }
            }
        }
    }

    fn replace_window(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        let nodes = self.window_nodes.remove(&from).unwrap_or_default();
        if nodes.0.is_empty() {
            return;
        }
        for info in &nodes.0 {
            self.windows.insert(info.node, to);
        }
        self.window_nodes.entry(to).or_default().0.extend(nodes.0);
    }

    fn take_nodes_for(&mut self, wid: WindowId) -> impl Iterator<Item = NodeId> + use<> {
        self.window_nodes
            .remove(&wid)
            .unwrap_or_default()
            .0
            .into_iter()
            .map(|info| info.node)
    }

    fn take_nodes_for_app(&mut self, pid: pid_t) -> impl Iterator<Item = NodeId> + use<> {
        use crate::common::collections::BTreeExt;
        self.window_nodes
            .remove_all_for_pid(pid)
            .into_values()
            .flat_map(|infos| infos.0.into_iter().map(|info| info.node))
    }

    fn remove_node(&mut self, node: NodeId) {
        if let Some(wid) = self.windows.remove(node)
            && let Some(nodes) = self.window_nodes.get_mut(&wid)
        {
            nodes.0.retain(|info| info.node != node);
            if nodes.0.is_empty() {
                self.window_nodes.remove(&wid);
            }
        }
    }
}

pub(super) struct StackLayoutResult {
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

    pub(super) fn get_frame_for_index(&self, index: usize) -> CGRect {
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
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub(crate) struct Layout {
    pub(crate) info: slotmap::SecondaryMap<NodeId, LayoutInfo>,
}

#[allow(unused)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LayoutInfo {
    pub(crate) size: f32,
    #[serde(default)]
    pub(crate) fixed_container: bool,
    pub(super) kind: LayoutKind,
    #[serde(default)]
    pub(crate) is_fullscreen: bool,
    #[serde(default)]
    pub(super) is_fullscreen_within_gaps: bool,
}

impl Layout {
    fn children_total(&self, map: &NodeMap, parent: NodeId) -> f32 {
        parent.children(map).map(|child| self.info[child].size.max(0.0)).sum()
    }

    // Leaf removal keeps sibling proportions while restoring their average weight to one.
    fn normalize_after_removal(&mut self, map: &NodeMap, node: NodeId) {
        let parent = node.parent(map).unwrap();
        let siblings = || parent.children(map).filter(|&child| child != node);
        let (count, total) = siblings().fold((0, 0.0_f32), |(count, total), child| {
            (count + 1, total + self.info[child].size.max(0.0))
        });
        for child in siblings() {
            self.info[child].size = if total.is_finite() && total > f32::EPSILON {
                self.info[child].size * (count as f32 / total)
            } else {
                1.0
            };
        }
    }

    fn assume_size_of(&mut self, new: NodeId, old: NodeId, map: &NodeMap) {
        assert_eq!(new.parent(map), old.parent(map));
        self.info[new].size = core::mem::replace(&mut self.info[old].size, 0.0);
    }

    fn set_kind(&mut self, node: NodeId, kind: LayoutKind) { self.info[node].kind = kind; }

    fn kind(&self, node: NodeId) -> LayoutKind { self.info[node].kind }

    fn proportion(&self, map: &NodeMap, node: NodeId) -> Option<f64> {
        let Some(parent) = node.parent(map) else { return None };
        let total = self.children_total(map, parent);
        (total.is_finite() && total > f32::EPSILON)
            .then(|| f64::from(self.info[node].size.max(0.0)) / f64::from(total))
    }

    fn take_share(&mut self, map: &NodeMap, node: NodeId, from: NodeId, share: f32) {
        assert_eq!(node.parent(map), from.parent(map));
        const MIN_NODE_SIZE: f32 = 0.05;
        let share = share.min(self.info[from].size - MIN_NODE_SIZE);
        let share = share.max(MIN_NODE_SIZE - self.info[node].size);
        self.info[from].size -= share;
        self.info[node].size += share;
        let parent = node.parent(map).unwrap();
        let (count, total) = parent.children(map).fold((0, 0.0_f32), |(count, total), child| {
            (count + 1, total + self.info[child].size.max(0.0))
        });
        if total > f32::EPSILON && total.is_finite() {
            let scale = count as f32 / total;
            for child in parent.children(map) {
                self.info[child].size *= scale;
            }
        }
    }

    fn is_effectively_fullscreen(&self, node: NodeId) -> bool {
        let info = &self.info[node];
        info.is_fullscreen || info.is_fullscreen_within_gaps
    }

    fn debug(&self, node: NodeId, map: &NodeMap, is_container: bool) -> String {
        let info = &self.info[node];
        if is_container {
            format!(
                "{:?} [size {} total={}]",
                info.kind,
                info.size,
                self.children_total(map, node)
            )
        } else {
            format!("[size {}]", info.size)
        }
    }

    fn node_axis_constraints(
        &self,
        prepared: &ConstraintCache,
        map: &NodeMap,
        window: &WindowIndex,
        node: NodeId,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        stack_offset: f64,
        stack_line: crate::common::config::StackLineReserve,
        gaps: &crate::common::config::GapSettings,
        horizontal: bool,
    ) -> (f64, Option<f64>, Option<f64>, bool) {
        if let Some(wid) = window.at(node) {
            if let Some(c) = constraints.get(&wid).copied() {
                return traditional_leaf_axis_constraints(c, horizontal);
            }
            return (0.0, None, None, true);
        }

        let kind = self.info[node].kind;
        container_axis_constraints(
            node.children(map)
                .filter_map(|child| prepared.get(&child))
                .map(|axes| axes[horizontal as usize]),
            kind,
            horizontal,
            if horizontal {
                gaps.inner.horizontal
            } else {
                gaps.inner.vertical
            },
            stack_offset,
            stack_line.on_axis(kind, horizontal),
        )
    }
}

struct Projection<'a> {
    components: &'a Components,
    prepared: &'a ConstraintCache,
    map: &'a NodeMap,
    screen: CGRect,
    stack_offset: f64,
    constraints: &'a HashMap<WindowId, WindowLayoutConstraints>,
    gaps: &'a crate::common::config::GapSettings,
    stack_line: crate::common::config::StackLineReserve,
}

impl Projection<'_> {
    fn apply(&self, node: NodeId, rect: CGRect, sizes: &mut Vec<(WindowId, CGRect)>) {
        let Self {
            components,
            prepared,
            map,
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
        } = *self;
        if !prepared.contains_key(&node) {
            return;
        }
        let info = &components.layout.info[node];
        let rect = if info.is_fullscreen {
            screen
        } else if info.is_fullscreen_within_gaps {
            compute_tiling_area(screen, gaps)
        } else {
            rect
        };
        if let Some(wid) = components.window.at(node) {
            debug_assert!(
                node.children(map).next().is_none(),
                "non-leaf node with window id"
            );
            let final_rect = constraints.get(&wid).copied().map_or(rect, |c| {
                constrain_frame(rect, |horizontal| {
                    traditional_leaf_axis_constraints(c, horizontal)
                })
            });
            sizes.push((wid, final_rect));
            return;
        }
        use LayoutKind::*;
        match info.kind {
            HorizontalStack | VerticalStack => {
                let children: Vec<_> =
                    node.children(map).filter(|child| prepared.contains_key(child)).collect();
                if children.is_empty() {
                    return;
                }
                let is_horizontal = matches!(info.kind, HorizontalStack);
                let focused_child = components
                    .selection
                    .local_selection(map, node)
                    .filter(|child| prepared.contains_key(child))
                    .unwrap_or_else(|| children[0]);
                let focused_idx = children.iter().position(|&c| c == focused_child).unwrap_or(0);
                let (min, fixed, _, _) = prepared[&children[focused_idx]][is_horizontal as usize];
                let layout = stack_layout_result(
                    rect,
                    children.len(),
                    stack_offset_with_required_size(
                        rect,
                        is_horizontal,
                        children.len(),
                        stack_offset,
                        fixed.unwrap_or(min),
                    ),
                    is_horizontal,
                    stack_line,
                );
                for (idx, child) in children.into_iter().enumerate() {
                    self.apply(child, layout.get_frame_for_index(idx), sizes);
                }
            }
            Horizontal | Vertical => self.layout_axis(node, rect, sizes, info.kind == Horizontal),
        }
    }

    fn layout_axis(
        &self,
        node: NodeId,
        rect: CGRect,
        sizes: &mut Vec<(WindowId, CGRect)>,
        horizontal: bool,
    ) {
        let Self {
            components,
            prepared,
            map,
            gaps,
            ..
        } = *self;
        let info = &components.layout.info;
        use objc2_core_foundation::{CGPoint, CGSize};
        let children: Vec<_> =
            node.children(map).filter(|child| prepared.contains_key(child)).collect();
        if children.is_empty() {
            return;
        }
        let min_size = 0.05;
        let mut needs_normalization = false;
        let mut actual_total = 0.0;
        for &child in &children {
            let sz = info[child].size;
            actual_total += sz;
            if !sz.is_finite() || sz < min_size - f32::EPSILON {
                needs_normalization = true;
            }
        }
        let normalize_sizes =
            !actual_total.is_finite() || actual_total <= f32::EPSILON || needs_normalization;
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
                let (min, fixed, max, can_grow) = prepared[&child][horizontal as usize];
                AxisConstraints {
                    min,
                    fixed,
                    max,
                    weight: f64::from(if normalize_sizes {
                        1.0
                    } else {
                        info[child].size.max(0.0)
                    }),
                    can_grow,
                }
            })
            .collect();
        let seg_lens = solve_axis_lengths(axis_constraints, usable_axis);
        for (child, seg_len) in children.into_iter().zip(seg_lens) {
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
            self.apply(child, child_rect, sizes);
            offset += seg_len;
            offset += inner_gap;
        }
    }
}

pub(super) fn stack_offset_with_required_size(
    rect: CGRect,
    horizontal: bool,
    count: usize,
    offset: f64,
    required: f64,
) -> f64 {
    if count <= 1 {
        return offset;
    }
    let axis = if horizontal {
        rect.size.width
    } else {
        rect.size.height
    };
    let required = required.clamp(0.0, axis.max(0.0));
    offset.min((axis - required).max(0.0) / (count - 1) as f64)
}

pub(super) fn stack_layout_result(
    rect: CGRect,
    child_count: usize,
    stack_offset: f64,
    is_horizontal: bool,
    stack_line: crate::common::config::StackLineReserve,
) -> StackLayoutResult {
    StackLayoutResult::new(
        if child_count > 1 {
            stack_line.inset(rect, is_horizontal)
        } else {
            rect
        },
        child_count,
        stack_offset,
        is_horizontal,
    )
}

impl TraditionalLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        let tree = self.get_ascii_tree_with_labels(self.root(layout), None);
        let mut out = String::new();
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
    }

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        self.container_tree_with_roles(layout, &HashMap::default())
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        crate::layout_engine::systems::node_slot(
            self.window_node(layout, window)?,
            self.root(layout),
            self.map(),
        )
    }
}
#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::layout_engine::{Direction, LayoutKind};

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn unjoin_preserves_nested_and_outer_resize_ratios() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);

        let w1 = w(163);
        let w2 = w(164);
        let w3 = w(165);
        system.add_window_after_selection(layout, w1);
        system.add_window_after_selection(layout, w2);
        system.add_window_after_selection(layout, w3);
        assert!(system.focus(layout, Focus::Select(w1)).handled);
        system.join_selection_with_direction(layout, Direction::Right);

        let n1 = system.tree.data.window.node_for(layout, w1).unwrap();
        let n2 = system.tree.data.window.node_for(layout, w2).unwrap();
        let n3 = system.tree.data.window.node_for(layout, w3).unwrap();
        let group = n1.parent(system.map()).unwrap();
        system.tree.data.layout.info[group].size = 6.0;
        system.tree.data.layout.info[n3].size = 4.0;

        system.tree.data.layout.info[n1].size = 3.0;
        system.tree.data.layout.info[n2].size = 1.0;

        system.select(n1);
        system.unjoin_selection(layout);

        assert!((system.tree.data.layout.info[n1].size - 4.5).abs() < 0.0001);
        assert!((system.tree.data.layout.info[n2].size - 1.5).abs() < 0.0001);
        assert!((system.tree.data.layout.info[n3].size - 4.0).abs() < 0.0001);
        assert!(
            (system.tree.data.layout.children_total(&system.tree.map, root) - 10.0).abs() < 0.0001
        );
    }

    #[test]
    fn reordering_siblings_preserves_user_resize_ratios() {
        let mut system = TraditionalLayoutSystem::default();
        let layout = system.create_layout();
        let root = system.root(layout);
        system.tree.data.layout.set_kind(root, LayoutKind::Horizontal);
        system.add_window_after_selection(layout, w(178));
        system.add_window_after_selection(layout, w(179));
        system.add_window_after_selection(layout, w(180));

        let n1 = system.tree.data.window.node_for(layout, w(178)).unwrap();
        let n2 = system.tree.data.window.node_for(layout, w(179)).unwrap();
        let n3 = system.tree.data.window.node_for(layout, w(180)).unwrap();
        system.tree.data.layout.info[n1].size = 5.0;
        system.tree.data.layout.info[n2].size = 2.0;
        system.tree.data.layout.info[n3].size = 1.0;

        assert!(system.move_node(layout, n2, Direction::Right));

        assert_eq!(root.children(system.map()).collect::<Vec<_>>(), vec![n1, n3, n2]);
        assert!((system.tree.data.layout.info[n1].size - 5.0).abs() < 0.0001);
        assert!((system.tree.data.layout.info[n2].size - 2.0).abs() < 0.0001);
        assert!((system.tree.data.layout.info[n3].size - 1.0).abs() < 0.0001);
        assert!(
            (system.tree.data.layout.children_total(&system.tree.map, root) - 8.0).abs() < 0.0001
        );
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
            let layout = system.create_layout();
            let root = system.root(layout);
            system.tree.data.layout.set_kind(root, kind);
            let a = w(170);
            let b = w(171);
            system.add_window_under(layout, root, a);
            system.add_window_under(layout, root, b);
            system.focus(layout, Focus::Select(b)).handled;
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
            assert_eq!(system.tree.data.layout.kind(root), kind);
        }
    }
}
