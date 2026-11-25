//! The mouse button as the reactor sees it. While a button is held, layout reaches the screen
//! only at the release (#689).

use objc2_core_foundation::CGPoint;
use tracing::debug;

use super::{Changes, Presentation, Reactor};
use crate::actor::app::WindowId;
use crate::actor::drag::{DragOrigin, MouseButton};
use crate::common::collections::HashSet;
use crate::model::window_store::{ClaimHolder, ClaimOwner};
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{DirectWindow, WindowServerId};

/// Whether a button is held. Nothing waits for the screen without a press.
#[derive(Default)]
pub(super) enum Pointer {
    #[default]
    Up,
    Down(Press),
}

/// A held button, and what waits for its release.
pub(super) struct Press {
    pub(super) button: MouseButton,
    /// Native windows that appeared under the left button. AX can stay silent about them
    /// while an app runs its own drag, the WindowServer reports no moves, and the window may
    /// never take focus, so the one following the cursor is the one that takes focus
    /// (`follow_focused_arrival`) or the one the pointer holds (`follow_arrival_under`).
    pub(super) arrivals: HashSet<WindowServerId>,
    /// The arrival following the cursor (a torn-out tab). The next pointer sample starts its
    /// drag.
    pub(super) following: Option<WindowId>,
    /// Layouts made while the button is held, not yet on screen.
    pub(super) held_layout: HeldLayout,
}

impl Press {
    pub(super) fn new(button: MouseButton) -> Self {
        Press {
            button,
            arrivals: HashSet::default(),
            following: None,
            held_layout: HeldLayout::default(),
        }
    }
}

/// Arrangements waiting for the release: one per space (or for all), at the strongest
/// presentation asked for, and whether any of them changed the layout.
#[derive(Default)]
pub(super) struct HeldLayout(Vec<(Option<SpaceId>, Presentation, bool)>);

impl HeldLayout {
    pub(super) fn hold(&mut self, space: Option<SpaceId>, policy: Presentation, changed: bool) {
        if let Some((_, current, layout_changed)) = self.0.iter_mut().find(|(id, ..)| *id == space)
        {
            *current = (*current).max(policy);
            *layout_changed |= changed;
        } else {
            self.0.push((space, policy, changed));
        }
    }

    /// Keep only the arrangements `keep` says still wait.
    pub(super) fn retain(
        &mut self,
        keep: impl FnMut(&(Option<SpaceId>, Presentation, bool)) -> bool,
    ) {
        self.0.retain(keep);
    }
}

impl IntoIterator for HeldLayout {
    type IntoIter = std::vec::IntoIter<(Option<SpaceId>, Presentation, bool)>;
    type Item = (Option<SpaceId>, Presentation, bool);

    fn into_iter(self) -> Self::IntoIter { self.0.into_iter() }
}

impl Reactor {
    /// Whether the left button is down, keeping the pointer in step: a held button the
    /// reactor had not seen starts a press, and one found released ends its press (a release
    /// the event tap missed heals here). A press with a drag ends only by the drag's own
    /// release (the tap's, or one inferred from the dragged window's frame). The plain left
    /// button has no mouse-down event, so every decision that depends on it reads it here.
    pub(super) fn observe_button(&mut self, changes: &mut Changes) -> bool {
        let down = crate::sys::event::left_button_down();
        match (&self.pointer, down) {
            (Pointer::Up, true) => self.pointer = Pointer::Down(Press::new(MouseButton::Left)),
            (Pointer::Down(press), false)
                if press.button == MouseButton::Left && !self.is_in_drag() =>
            {
                self.release(changes, MouseButton::Left);
            }
            _ => {}
        }
        down
    }

    /// The press under `button` ends: the one transition from Down to Up. Its drag ends, the
    /// windows it held return (one dragged before AX reported it waits with its drop), the drag
    /// lands, and the layout it held back reaches the screen, all in this one reconciliation.
    /// The drag ends before the windows return because reconciliation leaves a drag's source
    /// alone.
    pub(super) fn release(&mut self, changes: &mut Changes, button: MouseButton) {
        let press = match std::mem::take(&mut self.pointer) {
            Pointer::Down(press) if press.button == button => press,
            other => {
                self.pointer = other;
                return;
            }
        };
        let finishes = !self.is_mission_control_active()
            && (!self.is_in_drag() || self.drag_manager.actor.owns_button(button));
        let mut commit = finishes.then(|| self.end_drag(button)).flatten();
        if button == MouseButton::Left {
            commit = self.release_press(changes, commit);
        }
        if let Some(commit) = commit {
            self.land_drag(changes, commit);
        }
        for (space, policy, changed) in press.held_layout {
            changes.arrange(space, policy).layout_changed |= changed;
        }
        if !press.arrivals.is_empty() {
            debug!(windows = ?press.arrivals, "press-arrival: released");
            changes.refresh_notifications = true;
        }
    }

    /// One press can see many windows arrive and be dragged in turn: an app that moves a
    /// dragged tab into a new window (Chrome, when a tab docks and is torn out again in one
    /// press) leaves the previous window behind, still alive for a moment. Only one window
    /// follows the cursor, so a press-held window that moves takes the drag over from an
    /// arriving drag of another window. A drag of a window Rift manages is never taken over.
    pub(super) fn hand_over_arriving_drag(&mut self, changes: &mut Changes, window: WindowId) {
        let held = self
            .state
            .windows
            .claim(window)
            .is_some_and(|claim| matches!(claim.holder, ClaimHolder::Press));
        if held
            && let Some(source) = self.drag_manager.actor.source()
            && source.window != window
            && source.origin == DragOrigin::Arriving
        {
            debug!(from = ?source.window, to = ?window, "drag handed over");
            self.cancel_drag(changes);
        }
    }

    /// A window that appeared under the press took focus: it is the one following the cursor.
    /// It may have reached Rift as a WindowServer appearance or, first, as an AX report the
    /// press already claimed.
    pub(super) fn follow_focused_arrival(&mut self, changes: &mut Changes, window: WindowId) {
        self.follow_arrival(changes, window, "took focus");
    }

    /// No arrival has taken focus (a tab torn from the focused window may never take it): the
    /// one following the cursor is the only arrival at the normal window level whose bounds
    /// hold the pointer. Helpers (tooltips, menus, drag images) sit above that level.
    fn follow_arrival_under(&mut self, changes: &mut Changes, point: CGPoint) {
        let Pointer::Down(press) = &self.pointer else {
            return;
        };
        let mut under = press.arrivals.iter().copied().filter(|&wsid| {
            let direct = DirectWindow(wsid);
            let level = direct.level();
            let bounds = (level == Some(0)).then(|| direct.bounds()).flatten();
            let holds =
                bounds.is_some_and(|frame| crate::actor::drag::contains(frame, point, 12.0));
            debug!(
                ?wsid,
                ?level,
                ?bounds,
                holds,
                "press-arrival: under the pointer?"
            );
            holds
        });
        if let (Some(wsid), None) = (under.next(), under.next())
            && let Some(pid) = DirectWindow(wsid).pid()
        {
            self.follow_arrival(changes, WindowId::new(pid, wsid.as_u32()), "holds the pointer");
        }
    }

    /// Record the window following the cursor (AX may not have reported it) and claim it, so
    /// the pointer can drag it.
    fn follow_arrival(&mut self, changes: &mut Changes, window: WindowId, why: &'static str) {
        let wsid = WindowServerId::new(window.idx.get());
        let held = self
            .state
            .windows
            .claim(window)
            .is_some_and(|claim| matches!(claim.holder, ClaimHolder::Press));
        let Pointer::Down(press) = &mut self.pointer else {
            return;
        };
        if !(press.arrivals.contains(&wsid) || held) || press.following == Some(window) {
            return;
        }
        press.following = Some(window);
        debug!(?window, why, "press-arrival: follows the cursor");
        if self.state.windows.record(window).is_none() {
            self.arrive_under_press(changes, window);
        }
    }

    /// Whether the pointer's samples are wanted: for the window following the cursor, or, while
    /// windows have arrived and none follows and nothing is dragged, to find the one that does.
    pub(super) fn follower_awaits_drag(&self) -> bool {
        matches!(&self.pointer, Pointer::Down(press) if match press.following {
            Some(window) => !self.is_dragging_window(window),
            None => !press.arrivals.is_empty() && !self.is_in_drag(),
        })
    }

    /// The pointer moved under the press: the window following the cursor gets the drag, from
    /// this first sample, whatever the app and the WindowServer have reported of it.
    pub(super) fn drag_follower(&mut self, changes: &mut Changes, point: CGPoint) {
        if matches!(&self.pointer, Pointer::Down(press) if press.following.is_none())
            && !self.is_in_drag()
        {
            self.follow_arrival_under(changes, point);
        }
        let Pointer::Down(press) = &self.pointer else {
            return;
        };
        let Some(window) = press.following else {
            return;
        };
        if self.is_dragging_window(window) {
            return;
        }
        self.hand_over_arriving_drag(changes, window);
        // It follows the cursor, so no cursor check: it only has to be a window still.
        let Some(origin) = self.drag_may_start().then(|| self.drag_origin(window)).flatten() else {
            return;
        };
        let Some(frame) = self
            .state
            .windows
            .record(window)
            .and_then(|record| record.window_server_id())
            .and_then(|wsid| DirectWindow(wsid).bounds())
        else {
            return;
        };
        let space = self.screen_for_point(point).and_then(|screen| screen.space);
        self.begin_native_drag(window, frame, frame, space, space, origin);
    }

    /// Press claims and drags exist only under a press.
    pub(super) fn debug_assert_press_invariants(&self) {
        let down = matches!(self.pointer, Pointer::Down(_));
        debug_assert!(down || self.state.windows.owner_claims(ClaimOwner::Press).is_empty());
        debug_assert!(down || !self.is_in_drag());
    }
}
