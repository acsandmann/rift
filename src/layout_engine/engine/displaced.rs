//! Windows macOS carries off a display that goes away.
//!
//! When a display disappears, the window server moves its windows onto a surviving display and
//! topology reconciliation must follow them there. Each such window remembers the workspace it
//! came from and stays in that workspace's tree, so when the display returns the windows go back
//! to their own workspaces and the trees come back exactly as they were.

use serde::{Deserialize, Serialize};

use super::LayoutEngine;
use crate::actor::app::WindowId;
use crate::layout_engine::LayoutSystem;
use crate::model::{VirtualWorkspaceId, WindowStore};
use crate::sys::screen::SpaceId;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(super) struct Displaced {
    #[serde(with = "raw_space")]
    pub(super) space: SpaceId,
    pub(super) workspace: VirtualWorkspaceId,
    /// The workspace holding the window while it is away. `None` once it has been sent home and
    /// the window server has yet to confirm it arrived.
    #[serde(default)]
    pub(super) host: Option<VirtualWorkspaceId>,
}

/// Persisted inside flattened state, which loses RON newtype structs, like `saved_active_space`.
mod raw_space {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::sys::screen::SpaceId;

    pub(super) fn serialize<S: Serializer>(space: &SpaceId, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(space.get())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SpaceId, D::Error> {
        u64::deserialize(d).map(SpaceId::new)
    }
}

impl Displaced {
    pub(super) fn home(&self) -> (SpaceId, VirtualWorkspaceId) { (self.space, self.workspace) }
}

impl LayoutEngine {
    /// Records that `wid`, which belongs to `home`, was carried off its departed display and is
    /// now assigned wherever the window store has it.
    pub(crate) fn record_displaced_window(
        &mut self,
        window_store: &WindowStore,
        wid: WindowId,
        home: (SpaceId, VirtualWorkspaceId),
    ) {
        let host = window_store.workspace_info_for_window(wid).map(|info| info.workspace_id);
        let entry = self.persistence.displaced.entry(wid).or_insert(Displaced {
            space: home.0,
            workspace: home.1,
            host: None,
        });
        entry.host = host;
    }

    /// Whether `wid` was sent home to `space` and is waiting for the window server to move it.
    pub(crate) fn is_returning_home(&self, wid: WindowId, space: SpaceId) -> bool {
        self.persistence
            .displaced
            .get(&wid)
            .is_some_and(|entry| entry.host.is_none() && entry.space == space)
    }

    /// The window server reports `wid` on `space`, its assigned space. A window sent home there
    /// has arrived.
    pub(crate) fn confirm_window_space(&mut self, wid: WindowId, space: SpaceId) {
        if self.is_returning_home(wid, space) {
            self.persistence.displaced.remove(&wid);
        }
    }

    /// Sends windows home to workspaces on `spaces`, which are on screen again. Returns each
    /// window sent home with its space.
    pub(crate) fn return_displaced_windows(
        &mut self,
        window_store: &mut WindowStore,
        spaces: &[SpaceId],
    ) -> Vec<(WindowId, SpaceId)> {
        let arriving: Vec<_> = self
            .persistence
            .displaced
            .iter()
            .filter(|(_, entry)| entry.host.is_some() && spaces.contains(&entry.space))
            .map(|(&wid, entry)| (wid, entry.space))
            .collect();
        arriving
            .into_iter()
            .filter(|&(wid, _)| self.return_displaced_window(window_store, wid))
            .collect()
    }

    /// Assigns an away window back to its home workspace, out of the one holding it meanwhile.
    /// Returns false, dropping the record, if it was placed elsewhere since: that choice stands.
    pub(crate) fn return_displaced_window(
        &mut self,
        window_store: &mut WindowStore,
        wid: WindowId,
    ) -> bool {
        let Some(entry) = self.persistence.displaced.get(&wid).filter(|entry| entry.host.is_some())
        else {
            return false;
        };
        let entry = *entry;
        let (space, workspace) = entry.home();
        let current = window_store.workspace_info_for_window(wid).map(|info| info.workspace_id);
        let home_exists =
            self.workspaces.workspaces.get(workspace).is_some_and(|ws| ws.space == space);
        if current != entry.host || !home_exists {
            self.forget_displaced_window(wid, current != Some(workspace));
            return false;
        }
        self.remove_window_internal(window_store, wid, true);
        if self.floating.is_floating(wid) {
            // It may have been floated while away. Its home slot was only a placeholder;
            // keeping it after return would still allocate tiled space to the floating window.
            self.workspaces[workspace].layout_system.remove_window_and_rebalance_parent(wid);
        }
        self.workspaces.assign_window_to_workspace(window_store, space, wid, workspace);
        if let Some(entry) = self.persistence.displaced.get_mut(&wid) {
            entry.host = None;
        }
        true
    }

    /// Drops the record for `wid`, and its slot in the home tree unless it is back in it.
    pub(crate) fn forget_displaced_window(&mut self, wid: WindowId, remove_from_home: bool) {
        let Some(entry) = self.persistence.displaced.remove(&wid) else {
            return;
        };
        if remove_from_home
            && let Some(workspace) = self.workspaces.workspaces.get_mut(entry.workspace)
        {
            workspace.layout_system.remove_window_and_rebalance_parent(wid);
        }
    }

    /// The home of a window that is away. Its tree keeps the window without showing it.
    pub(crate) fn away_home(&self, wid: WindowId) -> Option<(SpaceId, VirtualWorkspaceId)> {
        self.persistence
            .displaced
            .get(&wid)
            .filter(|entry| entry.host.is_some())
            .map(Displaced::home)
    }

    /// Where an away window is held until its display returns.
    pub(super) fn away_host(&self, wid: WindowId) -> Option<(SpaceId, VirtualWorkspaceId)> {
        let host = self.persistence.displaced.get(&wid)?.host?;
        Some((self.workspaces.workspaces.get(host)?.space, host))
    }

    pub(super) fn remap_displaced_space(&mut self, old_space: SpaceId, new_space: SpaceId) {
        for entry in self.persistence.displaced.values_mut() {
            if entry.space == old_space {
                entry.space = new_space;
            }
        }
    }
}
