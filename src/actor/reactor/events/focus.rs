use crate::actor::app::WindowId;
use crate::sys::screen::SpaceId;

/// Resolves a workflow focus request without granting the focus service access
/// to the reactor or to unrelated mutable state.
pub(crate) fn resolve(
    requested_window: Option<WindowId>,
    space_for_window: impl FnOnce(WindowId) -> Option<SpaceId>,
) -> Option<(SpaceId, WindowId)> {
    let window = requested_window?;
    Some((space_for_window(window)?, window))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;

    #[test]
    fn focus_is_emitted_only_for_a_window_with_a_resolved_space() {
        let window = WindowId {
            pid: 7,
            idx: NonZeroU32::new(9).unwrap(),
        };
        let space = SpaceId::new(3);

        assert_eq!(resolve(Some(window), |_| Some(space)), Some((space, window)));
        assert_eq!(resolve(Some(window), |_| None), None);
        assert_eq!(resolve(None, |_| Some(space)), None);
    }
}
