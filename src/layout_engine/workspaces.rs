use objc2_core_foundation::CGSize;
use serde::{Deserialize, Serialize};

use super::{LayoutId, LayoutSystem};
use crate::sys::screen::SpaceId;

#[derive(Serialize, Deserialize, Debug, Default)]
pub(crate) struct WorkspaceLayouts {
    map: crate::common::collections::HashMap<
        (SpaceId, crate::model::VirtualWorkspaceId),
        SpaceLayoutInfo,
    >,
}

#[derive(Serialize, Deserialize, Debug)]
struct SpaceLayoutInfo {
    configurations: crate::common::collections::HashMap<Size, LayoutId>,
    active_size: Size,
    last_saved: Option<LayoutId>,
}

impl SpaceLayoutInfo {
    fn active(&self) -> Option<LayoutId> { self.configurations.get(&self.active_size).copied() }
}

#[derive(Serialize, Deserialize, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Debug)]
pub(crate) struct Size {
    width: i32,
    height: i32,
}

impl From<CGSize> for Size {
    fn from(value: CGSize) -> Self {
        Self {
            width: value.width.round() as i32,
            height: value.height.round() as i32,
        }
    }
}

impl WorkspaceLayouts {
    pub(crate) fn ensure_active_for_space(
        &mut self,
        space: SpaceId,
        size: CGSize,
        workspaces: impl IntoIterator<Item = crate::model::VirtualWorkspaceId>,
        tree: &mut impl LayoutSystem,
    ) {
        let size = Size::from(size);
        for workspace_id in workspaces {
            let workspace_key = (space, workspace_id);
            let (workspace_layout, mut unchanged) = match self.map.entry(workspace_key) {
                crate::common::collections::hash_map::Entry::Vacant(entry) => (
                    entry.insert(SpaceLayoutInfo {
                        active_size: size,
                        configurations: Default::default(),
                        last_saved: None,
                    }),
                    None,
                ),
                crate::common::collections::hash_map::Entry::Occupied(entry) => {
                    let info = entry.into_mut();
                    let old_size = info.active_size;
                    if old_size != size {
                        if let Some(active_layout) = info.active() {
                            info.configurations.entry(old_size).or_insert(active_layout);
                        }
                        let taken = info.configurations.remove(&old_size);
                        info.active_size = size;
                        (info, taken)
                    } else {
                        (info, None)
                    }
                }
            };

            let layout = match workspace_layout.configurations.entry(size) {
                crate::common::collections::hash_map::Entry::Vacant(entry) => {
                    *entry.insert(if let Some(source) = unchanged.take() {
                        source
                    } else if let Some(source) = workspace_layout.last_saved {
                        tree.clone_layout(source)
                    } else {
                        tree.create_layout()
                    })
                }
                crate::common::collections::hash_map::Entry::Occupied(entry) => {
                    workspace_layout.last_saved = Some(*entry.get());
                    *entry.get()
                }
            };

            if let Some(removed) = unchanged {
                tree.remove_layout(removed);
            }

            tracing::debug!(
                "Using layout {:?} for workspace {:?} on space {:?}",
                layout,
                workspace_id,
                space
            );
        }
    }

    pub(crate) fn remap_space(&mut self, old_space: SpaceId, new_space: SpaceId) {
        if old_space == new_space {
            return;
        }

        let old_keys: Vec<_> =
            self.map.keys().filter(|(space, _)| *space == old_space).cloned().collect();

        if old_keys.is_empty() {
            return;
        }

        // Prefer the migrated state over anything already associated with the
        // new space (e.g. default layouts created after a reconnect).
        self.map.retain(|(space, _), _| *space != new_space);

        for (space, workspace_id) in old_keys {
            if let Some(info) = self.map.remove(&(space, workspace_id)) {
                self.map.insert((new_space, workspace_id), info);
            }
        }
    }

    pub(crate) fn active(
        &self,
        space: SpaceId,
        workspace_id: crate::model::VirtualWorkspaceId,
    ) -> Option<LayoutId> {
        self.map.get(&(space, workspace_id)).and_then(|l| l.active())
    }

    pub(crate) fn mark_last_saved(
        &mut self,
        space: SpaceId,
        workspace_id: crate::model::VirtualWorkspaceId,
        layout: LayoutId,
    ) {
        if let Some(info) = self.map.get_mut(&(space, workspace_id)) {
            info.last_saved = Some(layout);
        }
    }

    pub(crate) fn active_layouts_for_space(
        &self,
        space: SpaceId,
    ) -> Vec<(crate::model::VirtualWorkspaceId, LayoutId)> {
        self.map
            .iter()
            .filter_map(|(&(sp, ws), info)| {
                if sp == space {
                    info.active().map(|l| (ws, l))
                } else {
                    None
                }
            })
            .collect()
    }

    pub(crate) fn for_each_active(&self, mut f: impl FnMut(LayoutId)) {
        for info in self.map.values() {
            if let Some(l) = info.active() {
                f(l);
            }
        }
    }

    pub(crate) fn spaces(&self) -> crate::common::collections::BTreeSet<SpaceId> {
        self.map.keys().map(|(sp, _)| *sp).collect()
    }
}
