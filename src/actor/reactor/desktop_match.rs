//! Matching the desktops Rift knew against the ones the window server lists now.
//!
//! Space ids do not survive a reconfiguration: macOS destroys desktops, mints new
//! ones for a return, and moves a desktop between displays as a matter of course.
//! What does survive is the windows. A desktop holding Safari and three TextEdits
//! before a display left is the desktop holding them after it returns, whatever
//! number the window server has given it in between. So this matches on content,
//! scores every candidate pairing, and takes the best.
//!
//! It is free of time, the reactor, and window server queries: it takes two lists
//! and returns a mapping, so the awkward cases can be tested directly. Ported from
//! the rift-plus fork.

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::screen::SpaceId;

/// A desktop and the windows on it, as recorded or as reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Desktop {
    pub(super) space: SpaceId,
    /// The display this desktop belonged to, by UUID. A desktop usually comes
    /// back to the display it left, so this breaks ties — but only ties: a
    /// desktop whose windows are plainly somewhere else has moved, and saying
    /// otherwise is how windows end up stranded on a display that is gone.
    pub(super) display: Option<String>,
    /// Position within that display's desktops, from the left.
    pub(super) index: usize,
    pub(super) windows: Vec<WindowId>,
}

/// How good a pairing is, in units that can be compared but not usefully
/// interpreted. Only the ordering matters.
type Score = i64;

/// Content agreement dominates everything else: a desktop that still holds the
/// same four windows is the same desktop even if it has changed display and
/// position.
const PER_SHARED_WINDOW: Score = 100;
/// A window that was there and is not costs less than a shared one gains, so a
/// partial match still beats no match. Windows legitimately move.
const PER_MISSING_WINDOW: Score = -30;
/// As does one that is there and was not.
const PER_EXTRA_WINDOW: Score = -30;
/// Tie-breaks, worth less than a single shared window so they can never
/// outvote content.
const SAME_DISPLAY: Score = 40;
const SAME_INDEX: Score = 20;
const SAME_SPACE_ID: Score = 10;
/// Two empty desktops have no content to agree on. Pairing them on position
/// alone is usually right and never costly — neither holds anything to lose.
const BOTH_EMPTY: Score = 30;

/// Below this, a pairing is worse than leaving both sides unmatched. A single
/// shared window clears it; nothing else does on its own, so a desktop is
/// never claimed by position alone when it has content that disagrees.
const FLOOR: Score = 60;

fn score(was: &Desktop, now: &Desktop) -> Score {
    let before: HashSet<WindowId> = was.windows.iter().copied().collect();
    let after: HashSet<WindowId> = now.windows.iter().copied().collect();

    let shared = before.intersection(&after).count() as Score;
    let missing = before.difference(&after).count() as Score;
    let extra = after.difference(&before).count() as Score;

    let mut total =
        shared * PER_SHARED_WINDOW + missing * PER_MISSING_WINDOW + extra * PER_EXTRA_WINDOW;

    if before.is_empty() && after.is_empty() {
        total += BOTH_EMPTY;
    }
    if was.display.is_some() && was.display == now.display {
        total += SAME_DISPLAY;
        // Position is only meaningful within one display.
        if was.index == now.index {
            total += SAME_INDEX;
        }
    }
    if was.space == now.space {
        total += SAME_SPACE_ID;
    }
    total
}

/// What the matching concluded about one recorded desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Match {
    /// It is this desktop now. The two are the same when the ids are equal,
    /// which is the ordinary case and not worth treating separately.
    Is(SpaceId),
    /// Nothing the window server lists is this desktop. Its windows are
    /// somewhere else, or it is genuinely gone.
    Gone,
}

/// The result of one matching pass.
#[derive(Clone, Debug, Default)]
pub(super) struct Matching {
    /// Every recorded desktop's verdict.
    pub(super) by_recorded: HashMap<SpaceId, Match>,
}

impl Matching {
    pub(super) fn get(&self, recorded: SpaceId) -> Option<SpaceId> {
        match self.by_recorded.get(&recorded) {
            Some(Match::Is(now)) => Some(*now),
            _ => None,
        }
    }
}

/// Pair recorded desktops with current ones by what is on them.
///
/// Greedy on the best remaining score. A full assignment solver would be more
/// principled, but the scores here are dominated by window overlap, which is
/// close to disjoint between candidates — two desktops rarely both hold most
/// of the same windows — so the greedy choice and the optimal one coincide
/// except in cases where neither is clearly right.
///
/// Running it twice on the same input gives the same answer, and running it on
/// a report that has not changed changes nothing, which is what lets the
/// caller re-run it on every report instead of choosing a moment.
pub(super) fn match_desktops(was: &[Desktop], now: &[Desktop]) -> Matching {
    let mut candidates: Vec<(Score, usize, usize)> = Vec::new();
    for (i, w) in was.iter().enumerate() {
        for (j, n) in now.iter().enumerate() {
            let s = score(w, n);
            if s >= FLOOR {
                candidates.push((s, i, j));
            }
        }
    }
    // Best first; ties broken by the recorded order so the result does not
    // depend on HashMap iteration order.
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut taken_was = vec![false; was.len()];
    let mut taken_now = vec![false; now.len()];
    let mut by_recorded = HashMap::default();

    for (_, i, j) in candidates {
        if taken_was[i] || taken_now[j] {
            continue;
        }
        taken_was[i] = true;
        taken_now[j] = true;
        by_recorded.insert(was[i].space, Match::Is(now[j].space));
    }

    for (i, w) in was.iter().enumerate() {
        if !taken_was[i] {
            by_recorded.insert(w.space, Match::Gone);
        }
    }
    Matching { by_recorded }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wid(pid: i32, idx: u32) -> WindowId { WindowId::new(pid, idx) }

    fn desk(space: u64, display: &str, index: usize, windows: &[WindowId]) -> Desktop {
        Desktop {
            space: SpaceId::new(space),
            display: Some(display.to_string()),
            index,
            windows: windows.to_vec(),
        }
    }

    #[test]
    fn a_desktop_that_kept_its_windows_is_matched_through_a_renumber() {
        let a = wid(1, 1);
        let b = wid(1, 2);
        // Two desktops, and the return renumbers both *and* swaps their order.
        // Nothing but the windows says which is which: the ids are all new,
        // and position now points at the wrong one. A decoy that matches the
        // old position exactly is what makes this test discriminate -- without
        // it, position alone produces the right answer and the test passes
        // against a matcher that never looks at content.
        let was = vec![desk(126, "builtin", 0, &[a]), desk(127, "builtin", 1, &[b])];
        let now = vec![desk(149, "builtin", 0, &[b]), desk(150, "builtin", 1, &[a])];

        let m = match_desktops(&was, &now);
        assert_eq!(m.get(SpaceId::new(126)), Some(SpaceId::new(150)));
        assert_eq!(m.get(SpaceId::new(127)), Some(SpaceId::new(149)));
    }

    #[test]
    fn content_outvotes_the_display_a_desktop_used_to_be_on() {
        let a = wid(1, 1);
        let b = wid(1, 2);
        let was = vec![desk(53, "lg", 0, &[a, b])];
        // Same windows, but the desktop is now listed under the built-in --
        // which the recorded traces show macOS doing routinely -- and under a
        // new id, so neither the display nor the id can carry this. A decoy
        // sits on the old display at the old index holding nothing.
        let now = vec![desk(161, "builtin", 2, &[a, b]), desk(162, "lg", 0, &[])];

        let m = match_desktops(&was, &now);
        assert_eq!(m.get(SpaceId::new(53)), Some(SpaceId::new(161)));
    }

    #[test]
    fn a_desktop_whose_windows_all_went_elsewhere_is_gone_not_guessed() {
        let a = wid(1, 1);
        let b = wid(1, 2);
        let was = vec![desk(53, "lg", 0, &[a, b])];
        // A desktop in the same place holding something else entirely is not
        // this one. Position alone must not claim it.
        let now = vec![desk(53, "lg", 0, &[wid(2, 9)])];

        let m = match_desktops(&was, &now);
        assert_eq!(m.by_recorded[&SpaceId::new(53)], Match::Gone);
    }

    #[test]
    fn two_desktops_do_not_both_claim_one() {
        let a = wid(1, 1);
        let b = wid(1, 2);
        let was = vec![desk(1, "d", 0, &[a]), desk(2, "d", 1, &[b])];
        // Both recorded desktops' windows landed on one.
        let now = vec![desk(9, "d", 0, &[a, b])];

        let m = match_desktops(&was, &now);
        let claimed: Vec<_> =
            [1u64, 2].iter().filter(|s| m.get(SpaceId::new(**s)).is_some()).collect();
        assert_eq!(
            claimed.len(),
            1,
            "exactly one recorded desktop may claim a current one"
        );
    }

    #[test]
    fn empty_desktops_pair_by_position_since_they_have_nothing_else() {
        let was = vec![desk(1, "d", 0, &[]), desk(2, "d", 1, &[])];
        let now = vec![desk(7, "d", 0, &[]), desk(8, "d", 1, &[])];

        let m = match_desktops(&was, &now);
        assert_eq!(m.get(SpaceId::new(1)), Some(SpaceId::new(7)));
        assert_eq!(m.get(SpaceId::new(2)), Some(SpaceId::new(8)));
    }

    #[test]
    fn a_partial_match_still_beats_no_match() {
        let a = wid(1, 1);
        let b = wid(1, 2);
        let c = wid(1, 3);
        let was = vec![desk(1, "d", 0, &[a, b, c])];
        // The user closed two of the three while the display was away.
        let now = vec![desk(5, "d", 0, &[a])];

        let m = match_desktops(&was, &now);
        assert_eq!(m.get(SpaceId::new(1)), Some(SpaceId::new(5)));
    }

    #[test]
    fn the_lid_case_two_displays_renumbered_at_once() {
        // Eric's 2026-09-21 report: lid shut while unplugging, opened before
        // plugging back in. Both displays' desktops were renumbered, and the
        // windows are the only thing that did not change.
        let safari = wid(10, 1);
        let te1 = wid(11, 1);
        let te2 = wid(11, 2);
        let other = wid(12, 1);
        // Every id is new, and the LG's two desktops come back in the other
        // order -- so position points at the wrong one for both of them, and
        // only the windows say which is which.
        let was = vec![
            desk(126, "builtin", 0, &[safari]),
            desk(53, "lg", 0, &[te1, te2]),
            desk(6, "lg", 1, &[other]),
        ];
        let now = vec![
            desk(160, "builtin", 0, &[safari]),
            desk(151, "lg", 0, &[other]),
            desk(5, "lg", 1, &[te1, te2]),
        ];

        let m = match_desktops(&was, &now);
        assert_eq!(m.get(SpaceId::new(126)), Some(SpaceId::new(160)));
        assert_eq!(m.get(SpaceId::new(53)), Some(SpaceId::new(5)));
        assert_eq!(m.get(SpaceId::new(6)), Some(SpaceId::new(151)));
    }
}
