//! The mouse button as the reactor sees it. While a button is held, layout reaches the screen
//! only at the release (#689).

use tracing::debug;

use super::{Changes, Presentation, Reactor};
use crate::actor::drag::MouseButton;
use crate::common::collections::HashSet;
use crate::model::window_store::ClaimOwner;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

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
    /// Native windows that appeared under the left button and have no record yet, subscribed
    /// for WindowServer moves: AX can stay silent while an app runs its own drag. The first
    /// move records and claims each one (`arrive_under_press`).
    pub(super) arrivals: HashSet<WindowServerId>,
    /// Layouts made while the button is held, not yet on screen.
    pub(super) held_layout: HeldLayout,
}

impl Press {
    pub(super) fn new(button: MouseButton) -> Self {
        Press {
            button,
            arrivals: HashSet::default(),
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

    /// Press claims and drags exist only under a press.
    pub(super) fn debug_assert_press_invariants(&self) {
        let down = matches!(self.pointer, Pointer::Down(_));
        debug_assert!(down || self.state.windows.owner_claims(ClaimOwner::Press).is_empty());
        debug_assert!(down || !self.is_in_drag());
    }
}
