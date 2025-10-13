use std::ops::{Deref, DerefMut, Index, IndexMut};

use serde::{Deserialize, Serialize};
use slotmap::SlotMap;

/// N-ary tree.
#[derive(Serialize, Deserialize)]
pub struct Tree<O> {
    pub map: NodeMap,
    pub data: O,
}

impl Tree<()> {
    pub fn new() -> Self { Self::with_observer(()) }
}

impl<O: Observer> Tree<O> {
    pub fn with_observer(data: O) -> Self { Tree { map: NodeMap::new(), data } }

    pub fn mk_node(&mut self) -> UnattachedNode<'_, O> {
        let id = self.map.map.insert(Node::default());
        self.data.added_to_forest(&self.map, id);
        UnattachedNode { id, tree: self }
    }
}

/// Map that holds the structure of the tree.
///
/// Multiple trees can be contained within a map. This also makes it easier
/// to move branches between trees.
#[derive(Serialize, Deserialize)]
pub struct NodeMap {
    map: SlotMap<NodeId, Node>,
}

impl NodeMap {
    fn new() -> NodeMap { NodeMap { map: SlotMap::default() } }

    pub fn capacity(&self) -> usize { self.map.capacity() }

    pub fn reserve(&mut self, additional: usize) { self.map.reserve(additional) }

    pub fn contains(&self, id: NodeId) -> bool { self.map.contains_key(id) }
}

impl Index<NodeId> for NodeMap {
    type Output = Node;

    fn index(&self, index: NodeId) -> &Self::Output { &self.map[index] }
}

impl IndexMut<NodeId> for NodeMap {
    fn index_mut(&mut self, index: NodeId) -> &mut Self::Output { &mut self.map[index] }
}

/// Represents ownership of a particular node in a tree.
///
/// Nodes must be removed manually, because removal requires a reference to the
/// [`map`].  If a value of this type is dropped without
/// [`OwnedNode::remove`] being called, it will panic in debug builds.
///
/// Every `OwnedNode` has a name which will be used in the panic message.
#[must_use]
#[derive(Debug, Serialize, Deserialize)]
pub struct OwnedNode(Option<NodeId>, String);

impl OwnedNode {
    /// Creates a new root node.
    pub fn new_root_in(map: &mut Tree<impl Observer>, name: &'static str) -> Self {
        let node = map.mk_node();
        Self::own(node.id, name)
    }

    /// Marks a non-root node as owned.
    pub fn own(node: NodeId, name: &'static str) -> Self { OwnedNode(Some(node), name.to_owned()) }

    pub fn id(&self) -> NodeId { self.0.expect("OwnedNode::id called on removed OwnedNode") }

    #[track_caller]
    pub fn remove(&mut self, tree: &mut Tree<impl Observer>) {
        if let Some(id) = self.0.take() {
            UnattachedNode { id, tree }.remove()
        }
    }

    #[track_caller]
    pub fn replace<'a, O>(&mut self, new: UnattachedNode<'a, O>) -> UnattachedNode<'a, O> {
        let id = self.0.replace(new.id).expect("Can't replace removed node");
        UnattachedNode { id, ..new }
    }
}

impl Deref for OwnedNode {
    type Target = NodeId;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("OwnedNode deref on removed OwnedNode")
    }
}

impl DerefMut for OwnedNode {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("OwnedNode deref_mut on removed OwnedNode")
    }
}

impl Drop for OwnedNode {
    fn drop(&mut self) {
        if cfg!(debug_assertions) {
            if let Some(node) = self.0 {
                panic!(
                    "OwnedNode {name:?} dropped without OwnedNode::remove being called: {node:?}",
                    name = self.1,
                );
            }
        }
    }
}

slotmap::new_key_type! {
    /// Represents a node somewhere in the tree.
    pub struct NodeId;
}

impl NodeId {
    #[track_caller]
    pub fn detach<'a, O: Observer>(self, tree: &'a mut Tree<O>) -> DetachedNode<'a, O> {
        DetachedNode { id: self, tree }
    }

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

    /// Creates a deep copy of the subtree rooted at this node.
    ///
    /// This method does not call observer events on the created nodes.
    #[track_caller]
    pub fn deep_copy<'a, O: Observer>(self, tree: &'a mut Tree<O>) -> UnattachedNode<'a, O> {
        let new_root = tree.mk_node().id;
        let mut stack = vec![(self, new_root)];
        let preorder = self.traverse_preorder(&tree.map).skip(1).collect::<Vec<_>>();
        for old in preorder {
            while old.parent(&tree.map) != stack.last().map(|(oldp, _newp)| *oldp) {
                stack.pop();
            }
            let parent = stack
                .last()
                .map(|(_, newp)| *newp)
                .expect("deep_copy: structural invariant violated");
            let new = tree.mk_node().push_back(parent);
            stack.push((old, new));
        }
        UnattachedNode { id: new_root, tree }
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

    #[track_caller]
    pub fn remove_root<O: Observer>(self, tree: &mut Tree<O>) {
        assert!(
            self.parent(&tree.map).is_none(),
            "remove_root called on non-root node"
        );
        if let Some(node) = tree.map.map.remove(self) {
            node.delete_recursive(tree, self);
        }
    }
}

pub trait Observer
where Self: Sized {
    fn added_to_forest(&mut self, map: &NodeMap, node: NodeId);
    fn added_to_parent(&mut self, map: &NodeMap, node: NodeId);
    fn removing_from_parent(&mut self, map: &NodeMap, node: NodeId);
    fn removed_child(tree: &mut Tree<Self>, parent: NodeId);
    fn removed_from_forest(&mut self, map: &NodeMap, node: NodeId);
}

impl Observer for () {
    fn added_to_forest(&mut self, _forest: &NodeMap, _node: NodeId) {}

    fn added_to_parent(&mut self, _forest: &NodeMap, _node: NodeId) {}

    fn removing_from_parent(&mut self, _forest: &NodeMap, _node: NodeId) {}

    fn removed_child(_tree: &mut Tree<Self>, _parent: NodeId) {}

    fn removed_from_forest(&mut self, _forest: &NodeMap, _node: NodeId) {}
}

#[must_use = "Unattached nodes should be inserted into the tree or created as a root with OwnedNode"]
pub struct UnattachedNode<'a, O> {
    // Nothing prevents this from being public, just haven't needed it yet.
    id: NodeId,
    tree: &'a mut Tree<O>,
}

impl<'a, O: Observer> UnattachedNode<'a, O> {
    pub(crate) fn make_root(self, name: &'static str) -> OwnedNode { OwnedNode::own(self.id, name) }
}

impl<'a, O: Observer> UnattachedNode<'a, O> {
    /// Consumes this unattached node and returns its `NodeId` without
    /// creating an `OwnedNode` root guard. The node remains in the forest
    /// as a root until attached or explicitly removed.
    pub fn into_id(self) -> NodeId { self.id }

    #[track_caller]
    pub(crate) fn push_back(self, parent: NodeId) -> NodeId {
        self.attach_with(|this| this.id.link_under_back(parent, &mut this.tree.map))
    }

    #[track_caller]
    pub(crate) fn insert_before(self, sibling: NodeId) -> NodeId {
        self.attach_with(|this| this.id.link_before(sibling, &mut this.tree.map))
    }

    #[track_caller]
    pub(crate) fn insert_after(self, sibling: NodeId) -> NodeId {
        self.attach_with(|this| this.id.link_after(sibling, &mut this.tree.map))
    }

    #[track_caller]
    pub(crate) fn remove(self) {
        debug_assert!(self.id.parent(&self.tree.map).is_none());
        if let Some(node) = self.tree.map.map.remove(self.id) {
            node.delete_recursive(self.tree, self.id);
        }
    }

    fn attach_with(mut self, attach: impl FnOnce(&mut Self)) -> NodeId {
        attach(&mut self);
        self.tree.data.added_to_parent(&self.tree.map, self.id);
        self.id
    }
}

#[must_use = "Detached nodes should be reattached to the tree or removed"]
pub struct DetachedNode<'a, O> {
    id: NodeId,
    tree: &'a mut Tree<O>,
}

impl<'a, O: Observer> DetachedNode<'a, O> {
    #[track_caller]
    pub(crate) fn push_back(self, parent: NodeId) -> ReattachedNode<'a, O> {
        self.attach_with(parent, |this| {
            this.id.link_under_back(parent, &mut this.tree.map)
        })
    }

    #[track_caller]
    pub(crate) fn insert_before(self, sibling: NodeId) -> ReattachedNode<'a, O> {
        let new_parent = sibling
            .parent(&self.tree.map)
            .expect("cannot make a sibling of a root node or invalid sibling");
        self.attach_with(new_parent, |this| {
            this.id.link_before(sibling, &mut this.tree.map)
        })
    }

    #[track_caller]
    pub(crate) fn insert_after(self, sibling: NodeId) -> ReattachedNode<'a, O> {
        let new_parent = sibling
            .parent(&self.tree.map)
            .expect("cannot make a sibling of a root node or invalid sibling");
        self.attach_with(new_parent, |this| {
            this.id.link_after(sibling, &mut this.tree.map)
        })
    }

    #[track_caller]
    pub(crate) fn remove(self) {
        if let Some(parent) = self.id.parent(&self.tree.map) {
            self.tree.data.removing_from_parent(&self.tree.map, self.id);
            self.tree.map.unlink(self.id);
            O::removed_child(self.tree, parent);
            if let Some(node) = self.tree.map.map.remove(self.id) {
                node.delete_recursive(self.tree, self.id);
            }
        }
    }

    fn attach_with(
        mut self,
        new_parent: NodeId,
        attach: impl FnOnce(&mut Self),
    ) -> ReattachedNode<'a, O> {
        let old_parent = self.id.parent(&self.tree.map);
        if old_parent.is_some() && old_parent != Some(new_parent) {
            self.tree.data.removing_from_parent(&self.tree.map, self.id);
        }
        self.tree.map.unlink(self.id);
        attach(&mut self);
        if old_parent.is_some() && old_parent != Some(new_parent) {
            self.tree.data.added_to_parent(&self.tree.map, self.id);
        }
        ReattachedNode {
            detached: self,
            old_parent,
            new_parent,
        }
    }
}

pub struct ReattachedNode<'a, O: Observer> {
    detached: DetachedNode<'a, O>,
    old_parent: Option<NodeId>,
    new_parent: NodeId,
}

impl<'a, O: Observer> ReattachedNode<'a, O> {
    pub(crate) fn with(self, f: impl FnOnce(NodeId, &mut Tree<O>)) -> Self {
        f(self.detached.id, self.detached.tree);
        self
    }

    pub(crate) fn finish(self) -> NodeId {
        self.detached.id
        // self is dropped at the end of the scope.
    }
}

impl<'a, O: Observer> Drop for ReattachedNode<'a, O> {
    fn drop(&mut self) {
        if let Some(old) = self.old_parent {
            if old != self.new_parent {
                O::removed_child(self.detached.tree, old);
            }
        }
    }
}

#[derive(Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Node {
    parent: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
}

impl NodeId {
    pub(crate) fn link_under_back(self, parent: NodeId, map: &mut NodeMap) {
        if self == parent {
            return;
        }
        if !map.contains(self) || !map.contains(parent) {
            return;
        }

        let prev_child = {
            let parent_node = map.map.get_mut(parent).expect("parent missing after contains check");
            parent_node.first_child.get_or_insert(self);
            parent_node.last_child.replace(self)
        };

        if let Some(self_node) = map.map.get_mut(self) {
            self_node.parent = Some(parent);
        } else {
            return;
        }

        if let Some(prev) = prev_child {
            self.hlink_after(prev, map);
        }
    }

    #[track_caller]
    pub(crate) fn link_before(self, next: NodeId, map: &mut NodeMap) {
        let parent = map.map.get(next).and_then(|n| n.parent);
        let parent = parent.expect("cannot make a sibling of a root node or invalid sibling");
        if let Some(self_node) = map.map.get_mut(self) {
            self_node.parent = Some(parent);
        } else {
            return;
        }
        if let Some(parent_node) = map.map.get_mut(parent) {
            debug_assert!(parent_node.first_child.is_some());
            if parent_node.first_child == Some(next) {
                parent_node.first_child.replace(self);
            }
        }
        self.hlink_before(next, map);
    }

    #[track_caller]
    pub(crate) fn link_after(self, prev: NodeId, map: &mut NodeMap) {
        let parent = map.map.get(prev).and_then(|n| n.parent);
        let parent = parent.expect("cannot make a sibling of a root node or invalid sibling");
        if let Some(self_node) = map.map.get_mut(self) {
            self_node.parent = Some(parent);
        } else {
            return;
        }
        if let Some(parent_node) = map.map.get_mut(parent) {
            debug_assert!(parent_node.last_child.is_some());
            if parent_node.last_child == Some(prev) {
                parent_node.last_child.replace(self);
            }
        }
        self.hlink_after(prev, map);
    }

    pub(crate) fn hlink_after(self, prev: NodeId, map: &mut NodeMap) {
        if self == prev {
            return;
        }
        if map.map.get(self).is_none() || map.map.get(prev).is_none() {
            return;
        }

        if let Some(self_node) = map.map.get_mut(self) {
            debug_assert_eq!(self_node.prev_sibling, None);
            self_node.prev_sibling.replace(prev);
        } else {
            return;
        }

        if let Some(prev_node) = map.map.get_mut(prev) {
            let next = prev_node.next_sibling.replace(self);
            if let Some(next) = next {
                if let Some(next_node) = map.map.get_mut(next) {
                    next_node.prev_sibling.replace(self);
                }
                if let Some(self_node) = map.map.get_mut(self) {
                    self_node.next_sibling.replace(next);
                }
            }
        }
    }

    pub(crate) fn hlink_before(self, next: NodeId, map: &mut NodeMap) {
        if self == next {
            return;
        }
        if map.map.get(self).is_none() || map.map.get(next).is_none() {
            return;
        }

        if let Some(self_node) = map.map.get_mut(self) {
            debug_assert_eq!(self_node.next_sibling, None);
            self_node.next_sibling.replace(next);
        } else {
            return;
        }

        if let Some(next_node) = map.map.get_mut(next) {
            let prev = next_node.prev_sibling.replace(self);
            if let Some(prev) = prev {
                if let Some(prev_node) = map.map.get_mut(prev) {
                    prev_node.next_sibling.replace(self);
                }
                if let Some(self_node) = map.map.get_mut(self) {
                    self_node.prev_sibling.replace(prev);
                }
            }
        }
    }
}

impl NodeMap {
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

impl Node {
    #[track_caller]
    pub(crate) fn delete_recursive(&self, cx: &mut Tree<impl Observer>, id: NodeId) {
        cx.data.removed_from_forest(&cx.map, id);
        let mut iter = self.first_child;
        while let Some(child) = iter {
            let next = cx.map.map.get(child).and_then(|n| n.next_sibling);
            if let Some(node) = cx.map.map.remove(child) {
                node.delete_recursive(cx, child);
            }
            iter = next;
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
        if let Some(child) = node.first_child(self.map) {
            self.cur = Some(child);
        } else {
            self.cur = None;
            for ancestor in node.ancestors(self.map) {
                if ancestor == self.top {
                    break;
                }
                if let Some(sibling) = ancestor.next_sibling(self.map) {
                    self.cur = Some(sibling);
                    break;
                }
            }
        }
        Some(node)
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
        tree: Tree<Events>,
        root_node: OwnedNode,
        root: NodeId,
        child1: NodeId,
        child2: NodeId,
        child3: NodeId,
        gc1: NodeId,
        other_root_node: OwnedNode,
        other_root: NodeId,
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            if !self.root_node.0.is_none() {
                self.root_node.remove(&mut self.tree);
            }
            if !self.other_root_node.0.is_none() {
                self.other_root_node.remove(&mut self.tree);
            }
        }
    }

    impl TestTree {
        #[rustfmt::skip]
        fn new() -> Self {
            let mut tree = Tree::with_observer(Events(vec![]));

            let root_node = OwnedNode::new_root_in(&mut tree, "tree");
            let root = root_node.id();
            let child1 = tree.mk_node().push_back(root);
            let child2 = tree.mk_node().push_back(root);
            let child3 = tree.mk_node().push_back(root);

            let gc1 = tree.mk_node().push_back(child2);
            let other_tree = OwnedNode::new_root_in(&mut tree, "other_tree");
            let other_root = other_tree.id();

            let mut t = TestTree {
                tree, root_node, root,
                child1, child2, child3, gc1,
                other_root_node: other_tree, other_root,
            };
            t.clear_events();
            t
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

        #[track_caller]
        fn assert_events_are<const N: usize>(&mut self, events: [TreeEvent; N]) {
            Self::assert_events_are_inner(&mut self.tree, &events);
        }

        #[track_caller]
        fn assert_events_are_inner(tree: &mut Tree<Events>, events: &[TreeEvent]) {
            let actual: Vec<_> = tree.data.0.drain(..).collect();
            pretty_assertions::assert_eq!(events, actual);
        }

        fn clear_events(&mut self) { self.tree.data.0.clear(); }
    }

    #[derive(Clone, PartialEq, Debug)]
    enum TreeEvent {
        AddedToForest(NodeId),
        AddedToParent(NodeId),
        RemovingFromParent(NodeId, NodeId),
        RemovedChild(NodeId),
        RemovedFromForest(NodeId),
    }
    use TreeEvent::*;

    struct Events(Vec<TreeEvent>);

    impl Observer for Events {
        fn added_to_forest(&mut self, _map: &NodeMap, node: NodeId) {
            self.0.push(AddedToForest(node))
        }

        fn added_to_parent(&mut self, _map: &NodeMap, node: NodeId) {
            self.0.push(AddedToParent(node))
        }

        fn removing_from_parent(&mut self, map: &NodeMap, node: NodeId) {
            let parent =
                node.parent(map).expect("removing_from_parent called on node without parent");
            self.0.push(RemovingFromParent(node, parent))
        }

        fn removed_child(tree: &mut Tree<Self>, parent: NodeId) {
            tree.data.0.push(RemovedChild(parent))
        }

        fn removed_from_forest(&mut self, _map: &NodeMap, node: NodeId) {
            self.0.push(RemovedFromForest(node))
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
            let copied_root = t.root.deep_copy(&mut t.tree).id;
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
            let copied = t.child1.deep_copy(&mut t.tree).id;
            assert_eq!(0, copied.children(&t.tree.map).count());
        }
    }

    #[test]
    fn push_back() {
        let mut t = TestTree::new();
        let child4 = t.tree.mk_node().push_back(t.root);
        t.assert_events_are([AddedToForest(child4), AddedToParent(child4)]);
        let gc0 = t.tree.mk_node().push_back(t.child1);
        t.assert_events_are([AddedToForest(gc0), AddedToParent(gc0)]);
        let gc2 = t.tree.mk_node().push_back(t.child2);
        t.assert_events_are([AddedToForest(gc2), AddedToParent(gc2)]);
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
        let child0 = t.tree.mk_node().insert_before(t.child1);
        t.assert_events_are([AddedToForest(child0), AddedToParent(child0)]);
        let child1_5 = t.tree.mk_node().insert_before(t.child2);
        let child2_5 = t.tree.mk_node().insert_before(t.child3);
        let gc0 = t.tree.mk_node().insert_before(t.gc1);
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
        let child1_5 = t.tree.mk_node().insert_after(t.child1);
        t.assert_events_are([AddedToForest(child1_5), AddedToParent(child1_5)]);
        let child2_5 = t.tree.mk_node().insert_after(t.child2);
        let child4 = t.tree.mk_node().insert_after(t.child3);
        let gc2 = t.tree.mk_node().insert_after(t.gc1);
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

        t.child2.detach(&mut t.tree).remove();
        t.assert_children_are([t.child1, t.child3], t.root);
        assert!(!t.tree.map.map.contains_key(t.child2));
        assert!(!t.tree.map.map.contains_key(t.gc1));
        t.assert_events_are([
            RemovingFromParent(t.child2, t.root),
            RemovedChild(t.root),
            RemovedFromForest(t.child2),
            RemovedFromForest(t.gc1),
        ]);

        t.child3.detach(&mut t.tree).remove();
        t.assert_children_are([t.child1], t.root);
        assert!(!t.tree.map.map.contains_key(t.child3));
        t.assert_events_are([
            RemovingFromParent(t.child3, t.root),
            RemovedChild(t.root),
            RemovedFromForest(t.child3),
        ]);

        t.child1.detach(&mut t.tree).remove();
        t.assert_children_are([], t.root);
        assert!(!t.tree.map.map.contains_key(t.child1));
        t.assert_events_are([
            RemovingFromParent(t.child1, t.root),
            RemovedChild(t.root),
            RemovedFromForest(t.child1),
        ]);

        assert!(t.tree.map.map.contains_key(t.root));
        assert!(t.tree.map.map.contains_key(t.other_root));
        t.root_node.remove(&mut t.tree);
        t.assert_events_are([RemovedFromForest(t.root)]);
        assert!(!t.tree.map.map.contains_key(t.root));
        t.other_root_node.remove(&mut t.tree);
        t.assert_events_are([RemovedFromForest(t.other_root)]);
        assert!(!t.tree.map.map.contains_key(t.other_root));
    }

    #[test]
    fn detach_and_reattach() {
        let mut t = TestTree::new();

        t.child1.detach(&mut t.tree).insert_after(t.child2);
        t.assert_children_are([t.child2, t.child1, t.child3], t.root);
        t.assert_children_are([t.gc1], t.child2);
        t.assert_events_are([]);

        t.child1.detach(&mut t.tree).insert_before(t.child2);
        t.assert_children_are([t.child1, t.child2, t.child3], t.root);
        t.assert_events_are([]);

        t.child2.detach(&mut t.tree).push_back(t.child1).with(|_id, tree| {
            TestTree::assert_events_are_inner(tree, &[
                RemovingFromParent(t.child2, t.root),
                AddedToParent(t.child2),
            ]);
        });
        t.assert_events_are([RemovedChild(t.root)]);
        t.assert_children_are([t.child1, t.child3], t.root);
        t.assert_children_are([t.child2], t.child1);

        t.child2.detach(&mut t.tree).insert_after(t.child1);
        t.assert_children_are([t.child1, t.child2, t.child3], t.root);
        t.assert_children_are([], t.child1);
        t.assert_events_are([
            RemovingFromParent(t.child2, t.child1),
            AddedToParent(t.child2),
            RemovedChild(t.child1),
        ]);

        t.child3.detach(&mut t.tree).push_back(t.child2);
        t.assert_children_are([t.child1, t.child2], t.root);
        t.assert_children_are([t.gc1, t.child3], t.child2);
        t.assert_events_are([
            RemovingFromParent(t.child3, t.root),
            AddedToParent(t.child3),
            RemovedChild(t.root),
        ]);

        t.child3.detach(&mut t.tree).insert_after(t.child2);
        t.assert_children_are([t.child1, t.child2, t.child3], t.root);
        t.assert_children_are([t.gc1], t.child2);
        t.assert_events_are([
            RemovingFromParent(t.child3, t.child2),
            AddedToParent(t.child3),
            RemovedChild(t.child2),
        ]);
    }
}
