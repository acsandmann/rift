//! The mouse button as the reactor sees it. While a button is held, layout reaches the screen
//! only at the release (#689).

use super::Presentation;
use crate::sys::screen::SpaceId;

/// Whether a button is held. Nothing waits for the screen without a press.
#[derive(Default)]
pub(super) enum Pointer {
    #[default]
    Up,
    Down(Press),
}

/// A held button, and what waits for its release.
#[derive(Default)]
pub(super) struct Press {
    /// Layouts made while the button is held, not yet on screen.
    pub(super) held_layout: HeldLayout,
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
