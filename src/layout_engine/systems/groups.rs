use std::ops::ControlFlow;

use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use slotmap::Key;

use super::{EditResult, Focus, FocusResult, WindowQuery};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashSet;
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation};
use crate::model::selection::*;
use crate::model::tree::{
    LayoutInfo, NodeId, NodeMap, Placement, Tree, WindowIndex, WindowNodeInfo, WindowNodeInfoVec,
};

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

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub(super) struct GroupLayouts {
    #[serde(skip)]
    pub(super) inactive: HashSet<WindowId>,
    #[serde(deserialize_with = "deserialize_tile_tree")]
    pub(super) tree: Tree,
    pub(super) layout_roots: slotmap::SlotMap<LayoutId, LayoutRoot>,
}

pub(super) fn deserialize_tile_tree<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Tree, D::Error> {
    #[derive(Deserialize, Default)]
    struct LegacyLayout {
        info: slotmap::SecondaryMap<NodeId, LayoutInfo>,
    }
    #[derive(Deserialize)]
    struct StoredIndex {
        #[serde(default)]
        windows: slotmap::SecondaryMap<NodeId, WindowId>,
        window_nodes: crate::common::collections::BTreeMap<WindowId, WindowNodeInfoVec>,
    }
    #[derive(Deserialize)]
    struct StoredData {
        #[serde(default)]
        selection: Selection,
        #[serde(default)]
        layout: LegacyLayout,
        window: StoredIndex,
    }
    #[derive(Deserialize)]
    struct StoredTree {
        map: NodeMap,
        #[serde(default, deserialize_with = "super::present")]
        data: Option<StoredData>,
        #[serde(default, deserialize_with = "super::present")]
        window: Option<WindowIndex>,
    }
    let StoredTree { mut map, data, window } = StoredTree::deserialize(deserializer)?;
    let Some(data) = data else {
        return Ok(Tree {
            map,
            window: window.unwrap_or_default(),
        });
    };
    for (node, layout) in data.layout.info {
        if !map.contains(node) {
            return Err(serde::de::Error::custom(
                "layout metadata references a missing node",
            ));
        }
        map[node].layout = layout;
    }
    for (node, selection) in &data.selection.nodes {
        if !map.contains(node)
            || map.window(node).is_some()
            || data.selection.last_selection(node).and_then(|child| child.parent(&map))
                != Some(node)
        {
            return Err(serde::de::Error::custom(
                "layout selection references an invalid child",
            ));
        }
        *map[node].selection_mut() = Some(*selection);
    }
    for (node, window) in data.window.windows {
        if !map.contains(node) || node.first_child(&map).is_some() {
            return Err(serde::de::Error::custom(
                "layout window references an invalid leaf",
            ));
        }
        map[node].set_window(window);
    }
    Ok(Tree {
        map,
        window: WindowIndex {
            window_nodes: data.window.window_nodes,
        },
    })
}

#[derive(Clone, Copy)]
enum BranchLimit {
    All,
    First,
    FirstVisible,
}

impl NodeMap {
    fn ordered_children(
        &self,
        node: NodeId,
        preferred: Option<NodeId>,
        reverse: bool,
    ) -> impl Iterator<Item = NodeId> + '_ {
        let edge = if reverse {
            node.last_child(self)
        } else {
            node.first_child(self)
        };
        preferred.into_iter().chain(
            std::iter::successors(edge, move |node| {
                if reverse {
                    node.prev_sibling(self)
                } else {
                    node.next_sibling(self)
                }
            })
            .filter(move |&child| Some(child) != preferred),
        )
    }

    pub(super) fn participating_child(
        &self,
        node: NodeId,
        reverse: bool,
        mut participates: impl FnMut(NodeId) -> bool,
    ) -> Option<NodeId> {
        self.ordered_children(node, self.local_selection(node), reverse)
            .find(|&child| participates(child))
    }

    fn walk_windows<B>(
        &self,
        node: NodeId,
        participates: &impl Fn(WindowId) -> bool,
        order: &impl Fn(NodeId) -> (Option<NodeId>, bool, BranchLimit),
        visit: &mut impl FnMut(NodeId, WindowId) -> ControlFlow<B>,
    ) -> ControlFlow<B, bool> {
        if let Some(window) = self.window(node) {
            let active = participates(window);
            if active {
                visit(node, window)?;
            }
            return ControlFlow::Continue(active);
        }
        let (preferred, reverse, limit) = order(node);
        let mut visited = false;
        for child in self.ordered_children(node, preferred, reverse) {
            visited |= self.walk_windows(child, participates, order, visit)?;
            if matches!(limit, BranchLimit::First)
                || (visited && matches!(limit, BranchLimit::FirstVisible))
            {
                break;
            }
        }
        ControlFlow::Continue(visited)
    }

    fn find_window(
        &self,
        node: NodeId,
        participates: &impl Fn(WindowId) -> bool,
        order: &impl Fn(NodeId) -> (Option<NodeId>, bool),
    ) -> Option<(NodeId, WindowId)> {
        self.walk_windows(
            node,
            participates,
            &|node| {
                let (preferred, reverse) = order(node);
                (preferred, reverse, BranchLimit::All)
            },
            &mut |node, window| ControlFlow::Break((node, window)),
        )
        .break_value()
    }

    pub(super) fn focus_target(
        &self,
        node: NodeId,
        participates: &impl Fn(WindowId) -> bool,
    ) -> Option<(NodeId, WindowId)> {
        self.find_window(node, participates, &|node| (self.last_selection(node), false))
    }

    pub(crate) fn assume_size_of(&mut self, new: NodeId, old: NodeId) {
        assert_eq!(new.parent(self), old.parent(self));
        self[new].layout.size = core::mem::replace(&mut self[old].layout.size, 0.0);
    }

    pub(super) fn set_kind(&mut self, node: NodeId, kind: LayoutKind) {
        self[node].layout.kind = kind;
    }

    pub(super) fn kind(&self, node: NodeId) -> LayoutKind { self[node].layout.kind }

    pub(super) fn is_effectively_fullscreen(&self, node: NodeId) -> bool {
        let info = &self[node].layout;
        info.is_fullscreen || info.is_fullscreen_within_gaps
    }

    fn debug(&self, node: NodeId, is_container: bool) -> String {
        let info = &self[node].layout;
        if is_container {
            format!(
                "{:?} [size {} total={}]",
                info.kind,
                info.size,
                self.children_total(node)
            )
        } else {
            format!("[size {}]", info.size)
        }
    }
}

impl GroupLayouts {
    pub(super) fn remove_window(&mut self, window: WindowId) -> Vec<WindowNodeInfo> {
        self.inactive.remove(&window);
        let removed = self.tree.window.take_window(window);
        for info in &removed {
            self.tree.remove(info.node);
        }
        removed
    }

    pub(super) fn replace_window(&mut self, from: WindowId, to: WindowId) {
        super::replace_participation(&mut self.inactive, from, to);
        self.tree.window.replace_window(&mut self.tree.map, from, to);
    }

    pub(super) fn close_app(&mut self, pid: pid_t) {
        self.inactive.retain(|w| w.pid != pid);
        for node in self.tree.window.take_nodes_for_app(pid) {
            self.tree.remove(node);
        }
    }

    pub(super) fn toggle_orientation(&mut self, layout: LayoutId) -> EditResult {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
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

    pub(super) fn toggle_fullscreen(
        &mut self,
        layout: LayoutId,
        within_gaps: bool,
    ) -> Vec<WindowId> {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return Vec::new();
        }
        let node = self.selection(layout);
        let info = &mut self.tree.map[node].layout;
        if super::toggle_fullscreen_mode(
            &mut info.is_fullscreen,
            &mut info.is_fullscreen_within_gaps,
            within_gaps,
        ) {
            self.visible_windows_under_internal(node)
        } else {
            Vec::new()
        }
    }

    pub(super) fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind) -> EditResult {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return EditResult::Unchanged;
        }
        let selection = self.selection(layout);
        self.nest_in_container_internal(layout, selection, kind);
        EditResult::Changed
    }

    pub(super) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        self.container_tree_with_roles(layout, &[])
    }

    pub(super) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        crate::layout_engine::systems::node_slot(
            self.window_node(layout, window)?,
            self.root(layout),
            self.map(),
        )
    }

    fn get_ascii_tree_with_labels(
        &self,
        node: NodeId,
        labels: &[(NodeId, &str)],
    ) -> ascii_tree::Tree {
        let status = match node.parent(&self.tree.map) {
            None => "",
            Some(parent) if self.tree.map.local_selection(parent) == Some(node) => "☒ ",
            _ => "☐ ",
        };
        let desc = format!("{status}{node:?}");
        let desc = match self.window_at(node) {
            Some(wid) => format!("{desc} {:?} {}", wid, self.tree.map.debug(node, false)),
            None => format!("{desc} {}", self.tree.map.debug(node, true)),
        };
        let desc = if let Some(label) =
            labels.iter().find_map(|&(id, label)| (id == node).then_some(label))
        {
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

    pub(super) fn add_window_under(
        &mut self,
        layout: LayoutId,
        parent: NodeId,
        wid: WindowId,
    ) -> NodeId {
        let node = self.tree.insert(Placement::Back(parent));
        self.tree.window.set_window(&mut self.tree.map, layout, node, wid);
        node
    }

    pub(super) fn window_at(&self, node: NodeId) -> Option<WindowId> { self.tree.map.window(node) }

    pub(super) fn container_tree_with_roles(
        &self,
        layout: LayoutId,
        roles: &[(NodeId, &str)],
    ) -> rift_protocol::ContainerTreeNode {
        fn snapshot(
            system: &GroupLayouts,
            node: NodeId,
            selected: NodeId,
            roles: &[(NodeId, &str)],
        ) -> rift_protocol::ContainerTreeNode {
            let window = system.window_at(node);
            let info = system.tree.map[node].layout;
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
                role: roles.iter().find_map(|&(id, role)| (id == node).then(|| role.to_owned())),
                pending_split: None,
                children: node
                    .children(system.map())
                    .map(|child| snapshot(system, child, selected, roles))
                    .collect(),
            }
        }

        snapshot(self, self.root(layout), self.selection(layout), roles)
    }

    pub(super) fn draw_tree(&self, layout: LayoutId, labels: &[(NodeId, &str)]) -> String {
        let tree = self.get_ascii_tree_with_labels(self.root(layout), labels);
        let mut out = String::new();
        ascii_tree::write_tree(&mut out, &tree).unwrap();
        out
    }

    pub(super) fn select(&mut self, selection: NodeId) { self.tree.map.select(selection) }

    pub(super) fn traverse_internal(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let map = &self.tree.map;
        if let Some(sibling) = self.move_over(from, direction) {
            return Some(sibling);
        }
        from.ancestors(map)
            .skip(1)
            .find_map(|ancestor| self.move_over(ancestor, direction))
            .map(|target| self.descend_into_target(target, direction, map))
    }

    fn descend_into_target(&self, target: NodeId, direction: Direction, map: &NodeMap) -> NodeId {
        let active = self.participating_nodes(target);
        let mut current = target;
        while let Some(child) = map.participating_child(
            current,
            !map.kind(current).is_stacked()
                && matches!(direction, Direction::Right | Direction::Down),
            |child| self.inactive.is_empty() || active.contains(&child),
        ) {
            current = child;
        }
        current
    }

    fn visible_windows_under_internal(&self, node: NodeId) -> Vec<WindowId> {
        let mut windows = Vec::new();
        self.visit_visible_windows(node, &mut |_, window| {
            windows.push(window);
            ControlFlow::<()>::Continue(())
        })
        .continue_value();
        windows
    }

    pub(super) fn first_visible_window(&self, node: NodeId) -> Option<(NodeId, WindowId)> {
        self.visit_visible_windows(node, &mut |node, window| ControlFlow::Break((node, window)))
            .break_value()
    }

    pub(super) fn visit_visible_windows<B>(
        &self,
        node: NodeId,
        visit: &mut impl FnMut(NodeId, WindowId) -> ControlFlow<B>,
    ) -> ControlFlow<B, bool> {
        let map = self.map();
        let group_limit = if self.inactive.is_empty() {
            BranchLimit::First
        } else {
            BranchLimit::FirstVisible
        };
        map.walk_windows(
            node,
            &|window| self.window_participates(window),
            &|node| {
                if map.kind(node).is_group() {
                    (map.local_selection(node), false, group_limit)
                } else {
                    (None, false, BranchLimit::All)
                }
            },
            visit,
        )
    }

    pub(super) fn move_over(&self, from: NodeId, direction: Direction) -> Option<NodeId> {
        let Some(parent) = from.parent(&self.tree.map) else {
            return None;
        };
        if self.tree.map.kind(parent).orientation() != direction.orientation() {
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

    pub(super) fn nest_in_container_internal(
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
                let is_selection = self.tree.map.local_selection(old_parent) == Some(node);
                let new_parent = self.tree.insert(Placement::Before(node));
                self.tree.map.assume_size_of(new_parent, node);
                self.tree.move_node(node, Placement::Back(new_parent));
                if is_selection {
                    self.tree.map.select_locally(new_parent);
                }
                new_parent
            } else {
                let new_root = self.tree.create_root();
                self.layout_roots[layout] = LayoutRoot(new_root);
                self.tree.move_node(node, Placement::Back(new_root));
                new_root
            };
            self.tree.map.select_locally(node);
            new_parent
        };
        self.tree.map.set_kind(parent, kind);
        parent
    }

    pub(super) fn stack_group_container_info(
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

    pub(super) fn descend_selection(&mut self, layout: LayoutId) -> bool {
        if !self.inactive.is_empty() && self.find_best_focus_target(self.root(layout)).is_none() {
            return false;
        }
        if let Some(child) = self.tree.map.last_selection(self.selection(layout)) {
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
            let mut highest_revealed = focus_node;
            let mut node = focus_node;
            while let Some(parent) = node.parent(&self.tree.map) {
                let is_group = self.layout(parent).is_group();
                if self.tree.map.select_locally(node) && is_group {
                    highest_revealed = node;
                }
                node = parent;
            }
            let raise_windows = self.visible_windows_under_internal(highest_revealed);
            (Some(focus_window), raise_windows)
        } else {
            (None, vec![])
        }
    }

    pub(super) fn apply_stacking_to_parent_of_selection(
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

    pub(super) fn unstack_parent_of_selection(
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

    pub(super) fn move_stack_tab(
        &mut self,
        layout: LayoutId,
        window: WindowId,
        target: WindowId,
    ) -> bool {
        if !self.window_participates(window) || !self.window_participates(target) {
            return false;
        }
        let map = &self.tree.map;
        let (Some(source), Some(target)) = (
            self.tree.window.node_for(layout, window),
            self.tree.window.node_for(layout, target),
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
        if moving == anchor {
            return false;
        }
        let forward = stack.children(map).position(|c| c == moving)
            < stack.children(map).position(|c| c == anchor);
        let place = if forward {
            Placement::After(anchor)
        } else {
            Placement::Before(anchor)
        };
        self.tree.move_node(moving, place)
    }

    pub(super) fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        if !self.window_participates(a) || !self.window_participates(b) {
            return false;
        }
        let node_a = match self.tree.window.node_for(layout, a) {
            Some(n) => n,
            None => return false,
        };
        let node_b = match self.tree.window.node_for(layout, b) {
            Some(n) => n,
            None => return false,
        };

        if node_a == node_b {
            return false;
        }

        let selection = self.selection(layout);
        // Exchange the occupants of the slots without restructuring the tree.
        // Reparenting a leaf can collapse a single-child container mid-swap.
        let index = &mut self.tree.window;
        self.tree.map[node_a].set_window(b);
        self.tree.map[node_b].set_window(a);
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

    pub(super) fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.app_members(layout, pid).next().is_some()
    }

    fn app_members(&self, layout: LayoutId, pid: pid_t) -> impl Iterator<Item = WindowId> + '_ {
        self.tree
            .window
            .window_nodes
            .range(WindowId::new(pid, 1)..=WindowId::new(pid, u32::MAX))
            .filter_map(move |(&window, nodes)| {
                nodes.0.iter().any(|info| info.layout == layout).then_some(window)
            })
    }

    pub(super) fn window_participates(&self, window: WindowId) -> bool {
        !self.inactive.contains(&window)
    }

    pub(super) fn set_window_participation(
        &mut self,
        window: WindowId,
        participates: bool,
    ) -> bool {
        super::update_participation(&mut self.inactive, window, participates)
    }

    pub(super) fn create_layout(&mut self) -> LayoutId {
        let root = self.tree.create_root();
        self.layout_roots.insert(LayoutRoot(root))
    }

    pub(super) fn contains_layout(&self, layout: LayoutId) -> bool {
        self.layout_roots.contains_key(layout)
    }

    pub(super) fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let root = self.root(layout);
        self.layout_roots
            .insert_with_key(|destination| LayoutRoot(self.tree.clone_branch(root, destination)))
    }

    pub(super) fn remove_layout(&mut self, layout: LayoutId) {
        self.tree.remove(self.layout_roots.remove(layout).unwrap().0)
    }

    pub(super) fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.focused_leaf(layout).map(|(_, window)| window)
    }

    pub(super) fn focused_leaf(&self, layout: LayoutId) -> Option<(NodeId, WindowId)> {
        let root = self.root(layout);
        let selection = self.tree.map.current_selection(root);
        self.find_best_focus_target(selection)
            .or_else(|| (selection != root).then(|| self.find_best_focus_target(root)).flatten())
    }

    pub(super) fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        let selection = self.selection(layout);
        self.window_node(layout, window)
            .is_some_and(|node| node.ancestors(self.map()).any(|ancestor| ancestor == selection))
    }

    pub(super) fn window_in_direction(
        &self,
        layout: LayoutId,
        direction: Direction,
    ) -> Option<WindowId> {
        let map = self.map();
        // Enter along the near edge. Across an axis, a split remembers the path used
        // to leave it, while a stack prefers its presented child.
        let reverse = matches!(direction, Direction::Left | Direction::Up);
        map.find_window(
            self.root(layout),
            &|window| self.window_participates(window),
            &|node| {
                let kind = map.kind(node);
                let preferred =
                    if kind.is_stacked() || kind.orientation() != direction.orientation() {
                        map.local_selection(node)
                    } else {
                        None
                    };
                (preferred, reverse)
            },
        )
        .map(|(_, window)| window)
    }

    pub(super) fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.window_node(layout, wid).is_some()
    }

    pub(super) fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.root(layout).traverse_preorder(self.map()).any(|node| {
            self.map().is_effectively_fullscreen(node)
                && (self.inactive.is_empty() || self.find_best_focus_target(node).is_some())
        })
    }

    pub(super) fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.stacked_selection_container(layout).is_some()
    }

    pub(super) fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
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
        }
    }

    pub(super) fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        match focus {
            Focus::Select(window) => FocusResult::selection({
                let wid = window;

                if self.inactive.contains(&wid) {
                    return FocusResult::selection(false);
                }
                if let Some(node) = self.tree.window.node_for(layout, wid) {
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

    pub(super) fn subtree_participates(
        &self,
        node: NodeId,
        child_participates: impl FnMut(NodeId) -> bool,
    ) -> bool {
        if let Some(window) = self.window_at(node) {
            self.window_participates(window)
        } else {
            node.children(self.map()).any(child_participates)
        }
    }

    fn participating_nodes(&self, root: NodeId) -> HashSet<NodeId> {
        let mut active = HashSet::default();
        if self.inactive.is_empty() {
            return active;
        }
        for node in root.traverse_postorder(self.map()) {
            if self.subtree_participates(node, |child| active.contains(&child)) {
                active.insert(node);
            }
        }
        active
    }

    pub(super) fn find_best_focus_target(&self, node: NodeId) -> Option<(NodeId, WindowId)> {
        self.tree.map.focus_target(node, &|window| self.window_participates(window))
    }

    pub(super) fn root(&self, layout: LayoutId) -> NodeId { self.layout_roots[layout].0 }

    pub(super) fn selection(&self, layout: LayoutId) -> NodeId {
        let root = self.root(layout);
        let selection = self.map().current_selection(root);
        if selection != root
            && !self.inactive.is_empty()
            && self.find_best_focus_target(selection).is_none()
        {
            return self.find_best_focus_target(root).map_or(root, |(node, _)| node);
        }
        selection
    }

    pub(super) fn map(&self) -> &NodeMap { &self.tree.map }

    pub(super) fn local_selection(&self, node: NodeId) -> Option<NodeId> {
        self.tree
            .map
            .local_selection(node)
            .or_else(|| self.tree.map.last_selection(node))
    }

    pub(super) fn layout(&self, node: NodeId) -> LayoutKind { self.tree.map.kind(node) }

    pub(super) fn window_node(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.tree.window.node_for(layout, wid)
    }

    pub(super) fn windows_with_nodes(
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
        std::iter::successors(start, move |&node| node.step(region, map, forward))
            .filter_map(|node| self.window_at(node))
    }

    pub(super) fn contains_any_window(&self, wid: WindowId) -> bool {
        self.tree.window.window_nodes.contains_key(&wid)
    }

    pub(super) fn visible_stack_child(&self, node: NodeId) -> Option<NodeId> {
        self.map()
            .participating_child(node, false, |child| self.first_visible_window(child).is_some())
    }

    pub(super) fn node_is_visible(&self, node: NodeId) -> bool {
        self.window_at(node).is_some_and(|window| self.window_participates(window))
            && node.ancestors_with_parent(self.map()).all(|(child, parent)| {
                parent.is_none_or(|parent| {
                    !self.layout(parent).is_stacked()
                        || self.visible_stack_child(parent) == Some(child)
                })
            })
    }

    pub(super) fn fullscreen_frame(
        &self,
        node: NodeId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> Option<CGRect> {
        node.ancestors(self.map()).find_map(|node| {
            let info = &self.tree.map[node].layout;
            if info.is_fullscreen {
                Some(screen)
            } else if info.is_fullscreen_within_gaps {
                Some(compute_tiling_area(screen, gaps))
            } else {
                None
            }
        })
    }

    pub(super) fn set_layout(&mut self, node: NodeId, kind: LayoutKind) {
        self.tree.map.set_kind(node, kind);
    }
}
