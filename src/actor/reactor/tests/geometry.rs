use test_log::test;

use super::super::animation::{AnimationManager, AnimationSender};
use super::*;

fn scrolling_displays(
    settings: crate::common::config::VirtualWorkspaceSettings,
) -> (Apps, Reactor, SpaceId, SpaceId) {
    let (apps, mut r, a, b) = two_displays(settings);
    r.config.settings.layout.scrolling.column_width_ratio = 0.5;
    r.config.settings.layout.scrolling.preserve_window_sizes = false;
    r.layout_manager.layout_engine.set_layout_settings(&r.config.settings.layout);
    for space in [a, b] {
        r.handle_test_workspace_command(space, &LayoutCommand::SetWorkspaceLayout {
            workspace: None,
            mode: LayoutMode::Scrolling,
        });
    }
    (apps, r, a, b)
}

#[test]
fn scrolling_proportions_survive_transfers_and_hotplug() {
    for operation in 0..3 {
        // window transfer, workspace transfer, disconnect/reconnect
        let settings = if operation == 2 {
            Default::default()
        } else {
            shared_settings()
        };
        let (mut apps, mut r, a, b) = scrolling_displays(settings);
        let wid = WindowId::new(1, 1);
        let wsid = r.test_window_server_id(wid);
        let connected = r.space_state.screens.clone();
        for destination in [1, 0, 1, 0] {
            let space = if destination == 0 { a } else { b };
            if operation == 2 {
                let screens = connected
                    .iter()
                    .filter(|s| destination == 0 || s.space == Some(b))
                    .cloned()
                    .collect();
                let mut snapshot = forwarded_space_state(screens);
                snapshot.display_set_changed = true;
                snapshot.should_force_refresh_layout = true;
                snapshot.membership_complete = true;
                snapshot.active_window_spaces.insert(wsid, space);
                window_server::set_window_spaces_override(wsid, Some(vec![space.get()]));
                r.handle_event(Event::SpaceStateChanged(snapshot));
            } else {
                r.handle_event(Event::Command(Command::Reactor(if operation == 0 {
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
            }
            let pending = r.transaction_manager.get_target_frame(wsid);
            r.reconcile_authoritative_active_window_snapshot(vec![(wsid, Some(space))], false, &[]);
            assert_eq!(
                r.transaction_manager.get_target_frame(wsid),
                pending,
                "Space membership cannot acknowledge geometry"
            );
            if let Some(mut intermediate) = pending {
                intermediate.size.width -= 127.;
                apps.windows.get_mut(&wid).unwrap().frame = intermediate;
                let txid = r.transaction_manager.get_last_sent_txid(wsid);
                r.handle_event(Event::WindowFrameChanged(
                    wid,
                    intermediate,
                    Some(txid),
                    Requested(false),
                    Some(MouseState::Down),
                ));
            }
            apps.simulate_until_quiet(&mut r);
            let space = r.assigned_space_for_window_id(wid).unwrap();
            let screen = r.space_state.screen_by_space(space).unwrap().frame;
            let mut intermediate = apps.windows[&wid].frame;
            intermediate.size.width -= 83.;
            // Late native frames and unrelated mouse activity cannot edit the column.
            for mouse in [MouseState::Down, MouseState::Up] {
                apps.windows.get_mut(&wid).unwrap().frame = intermediate;
                r.handle_event(Event::WindowFrameChanged(
                    wid,
                    intermediate,
                    None,
                    Requested(false),
                    Some(mouse),
                ));
                apps.simulate_until_quiet(&mut r);
                assert_eq!(
                    test_layout(&mut r, space, screen)[0].1.size.width,
                    screen.size.width * 0.5
                );
                assert_eq!(apps.windows[&wid].frame.size.width, screen.size.width * 0.5);
            }
        }
        window_server::set_window_spaces_override(wsid, None);
    }
}

#[test]
fn failed_attempts_release_batched_requests_and_fence_stale_acknowledgements() {
    for failure in ["partial", "clamped", "timeout"] {
        let (mut apps, mut r) = test_context();
        let space = SpaceId::new(1);
        apps.make_app_and_settle_on_screen(&mut r, shared_left(), space, 1, make_windows(2));
        let windows = [WindowId::new(1, 1), WindowId::new(1, 2)];
        r.transaction_manager.generate_next_txid(r.test_window_server_id(windows[0]));
        let layout = windows.map(|wid| {
            (
                wid,
                CGRect::new(
                    CGPoint::new((wid.idx.get() - 1) as f64 * 400., 40.),
                    CGSize::new(400., 600.),
                ),
            )
        });
        let observed = windows.map(|wid| r.state.windows.window(wid).unwrap().info.frame);
        AnimationManager::instant_layout(&mut r, space, &layout, None);
        let requests = apps.requests();
        let [Request::SetWindowFrames(frames, _, _, _)] = requests.as_slice() else {
            panic!("one app batch: {requests:?}")
        };
        assert_eq!(frames.len(), 2);
        assert_ne!(
            frames[0].2, frames[1].2,
            "batching must preserve independent generations"
        );
        for &(wid, _, txid) in frames {
            assert_eq!(
                r.state.windows.window(wid).unwrap().info.frame,
                observed[(wid.idx.get() - 1) as usize]
            );
            // The app's bounded correction also failed; completion is not success.
            for _ in 0..2 {
                let event = if failure == "timeout" {
                    Event::WindowFrameRequestFailed(wid, txid)
                } else {
                    let mut actual = observed[(wid.idx.get() - 1) as usize];
                    if failure == "clamped" {
                        actual.size = CGSize::new(300., 400.);
                    }
                    apps.windows.get_mut(&wid).unwrap().frame = actual;
                    Event::WindowFrameChanged(wid, actual, Some(txid), Requested(true), None)
                };
                r.handle_event(event);
            }
        }
        assert!(
            apps.requests().is_empty(),
            "failure must not create a retry loop"
        );
        AnimationManager::instant_layout(&mut r, space, &layout, None);
        let retry = apps.requests();
        assert!(
            matches!(retry.as_slice(), [Request::SetWindowFrames(batch, _, _, _)] if batch.len() == 2)
        );
        for &(wid, _, old) in frames {
            r.handle_event(Event::WindowFrameChanged(
                wid,
                CGRect::ZERO,
                Some(old),
                Requested(true),
                Some(MouseState::Down),
            ));
        }
        for request in retry {
            r.app_manager.apps[&1].handle.send(request).unwrap();
        }
        apps.simulate_until_quiet(&mut r);
        for (wid, frame) in layout {
            assert_eq!(apps.windows[&wid].frame, frame);
            assert_eq!(r.state.windows.window(wid).unwrap().info.frame, frame);
        }
        AnimationManager::instant_layout(&mut r, space, &layout, None);
        assert!(
            apps.requests().is_empty(),
            "matching observations need no writes"
        );
    }
}

#[test]
fn instant_layout_replaces_a_queued_animation_without_stranding_its_target() {
    for (published, position_only) in [(false, false), (true, false), (true, true)] {
        let (mut apps, mut r) = test_context();
        let wid = WindowId::new(1, 1);
        let space = SpaceId::new(1);
        apps.make_app_and_settle_on_screen(&mut r, shared_left(), space, 1, make_windows(1));
        let source = apps.windows[&wid].frame;
        let (sender, _receiver) = AnimationSender::channel();
        r.animation_tx = Some(sender);
        r.config.settings.animate = true;
        let layout = [(wid, CGRect::new(CGPoint::new(40., 50.), CGSize::new(600., 500.)))];
        AnimationManager::animate_layout(&mut r, space, &layout, false, None, true);
        assert!(apps.requests().is_empty());
        if published {
            // A presented sample can reach macOS before any AX observation.
            let sample = CGRect::new(CGPoint::new(20., 25.), CGSize::new(800., 700.));
            apps.windows.get_mut(&wid).unwrap().frame = sample;
            r.state.windows.window_mut(wid).unwrap().frame_monotonic = sample;
        }
        let replacement = if published { [(wid, source)] } else { layout };
        if position_only {
            AnimationManager::workspace_switch_layout(&mut r, space, &replacement, None);
        } else {
            AnimationManager::instant_layout(&mut r, space, &replacement, None);
        }
        apps.simulate_until_quiet(&mut r);
        assert_eq!(apps.windows[&wid].frame, replacement[0].1);
    }
}

#[test]
fn only_verified_native_resizing_changes_layout_dimensions() {
    for interaction in ["unrelated", "elsewhere", "resize", "custom titlebar move"] {
        let (mut apps, mut r, _, space) = scrolling_displays(shared_settings());
        let screen = shared_right();
        let wid = WindowId::new(1, 1);
        r.handle_event(Event::Command(Command::Reactor(
            ReactorCommand::MoveWindowToDisplay {
                selector: DisplaySelector::Index(1),
                window_id: Some(1),
            },
        )));
        apps.simulate_until_quiet(&mut r);
        let wsid = r.test_window_server_id(wid);
        r.reconcile_authoritative_active_window_snapshot(vec![(wsid, Some(space))], false, &[]);
        window_server::set_window_spaces_override(wsid, Some(vec![space.get()]));
        let frame = apps.windows[&wid].frame;
        let resize = matches!(interaction, "resize" | "elsewhere");
        let mut press = if resize { frame.max() } else { frame.mid() };
        if interaction == "elsewhere" {
            press.y += 50.;
        }
        if interaction != "unrelated" {
            r.handle_event(Event::NativeMouseDown { point: press });
        }
        for delta in [40., 80.] {
            let moved = CGRect::new(
                CGPoint::new(frame.origin.x + if resize { 0. } else { delta }, frame.origin.y),
                CGSize::new(frame.size.width - delta, frame.size.height),
            );
            window_server::set_cursor_location_override(Some(CGPoint::new(
                press.x + if resize { -delta } else { delta },
                press.y,
            )));
            apps.windows.get_mut(&wid).unwrap().frame = moved;
            r.handle_event(Event::WindowFrameChanged(
                wid,
                moved,
                None,
                Requested(false),
                Some(MouseState::Down),
            ));
            apps.simulate_until_quiet(&mut r);
            assert_eq!(
                test_layout(&mut r, space, screen)[0].1.size.width,
                if interaction == "resize" {
                    moved.size.width
                } else {
                    frame.size.width
                },
                "{interaction}"
            );
        }
        window_server::set_cursor_location_override(None);
        window_server::set_window_spaces_override(wsid, None);
    }
}

#[test]
fn inward_resize_uses_pre_change_observation_before_drag_motion() {
    let (mut apps, mut r, space, _) = scrolling_displays(shared_settings());
    apps.simulate_until_quiet(&mut r);
    let wid = WindowId::new(1, 1);
    let frame = apps.windows[&wid].frame;
    // Input enqueues this press before forwarding it to the application.
    r.handle_event(Event::NativeMouseDown { point: frame.max() });
    let mut resized = frame;
    resized.size.width -= 180.;
    window_server::set_cursor_location_override(Some(resized.max()));
    apps.windows.get_mut(&wid).unwrap().frame = resized;
    // AX arrives before the first physical drag event. The original press is
    // now outside the window; neither hit-testing it nor saving this frame works.
    r.handle_event(Event::WindowFrameChanged(
        wid,
        resized,
        None,
        Requested(false),
        Some(MouseState::Down),
    ));
    assert_eq!(
        test_layout(&mut r, space, shared_left())[0].1.size.width,
        resized.size.width
    );
    window_server::set_cursor_location_override(None);
}

#[test]
fn native_resize_commits_release_frame_before_delayed_ax_after_transfer() {
    let (mut apps, mut r, _, space) = scrolling_displays(shared_settings());
    let wid = WindowId::new(1, 1);
    r.handle_event(Event::Command(Command::Reactor(
        ReactorCommand::MoveWindowToDisplay {
            selector: DisplaySelector::Index(1),
            window_id: Some(1),
        },
    )));
    apps.simulate_until_quiet(&mut r);
    let wsid = r.test_window_server_id(wid);
    window_server::set_window_spaces_override(wsid, Some(vec![space.get()]));
    let frame = apps.windows[&wid].frame;
    r.drag_manager.native_down = Some(frame.max());
    let mut intermediate = frame;
    intermediate.size.width -= 40.;
    window_server::set_cursor_location_override(Some(intermediate.max()));
    apps.windows.get_mut(&wid).unwrap().frame = intermediate;
    r.handle_event(Event::WindowFrameChanged(
        wid,
        intermediate,
        None,
        Requested(false),
        Some(MouseState::Down),
    ));
    let old_txid = r.transaction_manager.get_last_sent_txid(wsid);
    let mut final_frame = frame;
    final_frame.size.width -= 90.;
    apps.windows.get_mut(&wid).unwrap().frame = final_frame;
    r.handle_event(Event::MouseUp(crate::actor::drag::MouseButton::Left));
    // A queued observation may be delivered before the authoritative release read.
    r.handle_event(Event::WindowFrameChanged(
        wid,
        intermediate,
        Some(old_txid),
        Requested(false),
        Some(MouseState::Up),
    ));
    apps.simulate_until_quiet(&mut r);
    // And duplicate/reordered native notifications may follow it.
    for observed in [final_frame, intermediate, final_frame] {
        r.handle_event(Event::WindowFrameChanged(
            wid,
            observed,
            Some(old_txid),
            Requested(false),
            Some(MouseState::Up),
        ));
        apps.simulate_until_quiet(&mut r);
    }
    assert_eq!(
        test_layout(&mut r, space, shared_right())[0].1.size.width,
        final_frame.size.width
    );
    assert_eq!(apps.windows[&wid].frame.size.width, final_frame.size.width);
    window_server::set_cursor_location_override(None);
    window_server::set_window_spaces_override(wsid, None);
}
