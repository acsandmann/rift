use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};

/// i3-style scratchpad membership. Parked windows live on no virtual workspace; shown
/// members are ordinary floating windows on whichever workspace they were summoned to.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Scratchpad {
    /// Hidden members, least recently parked first; `toggle` shows the front.
    parked: VecDeque<WindowId>,
    shown: BTreeSet<WindowId>,
    /// Parked while tiled and not shown since; first show applies the default geometry.
    fresh: BTreeSet<WindowId>,
}

impl Scratchpad {
    pub(crate) fn is_member(&self, wid: WindowId) -> bool {
        self.is_parked(wid) || self.is_shown(wid)
    }

    pub(crate) fn is_parked(&self, wid: WindowId) -> bool { self.parked.contains(&wid) }

    pub(crate) fn is_shown(&self, wid: WindowId) -> bool { self.shown.contains(&wid) }

    pub(crate) fn park(&mut self, wid: WindowId, was_tiled: bool) {
        self.shown.remove(&wid);
        if !self.parked.contains(&wid) {
            self.parked.push_back(wid);
        }
        if was_tiled {
            self.fresh.insert(wid);
        }
    }

    /// Marks `wid` shown. Returns true when this is its first show after a tiled park.
    pub(crate) fn show(&mut self, wid: WindowId) -> bool {
        self.parked.retain(|w| *w != wid);
        self.shown.insert(wid);
        self.fresh.remove(&wid)
    }

    pub(crate) fn parked(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.parked.iter().copied()
    }

    pub(crate) fn shown(&self) -> impl Iterator<Item = WindowId> + '_ { self.shown.iter().copied() }

    pub(crate) fn remove(&mut self, wid: WindowId) {
        self.parked.retain(|w| *w != wid);
        self.shown.remove(&wid);
        self.fresh.remove(&wid);
    }

    pub(crate) fn remove_for_pid(&mut self, pid: pid_t) { self.retain(|w| w.pid != pid); }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(WindowId) -> bool) {
        self.parked.retain(|w| keep(*w));
        self.shown.retain(|w| keep(*w));
        self.fresh.retain(|w| keep(*w));
    }

    pub(crate) fn transfer_identity(&mut self, from: WindowId, to: WindowId) {
        for w in self.parked.iter_mut().filter(|w| **w == from) {
            *w = to;
        }
        if self.shown.remove(&from) {
            self.shown.insert(to);
        }
        if self.fresh.remove(&from) {
            self.fresh.insert(to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wid(pid: pid_t, idx: u32) -> WindowId { WindowId::new(pid, idx) }

    #[test]
    fn park_makes_window_a_parked_member() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), false);
        assert!(pad.is_member(wid(1, 1)));
        assert!(pad.is_parked(wid(1, 1)));
        assert!(!pad.is_shown(wid(1, 1)));
        assert_eq!(pad.parked().collect::<Vec<_>>(), vec![wid(1, 1)]);
    }

    #[test]
    fn show_moves_window_from_parked_to_shown() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), false);
        pad.show(wid(1, 1));
        assert!(pad.is_shown(wid(1, 1)));
        assert!(!pad.is_parked(wid(1, 1)));
        assert_eq!(pad.parked().next(), None);
        assert_eq!(pad.shown().collect::<Vec<_>>(), vec![wid(1, 1)]);
    }

    #[test]
    fn show_is_fresh_exactly_once_after_a_tiled_park() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), true);
        assert!(pad.show(wid(1, 1)));
        pad.park(wid(1, 1), false);
        assert!(!pad.show(wid(1, 1)));
    }

    #[test]
    fn show_is_not_fresh_after_a_floating_park() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), false);
        assert!(!pad.show(wid(1, 1)));
    }

    #[test]
    fn parked_iterates_least_recently_parked_first() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), false);
        pad.park(wid(1, 2), false);
        assert_eq!(pad.parked().next(), Some(wid(1, 1)));
        pad.show(wid(1, 1));
        pad.park(wid(1, 1), false);
        assert_eq!(pad.parked().next(), Some(wid(1, 2)));
    }

    #[test]
    fn parking_an_already_parked_window_keeps_its_queue_position() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), false);
        pad.park(wid(1, 2), false);
        pad.park(wid(1, 1), false);
        assert_eq!(pad.parked().collect::<Vec<_>>(), vec![wid(1, 1), wid(1, 2)]);
    }

    #[test]
    fn remove_clears_all_membership() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), true);
        pad.remove(wid(1, 1));
        assert!(!pad.is_member(wid(1, 1)));
        pad.park(wid(1, 1), false);
        assert!(!pad.show(wid(1, 1)), "fresh flag must not survive removal");
    }

    #[test]
    fn remove_for_pid_drops_only_that_apps_windows() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), true);
        pad.park(wid(2, 1), false);
        pad.show(wid(2, 1));
        pad.park(wid(2, 2), false);
        pad.remove_for_pid(2);
        assert_eq!(pad.parked().collect::<Vec<_>>(), vec![wid(1, 1)]);
        assert_eq!(pad.shown().count(), 0);
    }

    #[test]
    fn transfer_identity_preserves_state_and_queue_order() {
        let mut pad = Scratchpad::default();
        pad.park(wid(1, 1), true);
        pad.park(wid(1, 2), false);
        pad.park(wid(1, 3), false);
        pad.show(wid(1, 3));
        pad.transfer_identity(wid(1, 1), wid(9, 1));
        pad.transfer_identity(wid(1, 3), wid(9, 3));
        assert_eq!(pad.parked().collect::<Vec<_>>(), vec![wid(9, 1), wid(1, 2)]);
        assert!(pad.is_shown(wid(9, 3)));
        assert!(!pad.is_member(wid(1, 1)));
        assert!(!pad.is_member(wid(1, 3)));
        assert!(pad.show(wid(9, 1)), "fresh flag follows the new identity");
    }
}
