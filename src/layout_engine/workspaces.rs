use objc2_core_foundation::CGSize;
use serde::{Deserialize, Serialize};

use super::{LayoutId, LayoutSystem};
use crate::sys::screen::SpaceId;

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub(crate) struct WorkspaceLayouts {
    map: crate::common::collections::HashMap<
        (SpaceId, crate::model::VirtualWorkspaceId),
        SpaceLayoutInfo,
    >,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SpaceLayoutInfo {
    configurations: crate::common::collections::HashMap<Size, LayoutId>,
    active_size: Size,
    last_saved: Option<LayoutId>,
}

/// Opaque workspace-layout payload used by transactional restore code.
/// Keeping `SpaceLayoutInfo` private prevents persistence from depending on its internal maps.
pub(crate) struct WorkspaceLayoutSnapshot(SpaceLayoutInfo);

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
    pub(crate) fn validate_persisted(
        &self,
        workspaces: &crate::model::WorkspaceStore,
    ) -> Result<(), String> {
        for (&(space, workspace), info) in &self.map {
            let Some(workspace_info) = workspaces.workspaces.get(workspace) else {
                return Err(format!(
                    "layout state references missing workspace {workspace:?}"
                ));
            };
            if workspace_info.space != space {
                return Err(format!(
                    "layout for workspace {workspace:?} is stored under native space {} instead of {}",
                    space.get(),
                    workspace_info.space.get()
                ));
            }
            if info.configurations.is_empty() {
                return Err(format!("workspace {workspace:?} has no layout configurations"));
            }
            if !info.configurations.contains_key(&info.active_size) {
                return Err(format!(
                    "workspace {workspace:?} has no configuration for its active display size"
                ));
            }
            for layout in info.configurations.values().copied().chain(info.last_saved) {
                if !workspace_info.layout_system.contains_layout(layout) {
                    return Err(format!(
                        "workspace {workspace:?} references missing layout {layout:?}"
                    ));
                }
            }
        }

        for space in workspaces.initialized_spaces() {
            for (workspace, _) in workspaces.existing_workspaces(space) {
                if !self.map.contains_key(&(space, workspace)) {
                    return Err(format!(
                        "workspace {workspace:?} on native space {} has no layout state",
                        space.get()
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn snapshot_workspace(
        &self,
        space: SpaceId,
        workspace: crate::model::VirtualWorkspaceId,
    ) -> Option<WorkspaceLayoutSnapshot> {
        self.map.get(&(space, workspace)).cloned().map(WorkspaceLayoutSnapshot)
    }

    pub(crate) fn install_workspace_snapshot(
        &mut self,
        space: SpaceId,
        workspace: crate::model::VirtualWorkspaceId,
        snapshot: WorkspaceLayoutSnapshot,
    ) {
        self.map.insert((space, workspace), snapshot.0);
    }

    pub(crate) fn contains_workspace(
        &self,
        space: SpaceId,
        workspace: crate::model::VirtualWorkspaceId,
    ) -> bool {
        self.map.contains_key(&(space, workspace))
    }

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
        let mut layouts = self
            .map
            .iter()
            .filter_map(|(&(sp, ws), info)| {
                if sp == space {
                    info.active().map(|l| (ws, l))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        layouts.sort_unstable();
        layouts
    }

    /// Enumerate every serialized layout configuration, not only the currently active display
    /// size. Old-size configurations are restored later and therefore must be sanitized too.
    pub(crate) fn all_layouts(&self) -> Vec<(SpaceId, crate::model::VirtualWorkspaceId, LayoutId)> {
        let mut layouts = Vec::new();
        for (&(space, workspace), info) in &self.map {
            layouts.extend(info.configurations.values().map(|layout| (space, workspace, *layout)));
            if let Some(layout) = info.last_saved {
                layouts.push((space, workspace, layout));
            }
        }
        layouts.sort_unstable();
        layouts.dedup();
        layouts
    }

    #[cfg(test)]
    pub(crate) fn insert_layout_configuration_for_test(
        &mut self,
        space: SpaceId,
        workspace: crate::model::VirtualWorkspaceId,
        size: CGSize,
        layout: LayoutId,
    ) {
        self.map
            .get_mut(&(space, workspace))
            .expect("test workspace must be initialized")
            .configurations
            .insert(Size::from(size), layout);
    }

    pub(crate) fn ensure_active_for_workspace(
        &mut self,
        space: SpaceId,
        size: CGSize,
        workspace_id: crate::model::VirtualWorkspaceId,
        tree: &mut impl LayoutSystem,
    ) {
        self.ensure_active_for_space(space, size, std::iter::once(workspace_id), tree);
    }

    pub(crate) fn replace_layouts_for_workspace(
        &mut self,
        space: SpaceId,
        workspace_id: crate::model::VirtualWorkspaceId,
        new_layout: LayoutId,
    ) {
        let active_size = self
            .map
            .get(&(space, workspace_id))
            .map(|info| info.active_size)
            .unwrap_or_else(|| Size::from(CGSize::new(1000.0, 1000.0)));

        let mut configurations = crate::common::collections::HashMap::default();
        configurations.insert(active_size, new_layout);

        self.map.insert((space, workspace_id), SpaceLayoutInfo {
            configurations,
            active_size,
            last_saved: Some(new_layout),
        });
    }

    pub(crate) fn spaces(&self) -> crate::common::collections::BTreeSet<SpaceId> {
        self.map.keys().map(|(sp, _)| *sp).collect()
    }
}
