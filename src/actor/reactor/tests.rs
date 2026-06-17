use objc2_core_foundation::{CGPoint, CGSize};
use test_log::test;

use super::testing::*;
use super::*;
use crate::actor::reactor::events::window_discovery::WindowDiscoveryHandler;
use crate::actor::app::{AppThreadHandle, Request, pid_t};
use crate::layout_engine::{Direction, LayoutCommand, LayoutEngine, LayoutEvent};
use crate::sys::app::{AppInfo, WindowInfo};
use crate::sys::window_server::WindowServerId;

#[test]
fn it_ignores_stale_resize_events() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.handle_event(space_state_event(
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
    reactor.handle_event(space_state_event(
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
    reactor.handle_event(space_state_event(
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

    reactor.handle_event(space_state_event(
        vec![screen],
        vec![Some(SpaceId::new(1))],
        vec![],
    ));
    assert_eq!(1, reactor.space_state.screens.len());

    reactor.handle_event(space_state_event(vec![], vec![], vec![]));
    assert!(reactor.space_state.screens.is_empty());

    reactor.handle_event(space_state_event(vec![], vec![], vec![]));
    assert!(reactor.space_state.screens.is_empty());

    reactor.handle_event(space_state_event(
        vec![screen],
        vec![Some(SpaceId::new(1))],
        vec![],
    ));
    assert_eq!(1, reactor.space_state.screens.len());
}

#[test]
fn workspace_command_space_follows_forwarded_space_snapshot() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let old_space = SpaceId::new(1);
    let new_space = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(old_space)], vec![]));
    reactor.handle_events(apps.make_app_with_opts(
        1,
        make_windows(1),
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    assert_eq!(reactor.workspace_command_space(), Some(old_space));

    reactor.handle_event(space_state_event(vec![screen], vec![Some(new_space)], vec![]));

    assert_eq!(
        reactor.workspace_command_space(),
        Some(new_space),
        "workspace commands must follow the forwarded active screen space, not stale main-window space",
    );
}

#[test]
fn forwarded_active_spaces_are_authoritative_for_workspace_context() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let old_space = SpaceId::new(1);
    let new_space = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(old_space)], vec![]));
    assert_eq!(reactor.workspace_command_space(), Some(old_space));

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen], vec![Some(new_space)]),
        fullscreen_by_space: Default::default(),
        has_seen_display_set: false,
        active_spaces: [new_space].into_iter().collect(),
        command_space: Some(new_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        resized_spaces: Vec::new(),
    }));

    assert_eq!(reactor.workspace_command_space(), Some(new_space));
}

#[test]
fn forwarded_space_state_updates_fullscreen_tracks() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

    let mut fullscreen_by_space = HashMap::default();
    fullscreen_by_space.insert(fullscreen_space.get(), FullscreenSpaceTrack::default());

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen], vec![Some(user_space)]),
        fullscreen_by_space: fullscreen_by_space.clone(),
        has_seen_display_set: false,
        active_spaces: [user_space].into_iter().collect(),
        command_space: Some(user_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        resized_spaces: Vec::new(),
    }));

    let track = reactor
        .space_state
        .fullscreen_by_space
        .get(&fullscreen_space.get())
        .expect("forwarded fullscreen track should be retained");
    assert!(track.windows.is_empty());
}

#[test]
fn queries_prefer_authoritative_active_space_over_stale_command_space() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space1)], vec![]));
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_virtual_workspace_command(space1, &LayoutCommand::SwitchToWorkspace(0));
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_virtual_workspace_command(space2, &LayoutCommand::SwitchToWorkspace(1));

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen], vec![Some(space2)]),
        fullscreen_by_space: Default::default(),
        has_seen_display_set: false,
        active_spaces: [space2].into_iter().collect(),
        command_space: Some(space1),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        resized_spaces: Vec::new(),
    }));

    assert_eq!(
        reactor.query_active_workspace(None),
        reactor.layout_manager.layout_engine.active_workspace(space2),
        "default queries must follow authoritative active space state, not stale command_space"
    );
}

#[test]
fn workspace_queries_are_isolated_per_macos_space() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(space1), Some(space2)],
        vec![],
    ));

    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_virtual_workspace_command(space1, &LayoutCommand::SwitchToWorkspace(0));
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_virtual_workspace_command(space2, &LayoutCommand::SwitchToWorkspace(1));

    let space1_workspaces = reactor.query_workspaces(Some(space1));
    let space2_workspaces = reactor.query_workspaces(Some(space2));

    assert_eq!(space1_workspaces.iter().filter(|ws| ws.is_active).count(), 1);
    assert_eq!(space2_workspaces.iter().filter(|ws| ws.is_active).count(), 1);
    assert_ne!(
        space1_workspaces.iter().position(|ws| ws.is_active),
        space2_workspaces.iter().position(|ws| ws.is_active),
        "each macOS space must retain its own active virtual workspace state",
    );

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![left], vec![Some(space2)]),
        fullscreen_by_space: Default::default(),
        has_seen_display_set: false,
        active_spaces: [space2].into_iter().collect(),
        command_space: Some(space2),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        resized_spaces: Vec::new(),
    }));

    let default_workspaces = reactor.query_workspaces(None);
    assert_eq!(
        default_workspaces.iter().position(|ws| ws.is_active),
        space2_workspaces.iter().position(|ws| ws.is_active),
        "default workspace queries must reflect the currently active macOS space",
    );
}

#[test]
fn best_space_prefers_authoritative_window_server_space_over_geometry() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);
    let wid = WindowId::new(1, 1);
    let wsid = WindowServerId::new(11);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space2)], vec![]));
    reactor.window_manager.track_window_server_id(wsid, wid);
    reactor.window_manager.set_window_server_space(wsid, Some(space1));
    reactor.window_manager.insert_window(wid, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Window".to_string(),
            frame,
            sys_id: Some(wsid),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: true,
        ignore_app_rule: false,
    });

    assert_eq!(reactor.best_space_for_window_id(wid), Some(space1));
}

#[test]
fn user_space_window_server_events_preserve_hidden_window_state() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let wid = WindowId::new(1, 1);
    let wsid = WindowServerId::new(21);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space1)], vec![]));
    reactor.window_manager.track_window_server_id(wsid, wid);
    reactor.window_manager.set_window_server_space(wsid, Some(space1));
    reactor.window_manager.insert_window(wid, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Window".to_string(),
            frame,
            sys_id: Some(wsid),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: true,
        ignore_app_rule: false,
    });

    SpaceEventHandler::handle_window_server_destroyed(
        &mut reactor,
        wsid,
        space1,
        SpaceEventKind::User,
    );

    assert!(reactor.window_manager.contains_window(wid));
    assert_eq!(reactor.window_manager.window_server_space(wsid), Some(space1));
    assert!(!reactor.window_manager.is_window_visible(wsid));
}

#[test]
fn known_window_server_appearance_updates_authoritative_space() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);
    let wid = WindowId::new(1, 1);
    let wsid = WindowServerId::new(31);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space1)], vec![]));
    reactor.window_manager.track_window_server_id(wsid, wid);
    reactor.window_manager.track_window_server_info(crate::sys::window_server::WindowServerInfo {
        id: wsid,
        pid: wid.pid,
        layer: 0,
        frame,
        min_frame: frame.size,
        max_frame: frame.size,
    });
    reactor.window_manager.set_window_server_space(wsid, Some(space1));

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        space2,
        SpaceEventKind::User,
    );

    assert_eq!(reactor.window_manager.window_server_space(wsid), Some(space2));
}

#[test]
fn discovery_preserves_hidden_windows_on_their_original_same_display_space() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let pid = 1;
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space1)], vec![]));
    let (app_tx, _app_rx) = crate::actor::channel();
    reactor.app_manager.apps.insert(pid, AppState {
        info: AppInfo {
            bundle_id: Some("com.test.app".to_string()),
            localized_name: Some("Test App".to_string()),
        },
        handle: AppThreadHandle::new_for_test(app_tx),
    });

    let space1_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space1)
        .first()
        .map(|(id, _)| *id)
        .expect("space1 workspace");
    let space2_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space2)
        .first()
        .map(|(id, _)| *id)
        .expect("space2 workspace");

    let windows = [
        (WindowId::new(pid, 1), WindowServerId::new(101), space1),
        (WindowId::new(pid, 2), WindowServerId::new(102), space1),
        (WindowId::new(pid, 3), WindowServerId::new(103), space2),
    ];

    for (wid, wsid, space) in windows {
        reactor.window_manager.track_window_server_id(wsid, wid);
        reactor.window_manager.set_window_server_space(wsid, Some(space));
        reactor.window_manager.insert_window(wid, WindowState {
            info: WindowInfo {
                is_standard: true,
                is_root: true,
                is_minimized: false,
                is_resizable: true,
                min_size: None,
                max_size: None,
                title: format!("Window {}", wid.idx),
                frame,
                sys_id: Some(wsid),
                bundle_id: None,
                path: None,
                ax_role: None,
                ax_subrole: None,
            },
            frame_monotonic: frame,
            is_manageable: true,
            ignore_app_rule: false,
        });
    }

    assert!(reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .assign_window_to_workspace(space1, WindowId::new(pid, 1), space1_workspace));
    assert!(reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .assign_window_to_workspace(space1, WindowId::new(pid, 2), space1_workspace));
    assert!(reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .assign_window_to_workspace(space2, WindowId::new(pid, 3), space2_workspace));

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space2)], vec![]));
    reactor.window_manager.clear_visible_windows();
    reactor.window_manager.mark_window_visible(WindowServerId::new(103));
    reactor
        .mission_control_manager
        .pending_mission_control_refresh
        .insert(pid);

    WindowDiscoveryHandler::handle_discovery(
        &mut reactor,
        pid,
        vec![],
        vec![WindowId::new(pid, 1), WindowId::new(pid, 2), WindowId::new(pid, 3)],
        None,
    );

    let space1_workspaces = reactor.query_workspaces(Some(space1));
    let space2_workspaces = reactor.query_workspaces(Some(space2));
    let space1_count: usize = space1_workspaces.iter().map(|ws| ws.window_count).sum();
    let space2_count: usize = space2_workspaces.iter().map(|ws| ws.window_count).sum();

    assert_eq!(space1_count, 2, "inactive native space windows must stay on space1");
    assert_eq!(space2_count, 1, "only the visible window should belong to space2");
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(space1, WindowId::new(pid, 1))
            .is_some()
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(space1, WindowId::new(pid, 2))
            .is_some()
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(space2, WindowId::new(pid, 1))
            .is_none()
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(space2, WindowId::new(pid, 2))
            .is_none()
    );
}

#[test]
fn forwarded_space_state_is_queued_during_mission_control_and_applied_on_exit() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let old_space = SpaceId::new(1);
    let new_space = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(old_space)], vec![]));
    reactor.handle_event(Event::MissionControlNativeEntered);
    reactor.handle_event(space_state_event(vec![screen], vec![Some(new_space)], vec![]));

    assert_eq!(
        reactor
            .pending_space_change_manager
            .pending_space_change
            .as_ref()
            .map(|pending| pending.screens.iter().map(|screen| screen.space).collect::<Vec<_>>()),
        Some(vec![Some(new_space)])
    );

    reactor.handle_event(Event::MissionControlNativeExited);

    assert_eq!(reactor.workspace_command_space(), Some(new_space));
    assert!(reactor.pending_space_change_manager.pending_space_change.is_none());
}

#[test]
fn mission_control_exit_refresh_drops_windows_missing_from_origin_space_snapshot() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let pid: pid_t = 42;
    let moved = WindowId::new(pid, 1);
    let retained = WindowId::new(pid, 2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(pid, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, space, screen, moved));
    assert!(has_window_in_layout(&mut reactor, space, screen, retained));

    apps.windows.remove(&moved);
    let retained_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 2);
    let retained_frame = apps
        .windows
        .get(&retained)
        .expect("retained window should still exist")
        .frame;

    reactor.refresh_windows_after_mission_control_with_ws_info(vec![WindowServerInfo {
        pid,
        id: retained_wsid,
        layer: 0,
        frame: retained_frame,
        min_frame: CGSize::ZERO,
        max_frame: CGSize::ZERO,
    }]);
    apps.simulate_until_quiet(&mut reactor);

    assert!(
        !has_window_in_layout(&mut reactor, space, screen, moved),
        "window moved to another native space during Mission Control should be removed from the origin layout immediately"
    );
    assert!(has_window_in_layout(&mut reactor, space, screen, retained));
}

#[test]
fn mission_control_enter_clears_active_drag_state() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);
    let frame = CGRect::new(CGPoint::new(50., 50.), CGSize::new(100., 100.));

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));
    reactor.window_manager.insert_window(wid, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Window".to_string(),
            frame,
            sys_id: Some(WindowServerId::new(1)),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: true,
        ignore_app_rule: false,
    });
    reactor.ensure_active_drag(wid, &frame);

    assert!(matches!(reactor.drag_manager.drag_state, DragState::Active { .. }));

    reactor.handle_event(Event::MissionControlNativeEntered);

    assert!(matches!(reactor.drag_manager.drag_state, DragState::Inactive));
    assert!(reactor.drag_manager.skip_layout_for_window.is_none());
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![None], vec![]));

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
    reactor.handle_event(space_state_event(
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
    reactor.handle_event(space_state_event(
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
    reactor.handle_event(space_state_event(
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
fn workspace_switch_batches_all_windows_with_eui_enabled() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(1, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);
    let _ = apps.requests();

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::MoveWindowToWorkspace {
            workspace: 1,
            window_id: Some(2),
        },
    )));
    apps.simulate_until_quiet(&mut reactor);
    let _ = apps.requests();

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::SwitchToWorkspace(1),
    )));

    let requests = apps.requests();
    assert!(
        requests.iter().any(|req| {
            matches!(
                req,
                Request::SetBatchWindowFrame(frames, _, true)
                    if frames.iter().any(|(wid, _)| *wid == WindowId::new(1, 1))
                        && frames.iter().any(|(wid, _)| *wid == WindowId::new(1, 2))
            )
        }),
        "expected workspace-switch batch to disable eui for both hidden and visible windows: {requests:?}"
    );
}

#[test]
fn auto_workspace_switch_focuses_activated_window_not_stale_workspace_focus() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let (raise_manager_tx, mut raise_manager_rx) = actor::channel();
    reactor.communication_manager.raise_manager_tx = raise_manager_tx;

    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let stale_focus = WindowId::new(1, 1);
    let activated = WindowId::new(2, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    reactor.handle_events(apps.make_app(2, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    reactor.send_layout_event(LayoutEvent::WindowFocused(space, stale_focus));
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::MoveWindowToWorkspace { workspace: 1, window_id: None },
    )));
    apps.simulate_until_quiet(&mut reactor);

    reactor.send_layout_event(LayoutEvent::WindowFocused(space, activated));
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::MoveWindowToWorkspace { workspace: 1, window_id: None },
    )));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::SwitchToWorkspace(1),
    )));
    reactor.send_layout_event(LayoutEvent::WindowFocused(space, stale_focus));
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::SwitchToWorkspace(0),
    )));
    while raise_manager_rx.try_recv().is_ok() {}

    reactor.maybe_auto_switch_to_window_workspace(activated.pid, activated, space);

    let msg = raise_manager_rx.try_recv().expect("Should have sent an event").1;
    match msg {
        raise_manager::Event::RaiseRequest(RaiseRequest { focus_window, focus_quiet, .. }) => {
            assert_eq!(focus_window.map(|(wid, _)| wid), Some(activated));
            assert_eq!(focus_quiet, Quiet::Yes);
        }
        _ => panic!("Unexpected event: {msg:?}"),
    }
}

#[test]
fn windows_discovered_does_not_reintroduce_inactive_workspace_window() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(1, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::MoveWindowToWorkspace {
            workspace: 1,
            window_id: Some(2),
        },
    )));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::SwitchToWorkspace(1),
    )));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![WindowId::new(1, 1), WindowId::new(1, 2)],
    });

    assert_eq!(
        reactor.layout_manager.layout_engine.windows_in_active_workspace(space),
        vec![WindowId::new(1, 2)],
    );
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)], vec![]));

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

    reactor.handle_event(space_state_event(vec![CGRect::ZERO], vec![None], vec![]));
    reactor.handle_event(space_state_event(
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)], vec![]));

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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)], vec![]));

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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)], vec![]));

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

    reactor.handle_event(space_state_event(vec![full_screen], vec![None], vec![]));

    // Simulate a native fullscreen transition: space temporarily becomes a fullscreen
    // space id (reactor suppresses it to None), then returns to the original space.
    let fullscreen_space = SpaceId::new(0x400000000 + space.get());
    reactor.handle_event(space_state_event(
        vec![full_screen],
        vec![Some(fullscreen_space)],
        vec![],
    ));

    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)], vec![]));

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
fn animated_layout_handles_windows_without_server_ids() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let space = SpaceId::new(1);
    reactor.handle_event(space_state_event(
        vec![CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.))],
        vec![Some(space)],
        vec![],
    ));

    let mut window = make_window(1);
    window.sys_id = None;
    window.frame = CGRect::new(CGPoint::new(50., 50.), CGSize::new(400., 400.));

    reactor.handle_events(apps.make_app_with_opts(
        1,
        vec![window],
        Some(WindowId::new(1, 1)),
        true,
        false,
    ));
    apps.requests();

    let target = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    assert!(super::animation::AnimationManager::animate_layout(
        &mut reactor,
        space,
        &[(WindowId::new(1, 1), target)],
        true,
        None,
    ));

    let requests = apps.requests();
    assert!(
        requests.iter().any(|request| matches!(
            request,
            Request::SetWindowFrame(..) | Request::SetBatchWindowFrame(..)
        )),
        "expected layout to still request a frame update without a server id: {requests:?}"
    );
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
    reactor.handle_event(space_state_event(
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
fn display_churn_capture_is_committed_on_next_forwarded_space_state() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    reactor.handle_event(space_state_event(vec![frame], vec![Some(space)], vec![]));

    reactor.pending_display_topology_commit = Some(PendingDisplayTopologyCommit {
        epoch: 2,
        started_at: std::time::Instant::now(),
        flags: crate::sys::skylight::DisplayReconfigFlags::ADD,
        pre_known_wsids: crate::common::collections::HashSet::default(),
    });
    assert!(reactor.pending_display_topology_commit.is_some());

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space)], vec![]));

    assert!(reactor.pending_display_topology_commit.is_none());
}

#[test]
fn display_churn_commit_removes_missing_window_from_active_layout() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let pid: pid_t = 42;
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 1);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(pid, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, space, frame, wid));
    reactor.window_manager.set_window_server_space(wsid, Some(space));
    reactor.window_manager.mark_window_visible(wsid);

    let mut pre_known_wsids = crate::common::collections::HashSet::default();
    pre_known_wsids.insert(wsid);
    let snapshot = crate::actor::reactor::display_topology::DisplaySnapshot {
        ordered_screens: reactor.space_state.screens.clone(),
        active_spaces: reactor.active_spaces.clone(),
        inactive_spaces: crate::common::collections::HashSet::default(),
        windows: crate::common::collections::HashMap::default(),
    };

    reactor.reconcile_windows_after_topology_commit(
        1,
        std::time::Instant::now(),
        crate::sys::skylight::DisplayReconfigFlags::REMOVE,
        pre_known_wsids,
        snapshot,
    );

    assert!(
        !has_window_in_layout(&mut reactor, space, frame, wid),
        "active-space window missing from the authoritative post-churn snapshot must be removed immediately"
    );
    assert!(
        !reactor.window_manager.is_window_visible(wsid),
        "synthetic destroy during topology commit should clear visible state"
    );
}

#[test]
fn display_churn_quarantines_window_frame_changed_events() {
    let reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let _ = crate::sys::display_churn::begin(crate::sys::skylight::DisplayReconfigFlags::ADD);

    let quarantined = reactor.should_quarantine_during_display_churn(&Event::WindowFrameChanged(
        WindowId::new(99, 1),
        CGRect::new(CGPoint::new(10., 10.), CGSize::new(500., 400.)),
        None,
        Requested(false),
        Some(MouseState::Up),
    ));

    let _ = crate::sys::display_churn::end();
    assert!(
        quarantined,
        "WindowFrameChanged should be quarantined during churn"
    );
}

#[test]
fn normal_macos_space_switch_does_not_arm_topology_relayout() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1280., 800.));
    let right = CGRect::new(CGPoint::new(1280., 0.), CGSize::new(1280., 800.));

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(SpaceId::new(11)), Some(SpaceId::new(22))],
        vec![],
    ));
    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(SpaceId::new(111)), Some(SpaceId::new(222))],
        vec![],
    ));
    assert_eq!(
        reactor.raw_spaces_for_current_screens(),
        vec![Some(SpaceId::new(111)), Some(SpaceId::new(222))],
        "Screen state should still advance to the newly active macOS spaces"
    );
    assert!(reactor.is_space_active(SpaceId::new(111)));
    assert!(reactor.is_space_active(SpaceId::new(222)));
}

#[test]
fn fullscreen_space_in_screen_params_does_not_trigger_topology_relayout() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1280., 800.));
    let user_space = SpaceId::new(11);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let display_uuid = "11111111-1111-1111-1111-111111111111".to_string();
    let screens_for = |space: SpaceId| -> Vec<ScreenInfo> {
        vec![ScreenInfo {
            id: crate::sys::screen::ScreenId::new(0),
            frame,
            space: Some(space),
            display_uuid: display_uuid.clone(),
            name: None,
        }]
    };

    reactor.handle_event(space_state_event_from_screens(screens_for(user_space)));
    assert_eq!(
        reactor.layout_manager.layout_engine.last_space_for_display_uuid(&display_uuid),
        Some(user_space)
    );

    reactor
        .space_state
        .fullscreen_by_space
        .insert(fullscreen_space.get(), FullscreenSpaceTrack::default());
    reactor.handle_event(space_state_event_from_screens(screens_for(user_space).into_iter().map(|mut screen| {
        screen.space = None;
        screen
    }).collect()));
    assert_eq!(
        reactor.layout_manager.layout_engine.last_space_for_display_uuid(&display_uuid),
        Some(user_space),
        "fullscreen spaces should not replace display->user-space history"
    );

    reactor.handle_event(space_state_event_from_screens(screens_for(user_space)));
    assert_eq!(
        reactor.layout_manager.layout_engine.last_space_for_display_uuid(&display_uuid),
        Some(user_space)
    );
}

#[test]
fn fullscreen_screen_params_preserves_other_display_space() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let left_space_2 = SpaceId::new(12);
    let right_space_1 = SpaceId::new(21);
    let right_fullscreen = SpaceId::new(0x400000000 + right_space_1.get());

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(left_space_2), Some(right_space_1)],
        vec![],
    ));
    reactor
        .space_state
        .fullscreen_by_space
        .insert(right_fullscreen.get(), FullscreenSpaceTrack::default());

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(left_space_2), None],
        vec![],
    ));

    assert_eq!(
        reactor.raw_spaces_for_current_screens(),
        vec![Some(left_space_2), None],
        "Entering fullscreen on one display must not accept a transient user-space change on another display"
    );
}

#[test]
fn fullscreen_space_changed_preserves_other_display_space() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let left_space_2 = SpaceId::new(12);
    let right_space_1 = SpaceId::new(21);
    let right_fullscreen = SpaceId::new(0x400000000 + right_space_1.get());

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(left_space_2), Some(right_space_1)],
        vec![],
    ));
    reactor
        .space_state
        .fullscreen_by_space
        .insert(right_fullscreen.get(), FullscreenSpaceTrack::default());

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(left_space_2), None],
        vec![],
    ));

    assert_eq!(
        reactor.raw_spaces_for_current_screens(),
        vec![Some(left_space_2), None],
        "Fullscreen SpaceChanged snapshots must preserve unrelated displays' previous user spaces"
    );
}

#[test]
fn user_space_switch_is_allowed_while_other_display_already_fullscreen() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let left_space_2 = SpaceId::new(12);
    let left_space_1 = SpaceId::new(11);
    let right_space_1 = SpaceId::new(21);
    let right_fullscreen = SpaceId::new(0x400000000 + right_space_1.get());

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(left_space_2), Some(right_space_1)],
        vec![],
    ));
    reactor
        .space_state
        .fullscreen_by_space
        .insert(right_fullscreen.get(), FullscreenSpaceTrack::default());
    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(left_space_2), None],
        vec![],
    ));

    reactor.handle_event(space_state_event(
        vec![left, right],
        vec![Some(left_space_1), None],
        vec![],
    ));

    assert_eq!(
        reactor.raw_spaces_for_current_screens(),
        vec![Some(left_space_1), None],
        "Once another display is already fullscreen, user space switches on this display should still be accepted"
    );
}

#[test]
fn fullscreen_screen_params_preserves_window_layout() {
    // Regression test for #308: waking from sleep while a fullscreen video is
    // active should not wipe workspace assignments.
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));

    // Set up a display with a user space and some windows.
    reactor.handle_event(space_state_event(
        vec![full_screen],
        vec![Some(user_space)],
        vec![],
    ));
    reactor.handle_events(apps.make_app_with_opts(
        1,
        make_windows(3),
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    // Rearrange layout so we can detect if it gets reset.
    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::MoveNode(
        Direction::Up,
    ))));
    apps.simulate_until_quiet(&mut reactor);
    let layout_before = reactor.layout_manager.layout_engine.calculate_layout(
        user_space,
        full_screen,
        &reactor.config.settings.layout.gaps,
        0.0,
        crate::common::config::HorizontalPlacement::Top,
        crate::common::config::VerticalPlacement::Right,
    );

    // Simulate sleep/wake while fullscreen: ScreenParametersChanged arrives
    // with the fullscreen space id.
    reactor
        .space_state
        .fullscreen_by_space
        .insert(fullscreen_space.get(), FullscreenSpaceTrack::default());
    reactor.handle_event(space_state_event_from_screens(vec![ScreenInfo {
        id: crate::sys::screen::ScreenId::new(0),
        frame: full_screen,
        space: None,
        display_uuid: "test-display-0".to_string(),
        name: None,
    }]));
    apps.simulate_until_quiet(&mut reactor);

    // The fullscreen space must not become the active space for the screen.
    assert_eq!(
        reactor.space_state.screens[0].space, None,
        "fullscreen space should be nulled out, not stored as screen space"
    );

    // Return to user space (simulates exiting fullscreen).
    reactor.handle_event(space_state_event(
        vec![full_screen],
        vec![Some(user_space)],
        vec![],
    ));
    apps.simulate_until_quiet(&mut reactor);

    let layout_after = reactor.layout_manager.layout_engine.calculate_layout(
        user_space,
        full_screen,
        &reactor.config.settings.layout.gaps,
        0.0,
        crate::common::config::HorizontalPlacement::Top,
        crate::common::config::VerticalPlacement::Right,
    );
    assert_eq!(
        layout_before, layout_after,
        "Window layout on user space must be preserved across fullscreen ScreenParametersChanged"
    );
}

// Helper: check whether any window owned by `pid` appears in the layout tree for `space`.
fn has_windows_in_layout(
    reactor: &mut Reactor,
    space: SpaceId,
    screen: CGRect,
    pid: pid_t,
) -> bool {
    let gaps = reactor.config.settings.layout.gaps.clone();
    reactor
        .layout_manager
        .layout_engine
        .calculate_layout(space, screen, &gaps, 0.0, Default::default(), Default::default())
        .iter()
        .any(|(wid, _)| wid.pid == pid)
}

fn has_window_in_layout(
    reactor: &mut Reactor,
    space: SpaceId,
    screen: CGRect,
    wid: WindowId,
) -> bool {
    let gaps = reactor.config.settings.layout.gaps.clone();
    reactor
        .layout_manager
        .layout_engine
        .calculate_layout(space, screen, &gaps, 0.0, Default::default(), Default::default())
        .iter()
        .any(|(layout_wid, _)| *layout_wid == wid)
}

type WindowUpdateTuple = (
    WindowId,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    CGSize,
    Option<CGSize>,
    Option<CGSize>,
);

fn window_update_tuple(wid: WindowId) -> WindowUpdateTuple {
    (
        wid,
        None,
        None,
        None,
        true,
        CGSize::new(100.0, 100.0),
        None,
        None,
    )
}

struct TwoSpaceFixture {
    reactor: Reactor,
    screen1: CGRect,
    screen2: CGRect,
    space1: SpaceId,
    space2: SpaceId,
}

fn two_space_fixture() -> TwoSpaceFixture {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let screen2 = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);

    reactor.handle_event(space_state_event(
        vec![screen1, screen2],
        vec![Some(space1), Some(space2)],
        vec![],
    ));

    TwoSpaceFixture {
        reactor,
        screen1,
        screen2,
        space1,
        space2,
    }
}

// --- Display oscillation bug regression tests ---
//
// These tests cover the bug where a window enters a permanent oscillation state after a
// display topology change (e.g. MacBook lid open/close with an external monitor).  The
// root cause was that `sync_tiled_windows_for_app` could leave a window in two space
// layout trees simultaneously: after the window moved to the destination space its
// original source space still retained it, causing both spaces to issue conflicting
// SetWindowFrame calls that fed back into each other indefinitely.

#[test]
fn window_removed_from_source_space_when_dest_claims_it_first() {
    // Case 1: the destination space's WindowsOnScreenUpdated event fires before the
    // source space's empty event.  The VWM is updated by the destination event, so when
    // the source guard logic runs it can see that the window was moved away.
    let TwoSpaceFixture {
        mut reactor,
        screen1,
        screen2,
        space1,
        space2,
    } = two_space_fixture();
    let pid: pid_t = 42;
    let wid = WindowId::new(pid, 1);

    // Place window in space1's layout tree via a direct layout event.
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(
            space1,
            pid,
            vec![window_update_tuple(wid)],
            None,
        ));
    assert!(has_windows_in_layout(&mut reactor, space1, screen1, pid));

    // Destination space2 claims the window first (updates VWM: wid moves out of space1).
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(
            space2,
            pid,
            vec![window_update_tuple(wid)],
            None,
        ));

    // Source space1 receives the authoritative empty update.
    // Before the fix the guard in sync_tiled_windows_for_app checked only
    // has_windows_for_app (true) and skipped removal.  After the fix it also checks
    // whether those tree windows have been moved away in the VWM, and proceeds with
    // removal when they have.
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(space1, pid, vec![], None));

    assert!(
        !has_windows_in_layout(&mut reactor, space1, screen1, pid),
        "window must be removed from source space after destination claimed it"
    );
    assert!(
        has_windows_in_layout(&mut reactor, space2, screen2, pid),
        "window must remain in destination space"
    );
}

#[test]
fn empty_update_removes_window_when_vwm_was_preupdated() {
    // The reactor-level pre-pass in emit_layout_events updates the VWM for all claimed
    // windows upfront. This test mirrors that by updating the VWM directly before the
    // source's empty event.
    let TwoSpaceFixture {
        mut reactor,
        screen1,
        screen2,
        space1,
        space2,
    } = two_space_fixture();
    let pid: pid_t = 42;
    let wid = WindowId::new(pid, 1);

    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(
            space1,
            pid,
            vec![window_update_tuple(wid)],
            None,
        ));
    assert!(has_windows_in_layout(&mut reactor, space1, screen1, pid));

    // Simulate the pre-pass: move wid from space1 to space2 in the VWM before any
    // per-space events fire.
    let space2_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager()
        .active_workspace(space2)
        .expect("space2 must have an active workspace");
    reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .assign_window_to_workspace(space2, wid, space2_workspace);

    // Source space1's empty event fires first.  Because the VWM was pre-updated the
    // loop no longer re-adds wid to `desired`, so removal proceeds.
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(space1, pid, vec![], None));

    assert!(
        !has_windows_in_layout(&mut reactor, space1, screen1, pid),
        "window must be removed from source space when VWM was pre-updated (pre-pass scenario)"
    );

    // Destination space2 event fires after.
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(
            space2,
            pid,
            vec![window_update_tuple(wid)],
            None,
        ));
    assert!(has_windows_in_layout(&mut reactor, space2, screen2, pid));
}

#[test]
fn empty_update_only_removes_same_app_windows_moved_to_another_space() {
    // Mixed same-app case: one window moved to another space, while another window is
    // still assigned here but temporarily omitted from discovery. The empty update
    // should remove only the moved window from the source layout tree.
    let TwoSpaceFixture {
        mut reactor,
        screen1,
        screen2,
        space1,
        space2,
    } = two_space_fixture();
    let pid: pid_t = 42;
    let moved = WindowId::new(pid, 1);
    let retained = WindowId::new(pid, 2);

    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(
            space1,
            pid,
            vec![window_update_tuple(moved), window_update_tuple(retained)],
            None,
        ));
    assert!(has_window_in_layout(&mut reactor, space1, screen1, moved));
    assert!(has_window_in_layout(&mut reactor, space1, screen1, retained));

    let space2_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager()
        .active_workspace(space2)
        .expect("space2 must have an active workspace");
    reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .assign_window_to_workspace(space2, moved, space2_workspace);

    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(space1, pid, vec![], None));

    assert!(
        !has_window_in_layout(&mut reactor, space1, screen1, moved),
        "moved window must be removed from the source layout tree"
    );
    assert!(
        has_window_in_layout(&mut reactor, space1, screen1, retained),
        "same-app window still assigned to source space must be preserved"
    );

    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(
            space2,
            pid,
            vec![window_update_tuple(moved)],
            None,
        ));
    assert!(has_window_in_layout(&mut reactor, space2, screen2, moved));
}

#[test]
fn window_preserved_in_space_on_empty_discovery_without_cross_space_move() {
    // Regression guard for the login-screen / AX-failure scenario: when the
    // accessibility API returns an empty window list but the window has NOT been moved
    // to another space in the VWM, the empty update must not destroy the layout.
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let pid: pid_t = 42;
    let wid = WindowId::new(pid, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));

    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(
            space,
            pid,
            vec![window_update_tuple(wid)],
            None,
        ));
    assert!(has_windows_in_layout(&mut reactor, space, screen, pid));

    // AX returns empty — window is still in the VWM for this space (it was never moved).
    let _ = reactor
        .layout_manager
        .layout_engine
        .handle_event(LayoutEvent::WindowsOnScreenUpdated(space, pid, vec![], None));

    assert!(
        has_windows_in_layout(&mut reactor, space, screen, pid),
        "window must be preserved when empty update has no cross-space move (login screen / AX failure)"
    );
}

#[test]
fn discovery_after_display_change_places_window_on_correct_display() {
    // End-to-end integration test: a window that physically moved to a different
    // display after a topology change (lid open/close) must end up in only the new
    // display's layout tree, with no conflicting SetWindowFrame from the old one.
    //
    // This exercises the full WindowsDiscovered → emit_layout_events path including
    // the pre-pass VWM update (Case 2: source space processed first in screen order).
    let mut apps = Apps::new();
    let TwoSpaceFixture {
        mut reactor,
        screen1,
        screen2,
        space1,
        space2,
    } = two_space_fixture();

    // Window starts on screen1.
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    assert_eq!(screen1, apps.windows[&WindowId::new(1, 1)].frame);

    // Simulate a topology change: the window has moved to screen2.
    // Passing it in `new` with an updated frame causes process_window_list to update
    // frame_monotonic so emit_layout_events assigns it to space2.
    // Note: without the fix this triggers the oscillation and simulate_until_quiet
    // would loop forever; the test itself documents that termination is part of the
    // expected behaviour.
    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(WindowId::new(1, 1), WindowInfo {
            frame: CGRect::new(CGPoint::new(1100., 100.), CGSize::new(50., 50.)),
            ..make_window(1)
        })],
        known_visible: vec![WindowId::new(1, 1)],
    });
    apps.simulate_until_quiet(&mut reactor);

    assert!(
        !has_windows_in_layout(&mut reactor, space1, screen1, 1),
        "space1 layout tree must not contain the window after it moved to screen2"
    );
    assert!(
        has_windows_in_layout(&mut reactor, space2, screen2, 1),
        "space2 layout tree must contain the window after it moved to screen2"
    );
    assert_eq!(
        screen2,
        apps.windows[&WindowId::new(1, 1)].frame,
        "window must be laid out on screen2"
    );
}

#[test]
fn discovery_minimize_transition_removes_window_from_layout() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, space, screen, wid));

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(wid, WindowInfo {
            is_minimized: true,
            ..make_window(1)
        })],
        known_visible: vec![],
    });

    assert!(
        !has_window_in_layout(&mut reactor, space, screen, wid),
        "minimized window must be removed from layout when discovery reports it minimized"
    );
    assert!(
        reactor
            .window_manager
            .window(wid)
            .is_some_and(|window| window.info.is_minimized),
        "reactor state must keep the window marked minimized"
    );
}

#[test]
fn discovery_restore_transition_readds_window_to_layout() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);
    let mut windows = make_windows(1);
    windows[0].is_minimized = true;

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)], vec![]));
    reactor.handle_events(apps.make_app(1, windows));
    apps.simulate_until_quiet(&mut reactor);

    assert!(
        !has_window_in_layout(&mut reactor, space, screen, wid),
        "startup-minimized window must not be inserted into layout"
    );

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(wid, make_window(1))],
        known_visible: vec![wid],
    });

    assert!(
        has_window_in_layout(&mut reactor, space, screen, wid),
        "restored window must return to layout when discovery reports it visible again"
    );
    assert!(
        reactor
            .window_manager
            .window(wid)
            .is_some_and(|window| !window.info.is_minimized),
        "reactor state must clear the minimized flag after restore"
    );
}

#[test]
fn unfullscreen_restores_window_tracking() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));

    // Set up a display with a user space and some windows.
    reactor.handle_event(space_state_event(
        vec![full_screen],
        vec![Some(user_space)],
        vec![],
    ));
    reactor.handle_events(apps.make_app_with_opts(
        1,
        make_windows(1),
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    // Record the window as fullscreened.
    let window_id = WindowId::new(1, 1);
    reactor
        .space_state
        .fullscreen_by_space
        .insert(fullscreen_space.get(), FullscreenSpaceTrack {
            windows: vec![FullscreenWindowTrack {
                pid: 1,
                window_id: Some(window_id),
                last_known_user_space: Some(user_space),
                _last_seen_fullscreen_space: fullscreen_space,
            }],
        });

    // Transition to fullscreen space.
    reactor.handle_event(space_state_event(
        vec![full_screen],
        vec![None],
        vec![],
    ));
    apps.simulate_until_quiet(&mut reactor);

    // Exit fullscreen (return to user space).
    reactor.handle_event(space_state_event(
        vec![full_screen],
        vec![Some(user_space)],
        vec![],
    ));

    // The reactor should trigger a GetVisibleWindows request.
    let mut saw_get_visible_windows = false;
    for request in apps.requests() {
        if matches!(request, Request::GetVisibleWindows) {
            saw_get_visible_windows = true;
        }
    }
    assert!(
        saw_get_visible_windows,
        "Should send GetVisibleWindows to app on unfullscreen"
    );

    // The fullscreen track should be removed.
    assert!(
        !reactor.space_state.fullscreen_by_space.contains_key(&fullscreen_space.get()),
        "Fullscreen track should be removed from space manager"
    );
}
