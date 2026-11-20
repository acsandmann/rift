#[cfg(test)]
mod tests {
    use crate::actor::app::{AppInfo, AppThreadHandle};
    use crate::actor::reactor::testing::test_reactor;
    use crate::actor::reactor::{AppState, Event};

    #[test]
    fn duplicate_registration_retains_original_and_terminates_duplicate() {
        let mut reactor = test_reactor();
        let (original, mut original_rx) = AppThreadHandle::channel();
        let (duplicate, mut duplicate_rx) = AppThreadHandle::channel();
        reactor.app_manager.apps.insert(42, AppState {
            info: AppInfo {
                bundle_id: None,
                localized_name: None,
            },
            handle: original.clone(),
        });
        assert!(reactor.app_manager.reject_duplicate(42, &duplicate));
        assert!(matches!(
            duplicate_rx.try_recv().unwrap().1,
            crate::actor::app::Request::Terminate
        ));
        assert!(reactor.app_manager.reject_duplicate(42, &original));
        assert!(original_rx.try_recv().is_err());
        reactor.handle_event(Event::AppActorExited(42, duplicate));
        assert!(reactor.app_manager.apps[&42].handle.same_actor(&original));
        reactor.handle_event(Event::AppActorExited(42, original));
        assert!(reactor.app_manager.apps.is_empty());
    }
}
