use test_log::test;

use super::*;

#[test]
fn scrolling_display_transfers_preserve_column_proportions() {
    for single_window in [false, true] {
        let (mut apps, mut reactor, a, b) = two_displays(shared_settings());
        let wid = WindowId::new(1, 1);
        let mut config = reactor.config.clone();
        config.settings.layout.scrolling.column_width_ratio = 0.5;
        config.settings.layout.scrolling.preserve_window_sizes = false;
        reactor
            .layout_manager
            .layout_engine
            .set_layout_settings(&config.settings.layout);
        reactor.config = config;
        for space in [a, b] {
            reactor.handle_test_workspace_command(space, &LayoutCommand::SetWorkspaceLayout {
                workspace: None,
                mode: LayoutMode::Scrolling,
            });
        }
        reactor.update_layout_or_warn(false, false, None);
        apps.simulate_until_quiet(&mut reactor);
        let wsid = reactor.test_window_server_id(wid);
        for destination in [1, 0, 1, 0] {
            reactor.handle_event(Event::Command(Command::Reactor(if single_window {
                ReactorCommand::MoveWindowToDisplay {
                    selector: DisplaySelector::Index(destination),
                    window_id: Some(1),
                }
            } else {
                ReactorCommand::MoveWorkspaceToDisplay {
                    selector: DisplaySelector::Index(destination),
                    wrap_around: false,
                }
            })));
            let target = reactor.transaction_manager.get_target_frame(wsid).unwrap();
            let txid = reactor.transaction_manager.get_last_sent_txid(wsid);
            let destination_space = if destination == 0 { a } else { b };
            reactor.reconcile_authoritative_active_window_snapshot(
                vec![(wsid, Some(destination_space))],
                false,
                &[],
            );
            assert_eq!(
                reactor.transaction_manager.get_target_frame(wsid),
                Some(target),
                "native Space membership is not a geometry acknowledgement"
            );
            // Source/destination intermediates and an unrelated mouse press are observations.
            let mut intermediate = target;
            intermediate.size.width -= 127.;
            for mouse in [MouseState::Up, MouseState::Down] {
                reactor.handle_event(Event::WindowFrameChanged(
                    wid,
                    intermediate,
                    Some(txid),
                    Requested(false),
                    Some(mouse),
                ));
            }
            apps.simulate_until_quiet(&mut reactor);
            let space = if destination == 0 { a } else { b };
            let screen = if destination == 0 {
                shared_left()
            } else {
                shared_right()
            };
            let expected = screen.size.width * 0.5;
            assert_eq!(
                test_layout(&mut reactor, space, screen)[0].1.size.width,
                expected
            );
            // A notification can arrive after the transfer's target has settled.
            apps.windows.get_mut(&wid).unwrap().frame = intermediate;
            reactor.handle_event(Event::WindowFrameChanged(
                wid,
                intermediate,
                Some(txid),
                Requested(false),
                Some(MouseState::Up),
            ));
            apps.simulate_until_quiet(&mut reactor);
            assert_eq!(apps.windows[&wid].frame.size.width, expected);
        }

        let frame = apps.windows[&wid].frame;
        let resized = CGRect::new(
            frame.origin,
            CGSize::new(frame.size.width - 100., frame.size.height),
        );
        reactor.drag_manager.native_down = Some((wid, frame, frame.max()));
        crate::sys::window_server::set_cursor_location_override(Some(resized.max()));
        apps.windows.get_mut(&wid).unwrap().frame = resized;
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            resized,
            None,
            Requested(false),
            Some(MouseState::Down),
        ));
        assert_eq!(
            reactor.drag_manager.actor.kind(),
            Some(crate::actor::drag::DragKind::NativeResize)
        );
        reactor.handle_event(Event::MouseUp(crate::actor::drag::MouseButton::Left));
        crate::sys::window_server::set_cursor_location_override(None);
        apps.simulate_until_quiet(&mut reactor);
        assert_eq!(
            test_layout(&mut reactor, a, shared_left())[0].1.size.width,
            resized.size.width,
            "a real native resize immediately after transfer must remain authoritative"
        );
    }
}

#[test]
fn scrolling_hotplug_preserves_relative_widths() {
    let (mut apps, mut reactor, a, b) = two_displays(Default::default());
    let wid = WindowId::new(1, 1);
    let mut config = reactor.config.clone();
    config.settings.layout.scrolling.column_width_ratio = 0.5;
    config.settings.layout.scrolling.preserve_window_sizes = false;
    reactor
        .layout_manager
        .layout_engine
        .set_layout_settings(&config.settings.layout);
    reactor.config = config;
    for space in [a, b] {
        reactor.handle_test_workspace_command(space, &LayoutCommand::SetWorkspaceLayout {
            workspace: None,
            mode: LayoutMode::Scrolling,
        });
    }
    reactor.update_layout_or_warn(false, false, None);
    apps.simulate_until_quiet(&mut reactor);
    let wsid = reactor.test_window_server_id(wid);
    let connected = reactor.space_state.screens.clone();
    for returning in [false, true, false, true] {
        let screens = connected
            .iter()
            .filter(|screen| returning || screen.space == Some(b))
            .cloned()
            .collect();
        let native_space = if returning { a } else { b };
        let mut snapshot = forwarded_space_state(screens);
        snapshot.display_set_changed = true;
        snapshot.should_force_refresh_layout = true;
        snapshot.membership_complete = true;
        snapshot.active_window_spaces.insert(wsid, native_space);
        crate::sys::window_server::set_window_spaces_override(wsid, Some(vec![native_space.get()]));
        reactor.handle_event(Event::SpaceStateChanged(snapshot));
        apps.simulate_until_quiet(&mut reactor);
        let space = reactor.assigned_space_for_window_id(wid).unwrap();
        let screen = reactor.space_state.screen_by_space(space).unwrap().frame;
        let mut intermediate = apps.windows[&wid].frame;
        intermediate.size.width -= 83.;
        apps.windows.get_mut(&wid).unwrap().frame = intermediate;
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            intermediate,
            None,
            Requested(false),
            Some(MouseState::Up),
        ));
        apps.simulate_until_quiet(&mut reactor);
        assert_eq!(
            test_layout(&mut reactor, space, screen)[0].1.size.width,
            screen.size.width * 0.5
        );
    }
    crate::sys::window_server::set_window_spaces_override(wsid, None);
}

#[test]
fn partial_frame_readback_repairs_once_and_fences_superseded_acknowledgements() {
    let (mut apps, mut reactor) = test_context();
    let space = SpaceId::new(1);
    let screen = shared_left();
    let wid = WindowId::new(1, 1);
    apps.make_app_and_settle_on_screen(&mut reactor, screen, space, 1, make_windows(1));
    let wsid = reactor.test_window_server_id(wid);
    let observed = reactor.state.windows.window(wid).unwrap().info.frame;
    let target = CGRect::new(CGPoint::new(40., 50.), CGSize::new(600., 500.));
    super::super::animation::AnimationManager::instant_layout(
        &mut reactor,
        space,
        &[(wid, target)],
        None,
    );
    assert_eq!(
        reactor.state.windows.window(wid).unwrap().info.frame,
        observed,
        "submission is not observation"
    );
    let txid = reactor.transaction_manager.get_last_sent_txid(wsid);
    apps.requests(); // Simulate a lost size write, then a partial AX readback.
    let partial = CGRect::new(target.origin, observed.size);
    for _ in 0..3 {
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            partial,
            Some(txid),
            Requested(true),
            Some(MouseState::Up),
        ));
    }
    let requests = apps.requests();
    assert_eq!(
        requests.iter().filter(|r| matches!(r, Request::SetWindowFrames(..))).count(),
        1
    );
    // Deliver the repair through the app simulator, rather than fabricating success.
    // Requeue its request on the real request transport.
    for request in requests {
        reactor.app_manager.apps[&wid.pid].handle.send(request).unwrap();
    }
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(apps.windows[&wid].frame, target);
    assert_eq!(reactor.transaction_manager.get_target_frame(wsid), None);
    for _ in 0..2 {
        reactor.handle_event(Event::WindowFrameChanged(
            wid,
            partial,
            Some(txid),
            Requested(true),
            Some(MouseState::Up),
        ));
    }
    assert_eq!(
        reactor.state.windows.window(wid).unwrap().info.frame,
        target,
        "late duplicate samples cannot undo a settled generation"
    );
    assert!(apps.requests().is_empty());
    let replacement = CGRect::new(CGPoint::new(80., 90.), target.size);
    super::super::animation::AnimationManager::instant_layout(
        &mut reactor,
        space,
        &[(wid, replacement)],
        None,
    );
    let current = reactor.transaction_manager.get_last_sent_txid(wsid);
    reactor.handle_event(Event::WindowFrameChanged(
        wid,
        partial,
        Some(txid),
        Requested(true),
        Some(MouseState::Down),
    ));
    assert_eq!(reactor.transaction_manager.get_last_sent_txid(wsid), current);
    assert_eq!(
        reactor.transaction_manager.get_target_frame(wsid),
        Some(replacement)
    );
    apps.simulate_until_quiet(&mut reactor);
    reactor.handle_event(Event::WindowFrameChanged(
        wid,
        partial,
        Some(txid),
        Requested(true),
        Some(MouseState::Up),
    ));
    assert_eq!(
        reactor.state.windows.window(wid).unwrap().info.frame,
        replacement
    );
}

#[test]
fn application_constraints_settle_without_becoming_layout_dimensions() {
    for (minimum, maximum, accepted) in [
        (Some(CGSize::new(700., 550.)), None, CGSize::new(700., 550.)),
        (None, Some(CGSize::new(300., 250.)), CGSize::new(300., 250.)),
    ] {
        let (mut reactor, wid, wsid, _, _, _) = reactor_with_window_on_space1();
        let window = reactor.state.windows.window_mut(wid).unwrap();
        window.info.min_size = minimum;
        window.info.max_size = maximum;
        let target = CGRect::new(CGPoint::new(40., 50.), CGSize::new(500., 400.));
        let txid = reactor.transaction_manager.generate_next_txid(wsid);
        reactor.transaction_manager.store_txid(wsid, txid, target, false);
        let observed = CGRect::new(CGPoint::new(40.25, 50.), accepted);
        let outcome = reactor
            .dispatch_workflow(Event::WindowFrameChanged(
                wid,
                observed,
                Some(txid),
                Requested(true),
                None,
            ))
            .unwrap();
        assert_eq!(reactor.transaction_manager.get_target_frame(wsid), None);
        assert_eq!(reactor.state.windows.window(wid).unwrap().info.frame, observed);
        assert!(outcome.layout_events.is_empty());
        assert_eq!(outcome.arrange.passes, 0);
    }
}

#[test]
fn native_resize_requires_the_pressed_window_border_and_can_replace_transfer_geometry() {
    let (mut reactor, wid, wsid, space, _, frame) = reactor_with_window_on_space1();
    reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
    let resized = CGRect::new(
        frame.origin,
        CGSize::new(frame.size.width - 100., frame.size.height),
    );
    let txid = reactor.transaction_manager.generate_next_txid(wsid);
    reactor.transaction_manager.store_txid(wsid, txid, frame, false);
    for (pressed, pointer) in [
        (None, resized.max()),
        (Some((WindowId::new(9, 9), frame, frame.max())), resized.max()),
        (Some((wid, frame, frame.mid())), resized.max()),
        (Some((wid, frame, frame.max())), frame.max()),
    ] {
        crate::sys::window_server::set_cursor_location_override(Some(pointer));
        reactor.drag_manager.native_down = pressed;
        let outcome = reactor
            .dispatch_workflow(Event::WindowFrameChanged(
                wid,
                resized,
                Some(txid),
                Requested(false),
                Some(MouseState::Down),
            ))
            .unwrap();
        assert!(outcome.layout_events.is_empty());
        assert_eq!(reactor.transaction_manager.get_target_frame(wsid), Some(frame));
        reactor.state.windows.window_mut(wid).unwrap().info.frame = frame;
    }
    reactor.drag_manager.native_down = Some((wid, frame, frame.max()));
    crate::sys::window_server::set_cursor_location_override(Some(resized.max()));
    let outcome = reactor
        .dispatch_workflow(Event::WindowFrameChanged(
            wid,
            resized,
            Some(txid),
            Requested(false),
            Some(MouseState::Down),
        ))
        .unwrap();
    assert_eq!(reactor.transaction_manager.get_target_frame(wsid), None);
    assert!(
        outcome
            .layout_events
            .iter()
            .any(|event| matches!(event, LayoutEvent::WindowResized { .. }))
    );
    assert_eq!(
        reactor.drag_manager.actor.kind(),
        Some(crate::actor::drag::DragKind::NativeResize)
    );
    crate::sys::window_server::set_cursor_location_override(None);
}

#[test]
fn failed_frame_request_uses_the_same_bounded_recovery_without_inventing_an_observation() {
    let (mut apps, mut reactor) = test_context();
    let wid = WindowId::new(1, 1);
    let space = SpaceId::new(1);
    apps.make_app_and_settle_on_screen(&mut reactor, shared_left(), space, 1, make_windows(1));
    let wsid = reactor.test_window_server_id(wid);
    let observed = reactor.state.windows.window(wid).unwrap().info.frame;
    let target = CGRect::new(CGPoint::new(40., 50.), CGSize::new(600., 500.));
    super::super::animation::AnimationManager::instant_layout(
        &mut reactor,
        space,
        &[(wid, target)],
        None,
    );
    let txid = reactor.transaction_manager.get_last_sent_txid(wsid);
    apps.requests();
    for _ in 0..3 {
        reactor.handle_event(Event::WindowFrameRequestFailed(wid, txid));
    }
    assert_eq!(reactor.state.windows.window(wid).unwrap().info.frame, observed);
    let requests = apps.requests();
    assert_eq!(requests.len(), 1);
    for request in requests {
        reactor.app_manager.apps[&wid.pid].handle.send(request).unwrap();
    }
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(apps.windows[&wid].frame, target);
    reactor.handle_event(Event::WindowFrameRequestFailed(wid, txid));
    assert!(apps.requests().is_empty());
}

#[test]
fn instant_layout_replaces_a_queued_animation_without_stranding_its_target() {
    let (mut apps, mut reactor) = test_context();
    let wid = WindowId::new(1, 1);
    let space = SpaceId::new(1);
    apps.make_app_and_settle_on_screen(&mut reactor, shared_left(), space, 1, make_windows(1));
    let (sender, _receiver) = super::super::animation::AnimationSender::channel();
    reactor.animation_tx = Some(sender);
    reactor.config.settings.animate = true;
    let target = CGRect::new(CGPoint::new(40., 50.), CGSize::new(600., 500.));
    // The presenter has not consumed this command or published a sample yet.
    super::super::animation::AnimationManager::animate_layout(
        &mut reactor,
        space,
        &[(wid, target)],
        false,
        None,
        true,
    );
    assert!(apps.requests().is_empty());
    super::super::animation::AnimationManager::instant_layout(
        &mut reactor,
        space,
        &[(wid, target)],
        None,
    );
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(apps.windows[&wid].frame, target);
    assert_eq!(reactor.state.windows.window(wid).unwrap().info.frame, target);
}

#[test]
fn native_move_clamping_never_becomes_resize_intent() {
    let (mut reactor, wid, _, space, _, frame) = reactor_with_window_on_space1();
    reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
    let press = CGPoint::new(frame.origin.x + 100., frame.origin.y + 20.);
    reactor.drag_manager.native_down = Some((wid, frame, press));
    for delta in [40., 80.] {
        let moved = CGRect::new(
            CGPoint::new(frame.origin.x + delta, frame.origin.y + 20.),
            CGSize::new(frame.size.width - delta, frame.size.height - 10.),
        );
        crate::sys::window_server::set_cursor_location_override(Some(CGPoint::new(
            press.x + delta,
            press.y + 20.,
        )));
        let outcome = reactor
            .dispatch_workflow(Event::WindowFrameChanged(
                wid,
                moved,
                None,
                Requested(false),
                Some(MouseState::Down),
            ))
            .unwrap();
        assert_eq!(
            reactor.drag_manager.actor.kind(),
            Some(crate::actor::drag::DragKind::NativeMove)
        );
        assert!(
            !outcome
                .layout_events
                .iter()
                .any(|event| matches!(event, LayoutEvent::WindowResized { .. })),
            "application clamping during a verified titlebar move cannot resize the layout"
        );
    }
    crate::sys::window_server::set_cursor_location_override(None);
}
