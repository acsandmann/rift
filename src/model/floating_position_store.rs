use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::model::{VirtualWorkspaceId, WorkspaceStore};
use crate::sys::app::pid_t;
use crate::sys::geometry::CGRectDef;

/// Viewport placement belongs to workspace identity, independent of its native owner.
#[serde_as]
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FloatingPositionStore {
    #[serde_as(as = "HashMap<_, CGRectDef>")]
    positions: HashMap<(VirtualWorkspaceId, WindowId), CGRect>,
}

impl FloatingPositionStore {
    pub(crate) fn validate_persisted(&self, workspaces: &WorkspaceStore) -> Result<(), String> {
        for (&(workspace, window), frame) in &self.positions {
            if !workspaces.workspaces.contains_key(workspace) {
                return Err(format!(
                    "floating frame for window {window:?} references missing workspace {workspace:?}"
                ));
            }
            if !frame.origin.x.is_finite()
                || !frame.origin.y.is_finite()
                || !frame.size.width.is_finite()
                || !frame.size.height.is_finite()
                || frame.size.width < 0.0
                || frame.size.height < 0.0
            {
                return Err(format!("window {window:?} has an invalid floating frame"));
            }
        }
        Ok(())
    }

    /// Every saved frame in a stable order, so a choice between duplicates is deterministic.
    pub(crate) fn into_sorted(self) -> Vec<(VirtualWorkspaceId, WindowId, CGRect)> {
        let mut positions: Vec<_> = self
            .positions
            .into_iter()
            .map(|((workspace, window), frame)| (workspace, window, frame))
            .collect();
        positions.sort_unstable_by_key(|&(workspace, window, _)| (workspace, window));
        positions
    }

    pub(crate) fn positioned_windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.positions.keys().map(|(_, window)| *window)
    }

    pub fn store(&mut self, workspace: VirtualWorkspaceId, window: WindowId, frame: CGRect) {
        self.positions.insert((workspace, window), frame);
    }

    pub fn store_if_absent(
        &mut self,
        workspace: VirtualWorkspaceId,
        window: WindowId,
        frame: CGRect,
    ) {
        self.positions.entry((workspace, window)).or_insert(frame);
    }

    pub fn get(&self, workspace: VirtualWorkspaceId, window: WindowId) -> Option<CGRect> {
        self.positions.get(&(workspace, window)).copied()
    }

    pub fn workspace_positions(&self, workspace: VirtualWorkspaceId) -> Vec<(WindowId, CGRect)> {
        let mut positions: Vec<_> = self
            .positions
            .iter()
            .filter_map(|(&(id, window), &frame)| (id == workspace).then_some((window, frame)))
            .collect();
        positions.sort_unstable_by_key(|(window, _)| *window);
        positions
    }

    pub fn remove_window(&mut self, window: WindowId) {
        self.positions.retain(|(_, stored), _| *stored != window);
    }

    pub(crate) fn remove_workspace_window(
        &mut self,
        workspace: VirtualWorkspaceId,
        window: WindowId,
    ) {
        self.positions.remove(&(workspace, window));
    }

    pub(crate) fn remove_workspace(&mut self, workspace: VirtualWorkspaceId) {
        self.positions.retain(|(stored, _), _| *stored != workspace);
    }

    pub fn remove_app(&mut self, pid: pid_t) {
        self.positions.retain(|(_, window), _| window.pid != pid);
    }

    pub fn transfer_window_identity(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        let frames: Vec<_> = self
            .positions
            .iter()
            .filter_map(|(&(workspace, window), &frame)| {
                (window == from).then_some((workspace, frame))
            })
            .collect();
        self.remove_window(from);
        self.remove_window(to);
        for (workspace, frame) in frames {
            self.store(workspace, to, frame);
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
        positions.store(workspace(), window, frame());
        positions.store(KeyData::from_ffi(2).into(), window, frame());

        positions.remove_window(window);

        assert!(positions.workspace_positions(workspace()).is_empty());
        assert!(positions.workspace_positions(KeyData::from_ffi(2).into()).is_empty());
    }

    #[test]
    fn identity_transfer_moves_saved_frames() {
        let mut positions = FloatingPositionStore::default();
        let old = WindowId::new(42, 1);
        let new = WindowId::new(42, 2);
        positions.store(workspace(), old, frame());

        positions.transfer_window_identity(old, new);

        assert_eq!(positions.get(workspace(), old), None);
        assert_eq!(positions.get(workspace(), new), Some(frame()));
    }
}
