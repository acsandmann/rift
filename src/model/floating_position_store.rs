use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::model::VirtualWorkspaceId;
use crate::sys::app::pid_t;
use crate::sys::geometry::CGRectDef;
use crate::sys::screen::SpaceId;

/// Saved floating frames. This is layout persistence, not workspace catalog
/// state; callers must remove entries as part of the corresponding window
/// lifecycle operation.
#[serde_as]
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FloatingPositionStore {
    #[serde_as(as = "HashMap<_, CGRectDef>")]
    positions: HashMap<(SpaceId, VirtualWorkspaceId, WindowId), CGRect>,
}

impl FloatingPositionStore {
    pub(crate) fn persisted_windows(&self) -> Vec<WindowId> {
        self.positions.keys().map(|(_, _, window)| *window).collect()
    }

    pub fn remap_space(&mut self, old_space: SpaceId, new_space: SpaceId) {
        if old_space == new_space {
            return;
        }
        let old = std::mem::take(&mut self.positions);
        self.positions = old
            .into_iter()
            .filter_map(|((space, workspace, window), frame)| {
                (space != new_space).then_some((
                    (
                        if space == old_space { new_space } else { space },
                        workspace,
                        window,
                    ),
                    frame,
                ))
            })
            .collect();
    }

    pub fn store(
        &mut self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
        window: WindowId,
        frame: CGRect,
    ) {
        self.positions.insert((space, workspace, window), frame);
    }

    pub fn store_if_absent(
        &mut self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
        window: WindowId,
        frame: CGRect,
    ) {
        self.positions.entry((space, workspace, window)).or_insert(frame);
    }

    pub fn get(
        &self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
        window: WindowId,
    ) -> Option<CGRect> {
        self.positions.get(&(space, workspace, window)).copied()
    }

    pub fn workspace_positions(
        &self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
    ) -> Vec<(WindowId, CGRect)> {
        self.positions
            .iter()
            .filter_map(|(&(stored_space, stored_workspace, window), &frame)| {
                (stored_space == space && stored_workspace == workspace).then_some((window, frame))
            })
            .collect()
    }

    pub fn remove_window(&mut self, window: WindowId) {
        self.positions.retain(|(_, _, stored_window), _| *stored_window != window);
    }

    /// Keep at most one persisted location for a window identity.
    ///
    /// Layout restore treats workspace membership as singular. Floating frames must obey the
    /// same invariant or a stale frame can resurrect a window in a different workspace later.
    pub fn retain_window_location(
        &mut self,
        window: WindowId,
        keep: (SpaceId, VirtualWorkspaceId),
    ) {
        self.positions.retain(|(space, workspace, stored_window), _| {
            *stored_window != window || (*space, *workspace) == keep
        });
    }

    /// Replace one target workspace's saved floating frames from a snapshot.
    /// Clearing first is essential: restore is replacement, not a merge with stale target state.
    pub fn replace_workspace_from(
        &mut self,
        source: &Self,
        source_space: SpaceId,
        source_workspace: VirtualWorkspaceId,
        target_space: SpaceId,
        target_workspace: VirtualWorkspaceId,
    ) {
        self.positions.retain(|(space, workspace, _), _| {
            (*space, *workspace) != (target_space, target_workspace)
        });
        for (window, frame) in source.workspace_positions(source_space, source_workspace) {
            self.store(target_space, target_workspace, window, frame);
        }
    }

    pub(crate) fn replace_workspace_positions(
        &mut self,
        target_space: SpaceId,
        target_workspace: VirtualWorkspaceId,
        positions: Vec<(WindowId, CGRect)>,
    ) {
        self.positions.retain(|(space, workspace, _), _| {
            (*space, *workspace) != (target_space, target_workspace)
        });
        for (window, frame) in positions {
            self.store(target_space, target_workspace, window, frame);
        }
    }

    pub fn remove_app(&mut self, pid: pid_t) {
        self.positions.retain(|(_, _, window), _| window.pid != pid);
    }

    pub fn transfer_window_identity(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        let transfers: Vec<_> = self
            .positions
            .iter()
            .filter_map(|(&(space, workspace, window), &frame)| {
                (window == from).then_some((space, workspace, frame))
            })
            .collect();
        self.remove_window(from);
        self.remove_window(to);
        for (space, workspace, frame) in transfers {
            self.store(space, workspace, to, frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};
    use slotmap::KeyData;

    use super::*;

    fn workspace() -> VirtualWorkspaceId { KeyData::from_ffi(1).into() }

    fn frame() -> CGRect { CGRect::new(CGPoint::new(10.0, 20.0), CGSize::new(300.0, 200.0)) }

    #[test]
    fn window_lifecycle_cleanup_removes_all_saved_frames() {
        let mut positions = FloatingPositionStore::default();
        let window = WindowId::new(42, 1);
        positions.store(SpaceId::new(1), workspace(), window, frame());
        positions.store(SpaceId::new(2), workspace(), window, frame());

        positions.remove_window(window);

        assert!(positions.workspace_positions(SpaceId::new(1), workspace()).is_empty());
        assert!(positions.workspace_positions(SpaceId::new(2), workspace()).is_empty());
    }

    #[test]
    fn identity_transfer_moves_saved_frames() {
        let mut positions = FloatingPositionStore::default();
        let old = WindowId::new(42, 1);
        let new = WindowId::new(42, 2);
        positions.store(SpaceId::new(1), workspace(), old, frame());

        positions.transfer_window_identity(old, new);

        assert_eq!(positions.get(SpaceId::new(1), workspace(), old), None);
        assert_eq!(positions.get(SpaceId::new(1), workspace(), new), Some(frame()));
    }

    #[test]
    fn workspace_restore_replaces_target_frames_instead_of_merging() {
        let mut source = FloatingPositionStore::default();
        let mut target = FloatingPositionStore::default();
        let source_space = SpaceId::new(1);
        let target_space = SpaceId::new(2);
        let source_window = WindowId::new(1, 1);
        let stale_target_window = WindowId::new(2, 2);

        source.store(source_space, workspace(), source_window, frame());
        target.store(target_space, workspace(), stale_target_window, frame());
        target.replace_workspace_from(
            &source,
            source_space,
            workspace(),
            target_space,
            workspace(),
        );

        assert_eq!(
            target.get(target_space, workspace(), source_window),
            Some(frame())
        );
        assert_eq!(target.get(target_space, workspace(), stale_target_window), None);
    }
}
