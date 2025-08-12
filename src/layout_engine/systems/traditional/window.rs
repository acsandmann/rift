use accessibility_sys::pid_t;
use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use crate::layout_engine::LayoutId;
use crate::model::selection::TreeEvent;
use crate::model::tree::{NodeId, NodeMap};

#[derive(Default, Serialize, Deserialize)]
pub struct Window {
    windows: slotmap::SecondaryMap<NodeId, WindowId>,
    window_nodes: crate::common::collections::BTreeMap<WindowId, WindowNodeInfoVec>,
}

#[derive(Serialize, Deserialize)]
struct WindowNodeInfo {
    layout: LayoutId,
    node: NodeId,
}

#[derive(Serialize, Deserialize, Default)]
struct WindowNodeInfoVec(Vec<WindowNodeInfo>);

impl Window {
    pub fn at(&self, node: NodeId) -> Option<WindowId> { self.windows.get(node).copied() }

    pub fn node_for(&self, layout: LayoutId, wid: WindowId) -> Option<NodeId> {
        self.window_nodes
            .get(&wid)
            .into_iter()
            .flat_map(|nodes| nodes.0.iter().filter(|info| info.layout == layout))
            .next()
            .map(|info| info.node)
    }

    pub fn set_window(&mut self, layout: LayoutId, node: NodeId, wid: WindowId) {
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

    pub fn take_nodes_for(&mut self, wid: WindowId) -> impl Iterator<Item = (LayoutId, NodeId)> {
        self.window_nodes
            .remove(&wid)
            .unwrap_or_default()
            .0
            .into_iter()
            .map(|info| (info.layout, info.node))
    }

    pub fn take_nodes_for_app(
        &mut self,
        pid: pid_t,
    ) -> impl Iterator<Item = (WindowId, LayoutId, NodeId)> {
        use crate::common::collections::BTreeExt;
        let removed = self.window_nodes.remove_all_for_pid(pid);
        removed.into_iter().flat_map(|(wid, infos)| {
            infos.0.into_iter().map(move |info| (wid, info.layout, info.node))
        })
    }

    pub fn handle_event(&mut self, map: &NodeMap, event: TreeEvent) {
        match event {
            TreeEvent::AddedToForest(_) => (),
            TreeEvent::AddedToParent(node) => debug_assert!(
                self.windows.get(node.parent(map).unwrap()).is_none(),
                "Window nodes are not allowed to have children: {:?}/{:?}",
                node.parent(map).unwrap(),
                node
            ),
            TreeEvent::Copied { src, dest, dest_layout } => {
                if let Some(&wid) = self.windows.get(src) {
                    self.set_window(dest_layout, dest, wid);
                }
            }
            TreeEvent::RemovingFromParent(_) => (),
            TreeEvent::RemovedFromForest(node) => {
                if let Some(wid) = self.windows.remove(node) {
                    if let Some(window_nodes) = self.window_nodes.get_mut(&wid) {
                        window_nodes.0.retain(|info| info.node != node);
                        if window_nodes.0.is_empty() {
                            self.window_nodes.remove(&wid);
                        }
                    }
                }
            }
        }
    }
}
