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

impl Tree {
    pub(crate) fn create_root(&mut self) -> NodeId { self.map.map.insert(Node::default()) }

    pub(crate) fn insert(&mut self, placement: Placement) -> NodeId {
        let node = self.create_root();
        self.map[node].layout.size = 1.0;
        self.link(node, placement);
        node
    }

    fn link(&mut self, node: NodeId, placement: Placement) {
        match placement {
            Placement::Back(parent) => node.link_under_back(parent, &mut self.map),
            Placement::Before(sibling) => node.link_before(sibling, &mut self.map),
            Placement::After(sibling) => node.link_after(sibling, &mut self.map),
        }
    }

    pub(crate) fn move_node(&mut self, node: NodeId, placement: Placement) -> NodeId {
        self.move_node_with(node, placement, |_, _| {})
    }

    pub(crate) fn move_node_with(
        &mut self,
        node: NodeId,
        placement: Placement,
        adjust: impl FnOnce(NodeId, &mut Self),
    ) -> NodeId {
        let parent = match placement {
            Placement::Back(parent) => parent,
            Placement::Before(sibling) | Placement::After(sibling) => {
                sibling.parent(&self.map).expect("cannot make a sibling of a root")
            }
        };
        let old_parent = node.parent(&self.map);
        let changed_parent = old_parent != Some(parent);
        if changed_parent && old_parent.is_some() {
            self.prepare_detach(node);
        }
        self.map.unlink(node);
        self.link(node, placement);
        if changed_parent {
            self.map[node].layout.size = 1.0;
        }
        adjust(node, self);
        if let Some(old) = old_parent.filter(|_| changed_parent) {
            self.prune_container(old);
        }
        node
    }

    pub(crate) fn remove(&mut self, node: NodeId) {
        if let Some(parent) = node.parent(&self.map) {
            self.prepare_detach(node);
            self.map.unlink(node);
            self.prune_container(parent);
        }
        self.delete_subtree(node);
    }

    fn prepare_detach(&mut self, node: NodeId) {
        self.map.detach_selection(node);
        if self.map.window(node).is_some() {
            self.map.normalize_after_removal(node);
        }
    }

    fn prune_container(&mut self, parent: NodeId) {
        if parent.parent(&self.map).is_none() || self.map[parent].layout.fixed_container {
            return;
        }
        match parent.first_child(&self.map) {
            None => self.remove(parent),
            Some(child) if child.next_sibling(&self.map).is_none() => {
                self.move_node_with(child, Placement::After(parent), |child, tree| {
                    tree.map.assume_size_of(child, parent);
                });
            }
            _ => {}
        }
    }

    fn delete_subtree(&mut self, root: NodeId) {
        let Some(node) = self.map.map.remove(root) else { return };
        if let Some(window) = node.window {
            self.window.remove_node(window, root);
        }
        let mut child = node.first_child;
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
            self.link(new, Placement::Back(*new_parent));
            self.map.remap_child_selection(*original_parent, *new_parent, old, new);
            if let Some(child) = old.first_child(&self.map) {
                stack.push((Some(child), old, new));
            }
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
    pub fn capacity(&self) -> usize { self.map.capacity() }

    pub fn reserve(&mut self, additional: usize) { self.map.reserve(additional) }

    pub(crate) fn window(&self, node: NodeId) -> Option<WindowId> { self.map.get(node)?.window }

    pub(crate) fn selection_info(&self, node: NodeId) -> Option<SelectionInfo> {
        self.map.get(node)?.selection
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
        ChildIterator {
            cur: map.map.get(self).and_then(|n| n.first_child),
            map,
        }
    }

    #[track_caller]
    pub fn children_rev(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        ChildRevIterator {
            cur: map.map.get(self).and_then(|n| n.last_child),
            map,
        }
    }

    #[track_caller]
    pub fn traverse_postorder(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        PostorderTraversal::new(map, self)
    }

    #[track_caller]
    pub fn traverse_preorder(self, map: &NodeMap) -> impl Iterator<Item = NodeId> + '_ {
        PreorderTraversal::new(map, self)
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

    /// Reorder siblings without changing membership, weights, or selection history.
    pub(crate) fn reorder(self, anchor: NodeId, before: bool, map: &mut NodeMap) {
        assert_ne!(self, anchor);
        assert!(self.parent(map).is_some());
        assert_eq!(self.parent(map), anchor.parent(map));
        map.unlink(self);
        if before {
            self.link_before(anchor, map);
        } else {
            self.link_after(anchor, map);
        }
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
        map.map.get(self).and_then(|n| n.first_child)
    }

    #[track_caller]
    pub fn last_child(self, map: &NodeMap) -> Option<NodeId> {
        map.map.get(self).and_then(|n| n.last_child)
    }

    #[track_caller]
    pub fn is_empty(self, map: &NodeMap) -> bool {
        map.map.get(self).map(|n| n.first_child.is_none()).unwrap_or(true)
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LayoutInfo {
    pub(crate) size: f32,
    #[serde(default)]
    pub(crate) fixed_container: bool,
    pub(crate) kind: LayoutKind,
    #[serde(default)]
    pub(crate) is_fullscreen: bool,
    #[serde(default)]
    pub(crate) is_fullscreen_within_gaps: bool,
}

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Node {
    parent: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    #[serde(default)]
    pub(crate) layout: LayoutInfo,
    #[serde(default)]
    pub(crate) selection: Option<SelectionInfo>,
    #[serde(default)]
    pub(crate) window: Option<WindowId>,
}

impl NodeId {
    fn link_under_back(self, parent: NodeId, map: &mut NodeMap) {
        if self != parent && map.contains(parent) {
            map.link_child(self, parent, parent.last_child(map), None);
        }
    }

    #[track_caller]
    fn link_before(self, next: NodeId, map: &mut NodeMap) {
        let parent = next
            .parent(map)
            .expect("cannot make a sibling of a root node or invalid sibling");
        if self != next {
            map.link_child(self, parent, next.prev_sibling(map), Some(next));
        }
    }

    #[track_caller]
    fn link_after(self, prev: NodeId, map: &mut NodeMap) {
        let parent = prev
            .parent(map)
            .expect("cannot make a sibling of a root node or invalid sibling");
        if self != prev {
            map.link_child(self, parent, Some(prev), prev.next_sibling(map));
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
            self[parent].first_child = Some(id);
        }
        if let Some(next) = next {
            self[next].prev_sibling = Some(id);
        } else {
            self[parent].last_child = Some(id);
        }
    }

    #[track_caller]
    pub(crate) fn unlink(&mut self, id: NodeId) {
        if let Some((prev_sibling, next_sibling, parent)) =
            self.map.get(id).map(|n| (n.prev_sibling, n.next_sibling, n.parent))
        {
            if let Some(prev) = prev_sibling {
                if let Some(prev_node) = self.map.get_mut(prev) {
                    prev_node.next_sibling = next_sibling;
                }
            }
            if let Some(next) = next_sibling {
                if let Some(next_node) = self.map.get_mut(next) {
                    next_node.prev_sibling = prev_sibling;
                }
            }
            if let Some(parent) = parent {
                if let Some(parent_node) = self.map.get_mut(parent) {
                    if parent_node.first_child == Some(id) {
                        parent_node.first_child = next_sibling;
                    }
                    if parent_node.last_child == Some(id) {
                        parent_node.last_child = prev_sibling;
                    }
                }
            }

            if let Some(node) = self.map.get_mut(id) {
                node.prev_sibling = None;
                node.next_sibling = None;
                node.parent = None;
            }
        }
    }
}

struct ChildIterator<'a> {
    cur: Option<NodeId>,
    map: &'a NodeMap,
}

impl<'a> Iterator for ChildIterator<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.cur?;
        self.cur = self.map.map.get(id).and_then(|n| n.next_sibling);
        Some(id)
    }
}

struct ChildRevIterator<'a> {
    cur: Option<NodeId>,
    map: &'a NodeMap,
}

impl<'a> Iterator for ChildRevIterator<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.cur?;
        self.cur = self.map.map.get(id).and_then(|n| n.prev_sibling);
        Some(id)
    }
}

struct PostorderTraversal<'a> {
    cur: Option<NodeId>,
    top: NodeId,
    map: &'a NodeMap,
}

impl<'a> PostorderTraversal<'a> {
    fn new(map: &'a NodeMap, root: NodeId) -> Self {
        Self {
            top: root,
            cur: Some(Self::descend_left(root, map)),
            map,
        }
    }

    fn descend_left(mut node: NodeId, map: &'a NodeMap) -> NodeId {
        while let Some(child) = node.first_child(map) {
            node = child;
        }
        node
    }
}

impl<'a> Iterator for PostorderTraversal<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.cur?;
        self.cur = None;
        if node != self.top {
            if let Some(next) = node.next_sibling(self.map) {
                self.cur = Some(Self::descend_left(next, self.map));
            } else {
                self.cur = node.parent(self.map);
            }
        }
        Some(node)
    }
}

struct PreorderTraversal<'a> {
    top: NodeId,
    cur: Option<NodeId>,
    map: &'a NodeMap,
}

impl<'a> PreorderTraversal<'a> {
    fn new(map: &'a NodeMap, root: NodeId) -> Self {
        Self {
            top: root,
            cur: Some(root),
            map,
        }
    }
}

impl<'a> Iterator for PreorderTraversal<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.cur?;
        self.cur = node.next_in_subtree(self.top, self.map);
        Some(node)
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
    pub(crate) fn layouts_for(&self, wid: WindowId) -> Vec<LayoutId> {
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

    pub(crate) fn set_window(
        &mut self,
        map: &mut NodeMap,
        layout: LayoutId,
        node: NodeId,
        wid: WindowId,
    ) {
        let existing = map[node].window.replace(wid);
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
            if let Some(window) = map.window(node) {
                for info in &mut self.window_nodes.get_mut(&window).unwrap().0 {
                    if info.node == node {
                        info.layout = layout;
                    }
                }
            }
        }
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
            map[info.node].window = Some(to);
        }
        self.window_nodes.entry(to).or_default().0.extend(nodes.0);
    }

    pub(crate) fn take_nodes_for(&mut self, wid: WindowId) -> impl Iterator<Item = NodeId> + use<> {
        self.window_nodes
            .remove(&wid)
            .unwrap_or_default()
            .0
            .into_iter()
            .map(|info| info.node)
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
            tree.map[child1].layout.fixed_container = true;
            tree.map[child2].layout.fixed_container = true;
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
        t.assert_children_are([t.child1, t.child2, t.child3], t.root);
        t.assert_children_are([], t.child1);

        t.tree.move_node(t.child3, Placement::Back(t.child2));
        t.assert_children_are([t.child1, t.child2], t.root);
        t.assert_children_are([t.gc1, t.child3], t.child2);

        t.tree.move_node(t.child3, Placement::After(t.child2));
        t.assert_children_are([t.child1, t.child2, t.child3], t.root);
        t.assert_children_are([t.gc1], t.child2);
    }
}
