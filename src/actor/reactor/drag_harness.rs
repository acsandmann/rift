//! Autonomous drag harness — reproduces manual drag without needing a real display.
//! Run: cargo test drag_harness -- --nocapture
//! Also via: ./scripts/autonomous-test.sh
//! Rewritten for upstream be8afef (ForwardedSpaceState + new testing helpers).

#[cfg(test)]
mod tests {
    use crate::actor::app::WindowId;
    use crate::actor::reactor::testing::{Apps, make_windows, space_state_event, test_reactor};
    use crate::actor::reactor::{Event, Requested};
    use crate::common::config::LayoutMode;
    use crate::layout_engine::LayoutCommand;
    use crate::sys::event::MouseState;
    use crate::sys::screen::SpaceId;
    use crate::sys::window_server::WindowServerId;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    fn two_display_reactor() -> (
        crate::actor::reactor::Reactor,
        Apps,
        SpaceId,
        SpaceId,
        WindowId,
        WindowServerId,
    ) {
        let mut reactor = test_reactor();
        let mut apps = Apps::new();
        let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 800.));
        let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 800.));
        let s1 = SpaceId::new(1);
        let s2 = SpaceId::new(2);
        reactor.handle_event(space_state_event(vec![left, right], vec![Some(s1), Some(s2)]));
        // create one window on s1 via Apps path (mirrors real app launch)
        reactor.handle_events(apps.make_app(1, make_windows(1)));
        apps.simulate_until_quiet(&mut reactor);
        let wid = WindowId::new(1, 1);
        let wsid = reactor.test_window_server_id(wid);
        (reactor, apps, s1, s2, wid, wsid)
    }

    fn is_on_space(
        reactor: &crate::actor::reactor::Reactor,
        space: SpaceId,
        wid: WindowId,
    ) -> bool {
        reactor.test_workspace_for_window(space, wid).is_some()
    }

    #[test]
    fn drag_cross_display_moves_space_via_finalize() {
        let (mut reactor, mut apps, s1, s2, wid, _wsid) = two_display_reactor();
        assert!(is_on_space(&reactor, s1, wid), "should start on s1");
        assert!(!is_on_space(&reactor, s2, wid));

        let right_frame = CGRect::new(CGPoint::new(1100., 100.), CGSize::new(50., 50.));
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            right_frame,
            None,
            Requested(false),
            Some(MouseState::Down),
        ));
        let session = reactor.get_active_drag_session().expect("drag should be active");
        assert_eq!(session.settled_space, Some(s2), "drag should resolve to s2");
        assert_eq!(session.origin_space, Some(s1));

        reactor.handle_event(Event::MouseUp);
        apps.simulate_until_quiet(&mut reactor);

        assert!(is_on_space(&reactor, s2, wid), "after MouseUp must be on s2");
        assert!(!is_on_space(&reactor, s1, wid));
        assert!(!reactor.is_in_drag(), "drag inactive after MouseUp");
    }

    #[test]
    fn drag_within_same_display_does_not_move_space() {
        let (mut reactor, mut apps, s1, s2, wid, _) = two_display_reactor();
        let f2 = CGRect::new(CGPoint::new(200., 200.), CGSize::new(50., 50.));
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            f2,
            None,
            Requested(false),
            Some(MouseState::Down),
        ));
        reactor.handle_event(Event::MouseUp);
        apps.simulate_until_quiet(&mut reactor);
        assert!(is_on_space(&reactor, s1, wid));
        assert!(!is_on_space(&reactor, s2, wid));
    }

    #[test]
    fn drag_cancel_via_window_destroy_clears_drag() {
        let (mut reactor, mut apps, _, _, wid, _) = two_display_reactor();
        let f = CGRect::new(CGPoint::new(1100., 100.), CGSize::new(50., 50.));
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            f,
            None,
            Requested(false),
            Some(MouseState::Down),
        ));
        assert!(reactor.is_in_drag());
        reactor.handle_event(Event::WindowDestroyed(wid));
        assert!(
            !reactor.is_in_drag(),
            "destroying dragged window must clear drag"
        );
        let _ = &mut apps; // keep apps alive for simulate
    }

    #[test]
    fn non_drag_space_change_still_works() {
        let (mut reactor, mut apps, s1, s2, wid, _) = two_display_reactor();
        let f_right = CGRect::new(CGPoint::new(1100., 100.), CGSize::new(50., 50.));
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            f_right,
            None,
            Requested(false),
            Some(MouseState::Up),
        ));
        // non-drag path should still move via WindowRemoved/Added
        apps.simulate_until_quiet(&mut reactor);
        assert!(is_on_space(&reactor, s2, wid), "non-drag must move s1->s2");
        assert!(!is_on_space(&reactor, s1, wid));
    }

    #[test]
    fn scrolling_tiled_window_ignores_geometry_only_space_change() {
        let mut reactor = test_reactor();
        let mut apps = Apps::new();
        let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 800.));
        let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 800.));
        let s1 = SpaceId::new(1);
        let s2 = SpaceId::new(2);
        reactor.handle_event(space_state_event(vec![left, right], vec![Some(s1), Some(s2)]));
        reactor.handle_events(apps.make_app(1, make_windows(1)));
        apps.simulate_until_quiet(&mut reactor);
        let wid = WindowId::new(1, 1);
        reactor.handle_event(Event::Command(crate::model::reactor::Command::Layout(
            LayoutCommand::SetWorkspaceLayout {
                workspace: None,
                mode: LayoutMode::Scrolling,
            },
        )));
        apps.simulate_until_quiet(&mut reactor);
        assert_eq!(
            reactor.layout_manager.layout_engine.active_layout_mode_at(s1),
            LayoutMode::Scrolling
        );
        let f_right = CGRect::new(CGPoint::new(1100., 100.), CGSize::new(50., 50.));
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            f_right,
            None,
            Requested(false),
            Some(MouseState::Up),
        ));
        apps.simulate_until_quiet(&mut reactor);
        assert!(
            is_on_space(&reactor, s1, wid),
            "scrolling tiled should NOT auto-move"
        );
        assert!(!is_on_space(&reactor, s2, wid));
    }
}
