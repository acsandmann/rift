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

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;

    fn make_wid(pid: i32, idx: u32) -> WindowId {
        WindowId {
            pid,
            idx: NonZero::new(idx).unwrap(),
        }
    }

    #[test]
    fn test_add_and_is_scratchpad() {
        let mut mgr = ScratchpadManager::new();
        let wid = make_wid(100, 1);

        assert!(!mgr.is_scratchpad(wid));
        mgr.add(wid, None);
        assert!(mgr.is_scratchpad(wid));
    }

    #[test]
    fn test_add_with_name() {
        let mut mgr = ScratchpadManager::new();
        let wid = make_wid(100, 1);

        mgr.add(wid, Some("terminal".to_string()));
        assert!(mgr.is_scratchpad(wid));
        assert_eq!(mgr.get_name(wid), Some(&"terminal".to_string()));
    }

    #[test]
    fn test_add_duplicate_does_not_duplicate() {
        let mut mgr = ScratchpadManager::new();
        let wid = make_wid(100, 1);

        mgr.add(wid, None);
        mgr.add(wid, None);
        assert_eq!(mgr.windows.len(), 1);
    }

    #[test]
    fn test_remove() {
        let mut mgr = ScratchpadManager::new();
        let wid = make_wid(100, 1);

        mgr.add(wid, Some("test".to_string()));
        mgr.set_active(wid, true);
        assert!(mgr.is_scratchpad(wid));
        assert!(mgr.is_active(wid));

        mgr.remove(wid);
        assert!(!mgr.is_scratchpad(wid));
        assert!(!mgr.is_active(wid));
        assert!(mgr.get_name(wid).is_none());
    }

    #[test]
    fn test_remove_for_app() {
        let mut mgr = ScratchpadManager::new();
        let wid1 = make_wid(100, 1);
        let wid2 = make_wid(100, 2);
        let wid3 = make_wid(200, 1);

        mgr.add(wid1, Some("a".to_string()));
        mgr.add(wid2, Some("b".to_string()));
        mgr.add(wid3, Some("c".to_string()));
        mgr.set_active(wid1, true);
        mgr.set_active(wid2, true);
        mgr.set_active(wid3, true);

        mgr.remove_for_app(100);

        assert!(!mgr.is_scratchpad(wid1));
        assert!(!mgr.is_scratchpad(wid2));
        assert!(mgr.is_scratchpad(wid3));
        assert!(!mgr.is_active(wid1));
        assert!(!mgr.is_active(wid2));
        assert!(mgr.is_active(wid3));
    }

    #[test]
    fn test_active_state() {
        let mut mgr = ScratchpadManager::new();
        let wid = make_wid(100, 1);

        mgr.add(wid, None);
        assert!(!mgr.is_active(wid));

        mgr.set_active(wid, true);
        assert!(mgr.is_active(wid));

        mgr.set_active(wid, false);
        assert!(!mgr.is_active(wid));
    }

    #[test]
    fn test_get_by_name() {
        let mut mgr = ScratchpadManager::new();
        let wid1 = make_wid(100, 1);
        let wid2 = make_wid(100, 2);

        mgr.add(wid1, Some("terminal".to_string()));
        mgr.add(wid2, Some("notes".to_string()));

        assert_eq!(mgr.get_by_name("terminal"), Some(wid1));
        assert_eq!(mgr.get_by_name("notes"), Some(wid2));
        assert_eq!(mgr.get_by_name("nonexistent"), None);
    }

    #[test]
    fn test_next_returns_front() {
        let mut mgr = ScratchpadManager::new();
        let wid1 = make_wid(100, 1);
        let wid2 = make_wid(100, 2);

        assert!(mgr.next().is_none());

        mgr.add(wid1, None);
        mgr.add(wid2, None);

        assert_eq!(mgr.next(), Some(wid1));
    }

    #[test]
    fn test_cycle() {
        let mut mgr = ScratchpadManager::new();
        let wid1 = make_wid(100, 1);
        let wid2 = make_wid(100, 2);
        let wid3 = make_wid(100, 3);

        mgr.add(wid1, None);
        mgr.add(wid2, None);
        mgr.add(wid3, None);

        assert_eq!(mgr.next(), Some(wid1));

        mgr.cycle();
        assert_eq!(mgr.next(), Some(wid2));

        mgr.cycle();
        assert_eq!(mgr.next(), Some(wid3));

        mgr.cycle();
        assert_eq!(mgr.next(), Some(wid1));
    }

    #[test]
    fn test_cycle_empty_does_not_panic() {
        let mut mgr = ScratchpadManager::new();
        mgr.cycle(); // should not panic
        assert!(mgr.next().is_none());
    }

    #[test]
    fn test_iter() {
        let mut mgr = ScratchpadManager::new();
        let wid1 = make_wid(100, 1);
        let wid2 = make_wid(100, 2);

        mgr.add(wid1, None);
        mgr.add(wid2, None);

        let collected: Vec<_> = mgr.iter().copied().collect();
        assert_eq!(collected, vec![wid1, wid2]);
    }

    #[test]
    fn test_add_updates_name_if_provided() {
        let mut mgr = ScratchpadManager::new();
        let wid = make_wid(100, 1);

        mgr.add(wid, Some("old".to_string()));
        assert_eq!(mgr.get_name(wid), Some(&"old".to_string()));

        mgr.add(wid, Some("new".to_string()));
        assert_eq!(mgr.get_name(wid), Some(&"new".to_string()));
    }

    #[test]
    fn test_multiple_windows_same_name() {
        let mut mgr = ScratchpadManager::new();
        let wid1 = make_wid(100, 1);
        let wid2 = make_wid(100, 2);

        mgr.add(wid1, Some("term".to_string()));
        mgr.add(wid2, Some("term".to_string()));

        // get_by_name returns first window with that name in order
        assert_eq!(mgr.get_by_name("term"), Some(wid1));
    }
}
