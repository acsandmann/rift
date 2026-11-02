use rift_protocol::WindowClaimFlags;
use test_log::test;

use super::*;
use crate::model::window_store::ClaimOwner;

const OWNER: ClaimOwner = ClaimOwner(101);
const OTHER: ClaimOwner = ClaimOwner(202);
const NONE: WindowClaimFlags = WindowClaimFlags::empty();
const SUPPRESS: WindowClaimFlags = WindowClaimFlags::SUPPRESS_FOCUS_FOLLOWS_MOUSE;
const SCREEN: CGRect = CGRect {
    origin: CGPoint { x: 0., y: 0. },
    size: CGSize { width: 1200., height: 1000. },
};

fn settled(count: usize) -> (Apps, Reactor, SpaceId) {
    let (mut apps, mut reactor) = test_context();
    let space = SpaceId::new(1);
    apps.make_app_and_settle_on_screen(&mut reactor, SCREEN, space, 1, make_windows(count));
    (apps, reactor, space)
}

fn wid(idx: u32) -> WindowId { WindowId::new(1, idx) }

fn tiled(reactor: &mut Reactor, space: SpaceId) -> Vec<WindowId> {
    test_layout(reactor, space, SCREEN).into_iter().map(|(wid, _)| wid).collect()
}

fn layout_updates(reactor: &mut Reactor, change: impl FnOnce(&mut Reactor)) -> usize {
    let before = reactor.layout_update_count;
    change(reactor);
    reactor.layout_update_count - before
}

fn focus(
    reactor: &mut Reactor,
    window: WindowId,
    space: SpaceId,
) -> actor::Receiver<input::Request> {
    let (input_tx, input_rx) = actor::channel();
    reactor.communication_manager.input_tx = Some(input_tx);
    let _ = reactor
        .main_window_tracker
        .handle_event(&Event::ApplicationGloballyActivated(window.pid));
    let _ = reactor
        .main_window_tracker
        .handle_event(&Event::WindowServerFocusChanged(window, space));
    assert_eq!(reactor.main_window(), Some(window));
    input_rx
}

fn ffm_updates(rx: &mut actor::Receiver<input::Request>) -> Vec<bool> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|(_, request)| match request {
            input::Request::SetFocusFollowsMouseEnabled(enabled) => Some(enabled),
            _ => None,
        })
        .collect()
}

#[test]
fn claim_detaches_without_destroying_and_release_restores_the_slot() {
    let (mut apps, mut reactor, space) = settled(3);
    let workspace = reactor.state.windows.workspace_for_window(wid(2)).unwrap();
    let before = tiled(&mut reactor, space);
    let parked = apps.windows[&wid(2)].frame;

    assert_eq!(reactor.claim_window(wid(2), OWNER, NONE), Ok(()));
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(tiled(&mut reactor, space), vec![wid(1), wid(3)]);
    assert!(
        apps.windows[&wid(1)].frame.size.width > parked.size.width,
        "siblings reflow"
    );
    assert_eq!(
        apps.windows[&wid(2)].frame,
        parked,
        "Rift never moves a claimed window"
    );
    assert!(reactor.state.windows.contains_window(wid(2)));
    assert_eq!(reactor.state.windows.workspace_for_window(wid(2)), None);
    assert_eq!(
        reactor.state.windows.claim(wid(2)).unwrap().workspace(),
        Some(workspace)
    );

    let info = reactor.query_window_info(wid(2)).unwrap();
    assert!(info.externally_managed && !info.is_floating);
    assert!(
        reactor
            .query_windows(Some(space))
            .iter()
            .any(|window| window.id == wid(2) && window.externally_managed)
    );
    assert!(!reactor.query_window_info(wid(1)).unwrap().externally_managed);

    // No layout path can reintroduce the window while it is claimed.
    reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid(2)));
    reactor.discover_test_windows(1, vec![], vec![wid(1), wid(2), wid(3)]);
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(tiled(&mut reactor, space), vec![wid(1), wid(3)]);

    assert_eq!(reactor.release_window(wid(2), OWNER), Ok(()));
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(tiled(&mut reactor, space), before);
    assert_eq!(
        reactor.state.windows.workspace_for_window(wid(2)),
        Some(workspace)
    );
    assert!(!reactor.query_window_info(wid(2)).unwrap().externally_managed);
    reactor.state.windows.debug_assert_invariants();
}

#[test]
fn ownership_is_exclusive_idempotent_and_validated() {
    let (_, mut reactor, _) = settled(2);
    assert_eq!(reactor.claim_window(wid(1), OWNER, NONE), Ok(()));
    assert_eq!(reactor.claim_window(wid(1), OWNER, NONE), Ok(()));
    let owned_elsewhere = Err("Window owned by another external manager");
    assert_eq!(reactor.claim_window(wid(1), OTHER, NONE), owned_elsewhere);
    assert_eq!(reactor.release_window(wid(1), OTHER), owned_elsewhere);
    assert_eq!(
        reactor.claim_window(wid(1), OWNER, WindowClaimFlags::from_bits_retain(2)),
        Err("Unsupported window claim flags")
    );
    assert_eq!(
        reactor.claim_window(WindowId::new(9, 9), OWNER, NONE),
        Err("Window not found")
    );
    assert_eq!(
        reactor.release_window(WindowId::new(9, 9), OWNER),
        Err("Window not found")
    );
    assert_eq!(
        reactor.release_window(wid(2), OTHER),
        Ok(()),
        "releasing an unclaimed window"
    );
    assert_eq!(reactor.state.windows.claim(wid(1)).unwrap().owner, OWNER);
    assert_eq!(reactor.state.windows.owner_claims(OWNER), &[wid(1)]);
    assert!(reactor.state.windows.owner_claims(OTHER).is_empty());
}

#[test]
fn claims_cost_one_arrangement_and_flag_updates_cost_none() {
    let (mut apps, mut reactor, space) = settled(3);
    assert_eq!(
        layout_updates(&mut reactor, |r| r.claim_window(wid(1), OWNER, NONE).unwrap()),
        1
    );
    apps.simulate_until_quiet(&mut reactor);
    let frames = apps.windows.clone();
    for flags in [NONE, SUPPRESS, NONE] {
        assert_eq!(
            layout_updates(&mut reactor, |r| r.claim_window(wid(1), OWNER, flags).unwrap()),
            0
        );
    }
    assert_eq!(reactor.state.windows.claim(wid(1)).unwrap().flags, NONE);
    assert!(apps.requests().is_empty());
    assert_eq!(apps.windows, frames);

    // An unadmitted window has no projection to remove.
    reactor.state.windows.window_mut(wid(2)).unwrap().manage_override = Some(false);
    reactor.send_layout_event(LayoutEvent::WindowRemoved(wid(2)));
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(
        layout_updates(&mut reactor, |r| r.claim_window(wid(2), OWNER, NONE).unwrap()),
        0
    );

    // A disconnect returns every window and arranges each affected space once.
    reactor.claim_window(wid(3), OWNER, NONE).unwrap();
    apps.simulate_until_quiet(&mut reactor);
    assert!(tiled(&mut reactor, space).is_empty());
    assert_eq!(layout_updates(&mut reactor, |r| r.release_claims(OWNER)), 1);
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(tiled(&mut reactor, space), vec![wid(1), wid(3)]);
    assert!(reactor.state.windows.owner_claims(OWNER).is_empty());
    assert!(
        !reactor.state.windows.is_admitted(wid(2)),
        "rules still govern a released window"
    );
    assert_eq!(layout_updates(&mut reactor, |r| r.release_claims(OWNER)), 0);
}

#[test]
fn disconnect_releases_only_that_owners_claims() {
    let (mut apps, mut reactor, space) = settled(3);
    reactor.claim_window(wid(1), OWNER, NONE).unwrap();
    reactor.claim_window(wid(2), OTHER, SUPPRESS).unwrap();
    reactor.release_claims(OWNER);
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(tiled(&mut reactor, space), vec![wid(1), wid(3)]);
    assert_eq!(reactor.state.windows.claim(wid(2)).unwrap().owner, OTHER);
    assert_eq!(reactor.state.windows.owner_claims(OTHER), &[wid(2)]);
}

#[test]
fn claimed_floating_window_keeps_its_classification_and_adopts_owner_geometry() {
    let (mut reactor, wid, space, _, floating_frame) = reactor_with_floating_window();
    let workspace = reactor.state.windows.workspace_for_window(wid).unwrap();
    reactor.claim_window(wid, OWNER, NONE).unwrap();
    assert!(reactor.layout_manager.layout_engine.is_window_floating(wid));
    assert_eq!(reactor.state.windows.workspace_for_window(wid), None);

    // The owner moves it; Rift neither records nor fights the geometry.
    let moved = CGRect::new(CGPoint::new(300., 200.), floating_frame.size);
    reactor.handle_event(Event::WindowFrameChanged(
        wid,
        moved,
        None,
        Requested(false),
        None,
    ));
    assert_eq!(reactor.state.windows.window(wid).unwrap().frame_monotonic, moved);
    assert!(!reactor.is_in_drag(), "an owner move is not a Rift drag");
    let stored = |r: &Reactor| r.layout_manager.layout_engine.get_floating_position(workspace, wid);
    assert_eq!(stored(&reactor), Some(floating_frame));

    reactor.release_window(wid, OWNER).unwrap();
    assert!(reactor.layout_manager.layout_engine.is_window_floating(wid));
    assert_eq!(stored(&reactor), Some(moved));
    assert!(reactor.test_active_workspace_windows(space).contains(&wid));
}

#[test]
fn claim_cancels_drag_animation_and_pending_geometry() {
    let (mut reactor, wid, wsid, space, _, frame) = reactor_with_window_on_space1();
    reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
    reactor.drag_manager.actor.begin_modifier(
        crate::actor::drag::DragSource {
            window: wid,
            origin_frame: frame,
            last_frame: frame,
            origin_space: Some(space),
            current_space: Some(space),
            tiled: true,
        },
        frame.mid(),
        crate::common::config::MouseAction::Move,
        crate::actor::drag::DragScene::default(),
    );
    reactor.drag_manager.externally_controlled_window = Some(wid);
    let txid = reactor.transaction_manager.generate_next_txid(wsid);
    reactor.transaction_manager.store_txid(wsid, txid, frame);
    let (animation_tx, animation_rx) = animation::AnimationSender::channel();
    reactor.animation_tx = Some(animation_tx);

    reactor.claim_window(wid, OWNER, NONE).unwrap();
    assert!(!reactor.drag_manager.actor.is_active());
    assert_eq!(reactor.drag_manager.externally_controlled_window, None);
    assert_eq!(reactor.transaction_manager.get_target_frame(wsid), None);
    assert!(
        std::iter::from_fn(|| animation_rx.commands.try_recv().ok())
            .any(|message| matches!(message, animation::Message::Stop(ids) if ids == [wid]))
    );

    // Rift-controlled drags never start on a claimed window.
    let moved = CGRect::new(CGPoint::new(50., 50.), frame.size);
    reactor.handle_event(Event::WindowFrameChanged(
        wid,
        moved,
        None,
        Requested(false),
        Some(crate::sys::event::MouseState::Down),
    ));
    assert!(!reactor.is_in_drag());
    assert!(!reactor.test_active_workspace_windows(space).contains(&wid));
}

#[test]
fn claim_follows_ax_rekey_and_release_accepts_the_original_id() {
    let (mut apps, mut reactor, space) = settled(2);
    reactor.claim_window(wid(1), OWNER, SUPPRESS).unwrap();
    let replacement = wid(7);
    let info = reactor.state.windows.window(wid(1)).unwrap().info.clone();
    reactor.discover_test_windows(1, vec![(replacement, info)], vec![replacement, wid(2)]);
    apps.simulate_until_quiet(&mut reactor);
    let claim = reactor.state.windows.claim(replacement).unwrap();
    assert_eq!((claim.owner, claim.flags), (OWNER, SUPPRESS));
    assert!(!reactor.state.windows.contains_window(wid(1)));
    assert!(!tiled(&mut reactor, space).contains(&replacement));
    assert_eq!(reactor.state.windows.owner_claims(OWNER), &[replacement]);

    // The client only knows the id it claimed.
    assert_eq!(reactor.claim_window(wid(1), OWNER, NONE), Ok(()));
    assert_eq!(reactor.state.windows.claim(replacement).unwrap().flags, NONE);
    assert_eq!(reactor.release_window(wid(1), OTHER), Err("Window not found"));
    assert_eq!(reactor.release_window(wid(1), OWNER), Ok(()));
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(tiled(&mut reactor, space), vec![replacement, wid(2)]);
    reactor.state.windows.debug_assert_invariants();
}

#[test]
fn destroying_a_claimed_window_ends_its_claim() {
    let (mut apps, mut reactor, space) = settled(2);
    reactor.claim_window(wid(1), OWNER, NONE).unwrap();
    reactor.handle_event(Event::WindowDestroyed(wid(1)));
    apps.simulate_until_quiet(&mut reactor);
    assert!(reactor.state.windows.owner_claims(OWNER).is_empty());
    assert_eq!(reactor.release_window(wid(1), OWNER), Err("Window not found"));
    assert_eq!(layout_updates(&mut reactor, |r| r.release_claims(OWNER)), 0);
    assert_eq!(tiled(&mut reactor, space), vec![wid(2)]);
    reactor.state.windows.debug_assert_invariants();
}

#[test]
fn claimed_window_keeps_its_workspace_across_workspace_switches() {
    let (mut apps, mut reactor) = test_context_with_workspace_count(2);
    let space = SpaceId::new(1);
    apps.make_app_and_settle_on_screen(&mut reactor, SCREEN, space, 1, make_windows(2));
    let [home, other] = reactor.test_workspace_ids(space)[..] else {
        panic!()
    };
    let parked = apps.windows[&wid(1)].frame;
    reactor.claim_window(wid(1), OWNER, NONE).unwrap();

    // Neither hiding nor revealing a workspace touches the claimed window.
    for workspace in [other, home] {
        assert!(reactor.set_test_active_workspace(space, workspace));
        reactor.update_layout_or_warn(false, true, Some(space));
        apps.simulate_until_quiet(&mut reactor);
        assert!(!tiled(&mut reactor, space).contains(&wid(1)));
        assert_eq!(apps.windows[&wid(1)].frame, parked);
    }

    assert!(reactor.set_test_active_workspace(space, other));
    reactor.release_window(wid(1), OWNER).unwrap();
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(reactor.state.windows.workspace_for_window(wid(1)), Some(home));
    assert!(reactor.state.windows.workspace_windows(home).contains(&wid(1)));
    assert!(!tiled(&mut reactor, space).contains(&wid(1)));
    assert_ne!(
        apps.windows[&wid(1)].frame,
        parked,
        "released into a hidden workspace"
    );
}

#[test]
fn display_churn_while_claimed_rehomes_the_window_on_release() {
    let (mut reactor, wid, wsid, space1, space2, _, screen2) =
        reactor_with_window_on_space1_two_displays();
    reactor.send_layout_event(LayoutEvent::WindowAdded(space1, wid));
    reactor.claim_window(wid, OWNER, NONE).unwrap();

    // Display 1 disappears; macOS moves the window to display 2.
    reactor.state.windows.set_window_server_space(wsid, Some(space2));
    reactor.handle_event(space_state_event(vec![screen2], vec![Some(space2)]));
    reactor.reassign_window_to_authoritative_space(wid, space2, true);
    assert!(reactor.state.windows.is_claimed(wid));
    assert!(!reactor.test_active_workspace_windows(space2).contains(&wid));

    reactor.release_window(wid, OWNER).unwrap();
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert!(reactor.test_active_workspace_windows(space2).contains(&wid));
    reactor.state.windows.debug_assert_invariants();
}

#[test]
fn fullscreen_exit_while_claimed_waits_for_release() {
    let (mut reactor, wid, wsid, space, _, _) = reactor_with_window_on_space1();
    reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
    let workspace = reactor.state.windows.workspace_for_window(wid).unwrap();
    reactor.claim_window(wid, OWNER, NONE).unwrap();
    let fullscreen = SpaceId::new(0x400000001);
    reactor.state.windows.suspend_window_to_native_fullscreen(
        wid,
        Some(wsid),
        Some(space),
        fullscreen,
        NativeFullscreenTransition::Suspended,
    );

    let outcome = reactor.restore_native_fullscreen(Some(wsid), wid, Some(space));
    reactor.apply_event_outcome(outcome);
    assert!(reactor.state.windows.is_claimed(wid));
    assert!(reactor.state.windows.native_fullscreen_record_for_window(wid).is_some());
    assert!(!reactor.test_active_workspace_windows(space).contains(&wid));

    reactor.release_window(wid, OWNER).unwrap();
    assert!(reactor.state.windows.native_fullscreen_record_for_window(wid).is_none());
    assert_eq!(reactor.state.windows.workspace_for_window(wid), Some(workspace));
    assert!(reactor.test_active_workspace_windows(space).contains(&wid));
}

#[test]
fn suppression_applies_only_while_the_flagged_claim_is_focused() {
    let (mut reactor, window, _, space, _, frame) = reactor_with_window_on_space1();
    let other = WindowId::new(window.pid, 2);
    reactor.add_test_window(other, WindowServerId::new(102), Some(space), frame);
    let mut rx = focus(&mut reactor, window, space);

    reactor.claim_window(other, OWNER, SUPPRESS).unwrap();
    assert!(
        ffm_updates(&mut rx).is_empty(),
        "an unfocused claim changes nothing"
    );
    reactor.claim_window(window, OWNER, NONE).unwrap();
    assert!(ffm_updates(&mut rx).is_empty());
    reactor.claim_window(window, OWNER, SUPPRESS).unwrap();
    assert_eq!(ffm_updates(&mut rx), [false]);
    reactor.claim_window(window, OWNER, SUPPRESS).unwrap();
    assert!(ffm_updates(&mut rx).is_empty());
    reactor.claim_window(window, OWNER, NONE).unwrap();
    assert_eq!(ffm_updates(&mut rx), [true]);
    reactor.claim_window(window, OWNER, SUPPRESS).unwrap();
    assert_eq!(ffm_updates(&mut rx), [false]);

    // Focus moving to another window re-enables it; returning suppresses again.
    reactor.release_window(other, OWNER).unwrap();
    reactor.handle_event(Event::WindowServerFocusChanged(other, space));
    assert_eq!(reactor.main_window(), Some(other));
    assert!(ffm_updates(&mut rx).contains(&true));
    reactor.handle_event(Event::WindowServerFocusChanged(window, space));
    assert!(ffm_updates(&mut rx).contains(&false));

    reactor.release_claims(OWNER);
    assert_eq!(ffm_updates(&mut rx), [true]);
}

#[test]
fn suppression_respects_menu_mission_control_and_config() {
    let (mut reactor, window, _, space, _, _) = reactor_with_window_on_space1();
    let mut rx = focus(&mut reactor, window, space);
    reactor.claim_window(window, OWNER, SUPPRESS).unwrap();
    assert_eq!(ffm_updates(&mut rx), [false]);

    reactor.menu_manager.menu_state = MenuState::Open(window.pid);
    reactor.release_window(window, OWNER).unwrap();
    assert_eq!(ffm_updates(&mut rx), [false]);
    reactor.menu_manager.menu_state = MenuState::Closed;

    reactor.claim_window(window, OWNER, SUPPRESS).unwrap();
    reactor.set_mission_control_active(true);
    ffm_updates(&mut rx);
    reactor.release_window(window, OWNER).unwrap();
    assert_eq!(ffm_updates(&mut rx), [false]);
    reactor.set_mission_control_active(false);

    reactor.claim_window(window, OWNER, SUPPRESS).unwrap();
    ffm_updates(&mut rx);
    reactor.config.settings.focus_follows_mouse = false;
    reactor.release_window(window, OWNER).unwrap();
    assert_eq!(ffm_updates(&mut rx), [false]);
}

#[test]
fn destroying_the_focused_suppressing_window_restores_focus_follows_mouse() {
    let (mut reactor, window, _, space, _, _) = reactor_with_window_on_space1();
    let mut rx = focus(&mut reactor, window, space);
    reactor.claim_window(window, OWNER, SUPPRESS).unwrap();
    assert_eq!(ffm_updates(&mut rx), [false]);
    reactor.handle_event(Event::WindowDestroyed(window));
    assert!(ffm_updates(&mut rx).contains(&true));
}

#[test]
fn unclaimed_windows_do_not_refresh_focus_policy_on_focus_changes() {
    let (mut reactor, window, _, space, _, frame) = reactor_with_window_on_space1();
    let other = WindowId::new(window.pid, 2);
    reactor.add_test_window(other, WindowServerId::new(102), Some(space), frame);
    let mut rx = focus(&mut reactor, window, space);
    reactor.handle_event(Event::WindowServerFocusChanged(other, space));
    reactor.handle_event(Event::WindowServerFocusChanged(window, space));
    assert!(ffm_updates(&mut rx).is_empty());
}
