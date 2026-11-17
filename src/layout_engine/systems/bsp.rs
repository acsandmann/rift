use objc2_core_foundation::{CGRect, CGSize};
use serde::{Deserialize, Serialize};
use slotmap::Key;

use super::{EditResult, Focus, FocusResult, GroupEdit, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::WindowInsertionPoint;
use crate::layout_engine::systems::constraints::{
    AxisConstraints, ConstraintCache, constrain_frame, container_axis_constraints,
    solve_axis_lengths,
};
use crate::layout_engine::systems::{
    LayoutSystem, WindowLayoutConstraints, reconcile_app_membership,
};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation, ResizeOrientation};
use crate::model::selection::*;
use crate::model::tree::{NodeId, NodeMap, Tree};

fn bsp_leaf_constraints(
    c: WindowLayoutConstraints,
    horizontal: bool,
) -> (f64, Option<f64>, Option<f64>, bool) {
    let max = c.max_for_axis(horizontal);
    (
        c.min_for_axis(horizontal),
        c.fixed_for_axis(horizontal),
        (max > 0.0).then_some(max),
        c.resizable_for_axis(horizontal),
    )
}

#[derive(Serialize, Deserialize, Clone, Debug)]
enum NodeKind {
    Split {
        orientation: Orientation,
        ratio: f32,
    },
    Leaf {
        window: Option<WindowId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stack: Option<Vec<WindowId>>,
        fullscreen: bool,
        fullscreen_within_gaps: bool,
        preselected: Option<Direction>,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct LayoutState {
    root: NodeId,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(from = "StoredBspSystem")]
pub struct BspLayoutSystem {
    #[serde(skip)]
    inactive: HashSet<WindowId>,
    layouts: slotmap::SlotMap<crate::layout_engine::LayoutId, LayoutState>,
    tree: Tree<Components>,
    #[serde(skip, default)]
    window_insertion_point: WindowInsertionPoint,
    #[serde(skip, default)]
    single_window_aspect_ratio: Option<f64>,
}

/// Historical stack maps are folded into leaf state once, while loading.
#[derive(Deserialize)]
struct StoredBspSystem {
    layouts: slotmap::SlotMap<LayoutId, LayoutState>,
    tree: Tree<Components>,
    #[serde(default)]
    kind: slotmap::SecondaryMap<NodeId, NodeKind>,
    #[serde(default)]
    stacks: HashMap<NodeId, Vec<WindowId>>,
}

impl From<StoredBspSystem> for BspLayoutSystem {
    fn from(stored: StoredBspSystem) -> Self {
        let mut system = Self {
            layouts: stored.layouts,
            tree: stored.tree,
            inactive: Default::default(),
            window_insertion_point: Default::default(),
            single_window_aspect_ratio: None,
        };
        if system.tree.data.kind.is_empty() {
            system.tree.data.kind = stored.kind;
        }
        for (node, members) in stored.stacks {
            system.set_stack(node, members);
        }
        system
    }
}

impl BspLayoutSystem {
    fn stack(&self, node: NodeId) -> Option<&Vec<WindowId>> {
        match self.tree.data.kind.get(node)? {
            NodeKind::Leaf { stack, .. } => stack.as_ref(),
            _ => None,
        }
    }

    fn stack_mut(&mut self, node: NodeId) -> Option<&mut Vec<WindowId>> {
        match self.tree.data.kind.get_mut(node)? {
            NodeKind::Leaf { stack, .. } => stack.as_mut(),
            _ => None,
        }
    }

    fn take_stack(&mut self, node: NodeId) -> Option<Vec<WindowId>> {
        match self.tree.data.kind.get_mut(node)? {
            NodeKind::Leaf { stack, .. } => stack.take(),
            _ => None,
        }
    }

    fn set_stack(&mut self, node: NodeId, members: Vec<WindowId>) {
        if let Some(NodeKind::Leaf { stack, .. }) = self.tree.data.kind.get_mut(node) {
            *stack = Some(members);
        }
    }

    fn participating_window(&self, node: NodeId) -> Option<WindowId> {
        let Some(NodeKind::Leaf { window, .. }) = self.tree.data.kind.get(node) else {
            return None;
        };
        window
            .filter(|w| !self.inactive.contains(w))
            .or_else(|| self.stack(node)?.iter().copied().find(|w| !self.inactive.contains(w)))
    }

    fn find_neighbor_leaf(&self, from_leaf: NodeId, direction: Direction) -> Option<NodeId> {
        let mut current = from_leaf;
        while let Some(next) = self.find_structural_neighbor_leaf(current, direction) {
            if self.inactive.is_empty() || self.participating_window(next).is_some() {
                return Some(next);
            }
            current = next;
        }
        None
    }

    fn find_structural_neighbor_leaf(
        &self,
        from_leaf: NodeId,
        direction: Direction,
    ) -> Option<NodeId> {
        // Track the source center across the movement axis, relative to each ancestor.
        let mut anchor = 0.5;
        let mut current = from_leaf;

        while let Some(parent) = current.parent(&self.tree.map) {
            if let Some(NodeKind::Split { orientation, ratio }) = self.tree.data.kind.get(parent) {
                if *orientation == direction.orientation() {
                    let mut children = parent.children(&self.tree.map);
                    if let (Some(first), Some(second), None) =
                        (children.next(), children.next(), children.next())
                    {
                        let target = match direction {
                            Direction::Left | Direction::Up if current != first => Some(first),
                            Direction::Right | Direction::Down if current == first => Some(second),
                            _ => None,
                        };
                        if let Some(target) = target {
                            return Some(
                                self.find_closest_leaf_in_direction(target, direction, anchor),
                            );
                        }
                    }
                } else {
                    let ratio = f64::from(*ratio);
                    anchor = if parent.first_child(&self.tree.map) == Some(current) {
                        anchor * ratio
                    } else {
                        ratio + anchor * (1.0 - ratio)
                    };
                }
            }
            current = parent;
        }

        None
    }

    fn find_closest_leaf_in_direction(
        &self,
        root: NodeId,
        direction: Direction,
        mut anchor: f64,
    ) -> NodeId {
        match self.tree.data.kind.get(root) {
            Some(NodeKind::Leaf { .. }) => root,
            Some(NodeKind::Split { orientation, ratio }) => {
                let target_child = if *orientation == direction.orientation() {
                    match direction {
                        Direction::Left | Direction::Up => root.last_child(&self.tree.map),
                        Direction::Right | Direction::Down => root.first_child(&self.tree.map),
                    }
                } else {
                    let ratio = f64::from(*ratio);
                    // Choose the adjacent side and express the anchor within that child.
                    if anchor <= ratio {
                        anchor /= ratio;
                        root.first_child(&self.tree.map)
                    } else {
                        anchor = (anchor - ratio) / (1.0 - ratio);
                        root.last_child(&self.tree.map)
                    }
                };

                if let Some(child) = target_child {
                    self.find_closest_leaf_in_direction(child, direction, anchor)
                } else {
                    root
                }
            }
            None => root,
        }
    }

    fn window_in_direction_from(&self, node: NodeId, direction: Direction) -> Option<WindowId> {
        match self.tree.data.kind.get(node) {
            Some(NodeKind::Leaf { window: Some(_), .. }) => self.participating_window(node),
            Some(NodeKind::Leaf { .. }) => None,
            Some(NodeKind::Split { .. }) => match direction {
                Direction::Left | Direction::Up => node
                    .children_rev(&self.tree.map)
                    .find_map(|child| self.window_in_direction_from(child, direction)),
                Direction::Right | Direction::Down => node
                    .children(&self.tree.map)
                    .find_map(|child| self.window_in_direction_from(child, direction)),
            },
            None => None,
        }
    }

    fn smart_insert_window(&mut self, layout: LayoutId, window: WindowId) -> bool {
        let Some(selection) = self.selection_of_layout(layout) else {
            return false;
        };
        let leaf = self.descend_to_leaf(selection);
        let Some(&NodeKind::Leaf {
            preselected: Some(direction), ..
        }) = self.tree.data.kind.get(leaf)
        else {
            return false;
        };
        self.split_leaf(
            leaf,
            direction.orientation(),
            Some(window),
            matches!(direction, Direction::Left | Direction::Up),
        )
    }

    fn split_leaf(
        &mut self,
        leaf: NodeId,
        orientation: Orientation,
        new_window: Option<WindowId>,
        new_first: bool,
    ) -> bool {
        let Some(&NodeKind::Leaf { window, .. }) = self.tree.data.kind.get(leaf) else {
            return false;
        };
        let stack = self.take_stack(leaf);
        let existing = self.make_leaf(window);
        if let Some(stack) = stack {
            self.set_stack(existing, stack);
        }
        let new = self.make_leaf(new_window);
        self.tree.data.kind.insert(leaf, NodeKind::Split { orientation, ratio: 0.5 });
        let children = if new_first {
            [new, existing]
        } else {
            [existing, new]
        };
        for child in children {
            child.detach(&mut self.tree).push_back(leaf);
        }
        self.tree.data.selection.select(&self.tree.map, new);
        true
    }
}

impl Default for BspLayoutSystem {
    fn default() -> Self {
        Self {
            inactive: Default::default(),
            layouts: Default::default(),
            tree: Tree::with_observer(Components::default()),
            window_insertion_point: WindowInsertionPoint::default(),
            single_window_aspect_ratio: None,
        }
    }
}

impl BspLayoutSystem {
    pub fn new(window_insertion_point: WindowInsertionPoint) -> Self {
        Self {
            window_insertion_point,
            ..Self::default()
        }
    }

    pub fn set_window_insertion_point(&mut self, value: WindowInsertionPoint) {
        self.window_insertion_point = value;
    }

    pub fn set_single_window_aspect_ratio(&mut self, ratio: Option<f64>) {
        self.single_window_aspect_ratio = ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    }

    fn node_for_window_in_layout(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        let root = self.layouts.get(layout)?.root;
        root.traverse_preorder(&self.tree.map)
            .find(|&node| match self.tree.data.kind.get(node) {
                Some(NodeKind::Leaf { window, .. }) => {
                    self.stack(node).map_or(*window == Some(wid), |stack| stack.contains(&wid))
                }
                _ => false,
            })
    }

    fn make_leaf(&mut self, window: Option<WindowId>) -> NodeId {
        let id = self.tree.mk_node().into_id();
        self.tree.data.kind.insert(id, NodeKind::Leaf {
            window,
            stack: None,
            fullscreen: false,
            fullscreen_within_gaps: false,
            preselected: None,
        });
        id
    }

    fn descend_to_leaf(&self, mut node: NodeId) -> NodeId {
        if !self.inactive.is_empty()
            && let Some(active) = node
                .traverse_preorder(&self.tree.map)
                .find(|&candidate| self.participating_window(candidate).is_some())
        {
            return active;
        }
        loop {
            match self.tree.data.kind.get(node) {
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

    /// Calculates the depth of a node in the BSP tree by counting parent levels.
    /// The root node has depth 0, its direct children have depth 1, and so on.
    fn node_depth(&self, node: NodeId) -> usize {
        let mut depth = 0;
        let mut current = node;
        while let Some(parent) = current.parent(&self.tree.map) {
            depth += 1;
            current = parent;
        }
        depth
    }

    /// Returns the orientation for a new split based on the node's depth.
    /// Creates a fibonacci spiral pattern by alternating orientations:
    /// - Even depth (0, 2, 4, ...) → Horizontal split
    /// - Odd depth (1, 3, 5, ...) → Vertical split
    fn orientation_for_depth(&self, depth: usize) -> Orientation {
        if depth % 2 == 0 {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }
    }

    fn windows_under(&self, root: NodeId) -> impl Iterator<Item = WindowId> + '_ {
        root.traverse_preorder(&self.tree.map).flat_map(|node| {
            match self.tree.data.kind.get(node) {
                Some(NodeKind::Leaf { window, stack, .. }) => {
                    stack.as_deref().unwrap_or_else(|| window.as_slice())
                }
                _ => &[],
            }
            .iter()
            .copied()
        })
    }

    fn has_fullscreen_in_subtree(&self, node: NodeId) -> bool {
        match self.tree.data.kind.get(node) {
            Some(NodeKind::Leaf {
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) => {
                self.participating_window(node).is_some()
                    && (*fullscreen || *fullscreen_within_gaps)
            }
            Some(NodeKind::Split { .. }) => {
                node.children(&self.tree.map).any(|child| self.has_fullscreen_in_subtree(child))
            }
            None => false,
        }
    }

    // Return the number of tiles sharing the parent's axis; perpendicular splits count as one.
    fn rebalance_node(&mut self, node: NodeId, parent_axis: Orientation) -> usize {
        let Some(&NodeKind::Split { orientation, .. }) = self.tree.data.kind.get(node) else {
            return 1;
        };
        let (Some(first), Some(second)) =
            (node.first_child(&self.tree.map), node.last_child(&self.tree.map))
        else {
            return 1;
        };
        let first_span = self.rebalance_node(first, orientation);
        let second_span = self.rebalance_node(second, orientation);
        let span = first_span + second_span;
        if let Some(NodeKind::Split { ratio, .. }) = self.tree.data.kind.get_mut(node) {
            *ratio = (first_span as f32 / span as f32).clamp(0.05, 0.95);
        }
        if orientation == parent_axis { span } else { 1 }
    }

    fn cleanup_after_removal(&mut self, node: NodeId) -> NodeId {
        let Some(parent_id) = node.parent(&self.tree.map) else {
            return node;
        };

        if !matches!(self.tree.data.kind.get(parent_id), Some(NodeKind::Split { .. })) {
            return parent_id;
        }

        let has_two_children = {
            let mut children = parent_id.children(&self.tree.map);
            matches!(
                (children.next(), children.next(), children.next()),
                (Some(_), Some(_), None)
            )
        };
        if !has_two_children {
            return parent_id;
        }
        node.detach(&mut self.tree).remove();
        self.collapse_split(parent_id);
        parent_id
    }

    /// Promote the sole remaining child while keeping the layout's root stable.
    fn collapse_split(&mut self, parent_id: NodeId) {
        if !matches!(self.tree.data.kind.get(parent_id), Some(NodeKind::Split { .. })) {
            return;
        }
        let map = &self.tree.map;
        let Some(sibling) = parent_id
            .first_child(map)
            .filter(|child| Some(*child) == parent_id.last_child(map))
        else {
            return;
        };
        let Some(kind) = self.tree.data.kind.remove(sibling) else {
            return;
        };
        if matches!(kind, NodeKind::Split { .. }) {
            while let Some(child) = sibling.first_child(&self.tree.map) {
                child.detach(&mut self.tree).push_back(parent_id);
            }
        }
        self.tree.data.kind.insert(parent_id, kind);
        sibling.detach(&mut self.tree).remove();
    }

    fn selection_of_layout(&self, layout: LayoutId) -> Option<NodeId> {
        let root = self.layouts.get(layout)?.root;
        let selected = self.tree.data.selection.current_selection(root);
        if self.inactive.is_empty() || self.participating_window(selected).is_some() {
            return Some(selected);
        }
        let first_active = root
            .traverse_preorder(&self.tree.map)
            .find(|&node| self.participating_window(node).is_some())?;
        if matches!(
            self.tree.data.kind.get(selected),
            Some(NodeKind::Leaf { window: Some(_), .. })
        ) {
            Some(first_active)
        } else {
            Some(selected)
        }
    }

    fn insert_window_at_selection(
        &mut self,
        layout: crate::layout_engine::LayoutId,
        wid: WindowId,
    ) {
        let Some(state) = self.layouts.get(layout).copied() else {
            return;
        };
        let sel = self
            .selection_of_layout(layout)
            .unwrap_or_else(|| self.tree.data.selection.current_selection(state.root));
        let leaf = self.descend_to_leaf(sel);
        if leaf != sel {
            self.tree.data.selection.select(&self.tree.map, leaf);
        }
        match self.tree.data.kind.get_mut(leaf) {
            Some(NodeKind::Leaf {
                window,
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) if window.is_none() => {
                (*window, *fullscreen, *fullscreen_within_gaps) = (Some(wid), false, false);
            }
            Some(NodeKind::Leaf { .. }) => {
                let orientation = self.orientation_for_depth(self.node_depth(leaf));
                self.split_leaf(leaf, orientation, Some(wid), false);
            }
            _ => {}
        }
    }

    /// Remove an occurrence and report progress so callers can clear legacy duplicates.
    fn remove_window_internal(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(node) = self.node_for_window_in_layout(layout, wid) else {
            return false;
        };
        if let Some(stack) = self.stack_mut(node) {
            stack.retain(|window| *window != wid);
            if let Some(&visible) = stack.last() {
                if let Some(NodeKind::Leaf { window, .. }) = self.tree.data.kind.get_mut(node) {
                    *window = Some(visible);
                }
                self.tree.data.selection.select(&self.tree.map, node);
                return true;
            }
            self.take_stack(node);
        }
        if let Some(NodeKind::Leaf { window, .. }) = self.tree.data.kind.get_mut(node) {
            *window = None;
        }
        let fallback = self.cleanup_after_removal(node);
        let selection = self.tree.data.selection.current_selection(self.layouts[layout].root);
        let new_selection = if self.tree.data.kind.contains_key(selection) {
            self.descend_to_leaf(selection)
        } else {
            self.descend_to_leaf(fallback)
        };
        self.tree.data.selection.select(&self.tree.map, new_selection);
        true
    }

    fn calculate_layout_recursive(
        &self,
        prepared: &ConstraintCache,
        node: NodeId,
        rect: CGRect,
        screen: CGRect,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        out: &mut Vec<(WindowId, CGRect)>,
    ) {
        if !prepared.contains_key(&node) {
            return;
        }
        match self.tree.data.kind.get(node) {
            Some(NodeKind::Leaf {
                window: _,
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) => {
                if let Some(w) = self.participating_window(node).as_ref() {
                    let mut target = if *fullscreen {
                        screen
                    } else if *fullscreen_within_gaps {
                        compute_tiling_area(screen, gaps)
                    } else {
                        rect
                    };
                    if !*fullscreen
                        && !*fullscreen_within_gaps
                        && let Some(c) = constraints.get(w).copied()
                    {
                        let c = c.normalized();
                        target = constrain_frame(target, |horizontal| {
                            bsp_leaf_constraints(c, horizontal)
                        });
                    }
                    out.push((*w, target));
                }
            }
            Some(NodeKind::Split { orientation, ratio }) => {
                let mut children =
                    node.children(&self.tree.map).filter(|c| prepared.contains_key(c));
                let Some(first) = children.next() else { return };
                let Some(second) = children.next() else {
                    self.calculate_layout_recursive(
                        prepared,
                        first,
                        rect,
                        screen,
                        constraints,
                        gaps,
                        out,
                    );
                    return;
                };
                let horizontal = *orientation == Orientation::Horizontal;
                let (total, gap) = if horizontal {
                    (rect.size.width, gaps.inner.horizontal)
                } else {
                    (rect.size.height, gaps.inner.vertical)
                };
                let ratio = f64::from(*ratio);
                let items = [(first, ratio), (second, 1.0 - ratio)].map(|(child, weight)| {
                    let (min, fixed, max, can_grow) = prepared[&child][horizontal as usize];
                    AxisConstraints {
                        min,
                        fixed,
                        max,
                        weight: weight.max(0.0),
                        can_grow,
                    }
                });
                let lengths = solve_axis_lengths(items, (total - gap).max(0.0));
                let (first_len, second_len) = (lengths[0], lengths[1]);
                let mut r1 = rect;
                let mut r2 = rect;
                if horizontal {
                    r1.size.width = first_len;
                    r2.origin.x += first_len + gap;
                    r2.size.width = second_len;
                } else {
                    r1.size.height = first_len;
                    r2.origin.y += first_len + gap;
                    r2.size.height = second_len;
                }
                for (child, frame) in [(first, r1), (second, r2)] {
                    self.calculate_layout_recursive(
                        prepared,
                        child,
                        frame,
                        screen,
                        constraints,
                        gaps,
                        out,
                    );
                }
            }
            None => {}
        }
    }

    fn prepare_geometry(
        &self,
        root: NodeId,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
    ) -> ConstraintCache {
        let map = &self.tree.map;
        let mut prepared = ConstraintCache::default();
        for node in root.traverse_postorder(map) {
            let axes = match self.tree.data.kind.get(node) {
                Some(NodeKind::Leaf { window, .. }) => {
                    let active = self.participating_window(node);
                    if active.is_none() && !(self.inactive.is_empty() && window.is_none()) {
                        continue;
                    }
                    let limits = active
                        .and_then(|window| constraints.get(&window).copied())
                        .map(WindowLayoutConstraints::normalized);
                    [false, true].map(|horizontal| {
                        limits.map_or((0.0, None, None, true), |limits| {
                            bsp_leaf_constraints(limits, horizontal)
                        })
                    })
                }
                Some(NodeKind::Split { orientation, .. }) => {
                    let children = || node.children(map).filter_map(|child| prepared.get(&child));
                    if children().next().is_none() {
                        continue;
                    }
                    let kind = LayoutKind::from(*orientation);
                    [false, true].map(|horizontal| {
                        let mut summary = container_axis_constraints(
                            children().map(|axes| axes[horizontal as usize]),
                            kind,
                            horizontal,
                            if horizontal {
                                gaps.inner.horizontal
                            } else {
                                gaps.inner.vertical
                            },
                            0.0,
                            0.0,
                        );
                        // BSP leaves the perpendicular maximum unconstrained.
                        if (*orientation == Orientation::Horizontal) != horizontal {
                            summary.2 = None;
                        }
                        summary
                    })
                }
                None => continue,
            };
            prepared.insert(node, axes);
        }
        prepared
    }

    fn selection_window(&self, state: &LayoutState) -> Option<WindowId> {
        let sel = self.tree.data.selection.current_selection(state.root);
        self.participating_window(sel)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Components {
    selection: Selection,
    #[serde(default, skip_serializing_if = "slotmap::SecondaryMap::is_empty")]
    kind: slotmap::SecondaryMap<NodeId, NodeKind>,
}

impl crate::model::tree::Observer for Components {
    fn removing_from_parent(&mut self, map: &NodeMap, node: NodeId) {
        self.selection.detach(map, node)
    }

    fn removed_from_forest(&mut self, _: &NodeMap, node: NodeId) {
        self.selection.remove(node);
        self.kind.remove(node);
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CGPoint;

    use super::*;

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    fn tiled(windows: &[u32]) -> (BspLayoutSystem, LayoutId) {
        let mut system = BspLayoutSystem::default();
        let layout = system.create_layout();
        for &window in windows {
            system.add_window_after_selection(layout, w(window));
        }
        (system, layout)
    }

    fn calculate_frames(
        system: &BspLayoutSystem,
        layout: LayoutId,
        size: CGSize,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
    ) -> HashMap<WindowId, CGRect> {
        system
            .calculate_layout(
                layout,
                CGRect::new(CGPoint::new(0.0, 0.0), size),
                0.0,
                constraints,
                &Default::default(),
                Default::default(),
            )
            .into_iter()
            .collect()
    }

    fn assert_frame(
        frames: &HashMap<WindowId, CGRect>,
        window: WindowId,
        rect: (f64, f64, f64, f64),
    ) {
        let (x, y, width, height) = rect;
        assert_eq!(
            frames[&window],
            CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
        );
    }

    #[test]
    fn app_exit_clears_saved_display_layouts_and_reclaims_frames() {
        use crate::layout_engine::workspaces::WorkspaceLayoutState;

        let mut system = BspLayoutSystem::default();
        let mut workspace = WorkspaceLayoutState::default();
        let sizes = [CGSize::new(1000.0, 800.0), CGSize::new(1600.0, 900.0)];
        workspace.ensure_active(sizes[0], &mut system);
        let original = workspace.active().unwrap();
        system.add_window_after_selection(original, w(1));
        let survivor = WindowId::new(2, 1);
        system.add_window_after_selection(original, survivor);
        workspace.ensure_active(sizes[1], &mut system);
        let cloned = workspace.active().unwrap();
        system.update(WindowChange::CloseApp(1));
        for (layout, size) in [(cloned, sizes[1]), (original, sizes[0])] {
            workspace.ensure_active(size, &mut system);
            assert_eq!(workspace.active(), Some(layout));
            assert_eq!(system.all_windows_in_layout(layout), vec![survivor]);
            assert_eq!(system.selected_window(layout), Some(survivor));
            let frames = calculate_frames(&system, layout, size, &Default::default());
            assert_frame(&frames, survivor, (0.0, 0.0, size.width, size.height));
        }
    }

    #[test]
    fn removal_clears_legacy_duplicates_and_empty_roots() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        // Simulate duplicate membership from an older saved tree.
        let duplicate = system.node_for_window_in_layout(layout, w(2)).unwrap();
        if let Some(NodeKind::Leaf { window, .. }) = system.tree.data.kind.get_mut(duplicate) {
            *window = Some(w(1));
        }
        system.remove_window(w(1));
        assert_eq!(system.all_windows_in_layout(layout), vec![w(3)]);
        let frames =
            calculate_frames(&system, layout, CGSize::new(1000.0, 800.0), &Default::default());
        assert_frame(&frames, w(3), (0.0, 0.0, 1000.0, 800.0));
        let stack_layout = system.clone_layout(layout);
        let node = system.node_for_window_in_layout(stack_layout, w(3)).unwrap();
        system.set_stack(node, vec![w(3), w(3)]);
        system.remove_window(w(3));
        system.remove_window(w(3));
        assert!(system.all_windows_in_layout(layout).is_empty());
        assert_eq!(system.selected_window(layout), None);
        assert!(system.all_windows_in_layout(stack_layout).is_empty());
        assert!(system.visible_windows_in_layout(stack_layout).is_empty());
    }

    #[test]
    fn clone_preserves_arrangement_and_selection() {
        let (mut system, original) = tiled(&[1, 2, 3, 4]);
        system.focus(original, Focus::Select(w(1))).handled;
        system
            .edit(original, LayoutEdit::Resize {
                amount: 0.2,
                orientation: ResizeOrientation::Horizontal,
            })
            .discard();
        system
            .edit(original, LayoutEdit::Resize {
                amount: -0.1,
                orientation: ResizeOrientation::Vertical,
            })
            .discard();
        let size = CGSize::new(1400.0, 900.0);
        let frames = calculate_frames(&system, original, size, &Default::default());
        let cloned = system.clone_layout(original);
        assert_eq!(system.selected_window(cloned), Some(w(1)));
        assert_eq!(
            calculate_frames(&system, cloned, size, &Default::default()),
            frames
        );
        system.toggle_fullscreen(original, true);
        let fullscreen_clone = system.clone_layout(original);
        assert_eq!(
            calculate_frames(&system, fullscreen_clone, size, &Default::default()),
            calculate_frames(&system, original, size, &Default::default())
        );
        assert_eq!(
            calculate_frames(&system, cloned, size, &Default::default()),
            frames
        );
    }

    #[test]
    fn cloned_stacks_replace_and_remove_all_members() {
        use crate::layout_engine::WindowDropAction::Stack;

        let (mut system, original) = tiled(&[1, 2, 3]);
        assert!(
            system
                .edit(original, LayoutEdit::Drop {
                    source: w(2),
                    target: w(1),
                    action: Stack
                })
                .changed()
        );
        // Recreate the historical separate-membership-map encoding, then run
        // the same clone/replacement/removal contract against its migrated state.
        #[derive(Serialize)]
        struct Legacy<'a> {
            layouts: &'a slotmap::SlotMap<LayoutId, LayoutState>,
            tree: &'a Tree<Components>,
            kind: slotmap::SecondaryMap<NodeId, NodeKind>,
            stacks: HashMap<NodeId, Vec<WindowId>>,
        }
        let mut kind = system.tree.data.kind.clone();
        let mut stacks = HashMap::default();
        for (node, state) in kind.iter_mut() {
            if let NodeKind::Leaf { stack, .. } = state
                && let Some(members) = stack.take()
            {
                stacks.insert(node, members);
            }
        }
        let expected = system.container_tree(original);
        let mut legacy_tree = system.tree.clone();
        legacy_tree.data.kind.clear();
        let saved = ron::to_string(&Legacy {
            layouts: &system.layouts,
            tree: &legacy_tree,
            kind,
            stacks,
        })
        .unwrap();
        system = ron::from_str(&saved).unwrap();
        assert_eq!(system.container_tree(original), expected);
        let cloned = system.clone_layout(original);
        system.update(WindowChange::Replace(w(1), w(4)));
        system.update(WindowChange::Replace(w(2), w(5)));
        for layout in [original, cloned] {
            assert_eq!(system.windows(layout, WindowQuery::Stack(w(4))), vec![
                w(4),
                w(5)
            ]);
            assert_eq!(system.visible_windows_in_layout(layout), vec![w(5), w(3)]);
        }
        system.remove_window(w(5));
        for layout in [original, cloned] {
            assert_eq!(system.windows(layout, WindowQuery::Stack(w(4))), vec![w(4)]);
            assert_eq!(system.visible_windows_in_layout(layout), vec![w(4), w(3)]);
        }
        system.update(WindowChange::Replace(w(4), w(3)));
        for layout in [original, cloned] {
            assert_eq!(system.all_windows_in_layout(layout), vec![w(3)]);
        }
        system.update(WindowChange::CloseApp(1));
        for layout in [original, cloned] {
            assert!(system.all_windows_in_layout(layout).is_empty());
            assert!(system.visible_windows_in_layout(layout).is_empty());
            assert_eq!(system.selected_window(layout), None);
        }
    }

    #[test]
    fn interactions_are_scoped_and_destroying_a_clone_preserves_original() {
        use crate::layout_engine::WindowDropAction::{Insert, Stack};

        let (mut system, original) = tiled(&[1, 2]);
        let cloned = system.clone_layout(original);
        assert!(system.swap_windows(original, w(1), w(2)));
        assert_eq!(system.all_windows_in_layout(original), vec![w(2), w(1)]);
        assert_eq!(system.all_windows_in_layout(cloned), vec![w(1), w(2)]);
        assert!(
            system
                .edit(original, LayoutEdit::Drop {
                    source: w(1),
                    target: w(2),
                    action: Stack
                })
                .changed()
        );
        assert!(system.focus(original, Focus::Select(w(2))).handled);
        assert_eq!(system.selected_window(original), Some(w(2)));
        assert_eq!(system.selected_window(cloned), Some(w(2)));
        assert!(system.focus(cloned, Focus::Select(w(1))).handled);
        assert_eq!(system.selected_window(original), Some(w(2)));
        assert!(
            system
                .edit(original, LayoutEdit::Drop {
                    source: w(2),
                    target: w(1),
                    action: Insert(Direction::Left)
                })
                .changed()
        );
        assert_eq!(system.all_windows_in_layout(original), vec![w(2), w(1)]);
        system.remove_layout(cloned);
        assert!(system.focus(original, Focus::Select(w(1))).handled);
        system.remove_window(w(2));
        assert_eq!(system.all_windows_in_layout(original), vec![w(1)]);
    }

    #[test]
    fn insertion_preserves_stacks_and_rejects_existing_members() {
        use crate::layout_engine::WindowDropAction::Stack;

        for (insertion, explicit_split) in [
            (WindowInsertionPoint::default(), false),
            (WindowInsertionPoint::default(), true),
            (WindowInsertionPoint::EndOfTree, false),
        ] {
            let (mut system, layout) = tiled(&[1, 2]);
            system.set_window_insertion_point(insertion);
            assert!(
                system
                    .edit(layout, LayoutEdit::Drop {
                        source: w(2),
                        target: w(1),
                        action: Stack
                    })
                    .changed()
            );
            system.add_window_after_selection(layout, w(1));
            system.add_window_after_selection(layout, w(2));
            assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2)]);
            if explicit_split {
                system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
            }
            system.add_window_after_selection(layout, w(3));
            assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2), w(3)]);
            assert_eq!(system.windows(layout, WindowQuery::Stack(w(1))), vec![
                w(1),
                w(2)
            ]);
            system.update(WindowChange::CloseApp(1));
            assert!(system.all_windows_in_layout(layout).is_empty());
        }
    }

    #[test]
    fn joining_preserves_membership_for_cloning_and_app_exit() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        system.join_selection_with_direction(layout, Direction::Up);
        assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2), w(3)]);
        assert_eq!(system.windows(layout, WindowQuery::Stack(w(2))), vec![
            w(2),
            w(3)
        ]);
        let cloned = system.clone_layout(layout);
        system.update(WindowChange::CloseApp(1));
        for layout in [layout, cloned] {
            assert!(system.all_windows_in_layout(layout).is_empty());
        }
    }

    #[test]
    fn unjoining_retiles_the_stack_in_place() {
        for (windows, direction, joined) in [
            // Stack directly under the root: [1 | stack(2, 3)].
            (&[1, 2, 3][..], Direction::Up, [2, 3]),
            // Deeper stack in the spiral: [1 | [2 / stack(3, 4)]].
            (&[1, 2, 3, 4][..], Direction::Left, [3, 4]),
        ] {
            let (mut system, layout) = tiled(windows);
            let tiled_tree = system.draw_tree(layout);
            let focused = w(joined[1]);

            system.join_selection_with_direction(layout, direction);

            assert_eq!(
                system.windows(layout, WindowQuery::Stack(focused)),
                joined.map(w)
            );

            system.unjoin_selection(layout);

            assert_eq!(system.draw_tree(layout), tiled_tree);
            assert!(system.windows(layout, WindowQuery::Stack(focused)).is_empty());
            assert_eq!(system.selected_window(layout), Some(focused));
        }

        // Members keep their stack order and the active member keeps the selection.
        let (mut system, layout) = tiled(&[1, 2, 3]);
        system.join_selection_with_direction(layout, Direction::Up);
        assert!(system.focus(layout, Focus::Select(w(2))).handled);

        system.unjoin_selection(layout);

        assert_eq!(system.visible_windows_in_layout(layout), vec![w(1), w(3), w(2)]);
        assert_eq!(system.selected_window(layout), Some(w(2)));
    }

    #[test]
    fn unjoining_a_window_that_is_not_stacked_is_a_noop() {
        for windows in [&[1][..], &[1, 2], &[1, 2, 3], &[1, 2, 3, 4]] {
            let (mut system, layout) = tiled(windows);
            let tree = system.draw_tree(layout);
            let selected = system.selected_window(layout);

            system.unjoin_selection(layout);

            assert_eq!(system.draw_tree(layout), tree);
            assert_eq!(system.selected_window(layout), selected);
        }
    }

    #[test]
    fn unjoining_keeps_fullscreen_on_the_visible_member() {
        let toggles: [fn(&mut BspLayoutSystem, LayoutId) -> Vec<WindowId>; 2] = [
            |system: &mut BspLayoutSystem, layout| system.toggle_fullscreen(layout, false),
            |system: &mut BspLayoutSystem, layout| system.toggle_fullscreen(layout, true),
        ];

        for toggle in toggles {
            let (mut system, layout) = tiled(&[1, 2, 3]);
            system.join_selection_with_direction(layout, Direction::Up);
            toggle(&mut system, layout);

            system.unjoin_selection(layout);

            assert_eq!(system.selected_window(layout), Some(w(3)));
            assert!(system.has_any_fullscreen_node(layout));

            let frames =
                calculate_frames(&system, layout, CGSize::new(1000.0, 800.0), &Default::default());

            assert_frame(&frames, w(2), (500.0, 0.0, 500.0, 400.0));
            assert_frame(&frames, w(3), (0.0, 0.0, 1000.0, 800.0));

            toggle(&mut system, layout);

            assert!(!system.has_any_fullscreen_node(layout));
        }
    }

    #[test]
    fn consume_or_expel_toggles_between_joined_and_unjoined() {
        for (windows, direction, joined) in [
            (&[1, 2, 3][..], Direction::Up, [2, 3]),
            (&[1, 2, 3, 4][..], Direction::Left, [3, 4]),
        ] {
            let (mut system, layout) = tiled(windows);
            let tiled_tree = system.draw_tree(layout);

            system
                .edit(layout, LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)))
                .discard();

            assert_eq!(
                system.windows(layout, WindowQuery::Stack(w(joined[0]))),
                joined.map(w)
            );

            system
                .edit(layout, LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)))
                .discard();

            assert_eq!(system.draw_tree(layout), tiled_tree);
        }

        // A joined first child keeps its side: [[2 / 5] / [3 | 4]].
        let (mut system, layout) = tiled(&[1, 2, 3, 4]);
        system.remove_window(w(1));
        assert!(system.focus(layout, Focus::Select(w(2))).handled);
        system.add_window_after_selection(layout, w(5));
        assert!(system.focus(layout, Focus::Select(w(5))).handled);
        let tiled_tree = system.draw_tree(layout);

        system
            .edit(
                layout,
                LayoutEdit::Group(GroupEdit::ConsumeOrExpel(Direction::Up)),
            )
            .discard();

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(5))), vec![
            w(2),
            w(5)
        ]);
        assert_eq!(system.visible_windows_in_layout(layout), [5, 3, 4].map(w));

        system
            .edit(
                layout,
                LayoutEdit::Group(GroupEdit::ConsumeOrExpel(Direction::Up)),
            )
            .discard();

        assert_eq!(system.draw_tree(layout), tiled_tree);
        assert_eq!(system.visible_windows_in_layout(layout), [2, 5, 3, 4].map(w));
    }

    #[test]
    fn consume_or_expel_joins_a_window_left_alone_in_its_stack() {
        // [1 | [2 / stack(3)]] once the other stacked window has closed.
        let (mut system, layout) = tiled(&[1, 2, 3, 4]);
        system.join_selection_with_direction(layout, Direction::Left);
        system.remove_window(w(4));

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(3))), vec![w(3)]);

        system
            .edit(
                layout,
                LayoutEdit::Group(GroupEdit::ConsumeOrExpel(Direction::Up)),
            )
            .discard();

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(3))), vec![
            w(2),
            w(3)
        ]);
    }

    #[test]
    fn unjoining_a_window_left_alone_in_its_stack_clears_the_stack() {
        // [1 | [2 / stack(3)]] once the other stacked window has closed.
        let (mut system, layout) = tiled(&[1, 2, 3, 4]);
        system.join_selection_with_direction(layout, Direction::Left);
        system.remove_window(w(4));
        let tree = system.draw_tree(layout);

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(3))), vec![w(3)]);

        system.unjoin_selection(layout);

        assert_eq!(system.draw_tree(layout), tree);
        assert!(system.windows(layout, WindowQuery::Stack(w(3))).is_empty());
        assert_eq!(system.selected_window(layout), Some(w(3)));
    }

    #[test]
    fn joining_keeps_the_selected_window_visible() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.focus(layout, Focus::Select(w(2))).handled);

        system.join_selection_with_direction(layout, Direction::Down);

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(2))), vec![
            w(3),
            w(2)
        ]);
        assert_eq!(system.visible_windows_in_layout(layout), vec![w(1), w(2)]);
        assert_eq!(system.selected_window(layout), Some(w(2)));

        system.unjoin_selection(layout);

        let (reference, reference_layout) = tiled(&[1, 3, 2]);

        assert_eq!(system.draw_tree(layout), reference.draw_tree(reference_layout));
        assert_eq!(system.selected_window(layout), Some(w(2)));
    }

    #[test]
    fn legacy_ron_index_is_ignored_when_loading_saved_layouts() {
        let (system, original) = tiled(&[1, 2]);
        let node = system.node_for_window_in_layout(original, w(1)).unwrap();
        let obsolete_index: HashMap<_, _> = [(w(1), node)].into_iter().collect();
        let saved = ron::to_string(&system).unwrap();
        let legacy = saved.replacen(
            "tree:",
            &format!(
                "window_to_node:{},stacks:{{}},tree:",
                ron::to_string(&obsolete_index).unwrap()
            ),
            1,
        );
        let mut restored: BspLayoutSystem = ron::from_str(&legacy).unwrap();
        assert_eq!(restored.all_windows_in_layout(original), vec![w(1), w(2)]);
        restored.remove_window(w(1));
        assert_eq!(restored.all_windows_in_layout(original), vec![w(2)]);
    }

    #[test]
    fn insertion_alternates_splits_and_navigation_follows_orientation() {
        let (mut system, layout) = tiled(&[1, 2]);
        assert_eq!(system.window_in_direction(layout, Direction::Right), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Left), Some(w(2)));
        system.edit(layout, LayoutEdit::ToggleOrientation).discard();
        assert_eq!(system.window_in_direction(layout, Direction::Down), Some(w(1)));
        assert_eq!(system.window_in_direction(layout, Direction::Up), Some(w(2)));

        let (mut spiral, layout) = tiled(&[1]);
        for (window, horizontal, vertical) in [(2, 1, 0), (3, 1, 1), (4, 2, 1), (5, 2, 2)] {
            spiral.add_window_after_selection(layout, w(window));
            let tree = spiral.draw_tree(layout);
            assert_eq!(tree.matches("Horizontal").count(), horizontal);
            assert_eq!(tree.matches("Vertical").count(), vertical);
        }
    }

    #[test]
    fn rebalance_splits_each_axis_evenly() {
        let size = CGSize::new(1200.0, 900.0);

        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        system
            .edit(layout, LayoutEdit::Resize {
                amount: -0.4,
                orientation: ResizeOrientation::Smart,
            })
            .discard();

        assert!(system.focus(layout, Focus::Select(w(2))).handled);
        system
            .edit(layout, LayoutEdit::Resize {
                amount: -0.3,
                orientation: ResizeOrientation::Vertical,
            })
            .discard();

        system.edit(layout, LayoutEdit::Rebalance).discard();

        let frames = calculate_frames(&system, layout, size, &Default::default());
        assert_frame(&frames, w(1), (0.0, 0.0, 600.0, 900.0));
        assert_frame(&frames, w(2), (600.0, 0.0, 600.0, 450.0));
        assert_frame(&frames, w(3), (600.0, 450.0, 600.0, 450.0));
    }

    #[test]
    fn rebalance_shares_an_axis_across_nested_splits() {
        let size = CGSize::new(1200.0, 900.0);

        let (mut system, layout) = tiled(&[1]);
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        system.add_window_after_selection(layout, w(2));
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        system.add_window_after_selection(layout, w(3));

        let frames = calculate_frames(&system, layout, size, &Default::default());
        assert_frame(&frames, w(1), (0.0, 0.0, 600.0, 900.0));

        system.edit(layout, LayoutEdit::Rebalance).discard();

        let frames = calculate_frames(&system, layout, size, &Default::default());
        for window in [1, 2, 3] {
            assert!((frames[&w(window)].size.width - 400.0).abs() < 1.0);
        }
    }

    #[test]
    fn rebalance_counts_a_stacked_leaf_as_one_tile() {
        let size = CGSize::new(1200.0, 900.0);

        let (mut system, layout) = tiled(&[1]);
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        system.add_window_after_selection(layout, w(2));
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        system.add_window_after_selection(layout, w(3));
        system.join_selection_with_direction(layout, Direction::Left);

        assert_eq!(system.windows(layout, WindowQuery::Stack(w(2))), vec![
            w(2),
            w(3)
        ]);

        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        system
            .edit(layout, LayoutEdit::Resize {
                amount: -0.4,
                orientation: ResizeOrientation::Smart,
            })
            .discard();

        system.edit(layout, LayoutEdit::Rebalance).discard();

        let frames = calculate_frames(&system, layout, size, &Default::default());
        assert_frame(&frames, w(1), (0.0, 0.0, 600.0, 900.0));
    }

    #[test]
    fn focus_into_perpendicular_splits_tracks_the_source_center() {
        for vertical in [false, true] {
            for (left_resize, right_resize, targets) in [
                (0.0, 0.0, [2, 4, 1, 3]),
                (-0.5, 0.5, [2, 2, 3, 3]),
                (0.5, -0.5, [4, 4, 1, 1]),
            ] {
                let (mut system, layout) = tiled(&[1, 2]);
                if vertical {
                    system.edit(layout, LayoutEdit::ToggleOrientation).discard();
                }
                for (window, new, resize) in [(1, 3, left_resize), (2, 4, right_resize)] {
                    assert!(system.focus(layout, Focus::Select(w(window))).handled);
                    system.add_window_after_selection(layout, w(new));
                    if vertical {
                        system.edit(layout, LayoutEdit::ToggleOrientation).discard();
                    }
                    assert!(system.focus(layout, Focus::Select(w(window))).handled);
                    system
                        .edit(layout, LayoutEdit::Resize {
                            amount: resize,
                            orientation: ResizeOrientation::Smart,
                        })
                        .discard();
                }
                let (forward, backward) = if vertical {
                    (Direction::Down, Direction::Up)
                } else {
                    (Direction::Right, Direction::Left)
                };
                for ((from, direction), to) in
                    [(1, forward), (3, forward), (2, backward), (4, backward)]
                        .into_iter()
                        .zip(targets)
                {
                    assert!(system.focus(layout, Focus::Select(w(from))).handled);
                    assert_eq!(system.move_focus(layout, direction), (Some(w(to)), vec![w(to)]));
                }
            }
        }
    }

    #[test]
    fn focus_raises_only_its_target_and_layout_membership_is_independent() {
        let (mut system, first) = tiled(&[1, 2, 3]);
        assert_eq!(
            system.move_focus(first, Direction::Left),
            (Some(w(1)), vec![w(1)])
        );
        let second = system.create_layout();
        system.add_window_after_selection(second, w(1));
        system.update(WindowChange::Reconcile(first, w(1).pid, vec![w(1)]));
        assert_eq!(system.all_windows_in_layout(first), vec![w(1)]);
        assert_eq!(system.all_windows_in_layout(second), vec![w(1)]);
    }

    #[test]
    fn directional_moves_retile_in_all_axes_and_stop_at_the_edge() {
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            let vertical = matches!(direction, Direction::Up | Direction::Down);
            let backwards = matches!(direction, Direction::Left | Direction::Up);
            let (mut system, layout) = tiled(&[1, 2]);
            if vertical {
                system.edit(layout, LayoutEdit::ToggleOrientation).discard();
            }
            system.add_window_after_selection(layout, w(3));
            if vertical {
                system.edit(layout, LayoutEdit::ToggleOrientation).discard();
            }

            let moving = if backwards { w(3) } else { w(1) };
            assert!(system.focus(layout, Focus::Select(moving)).handled);
            assert!(
                system
                    .edit(layout, LayoutEdit::Move {
                        direction: direction,
                        has_display_neighbor: false
                    })
                    .changed()
            );
            assert_eq!(system.selected_window(layout), Some(moving));

            let expected = if backwards {
                [
                    (250.0, 0.0, 250.0, 1000.0),
                    (500.0, 0.0, 500.0, 1000.0),
                    (0.0, 0.0, 250.0, 1000.0),
                ]
            } else {
                [
                    (500.0, 0.0, 500.0, 500.0),
                    (0.0, 0.0, 500.0, 500.0),
                    (0.0, 500.0, 1000.0, 500.0),
                ]
            };
            let frames =
                calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default());
            for (index, (x, y, width, height)) in expected.into_iter().enumerate() {
                assert_frame(
                    &frames,
                    w(index as u32 + 1),
                    if vertical {
                        (y, x, height, width)
                    } else {
                        (x, y, width, height)
                    },
                );
            }
            assert!(
                !system
                    .edit(layout, LayoutEdit::Move {
                        direction: direction,
                        has_display_neighbor: false
                    })
                    .changed()
            );
            assert_eq!(system.selected_window(layout), Some(moving));
        }
    }

    #[test]
    fn directional_move_fills_an_empty_slot_and_collapses_the_old_split() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        assert!(
            !system
                .edit(layout, LayoutEdit::Move {
                    direction: Direction::Right,
                    has_display_neighbor: false
                })
                .changed()
        );
        assert!(system.focus(layout, Focus::Select(w(3))).handled);
        assert!(
            system
                .edit(layout, LayoutEdit::Move {
                    direction: Direction::Left,
                    has_display_neighbor: false
                })
                .changed()
        );
        assert_eq!(system.selected_window(layout), Some(w(3)));

        let frames =
            calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default());
        assert_eq!(frames.len(), 3);
        for (window, x, width) in [
            (w(1), 0.0, 250.0),
            (w(3), 250.0, 250.0),
            (w(2), 500.0, 500.0),
        ] {
            assert_frame(&frames, window, (x, 0.0, width, 1000.0));
        }
        assert!(system.focus(layout, Focus::Select(w(3))).handled);
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        let before =
            calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default());
        system.set_window_participation(w(3), false);
        let collapsed =
            calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default());
        assert_eq!(collapsed.len(), 2);
        assert_frame(&collapsed, w(1), (0.0, 0.0, 500.0, 1000.0));
        assert_frame(&collapsed, w(2), (500.0, 0.0, 500.0, 1000.0));
        system.set_window_participation(w(3), true);
        assert_eq!(
            calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default()),
            before
        );
    }

    #[test]
    fn moving_a_stack_keeps_its_membership_and_fullscreen_state() {
        use crate::layout_engine::WindowDropAction::Stack;

        for (windows, source) in [(&[1, 2, 3][..], w(3)), (&[1, 2][..], w(2))] {
            let (mut system, layout) = tiled(windows);
            for window in [4, 5] {
                system.add_window_after_selection(layout, w(window));
            }
            assert!(
                system
                    .edit(layout, LayoutEdit::Drop {
                        source: w(4),
                        target: source,
                        action: Stack
                    })
                    .changed()
            );
            assert!(
                system
                    .edit(layout, LayoutEdit::Drop {
                        source: w(5),
                        target: w(1),
                        action: Stack
                    })
                    .changed()
            );
            assert!(system.focus(layout, Focus::Select(w(4))).handled);
            system.toggle_fullscreen(layout, false);
            assert!(
                system
                    .edit(layout, LayoutEdit::Move {
                        direction: Direction::Left,
                        has_display_neighbor: false
                    })
                    .changed()
            );

            assert_eq!(system.windows(layout, WindowQuery::Stack(w(4))), vec![
                source,
                w(4)
            ]);
            assert_eq!(system.windows(layout, WindowQuery::Stack(w(5))), vec![
                w(1),
                w(5)
            ]);
            assert!(system.has_any_fullscreen_node(layout));
            system.toggle_fullscreen(layout, false);
            assert!(!system.has_any_fullscreen_node(layout));
            assert!(system.focus(layout, Focus::Select(w(4))).handled);
            system.set_window_participation(w(4), false);
            assert_eq!(system.selected_window(layout), Some(source));
            assert_eq!(system.toggle_fullscreen(layout, false), vec![source]);
            assert_eq!(system.windows(layout, WindowQuery::Stack(source)), vec![
                source,
                w(4)
            ]);
            system.set_window_participation(w(4), true);
            assert_eq!(system.toggle_fullscreen(layout, false), vec![w(4)]);
            for window in system.all_windows_in_layout(layout) {
                system.remove_window(window);
            }
            assert!(system.all_windows_in_layout(layout).is_empty());
        }
    }

    #[test]
    fn split_constraints_stay_local_and_nonbinding_minima_keep_equal_halves() {
        let (system, layout) = tiled(&[101, 102]);
        let mut constraints = HashMap::default();
        constraints.insert(
            w(101),
            WindowLayoutConstraints {
                is_resizable: true,
                max_width: 600.0,
                ..Default::default()
            }
            .normalized(),
        );
        let frames = calculate_frames(&system, layout, CGSize::new(1600.0, 900.0), &constraints);
        assert!((frames[&w(101)].size.width - 600.0).abs() < 1.0);
        assert!((frames[&w(102)].size.width - 1000.0).abs() < 1.0);
        assert!((frames[&w(102)].origin.x - 600.0).abs() < 1.0);

        let (system, layout) = tiled(&[106, 107]);
        constraints.clear();
        constraints.insert(
            w(107),
            WindowLayoutConstraints {
                is_resizable: true,
                min_width: 400.0,
                ..Default::default()
            }
            .normalized(),
        );
        let frames = calculate_frames(&system, layout, CGSize::new(1200.0, 900.0), &constraints);
        assert!((frames[&w(106)].size.width - 600.0).abs() < 1.0);
        assert!((frames[&w(107)].size.width - 600.0).abs() < 1.0);
        assert!((frames[&w(107)].origin.x - 600.0).abs() < 1.0);

        let (mut system, layout) = tiled(&[103]);
        system.edit(layout, LayoutEdit::Split(LayoutKind::Vertical)).discard();
        system.add_window_after_selection(layout, w(105));
        assert!(system.focus(layout, Focus::Select(w(103))).handled);
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        system.add_window_after_selection(layout, w(104));
        constraints.clear();
        constraints.insert(
            w(103),
            WindowLayoutConstraints {
                is_resizable: true,
                max_height: 200.0,
                ..Default::default()
            }
            .normalized(),
        );
        let frames = calculate_frames(&system, layout, CGSize::new(1200.0, 800.0), &constraints);
        assert!(frames[&w(103)].size.height <= 201.0);
        assert!(frames[&w(104)].size.height >= 399.0);
        assert!((frames[&w(105)].size.height - 400.0).abs() < 1.0);
    }
}

impl LayoutSystem for BspLayoutSystem {
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.layouts.get(layout).is_some_and(|state| state.root.traverse_preorder(&self.tree.map).any(|node| {
            matches!(self.tree.data.kind.get(node), Some(NodeKind::Leaf { window: Some(window), .. }) if window.pid == pid)
                || self.stack(node).is_some_and(|members| members.iter().any(|window| window.pid == pid))
        }))
    }

    fn window_participates(&self, window: WindowId) -> bool { !self.inactive.contains(&window) }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        if participates {
            self.inactive.remove(&window)
        } else {
            self.inactive.insert(window)
        }
    }

    fn create_layout(&mut self) -> LayoutId {
        let leaf = self.make_leaf(None);
        let state = LayoutState { root: leaf };
        self.layouts.insert(state)
    }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.layouts.contains_key(layout) }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let Some(state) = self.layouts.get(layout).copied() else {
            return self.create_layout();
        };
        let root = state.root.deep_copy(&mut self.tree).into_id();
        let cloned = self.layouts.insert(LayoutState { root });
        for (src, dest) in std::iter::zip(
            state.root.traverse_preorder(&self.tree.map),
            root.traverse_preorder(&self.tree.map),
        ) {
            if let Some(kind) = self.tree.data.kind.get(src).cloned() {
                self.tree.data.kind.insert(dest, kind);
            }
            self.tree.data.selection.copy_branch(&self.tree.map, src, dest);
        }
        cloned
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.remove(layout) {
            state.root.remove_root(&mut self.tree);
        }
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        _stack_offset: f64,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        _stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let mut out = Vec::new();
        if let Some(state) = self.layouts.get(layout).copied() {
            let rect = compute_tiling_area(screen, gaps);
            let prepared = self.prepare_geometry(state.root, constraints, gaps);
            self.calculate_layout_recursive(
                &prepared,
                state.root,
                rect,
                screen,
                constraints,
                gaps,
                &mut out,
            );
            if let (Some(ratio), [(_, frame)]) =
                (self.single_window_aspect_ratio, out.as_mut_slice())
                && matches!(
                    self.tree.data.kind.get(state.root),
                    Some(NodeKind::Leaf {
                        fullscreen: false,
                        fullscreen_within_gaps: false,
                        ..
                    })
                )
                && !self.stack(state.root).is_some()
            {
                let (width, height) = if frame.size.width > frame.size.height * ratio {
                    (frame.size.height * ratio, frame.size.height)
                } else {
                    (frame.size.width, frame.size.width / ratio)
                };
                frame.origin.x = rect.origin.x + (rect.size.width - width) / 2.0;
                frame.origin.y = rect.origin.y + (rect.size.height - height) / 2.0;
                frame.size = CGSize::new(width, height);
            }
        }
        out
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.layouts.get(layout).and_then(|s| self.selection_window(s))
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.layouts
            .get(layout)
            .and_then(|state| self.window_in_direction_from(state.root, direction))
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.node_for_window_in_layout(layout, wid).is_some()
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        if let Some(state) = self.layouts.get(layout).copied() {
            self.has_fullscreen_in_subtree(state.root)
        } else {
            false
        }
    }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        match query {
            WindowQuery::All => self.all_windows_in_layout(layout),
            WindowQuery::Visible => self.visible_windows_in_layout(layout),
            WindowQuery::Selection => self
                .selection_of_layout(layout)
                .and_then(|node| self.participating_window(self.descend_to_leaf(node)))
                .into_iter()
                .collect(),
            WindowQuery::Stack(window) => self
                .node_for_window_in_layout(layout, window)
                .and_then(|node| self.stack(node))
                .cloned()
                .unwrap_or_default(),
            WindowQuery::App(pid) => self
                .layouts
                .get(layout)
                .into_iter()
                .flat_map(|state| self.windows_under(state.root))
                .filter(|window| window.pid == pid)
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
                let Some(node) = self.node_for_window_in_layout(layout, wid) else {
                    return FocusResult::selection(false);
                };
                if let Some(stack) = self.stack_mut(node)
                    && let Some(index) = stack.iter().position(|member| *member == wid)
                {
                    stack.remove(index);
                    stack.push(wid);
                    if let Some(NodeKind::Leaf { window, .. }) = self.tree.data.kind.get_mut(node) {
                        *window = Some(wid);
                    }
                }
                self.tree.data.selection.select(&self.tree.map, node);
                true
            }),
            Focus::Ascend => FocusResult::selection({
                if let Some(sel) = self.selection_of_layout(layout) {
                    if self.tree.data.kind.get(sel).is_none() {
                        return FocusResult::selection(false);
                    }
                    let parent_opt = sel.parent(&self.tree.map);
                    if let Some(parent) = parent_opt {
                        let new_sel = self.descend_to_leaf(parent);
                        self.tree.data.selection.select(&self.tree.map, new_sel);
                        return FocusResult::selection(true);
                    }
                }
                false
            }),
            Focus::Descend => FocusResult::selection({
                if let Some(sel) = self.selection_of_layout(layout) {
                    let new_sel = self.descend_to_leaf(sel);
                    if new_sel != sel {
                        self.tree.data.selection.select(&self.tree.map, new_sel);
                        return FocusResult::selection(true);
                    }
                }
                false
            }),
            Focus::Direction(direction) => {
                FocusResult::movement(self.move_focus(layout, direction))
            }
        }
    }

    fn update(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Add(layout, window, _) => self.add_window_after_selection(layout, window),
            WindowChange::Remove(window) | WindowChange::RemoveAndRebalance(window) => {
                self.remove_window(window)
            }
            WindowChange::Replace(from, to) => {
                if self.inactive.remove(&from) {
                    self.inactive.insert(to);
                }
                if from == to {
                    return;
                }
                let layouts: Vec<_> = self.layouts.keys().collect();
                for layout in layouts {
                    if !self.contains_window(layout, from) {
                        continue;
                    }
                    // A replacement may already be present after restore matching.
                    while self.remove_window_internal(layout, to) {}
                    let root = self.layouts[layout].root;
                    for node in root.traverse_preorder(&self.tree.map) {
                        if let Some(NodeKind::Leaf { window, stack, .. }) =
                            self.tree.data.kind.get_mut(node)
                        {
                            if let Some(stack) = stack {
                                for member in stack.iter_mut().filter(|member| **member == from) {
                                    *member = to;
                                }
                            }
                            if *window == Some(from) {
                                *window = Some(to);
                            }
                        }
                    }
                }
            }
            WindowChange::CloseApp(pid) => {
                self.inactive.retain(|window| window.pid != pid);
                let mut windows: Vec<_> = self
                    .layouts
                    .values()
                    .flat_map(|state| self.windows_under(state.root))
                    .filter(|window| window.pid == pid)
                    .collect();
                windows.sort_unstable();
                windows.dedup();
                for window in windows {
                    self.remove_window(window);
                }
            }
            WindowChange::Reconcile(layout, pid, desired) => {
                let current = self.windows(layout, WindowQuery::App(pid));
                let delta = reconcile_app_membership(pid, current, desired, |w| {
                    self.window_participates(w)
                });
                for wid in delta.removals {
                    if let Some(node) = self.node_for_window_in_layout(layout, wid)
                        && let Some(NodeKind::Leaf {
                            fullscreen,
                            fullscreen_within_gaps,
                            ..
                        }) = self.tree.data.kind.get(node)
                        && (*fullscreen || *fullscreen_within_gaps)
                    {
                        continue;
                    }
                    self.remove_window_internal(layout, wid);
                }
                for wid in delta.additions {
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
                if let Some(node) = self.node_for_window_in_layout(layout, wid) {
                    if let Some(NodeKind::Leaf {
                        fullscreen,
                        fullscreen_within_gaps,
                        ..
                    }) = self.tree.data.kind.get_mut(node)
                        && super::track_fullscreen_resize(
                            fullscreen,
                            fullscreen_within_gaps,
                            old_frame,
                            new_frame,
                            screen,
                            gaps,
                        )
                    {
                        return;
                    }

                    let width_changed = (new_frame.size.width - old_frame.size.width).abs() > 0.5;
                    let height_changed =
                        (new_frame.size.height - old_frame.size.height).abs() > 0.5;
                    if !width_changed && !height_changed {
                        return;
                    }

                    let mut current = node;
                    while let Some(parent) = current.parent(&self.tree.map) {
                        let Some(NodeKind::Split { orientation, ratio }) =
                            self.tree.data.kind.get_mut(parent)
                        else {
                            current = parent;
                            continue;
                        };

                        let (old_len, new_len) = match orientation {
                            Orientation::Horizontal if width_changed => {
                                (old_frame.size.width, new_frame.size.width)
                            }
                            Orientation::Vertical if height_changed => {
                                (old_frame.size.height, new_frame.size.height)
                            }
                            _ => {
                                current = parent;
                                continue;
                            }
                        };

                        let current_ratio = f64::from(*ratio);
                        let is_first = Some(current) == parent.first_child(&self.tree.map);
                        let denom = if is_first {
                            current_ratio
                        } else {
                            1.0 - current_ratio
                        };
                        if old_len <= 0.0 || denom <= f64::EPSILON {
                            break;
                        }

                        let parent_len = old_len / denom;
                        if parent_len <= 0.0 {
                            break;
                        }

                        let raw_ratio = (new_len / parent_len).clamp(0.05, 0.95);
                        *ratio = if is_first {
                            raw_ratio as f32
                        } else {
                            (1.0 - raw_ratio) as f32
                        };
                        break;
                    }
                }
            }
        }
    }

    fn edit_layout(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult {
        match edit {
            LayoutEdit::Move {
                direction,
                has_display_neighbor: _,
            } => EditResult::from({
                let Some(sel) = self.selection_of_layout(layout) else {
                    return EditResult::Unchanged;
                };
                let moving = self.descend_to_leaf(sel);
                if !matches!(
                    self.tree.data.kind.get(moving),
                    Some(NodeKind::Leaf { window: Some(_), .. })
                ) {
                    return EditResult::Unchanged;
                }
                let Some(target) = self.find_neighbor_leaf(moving, direction) else {
                    return EditResult::Unchanged;
                };
                let Some(old_parent) = moving.parent(&self.tree.map) else {
                    return EditResult::Unchanged;
                };

                // Siblings already share the requested split: reorder them without
                // resetting the split ratio or exchanging their window state.
                if target.parent(&self.tree.map) == Some(old_parent) {
                    moving.reorder(
                        target,
                        matches!(direction, Direction::Left | Direction::Up),
                        &mut self.tree.map,
                    );
                } else if matches!(
                    self.tree.data.kind.get(target),
                    Some(NodeKind::Leaf { window: None, .. })
                ) {
                    // A pre-created empty slot receives the leaf directly.
                    moving.detach(&mut self.tree).insert_before(target);
                    target.detach(&mut self.tree).remove();
                    self.collapse_split(old_parent);
                } else {
                    // Transfer the entire leaf (including stacks and fullscreen state)
                    // onto a new split at the directional neighbor, then reclaim its
                    // old region for the sibling left behind.
                    let split = self.tree.mk_node().into_id();
                    self.tree.data.kind.insert(split, NodeKind::Split {
                        orientation: direction.orientation(),
                        ratio: 0.5,
                    });
                    split.detach(&mut self.tree).insert_before(target);
                    let (first, second) = match direction {
                        Direction::Left | Direction::Up => (moving, target),
                        Direction::Right | Direction::Down => (target, moving),
                    };
                    first.detach(&mut self.tree).push_back(split);
                    second.detach(&mut self.tree).push_back(split);
                    self.collapse_split(old_parent);
                }
                self.tree.data.selection.select(&self.tree.map, moving);
                true
            }),
            LayoutEdit::Transfer(to) => {
                let from_layout = layout;
                let to_layout = to;

                let sel = self.selected_window(from_layout);
                if let Some(w) = sel {
                    while self.remove_window_internal(from_layout, w) {}
                    self.add_window_after_selection(to_layout, w);
                };
                EditResult::Changed
            }
            LayoutEdit::Split(kind) => {
                if kind.is_stacked() {
                    return EditResult::Unsupported;
                }
                let Some(selection) = self.selection_of_layout(layout) else {
                    return EditResult::Unchanged;
                };
                let leaf = self.descend_to_leaf(selection);
                EditResult::from(self.split_leaf(leaf, kind.orientation(), None, false))
            }
            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::Swap(a, b) => EditResult::from(self.swap_windows(layout, a, b)),
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(self.apply_target_drop(layout, source, target, action))
            }
            LayoutEdit::Group(GroupEdit::Join(direction)) => {
                self.join_selection_with_direction(layout, direction);
                EditResult::Changed
            }
            LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)) => {
                let is_joined = self
                    .selection_of_layout(layout)
                    .map(|selection| self.descend_to_leaf(selection))
                    .and_then(|leaf| self.stack(leaf))
                    .is_some_and(|members| members.len() > 1);

                if is_joined {
                    self.unjoin_selection(layout);
                } else {
                    self.join_selection_with_direction(layout, direction);
                };
                EditResult::Changed
            }
            LayoutEdit::Group(GroupEdit::Unjoin) => {
                self.unjoin_selection(layout);
                EditResult::Changed
            }
            LayoutEdit::Resize { amount, orientation } => {
                let sel_snapshot = self.selection_of_layout(layout);
                let Some(mut node) = sel_snapshot else {
                    return EditResult::Unchanged;
                };

                while let Some(parent) = node.parent(&self.tree.map) {
                    if let Some(NodeKind::Split {
                        orientation: split_orientation,
                        ratio,
                    }) = self.tree.data.kind.get_mut(parent)
                        && match orientation {
                            ResizeOrientation::Horizontal => {
                                *split_orientation == Orientation::Horizontal
                            }
                            ResizeOrientation::Vertical => {
                                *split_orientation == Orientation::Vertical
                            }
                            ResizeOrientation::Smart => true,
                        }
                    {
                        let is_first = Some(node) == parent.first_child(&self.tree.map);
                        let delta = (amount as f32) * 0.5;
                        if is_first {
                            *ratio = (*ratio + delta).clamp(0.05, 0.95);
                        } else {
                            *ratio = (*ratio - delta).clamp(0.05, 0.95);
                        }
                        break;
                    }
                    node = parent;
                }
                EditResult::Changed
            }
            LayoutEdit::Rebalance => {
                if let Some(state) = self.layouts.get(layout).copied() {
                    self.rebalance_node(state.root, Orientation::Horizontal);
                };
                EditResult::Changed
            }
            LayoutEdit::ToggleOrientation => {
                let Some(start) = self.selection_of_layout(layout) else {
                    return EditResult::Unchanged;
                };
                for node in start.ancestors(&self.tree.map) {
                    if let Some(NodeKind::Split { orientation, .. }) =
                        self.tree.data.kind.get_mut(node)
                    {
                        *orientation = match *orientation {
                            Orientation::Horizontal => Orientation::Vertical,
                            Orientation::Vertical => Orientation::Horizontal,
                        };
                        return EditResult::Changed;
                    }
                }
                EditResult::Unchanged
            }
            _ => EditResult::Unsupported,
        }
    }
}

impl BspLayoutSystem {
    fn all_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layouts
            .get(layout)
            .into_iter()
            .flat_map(|state| self.windows_under(state.root))
            .collect()
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.layouts
            .get(layout)
            .into_iter()
            .flat_map(|state| state.root.traverse_preorder(&self.tree.map))
            .filter_map(|node| self.participating_window(node))
            .collect()
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        let sel_snapshot = self.selection_of_layout(layout);
        let Some(current_sel) = sel_snapshot else {
            return (None, vec![]);
        };
        let current_leaf = self.descend_to_leaf(current_sel);
        let Some(next_leaf) = self.find_neighbor_leaf(current_leaf, direction) else {
            return (None, vec![]);
        };
        self.tree.data.selection.select(&self.tree.map, next_leaf);
        let focus = self.participating_window(next_leaf);
        // Tiled BSP windows never overlap, so only the focus target needs to be
        // raised. Raising every visible window makes the raise manager front
        // each app in turn (via make_key_window) before the target, which
        // flickers the menu bar and any focus border on every focus move. The
        // traditional engine already raises only the revealed group.
        (focus, focus.into_iter().collect())
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.contains_window(layout, wid) {
            return;
        }
        if self.layouts.get(layout).is_some() {
            if self.window_insertion_point == WindowInsertionPoint::EndOfTree {
                let root = self.layouts[layout].root;
                if let Some(leaf) = root
                    .traverse_preorder(&self.tree.map)
                    .filter(|node| {
                        matches!(self.tree.data.kind.get(*node), Some(NodeKind::Leaf { .. }))
                    })
                    .last()
                {
                    self.tree.data.selection.select(&self.tree.map, leaf);
                }
                self.insert_window_at_selection(layout, wid);
                return;
            }
            // Try smart insertion first (with preselection support)
            if !self.smart_insert_window(layout, wid) {
                // Fall back to default insertion
                self.insert_window_at_selection(layout, wid);
            }
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        self.inactive.remove(&wid);
        let layouts: Vec<_> = self.layouts.keys().collect();
        for layout in layouts {
            while self.remove_window_internal(layout, wid) {}
        }
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        if !self.window_participates(a) || !self.window_participates(b) {
            return false;
        }
        let Some(node_a) = self.node_for_window_in_layout(layout, a) else {
            return false;
        };
        let Some(node_b) = self.node_for_window_in_layout(layout, b) else {
            return false;
        };
        if node_a == node_b {
            let Some(stack) = self.stack_mut(node_a) else {
                return false;
            };
            if !stack.contains(&a) || !stack.contains(&b) {
                return false;
            }
            // A BSP stack has one active leaf window. Keep the dragged source active
            // while moving the target immediately behind it in stack order.
            stack.retain(|window| *window != a && *window != b);
            stack.push(b);
            stack.push(a);
            if let Some(NodeKind::Leaf { window, .. }) = self.tree.data.kind.get_mut(node_a) {
                *window = Some(a);
            }
            self.tree.data.selection.select(&self.tree.map, node_a);
            return true;
        }

        let Some(
            [
                NodeKind::Leaf {
                    window: first,
                    stack: first_group,
                    ..
                },
                NodeKind::Leaf {
                    window: second,
                    stack: second_group,
                    ..
                },
            ],
        ) = self.tree.data.kind.get_disjoint_mut([node_a, node_b])
        else {
            return false;
        };
        // Swap occupants while retaining each slot's fullscreen and split state.
        // A transferred group uses its last tab, just as an insertion does.
        for (window, group) in [
            (&mut *first, &mut *first_group),
            (&mut *second, &mut *second_group),
        ] {
            if let Some(members) = group {
                *window = members.last().copied();
                if members.len() <= 1 {
                    *group = None;
                }
            }
        }
        std::mem::swap(first, second);
        std::mem::swap(first_group, second_group);

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
        if source == target
            || !self.contains_window(layout, source)
            || !self.contains_window(layout, target)
        {
            return false;
        }
        if action == crate::layout_engine::WindowDropAction::Swap {
            return self.swap_windows(layout, source, target);
        }

        while self.remove_window_internal(layout, source) {}
        let Some(target_node) = self.node_for_window_in_layout(layout, target) else {
            return false;
        };
        match action {
            crate::layout_engine::WindowDropAction::Stack => {
                let Some(NodeKind::Leaf { stack, .. }) = self.tree.data.kind.get_mut(target_node)
                else {
                    return false;
                };
                let stack = stack.get_or_insert_with(|| vec![target]);
                stack.retain(|window| *window != source);
                stack.push(source);
                if let Some(NodeKind::Leaf { window, .. }) =
                    self.tree.data.kind.get_mut(target_node)
                {
                    *window = Some(source);
                }

                self.tree.data.selection.select(&self.tree.map, target_node);
                true
            }
            crate::layout_engine::WindowDropAction::Insert(direction) => {
                self.split_leaf(
                    target_node,
                    direction.orientation(),
                    Some(source),
                    matches!(direction, Direction::Left | Direction::Up),
                );
                true
            }
            crate::layout_engine::WindowDropAction::Swap
            | crate::layout_engine::WindowDropAction::Move(_)
            | crate::layout_engine::WindowDropAction::ConsumeOrExpel(_) => {
                unreachable!()
            }
        }
    }

    fn toggle_fullscreen(&mut self, layout: LayoutId, within_gaps: bool) -> Vec<WindowId> {
        let Some(selection) = self.selection_of_layout(layout) else {
            return Vec::new();
        };
        let leaf = self.descend_to_leaf(selection);
        let active = self.participating_window(leaf);
        let Some(NodeKind::Leaf {
            window: Some(_),
            fullscreen,
            fullscreen_within_gaps,
            ..
        }) = self.tree.data.kind.get_mut(leaf)
        else {
            return Vec::new();
        };
        super::toggle_fullscreen_mode(fullscreen, fullscreen_within_gaps, within_gaps);
        active.into_iter().collect()
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
            if neighbor.parent(&self.tree.map) == Some(parent) {
                if parent.parent(&self.tree.map).is_some() {
                    let mut windows: Vec<_> = self.windows_under(parent).collect();

                    // The last member is the visible one, so keep the selected window there.
                    let selected = self.selected_window(layout);

                    if let Some(index) = windows.iter().position(|window| Some(*window) == selected)
                    {
                        windows[index..].rotate_left(1);
                    }

                    if let Some(visible) = windows.last().copied() {
                        // Take the collapsed split's place so a first child stays first.
                        let new_leaf = self.make_leaf(Some(visible));
                        new_leaf.detach(&mut self.tree).insert_after(parent);

                        parent.detach(&mut self.tree).remove();

                        self.set_stack(new_leaf, windows);
                        self.tree.data.selection.select(&self.tree.map, new_leaf);
                    }
                }
                break;
            }
            current = parent;
        }
    }

    fn unjoin_selection(&mut self, layout: LayoutId) {
        let Some(sel) = self.selection_of_layout(layout) else {
            return;
        };

        let leaf = self.descend_to_leaf(sel);
        let Some(NodeKind::Leaf {
            window,
            stack,
            fullscreen,
            fullscreen_within_gaps,
            ..
        }) = self.tree.data.kind.get_mut(leaf)
        else {
            return;
        };

        let Some(members) = stack.take() else {
            return;
        };

        let flags = (*fullscreen, *fullscreen_within_gaps);

        // Retile this stack in place using depth-based splits.
        // The visible member is last in the stack, so it ends up selected.
        *window = None;
        self.tree.data.selection.select(&self.tree.map, leaf);

        for member in members {
            self.insert_window_at_selection(layout, member);
        }

        // Insertion rebuilds leaves without fullscreen, so hand it back to the visible member.
        if let Some(visible) = self.selection_of_layout(layout)
            && let Some(NodeKind::Leaf {
                fullscreen,
                fullscreen_within_gaps,
                ..
            }) = self.tree.data.kind.get_mut(visible)
        {
            (*fullscreen, *fullscreen_within_gaps) = flags;
        }
    }
}

impl BspLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        fn write_node(this: &BspLayoutSystem, node: NodeId, out: &mut String, indent: usize) {
            for _ in 0..indent {
                out.push_str("  ");
            }
            match this.tree.data.kind.get(node) {
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

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        fn snapshot(
            system: &BspLayoutSystem,
            node: NodeId,
            selected: NodeId,
        ) -> rift_protocol::ContainerTreeNode {
            let weight = node.parent(&system.tree.map).and_then(|parent| {
                let NodeKind::Split { ratio, .. } = system.tree.data.kind.get(parent)? else {
                    return None;
                };
                let first = parent.first_child(&system.tree.map);
                Some(if first == Some(node) {
                    f64::from(*ratio)
                } else {
                    1.0 - f64::from(*ratio)
                })
            });

            match system.tree.data.kind.get(node) {
                Some(NodeKind::Split { orientation, .. }) => rift_protocol::ContainerTreeNode {
                    node_id: node.data().as_ffi(),
                    node_type: rift_protocol::ContainerNodeType::Container,
                    frame: Default::default(),
                    layout_kind: Some(rift_protocol::LayoutKind::from(*orientation)),
                    weight,
                    window_id: None,
                    is_selected: node == selected,
                    is_fullscreen: false,
                    is_fullscreen_within_gaps: false,
                    role: None,
                    pending_split: None,
                    children: node
                        .children(&system.tree.map)
                        .map(|child| snapshot(system, child, selected))
                        .collect(),
                },
                Some(NodeKind::Leaf {
                    window,
                    fullscreen,
                    fullscreen_within_gaps,
                    preselected,
                    ..
                }) => {
                    let members = system.stack(node);
                    let stacked = members.is_some();
                    rift_protocol::ContainerTreeNode {
                        node_id: node.data().as_ffi(),
                        node_type: if stacked {
                            rift_protocol::ContainerNodeType::Container
                        } else if window.is_some() {
                            rift_protocol::ContainerNodeType::Window
                        } else {
                            rift_protocol::ContainerNodeType::Placeholder
                        },
                        frame: Default::default(),
                        layout_kind: stacked.then_some(rift_protocol::LayoutKind::HorizontalStack),
                        weight,
                        window_id: if stacked {
                            None
                        } else {
                            window.map(Into::into)
                        },
                        is_selected: node == selected,
                        is_fullscreen: *fullscreen,
                        is_fullscreen_within_gaps: *fullscreen_within_gaps,
                        role: None,
                        pending_split: preselected.map(Into::into),
                        children: members.map_or_else(Vec::new, |members| {
                            members
                                .iter()
                                .map(|member| rift_protocol::ContainerTreeNode {
                                    node_id: (u64::from(member.pid as u32) << 32)
                                        | u64::from(member.idx.get()),
                                    node_type: rift_protocol::ContainerNodeType::Window,
                                    frame: Default::default(),
                                    layout_kind: None,
                                    weight: None,
                                    window_id: Some((*member).into()),
                                    is_selected: node == selected && *window == Some(*member),
                                    is_fullscreen: *fullscreen,
                                    is_fullscreen_within_gaps: *fullscreen_within_gaps,
                                    role: None,
                                    pending_split: None,
                                    children: Vec::new(),
                                })
                                .collect()
                        }),
                    }
                }
                None => unreachable!("BSP layout contains a node without metadata"),
            }
        }

        let state = self.layouts.get(layout).expect("unknown BSP layout");
        let selected = self.tree.data.selection.current_selection(state.root);
        snapshot(self, state.root, selected)
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        let root = self.layouts.get(layout)?.root;
        let node = self.node_for_window_in_layout(layout, window)?;
        let mut slot = crate::layout_engine::systems::node_slot(node, root, &self.tree.map)?;
        if let Some(members) = self.stack(node) {
            slot.push(members.iter().position(|&member| member == window)?);
        }
        Some(slot)
    }
}
