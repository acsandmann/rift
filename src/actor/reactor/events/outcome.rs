use crate::actor::app::WindowId;

/// Follow-up work requested by an event workflow.
///
/// Workflows mutate reactor-owned domain state synchronously, then describe the
/// ordered integration work which must happen after the mutation.  Keeping the
/// description small and concrete makes it possible to test policy without
/// turning platform operations into a generic effect system.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct EventOutcome {
    pub(crate) arrange: ArrangeRequest,
    pub(crate) focused_window: Option<WindowId>,
    pub(crate) refresh_window_notifications: bool,
    pub(crate) refresh_layout_mode: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ArrangeRequest {
    pub(crate) requested: bool,
    pub(crate) is_resize: bool,
    pub(crate) window_was_destroyed: bool,
}

impl EventOutcome {
    pub(crate) fn finalized_event(
        focused_window: Option<WindowId>,
        is_resize: bool,
        window_was_destroyed: bool,
        refresh_window_notifications: bool,
    ) -> Self {
        Self {
            arrange: ArrangeRequest {
                requested: true,
                is_resize,
                window_was_destroyed,
            },
            focused_window,
            refresh_window_notifications,
            refresh_layout_mode: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalized_events_explicitly_request_all_legacy_follow_up_work() {
        let outcome = EventOutcome::finalized_event(None, true, false, true);

        assert!(outcome.arrange.requested);
        assert!(outcome.arrange.is_resize);
        assert!(!outcome.arrange.window_was_destroyed);
        assert!(outcome.refresh_window_notifications);
        assert!(outcome.refresh_layout_mode);
    }
}
