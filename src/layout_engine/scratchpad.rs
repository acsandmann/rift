use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ScratchpadManager {
    windows: VecDeque<WindowId>,
    #[serde(default)]
    names: HashMap<WindowId, String>,
    #[serde(skip)]
    active_windows: HashSet<WindowId>,
}

impl ScratchpadManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_scratchpad(&self, window: WindowId) -> bool {
        self.windows.contains(&window)
    }

    pub fn is_active(&self, window: WindowId) -> bool {
        self.active_windows.contains(&window)
    }

    pub fn set_active(&mut self, window: WindowId, active: bool) {
        if active {
            self.active_windows.insert(window);
        } else {
            self.active_windows.remove(&window);
        }
    }

    pub fn add(&mut self, window: WindowId, name: Option<String>) {
        if !self.windows.contains(&window) {
            self.windows.push_back(window);
        }
        if let Some(n) = name {
            self.names.insert(window, n);
        }
    }

    pub fn remove(&mut self, window: WindowId) {
        if let Some(pos) = self.windows.iter().position(|&w| w == window) {
            self.windows.remove(pos);
        }
        self.names.remove(&window);
        self.active_windows.remove(&window);
    }

    pub fn remove_for_app(&mut self, pid: i32) {
        self.windows.retain(|w| w.pid != pid);
        self.names.retain(|w, _| w.pid != pid);
        self.active_windows.retain(|w| w.pid != pid);
    }

    pub fn get_by_name(&self, name: &str) -> Option<WindowId> {
        // Find the most recently added window with this name? Or just any?
        // Since VecDeque is ordered, we can search in reverse to find top-most?
        // But names map is unordered.
        // Let's iterate windows (ordered) and check names.
        self.windows
            .iter()
            .find(|&&w| self.names.get(&w).map(|s| s.as_str()) == Some(name))
            .copied()
    }

    pub fn get_name(&self, window: WindowId) -> Option<&String> {
        self.names.get(&window)
    }

    pub fn next(&self) -> Option<WindowId> {
        self.windows.front().cloned()
    }

    pub fn cycle(&mut self) {
        if let Some(w) = self.windows.pop_front() {
            self.windows.push_back(w);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &WindowId> {
        self.windows.iter()
    }
}
