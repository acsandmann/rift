use serde::{Deserialize, Serialize};

use crate::model::tree::{NodeId, NodeMap};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Selection {
    pub(crate) nodes: slotmap::SecondaryMap<NodeId, SelectionInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SelectionInfo {
    selected_child: NodeId,
    stop_here: bool,
}

impl Selection {
    pub fn current_selection(&self, root: NodeId) -> NodeId {
        let mut node = root;
        while let Some(info) = self.nodes.get(node) {
            if info.stop_here {
                break;
            }
            node = info.selected_child;
        }
        node
    }

    pub(crate) fn selected_child(&self, node: NodeId) -> Option<NodeId> {
        self.nodes
            .get(node)
            .filter(|info| !info.stop_here)
            .map(|info| info.selected_child)
    }

    pub fn last_selection(&self, node: NodeId) -> Option<NodeId> {
        self.nodes.get(node).map(|info| info.selected_child)
    }
}

impl NodeMap {
    pub(crate) fn current_selection(&self, mut node: NodeId) -> NodeId {
        while let Some(info) = self.selection_info(node).filter(|info| !info.stop_here) {
            node = info.selected_child;
        }
        node
    }

    pub(crate) fn last_selection(&self, node: NodeId) -> Option<NodeId> {
        self.selection_info(node).map(|info| info.selected_child)
    }

    pub(crate) fn local_selection(&self, node: NodeId) -> Option<NodeId> {
        self.selection_info(node)
            .filter(|info| !info.stop_here)
            .map(|info| info.selected_child)
    }

    pub(crate) fn select_locally(&mut self, node: NodeId) -> bool {
        let Some(parent) = node.parent(self) else { return false };
        self[parent]
            .selection_mut()
            .replace(SelectionInfo {
                selected_child: node,
                stop_here: false,
            })
            .is_none_or(|info| info.selected_child != node)
    }

    pub(crate) fn select(&mut self, selection: NodeId) {
        if self.selection_info(selection).is_some() {
            self[selection].selection_mut().as_mut().unwrap().stop_here = true;
        }
        let mut node = selection;
        while let Some(parent) = node.parent(self) {
            *self[parent].selection_mut() = Some(SelectionInfo {
                selected_child: node,
                stop_here: false,
            });
            node = parent;
        }
    }

    pub(crate) fn remap_child_selection(
        &mut self,
        source: NodeId,
        destination: NodeId,
        old_child: NodeId,
        new_child: NodeId,
    ) {
        if let Some(info) =
            self.selection_info(source).filter(|info| info.selected_child == old_child)
        {
            *self[destination].selection_mut() = Some(SelectionInfo {
                selected_child: new_child,
                ..info
            });
        }
    }

    pub(crate) fn detach_selection(&mut self, node: NodeId) {
        let parent = node.parent(self).unwrap();
        if let Some(info) = self.selection_info(parent).filter(|info| info.selected_child == node) {
            let replacement = node.next_sibling(self).or_else(|| node.prev_sibling(self));
            *self[parent].selection_mut() =
                replacement.map(|selected_child| SelectionInfo { selected_child, ..info });
        }
    }
}
