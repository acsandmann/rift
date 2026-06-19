use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::*;
use crate::actor;
use crate::actor::{reactor, wm_controller};

fn make_screen(space: Option<SpaceId>) -> ScreenInfo {
    ScreenInfo {
        id: crate::sys::screen::ScreenId::new(1),
        frame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0)),
        display_uuid: "display-1".to_string(),
        name: Some("Display".to_string()),
        space,
    }
}

fn make_screen_with(
    screen_id: u32,
    display_uuid: &str,
    origin_x: f64,
    width: f64,
    space: Option<SpaceId>,
) -> ScreenInfo {
    ScreenInfo {
        id: crate::sys::screen::ScreenId::new(screen_id),
        frame: CGRect::new(CGPoint::new(origin_x, 0.0), CGSize::new(width, 800.0)),
        display_uuid: display_uuid.to_string(),
        name: Some(display_uuid.to_string()),
        space,
    }
}

fn fullscreen_space_for(user_space: SpaceId) -> SpaceId {
    SpaceId::new(0x400000000 + user_space.get())
}

fn recv_wm(rx: &mut actor::Receiver<wm_controller::WmEvent>) -> wm_controller::WmEvent {
    rx.try_recv().expect("expected wm event").1
}

fn recv_reactor(rx: &mut actor::Receiver<reactor::Event>) -> reactor::Event {
    rx.try_recv().expect("expected reactor event").1
}

fn assert_no_wm_event(rx: &mut actor::Receiver<wm_controller::WmEvent>) {
    assert!(rx.try_recv().is_err(), "expected no wm event");
}

fn assert_no_reactor_event(rx: &mut actor::Receiver<reactor::Event>) {
    assert!(rx.try_recv().is_err(), "expected no reactor event");
}

fn build_actor() -> (
    SpacesActor,
    actor::Receiver<wm_controller::WmEvent>,
    actor::Receiver<reactor::Event>,
) {
    let (wm_tx, wm_rx) = actor::channel();
    let (reactor_tx, reactor_rx) = actor::channel();
    let (actor, _) = SpacesActor::new_for_tests(reactor_tx, wm_tx);
    (actor, wm_rx, reactor_rx)
}

#[test]
fn forwards_stable_screen_and_space_updates_immediately() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let space = SpaceId::new(11);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(space))],
        CoordinateConverter::default(),
    ));
    actor.handle_event(Event::SpaceChanged(vec![Some(space)]));

    assert!(matches!(
        recv_wm(&mut wm_rx),
        wm_controller::WmEvent::SpaceStateUpdated(..)
    ));

    actor.handle_event(Event::SpaceChanged(vec![Some(space)]));
    assert_no_wm_event(&mut wm_rx);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn active_space_changed_waits_for_confirmed_refresh() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    actor.state.last_sent_spaces = Some(vec![Some(SpaceId::new(11))]);

    actor.handle_event(Event::ActiveSpaceChanged);

    assert!(actor.state.awaiting_space_switch_confirmation);
    assert_no_wm_event(&mut wm_rx);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn active_space_changed_forwards_immediately_when_space_snapshot_changes() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let old_space = SpaceId::new(11);
    let new_space = SpaceId::new(12);

    actor.state.screens = vec![make_screen(Some(new_space))];
    actor.state.last_sent_spaces = Some(vec![Some(old_space)]);

    actor.handle_event(Event::ActiveSpaceChanged);

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert_eq!(
                state.screens.iter().map(|screen| screen.space).collect::<Vec<_>>(),
                vec![Some(new_space)]
            );
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
    assert!(!actor.state.awaiting_space_switch_confirmation);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn quarantines_window_space_events_during_sleep_before_churn_begins() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();

    actor.handle_event(Event::SystemWillSleep);
    actor.handle_event(Event::WindowServerAppeared(
        WindowServerId::new(7),
        SpaceId::new(3),
    ));
    actor.handle_event(Event::WindowServerDestroyed(
        WindowServerId::new(8),
        SpaceId::new(4),
    ));

    assert_eq!(
        actor.state.quarantine_stats,
        QuarantineStats { appeared_dropped: 1, destroyed_dropped: 1 }
    );
    assert_no_wm_event(&mut wm_rx);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn buffers_screen_and_space_updates_until_display_churn_ends() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let space = SpaceId::new(21);

    actor.handle_event(Event::DisplayChurnBegin);
    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(space))],
        CoordinateConverter::default(),
    ));
    actor.handle_event(Event::SpaceChanged(vec![Some(space)]));

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));
    assert_no_wm_event(&mut wm_rx);

    actor.handle_event(Event::DisplayChurnEnd);

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnEnd
    ));
    assert!(matches!(
        recv_wm(&mut wm_rx),
        wm_controller::WmEvent::SpaceStateUpdated(state, _) if state.screens.iter().map(|s| s.space).collect::<Vec<_>>() == vec![Some(space)]
    ));
    assert_no_wm_event(&mut wm_rx);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn wake_does_not_flush_pending_updates_while_churn_is_still_active() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let space = SpaceId::new(31);

    actor.handle_event(Event::SystemWillSleep);
    actor.handle_event(Event::DisplayChurnBegin);
    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(space))],
        CoordinateConverter::default(),
    ));
    actor.handle_event(Event::SystemDidWake);

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));
    assert_no_wm_event(&mut wm_rx);

    actor.handle_event(Event::DisplayChurnEnd);

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnEnd
    ));
    assert!(matches!(
        recv_wm(&mut wm_rx),
        wm_controller::WmEvent::SpaceStateUpdated(..)
    ));
    assert_no_wm_event(&mut wm_rx);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn drops_duplicate_space_snapshots_after_flush() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let space = SpaceId::new(41);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(space))],
        CoordinateConverter::default(),
    ));
    let _ = recv_wm(&mut wm_rx);
    actor.handle_event(Event::SpaceChanged(vec![Some(space)]));
    assert_no_wm_event(&mut wm_rx);

    actor.handle_event(Event::DisplayChurnBegin);
    actor.handle_event(Event::SpaceChanged(vec![Some(space)]));
    actor.handle_event(Event::DisplayChurnEnd);

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnEnd
    ));
    assert_no_wm_event(&mut wm_rx);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn retains_only_latest_pending_screen_snapshot_during_churn() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();

    actor.handle_event(Event::DisplayChurnBegin);
    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(SpaceId::new(51)))],
        CoordinateConverter::from_height(10.0),
    ));
    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(SpaceId::new(52)))],
        CoordinateConverter::from_height(20.0),
    ));
    actor.handle_event(Event::DisplayChurnEnd);

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnEnd
    ));
    let forwarded = recv_wm(&mut wm_rx);
    match forwarded {
        wm_controller::WmEvent::SpaceStateUpdated(state, converter) => {
            assert_eq!(state.screens[0].space, Some(SpaceId::new(52)));
            assert_eq!(converter.screen_height(), Some(20.0));
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn forwards_space_lifecycle_events_through_actor_even_during_churn() {
    let (mut actor, _wm_rx, mut reactor_rx) = build_actor();
    let space = SpaceId::new(61);

    actor.handle_event(Event::DisplayChurnBegin);
    actor.handle_event(Event::SpaceCreated(space));
    actor.handle_event(Event::SpaceDestroyed(space));

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::SpaceCreated(seen) if seen == space
    ));
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::SpaceDestroyed(seen) if seen == space
    ));
}

#[test]
fn topology_delta_uses_last_forwarded_screens_as_diff_base() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-1", 0.0, 1000.0, Some(SpaceId::new(1)))],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-1", 0.0, 1000.0, Some(SpaceId::new(1))),
            make_screen_with(2, "display-2", 1000.0, 1200.0, Some(SpaceId::new(2))),
        ],
        CoordinateConverter::from_height(800.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert!(state.display_set_changed);
            assert!(state.topology_changed);
            assert!(state.allow_space_remap);
            assert!(state.should_force_refresh_layout);
            assert_eq!(state.resized_spaces.len(), 1);
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn space_only_updates_retain_last_coordinate_converter() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let space = SpaceId::new(71);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(space))],
        CoordinateConverter::from_height(900.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::SpaceChanged(vec![Some(space)]));
    assert_no_wm_event(&mut wm_rx);
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn space_inventory_changes_force_a_fresh_forwarded_snapshot() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let space = SpaceId::new(72);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen(Some(space))],
        CoordinateConverter::from_height(900.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::SpaceInventoryChanged);

    assert!(matches!(
        recv_wm(&mut wm_rx),
        wm_controller::WmEvent::SpaceStateUpdated(state, _)
            if state.screens.iter().map(|screen| screen.space).collect::<Vec<_>>() == vec![Some(space)]
    ));
    assert_no_reactor_event(&mut reactor_rx);
}

#[test]
fn fullscreen_transition_is_normalized_before_forwarding() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let left_space_2 = SpaceId::new(12);
    let left_space_1 = SpaceId::new(11);
    let right_space_1 = SpaceId::new(21);
    let right_fullscreen = fullscreen_space_for(right_space_1);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left_space_2)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_space_1)),
        ],
        CoordinateConverter::from_height(1000.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left_space_1)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_fullscreen)),
        ],
        CoordinateConverter::from_height(1000.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            let spaces: Vec<Option<SpaceId>> =
                state.screens.iter().map(|screen| screen.space).collect();
            assert_eq!(spaces, vec![Some(left_space_1), None]);
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn topology_change_emits_space_remap_from_display_history() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let original_space = SpaceId::new(31);
    let remapped_space = SpaceId::new(41);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-1", 0.0, 1000.0, Some(original_space))],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-1", 0.0, 1000.0, Some(remapped_space)),
            make_screen_with(2, "display-2", 1000.0, 1000.0, Some(SpaceId::new(51))),
        ],
        CoordinateConverter::from_height(800.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert_eq!(state.space_remaps, vec![(original_space, remapped_space)]);
            assert!(state.allow_space_remap);
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn sleep_wake_display_reattach_flushes_latest_stable_spaces_only() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let left = SpaceId::new(201);
    let right = SpaceId::new(202);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right)),
        ],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::SystemWillSleep);
    actor.handle_event(Event::DisplayChurnBegin);
    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-left", 0.0, 1000.0, Some(left))],
        CoordinateConverter::from_height(800.0),
    ));
    actor.handle_event(Event::SpaceChanged(vec![Some(left)]));
    actor.handle_event(Event::SystemDidWake);
    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right)),
        ],
        CoordinateConverter::from_height(800.0),
    ));
    actor.handle_event(Event::SpaceChanged(vec![Some(left), Some(right)]));
    actor.handle_event(Event::DisplayChurnEnd);

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnEnd
    ));
    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert_eq!(
                state.screens.iter().map(|screen| screen.space).collect::<Vec<_>>(),
                vec![Some(left), Some(right)]
            );
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn topology_window_delta_is_emitted_when_windows_leave_space_during_churn_without_space_change() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let space = SpaceId::new(301);
    let wsid = WindowServerId::new(77);

    actor.state.visible_window_spaces.insert(wsid, space);
    actor.state.pre_churn_visible_window_spaces.insert(wsid, space);
    actor.state.display_churn_flags = crate::sys::skylight::DisplayReconfigFlags::MOVED;

    actor.forward_screen_parameters(
        vec![make_screen(Some(space))],
        CoordinateConverter::from_height(800.0),
    );
    let _ = recv_wm(&mut wm_rx);

    actor.state.visible_window_spaces.clear();
    actor.synthesize_topology_window_delta(9, actor.state.display_churn_flags, &[make_screen(Some(space))]);
    actor.forward_screen_parameters(
        vec![make_screen(Some(space))],
        CoordinateConverter::from_height(800.0),
    );

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            let delta = state
                .topology_window_delta
                .expect("expected topology window delta");
            assert_eq!(delta.epoch, 9);
            assert!(delta.appeared.is_empty());
            assert_eq!(delta.disappeared, vec![(wsid, space)]);
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn topology_window_delta_treats_same_window_space_move_as_remove_then_add() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let old_space = SpaceId::new(311);
    let new_space = SpaceId::new(312);
    let wsid = WindowServerId::new(78);

    actor.state.visible_window_spaces.insert(wsid, old_space);
    actor.state.pre_churn_visible_window_spaces.insert(wsid, old_space);
    actor.state.display_churn_flags = crate::sys::skylight::DisplayReconfigFlags::MOVED;

    actor.forward_screen_parameters(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(old_space)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(new_space)),
        ],
        CoordinateConverter::from_height(800.0),
    );
    let _ = recv_wm(&mut wm_rx);

    actor.state.visible_window_spaces.clear();
    actor.state.visible_window_spaces.insert(wsid, new_space);
    actor.synthesize_topology_window_delta(
        10,
        actor.state.display_churn_flags,
        &[
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(old_space)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(new_space)),
        ],
    );
    actor.forward_screen_parameters(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(old_space)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(new_space)),
        ],
        CoordinateConverter::from_height(800.0),
    );

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            let delta = state
                .topology_window_delta
                .expect("expected topology window delta");
            assert_eq!(delta.disappeared, vec![(wsid, old_space)]);
            assert_eq!(delta.appeared, vec![(wsid, new_space)]);
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn display_order_change_is_topology_change_without_display_set_change() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let left_space = SpaceId::new(401);
    let right_space = SpaceId::new(402);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left_space)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_space)),
        ],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_space)),
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left_space)),
        ],
        CoordinateConverter::from_height(800.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert!(!state.display_set_changed);
            assert!(state.topology_changed);
            assert!(state.should_force_refresh_layout);
            assert!(state.space_remaps.is_empty());
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn duplicate_space_transient_during_wake_is_not_forwarded_when_stable_snapshot_recovers() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let left = SpaceId::new(501);
    let right = SpaceId::new(502);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right)),
        ],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::SystemWillSleep);
    actor.handle_event(Event::DisplayChurnBegin);
    actor.handle_event(Event::SpaceChanged(vec![Some(left), Some(left)]));
    actor.handle_event(Event::SystemDidWake);
    actor.handle_event(Event::SpaceChanged(vec![Some(left), Some(right)]));
    actor.handle_event(Event::DisplayChurnEnd);

    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnEnd
    ));
    assert_no_wm_event(&mut wm_rx);
}

#[test]
fn fullscreen_transition_tracks_display_identity_across_reordered_screens() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let left_space_2 = SpaceId::new(12);
    let left_space_1 = SpaceId::new(11);
    let right_space_1 = SpaceId::new(21);
    let right_fullscreen = fullscreen_space_for(right_space_1);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left_space_2)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_space_1)),
        ],
        CoordinateConverter::from_height(1000.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_fullscreen)),
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left_space_1)),
        ],
        CoordinateConverter::from_height(1000.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert_eq!(state.screens[0].display_uuid, "display-right");
            assert_eq!(state.screens[0].space, None);
            assert_eq!(state.screens[1].display_uuid, "display-left");
            assert_eq!(state.screens[1].space, Some(left_space_1));
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn fullscreen_transition_rewrites_cross_display_space_contamination_only() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let left_space = SpaceId::new(212);
    let right_space = SpaceId::new(221);
    let right_fullscreen = fullscreen_space_for(right_space);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(left_space)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_space)),
        ],
        CoordinateConverter::from_height(1000.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-left", 0.0, 1000.0, Some(right_space)),
            make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right_fullscreen)),
        ],
        CoordinateConverter::from_height(1000.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            let spaces: Vec<Option<SpaceId>> =
                state.screens.iter().map(|screen| screen.space).collect();
            assert_eq!(spaces, vec![Some(left_space), None]);
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn display_churn_stabilization_rejects_duplicate_space_snapshot_until_valid() {
    let (mut actor, mut wm_rx, mut reactor_rx) = build_actor();
    let left = SpaceId::new(601);
    let right = SpaceId::new(602);

    actor.handle_event(Event::DisplayChurnBegin);
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnBegin
    ));

    let epoch = actor.state.display_churn_epoch;
    actor.state.screens = vec![
        make_screen_with(1, "display-left", 0.0, 1000.0, Some(left)),
        make_screen_with(2, "display-right", 1000.0, 1000.0, Some(left)),
    ];

    actor.attempt_finish_display_churn(epoch, 0);
    actor.attempt_finish_display_churn(epoch, 1);
    assert_no_wm_event(&mut wm_rx);

    actor.state.screens = vec![
        make_screen_with(1, "display-left", 0.0, 1000.0, Some(left)),
        make_screen_with(2, "display-right", 1000.0, 1000.0, Some(right)),
    ];

    actor.attempt_finish_display_churn(epoch, 2);
    actor.attempt_finish_display_churn(epoch, 3);

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert_eq!(
                state.screens.iter().map(|screen| screen.space).collect::<Vec<_>>(),
                vec![Some(left), Some(right)]
            );
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
    assert!(matches!(
        recv_reactor(&mut reactor_rx),
        reactor::Event::DisplayChurnEnd
    ));
}

#[test]
fn mismatched_space_snapshot_count_falls_back_to_authoritative_screen_state() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-1", 0.0, 1000.0, Some(SpaceId::new(81))),
            make_screen_with(2, "display-2", 1000.0, 1000.0, Some(SpaceId::new(82))),
        ],
        CoordinateConverter::from_height(1000.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::SpaceChanged(vec![Some(SpaceId::new(99))]));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert_eq!(
                state.screens.iter().map(|screen| screen.space).collect::<Vec<_>>(),
                vec![Some(SpaceId::new(81)), Some(SpaceId::new(82))]
            );
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn duplicate_visible_spaces_disable_remaps_and_layout_forcing() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-1", 0.0, 1000.0, Some(SpaceId::new(91)))],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![
            make_screen_with(1, "display-1", 0.0, 1000.0, Some(SpaceId::new(92))),
            make_screen_with(2, "display-2", 1000.0, 1000.0, Some(SpaceId::new(92))),
        ],
        CoordinateConverter::from_height(800.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert!(state.display_set_changed);
            assert!(state.topology_changed);
            assert!(!state.allow_space_remap);
            assert!(state.space_remaps.is_empty());
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn resize_updates_are_treated_as_topology_changes_and_report_resized_spaces() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let space = SpaceId::new(101);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-1", 0.0, 1000.0, Some(space))],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-1", 0.0, 1200.0, Some(space))],
        CoordinateConverter::from_height(800.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert!(!state.display_set_changed);
            assert!(state.topology_changed);
            assert!(state.should_force_refresh_layout);
            assert_eq!(state.resized_spaces, vec![(space, CGSize::new(1200.0, 800.0))]);
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}

#[test]
fn display_origin_change_is_treated_as_topology_change() {
    let (mut actor, mut wm_rx, _reactor_rx) = build_actor();
    let space = SpaceId::new(111);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-1", 0.0, 1000.0, Some(space))],
        CoordinateConverter::from_height(800.0),
    ));
    let _ = recv_wm(&mut wm_rx);

    actor.handle_event(Event::ScreenParametersChanged(
        vec![make_screen_with(1, "display-1", 200.0, 1000.0, Some(space))],
        CoordinateConverter::from_height(800.0),
    ));

    match recv_wm(&mut wm_rx) {
        wm_controller::WmEvent::SpaceStateUpdated(state, _) => {
            assert!(!state.display_set_changed);
            assert!(state.topology_changed);
            assert!(state.should_force_refresh_layout);
            assert!(state.resized_spaces.is_empty());
        }
        other => panic!("unexpected wm event: {other:?}"),
    }
}
