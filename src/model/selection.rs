use serde::{Deserialize, Serialize};

use crate::model::tree::{NodeId, NodeMap};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Selection {
    nodes: slotmap::SecondaryMap<NodeId, SelectionInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SelectionInfo {
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

    pub fn last_selection(&self, node: NodeId) -> Option<NodeId> {
        self.nodes.get(node).map(|info| info.selected_child)
    }

    pub fn local_selection(&self, map: &NodeMap, node: NodeId) -> Option<NodeId> {
        let result = self.nodes.get(node);
        if let Some(result) = result {
            debug_assert_eq!(result.selected_child.parent(map), Some(node));
        }
        result.filter(|info| !info.stop_here).map(|info| info.selected_child)
    }

    pub fn select_locally(&mut self, map: &NodeMap, node: NodeId) -> bool {
        if let Some(parent) = node.parent(map) {
            self.nodes
                .insert(parent, SelectionInfo {
                    selected_child: node,
                    stop_here: false,
                })
                .map(|info| info.selected_child != node)
                .unwrap_or(true)
        } else {
            false
        }
    }

    pub fn select(&mut self, map: &NodeMap, selection: NodeId) {
        if let Some(info) = self.nodes.get_mut(selection) {
            info.stop_here = true;
        }
        let mut node = selection;
        while let Some(parent) = node.parent(map) {
            self.nodes.insert(parent, SelectionInfo {
                selected_child: node,
                stop_here: false,
            });
            node = parent;
        }
    }

    pub fn copy_branch(&mut self, map: &NodeMap, source: NodeId, destination: NodeId) {
        let Some(info) = self.nodes.get(source) else { return };
        let selected_child = std::iter::zip(source.children(map), destination.children(map))
            .find_map(|(old, new)| (old == info.selected_child).then_some(new))
            .expect("copied branches must have matching children");
        self.nodes.insert(destination, SelectionInfo {
            selected_child,
            stop_here: info.stop_here,
        });
    }

    pub fn detach(&mut self, map: &NodeMap, node: NodeId) {
        let parent = node.parent(map).unwrap();
        if let Some(info) = self.nodes.get_mut(parent)
            && info.selected_child == node
        {
            if let Some(next) = node.next_sibling(map).or(node.prev_sibling(map)) {
                info.selected_child = next;
            } else {
                self.nodes.remove(parent);
            }
        }
    }

    pub fn remove(&mut self, node: NodeId) { self.nodes.remove(node); }
}
