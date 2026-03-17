use objc2_core_foundation::{CGPoint, CGSize};
use test_log::test;

use super::display_topology::TopologyState;
use super::events::window::WindowEventHandler;
use super::testing::*;
use super::*;
use crate::actor::app::Request;
use crate::layout_engine::{Direction, LayoutCommand, LayoutEngine};
use crate::sys::app::WindowInfo;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

fn test_reactor() -> Reactor {
    Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ))
}

fn native_tab_test_setup(space: u64) -> (Apps, Reactor, SpaceId) {
    let mut apps = Apps::new();
    let mut reactor = test_reactor();
    let space = SpaceId::new(space);
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![screen], vec![Some(space)], vec![]));

    reactor.handle_events(apps.make_app(1, vec![make_window(1)]));
    apps.simulate_until_quiet(&mut reactor);
    let _ = apps.requests();

    (apps, reactor, space)
}

fn replacement_tab(
    reactor: &Reactor,
    old_wid: WindowId,
    new_window_number: u32,
) -> (WindowId, WindowInfo, WindowServerInfo) {
    let mut replacement = make_window(new_window_number as usize);
    replacement.frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let ws_info = WindowServerInfo {
        id: WindowServerId::new(new_window_number),
        pid: old_wid.pid,
        layer: 0,
        frame: replacement.frame,
        min_frame: CGSize::ZERO,
        max_frame: CGSize::ZERO,
    };
    (
        WindowId::new(old_wid.pid, new_window_number),
        replacement,
        ws_info,
    )
}

fn create_native_tab_replacement(
    reactor: &mut Reactor,
    space: SpaceId,
    old_ws_id: WindowServerId,
    new_wid: WindowId,
    replacement: WindowInfo,
    replacement_ws_info: WindowServerInfo,
) -> bool {
    assert!(reactor.stage_native_tab_destroy(old_ws_id, space));
    WindowEventHandler::handle_window_created(
        reactor,
        new_wid,
        replacement,
        Some(replacement_ws_info),
        Some(MouseState::Up),
    );
    reactor.note_native_tab_appearance(replacement_ws_info.id, space, replacement_ws_info)
}

fn create_three_tab_group(reactor: &mut Reactor, space: SpaceId) -> (WindowId, WindowId, WindowId) {
    let first = WindowId::new(1, 1);
    let (second, second_info, second_ws_info) = replacement_tab(reactor, first, 2);
    assert!(create_native_tab_replacement(
        reactor,
        space,
        WindowServerId::new(1),
        second,
        second_info,
        second_ws_info,
    ));

    let (third, third_info, third_ws_info) = replacement_tab(reactor, second, 3);
    assert!(create_native_tab_replacement(
        reactor,
        space,
        WindowServerId::new(2),
        third,
        third_info,
        third_ws_info,
    ));

    (first, second, third)
}

fn assert_native_tab_switch_state(
    reactor: &mut Reactor,
    space: SpaceId,
    old_wid: WindowId,
    new_wid: WindowId,
) {
    let old = reactor
        .window_manager
        .windows
        .get(&old_wid)
        .expect("old tab should remain tracked as suppressed");
    let new = reactor
        .window_manager
        .windows
        .get(&new_wid)
        .expect("new tab should become the active managed slot");
    assert!(old.is_native_tab_suppressed());
    assert_eq!(
        new.native_tab.expect("new tab should be part of a native tab group").role,
        NativeTabRole::Active
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.selected_window(space),
        Some(new_wid)
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![new_wid]
    );
}

fn native_tab_window_and_space_setup(space: u64, windows: usize) -> (Apps, Reactor, SpaceId) {
    let mut apps = Apps::new();
    let mut reactor = test_reactor();
    let space = SpaceId::new(space);
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![screen], vec![Some(space)], vec![]));

    reactor.handle_events(apps.make_app(1, make_windows(windows)));
    apps.simulate_until_quiet(&mut reactor);
    let _ = apps.requests();

    (apps, reactor, space)
}

fn assert_window_removed_from_layout(reactor: &Reactor, space: SpaceId, wid: WindowId) {
    assert!(!reactor.window_manager.windows.contains_key(&wid));
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .windows_in_active_workspace(space)
            .is_empty()
    );
}

fn assert_has_set_window_frame_request(requests: &[Request], wid: WindowId, frame: CGRect) {
    assert!(requests.iter().any(|request| matches!(
        request,
        Request::SetWindowFrame(req_wid, req_frame, _, _) if *req_wid == wid && *req_frame == frame
    )));
}

#[test]
fn it_ignores_stale_resize_events() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.handle_event(screen_params_event(
        vec![CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.))],
        vec![Some(SpaceId::new(1))],
        vec![],
    ));

    reactor.handle_events(apps.make_app(1, make_windows(2)));
    let requests = apps.requests();
    assert!(!requests.is_empty());
    let events_1 = apps.simulate_events_for_requests(requests);

    reactor.handle_events(apps.make_app(2, make_windows(2)));
    assert!(!apps.requests().is_empty());

    for event in dbg!(events_1) {
        reactor.handle_event(event);
    }
    let requests = apps.requests();
    assert!(
        requests.is_empty(),
        "got requests when there should have been none: {requests:?}"
    );
}

#[test]
fn it_sends_writes_when_stale_read_state_looks_same_as_written_state() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.handle_event(screen_params_event(
        vec![CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.))],
        vec![Some(SpaceId::new(1))],
        vec![],
    ));

    reactor.handle_events(apps.make_app(1, make_windows(2)));
    let events_1 = apps.simulate_events();
    let state_1 = apps.windows.clone();
    assert!(!state_1.is_empty());

    for event in events_1 {
        reactor.handle_event(event);
    }
    assert!(apps.requests().is_empty());

    reactor.handle_events(apps.make_app(2, make_windows(1)));
    let _events_2 = apps.simulate_events();

    reactor.handle_event(Event::WindowDestroyed(WindowId::new(2, 1)));
    let _events_3 = apps.simulate_events();
    let state_3 = apps.windows;

    // These should be the same, because we should have resized the first
    // two windows both at the beginning, and at the end when the third
    // window was destroyed.
    for (wid, state) in dbg!(state_1) {
        assert!(state_3.contains_key(&wid), "{wid:?} not in {state_3:#?}");
        assert_eq!(state.frame, state_3[&wid].frame);
    }
}

#[test]
fn it_manages_windows_on_enabled_spaces() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(
        vec![full_screen],
        vec![Some(SpaceId::new(1))],
        vec![],
    ));

    reactor.handle_events(apps.make_app(1, make_windows(1)));

    let _events = apps.simulate_events();
    assert_eq!(
        full_screen,
        apps.windows.get(&WindowId::new(1, 1)).expect("Window was not resized").frame,
    );
}

#[test]
fn it_clears_screen_state_when_no_displays_are_reported() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));

    reactor.handle_event(screen_params_event(
        vec![screen],
        vec![Some(SpaceId::new(1))],
        vec![],
    ));
    assert_eq!(1, reactor.space_manager.screens.len());

    reactor.handle_event(screen_params_event(vec![], vec![], vec![]));
    assert!(reactor.space_manager.screens.is_empty());

    reactor.handle_event(Event::SpaceChanged(vec![]));
    assert!(reactor.space_manager.screens.is_empty());

    reactor.handle_event(screen_params_event(
        vec![screen],
        vec![Some(SpaceId::new(1))],
        vec![],
    ));
    assert_eq!(1, reactor.space_manager.screens.len());
}

#[test]
fn duplicate_space_changed_snapshot_is_ignored() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);

    reactor.handle_event(screen_params_event(vec![frame], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    let _ = apps.requests();

    reactor.handle_event(Event::SpaceChanged(vec![Some(space)]));
    let requests = apps.requests();
    assert!(
        requests.is_empty(),
        "duplicate SpaceChanged should not trigger refresh requests: {requests:?}"
    );
}

#[test]
fn it_ignores_windows_on_disabled_spaces() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![full_screen], vec![None], vec![]));

    reactor.handle_events(apps.make_app(1, make_windows(1)));

    let state_before = apps.windows.clone();
    let _events = apps.simulate_events();
    assert_eq!(state_before, apps.windows, "Window should not have been moved",);

    // Make sure it doesn't choke on destroyed events for ignored windows.
    reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 1)));
    reactor.handle_event(Event::WindowCreated(
        WindowId::new(1, 2),
        make_window(2),
        None,
        Some(MouseState::Up),
    ));
    reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 2)));
}

#[test]
fn it_keeps_discovered_windows_on_their_initial_screen() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let screen2 = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(
        vec![screen1, screen2],
        vec![Some(SpaceId::new(1)), Some(SpaceId::new(2))],
        vec![],
    ));

    let mut windows = make_windows(2);
    windows[1].frame.origin = CGPoint::new(1100., 100.);
    reactor.handle_events(apps.make_app(1, windows));

    let _events = apps.simulate_events();
    assert_eq!(
        screen1,
        apps.windows.get(&WindowId::new(1, 1)).expect("Window was not resized").frame,
    );
    assert_eq!(
        screen2,
        apps.windows.get(&WindowId::new(1, 2)).expect("Window was not resized").frame,
    );
}

#[test]
fn it_ignores_windows_on_nonzero_layers() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(
        vec![full_screen],
        vec![Some(SpaceId::new(1))],
        vec![WindowServerInfo {
            id: WindowServerId::new(1),
            pid: 1,
            layer: 10,
            frame: CGRect::ZERO,
            min_frame: CGSize::ZERO,
            max_frame: CGSize::ZERO,
        }],
    ));

    reactor.handle_events(apps.make_app_with_opts(1, make_windows(1), None, true, false));

    let state_before = apps.windows.clone();
    let _events = apps.simulate_events();
    assert_eq!(state_before, apps.windows, "Window should not have been moved",);

    // Make sure it doesn't choke on destroyed events for ignored windows.
    reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 1)));
    reactor.handle_event(Event::WindowCreated(
        WindowId::new(1, 2),
        make_window(2),
        None,
        Some(MouseState::Up),
    ));
    reactor.handle_event(Event::WindowDestroyed(WindowId::new(1, 2)));
}

#[test]
fn native_tab_switch_rekeys_the_active_layout_slot() {
    let (apps, mut reactor, space) = native_tab_test_setup(30);
    let old_wid = WindowId::new(1, 1);
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);
    let txid = reactor.transaction_manager.generate_next_txid(WindowServerId::new(1));
    reactor
        .transaction_manager
        .store_txid(WindowServerId::new(1), txid, replacement.frame);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement.clone(),
        replacement_ws_info,
    ));

    assert_native_tab_switch_state(&mut reactor, space, old_wid, new_wid);
    assert_eq!(
        reactor.transaction_manager.get_last_sent_txid(WindowServerId::new(2)),
        txid.next()
    );
    assert_eq!(
        reactor.transaction_manager.get_target_frame(WindowServerId::new(2)),
        Some(replacement.frame)
    );
    assert_eq!(
        reactor.transaction_manager.get_target_frame(WindowServerId::new(1)),
        None
    );
    let _ = apps;
}

#[test]
fn native_tab_switch_reconciles_when_windowserver_appears_first() {
    let (_apps, mut reactor, space) = native_tab_test_setup(31);
    let old_wid = WindowId::new(1, 1);
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    assert!(!reactor.note_native_tab_appearance(
        WindowServerId::new(2),
        space,
        replacement_ws_info,
    ));
    WindowEventHandler::handle_window_created(
        &mut reactor,
        new_wid,
        replacement,
        Some(replacement_ws_info),
        Some(MouseState::Up),
    );
    reactor.reconcile_native_tabs_for_pid(1, &[old_wid, new_wid]);

    assert_native_tab_switch_state(&mut reactor, space, old_wid, new_wid);
}

#[test]
fn native_tab_window_created_before_destroy_is_held_out_of_layout() {
    let (_apps, mut reactor, space) = native_tab_test_setup(32);
    let old_wid = WindowId::new(1, 1);
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(!reactor.note_native_tab_appearance(
        WindowServerId::new(2),
        space,
        replacement_ws_info,
    ));
    WindowEventHandler::handle_window_created(
        &mut reactor,
        new_wid,
        replacement,
        Some(replacement_ws_info),
        Some(MouseState::Up),
    );

    assert!(!reactor.layout_manager.layout_engine.has_window_membership(new_wid));
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![old_wid]
    );
}

#[test]
fn native_tab_main_window_changed_rekeys_before_windowserver_destroy() {
    let (_apps, mut reactor, space) = native_tab_test_setup(33);
    let old_wid = WindowId::new(1, 1);
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(!reactor.note_native_tab_appearance(
        WindowServerId::new(2),
        space,
        replacement_ws_info,
    ));
    WindowEventHandler::handle_window_created(
        &mut reactor,
        new_wid,
        replacement,
        Some(replacement_ws_info),
        Some(MouseState::Up),
    );
    reactor.handle_event(Event::ApplicationMainWindowChanged(1, Some(new_wid), Quiet::No));

    assert_native_tab_switch_state(&mut reactor, space, old_wid, new_wid);
}

#[test]
fn moving_active_native_tab_updates_suppressed_siblings() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(331);
    let old_wid = WindowId::new(1, 1);
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    let mut moved_frame = reactor.window_manager.windows[&new_wid].frame_monotonic;
    moved_frame.origin.x += 137.0;
    moved_frame.origin.y += 41.0;
    reactor.handle_event(Event::WindowFrameChanged(
        new_wid,
        moved_frame,
        None,
        Requested(false),
        Some(MouseState::Down),
    ));
    reactor.handle_event(Event::MouseUp);

    let final_frame = reactor.window_manager.windows[&new_wid].frame_monotonic;
    assert_eq!(
        reactor.window_manager.windows[&old_wid].frame_monotonic,
        final_frame
    );
    assert_eq!(reactor.window_manager.windows[&old_wid].info.frame, final_frame);

    let requests = apps.requests();
    assert!(requests.iter().any(|request| matches!(
        request,
        Request::SetWindowFrame(wid, frame, _, _) if *wid == old_wid && *frame == final_frame
    )));
    for event in apps.simulate_events_for_requests(requests) {
        reactor.handle_event(event);
    }
    assert_eq!(apps.windows[&old_wid].frame, final_frame);

    let _ = apps.requests();
    reactor.handle_event(Event::ApplicationMainWindowChanged(1, Some(old_wid), Quiet::No));
    let requests = apps.requests();
    assert!(requests.iter().any(|request| matches!(
        request,
        Request::SetWindowFrame(wid, frame, _, _) if *wid == old_wid && *frame == final_frame
    )));
}

#[test]
fn requested_move_of_active_native_tab_updates_suppressed_siblings() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(333);
    let old_wid = WindowId::new(1, 1);
    let original_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    let mut requested_frame = original_frame;
    requested_frame.origin.x += 137.0;
    requested_frame.origin.y += 41.0;
    assert!(!WindowEventHandler::handle_window_frame_changed(
        &mut reactor,
        new_wid,
        requested_frame,
        None,
        Requested(true),
        Some(MouseState::Up),
    ));

    assert_eq!(
        reactor.window_manager.windows[&new_wid].frame_monotonic,
        requested_frame
    );
    assert_eq!(
        reactor.window_manager.windows[&old_wid].frame_monotonic,
        requested_frame
    );
    assert_eq!(
        reactor.window_manager.windows[&old_wid].info.frame,
        requested_frame
    );

    let requests = apps.requests();
    assert_has_set_window_frame_request(&requests, old_wid, requested_frame);
}

#[test]
fn reactivating_native_tab_retries_pending_frame_after_stale_event() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(334);
    let old_wid = WindowId::new(1, 1);
    let original_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    let mut final_frame = original_frame;
    final_frame.origin.x += 137.0;
    final_frame.origin.y += 41.0;
    if let Some(window) = reactor.window_manager.windows.get_mut(&new_wid) {
        window.frame_monotonic = final_frame;
        window.info.frame = final_frame;
    }
    let group_id = reactor.window_manager.windows[&new_wid]
        .native_tab
        .expect("new tab should belong to a native-tab group")
        .group_id;
    reactor.native_tab_manager.groups.get_mut(&group_id).unwrap().canonical_frame = final_frame;

    assert!(reactor.activate_native_tab_replacement(new_wid, old_wid));
    let requests = apps.requests();
    assert_has_set_window_frame_request(&requests, old_wid, final_frame);

    assert!(!WindowEventHandler::handle_window_frame_changed(
        &mut reactor,
        old_wid,
        original_frame,
        None,
        Requested(false),
        Some(MouseState::Up),
    ));

    let retry_requests = apps.requests();
    assert_has_set_window_frame_request(&retry_requests, old_wid, final_frame);
}

#[test]
fn reactivating_native_tab_uses_group_canonical_frame_not_stale_active_window_frame() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(335);
    let old_wid = WindowId::new(1, 1);
    let original_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    let mut final_frame = original_frame;
    final_frame.origin.x += 137.0;
    final_frame.origin.y += 41.0;
    let group_id = reactor.window_manager.windows[&new_wid]
        .native_tab
        .expect("new tab should belong to a native-tab group")
        .group_id;
    reactor.native_tab_manager.groups.get_mut(&group_id).unwrap().canonical_frame = final_frame;
    if let Some(window) = reactor.window_manager.windows.get_mut(&new_wid) {
        window.frame_monotonic = original_frame;
        window.info.frame = original_frame;
    }

    assert!(reactor.activate_native_tab_replacement(new_wid, old_wid));
    assert_eq!(
        reactor.window_manager.windows[&old_wid].frame_monotonic,
        final_frame
    );

    let requests = apps.requests();
    assert_has_set_window_frame_request(&requests, old_wid, final_frame);
}

#[test]
fn window_created_for_suppressed_native_tab_preserves_membership() {
    let (_apps, mut reactor, space) = native_tab_test_setup(337);
    let old_wid = WindowId::new(1, 1);
    let original_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    let mut stale_frame = original_frame;
    stale_frame.origin.x += 504.0;
    let recreated_old = WindowInfo {
        title: "tab-a".to_string(),
        frame: stale_frame,
        sys_id: Some(WindowServerId::new(1)),
        ..make_window(1)
    };
    let recreated_old_ws_info = WindowServerInfo {
        id: WindowServerId::new(1),
        pid: 1,
        layer: 0,
        frame: stale_frame,
        min_frame: CGSize::ZERO,
        max_frame: CGSize::ZERO,
    };

    WindowEventHandler::handle_window_created(
        &mut reactor,
        old_wid,
        recreated_old,
        Some(recreated_old_ws_info),
        Some(MouseState::Up),
    );

    let old_state = reactor.window_manager.windows.get(&old_wid).unwrap();
    assert_eq!(
        old_state
            .native_tab
            .expect("recreated suppressed tab should stay in its native-tab group")
            .role,
        NativeTabRole::Suppressed
    );
    assert_eq!(old_state.frame_monotonic, original_frame);
    assert_eq!(
        reactor.window_manager.windows[&new_wid]
            .native_tab
            .expect("active tab should stay grouped")
            .role,
        NativeTabRole::Active
    );
}

#[test]
fn transient_empty_visibility_during_native_tab_switch_preserves_group_state() {
    let (_apps, mut reactor, space) = native_tab_test_setup(336);
    let old_wid = WindowId::new(1, 1);
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    reactor.handle_event(Event::ApplicationMainWindowChanged(1, None, Quiet::No));
    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![],
    });

    assert!(reactor.window_manager.windows[&old_wid].is_native_tab_suppressed());
    assert_eq!(
        reactor.window_manager.windows[&new_wid]
            .native_tab
            .expect("new tab should remain active")
            .role,
        NativeTabRole::Active
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![new_wid]
    );

    reactor.handle_event(Event::ApplicationMainWindowChanged(1, Some(old_wid), Quiet::No));

    assert_eq!(
        reactor.window_manager.windows[&old_wid]
            .native_tab
            .expect("old tab should still belong to the group")
            .role,
        NativeTabRole::Active
    );
    assert_eq!(
        reactor.window_manager.windows[&new_wid]
            .native_tab
            .expect("new tab should stay grouped")
            .role,
        NativeTabRole::Suppressed
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![old_wid]
    );
}

#[test]
fn reconcile_native_tabs_reactivates_visible_group_member_instead_of_dissolving_group() {
    let (_apps, mut reactor, space) = native_tab_test_setup(338);
    let old_wid = WindowId::new(1, 1);
    let original_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 2);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(2), space));
    if let Some(window) = reactor.window_manager.windows.get_mut(&old_wid) {
        window.info.sys_id = Some(WindowServerId::new(1));
        window.info.frame = original_frame;
    }
    reactor.window_manager.window_ids.insert(WindowServerId::new(1), old_wid);
    reactor
        .native_tab_manager
        .stage_appearance(WindowServerId::new(1), 1, space, original_frame);

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(old_wid, WindowInfo {
            sys_id: Some(WindowServerId::new(1)),
            frame: original_frame,
            ..make_window(1)
        })],
        known_visible: vec![old_wid],
    });

    assert!(reactor.window_manager.windows.contains_key(&new_wid));
    assert_eq!(
        reactor.window_manager.windows[&old_wid]
            .native_tab
            .expect("old tab should remain grouped")
            .role,
        NativeTabRole::Active
    );
    assert_eq!(
        reactor.window_manager.windows[&new_wid]
            .native_tab
            .expect("new tab should remain grouped")
            .role,
        NativeTabRole::Suppressed
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![old_wid]
    );
}

#[test]
fn removing_active_native_tab_member_promotes_a_survivor_role() {
    let (_apps, mut reactor, space) = native_tab_test_setup(339);
    let (_first, _second, third) = create_three_tab_group(&mut reactor, space);

    reactor.finalize_native_tab_window_destroy(third);

    let active_count = [WindowId::new(1, 1), WindowId::new(1, 2)]
        .into_iter()
        .filter_map(|wid| {
            reactor
                .window_manager
                .windows
                .get(&wid)
                .and_then(|window| window.native_tab)
        })
        .filter(|membership| membership.role == NativeTabRole::Active)
        .count();
    assert_eq!(active_count, 1);
}

#[test]
fn closing_active_native_tab_rekeys_to_existing_group_member_without_pending_appearance() {
    let (_apps, mut reactor, space) = native_tab_test_setup(340);
    let (_first, second, third) = create_three_tab_group(&mut reactor, space);

    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(3), space));
    assert!(!WindowEventHandler::handle_window_destroyed(&mut reactor, third));
    assert!(reactor.window_manager.windows.contains_key(&third));

    reactor.reconcile_native_tabs_for_pid(1, &[second]);

    assert!(!reactor.window_manager.windows.contains_key(&third));
    assert_eq!(
        reactor.window_manager.windows[&second]
            .native_tab
            .expect("existing grouped member should become active")
            .role,
        NativeTabRole::Active
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![second]
    );
}

#[test]
fn dragging_tabbed_window_still_detects_swap_targets() {
    let mut apps = Apps::new();
    let mut reactor = test_reactor();
    let space = SpaceId::new(332);
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![screen], vec![Some(space)], vec![]));

    reactor.handle_events(apps.make_app(1, vec![make_window(1), make_window(2)]));
    apps.simulate_until_quiet(&mut reactor);
    let _ = apps.requests();

    let old_wid = WindowId::new(1, 1);
    let sibling_wid = WindowId::new(1, 2);
    let (new_wid, replacement, replacement_ws_info) = replacement_tab(&reactor, old_wid, 3);

    assert!(create_native_tab_replacement(
        &mut reactor,
        space,
        WindowServerId::new(1),
        new_wid,
        replacement,
        replacement_ws_info,
    ));

    let sibling_frame = reactor.window_manager.windows[&sibling_wid].frame_monotonic;
    reactor.handle_event(Event::WindowFrameChanged(
        new_wid,
        sibling_frame,
        None,
        Requested(false),
        Some(MouseState::Down),
    ));

    assert_eq!(reactor.get_pending_drag_swap(), Some((new_wid, sibling_wid)));

    reactor.handle_event(Event::MouseUp);
    assert!(reactor.get_pending_drag_swap().is_none());
}

#[test]
fn native_tab_window_destroy_is_deferred_until_replacement_is_discovered() {
    let (_apps, mut reactor, space) = native_tab_test_setup(34);
    let old_wid = WindowId::new(1, 1);
    let old_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let replacement_ws_info = WindowServerInfo {
        id: WindowServerId::new(2),
        pid: 1,
        layer: 0,
        frame: old_frame,
        min_frame: CGSize::ZERO,
        max_frame: CGSize::ZERO,
    };

    assert!(!reactor.note_native_tab_appearance(
        WindowServerId::new(2),
        space,
        replacement_ws_info,
    ));
    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    assert!(!WindowEventHandler::handle_window_destroyed(
        &mut reactor,
        old_wid
    ));
    assert!(reactor.window_manager.windows.contains_key(&old_wid));
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![old_wid]
    );

    let (new_wid, replacement, _) = replacement_tab(&reactor, old_wid, 2);
    WindowEventHandler::handle_window_created(
        &mut reactor,
        new_wid,
        replacement,
        Some(replacement_ws_info),
        Some(MouseState::Up),
    );
    reactor.reconcile_native_tabs_for_pid(1, &[]);

    assert_native_tab_switch_state(&mut reactor, space, old_wid, new_wid);
}

#[test]
fn deferred_native_tab_destroy_finalizes_when_refresh_reports_no_visible_windows() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(35);

    let wid = WindowId::new(1, 1);
    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    apps.windows.remove(&wid);
    assert!(!WindowEventHandler::handle_window_destroyed(&mut reactor, wid));
    reactor
        .app_manager
        .apps
        .get(&1)
        .unwrap()
        .handle
        .send(Request::WindowMaybeDestroyed(wid))
        .unwrap();

    for event in apps.simulate_events() {
        reactor.handle_event(event);
    }

    assert_window_removed_from_layout(&reactor, space, wid);
}

#[test]
fn transient_empty_visibility_grace_is_one_shot_for_real_native_tab_close() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(350);

    let wid = WindowId::new(1, 1);
    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    apps.windows.remove(&wid);
    assert!(!WindowEventHandler::handle_window_destroyed(&mut reactor, wid));

    reactor.handle_event(Event::ApplicationMainWindowChanged(1, None, Quiet::No));
    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![],
    });
    assert!(
        reactor.window_manager.windows.contains_key(&wid),
        "first empty refresh should be treated as a transient native-tab handoff"
    );

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![],
    });
    assert_window_removed_from_layout(&reactor, space, wid);
}

#[test]
fn transient_empty_visibility_requests_follow_up_refresh_to_finalize_last_tab_close() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(351);

    let wid = WindowId::new(1, 1);
    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    apps.windows.remove(&wid);
    assert!(!WindowEventHandler::handle_window_destroyed(&mut reactor, wid));

    reactor.handle_event(Event::ApplicationMainWindowChanged(1, None, Quiet::No));
    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![],
    });
    assert!(reactor.window_manager.windows.contains_key(&wid));

    apps.simulate_until_quiet(&mut reactor);
    assert_window_removed_from_layout(&reactor, space, wid);
}

#[test]
fn pending_native_tab_appearance_does_not_defer_regular_window_close() {
    let (_apps, mut reactor, space) = native_tab_test_setup(36);

    let wid = WindowId::new(1, 1);
    let frame = reactor.window_manager.windows[&wid].frame_monotonic;
    reactor
        .native_tab_manager
        .stage_appearance(WindowServerId::new(2), 1, space, frame);

    assert!(WindowEventHandler::handle_window_destroyed(&mut reactor, wid));
    assert_window_removed_from_layout(&reactor, space, wid);
}

#[test]
fn main_window_changed_same_frame_visible_sibling_without_pending_appearance_stays_standalone() {
    let (_apps, mut reactor, space) = native_tab_window_and_space_setup(37, 2);

    let old_wid = WindowId::new(1, 1);
    let new_wid = WindowId::new(1, 2);
    let old_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    if let Some(window) = reactor.window_manager.windows.get_mut(&new_wid) {
        window.frame_monotonic = old_frame;
    }

    reactor.handle_event(Event::ApplicationMainWindowChanged(1, Some(new_wid), Quiet::No));

    assert!(reactor.window_manager.windows[&old_wid].native_tab.is_none());
    assert!(reactor.window_manager.windows[&new_wid].native_tab.is_none());
    let mut windows = reactor.layout_manager.layout_engine.windows_in_active_workspace(space);
    windows.sort_unstable();
    assert_eq!(windows, vec![old_wid, new_wid]);
}

#[test]
fn pending_destroy_does_not_rekey_to_existing_same_frame_sibling_without_appearance() {
    let (_apps, mut reactor, space) = native_tab_window_and_space_setup(38, 2);

    let old_wid = WindowId::new(1, 1);
    let sibling_wid = WindowId::new(1, 2);
    let old_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    if let Some(window) = reactor.window_manager.windows.get_mut(&sibling_wid) {
        window.frame_monotonic = old_frame;
    }

    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    assert!(!WindowEventHandler::handle_window_destroyed(
        &mut reactor,
        old_wid
    ));
    reactor.reconcile_native_tabs_for_pid(1, &[sibling_wid]);

    assert!(!reactor.window_manager.windows.contains_key(&old_wid));
    assert!(reactor.window_manager.windows[&sibling_wid].native_tab.is_none());
    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![sibling_wid]
    );
}

#[test]
fn native_tab_discovery_before_destroy_does_not_append_new_tab_to_layout() {
    let (_apps, mut reactor, space) = native_tab_test_setup(39);

    let old_wid = WindowId::new(1, 1);
    let old_frame = reactor.window_manager.windows[&old_wid].frame_monotonic;
    let new_wid = WindowId::new(1, 2);
    let mut replacement = make_window(2);
    replacement.frame = old_frame;
    replacement.sys_id = Some(WindowServerId::new(2));

    assert!(
        !reactor.note_native_tab_appearance(WindowServerId::new(2), space, WindowServerInfo {
            id: WindowServerId::new(2),
            pid: 1,
            layer: 0,
            frame: old_frame,
            min_frame: CGSize::ZERO,
            max_frame: CGSize::ZERO,
        })
    );

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(new_wid, replacement)],
        known_visible: vec![new_wid],
    });

    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![old_wid]
    );

    reactor.handle_event(Event::ApplicationMainWindowChanged(1, Some(new_wid), Quiet::No));
    assert_native_tab_switch_state(&mut reactor, space, old_wid, new_wid);
}

#[test]
fn unmatched_window_server_destroy_still_removes_closed_window() {
    let (_apps, mut reactor, space) = native_tab_test_setup(31);

    let wid = WindowId::new(1, 1);
    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    reactor.reconcile_native_tabs_for_pid(1, &[]);

    assert_window_removed_from_layout(&reactor, space, wid);
}

#[test]
fn system_wake_finalizes_deferred_native_tab_destroy_after_close() {
    let (mut apps, mut reactor, space) = native_tab_test_setup(40);

    let wid = WindowId::new(1, 1);
    assert!(reactor.stage_native_tab_destroy(WindowServerId::new(1), space));
    apps.windows.remove(&wid);
    assert!(!WindowEventHandler::handle_window_destroyed(&mut reactor, wid));

    reactor.handle_event(Event::SystemWoke);
    apps.simulate_until_quiet(&mut reactor);

    assert_window_removed_from_layout(&reactor, space, wid);
}

#[test]
fn pending_refresh_empty_discovery_is_one_shot_and_allows_later_stale_cleanup() {
    let (_apps, mut reactor, space) = native_tab_test_setup(41);

    let wid = WindowId::new(1, 1);
    assert!(reactor.window_manager.windows.contains_key(&wid));
    reactor.active_spaces.clear();
    reactor.window_manager.visible_windows.clear();
    reactor.window_manager.window_ids.remove(&WindowServerId::new(1));

    reactor.mission_control_manager.pending_mission_control_refresh.insert(1);

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![],
    });

    assert!(!reactor.mission_control_manager.pending_mission_control_refresh.contains(&1));
    assert!(reactor.window_manager.windows.contains_key(&wid));

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![],
    });

    assert_window_removed_from_layout(&reactor, space, wid);
}

#[test]
fn handle_layout_response_groups_windows_by_app_and_screen() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let (raise_manager_tx, mut raise_manager_rx) = actor::channel();
    reactor.communication_manager.raise_manager_tx = raise_manager_tx;
    let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let screen2 = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(
        vec![screen1, screen2],
        vec![Some(SpaceId::new(1)), Some(SpaceId::new(2))],
        vec![],
    ));

    reactor.handle_events(apps.make_app(1, make_windows(2)));

    let mut windows = make_windows(2);
    windows[1].frame.origin = CGPoint::new(1100., 100.);
    reactor.handle_events(apps.make_app(2, windows));

    let _events = apps.simulate_events();
    while raise_manager_rx.try_recv().is_ok() {}

    reactor.handle_layout_response(
        layout::EventResponse {
            raise_windows: vec![
                WindowId::new(1, 1),
                WindowId::new(1, 2),
                WindowId::new(2, 1),
                WindowId::new(2, 2),
            ],
            focus_window: None,
            boundary_hit: None,
        },
        None,
    );
    let msg = raise_manager_rx.try_recv().expect("Should have sent an event").1;
    match msg {
        raise_manager::Event::RaiseRequest(RaiseRequest {
            raise_windows, focus_window, ..
        }) => {
            let raise_windows: HashSet<Vec<WindowId>> = raise_windows.into_iter().collect();
            let expected = [
                vec![WindowId::new(1, 1), WindowId::new(1, 2)],
                vec![WindowId::new(2, 1)],
                vec![WindowId::new(2, 2)],
            ]
            .into_iter()
            .collect();
            assert_eq!(raise_windows, expected);
            assert!(focus_window.is_none());
        }
        _ => panic!("Unexpected event: {msg:?}"),
    }
}

#[test]
fn handle_layout_response_includes_handles_for_raise_and_focus_windows() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let (raise_manager_tx, mut raise_manager_rx) = actor::channel();
    reactor.communication_manager.raise_manager_tx = raise_manager_tx;

    reactor.handle_events(apps.make_app(1, make_windows(1)));
    reactor.handle_events(apps.make_app(2, make_windows(1)));

    let _events = apps.simulate_events();
    while raise_manager_rx.try_recv().is_ok() {}
    reactor.handle_layout_response(
        layout::EventResponse {
            raise_windows: vec![WindowId::new(1, 1)],
            focus_window: Some(WindowId::new(2, 1)),
            boundary_hit: None,
        },
        None,
    );
    let msg = raise_manager_rx.try_recv().expect("Should have sent an event").1;
    match msg {
        raise_manager::Event::RaiseRequest(RaiseRequest { app_handles, .. }) => {
            assert!(app_handles.contains_key(&1));
            assert!(app_handles.contains_key(&2));
        }
        _ => panic!("Unexpected event: {msg:?}"),
    }
}

#[test]
fn it_preserves_layout_after_login_screen() {
    // TODO: This would be better tested with a more complete simulation.
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let space = SpaceId::new(1);
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![full_screen], vec![Some(space)], vec![]));

    reactor.handle_events(apps.make_app_with_opts(
        1,
        make_windows(3),
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);
    let default = reactor.layout_manager.layout_engine.calculate_layout(
        space,
        full_screen,
        &reactor.config.settings.layout.gaps,
        0.0,
        crate::common::config::HorizontalPlacement::Top,
        crate::common::config::VerticalPlacement::Right,
    );

    assert!(reactor.layout_manager.layout_engine.selected_window(space).is_some());
    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::MoveNode(
        Direction::Up,
    ))));
    apps.simulate_until_quiet(&mut reactor);
    let modified = reactor.layout_manager.layout_engine.calculate_layout(
        space,
        full_screen,
        &reactor.config.settings.layout.gaps,
        0.0,
        crate::common::config::HorizontalPlacement::Top,
        crate::common::config::VerticalPlacement::Right,
    );
    assert_ne!(default, modified);

    reactor.handle_event(screen_params_event(vec![CGRect::ZERO], vec![None], vec![]));
    reactor.handle_event(screen_params_event(
        vec![full_screen],
        vec![Some(space)],
        (1..=3)
            .map(|n| WindowServerInfo {
                pid: 1,
                id: WindowServerId::new(n),
                layer: 0,
                frame: CGRect::ZERO,
                min_frame: CGSize::ZERO,
                max_frame: CGSize::ZERO,
            })
            .collect(),
    ));
    let requests = apps.requests();
    for request in requests {
        match request {
            Request::GetVisibleWindows => {
                // Simulate the login screen condition: No windows are
                // considered visible by the accessibility API, but they are
                // from the window server API in the event above.
                reactor.handle_event(Event::WindowsDiscovered {
                    pid: 1,
                    new: vec![],
                    known_visible: vec![],
                });
            }
            req => {
                let events = apps.simulate_events_for_requests(vec![req]);
                for event in events {
                    reactor.handle_event(event);
                }
            }
        }
    }
    apps.simulate_until_quiet(&mut reactor);

    assert_eq!(
        reactor.layout_manager.layout_engine.calculate_layout(
            space,
            full_screen,
            &reactor.config.settings.layout.gaps,
            0.0,
            crate::common::config::HorizontalPlacement::Top,
            crate::common::config::VerticalPlacement::Right,
        ),
        modified
    );
}

#[test]
fn title_change_reapply_does_not_rebalance_unchanged_layout() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.config.virtual_workspaces.reapply_app_rules_on_title_change = true;

    let space = SpaceId::new(1);
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![full_screen], vec![Some(space)], vec![]));

    reactor.handle_events(apps.make_app_with_opts(
        1,
        make_windows(3),
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    assert!(reactor.layout_manager.layout_engine.selected_window(space).is_some());
    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::MoveNode(
        Direction::Up,
    ))));
    apps.simulate_until_quiet(&mut reactor);

    let modified = reactor.layout_manager.layout_engine.calculate_layout(
        space,
        full_screen,
        &reactor.config.settings.layout.gaps,
        0.0,
        crate::common::config::HorizontalPlacement::Top,
        crate::common::config::VerticalPlacement::Right,
    );

    reactor.handle_event(Event::WindowTitleChanged(
        WindowId::new(1, 1),
        "Renamed window".to_string(),
    ));

    assert_eq!(
        reactor.layout_manager.layout_engine.calculate_layout(
            space,
            full_screen,
            &reactor.config.settings.layout.gaps,
            0.0,
            crate::common::config::HorizontalPlacement::Top,
            crate::common::config::VerticalPlacement::Right,
        ),
        modified
    );
}

#[test]
fn title_change_reapply_does_not_rebalance_when_window_stays_floating() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.config.virtual_workspaces.reapply_app_rules_on_title_change = true;

    let space = SpaceId::new(1);
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![full_screen], vec![Some(space)], vec![]));

    reactor.handle_events(apps.make_app_with_opts(
        1,
        make_windows(3),
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    assert!(reactor.layout_manager.layout_engine.selected_window(space).is_some());
    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::MoveNode(
        Direction::Up,
    ))));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::ToggleWindowFloating,
    )));
    apps.simulate_until_quiet(&mut reactor);
    assert!(reactor.layout_manager.layout_engine.is_window_floating(WindowId::new(1, 1)));

    let modified = reactor.layout_manager.layout_engine.calculate_layout(
        space,
        full_screen,
        &reactor.config.settings.layout.gaps,
        0.0,
        crate::common::config::HorizontalPlacement::Top,
        crate::common::config::VerticalPlacement::Right,
    );

    reactor.handle_event(Event::WindowTitleChanged(
        WindowId::new(1, 1),
        "Renamed floating window".to_string(),
    ));

    assert!(reactor.layout_manager.layout_engine.is_window_floating(WindowId::new(1, 1)));
    assert_eq!(
        reactor.layout_manager.layout_engine.calculate_layout(
            space,
            full_screen,
            &reactor.config.settings.layout.gaps,
            0.0,
            crate::common::config::HorizontalPlacement::Top,
            crate::common::config::VerticalPlacement::Right,
        ),
        modified
    );
}

#[test]
fn menu_open_state_is_cleared_when_owner_deactivates() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let (event_tap_tx, mut event_tap_rx) = actor::channel();
    reactor.communication_manager.event_tap_tx = Some(event_tap_tx);

    reactor.handle_event(Event::MenuOpened(1));
    let disable = event_tap_rx.try_recv().expect("menu-open should update event tap").1;
    assert!(matches!(
        disable,
        crate::actor::event_tap::Request::SetFocusFollowsMouseEnabled(false)
    ));
    assert_eq!(reactor.menu_manager.menu_state, MenuState::Open(1));

    reactor.handle_event(Event::ApplicationDeactivated(1));
    let enable = event_tap_rx
        .try_recv()
        .expect("app deactivation should re-enable focus-follows-mouse")
        .1;
    assert!(matches!(
        enable,
        crate::actor::event_tap::Request::SetFocusFollowsMouseEnabled(true)
    ));
    assert_eq!(reactor.menu_manager.menu_state, MenuState::Closed);
}

#[test]
fn stale_menu_open_state_is_cleared_when_other_app_activates() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let (event_tap_tx, mut event_tap_rx) = actor::channel();
    reactor.communication_manager.event_tap_tx = Some(event_tap_tx);

    reactor.handle_event(Event::MenuOpened(1));
    let _ = event_tap_rx.try_recv().expect("menu-open should update event tap");
    assert_eq!(reactor.menu_manager.menu_state, MenuState::Open(1));

    reactor.handle_event(Event::ApplicationGloballyActivated(2));
    let enable = event_tap_rx
        .try_recv()
        .expect("activation of another app should clear stale menu state")
        .1;
    assert!(matches!(
        enable,
        crate::actor::event_tap::Request::SetFocusFollowsMouseEnabled(true)
    ));
    assert_eq!(reactor.menu_manager.menu_state, MenuState::Closed);
}

#[test]
fn it_retains_windows_without_server_ids_after_login_visibility_failure() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let space = SpaceId::new(1);
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(vec![full_screen], vec![Some(space)], vec![]));

    let window = WindowInfo {
        is_standard: true,
        is_root: true,
        is_minimized: false,
        is_resizable: true,
        min_size: None,
        max_size: None,
        title: "NoServerId".to_string(),
        frame: CGRect::new(CGPoint::new(50., 50.), CGSize::new(400., 400.)),
        sys_id: None,
        bundle_id: None,
        path: None,
        ax_role: None,
        ax_subrole: None,
    };

    reactor.handle_events(apps.make_app_with_opts(
        1,
        vec![window],
        Some(WindowId::new(1, 1)),
        true,
        false,
    ));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::SpaceChanged(vec![None]));

    // Simulate a native fullscreen transition: space temporarily becomes a fullscreen
    // space id (reactor suppresses it to None), then returns to the original space.
    let fullscreen_space = SpaceId::new(0x400000000 + space.get());
    reactor.handle_event(Event::SpaceChanged(vec![Some(fullscreen_space)]));

    reactor.handle_event(Event::SpaceChanged(vec![Some(space)]));

    loop {
        let requests = apps.requests();
        if requests.is_empty() {
            break;
        }

        let mut other_requests = Vec::new();
        for request in requests {
            match request {
                Request::GetVisibleWindows => {
                    reactor.handle_event(Event::WindowsDiscovered {
                        pid: 1,
                        new: vec![],
                        known_visible: vec![],
                    });
                }
                other => other_requests.push(other),
            }
        }

        if !other_requests.is_empty() {
            let events = apps.simulate_events_for_requests(other_requests);
            for event in events {
                reactor.handle_event(event);
            }
        }
    }
}

#[test]
fn display_index_selector_uses_physical_left_to_right_order() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let right = CGRect::new(CGPoint::new(200000., 0.), CGSize::new(1000., 1000.));
    let left = CGRect::new(CGPoint::new(100000., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(screen_params_event(
        vec![right, left],
        vec![Some(SpaceId::new(1)), Some(SpaceId::new(2))],
        vec![],
    ));

    let selected = reactor
        .screen_for_selector(&DisplaySelector::Index(0), None)
        .expect("expected display index 0 to resolve");

    assert_eq!(selected.frame, left);
}

#[test]
fn display_churn_quarantine_counters_increment() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.display_topology_manager.quarantine_appeared();
    reactor.display_topology_manager.quarantine_destroyed();
    reactor.display_topology_manager.quarantine_resync();

    let stats = reactor.display_topology_manager.quarantine_stats.clone();
    assert_eq!(stats.appeared_dropped, 1);
    assert_eq!(stats.destroyed_dropped, 1);
    assert_eq!(stats.resync_dropped, 1);
}

#[test]
fn display_churn_transitions_to_awaiting_commit_then_stable() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    reactor.handle_event(screen_params_event(vec![frame], vec![Some(space)], vec![]));

    reactor.display_topology_manager.begin_churn(
        2,
        crate::sys::skylight::DisplayReconfigFlags::ADD,
        crate::common::collections::HashSet::default(),
    );
    reactor
        .display_topology_manager
        .end_churn_to_awaiting(2, crate::sys::skylight::DisplayReconfigFlags::ADD);

    assert!(matches!(
        reactor.display_topology_manager.state(),
        TopologyState::AwaitingCommitSnapshot { .. }
    ));

    reactor.handle_event(screen_params_event(vec![frame], vec![Some(space)], vec![]));

    assert!(matches!(
        reactor.display_topology_manager.state(),
        TopologyState::Stable
    ));
}

#[test]
fn display_churn_quarantines_window_frame_changed_events() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.display_topology_manager.begin_churn(
        3,
        crate::sys::skylight::DisplayReconfigFlags::ADD,
        crate::common::collections::HashSet::default(),
    );

    let quarantined = reactor.maybe_quarantine_during_churn(&Event::WindowFrameChanged(
        WindowId::new(99, 1),
        CGRect::new(CGPoint::new(10., 10.), CGSize::new(500., 400.)),
        None,
        Requested(false),
        Some(MouseState::Up),
    ));
    assert!(
        quarantined,
        "WindowFrameChanged should be quarantined during churn"
    );
}

#[test]
fn topology_relayout_pending_when_space_ids_change_for_same_displays() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1280., 800.));
    let right = CGRect::new(CGPoint::new(1280., 0.), CGSize::new(1280., 800.));

    reactor.handle_event(screen_params_event(
        vec![left, right],
        vec![Some(SpaceId::new(11)), Some(SpaceId::new(22))],
        vec![],
    ));
    assert!(!reactor.pending_space_change_manager.topology_relayout_pending);

    reactor.handle_event(screen_params_event(
        vec![left, right],
        vec![Some(SpaceId::new(111)), Some(SpaceId::new(222))],
        vec![],
    ));
    assert!(
        reactor.pending_space_change_manager.topology_relayout_pending,
        "Space-id churn on unchanged displays should trigger topology relayout"
    );
}
