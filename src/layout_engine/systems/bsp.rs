use objc2_core_foundation::{CGRect, CGSize};
use serde::{Deserialize, Serialize};
use slotmap::{Key, KeyData, SlotMap};

use super::{EditResult, Focus, FocusResult, GroupEdit, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::WindowInsertionPoint;
use crate::layout_engine::systems::constraints::{
    AxisConstraints, ConstraintCache, axis_slot, constrain_frame, container_axis_constraints,
    solve_axis_lengths,
};
use crate::layout_engine::systems::{LayoutSystem, WindowLayoutConstraints};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, Orientation, ResizeOrientation};
use crate::model::selection::*;
use crate::model::tree::NodeId;

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
        children: [NodeId; 2],
        #[serde(skip)]
        selected: Option<bool>,
    },
    Leaf {
        members: LeafMembers,
        fullscreen: bool,
        fullscreen_within_gaps: bool,
        preselected: Option<Direction>,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
enum LeafMembers {
    #[default]
    Empty,
    Single(WindowId),
    Stack {
        windows: Vec<WindowId>,
        active: usize,
    },
}

impl LeafMembers {
    fn active(&self) -> Option<WindowId> {
        match self {
            Self::Empty => None,
            Self::Single(window) => Some(*window),
            Self::Stack { windows, active } => Some(windows[*active]),
        }
    }

    fn windows(&self) -> &[WindowId] {
        match self {
            Self::Empty => &[],
            Self::Single(window) => std::slice::from_ref(window),
            Self::Stack { windows, .. } => windows,
        }
    }

    fn windows_mut(&mut self) -> &mut [WindowId] {
        match self {
            Self::Empty => &mut [],
            Self::Single(window) => std::slice::from_mut(window),
            Self::Stack { windows, .. } => windows,
        }
    }

    fn focus(&mut self, window: WindowId) {
        if let Self::Stack { windows, active } = self {
            let index = windows
                .iter()
                .position(|&member| member == window)
                .expect("focused window belongs to its leaf");
            windows[index..].rotate_left(1);
            *active = windows.len() - 1;
        }
    }

    fn remove(&mut self, window: WindowId) {
        if let Self::Stack { windows, active } = self {
            windows.retain(|&member| member != window);
            if let Some(last) = windows.len().checked_sub(1) {
                *active = last;
                return;
            }
        }
        *self = Self::Empty;
    }

    fn append(&mut self, window: WindowId) {
        let mut windows = match std::mem::take(self) {
            Self::Empty => Vec::new(),
            Self::Single(member) => vec![member],
            Self::Stack { windows, .. } => windows,
        };
        windows.push(window);
        *self = Self::from_stack(windows);
    }

    fn prepare_slot_swap(&mut self) {
        if let Self::Stack { windows, active } = self {
            *active = windows.len() - 1;
            if windows.len() == 1 {
                *self = Self::Single(windows[0]);
            }
        }
    }

    fn take_stack(&mut self) -> Option<Vec<WindowId>> {
        match self {
            Self::Stack { windows, .. } => {
                let windows = std::mem::take(windows);
                *self = Self::Empty;
                Some(windows)
            }
            _ => None,
        }
    }

    fn swap_tabs(&mut self, first: WindowId, second: WindowId) {
        let Self::Stack { windows, active } = self else {
            unreachable!("distinct windows in one leaf require a stack");
        };
        windows.retain(|&window| window != first && window != second);
        windows.extend([second, first]);
        *active = windows.len() - 1;
    }

    fn from_stack(windows: Vec<WindowId>) -> Self {
        match windows.len().checked_sub(1) {
            Some(active) => Self::Stack { windows, active },
            None => Self::Empty,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct LayoutState {
    root: NodeId,
}

#[derive(Clone, Deserialize, Debug)]
#[serde(from = "StoredNode")]
struct Node {
    parent: Option<NodeId>,
    kind: NodeKind,
}

#[derive(Deserialize)]
#[serde(rename = "Node")]
struct StoredNode {
    parent: Option<NodeId>,
    selected: Option<bool>,
    kind: NodeKind,
}

impl From<StoredNode> for Node {
    fn from(mut stored: StoredNode) -> Self {
        if let NodeKind::Split { selected, .. } = &mut stored.kind {
            *selected = stored.selected;
        }
        Self {
            parent: stored.parent,
            kind: stored.kind,
        }
    }
}

impl Serialize for Node {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let selected = match self.kind {
            NodeKind::Split { selected, .. } => selected,
            NodeKind::Leaf { .. } => None,
        };
        let mut state = serializer.serialize_struct("Node", 3)?;
        state.serialize_field("parent", &self.parent)?;
        state.serialize_field("selected", &selected)?;
        state.serialize_field("kind", &self.kind)?;
        state.end()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
#[serde(try_from = "StoredBspSystem")]
pub struct BspLayoutSystem {
    #[serde(skip)]
    inactive: HashSet<WindowId>,
    layouts: SlotMap<LayoutId, LayoutState>,
    nodes: SlotMap<NodeId, Node>,
    #[serde(skip)]
    window_insertion_point: WindowInsertionPoint,
    #[serde(skip)]
    single_window_aspect_ratio: Option<f64>,
}

#[derive(Deserialize)]
struct StoredBspSystem {
    layouts: SlotMap<LayoutId, LayoutState>,
    #[serde(default)]
    nodes: SlotMap<NodeId, Node>,
    #[serde(default, deserialize_with = "super::present")]
    tree: Option<LegacyTree>,
    #[serde(default)]
    kind: slotmap::SecondaryMap<NodeId, LegacyKind>,
    #[serde(default)]
    stacks: HashMap<NodeId, Vec<WindowId>>,
}

#[derive(Deserialize)]
struct LegacyTree {
    map: LegacyTopology,
    data: LegacyComponents,
}

#[derive(Deserialize)]
struct LegacyTopology {
    map: Vec<super::SavedSlot<LegacyNode>>,
}

#[derive(Deserialize)]
struct LegacyNode {
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
}

#[derive(Deserialize)]
struct LegacyComponents {
    selection: Selection,
    #[serde(default)]
    kind: slotmap::SecondaryMap<NodeId, LegacyKind>,
}

#[derive(Deserialize)]
enum LegacyKind {
    Split {
        orientation: Orientation,
        ratio: f32,
    },
    Leaf {
        window: Option<WindowId>,
        #[serde(default)]
        stack: Option<Vec<WindowId>>,
        fullscreen: bool,
        fullscreen_within_gaps: bool,
        preselected: Option<Direction>,
    },
}

impl TryFrom<StoredBspSystem> for BspLayoutSystem {
    type Error = String;

    fn try_from(stored: StoredBspSystem) -> Result<Self, String> {
        let mut nodes = if let Some(tree) = stored.tree {
            let mut kinds = if tree.data.kind.is_empty() {
                stored.kind
            } else {
                tree.data.kind
            };
            let mut stacks = stored.stacks;
            let mut slots = Vec::with_capacity(tree.map.map.len());
            for (index, slot) in tree.map.map.into_iter().enumerate() {
                let value = if let Some(node) = slot.value {
                    let id = NodeId::from(KeyData::from_ffi(
                        (u64::from(slot.version) << 32) | index as u64,
                    ));
                    let kind = match kinds.remove(id).ok_or("missing saved BSP node kind")? {
                        LegacyKind::Split { orientation, ratio } => {
                            let first = node.first_child.ok_or("missing first BSP child")?;
                            let second = node
                                .last_child
                                .filter(|&last| last != first)
                                .ok_or("missing second BSP child")?;
                            NodeKind::Split {
                                orientation,
                                ratio,
                                children: [first, second],
                                selected: None,
                            }
                        }
                        LegacyKind::Leaf {
                            window,
                            stack,
                            fullscreen,
                            fullscreen_within_gaps,
                            preselected,
                        } => NodeKind::Leaf {
                            members: if let Some(windows) = stacks.remove(&id).or(stack) {
                                let active = windows
                                    .iter()
                                    .position(|wid| Some(*wid) == window)
                                    .unwrap_or_else(|| windows.len().saturating_sub(1));
                                LeafMembers::Stack { windows, active }
                            } else {
                                window.map_or(LeafMembers::Empty, LeafMembers::Single)
                            },
                            fullscreen,
                            fullscreen_within_gaps,
                            preselected,
                        },
                    };
                    let selected = match (&kind, tree.data.selection.selected_child(id)) {
                        (NodeKind::Split { children, .. }, Some(child)) => {
                            if !children.contains(&child) {
                                return Err("saved BSP selection is not a split child".into());
                            }
                            Some(child == children[1])
                        }
                        _ => None,
                    };
                    Some(Node::from(StoredNode {
                        parent: node.parent,
                        selected,
                        kind,
                    }))
                } else {
                    None
                };
                slots.push(super::SavedSlot { value, version: slot.version });
            }
            let wire = ron::to_string(&slots).map_err(|error| error.to_string())?;
            ron::from_str(&wire).map_err(|error| error.to_string())?
        } else {
            stored.nodes
        };
        let mut pending: Vec<_> = stored.layouts.values().map(|state| (state.root, None)).collect();
        let mut visited = HashSet::default();
        while let Some((id, parent)) = pending.pop() {
            let node = nodes.get_mut(id).ok_or("missing saved BSP node")?;
            if node.parent != parent || !visited.insert(id) {
                return Err("saved BSP nodes must belong to exactly one branch".into());
            }
            match &mut node.kind {
                NodeKind::Split { children, .. } => {
                    pending.extend(children.iter().map(|&child| (child, Some(id))));
                }
                NodeKind::Leaf { members, .. } => {
                    if let LeafMembers::Stack { windows, active } = members {
                        if windows.is_empty() {
                            *members = LeafMembers::Empty;
                        } else if *active >= windows.len() {
                            *active = 0;
                        }
                    }
                }
            }
        }
        // Older arenas can contain abandoned nodes. Keep the live IDs and generations.
        nodes.retain(|id, _| visited.contains(&id));
        Ok(Self {
            layouts: stored.layouts,
            nodes,
            ..Self::default()
        })
    }
}

impl BspLayoutSystem {
    fn kind(&self, node: NodeId) -> &NodeKind { &self.nodes[node].kind }

    fn kind_mut(&mut self, node: NodeId) -> &mut NodeKind { &mut self.nodes[node].kind }

    fn make_node(&mut self, kind: NodeKind) -> NodeId {
        self.nodes.insert(Node { parent: None, kind })
    }

    fn parent(&self, node: NodeId) -> Option<NodeId> { self.nodes[node].parent }

    fn children(&self, node: NodeId) -> Option<[NodeId; 2]> {
        match self.kind(node) {
            NodeKind::Split { children, .. } => Some(*children),
            NodeKind::Leaf { .. } => None,
        }
    }

    fn first_child(&self, node: NodeId) -> Option<NodeId> { Some(self.children(node)?[0]) }

    fn child_iter(&self, node: NodeId) -> impl DoubleEndedIterator<Item = NodeId> {
        self.children(node).into_iter().flatten()
    }

    fn ancestors(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(Some(node), |&node| self.parent(node))
    }

    fn preorder(&self, root: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(Some(root), move |&node| {
            self.first_child(node).or_else(|| {
                self.ancestors(node)
                    .take_while(|&ancestor| ancestor != root)
                    .find_map(|ancestor| {
                        let children = self.children(self.parent(ancestor)?)?;
                        (children[0] == ancestor).then_some(children[1])
                    })
            })
        })
    }

    fn postorder(&self, root: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let leftmost = |node| {
            std::iter::successors(Some(node), |&node| self.first_child(node))
                .last()
                .unwrap()
        };
        std::iter::successors(Some(leftmost(root)), move |&node| {
            if node == root {
                return None;
            }
            let parent = self.parent(node)?;
            let children = self.children(parent)?;
            Some(if children[0] == node {
                leftmost(children[1])
            } else {
                parent
            })
        })
    }

    fn current_selection(&self, mut node: NodeId) -> NodeId {
        while let NodeKind::Split {
            children,
            selected: Some(second),
            ..
        } = self.kind(node)
        {
            node = children[usize::from(*second)];
        }
        node
    }

    fn select(&mut self, mut node: NodeId) {
        if let NodeKind::Split { selected, .. } = self.kind_mut(node) {
            *selected = None;
        }
        while let Some(parent) = self.parent(node) {
            let NodeKind::Split { children, selected, .. } = self.kind_mut(parent) else {
                unreachable!("parent of a BSP node is a split");
            };
            *selected = Some(children[1] == node);
            node = parent;
        }
    }

    fn set_split(&mut self, node: NodeId, orientation: Orientation, children: [NodeId; 2]) {
        self.nodes[node].kind = NodeKind::Split {
            orientation,
            ratio: 0.5,
            children,
            selected: None,
        };
        for child in children {
            self.nodes[child].parent = Some(node);
        }
    }

    fn replace_child(&mut self, old: NodeId, new: NodeId) {
        let parent = self.nodes[old].parent.expect("cannot replace BSP root");
        let entry = &mut self.nodes[parent];
        let NodeKind::Split { children, .. } = &mut entry.kind else {
            unreachable!()
        };
        debug_assert!(children.contains(&old));
        children[usize::from(children[1] == old)] = new;
        self.nodes[new].parent = Some(parent);
    }

    fn clone_node(&mut self, source: NodeId, parent: Option<NodeId>) -> NodeId {
        let clone = self.nodes.insert(self.nodes[source].clone());
        self.nodes[clone].parent = parent;
        if let Some([first, second]) = self.children(source) {
            let new = [
                self.clone_node(first, Some(clone)),
                self.clone_node(second, Some(clone)),
            ];
            let node = &mut self.nodes[clone];
            if let NodeKind::Split { children, .. } = &mut node.kind {
                *children = new;
            }
        }
        clone
    }

    fn remove_subtree(&mut self, root: NodeId) {
        if let Some(children) = self.children(root) {
            for child in children {
                self.remove_subtree(child);
            }
        }
        self.nodes.remove(root);
    }

    fn members(&self, node: NodeId) -> Option<&LeafMembers> {
        let NodeKind::Leaf { members, .. } = self.kind(node) else {
            return None;
        };
        Some(members)
    }

    fn leaf_members_mut(&mut self, leaf: NodeId) -> &mut LeafMembers {
        let NodeKind::Leaf { members, .. } = self.kind_mut(leaf) else {
            unreachable!("expected a BSP leaf");
        };
        members
    }

    fn stack(&self, node: NodeId) -> Option<&[WindowId]> {
        match self.members(node)? {
            LeafMembers::Stack { windows, .. } => Some(windows),
            _ => None,
        }
    }

    fn participating_window(&self, node: NodeId) -> Option<WindowId> {
        self.participating_member(self.members(node)?)
    }

    fn participating_member(&self, members: &LeafMembers) -> Option<WindowId> {
        members
            .active()
            .filter(|w| self.window_participates(*w))
            .or_else(|| members.windows().iter().copied().find(|w| self.window_participates(*w)))
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
        let backwards = matches!(direction, Direction::Left | Direction::Up);
        let axis = direction.orientation();
        let mut anchor = 0.5;
        let mut current = from_leaf;
        while let Some(parent) = self.parent(current) {
            let NodeKind::Split {
                orientation, ratio, children, ..
            } = self.nodes[parent].kind
            else {
                unreachable!("a BSP parent is a split")
            };
            let second = children[1] == current;
            if orientation == axis {
                if second == backwards {
                    let mut target = children[usize::from(!second)];
                    while let NodeKind::Split {
                        orientation, ratio, children, ..
                    } = self.nodes[target].kind
                    {
                        let ratio = f64::from(ratio);
                        let second = if orientation == axis {
                            backwards
                        } else if anchor <= ratio {
                            anchor /= ratio;
                            false
                        } else {
                            anchor = (anchor - ratio) / (1.0 - ratio);
                            true
                        };
                        target = children[usize::from(second)];
                    }
                    return Some(target);
                }
            } else {
                let ratio = f64::from(ratio);
                anchor = if second {
                    ratio + anchor * (1.0 - ratio)
                } else {
                    anchor * ratio
                };
            }
            current = parent;
        }
        None
    }

    fn window_in_direction_from(&self, node: NodeId, direction: Direction) -> Option<WindowId> {
        let Some(mut children) = self.children(node) else {
            return self.participating_window(node);
        };
        if matches!(direction, Direction::Left | Direction::Up) {
            children.swap(0, 1);
        }
        children
            .into_iter()
            .find_map(|child| self.window_in_direction_from(child, direction))
    }

    fn smart_insert_window(&mut self, layout: LayoutId, window: WindowId) -> bool {
        let Some(selection) = self.selection_of_layout(layout) else {
            return false;
        };
        let leaf = self.descend_to_leaf(selection);
        let &NodeKind::Leaf {
            preselected: Some(direction), ..
        } = self.kind(leaf)
        else {
            return false;
        };
        self.split_leaf(
            leaf,
            direction.orientation(),
            Some(window),
            matches!(direction, Direction::Left | Direction::Up),
        );
        true
    }

    fn split_leaf(
        &mut self,
        leaf: NodeId,
        orientation: Orientation,
        new_window: Option<WindowId>,
        new_first: bool,
    ) {
        let members = std::mem::take(self.leaf_members_mut(leaf));
        let existing = self.make_leaf_with_members(members);
        let new = self.make_leaf(new_window);
        let children = if new_first {
            [new, existing]
        } else {
            [existing, new]
        };
        self.set_split(leaf, orientation, children);
        self.select(new);
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
        fn find(arena: &SlotMap<NodeId, Node>, node: NodeId, window: WindowId) -> Option<NodeId> {
            match &arena[node].kind {
                NodeKind::Split { children: [first, second], .. } => {
                    find(arena, *first, window).or_else(|| find(arena, *second, window))
                }
                NodeKind::Leaf { members, .. } => {
                    members.windows().contains(&window).then_some(node)
                }
            }
        }
        find(&self.nodes, self.layouts.get(layout)?.root, wid)
    }

    fn make_leaf(&mut self, window: Option<WindowId>) -> NodeId {
        self.make_leaf_with_members(window.map_or(LeafMembers::Empty, LeafMembers::Single))
    }

    fn make_leaf_with_members(&mut self, members: LeafMembers) -> NodeId {
        self.make_node(NodeKind::Leaf {
            members,
            fullscreen: false,
            fullscreen_within_gaps: false,
            preselected: None,
        })
    }

    fn descend_to_leaf(&self, mut node: NodeId) -> NodeId {
        if !self.inactive.is_empty()
            && let Some(active) = self
                .preorder(node)
                .find(|&candidate| self.participating_window(candidate).is_some())
        {
            return active;
        }
        while let Some(child) = self.first_child(node) {
            node = child;
        }
        node
    }

    fn orientation_for_leaf(&self, node: NodeId) -> Orientation {
        if self.ancestors(node).count() % 2 == 1 {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }
    }

    fn windows_under(&self, root: NodeId) -> impl Iterator<Item = WindowId> + '_ {
        self.preorder(root)
            .filter_map(|node| self.members(node))
            .flat_map(|members| members.windows().iter().copied())
    }

    fn has_fullscreen_in_subtree(&self, node: NodeId) -> bool {
        match self.kind(node) {
            NodeKind::Leaf {
                fullscreen,
                fullscreen_within_gaps,
                ..
            } => {
                self.participating_window(node).is_some()
                    && (*fullscreen || *fullscreen_within_gaps)
            }
            NodeKind::Split { .. } => {
                self.child_iter(node).any(|child| self.has_fullscreen_in_subtree(child))
            }
        }
    }

    // Return the number of tiles sharing the parent's axis; perpendicular splits count as one.
    fn rebalance_node(&mut self, node: NodeId, parent_axis: Orientation) -> usize {
        let NodeKind::Split {
            orientation,
            children: [first, second],
            ..
        } = self.nodes[node].kind
        else {
            return 1;
        };
        let first_span = self.rebalance_node(first, orientation);
        let second_span = self.rebalance_node(second, orientation);
        let span = first_span + second_span;
        let NodeKind::Split { ratio, .. } = self.kind_mut(node) else {
            unreachable!("rebalancing children does not replace their parent");
        };
        *ratio = (first_span as f32 / span as f32).clamp(0.05, 0.95);
        if orientation == parent_axis { span } else { 1 }
    }

    // Remove one child and promote the other into the parent's stable slot.
    fn detach_child(&mut self, node: NodeId) {
        let parent = self.nodes[node].parent.expect("cannot detach BSP root");
        let children = self.children(parent).unwrap();
        debug_assert!(children.contains(&node));
        let sibling = children[usize::from(children[0] == node)];
        let sibling = self.nodes.remove(sibling).unwrap();
        self.nodes[node].parent = None;
        let parent_node = &mut self.nodes[parent];
        parent_node.kind = sibling.kind;
        // The former parent's selection pointed to one of the two removed child slots.
        if let NodeKind::Split { selected, .. } = &mut parent_node.kind {
            *selected = None;
        }
        if let Some(children) = self.children(parent) {
            for child in children {
                self.nodes[child].parent = Some(parent);
            }
        }
    }

    fn selection_of_layout(&self, layout: LayoutId) -> Option<NodeId> {
        let root = self.layouts.get(layout)?.root;
        let selected = self.current_selection(root);
        if self.inactive.is_empty() || self.participating_window(selected).is_some() {
            return Some(selected);
        }
        let first_active =
            self.preorder(root).find(|&node| self.participating_window(node).is_some())?;
        if self.members(selected).and_then(LeafMembers::active).is_some() {
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
            .unwrap_or_else(|| self.current_selection(state.root));
        let leaf = self.descend_to_leaf(sel);
        if leaf != sel {
            self.select(leaf);
        }
        match self.kind_mut(leaf) {
            NodeKind::Leaf {
                members,
                fullscreen,
                fullscreen_within_gaps,
                ..
            } if matches!(members, LeafMembers::Empty) => {
                (*members, *fullscreen, *fullscreen_within_gaps) =
                    (LeafMembers::Single(wid), false, false);
            }
            NodeKind::Leaf { .. } => {
                let orientation = self.orientation_for_leaf(leaf);
                self.split_leaf(leaf, orientation, Some(wid), false);
            }
            NodeKind::Split { .. } => unreachable!("descent ends at a leaf"),
        }
    }

    /// Remove an occurrence and report progress so callers can clear legacy duplicates.
    fn remove_window_internal(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        let Some(node) = self.node_for_window_in_layout(layout, wid) else {
            return false;
        };
        self.remove_member(layout, node, wid);
        true
    }

    fn remove_member(&mut self, layout: LayoutId, node: NodeId, wid: WindowId) {
        let members = self.leaf_members_mut(node);
        members.remove(wid);
        if members.active().is_some() {
            self.select(node);
            return;
        }
        if let NodeKind::Leaf {
            fullscreen,
            fullscreen_within_gaps,
            ..
        } = self.kind_mut(node)
        {
            *fullscreen = false;
            *fullscreen_within_gaps = false;
        }
        if self.nodes[node].parent.is_some() {
            self.detach_child(node);
            self.nodes.remove(node);
        }
        let selection = self.current_selection(self.layouts[layout].root);
        let leaf = self.descend_to_leaf(selection);
        self.select(leaf);
    }

    fn prepare_geometry(
        &self,
        root: NodeId,
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
    ) -> ConstraintCache {
        let mut prepared = ConstraintCache::default();
        for node in self.postorder(root) {
            let axes = match self.kind(node) {
                NodeKind::Leaf { members, .. } => {
                    let active = self.participating_member(members);
                    if active.is_none()
                        && !(self.inactive.is_empty() && matches!(members, LeafMembers::Empty))
                    {
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
                NodeKind::Split { orientation, children, .. } => {
                    let children = children.map(|child| prepared.get(&child));
                    if children == [None; 2] {
                        continue;
                    }
                    let kind = LayoutKind::from(*orientation);
                    [false, true].map(|horizontal| {
                        let mut summary = container_axis_constraints(
                            children.into_iter().flatten().map(|axes| axes[horizontal as usize]),
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
            };
            prepared.insert(node, axes);
        }
        prepared
    }

    fn selection_window(&self, state: &LayoutState) -> Option<WindowId> {
        let sel = self.current_selection(state.root);
        self.participating_window(sel)
    }
}

struct BspProjection<'a> {
    system: &'a BspLayoutSystem,
    prepared: &'a ConstraintCache,
    screen: CGRect,
    tiling: CGRect,
    constraints: &'a HashMap<WindowId, WindowLayoutConstraints>,
    gaps: &'a crate::common::config::GapSettings,
}

impl BspProjection<'_> {
    fn apply(&self, node: NodeId, slot: CGRect, out: &mut Vec<(WindowId, CGRect)>) {
        let Some(axes) = self.prepared.get(&node) else { return };
        match &self.system.nodes[node].kind {
            NodeKind::Leaf {
                members,
                fullscreen,
                fullscreen_within_gaps,
                ..
            } => {
                let Some(window) = self.system.participating_member(members) else {
                    return;
                };
                let frame = if *fullscreen {
                    self.screen
                } else if *fullscreen_within_gaps {
                    self.tiling
                } else if self.constraints.contains_key(&window) {
                    constrain_frame(slot, |horizontal| axes[horizontal as usize])
                } else {
                    slot
                };
                out.push((window, frame));
            }
            NodeKind::Split {
                orientation,
                ratio,
                children: [first, second],
                ..
            } => {
                let (Some(first_axes), Some(second_axes)) =
                    (self.prepared.get(first), self.prepared.get(second))
                else {
                    self.apply(*first, slot, out);
                    self.apply(*second, slot, out);
                    return;
                };
                let horizontal = *orientation == Orientation::Horizontal;
                let (origin, total, gap) = if horizontal {
                    (slot.origin.x, slot.size.width, self.gaps.inner.horizontal)
                } else {
                    (slot.origin.y, slot.size.height, self.gaps.inner.vertical)
                };
                let ratio = f64::from(*ratio);
                let items =
                    [(first_axes, ratio), (second_axes, 1.0 - ratio)].map(|(axes, weight)| {
                        let (min, fixed, max, can_grow) = axes[horizontal as usize];
                        AxisConstraints {
                            min,
                            fixed,
                            max,
                            weight,
                            can_grow,
                        }
                    });
                let lengths = solve_axis_lengths(items, (total - gap).max(0.0));
                self.apply(*first, axis_slot(slot, horizontal, origin, lengths[0]), out);
                self.apply(
                    *second,
                    axis_slot(slot, horizontal, origin + (lengths[0] + gap), lengths[1]),
                    out,
                );
            }
        }
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
    fn moving_into_a_placeholder_promotes_the_old_sibling() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        system.edit(layout, LayoutEdit::Split(LayoutKind::Horizontal)).discard();
        assert!(system.focus(layout, Focus::Select(w(3))).handled);

        assert!(
            system
                .edit(layout, LayoutEdit::Move {
                    direction: Direction::Left,
                    has_display_neighbor: false,
                })
                .changed()
        );

        let frames =
            calculate_frames(&system, layout, CGSize::new(1000.0, 1000.0), &Default::default());
        assert_eq!(system.selected_window(layout), Some(w(3)));
        assert_eq!(frames.len(), 3);
        for (window, x, width) in [
            (w(1), 0.0, 250.0),
            (w(3), 250.0, 250.0),
            (w(2), 500.0, 500.0),
        ] {
            assert_eq!(
                frames[&window],
                CGRect::new(CGPoint::new(x, 0.0), CGSize::new(width, 1000.0))
            );
        }
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
        // Snapshot of the former separate topology and stack maps.
        let expected = system.container_tree(original);
        let saved = include_str!("bsp_legacy.ron");
        system = ron::from_str(saved).unwrap();
        assert_eq!(system.container_tree(original), expected);
        assert!(!system.edit(original, LayoutEdit::Swap(w(2), w(2))).changed());
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
    fn closing_the_last_fullscreen_window_clears_the_tile() {
        let (mut system, layout) = tiled(&[1]);
        system.toggle_fullscreen(layout, false);
        assert!(system.container_tree(layout).is_fullscreen);
        system.remove_window(w(1));
        assert!(!system.container_tree(layout).is_fullscreen);
        system.add_window_after_selection(layout, w(2));
        assert!(!system.container_tree(layout).is_fullscreen);
    }

    #[test]
    fn join_adds_the_selected_window_to_the_neighboring_stack() {
        let (mut system, layout) = tiled(&[1, 2, 3]);
        assert!(system.apply_target_drop(
            layout,
            w(2),
            w(1),
            crate::layout_engine::WindowDropAction::Stack
        ));
        assert!(system.focus(layout, Focus::Select(w(3))).handled);
        system.join_selection_with_direction(layout, Direction::Left);
        assert_eq!(system.all_windows_in_layout(layout), vec![w(1), w(2), w(3)]);
        assert_eq!(system.visible_windows_in_layout(layout), vec![w(3)]);
    }
}

impl LayoutSystem for BspLayoutSystem {
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.layouts
            .get(layout)
            .is_some_and(|state| self.windows_under(state.root).any(|window| window.pid == pid))
    }

    fn window_participates(&self, window: WindowId) -> bool { !self.inactive.contains(&window) }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        super::update_participation(&mut self.inactive, window, participates)
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
        let root = self.clone_node(state.root, None);
        self.layouts.insert(LayoutState { root })
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        if let Some(state) = self.layouts.remove(layout) {
            self.remove_subtree(state.root);
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
            BspProjection {
                system: self,
                prepared: &prepared,
                screen,
                tiling: rect,
                constraints,
                gaps,
            }
            .apply(state.root, rect, &mut out);
            if let (Some(ratio), [(_, frame)]) =
                (self.single_window_aspect_ratio, out.as_mut_slice())
                && matches!(self.kind(state.root), NodeKind::Leaf {
                    fullscreen: false,
                    fullscreen_within_gaps: false,
                    ..
                })
                && self.stack(state.root).is_none()
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
                .unwrap_or_default()
                .to_vec(),
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
                self.leaf_members_mut(node).focus(wid);
                self.select(node);
                true
            }),
            Focus::Ascend => FocusResult::selection({
                if let Some(sel) = self.selection_of_layout(layout) {
                    let parent_opt = self.parent(sel);
                    if let Some(parent) = parent_opt {
                        let new_sel = self.descend_to_leaf(parent);
                        self.select(new_sel);
                        return FocusResult::selection(true);
                    }
                }
                false
            }),
            Focus::Descend => FocusResult::selection({
                if let Some(sel) = self.selection_of_layout(layout) {
                    let new_sel = self.descend_to_leaf(sel);
                    if new_sel != sel {
                        self.select(new_sel);
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
                super::replace_participation(&mut self.inactive, from, to);
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
                }
                for node in self.nodes.values_mut() {
                    if let NodeKind::Leaf { members, .. } = &mut node.kind {
                        for member in
                            members.windows_mut().iter_mut().filter(|member| **member == from)
                        {
                            *member = to;
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
                    if let NodeKind::Leaf {
                        fullscreen,
                        fullscreen_within_gaps,
                        ..
                    } = self.kind_mut(node)
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
                    while let Some(parent) = self.parent(current) {
                        let NodeKind::Split {
                            orientation, ratio, children, ..
                        } = self.kind_mut(parent)
                        else {
                            unreachable!("parent of a BSP node is a split");
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
                        let is_first = current == children[0];
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
            } => EditResult::from(self.move_selection(layout, direction)),
            LayoutEdit::Split(kind) => {
                if kind.is_stacked() {
                    return EditResult::Unsupported;
                }
                let Some(selection) = self.selection_of_layout(layout) else {
                    return EditResult::Unchanged;
                };
                let leaf = self.descend_to_leaf(selection);
                self.split_leaf(leaf, kind.orientation(), None, false);
                EditResult::Changed
            }
            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::Swap(a, b) => EditResult::from(self.swap_windows(layout, a, b)),
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(self.apply_target_drop(layout, source, target, action))
            }
            LayoutEdit::Group(GroupEdit::Join(direction)) => {
                EditResult::from(self.join_selection_with_direction(layout, direction))
            }
            LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction)) => {
                let is_joined = self
                    .selection_of_layout(layout)
                    .map(|selection| self.descend_to_leaf(selection))
                    .and_then(|leaf| self.stack(leaf))
                    .is_some_and(|members| members.len() > 1);

                if is_joined {
                    EditResult::from(self.unjoin_selection(layout))
                } else {
                    EditResult::from(self.join_selection_with_direction(layout, direction))
                }
            }
            LayoutEdit::Group(GroupEdit::Unjoin) => EditResult::from(self.unjoin_selection(layout)),
            LayoutEdit::Resize { amount, orientation } => {
                let sel_snapshot = self.selection_of_layout(layout);
                let Some(mut node) = sel_snapshot else {
                    return EditResult::Unchanged;
                };

                while let Some(parent) = self.parent(node) {
                    if let NodeKind::Split {
                        orientation: split_orientation,
                        ratio,
                        children,
                        ..
                    } = self.kind_mut(parent)
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
                        let delta = (amount as f32) * if node == children[0] { 0.5 } else { -0.5 };
                        let next = (*ratio + delta).clamp(0.05, 0.95);
                        return EditResult::from(std::mem::replace(ratio, next) != next);
                    }
                    node = parent;
                }
                EditResult::Unchanged
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
                let split = self.ancestors(start).find(|&node| self.children(node).is_some());
                if let Some(node) = split
                    && let NodeKind::Split { orientation, .. } = self.kind_mut(node)
                {
                    *orientation = match *orientation {
                        Orientation::Horizontal => Orientation::Vertical,
                        Orientation::Vertical => Orientation::Horizontal,
                    };
                    return EditResult::Changed;
                }
                EditResult::Unchanged
            }
            _ => EditResult::Unsupported,
        }
    }
}

impl BspLayoutSystem {
    pub(in crate::layout_engine) fn can_move_window(
        &self,
        layout: LayoutId,
        window: WindowId,
        direction: Direction,
    ) -> bool {
        self.window_participates(window)
            && self
                .node_for_window_in_layout(layout, window)
                .is_some_and(|node| self.find_neighbor_leaf(node, direction).is_some())
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(sel) = self.selection_of_layout(layout) else {
            return false;
        };
        let moving = self.descend_to_leaf(sel);
        if self.members(moving).and_then(LeafMembers::active).is_none() {
            return false;
        }
        let Some(target) = self.find_neighbor_leaf(moving, direction) else {
            return false;
        };
        let Some(old_parent) = self.parent(moving) else {
            return false;
        };

        let moving_first = matches!(direction, Direction::Left | Direction::Up);
        if self.nodes[target].parent == Some(old_parent) {
            let NodeKind::Split { children, .. } = &mut self.nodes[old_parent].kind else {
                unreachable!()
            };
            if (children[0] == moving) != moving_first {
                children.swap(0, 1);
            }
        } else {
            self.detach_child(moving);
            if matches!(self.members(target), Some(LeafMembers::Empty)) {
                self.replace_child(target, moving);
                self.nodes.remove(target);
            } else {
                let split = self.make_leaf(None);
                self.replace_child(target, split);
                self.set_split(
                    split,
                    direction.orientation(),
                    if moving_first {
                        [moving, target]
                    } else {
                        [target, moving]
                    },
                );
            }
        }
        self.select(moving);
        true
    }

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
            .flat_map(|state| self.preorder(state.root))
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
        self.select(next_leaf);
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
                let mut leaf = self.layouts[layout].root;
                while let Some([_, right]) = self.children(leaf) {
                    leaf = right;
                }
                self.select(leaf);
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
        if a == b || !self.window_participates(a) || !self.window_participates(b) {
            return false;
        }
        let Some(node_a) = self.node_for_window_in_layout(layout, a) else {
            return false;
        };
        let Some(node_b) = self.node_for_window_in_layout(layout, b) else {
            return false;
        };
        if node_a == node_b {
            self.leaf_members_mut(node_a).swap_tabs(a, b);
            self.select(node_a);
            return true;
        }

        let [first, second] =
            self.nodes.get_disjoint_mut([node_a, node_b]).expect("distinct live BSP nodes");
        let (NodeKind::Leaf { members: first, .. }, NodeKind::Leaf { members: second, .. }) =
            (&mut first.kind, &mut second.kind)
        else {
            unreachable!("window lookup returns leaves");
        };
        // Occupancy moves while fullscreen stays attached to the slot.
        first.prepare_slot_swap();
        second.prepare_slot_swap();
        std::mem::swap(first, second);

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
        use crate::layout_engine::WindowDropAction::*;
        match action {
            Swap => return self.swap_windows(layout, source, target),
            Move(_) | ConsumeOrExpel(_) => return false,
            Stack | Insert(_) => {}
        }

        while self.remove_window_internal(layout, source) {}
        // Removal can promote the target into a different arena slot.
        let target_node = self
            .node_for_window_in_layout(layout, target)
            .expect("removing a different window preserves the drop target");
        match action {
            crate::layout_engine::WindowDropAction::Stack => {
                self.leaf_members_mut(target_node).append(source);
                self.select(target_node);
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
        let NodeKind::Leaf {
            members,
            fullscreen,
            fullscreen_within_gaps,
            ..
        } = self.kind_mut(leaf)
        else {
            return Vec::new();
        };
        if members.active().is_none() {
            return Vec::new();
        }
        super::toggle_fullscreen_mode(fullscreen, fullscreen_within_gaps, within_gaps);
        active.into_iter().collect()
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some(sel) = self.selection_of_layout(layout) else {
            return false;
        };
        let sel_leaf = self.descend_to_leaf(sel);

        let Some(neighbor) = self.find_neighbor_leaf(sel_leaf, direction) else {
            return false;
        };

        if self.stack(neighbor).is_some()
            && self.stack(sel_leaf).is_none()
            && let Some(source) = self.participating_window(sel_leaf)
            && let Some(target) = self.participating_window(neighbor)
        {
            return self.apply_target_drop(
                layout,
                source,
                target,
                crate::layout_engine::WindowDropAction::Stack,
            );
        }

        let mut current = sel_leaf;
        while let Some(parent) = self.parent(current) {
            if self.parent(neighbor) == Some(parent) {
                if self.parent(parent).is_some() {
                    let mut windows: Vec<_> = self.windows_under(parent).collect();

                    // The last member is the visible one, so keep the selected window there.
                    let selected = self.selected_window(layout);

                    if let Some(index) = windows.iter().position(|window| Some(*window) == selected)
                    {
                        windows[index..].rotate_left(1);
                    }

                    if !windows.is_empty() {
                        let new_leaf =
                            self.make_leaf_with_members(LeafMembers::from_stack(windows));
                        self.replace_child(parent, new_leaf);
                        self.remove_subtree(parent);

                        self.select(new_leaf);
                        return true;
                    }
                }
                break;
            }
            current = parent;
        }
        false
    }

    fn unjoin_selection(&mut self, layout: LayoutId) -> bool {
        let Some(sel) = self.selection_of_layout(layout) else {
            return false;
        };

        let leaf = self.descend_to_leaf(sel);
        let NodeKind::Leaf {
            members,
            fullscreen,
            fullscreen_within_gaps,
            ..
        } = self.kind_mut(leaf)
        else {
            unreachable!("descent ends at a BSP leaf");
        };
        let Some(windows) = members.take_stack() else {
            return false;
        };
        let flags = (*fullscreen, *fullscreen_within_gaps);

        self.select(leaf);

        for member in windows {
            self.insert_window_at_selection(layout, member);
        }

        // Insertion rebuilds leaves without fullscreen, so hand it back to the visible member.
        if let Some(visible) = self.selection_of_layout(layout)
            && let NodeKind::Leaf {
                fullscreen,
                fullscreen_within_gaps,
                ..
            } = self.kind_mut(visible)
        {
            (*fullscreen, *fullscreen_within_gaps) = flags;
        }
        true
    }
}

impl BspLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        fn write_node(this: &BspLayoutSystem, node: NodeId, out: &mut String, indent: usize) {
            for _ in 0..indent {
                out.push_str("  ");
            }
            match this.kind(node) {
                NodeKind::Leaf { members, .. } => {
                    out.push_str(&format!("Leaf {:?}\n", members.active()));
                }
                NodeKind::Split { orientation, ratio, .. } => {
                    out.push_str(&format!("Split {:?} {:.2}\n", orientation, ratio));
                    let mut it = this.child_iter(node);
                    if let Some(first) = it.next() {
                        write_node(this, first, out, indent + 1);
                    }
                    if let Some(second) = it.next() {
                        write_node(this, second, out, indent + 1);
                    }
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

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        fn snapshot(
            system: &BspLayoutSystem,
            node: NodeId,
            selected: NodeId,
        ) -> rift_protocol::ContainerTreeNode {
            let weight = system.parent(node).and_then(|parent| {
                let NodeKind::Split { ratio, .. } = system.kind(parent) else {
                    return None;
                };
                let first = system.first_child(parent);
                Some(if first == Some(node) {
                    f64::from(*ratio)
                } else {
                    1.0 - f64::from(*ratio)
                })
            });

            use rift_protocol::{ContainerNodeType, ContainerTreeNode, LayoutKind};
            let mut result = ContainerTreeNode {
                node_id: node.data().as_ffi(),
                is_selected: node == selected,
                weight,
                ..Default::default()
            };
            match system.kind(node) {
                NodeKind::Split { orientation, children, .. } => {
                    result.layout_kind = Some(LayoutKind::from(*orientation));
                    result.children =
                        children.iter().map(|&child| snapshot(system, child, selected)).collect();
                }
                NodeKind::Leaf {
                    members,
                    fullscreen,
                    fullscreen_within_gaps,
                    preselected,
                } => {
                    let window = members.active();
                    result.is_fullscreen = *fullscreen;
                    result.is_fullscreen_within_gaps = *fullscreen_within_gaps;
                    result.pending_split = preselected.map(Into::into);
                    if let LeafMembers::Stack { windows, .. } = members {
                        result.layout_kind = Some(LayoutKind::HorizontalStack);
                        result.children = windows
                            .iter()
                            .map(|&member| ContainerTreeNode {
                                node_id: (u64::from(member.pid as u32) << 32)
                                    | u64::from(member.idx.get()),
                                node_type: ContainerNodeType::Window,
                                window_id: Some(member.into()),
                                is_selected: result.is_selected && window == Some(member),
                                is_fullscreen: *fullscreen,
                                is_fullscreen_within_gaps: *fullscreen_within_gaps,
                                ..Default::default()
                            })
                            .collect();
                    } else if let Some(window) = window {
                        result.node_type = ContainerNodeType::Window;
                        result.window_id = Some(window.into());
                    } else {
                        result.node_type = ContainerNodeType::Placeholder;
                    }
                }
            }
            result
        }

        let Some(state) = self.layouts.get(layout) else {
            return Default::default();
        };
        let selected = self.current_selection(state.root);
        snapshot(self, state.root, selected)
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        let root = self.layouts.get(layout)?.root;
        let node = self.node_for_window_in_layout(layout, window)?;
        let mut current = node;
        let mut slot = Vec::new();
        while current != root {
            let parent = self.nodes[current].parent?;
            slot.push(usize::from(self.children(parent)?[1] == current));
            current = parent;
        }
        slot.reverse();
        if let Some(members) = self.stack(node) {
            slot.push(members.iter().position(|&member| member == window)?);
        }
        Some(slot)
    }
}
