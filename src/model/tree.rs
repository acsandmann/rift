use std::ops::{Index, IndexMut};

use serde::{Deserialize, Serialize};
use slotmap::SlotMap;

use crate::actor::app::{WindowId, pid_t};
use crate::layout_engine::{LayoutId, LayoutKind};
use crate::model::selection::SelectionInfo;

/// Owns ordered tiles and an index of windows in each saved arrangement.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Tree {
    pub map: NodeMap,
    pub(crate) window: WindowIndex,
}

#[derive(Clone, Copy)]
pub(crate) enum Placement {
    Back(NodeId),
    Before(NodeId),
    After(NodeId),
}

impl Placement {
    fn parent(self, map: &NodeMap) -> Option<NodeId> {
        match self {
            Self::Back(parent) => map.contains(parent).then_some(parent),
            Self::Before(sibling) | Self::After(sibling) => sibling.parent(map),
        }
        .filter(|&parent| map.window(parent).is_none())
    }

    fn link(self, node: NodeId, parent: NodeId, map: &mut NodeMap) {
        let (prev, next) = match self {
            Self::Back(_) => (parent.last_child(map), None),
            Self::Before(sibling) => (sibling.prev_sibling(map), Some(sibling)),
            Self::After(sibling) => (Some(sibling), sibling.next_sibling(map)),
        };
        map.link_child(node, parent, prev, next);
    }
}

impl Tree {
    pub(crate) fn create_root(&mut self) -> NodeId { self.map.map.insert(Node::default()) }

    pub(crate) fn insert(&mut self, placement: Placement) -> NodeId {
        let parent = placement.parent(&self.map).expect("invalid tile insertion");
        assert!(!self.map[parent].is_region_pair());
        let node = self.create_root();
        self.map[node].layout.size = 1.0;
        placement.link(node, parent, &mut self.map);
        node
    }

    pub(crate) fn move_node(&mut self, node: NodeId, placement: Placement) -> bool {
        let Some(parent) = placement.parent(&self.map) else {
            return false;
        };
        if !self.map.contains(node)
            || matches!(placement, Placement::Before(anchor) | Placement::After(anchor) if anchor == node)
        {
            return false;
        }
        let old_parent = node.parent(&self.map);
        let changed_parent = old_parent != Some(parent);
        if self.map[parent].is_region_pair()
            || old_parent.is_some_and(|old| self.map[old].is_region_pair())
            || self.map[node].is_region_pair()
            || (changed_parent && parent.ancestors(&self.map).any(|ancestor| ancestor == node))
        {
            return false;
        }
        if !changed_parent
            && match placement {
                Placement::Back(_) => node.next_sibling(&self.map).is_none(),
                Placement::Before(anchor) => node.next_sibling(&self.map) == Some(anchor),
                Placement::After(anchor) => node.prev_sibling(&self.map) == Some(anchor),
            }
        {
            return false;
        }
        if let Some(old) = old_parent.filter(|_| changed_parent) {
            self.detach(node, old);
        } else {
            self.map.unlink(node);
        }
        placement.link(node, parent, &mut self.map);
        if changed_parent {
            self.map[node].layout.size = 1.0;
        }
        if let Some(old) = old_parent.filter(|_| changed_parent) {
            self.prune_container(old);
        }
        true
    }

    pub(crate) fn remove(&mut self, node: NodeId) {
        if let Some(parent) = node.parent(&self.map) {
            if self.map[parent].is_region_pair() {
                return;
            }
            self.detach(node, parent);
            self.prune_container(parent);
        }
        self.delete_subtree(node);
    }

    fn detach(&mut self, node: NodeId, parent: NodeId) {
        self.map.detach_selection(node);
        self.map.unlink(node);
        if self.map.window(node).is_none() {
            return;
        }

        if !self.map.normalize_weights(parent) {
            let mut child = parent.first_child(&self.map);
            while let Some(node) = child {
                self.map[node].layout.size = 1.0;
                child = node.next_sibling(&self.map);
            }
        }
    }

    fn prune_container(&mut self, mut node: NodeId) {
        while let Some(parent) = node.parent(&self.map) {
            if self.map[parent].is_region_pair() {
                break;
            }
            if let Some(child) = node.first_child(&self.map) {
                if child.next_sibling(&self.map).is_some() {
                    break;
                }
                self.map.unlink(child);
                Placement::After(node).link(child, parent, &mut self.map);
                self.map[child].layout.size = self.map[node].layout.size;
            }
            self.map.detach_selection(node);
            self.map.unlink(node);
            self.map.map.remove(node);
            node = parent;
        }
    }

    fn delete_subtree(&mut self, root: NodeId) {
        let Some(node) = self.map.map.remove(root) else { return };
        if let Some(window) = node.window() {
            self.window.remove_node(window, root);
        }
        let mut child = node.children().and_then(|children| children.first);
        while let Some(node) = child {
            child = node.next_sibling(&self.map);
            self.delete_subtree(node);
        }
    }

    pub(crate) fn clone_branch(&mut self, source: NodeId, layout: LayoutId) -> NodeId {
        let root = self.copy_tile(source, layout);
        let mut stack = vec![(source.first_child(&self.map), source, root)];
        while let Some((next, original_parent, new_parent)) = stack.last_mut() {
            let Some(old) = *next else {
                stack.pop();
                continue;
            };
            *next = old.next_sibling(&self.map);
            let new = self.copy_tile(old, layout);
            Placement::Back(*new_parent).link(new, *new_parent, &mut self.map);
            self.map.remap_child_selection(*original_parent, *new_parent, old, new);
            if let Some(child) = old.first_child(&self.map) {
                stack.push((Some(child), old, new));
            }
        }
        if let Contents::RegionPair { primary_first, .. } = self.map[source].contents {
            let first = root.first_child(&self.map).unwrap();
            let last = root.last_child(&self.map).unwrap();
            let (primary, secondary) = if primary_first {
                (first, last)
            } else {
                (last, first)
            };
            self.map.set_region_pair(root, primary, secondary);
        }
        root
    }

    fn copy_tile(&mut self, source: NodeId, layout: LayoutId) -> NodeId {
        let node = self.create_root();
        self.map[node].layout = self.map[source].layout;
        if let Some(window) = self.map.window(source) {
            self.window.set_window(&mut self.map, layout, node, window);
        }
        node
    }
}

/// Map that holds the structure of the tree.
///
/// Multiple trees can be contained within a map. This also makes it easier
/// to move branches between trees.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NodeMap {
    map: SlotMap<NodeId, Node>,
}

impl NodeMap {
    pub(crate) fn set_region_pair(&mut self, root: NodeId, primary: NodeId, secondary: NodeId) {
        let Contents::Container(children) = self[root].contents else {
            panic!("expected an arrangement container");
        };
        let (Some(first), Some(last)) = (children.first, children.last) else {
            panic!("expected two region containers");
        };
        assert!(root.parent(self).is_none() && first.next_sibling(self) == Some(last));
        assert!(first != last && self.window(first).is_none() && self.window(last).is_none());
        assert!([first, last] == [primary, secondary] || [first, last] == [secondary, primary]);
        self[root].contents = Contents::RegionPair {
            regions: [first, last],
            selection: children.selection,
            primary_first: first == primary,
        };
    }

    pub(crate) fn region_pair(&self, root: NodeId) -> Option<(NodeId, NodeId)> {
        let Contents::RegionPair {
            regions: [first, last],
            primary_first,
            ..
        } = self.map.get(root)?.contents
        else {
            return None;
        };
        Some(if primary_first {
            (first, last)
        } else {
            (last, first)
        })
    }

    pub(crate) fn set_primary_region_first(&mut self, root: NodeId, first: bool) {
        let Contents::RegionPair {
            regions,
            selection,
            primary_first,
        } = self[root].contents
        else {
            unreachable!("expected a region pair");
        };
        if primary_first != first {
            let [left, right] = regions;
            self[right].prev_sibling = None;
            self[right].next_sibling = Some(left);
            self[left].prev_sibling = Some(right);
            self[left].next_sibling = None;
            self[root].contents = Contents::RegionPair {
                regions: [right, left],
                selection,
                primary_first: first,
            };
        }
    }

    pub(crate) fn children_total(&self, parent: NodeId) -> f32 {
        parent.children(self).map(|child| self[child].layout.size.max(0.0)).sum()
    }

    /// Preserve sibling proportions with a mean weight of one.
    pub(crate) fn normalize_weights(&mut self, parent: NodeId) -> bool {
        let (count, total) = parent.children(self).fold((0, 0.0_f32), |(count, total), child| {
            (count + 1, total + self[child].layout.size.max(0.0))
        });
        if !total.is_finite() || total <= f32::EPSILON {
            return false;
        }
        let scale = count as f32 / total;
        let mut child = parent.first_child(self);
        while let Some(node) = child {
            self[node].layout.size *= scale;
            child = node.next_sibling(self);
        }
        true
    }

    pub fn capacity(&self) -> usize { self.map.capacity() }

    pub fn reserve(&mut self, additional: usize) { self.map.reserve(additional) }

    pub(crate) fn window(&self, node: NodeId) -> Option<WindowId> { self.map.get(node)?.window() }

    pub(crate) fn selection_info(&self, node: NodeId) -> Option<SelectionInfo> {
        self.map.get(node)?.children()?.selection
    }

    pub fn contains(&self, id: NodeId) -> bool { self.map.contains_key(id) }
}

impl Index<NodeId> for NodeMap {
    type Output = Node;

    fn index(&self, index: NodeId) -> &Self::Output { &self.map[index] }
}

impl IndexMut<NodeId> for NodeMap {
    fn index_mut(&mut self, index: NodeId) -> &mut Self::Output { &mut self.map[index] }
}

slotmap::new_key_type! {
    /// Represents a node somewhere in the tree.
    pub struct NodeId;
}

impl NodeId {
    #[track_caller]
    pub fn parent(self, map: &NodeMap) -> Option<NodeId> {
        map.map.get(self).and_then(|n| n.parent)
    }

    #[track_caller]
    pub fn children(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.first_child(map), |node| node.next_sibling(map))
    }

    #[track_caller]
    pub fn children_rev(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.last_child(map), |node| node.prev_sibling(map))
    }

    #[track_caller]
    pub fn traverse_postorder(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        fn leftmost(mut node: NodeId, map: &NodeMap) -> NodeId {
            while let Some(child) = node.first_child(map) {
                node = child;
            }
            node
        }
        std::iter::successors(Some(leftmost(self, map)), move |&node| {
            if node == self {
                None
            } else if let Some(next) = node.next_sibling(map) {
                Some(leftmost(next, map))
            } else {
                node.parent(map)
            }
        })
    }

    #[track_caller]
    pub fn traverse_preorder(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(Some(self), move |node| node.next_in_subtree(self, map))
    }

    pub fn next_in_subtree(self, root: NodeId, map: &NodeMap) -> Option<NodeId> {
        self.first_child(map).or_else(|| {
            self.ancestors(map)
                .take_while(|&node| node != root)
                .find_map(|node| node.next_sibling(map))
        })
    }

    pub fn prev_in_subtree(self, root: NodeId, map: &NodeMap) -> Option<NodeId> {
        if self == root {
            return None;
        }
        self.prev_sibling(map)
            .map(|node| node.last_descendant(map))
            .or_else(|| self.parent(map))
    }

    pub(crate) fn step(self, root: NodeId, map: &NodeMap, forward: bool) -> Option<NodeId> {
        if forward {
            self.next_in_subtree(root, map)
        } else {
            self.prev_in_subtree(root, map)
        }
    }

    pub fn last_descendant(mut self, map: &NodeMap) -> NodeId {
        while let Some(last) = self.last_child(map) {
            self = last;
        }
        self
    }

    /// Returns an iterator over all ancestors of the current node, including itself.
    #[track_caller]
    pub fn ancestors(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        let mut next = Some(self);
        std::iter::from_fn(move || {
            let node = next;
            next = node.and_then(|n| map.map.get(n).and_then(|nd| nd.parent));
            node
        })
    }

    pub fn common_ancestor(self, other: NodeId, map: &NodeMap) -> Option<NodeId> {
        if self == other {
            return Some(self);
        }
        let a = self.ancestors(map).count();
        let b = other.ancestors(map).count();
        self.ancestors(map)
            .skip(a.saturating_sub(b))
            .zip(other.ancestors(map).skip(b.saturating_sub(a)))
            .find_map(|(left, right)| (left == right).then_some(left))
    }

    /// Returns an iterator over all ancestors of the current node, including itself.
    #[track_caller]
    pub fn ancestors_with_parent(
        self,
        map: &NodeMap,
    ) -> impl Iterator<Item = (NodeId, Option<NodeId>)> + '_ {
        let mut next = Some(self);
        std::iter::from_fn(move || {
            let node = next;
            next = node.and_then(|n| map.map.get(n).and_then(|nd| nd.parent));
            node.map(|n| (n, next))
        })
    }

    #[track_caller]
    pub fn next_sibling(self, map: &NodeMap) -> Option<NodeId> {
        map.map.get(self).and_then(|n| n.next_sibling)
    }

    #[track_caller]
    pub fn prev_sibling(self, map: &NodeMap) -> Option<NodeId> {
        map.map.get(self).and_then(|n| n.prev_sibling)
    }

    #[track_caller]
    pub fn first_child(self, map: &NodeMap) -> Option<NodeId> {
        map.map.get(self)?.children()?.first
    }

    #[track_caller]
    pub fn last_child(self, map: &NodeMap) -> Option<NodeId> { map.map.get(self)?.children()?.last }

    #[track_caller]
    pub fn is_empty(self, map: &NodeMap) -> bool { self.first_child(map).is_none() }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LayoutInfo {
    pub(crate) size: f32,
    pub(crate) kind: LayoutKind,
    #[serde(default)]
    pub(crate) is_fullscreen: bool,
    #[serde(default)]
    pub(crate) is_fullscreen_within_gaps: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(from = "StoredNode", into = "StoredNode")]
pub struct Node {
    parent: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    pub(crate) layout: LayoutInfo,
    contents: Contents,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Contents {
    Container(Children),
    RegionPair {
        regions: [NodeId; 2],
        selection: Option<SelectionInfo>,
        primary_first: bool,
    },
    Window(WindowId),
}

impl Default for Contents {
    fn default() -> Self { Self::Container(Children::default()) }
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Children {
    first: Option<NodeId>,
    last: Option<NodeId>,
    selection: Option<SelectionInfo>,
}

// Keep the saved SlotMap entry shape, including its original links and keys.
#[derive(Serialize, Deserialize)]
struct StoredNode {
    parent: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    #[serde(default)]
    layout: LayoutInfo,
    #[serde(default)]
    selection: Option<SelectionInfo>,
    #[serde(default)]
    window: Option<WindowId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    master_first: Option<bool>,
}

impl From<StoredNode> for Node {
    fn from(stored: StoredNode) -> Self {
        let contents = match (
            stored.window,
            stored.master_first,
            stored.first_child,
            stored.last_child,
        ) {
            (Some(window), _, _, _) => Contents::Window(window),
            (None, Some(primary_first), Some(first), Some(last)) => Contents::RegionPair {
                regions: [first, last],
                selection: stored.selection,
                primary_first,
            },
            _ => Contents::Container(Children {
                first: stored.first_child,
                last: stored.last_child,
                selection: stored.selection,
            }),
        };
        Self {
            parent: stored.parent,
            prev_sibling: stored.prev_sibling,
            next_sibling: stored.next_sibling,
            layout: stored.layout,
            contents,
        }
    }
}

impl From<Node> for StoredNode {
    fn from(node: Node) -> Self {
        let children = node.children().unwrap_or_default();
        Self {
            parent: node.parent,
            prev_sibling: node.prev_sibling,
            next_sibling: node.next_sibling,
            first_child: children.first,
            last_child: children.last,
            layout: node.layout,
            selection: children.selection,
            window: node.window(),
            master_first: match node.contents {
                Contents::RegionPair { primary_first, .. } => Some(primary_first),
                _ => None,
            },
        }
    }
}

impl Node {
    fn is_region_pair(&self) -> bool { matches!(self.contents, Contents::RegionPair { .. }) }

    fn children(&self) -> Option<Children> {
        match self.contents {
            Contents::Container(children) => Some(children),
            Contents::RegionPair {
                regions: [first, last],
                selection,
                ..
            } => Some(Children {
                first: Some(first),
                last: Some(last),
                selection,
            }),
            Contents::Window(_) => None,
        }
    }

    fn children_mut(&mut self) -> &mut Children {
        match &mut self.contents {
            Contents::Container(children) => children,
            _ => unreachable!("only regular containers accept structural edits"),
        }
    }

    fn window(&self) -> Option<WindowId> {
        match self.contents {
            Contents::Window(window) => Some(window),
            _ => None,
        }
    }

    pub(crate) fn set_window(&mut self, window: WindowId) {
        assert!(self.children().is_none_or(|children| children.first.is_none()));
        self.contents = Contents::Window(window);
    }

    pub(crate) fn selection_mut(&mut self) -> &mut Option<SelectionInfo> {
        match &mut self.contents {
            Contents::Container(children) => &mut children.selection,
            Contents::RegionPair { selection, .. } => selection,
            Contents::Window(_) => unreachable!("window cannot select a child"),
        }
    }
}

impl NodeMap {
    fn link_child(
        &mut self,
        id: NodeId,
        parent: NodeId,
        prev: Option<NodeId>,
        next: Option<NodeId>,
    ) {
        let Some(node) = self.map.get_mut(id) else { return };
        debug_assert!(node.prev_sibling.is_none() && node.next_sibling.is_none());
        node.parent = Some(parent);
        node.prev_sibling = prev;
        node.next_sibling = next;
        if let Some(prev) = prev {
            self[prev].next_sibling = Some(id);
        } else {
            self[parent].children_mut().first = Some(id);
        }
        if let Some(next) = next {
            self[next].prev_sibling = Some(id);
        } else {
            self[parent].children_mut().last = Some(id);
        }
    }

    #[track_caller]
    fn unlink(&mut self, id: NodeId) {
        let Some(node) = self.map.get_mut(id) else { return };
        let prev = node.prev_sibling.take();
        let next = node.next_sibling.take();
        let Some(parent) = node.parent.take() else { return };
        if let Some(prev) = prev {
            self[prev].next_sibling = next;
        } else {
            self[parent].children_mut().first = next;
        }
        if let Some(next) = next {
            self[next].prev_sibling = prev;
        } else {
            self[parent].children_mut().last = prev;
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub(crate) struct WindowIndex {
    pub(crate) window_nodes: crate::common::collections::BTreeMap<WindowId, WindowNodeInfoVec>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(crate) struct WindowNodeInfo {
    pub(crate) layout: LayoutId,
    pub(crate) node: NodeId,
}

#[derive(Clone, Serialize, Deserialize, Default, Debug)]
pub(crate) struct WindowNodeInfoVec(pub(crate) Vec<WindowNodeInfo>);

impl WindowIndex {
    pub(crate) fn occurrences(&self, wid: WindowId) -> &[WindowNodeInfo] {
        self.window_nodes.get(&wid).map_or(&[], |nodes| &nodes.0)
    }

    pub(crate) fn node_for(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.occurrences(wid)
            .iter()
            .find(|info| info.layout == layout)
            .map(|info| info.node)
    }

    pub(crate) fn set_window(
        &mut self,
        map: &mut NodeMap,
        layout: LayoutId,
        node: NodeId,
        wid: WindowId,
    ) {
        assert!(map.window(node).is_none(), "window already assigned to {node:?}");
        map[node].set_window(wid);
        self.window_nodes
            .entry(wid)
            .or_default()
            .0
            .push(WindowNodeInfo { layout, node });
    }

    pub(crate) fn replace_window(&mut self, map: &mut NodeMap, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        let nodes = self.window_nodes.remove(&from).unwrap_or_default();
        if nodes.0.is_empty() {
            return;
        }
        for info in &nodes.0 {
            map[info.node].set_window(to);
        }
        self.window_nodes.entry(to).or_default().0.extend(nodes.0);
    }

    pub(crate) fn take_window(&mut self, wid: WindowId) -> Vec<WindowNodeInfo> {
        self.window_nodes.remove(&wid).unwrap_or_default().0
    }

    pub(crate) fn take_nodes_for_app(
        &mut self,
        pid: pid_t,
    ) -> impl Iterator<Item = NodeId> + use<> {
        use crate::common::collections::BTreeExt;
        self.window_nodes
            .remove_all_for_pid(pid)
            .into_values()
            .flat_map(|infos| infos.0.into_iter().map(|info| info.node))
    }

    pub(crate) fn remove_node(&mut self, wid: WindowId, node: NodeId) {
        if let Some(nodes) = self.window_nodes.get_mut(&wid) {
            nodes.0.retain(|info| info.node != node);
            if nodes.0.is_empty() {
                self.window_nodes.remove(&wid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::*;

    /// A tree with the following structure:
    /// ```text
    ///         [tree]              [other_tree]
    ///        __root__              other_root
    ///       /    |   \
    /// child1  child2  child3
    ///            |
    ///           gc1
    /// ```
    struct TestTree {
        tree: Tree,
        root: NodeId,
        child1: NodeId,
        child2: NodeId,
        child3: NodeId,
        gc1: NodeId,
        other_root: NodeId,
    }

    impl TestTree {
        #[rustfmt::skip]
        fn new() -> Self {
            let mut tree = Tree::default();

            let root = tree.create_root();
            let child1 = tree.insert(Placement::Back(root));
            let child2 = tree.insert(Placement::Back(root));
            let child3 = tree.insert(Placement::Back(root));

            let gc1 = tree.insert(Placement::Back(child2));
            let other_root = tree.create_root();

            TestTree {
                tree, root,
                child1, child2, child3, gc1,
                other_root,
            }
        }

        fn get_children(&self, node: NodeId) -> Vec<NodeId> {
            node.children(&self.tree.map).collect()
        }

        fn get_children_rev(&self, node: NodeId) -> Vec<NodeId> {
            node.children_rev(&self.tree.map).collect()
        }

        #[track_caller]
        fn assert_children_are<const N: usize>(&self, children: [NodeId; N], parent: NodeId) {
            self.assert_children_are_inner(&children, parent);
        }

        #[track_caller]
        fn assert_children_are_inner(&self, children: &[NodeId], parent: NodeId) {
            assert_eq!(children, self.get_children(parent), "children did not match");
            assert_eq!(
                children.iter().copied().rev().collect::<Vec<_>>(),
                self.get_children_rev(parent),
                "reverse children did not match"
            );
            for child in self.get_children(parent) {
                assert_eq!(
                    self.tree.map[child].parent,
                    Some(parent),
                    "child has incorrect parent"
                );
            }
        }
    }

    #[test]
    fn iterator() {
        let t = TestTree::new();
        assert_eq!([t.child1, t.child2, t.child3], *t.get_children(t.root));
        assert!(t.get_children(t.child1).is_empty());
        assert_eq!([t.gc1], *t.get_children(t.child2));
        assert!(t.get_children(t.gc1).is_empty());
        assert!(t.get_children(t.child3).is_empty());
        assert!(t.get_children(t.other_root).is_empty());
    }

    #[test]
    fn rev_iterator() {
        let t = TestTree::new();
        assert_eq!([t.child3, t.child2, t.child1], *t.get_children_rev(t.root));
        assert!(t.get_children_rev(t.child1).is_empty());
        assert_eq!([t.gc1], *t.get_children_rev(t.child2));
        assert!(t.get_children_rev(t.gc1).is_empty());
        assert!(t.get_children_rev(t.child3).is_empty());
        assert!(t.get_children_rev(t.other_root).is_empty());
    }

    #[test]
    fn ancestors() {
        let t = TestTree::new();
        let ancestors = |node: NodeId| node.ancestors(&t.tree.map).collect::<Vec<_>>();
        assert_eq!([t.child1, t.root], *ancestors(t.child1));
        assert_eq!([t.gc1, t.child2, t.root], *ancestors(t.gc1));
        assert_eq!([t.child2, t.root], *ancestors(t.child2));
        assert_eq!([t.root], *ancestors(t.root));
        assert_eq!([t.other_root], *ancestors(t.other_root));
    }

    #[test]
    fn traverse_postorder() {
        let t = TestTree::new();
        let traverse = |node: NodeId| node.traverse_postorder(&t.tree.map).collect::<Vec<_>>();
        assert_eq!([t.child1, t.gc1, t.child2, t.child3, t.root], *traverse(t.root));
        assert_eq!([t.child1], *traverse(t.child1));
    }

    #[test]
    fn traverse_preorder() {
        let t = TestTree::new();
        let traverse = |node: NodeId| node.traverse_preorder(&t.tree.map).collect::<Vec<_>>();
        assert_eq!([t.root, t.child1, t.child2, t.gc1, t.child3], *traverse(t.root));
        assert_eq!([t.child1], *traverse(t.child1));
    }

    #[test]
    fn deep_copy() {
        let mut t = TestTree::new();
        {
            let copied_root = t.tree.clone_branch(t.root, LayoutId::default());
            let orig_ids = t.root.traverse_preorder(&t.tree.map).collect::<Vec<_>>();
            let copied_ids = copied_root.traverse_preorder(&t.tree.map).collect::<Vec<_>>();
            assert_eq!(orig_ids.len(), copied_ids.len());
            for (orig, copied) in iter::zip(orig_ids, copied_ids) {
                assert_ne!(orig, copied, "deep_copy reused id {orig:?}");
                assert_eq!(
                    orig.parent(&t.tree.map).is_some(),
                    copied.parent(&t.tree.map).is_some()
                );
                assert_eq!(
                    orig.children(&t.tree.map).count(),
                    copied.children(&t.tree.map).count()
                );
                assert_eq!(
                    orig.ancestors(&t.tree.map).count(),
                    copied.ancestors(&t.tree.map).count()
                );
            }
        }
        {
            let copied = t.tree.clone_branch(t.child1, LayoutId::default());
            assert_eq!(0, copied.children(&t.tree.map).count());
        }
    }

    #[test]
    fn push_back() {
        let mut t = TestTree::new();
        let child4 = t.tree.insert(Placement::Back(t.root));
        let gc0 = t.tree.insert(Placement::Back(t.child1));
        let gc2 = t.tree.insert(Placement::Back(t.child2));
        t.assert_children_are([t.child1, t.child2, t.child3, child4], t.root);
        t.assert_children_are([gc0], t.child1);
        t.assert_children_are([t.gc1, gc2], t.child2);
        t.assert_children_are([], gc2);
        t.assert_children_are([], t.child3);
        t.assert_children_are([], t.other_root);
    }

    #[test]
    fn insert_before() {
        let mut t = TestTree::new();
        let child0 = t.tree.insert(Placement::Before(t.child1));
        let child1_5 = t.tree.insert(Placement::Before(t.child2));
        let child2_5 = t.tree.insert(Placement::Before(t.child3));
        let gc0 = t.tree.insert(Placement::Before(t.gc1));
        t.assert_children_are(
            [child0, t.child1, child1_5, t.child2, child2_5, t.child3],
            t.root,
        );
        t.assert_children_are([], child0);
        t.assert_children_are([], t.child1);
        t.assert_children_are([], child1_5);
        t.assert_children_are([gc0, t.gc1], t.child2);
        t.assert_children_are([], child2_5);
        t.assert_children_are([], t.child3);
        t.assert_children_are([], t.other_root);
    }

    #[test]
    fn insert_after() {
        let mut t = TestTree::new();
        let child1_5 = t.tree.insert(Placement::After(t.child1));
        let child2_5 = t.tree.insert(Placement::After(t.child2));
        let child4 = t.tree.insert(Placement::After(t.child3));
        let gc2 = t.tree.insert(Placement::After(t.gc1));
        t.assert_children_are(
            [t.child1, child1_5, t.child2, child2_5, t.child3, child4],
            t.root,
        );
        t.assert_children_are([], t.child1);
        t.assert_children_are([], child1_5);
        t.assert_children_are([t.gc1, gc2], t.child2);
        t.assert_children_are([], child2_5);
        t.assert_children_are([], t.child3);
        t.assert_children_are([], child4);
        t.assert_children_are([], t.other_root);
    }

    #[test]
    fn remove() {
        let mut t = TestTree::new();

        t.tree.remove(t.child2);
        t.assert_children_are([t.child1, t.child3], t.root);
        assert!(!t.tree.map.map.contains_key(t.child2));
        assert!(!t.tree.map.map.contains_key(t.gc1));

        t.tree.remove(t.child3);
        t.assert_children_are([t.child1], t.root);
        assert!(!t.tree.map.map.contains_key(t.child3));

        t.tree.remove(t.child1);
        t.assert_children_are([], t.root);
        assert!(!t.tree.map.map.contains_key(t.child1));

        assert!(t.tree.map.map.contains_key(t.root));
        assert!(t.tree.map.map.contains_key(t.other_root));
        t.tree.remove(t.root);
        assert!(!t.tree.map.map.contains_key(t.root));
        t.tree.remove(t.other_root);
        assert!(!t.tree.map.map.contains_key(t.other_root));
    }

    #[test]
    fn invalid_placements_preserve_the_original_links() {
        let mut t = TestTree::new();
        t.tree.move_node(t.child2, Placement::After(t.child2));
        t.tree.move_node(t.child2, Placement::Back(t.gc1));
        t.assert_children_are([t.child1, t.child2, t.child3], t.root);
        t.assert_children_are([t.gc1], t.child2);
        assert!(t.get_children(t.gc1).is_empty());
    }

    #[test]
    fn detach_and_reattach() {
        let mut t = TestTree::new();

        t.tree.move_node(t.child1, Placement::After(t.child2));
        t.assert_children_are([t.child2, t.child1, t.child3], t.root);
        t.assert_children_are([t.gc1], t.child2);

        t.tree.move_node(t.child1, Placement::Before(t.child2));
        t.assert_children_are([t.child1, t.child2, t.child3], t.root);

        t.tree.move_node(t.child2, Placement::Back(t.child1));
        t.assert_children_are([t.child1, t.child3], t.root);
        t.assert_children_are([t.child2], t.child1);

        t.tree.move_node(t.child2, Placement::After(t.child1));
        t.assert_children_are([t.child2, t.child3], t.root);
        assert!(!t.tree.map.contains(t.child1));

        t.tree.move_node(t.child3, Placement::Back(t.child2));
        t.assert_children_are([t.child2], t.root);
        t.assert_children_are([t.gc1, t.child3], t.child2);

        t.tree.move_node(t.child3, Placement::After(t.child2));
        t.assert_children_are([t.gc1, t.child3], t.root);
        assert!(!t.tree.map.contains(t.child2));
    }
}
