use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{BTreeExt, BTreeSet, HashMap, HashSet};
use crate::sys::screen::SpaceId;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FloatingFullscreenKind {
    Full,
    WithinGaps,
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct FloatingManager {
    floating_windows: BTreeSet<WindowId>,
    #[serde(skip)]
    active_floating_windows: HashMap<SpaceId, HashMap<pid_t, HashSet<WindowId>>>,
    last_floating_focus: Option<WindowId>,
    #[serde(skip)]
    fullscreen_windows: HashMap<WindowId, FloatingFullscreenKind>,
}

impl FloatingManager {
    pub(crate) fn new() -> Self { Self::default() }

    pub(crate) fn is_floating(&self, window_id: WindowId) -> bool {
        self.floating_windows.contains(&window_id)
    }

    pub(crate) fn persisted_windows(&self) -> Vec<WindowId> {
        self.floating_windows.iter().copied().collect()
    }

    pub(crate) fn add_floating(&mut self, window_id: WindowId) {
        self.floating_windows.insert(window_id);
    }

    pub(crate) fn remove_floating(&mut self, window_id: WindowId) {
        self.floating_windows.remove(&window_id);
        self.fullscreen_windows.remove(&window_id);
        self.remove_active_entries(window_id);
        if self.last_floating_focus == Some(window_id) {
            self.last_floating_focus = None;
        }
    }

    pub(crate) fn set_fullscreen(
        &mut self,
        window_id: WindowId,
        kind: Option<FloatingFullscreenKind>,
    ) {
        match kind {
            Some(k) => {
                self.fullscreen_windows.insert(window_id, k);
            }
            None => {
                self.fullscreen_windows.remove(&window_id);
            }
        }
    }

    pub(crate) fn fullscreen_kind(&self, window_id: WindowId) -> Option<FloatingFullscreenKind> {
        self.fullscreen_windows.get(&window_id).copied()
    }

    pub(crate) fn clear_active_for_app(&mut self, space: SpaceId, pid: pid_t) {
        if let Some(space_map) = self.active_floating_windows.get_mut(&space) {
            space_map.remove(&pid);
        }
    }

    pub(crate) fn add_active(&mut self, space: SpaceId, pid: pid_t, wid: WindowId) {
        self.active_floating_windows
            .entry(space)
            .or_default()
            .entry(pid)
            .or_default()
            .insert(wid);
    }

    pub(crate) fn remove_active(&mut self, space: SpaceId, pid: pid_t, wid: WindowId) {
        if let Some(space_map) = self.active_floating_windows.get_mut(&space) {
            if let Some(app_set) = space_map.get_mut(&pid) {
                app_set.remove(&wid);
                if app_set.is_empty() {
                    space_map.remove(&pid);
                }
            }
        }
    }

    pub(crate) fn remove_active_for_window(&mut self, window_id: WindowId) {
        self.remove_active_entries(window_id);
    }

    pub(crate) fn transfer_window_identity(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }

        // Identity transfer is replacement, not union. `to` may already have provisional live
        // state while `from` carries restored state. Keeping both lets stale floating/fullscreen
        // flags survive reconciliation and disagree with the restored workspace tree.
        let was_floating = self.floating_windows.remove(&from);
        self.floating_windows.remove(&to);
        if was_floating {
            self.floating_windows.insert(to);
        }

        let fullscreen = self.fullscreen_windows.remove(&from);
        self.fullscreen_windows.remove(&to);
        if let Some(k) = fullscreen {
            self.fullscreen_windows.insert(to, k);
        }

        let active_spaces: Vec<_> = self
            .active_floating_windows
            .iter()
            .filter_map(|(space, apps)| {
                apps.get(&from.pid)
                    .is_some_and(|windows| windows.contains(&from))
                    .then_some(*space)
            })
            .collect();
        self.remove_active_entries(from);
        self.remove_active_entries(to);
        for space in active_spaces {
            self.add_active(space, to.pid, to);
        }

        if self.last_floating_focus == Some(from) {
            self.last_floating_focus = Some(to);
        }
    }

    pub(crate) fn active_flat(&self, space: SpaceId) -> Vec<WindowId> {
        self.active_floating_windows
            .get(&space)
            .map(|space_floating| space_floating.values().flatten().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn set_last_focus(&mut self, wid: Option<WindowId>) {
        self.last_floating_focus = wid;
    }

    pub(crate) fn last_focus(&self) -> Option<WindowId> { self.last_floating_focus }

    pub(crate) fn normalize_persisted_focus(&mut self) {
        if self
            .last_floating_focus
            .is_some_and(|window| !self.floating_windows.contains(&window))
        {
            self.last_floating_focus = None;
        }
    }

    pub(crate) fn remove_all_for_pid(&mut self, pid: pid_t) {
        let _ = self.floating_windows.remove_all_for_pid(pid);

        self.fullscreen_windows.retain(|w, _| w.pid != pid);

        for space_map in self.active_floating_windows.values_mut() {
            space_map.remove(&pid);
        }

        if let Some(focus) = self.last_floating_focus {
            if focus.pid == pid {
                self.last_floating_focus = None;
            }
        }
    }

    pub(crate) fn rebuild_active_for_workspace(
        &mut self,
        space: SpaceId,
        windows_in_workspace: Vec<WindowId>,
    ) {
        let space_map = self.active_floating_windows.entry(space).or_default();
        space_map.clear();
        for wid in windows_in_workspace.into_iter().filter(|&w| self.floating_windows.contains(&w))
        {
            space_map.entry(wid.pid).or_default().insert(wid);
        }
    }

    pub(crate) fn remap_space(&mut self, old_space: SpaceId, new_space: SpaceId) {
        if old_space == new_space {
            return;
        }

        let mut merged = self.active_floating_windows.remove(&new_space).unwrap_or_default();

        if let Some(old) = self.active_floating_windows.remove(&old_space) {
            for (pid, windows) in old {
                merged.entry(pid).or_default().extend(windows);
            }
        }

        if !merged.is_empty() {
            self.active_floating_windows.insert(new_space, merged);
        }
    }

    fn remove_active_entries(&mut self, window_id: WindowId) {
        for space_map in self.active_floating_windows.values_mut() {
            if let Some(app_set) = space_map.get_mut(&window_id.pid) {
                app_set.remove(&window_id);
                if app_set.is_empty() {
                    space_map.remove(&window_id.pid);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transfer_replaces_provisional_target_state() {
        let mut floating = FloatingManager::new();
        let restored_tiled = WindowId::new(1, 1);
        let provisional_live = WindowId::new(2, 2);
        let space = SpaceId::new(3);

        floating.add_floating(provisional_live);
        floating.set_fullscreen(provisional_live, Some(FloatingFullscreenKind::Full));
        floating.add_active(space, provisional_live.pid, provisional_live);

        floating.transfer_window_identity(restored_tiled, provisional_live);

        assert!(!floating.is_floating(provisional_live));
        assert_eq!(floating.fullscreen_kind(provisional_live), None);
        assert!(!floating.active_flat(space).contains(&provisional_live));
    }
}
