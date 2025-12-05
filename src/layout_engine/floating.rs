use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{BTreeExt, BTreeSet, HashMap, HashSet};
use crate::sys::screen::SpaceId;

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct FloatingManager {
    floating_windows: BTreeSet<WindowId>,
    #[serde(skip)]
    active_floating_windows: HashMap<SpaceId, HashMap<pid_t, HashSet<WindowId>>>,
    last_floating_focus: Option<WindowId>,
}

impl FloatingManager {
    pub(crate) fn new() -> Self { Self::default() }

    pub(crate) fn is_floating(&self, window_id: WindowId) -> bool {
        self.floating_windows.contains(&window_id)
    }

    pub(crate) fn add_floating(&mut self, window_id: WindowId) {
        self.floating_windows.insert(window_id);
    }

    pub(crate) fn remove_floating(&mut self, window_id: WindowId) {
        self.floating_windows.remove(&window_id);

        for space_floating in self.active_floating_windows.values_mut() {
            if let Some(app_set) = space_floating.get_mut(&window_id.pid) {
                app_set.remove(&window_id);
                if app_set.is_empty() {
                    space_floating.remove(&window_id.pid);
                }
            }
        }
        if self.last_floating_focus == Some(window_id) {
            self.last_floating_focus = None;
        }
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

    pub(crate) fn remove_all_for_pid(&mut self, pid: pid_t) {
        let _ = self.floating_windows.remove_all_for_pid(pid);

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
}
