use objc2_core_foundation::{CGPoint, CGSize};
use test_log::test;

use super::testing::*;
use super::*;
use crate::actor::app::{AppThreadHandle, Request, pid_t};
use crate::common::config::OuterGaps;
use crate::layout_engine::{Direction, LayoutCommand, LayoutEngine, LayoutEvent};
use crate::model::window_store::NativeFullscreenTransition;
use crate::sys::app::{AppInfo, WindowInfo};
use crate::sys::geometry::SameAs;
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(SpaceId::new(1))]));

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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(SpaceId::new(1))]));
    assert_eq!(1, reactor.space_state.screens.len());

    reactor.handle_event(space_state_event(vec![], vec![]));
    assert!(reactor.space_state.screens.is_empty());
    assert_eq!(reactor.raw_command_space(), None);
    assert_eq!(reactor.space_state.menu_bar_space, None);
    assert!(reactor.space_state.display_space_ids.is_empty());

    reactor.handle_event(space_state_event(vec![], vec![]));
    assert!(reactor.space_state.screens.is_empty());
    assert_eq!(reactor.raw_command_space(), None);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(SpaceId::new(1))]));
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(old_space)]));
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(new_space)]));

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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(old_space)]));
    assert_eq!(reactor.workspace_command_space(), Some(old_space));

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen], vec![Some(new_space)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: false,
        active_spaces: [new_space].into_iter().collect(),
        menu_bar_space: Some(new_space),
        command_space: Some(new_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    assert_eq!(reactor.workspace_command_space(), Some(new_space));
}

#[test]
fn forwarded_active_spaces_filter_active_workspace_context() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let inactive_space = SpaceId::new(1);
    let active_space = SpaceId::new(2);

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![left, right], vec![
            Some(inactive_space),
            Some(active_space),
        ]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: false,
        active_spaces: [active_space].into_iter().collect(),
        menu_bar_space: Some(active_space),
        command_space: Some(active_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    assert!(!reactor.is_space_active(inactive_space));
    assert!(reactor.is_space_active(active_space));
    assert_eq!(
        reactor.space_state.active_spaces,
        [active_space].into_iter().collect(),
        "the stored forwarded state should reflect the authority's active-space set",
    );
}

#[test]
fn forwarded_space_snapshot_respects_default_disable_policy() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.config.settings.default_disable = true;

    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));

    assert!(
        !reactor.is_space_active(space),
        "forwarded raw active spaces must still be filtered by default_disable policy"
    );
}

#[test]
fn forwarded_space_snapshot_respects_one_space_policy() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    reactor.one_space = true;

    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(space1),
        Some(space2),
    ]));

    assert!(reactor.is_space_active(space1));
    assert!(
        !reactor.is_space_active(space2),
        "forwarded raw active spaces must not bypass one_space filtering"
    );
}

#[test]
fn forwarded_space_snapshot_respects_toggled_space_activation_policy() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    assert!(reactor.is_space_active(space));

    reactor.handle_event(Event::Command(Command::Reactor(
        ReactorCommand::ToggleSpaceActivated,
    )));
    assert!(!reactor.is_space_active(space));

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));

    assert!(
        !reactor.is_space_active(space),
        "forwarded raw active spaces must not re-enable a space disabled by ToggleSpaceActivated"
    );
}

#[test]
fn layout_commands_follow_active_display_space_across_active_displays() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
    let right = CGRect::new(CGPoint::new(1440., 0.), CGSize::new(1440., 900.));
    let left_space = SpaceId::new(1);
    let right_space = SpaceId::new(2);
    let source = WindowId::new(1, 1);
    let target_a = WindowId::new(1, 2);
    let target_b = WindowId::new(1, 3);
    let windows = [
        (source, WindowServerId::new(101), left_space, left),
        (target_a, WindowServerId::new(102), right_space, right),
        (target_b, WindowServerId::new(103), right_space, right),
    ];

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space),
        Some(right_space),
    ]));

    let (app_tx, _app_rx) = crate::actor::channel();
    reactor.app_manager.apps.insert(1, AppState {
        info: AppInfo {
            bundle_id: Some("com.test.app".to_string()),
            localized_name: Some("Test App".to_string()),
        },
        handle: AppThreadHandle::new_for_test(app_tx),
    });

    reactor.send_layout_event(LayoutEvent::SpaceExposed(left_space, left.size));
    reactor.send_layout_event(LayoutEvent::SpaceExposed(right_space, right.size));

    let left_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(left_space)
        .first()
        .map(|(id, _)| *id)
        .expect("left workspace");
    let right_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(right_space)
        .first()
        .map(|(id, _)| *id)
        .expect("right workspace");

    for (wid, wsid, space, frame) in windows {
        reactor.state.windows.track_window_server_id(wsid, wid);
        reactor.state.windows.track_window_server_info(
            crate::sys::window_server::WindowServerInfo {
                id: wsid,
                pid: wid.pid,
                layer: 0,
                frame,
                min_frame: frame.size,
                max_frame: frame.size,
            },
        );
        reactor.state.windows.set_window_server_space(wsid, Some(space));
        reactor.state.windows.mark_window_visible(wsid);
        reactor.state.windows.insert_window(wid, WindowState {
            info: WindowInfo {
                is_standard: true,
                is_root: true,
                is_minimized: false,
                is_resizable: true,
                min_size: None,
                max_size: None,
                title: format!("Window {:?}", wid),
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
        let workspace = if space == left_space {
            left_workspace
        } else {
            right_workspace
        };
        assert!(
            reactor
                .layout_manager
                .layout_engine
                .virtual_workspace_manager_mut()
                .assign_window_to_workspace(&mut reactor.state.windows, space, wid, workspace)
        );
        reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
    }

    reactor.send_layout_event(LayoutEvent::WindowFocused(right_space, target_a));

    assert_eq!(reactor.workspace_command_space(), Some(left_space));
    assert_eq!(reactor.command_context_space(), Some(left_space));
    assert_eq!(
        reactor.layout_manager.layout_engine.focused_window(),
        Some(target_a)
    );

    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::NextWindow)));

    assert_eq!(
        reactor.layout_manager.layout_engine.focused_window(),
        Some(source),
        "non-workspace layout commands should follow the active display space"
    );
}

#[test]
fn workspace_commands_follow_active_display_space_across_active_displays() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
    let right = CGRect::new(CGPoint::new(1440., 0.), CGSize::new(1440., 900.));
    let left_space = SpaceId::new(1);
    let right_space = SpaceId::new(2);
    let source = WindowId::new(1, 1);
    let target = WindowId::new(1, 2);
    let windows = [
        (source, WindowServerId::new(201), left_space, left),
        (target, WindowServerId::new(202), right_space, right),
    ];

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space),
        Some(right_space),
    ]));

    let (app_tx, _app_rx) = crate::actor::channel();
    reactor.app_manager.apps.insert(1, AppState {
        info: AppInfo {
            bundle_id: Some("com.test.app".to_string()),
            localized_name: Some("Test App".to_string()),
        },
        handle: AppThreadHandle::new_for_test(app_tx),
    });

    reactor.send_layout_event(LayoutEvent::SpaceExposed(left_space, left.size));
    reactor.send_layout_event(LayoutEvent::SpaceExposed(right_space, right.size));

    let left_workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(left_space)
        .to_vec();
    let right_workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(right_space)
        .to_vec();
    let left_workspace = left_workspaces.first().map(|(id, _)| *id).expect("left workspace");
    let next_left_workspace =
        left_workspaces.get(1).map(|(id, _)| *id).expect("left next workspace");
    let right_workspace = right_workspaces.first().map(|(id, _)| *id).expect("right workspace");

    for (wid, wsid, space, frame) in windows {
        reactor.state.windows.track_window_server_id(wsid, wid);
        reactor.state.windows.track_window_server_info(
            crate::sys::window_server::WindowServerInfo {
                id: wsid,
                pid: wid.pid,
                layer: 0,
                frame,
                min_frame: frame.size,
                max_frame: frame.size,
            },
        );
        reactor.state.windows.set_window_server_space(wsid, Some(space));
        reactor.state.windows.mark_window_visible(wsid);
        reactor.state.windows.insert_window(wid, WindowState {
            info: WindowInfo {
                is_standard: true,
                is_root: true,
                is_minimized: false,
                is_resizable: true,
                min_size: None,
                max_size: None,
                title: format!("Window {:?}", wid),
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
        let workspace = if space == left_space {
            left_workspace
        } else {
            right_workspace
        };
        assert!(
            reactor
                .layout_manager
                .layout_engine
                .virtual_workspace_manager_mut()
                .assign_window_to_workspace(&mut reactor.state.windows, space, wid, workspace)
        );
        reactor.send_layout_event(LayoutEvent::WindowAdded(space, wid));
    }

    reactor.send_layout_event(LayoutEvent::WindowFocused(right_space, target));

    assert_eq!(reactor.workspace_command_space(), Some(left_space));
    assert_eq!(reactor.command_context_space(), Some(left_space));
    assert_eq!(
        reactor.layout_manager.layout_engine.active_workspace(right_space),
        Some(right_workspace)
    );

    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::NextWorkspace(
        None,
    ))));

    assert_eq!(
        reactor.layout_manager.layout_engine.active_workspace(left_space),
        Some(next_left_workspace),
        "workspace commands should follow the active display space"
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.active_workspace(right_space),
        Some(right_workspace),
        "workspace commands should not switch the focused window's display when it is not active"
    );
}

#[test]
fn command_space_only_snapshot_does_not_trigger_full_space_reconcile() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![left, right], vec![Some(space1), Some(space2)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [space1, space2].into_iter().collect(),
        menu_bar_space: Some(space1),
        command_space: Some(space1),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    assert!(apps.requests().is_empty());

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![left, right], vec![Some(space1), Some(space2)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [space1, space2].into_iter().collect(),
        menu_bar_space: Some(space2),
        command_space: Some(space2),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    assert_eq!(reactor.workspace_command_space(), Some(space2));
    assert!(
        apps.requests().is_empty(),
        "changing only command_space should not trigger visible-window refresh or space reconciliation"
    );
}

#[test]
fn active_display_update_only_changes_command_context() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let left_space = SpaceId::new(1);
    let right_space = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space),
        Some(right_space),
    ]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    assert!(apps.requests().is_empty());

    reactor.handle_event(Event::ActiveDisplayChanged {
        menu_bar_space: Some(right_space),
        command_space: Some(right_space),
    });

    assert_eq!(reactor.workspace_command_space(), Some(right_space));
    assert_eq!(reactor.space_state.menu_bar_space, Some(right_space));
    assert!(
        apps.requests().is_empty(),
        "active-display updates must not trigger window discovery"
    );
}

#[test]
fn passive_command_space_change_does_not_override_clicked_window_focus() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let (raise_manager_tx, mut raise_manager_rx) = actor::channel();
    reactor.communication_manager.raise_manager_tx = raise_manager_tx;

    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let left_space = SpaceId::new(1);
    let right_space = SpaceId::new(2);
    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![left, right], vec![
            Some(left_space),
            Some(right_space),
        ]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [left_space, right_space].into_iter().collect(),
        menu_bar_space: Some(left_space),
        command_space: Some(left_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    let mut windows = make_windows(2);
    windows[1].frame.origin = CGPoint::new(1100., 100.);
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    reactor.handle_events(apps.make_app_with_opts(
        1,
        windows,
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    apps.simulate_until_quiet(&mut reactor);

    let old_focus = WindowId::new(1, 1);
    let destination_focus = WindowId::new(1, 2);
    reactor.send_layout_event(LayoutEvent::WindowFocused(right_space, destination_focus));
    reactor.send_layout_event(LayoutEvent::WindowFocused(left_space, old_focus));
    while raise_manager_rx.try_recv().is_ok() {}

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![left, right], vec![
            Some(left_space),
            Some(right_space),
        ]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [left_space, right_space].into_iter().collect(),
        menu_bar_space: Some(right_space),
        command_space: Some(right_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    assert_eq!(
        reactor.layout_manager.layout_engine.focused_window(),
        Some(old_focus),
        "a passive display snapshot must leave focus ownership to the AX click event"
    );
    assert!(
        raise_manager_rx.try_recv().is_err(),
        "a passive active-display change must not raise the workspace's stale selection"
    );

    reactor.handle_event(Event::ApplicationMainWindowChanged(
        1,
        Some(destination_focus),
        Quiet::No,
    ));
    assert_eq!(
        reactor.layout_manager.layout_engine.focused_window(),
        Some(destination_focus),
        "the subsequent AX focus event should select the window that activated the display"
    );
}

#[test]
fn discovery_does_not_replay_another_apps_global_main_window() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let space = SpaceId::new(1);
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));

    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    reactor.handle_events(apps.make_app_with_opts(
        1,
        make_windows(1),
        Some(WindowId::new(1, 1)),
        true,
        true,
    ));
    reactor.handle_events(apps.make_app_with_opts(2, make_windows(1), None, false, true));
    apps.simulate_until_quiet(&mut reactor);

    let app_two_window = WindowId::new(2, 1);
    reactor.send_layout_event(LayoutEvent::WindowFocused(space, app_two_window));
    let info = reactor
        .state
        .windows
        .window(app_two_window)
        .expect("app two window should be tracked")
        .info
        .clone();

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 2,
        new: vec![(app_two_window, info)],
        known_visible: vec![app_two_window],
    });

    assert_eq!(reactor.main_window(), Some(WindowId::new(1, 1)));
    assert_eq!(
        reactor.layout_manager.layout_engine.focused_window(),
        Some(app_two_window),
        "app-scoped discovery must not replay another app's global main window"
    );
}

#[test]
fn forwarded_space_state_updates_fullscreen_spaces() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen], vec![Some(user_space)]),
        fullscreen_spaces: [fullscreen_space].into_iter().collect(),
        has_seen_display_set: false,
        active_spaces: [user_space].into_iter().collect(),
        menu_bar_space: Some(user_space),
        command_space: Some(user_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    assert!(reactor.space_state.fullscreen_spaces.contains(&fullscreen_space));
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space1)]));
    let _ = reactor.layout_manager.layout_engine.handle_virtual_workspace_command(
        &mut reactor.state.windows,
        space1,
        &LayoutCommand::SwitchToWorkspace(0),
    );
    let _ = reactor.layout_manager.layout_engine.handle_virtual_workspace_command(
        &mut reactor.state.windows,
        space2,
        &LayoutCommand::SwitchToWorkspace(1),
    );

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen], vec![Some(space2)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: false,
        active_spaces: [space2].into_iter().collect(),
        menu_bar_space: Some(space2),
        command_space: Some(space1),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    assert_eq!(
        reactor.query_active_workspace(None),
        reactor.layout_manager.layout_engine.active_workspace(space2),
        "default queries must follow authoritative active space state, not stale command_space"
    );
}

#[test]
fn menu_bar_space_prefers_active_menu_bar_display_space() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(space1),
        Some(space2),
    ]));

    assert_eq!(reactor.test_default_query_space(), Some(space1));
    assert_eq!(
        reactor.test_resolve_menu_bar_space_with_preferred(Some(space2)),
        Some(space2),
        "menubar updates should follow the display currently hosting the menu bar"
    );
}

#[test]
fn menu_bar_space_falls_back_when_preferred_space_is_not_visible() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let visible_space = SpaceId::new(1);
    let hidden_space = SpaceId::new(2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(visible_space)]));

    assert_eq!(
        reactor.test_resolve_menu_bar_space_with_preferred(Some(hidden_space)),
        Some(visible_space),
        "menubar updates should fall back to the normal active context if the preferred menubar space is unavailable"
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

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(space1),
        Some(space2),
    ]));

    let _ = reactor.layout_manager.layout_engine.handle_virtual_workspace_command(
        &mut reactor.state.windows,
        space1,
        &LayoutCommand::SwitchToWorkspace(0),
    );
    let _ = reactor.layout_manager.layout_engine.handle_virtual_workspace_command(
        &mut reactor.state.windows,
        space2,
        &LayoutCommand::SwitchToWorkspace(1),
    );

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
        fullscreen_spaces: Default::default(),
        has_seen_display_set: false,
        active_spaces: [space2].into_iter().collect(),
        menu_bar_space: Some(space2),
        command_space: Some(space2),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
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

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space2)]));
    reactor.state.windows.track_window_server_id(wsid, wid);
    reactor.state.windows.set_window_server_space(wsid, Some(space1));
    reactor.state.windows.insert_window(wid, WindowState {
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

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space1)]));
    reactor.state.windows.track_window_server_id(wsid, wid);
    reactor.state.windows.set_window_server_space(wsid, Some(space1));
    reactor.state.windows.insert_window(wid, WindowState {
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

    crate::sys::window_server::set_window_ordered_in_override(wsid, Some(true));
    SpaceEventHandler::handle_window_server_destroyed(
        &mut reactor,
        SpaceEventHandler::WindowServerLifecyclePayload {
            window_server_id: wsid,
            space: space1,
            kind: SpaceEventKind::User,
        },
    )
    .unwrap();
    crate::sys::window_server::set_window_ordered_in_override(wsid, None);

    assert!(reactor.state.windows.contains_window(wid));
    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space1));
    assert!(!reactor.state.windows.is_window_visible(wsid));
}

#[test]
fn user_space_window_server_destroyed_removes_window_when_window_server_is_gone() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let wid = WindowId::new(1, 1);
    let wsid = WindowServerId::new(22);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space1)]));
    reactor.state.windows.track_window_server_id(wsid, wid);
    reactor.state.windows.set_window_server_space(wsid, Some(space1));
    reactor.state.windows.mark_window_visible(wsid);
    reactor.state.windows.insert_window(wid, WindowState {
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

    crate::sys::window_server::set_window_ordered_in_override(wsid, Some(false));
    SpaceEventHandler::handle_window_server_destroyed(
        &mut reactor,
        SpaceEventHandler::WindowServerLifecyclePayload {
            window_server_id: wsid,
            space: space1,
            kind: SpaceEventKind::User,
        },
    )
    .unwrap();
    crate::sys::window_server::set_window_ordered_in_override(wsid, None);

    assert!(!reactor.state.windows.contains_window(wid));
    assert_eq!(reactor.state.windows.tracked_window_id(wsid), None);
    assert_eq!(reactor.assigned_space_for_window_id(wid), None);
}

/// Builds a reactor with `space1` active on a screen and a single tiled window
/// (`wid`/`wsid`) assigned to `space1`. `space2` exists with workspaces so it can
/// be a reassignment target. Returns the pieces the `appeared` tests need.
fn reactor_with_window_on_space1() -> (Reactor, WindowId, WindowServerId, SpaceId, SpaceId, CGRect)
{
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let pid = 1;
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new(101);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space1)]));

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
    reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space2);

    reactor.state.windows.track_window_server_id(wsid, wid);
    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame,
            min_frame: frame.size,
            max_frame: frame.size,
        });
    reactor.state.windows.set_window_server_space(wsid, Some(space1));
    reactor.state.windows.mark_window_visible(wsid);
    reactor.state.windows.insert_window(wid, WindowState {
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

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space1, wid, space1_workspace)
    );
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space1));

    (reactor, wid, wsid, space1, space2, frame)
}

fn reactor_with_window_moved_to_space2()
-> (Reactor, WindowId, WindowServerId, SpaceId, SpaceId, CGRect) {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let pid = 1;
    let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
    let screen2 = CGRect::new(CGPoint::new(1440., 0.), CGSize::new(1440., 900.));
    let moved_frame = CGRect::new(CGPoint::new(1600., 100.), CGSize::new(800., 600.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new(111);

    reactor.handle_event(space_state_event(vec![screen1, screen2], vec![
        Some(space1),
        Some(space2),
    ]));

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

    reactor.state.windows.track_window_server_id(wsid, wid);
    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame: moved_frame,
            min_frame: moved_frame.size,
            max_frame: moved_frame.size,
        });
    reactor.state.windows.set_window_server_space(wsid, Some(space2));
    reactor.state.windows.mark_window_visible(wsid);
    reactor.state.windows.insert_window(wid, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Window".to_string(),
            frame: moved_frame,
            sys_id: Some(wsid),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: moved_frame,
        is_manageable: true,
        ignore_app_rule: false,
    });

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space1, wid, space1_workspace)
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space2, wid, space2_workspace)
    );
    let txid = reactor.transaction_manager.generate_next_txid(wsid);
    reactor.transaction_manager.store_txid(wsid, txid, moved_frame);
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));

    (reactor, wid, wsid, space1, space2, moved_frame)
}

fn reactor_with_window_on_space1_two_displays() -> (
    Reactor,
    WindowId,
    WindowServerId,
    SpaceId,
    SpaceId,
    CGRect,
    CGRect,
) {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let pid = 1;
    let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
    let screen2 = CGRect::new(CGPoint::new(1440., 0.), CGSize::new(1440., 900.));
    let initial_frame = CGRect::new(CGPoint::new(100., 100.), CGSize::new(800., 600.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new(121);

    reactor.handle_event(space_state_event(vec![screen1, screen2], vec![
        Some(space1),
        Some(space2),
    ]));

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
    reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space2);

    reactor.state.windows.track_window_server_id(wsid, wid);
    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame: initial_frame,
            min_frame: initial_frame.size,
            max_frame: initial_frame.size,
        });
    reactor.state.windows.set_window_server_space(wsid, Some(space1));
    reactor.state.windows.mark_window_visible(wsid);
    reactor.state.windows.insert_window(wid, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Window".to_string(),
            frame: initial_frame,
            sys_id: Some(wsid),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: initial_frame,
        is_manageable: true,
        ignore_app_rule: false,
    });

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space1, wid, space1_workspace)
    );

    (reactor, wid, wsid, space1, space2, initial_frame, screen2)
}

fn reactor_with_floating_window() -> (Reactor, WindowId, SpaceId, CGRect, CGRect) {
    let (mut reactor, wid, _wsid, space1, _space2, screen) = reactor_with_window_on_space1();
    reactor.send_layout_event(LayoutEvent::WindowAdded(space1, wid));
    reactor.send_layout_event(LayoutEvent::WindowFocused(space1, wid));
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::ToggleWindowFloating,
    )));
    assert!(reactor.layout_manager.layout_engine.is_window_floating(wid));

    let workspace = reactor
        .layout_manager
        .layout_engine
        .active_workspace(space1)
        .expect("workspace");
    let floating_frame = CGRect::new(CGPoint::new(100., 100.), CGSize::new(400., 300.));
    if let Some(w) = reactor.state.windows.window_mut(wid) {
        w.frame_monotonic = floating_frame;
    }
    reactor.layout_manager.layout_engine.store_floating_position(
        space1,
        workspace,
        wid,
        floating_frame,
    );

    (reactor, wid, space1, screen, floating_frame)
}

#[test]
fn appeared_reassigns_window_without_pending_rift_move() {
    let (mut reactor, wid, wsid, space1, space2, _frame) = reactor_with_window_on_space1();

    // No pending transaction: this is a genuine external space change, so Rift should
    // follow it and reassign the window to the reported space.
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space1));

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        space2,
        SpaceEventKind::User,
    );

    assert_eq!(
        reactor.assigned_space_for_window_id(wid),
        Some(space2),
        "window without an in-flight Rift move must follow a genuine external space change"
    );
}

#[test]
fn geometry_cross_display_frame_change_updates_authoritative_space() {
    let (mut reactor, wid, wsid, _space1, space2, _initial_frame, screen2) =
        reactor_with_window_on_space1_two_displays();
    let moved_frame = CGRect::new(
        CGPoint::new(screen2.origin.x + 100., 100.),
        CGSize::new(800., 600.),
    );

    reactor.handle_event(Event::WindowFrameChanged(
        wid,
        moved_frame,
        None,
        Requested(false),
        Some(MouseState::Up),
    ));

    assert_eq!(
        reactor.assigned_space_for_window_id(wid),
        Some(space2),
        "geometry-only cross-display move should update workspace ownership"
    );
    assert_eq!(
        reactor.state.windows.window_server_space(wsid),
        Some(space2),
        "geometry-only cross-display move should update authoritative server space"
    );
}

#[test]
fn matching_rift_frame_clears_pending_target() {
    let (mut reactor, wid, wsid, _space1, _space2, frame) = reactor_with_window_on_space1();
    let target_frame = CGRect::new(
        CGPoint::new(frame.origin.x + 40.0, frame.origin.y + 25.0),
        frame.size,
    );
    let txid = reactor.transaction_manager.generate_next_txid(wsid);
    reactor.transaction_manager.store_txid(wsid, txid, target_frame);

    reactor.handle_event(Event::WindowFrameChanged(
        wid,
        target_frame,
        Some(txid),
        Requested(true),
        Some(MouseState::Up),
    ));

    assert_eq!(
        reactor.transaction_manager.get_target_frame(wsid),
        None,
        "a confirmed Rift frame must clear the pending target"
    );
    assert!(
        reactor
            .state
            .windows
            .window(wid)
            .expect("window should still exist")
            .frame_monotonic
            .same_as(target_frame)
    );
}

#[test]
fn cross_display_drag_clears_source_floating_position() {
    let (mut reactor, wid, _wsid, space1, space2, initial_frame, screen2) =
        reactor_with_window_on_space1_two_displays();
    let source_workspace = reactor
        .layout_manager
        .layout_engine
        .active_workspace(space1)
        .expect("source workspace");
    let target_workspace = reactor
        .layout_manager
        .layout_engine
        .active_workspace(space2)
        .expect("target workspace");

    reactor.send_layout_event(LayoutEvent::WindowAdded(space1, wid));
    reactor.send_layout_event(LayoutEvent::WindowFocused(space1, wid));
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::ToggleWindowFloating,
    )));
    assert!(reactor.layout_manager.layout_engine.is_window_floating(wid));
    reactor.layout_manager.layout_engine.store_floating_position(
        space1,
        source_workspace,
        wid,
        initial_frame,
    );

    let moved_frame = CGRect::new(
        CGPoint::new(screen2.origin.x + 120.0, initial_frame.origin.y),
        initial_frame.size,
    );
    reactor.drag_manager.drag_state = DragState::Active {
        session: DragSession {
            window: wid,
            last_frame: moved_frame,
            origin_space: None,
            settled_space: Some(space2),
            layout_dirty: true,
        },
    };

    let (visible_spaces, visible_space_centers) = reactor.visible_spaces_for_layout(true);
    let outcome = crate::actor::reactor::events::drag::handle_mouse_up(
        &mut reactor.state,
        &mut reactor.layout_manager,
        &mut reactor.drag_manager,
        crate::actor::reactor::events::drag::MouseUpPayload {
            pending_swap: None,
            swap_space: Some(space2),
            final_space: Some(space2),
            visible_spaces,
            visible_space_centers,
        },
    )
    .unwrap();
    assert!(outcome.arrange.requested);
    assert!(matches!(reactor.drag_manager.drag_state, DragState::Inactive));

    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .get_floating_position(space1, source_workspace, wid),
        None,
        "cross-display drags must clear the source workspace's floating position"
    );
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .get_floating_position(space2, target_workspace, wid),
        Some(moved_frame)
    );
}

#[test]
fn stale_user_space_disappearance_does_not_restore_old_display_assignment() {
    let (mut reactor, wid, wsid, space1, space2, _) = reactor_with_window_moved_to_space2();

    SpaceEventHandler::handle_window_server_destroyed(
        &mut reactor,
        SpaceEventHandler::WindowServerLifecyclePayload {
            window_server_id: wsid,
            space: space1,
            kind: SpaceEventKind::User,
        },
    )
    .unwrap();

    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert!(reactor.state.windows.is_window_visible(wsid));

    let _ = reactor.reconcile_windows_with_authoritative_spaces();

    assert_eq!(
        reactor.assigned_space_for_window_id(wid),
        Some(space2),
        "late disappearance from the old display must not drag a moved window back"
    );
}

#[test]
fn stale_user_space_appearance_does_not_restore_old_display_assignment() {
    let (mut reactor, wid, wsid, space1, space2, _) = reactor_with_window_moved_to_space2();

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        space1,
        SpaceEventKind::User,
    );

    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));

    let _ = reactor.reconcile_windows_with_authoritative_spaces();

    assert_eq!(
        reactor.assigned_space_for_window_id(wid),
        Some(space2),
        "late appearance on the old display must not overwrite the newer target assignment"
    );
}

#[test]
fn stale_user_space_appearance_is_ignored_when_server_state_already_matches_pending_target() {
    let (mut reactor, wid, wsid, space1, space2, _frame) = reactor_with_window_moved_to_space2();
    let space1_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space1)
        .first()
        .map(|(id, _)| *id)
        .expect("space1 workspace");

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space1, wid, space1_workspace)
    );
    reactor.state.windows.set_window_server_space(wsid, Some(space1));
    let txid = reactor.transaction_manager.generate_next_txid(wsid);
    let target_frame = CGRect::new(CGPoint::new(100., 100.), CGSize::new(800., 600.));
    reactor.transaction_manager.store_txid(wsid, txid, target_frame);

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        space2,
        SpaceEventKind::User,
    );

    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space1));
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space1));
    assert_eq!(
        reactor.authoritative_space_for_window_id(wid),
        Some(space1),
        "late appearance from the old display should be ignored once Rift has already committed the new server-space target"
    );
}

#[test]
fn stale_user_space_appearance_is_ignored_when_authoritative_window_space_differs() {
    let (mut reactor, wid, wsid, space1, space2, _frame) = reactor_with_window_moved_to_space2();
    crate::sys::window_server::set_window_spaces_override(wsid, Some(vec![space2.get()]));

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        space1,
        SpaceEventKind::User,
    );

    crate::sys::window_server::set_window_spaces_override(wsid, None);

    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert_eq!(reactor.authoritative_space_for_window_id(wid), Some(space2));
}

#[test]
fn multi_active_visible_window_appearance_keeps_display_assignment_and_visibility() {
    let (mut reactor, wid, wsid, space1, space2, _frame) = reactor_with_window_moved_to_space2();

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        space1,
        SpaceEventKind::User,
    );

    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert_eq!(reactor.authoritative_space_for_window_id(wid), Some(space2));
    assert!(reactor.state.windows.is_window_visible(wsid));
}

#[test]
fn multi_active_visible_window_disappearance_does_not_reassign_between_display_spaces() {
    let (mut reactor, wid, wsid, space1, space2, _frame) = reactor_with_window_moved_to_space2();

    SpaceEventHandler::handle_window_server_destroyed(
        &mut reactor,
        SpaceEventHandler::WindowServerLifecyclePayload {
            window_server_id: wsid,
            space: space1,
            kind: SpaceEventKind::User,
        },
    )
    .unwrap();

    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert!(reactor.state.windows.is_window_visible(wsid));
}

#[test]
fn hidden_window_can_move_to_another_native_space_without_staying_pinned_to_old_display() {
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let pid = 1;
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));
    let right = CGRect::new(CGPoint::new(1440., 0.), CGSize::new(1440., 900.));
    let frame = CGRect::new(CGPoint::new(100., 100.), CGSize::new(800., 600.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new(121);

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(space1),
        Some(space2),
    ]));

    let (app_tx, _app_rx) = crate::actor::channel();
    reactor.app_manager.apps.insert(pid, AppState {
        info: AppInfo {
            bundle_id: Some("com.test.app".to_string()),
            localized_name: Some("Test App".to_string()),
        },
        handle: AppThreadHandle::new_for_test(app_tx),
    });

    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space1)
        .to_vec();
    let hidden_workspace = workspaces[0].0;
    let visible_workspace = workspaces[1].0;
    reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space2);

    reactor.state.windows.track_window_server_id(wsid, wid);
    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame,
            min_frame: frame.size,
            max_frame: frame.size,
        });
    reactor.state.windows.set_window_server_space(wsid, Some(space1));
    reactor.state.windows.mark_window_visible(wsid);
    reactor.state.windows.insert_window(wid, WindowState {
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
        ignore_app_rule: false,
        is_manageable: true,
    });

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .set_active_workspace(space1, visible_workspace)
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space1, wid, hidden_workspace)
    );
    assert_eq!(reactor.hidden_assigned_space_for_window_id(wid), Some(space1));

    crate::sys::window_server::set_window_spaces_override(wsid, Some(vec![space2.get()]));
    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        space2,
        SpaceEventKind::User,
    );
    crate::sys::window_server::set_window_spaces_override(wsid, None);

    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert_eq!(reactor.authoritative_space_for_window_id(wid), Some(space2));
}

#[test]
fn discovery_prefers_authoritative_space_over_geometry_when_displays_overlap_workspaces() {
    let (mut reactor, wid, wsid, space1, space2, _moved_frame) =
        reactor_with_window_moved_to_space2();
    let conflicting_frame = CGRect::new(CGPoint::new(100., 100.), CGSize::new(800., 600.));

    reactor
        .state
        .windows
        .window_mut(wid)
        .expect("window should exist")
        .frame_monotonic = conflicting_frame;
    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid: wid.pid,
            layer: 0,
            frame: conflicting_frame,
            min_frame: conflicting_frame.size,
            max_frame: conflicting_frame.size,
        });

    assert_eq!(
        reactor.discovery_space_for_window_id(wid),
        Some(space2),
        "discovery should stay in the authoritative native space instead of hopping to another display's geometry"
    );
    assert_ne!(
        reactor.discovery_space_for_window_id(wid),
        Some(space1),
        "same-index workspaces on other displays must stay isolated"
    );
}

#[test]
fn recent_cross_display_move_ignores_conflicting_geometry_space_change() {
    let (mut reactor, wid, wsid, _space1, space2, _) = reactor_with_window_moved_to_space2();
    let conflicting_frame = CGRect::new(CGPoint::new(100., 100.), CGSize::new(800., 600.));

    reactor.handle_event(Event::WindowFrameChanged(
        wid,
        conflicting_frame,
        None,
        Requested(false),
        Some(MouseState::Up),
    ));

    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
}

#[test]
fn central_space_resolution_prefers_recent_move_target_over_stale_server_space() {
    let (mut reactor, wid, wsid, space1, space2, moved_frame) =
        reactor_with_window_moved_to_space2();

    reactor.state.windows.set_window_server_space(wsid, Some(space1));

    assert_eq!(reactor.authoritative_space_for_window_id(wid), Some(space2));
    assert_eq!(
        reactor.best_space_for_window(&moved_frame, Some(wsid)),
        Some(space2),
        "core space resolution should prefer the recent move target when geometry and assignment agree"
    );
}

#[test]
fn active_space_membership_refresh_does_not_overwrite_recent_move_target() {
    let (mut reactor, wid, wsid, space1, space2, _) = reactor_with_window_moved_to_space2();

    reactor.refresh_active_space_window_membership(vec![(wsid, Some(space1))]);

    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space2));
    assert_eq!(
        reactor.state.windows.window_server_space(wsid),
        Some(space2),
        "active-space reconciliation must not overwrite a recent cross-display move with stale membership"
    );
    assert!(reactor.state.windows.is_window_visible(wsid));
}

#[test]
fn known_fullscreen_window_appearance_removes_window_from_layout() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(user_space)]));
    reactor.handle_events(apps.make_app_with_opts(1, make_windows(1), Some(wid), true, true));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, user_space, frame, wid));
    let wsid = reactor.state.windows.window(wid).unwrap().info.sys_id.unwrap();

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        fullscreen_space,
        SpaceEventKind::Fullscreen,
    );

    assert!(
        !has_window_in_layout(&mut reactor, user_space, frame, wid),
        "managed window should be removed from layout when it enters native fullscreen"
    );
    assert!(
        reactor
            .state
            .windows
            .native_fullscreen_record_for_window_server_id(wsid)
            .is_some_and(|record| record.fullscreen_space == fullscreen_space),
        "fullscreen transition should record suspended window state"
    );
}

#[test]
fn known_window_server_appearance_restores_same_workspace_after_fullscreen() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(user_space)]));
    reactor.handle_events(apps.make_app_with_opts(1, make_windows(1), Some(wid), true, true));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    let wsid = reactor.state.windows.window(wid).unwrap().info.sys_id.unwrap();
    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        fullscreen_space,
        SpaceEventKind::Fullscreen,
    );
    assert!(!has_window_in_layout(&mut reactor, user_space, frame, wid));

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        user_space,
        SpaceEventKind::User,
    );

    assert!(
        has_window_in_layout(&mut reactor, user_space, frame, wid),
        "managed window should return to layout when native fullscreen exits back to the same space"
    );
}

#[test]
fn fullscreen_tracking_survives_until_ax_window_id_arrives() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let pid: pid_t = 61;
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 1);
    let frame = CGRect::new(CGPoint::new(50., 50.), CGSize::new(900., 700.));

    reactor.handle_event(space_state_event(vec![screen], vec![Some(user_space)]));

    let (app_tx, mut app_rx) = crate::actor::channel();
    reactor.app_manager.apps.insert(pid, AppState {
        info: AppInfo {
            bundle_id: Some("com.test.pending-fullscreen".to_string()),
            localized_name: Some("Pending Fullscreen".to_string()),
        },
        handle: AppThreadHandle::new_for_test(app_tx),
    });

    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame,
            min_frame: frame.size,
            max_frame: frame.size,
        });

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        fullscreen_space,
        SpaceEventKind::Fullscreen,
    );

    assert!(
        reactor
            .state
            .windows
            .pending_native_fullscreen_record_for_window_server_id(wsid)
            .is_some_and(|record| {
                record.pid == pid
                    && record.last_known_user_space == Some(user_space)
                    && record.fullscreen_space == fullscreen_space
            }),
        "fullscreen lifecycle should be retained by wsid until AX tracking binds the window"
    );
    assert!(
        matches!(app_rx.try_recv(), Ok((_, Request::GetVisibleWindows))),
        "fullscreen appearance without AX tracking should still request a visible-window refresh"
    );

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        user_space,
        SpaceEventKind::User,
    );

    assert!(
        matches!(app_rx.try_recv(), Ok((_, Request::GetVisibleWindows))),
        "fullscreen exit without AX tracking should request a visible-window refresh"
    );

    reactor.handle_event(Event::WindowsDiscovered {
        pid,
        new: vec![(wid, WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Recovered Window".to_string(),
            frame,
            sys_id: Some(wsid),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        })],
        known_visible: vec![wid],
    });

    assert!(
        reactor
            .state
            .windows
            .pending_native_fullscreen_record_for_window_server_id(wsid)
            .is_none(),
        "binding the AX window id should consume the pending fullscreen record"
    );
    assert!(
        reactor.state.windows.native_fullscreen_record_for_window(wid).is_none(),
        "once the window is back on its user space, the fullscreen lifecycle should retire"
    );
    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(user_space));
}

#[test]
fn fullscreen_does_not_suppress_other_same_pid_windows() {
    let (mut reactor, original_wid, original_wsid, user_space, _other_space, frame) =
        reactor_with_window_on_space1();
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let second_wid = WindowId::new(original_wid.pid, 1002);
    let second_wsid = WindowServerId::new(10002);

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        original_wsid,
        fullscreen_space,
        SpaceEventKind::Fullscreen,
    );

    reactor.handle_event(Event::WindowCreated(
        second_wid,
        WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Second Window".to_string(),
            frame,
            sys_id: Some(second_wsid),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        Some(crate::sys::window_server::WindowServerInfo {
            id: second_wsid,
            pid: original_wid.pid,
            layer: 0,
            frame,
            min_frame: frame.size,
            max_frame: frame.size,
        }),
        None,
    ));

    assert_eq!(
        reactor.assigned_space_for_window_id(second_wid),
        Some(user_space)
    );
}

#[test]
fn fullscreen_exit_removes_non_queryable_duplicate_from_layout() {
    let (mut reactor, original_wid, original_wsid, user_space, other_space, frame) =
        reactor_with_window_on_space1();
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let duplicate_wid = WindowId::new(original_wid.pid, 27481);
    let duplicate_wsid = WindowServerId::new(27481);
    let active_workspace = reactor
        .layout_manager
        .layout_engine
        .active_workspace(user_space)
        .expect("active workspace");

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        original_wsid,
        fullscreen_space,
        SpaceEventKind::Fullscreen,
    );

    reactor.state.windows.track_window_server_id(duplicate_wsid, duplicate_wid);
    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: duplicate_wsid,
            pid: original_wid.pid,
            layer: 0,
            frame,
            min_frame: frame.size,
            max_frame: frame.size,
        });
    reactor
        .state
        .windows
        .set_window_server_space(duplicate_wsid, Some(fullscreen_space));
    reactor.state.windows.mark_window_visible(duplicate_wsid);
    reactor.state.windows.insert_window(duplicate_wid, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Electron fullscreen projection".to_string(),
            frame,
            sys_id: Some(duplicate_wsid),
            bundle_id: None,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: false,
        ignore_app_rule: false,
    });

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        duplicate_wsid,
        fullscreen_space,
        SpaceEventKind::Fullscreen,
    );

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                user_space,
                duplicate_wid,
                active_workspace
            )
    );
    reactor.send_layout_event(LayoutEvent::WindowAdded(user_space, duplicate_wid));
    assert!(has_window_in_layout(
        &mut reactor,
        user_space,
        frame,
        duplicate_wid
    ));
    assert!(
        reactor.create_window_data(duplicate_wid).is_none(),
        "duplicate is absent from query windows because it is not manageable"
    );

    reactor.state.windows.set_window_server_space(duplicate_wsid, Some(user_space));
    reactor.state.windows.mark_window_visible(duplicate_wsid);
    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        duplicate_wsid,
        user_space,
        SpaceEventKind::User,
    );

    assert!(
        !has_window_in_layout(&mut reactor, user_space, frame, duplicate_wid),
        "fullscreen restore must evict non-queryable duplicate layout ghosts"
    );
    assert_eq!(reactor.assigned_space_for_window_id(duplicate_wid), None);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(other_space)]));
    assert_eq!(reactor.assigned_space_for_window_id(duplicate_wid), None);
    reactor.handle_event(space_state_event(vec![frame], vec![Some(user_space)]));
    assert_eq!(reactor.assigned_space_for_window_id(duplicate_wid), None);
    assert!(
        !has_window_in_layout(&mut reactor, user_space, frame, duplicate_wid),
        "ghost must not reappear when switching back to the original space"
    );
}

#[test]
fn fullscreen_restore_uses_live_rekeyed_window_id() {
    let (mut reactor, old_wid, wsid, user_space, _other_space, frame) =
        reactor_with_window_on_space1();
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let new_wid = WindowId::new(old_wid.pid, 1999);

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        fullscreen_space,
        SpaceEventKind::Fullscreen,
    );

    let old_info = reactor
        .state
        .windows
        .window(old_wid)
        .expect("old window should still exist while fullscreen is active")
        .info
        .clone();

    reactor.handle_event(Event::WindowsDiscovered {
        pid: old_wid.pid,
        new: vec![(new_wid, WindowInfo {
            sys_id: old_info.sys_id,
            ..old_info
        })],
        known_visible: vec![new_wid],
    });

    assert!(
        reactor.state.windows.window(old_wid).is_none(),
        "rekey should retire the old AX window id before fullscreen restore"
    );

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        user_space,
        SpaceEventKind::User,
    );

    assert!(has_window_in_layout(&mut reactor, user_space, frame, new_wid));
    assert!(!has_window_in_layout(&mut reactor, user_space, frame, old_wid));
}

#[test]
fn known_window_server_appearance_restores_layout_membership_without_reassignment() {
    let (mut reactor, wid, wsid, user_space, _other_space, frame) = reactor_with_window_on_space1();

    reactor.send_layout_event(LayoutEvent::WindowAdded(user_space, wid));
    assert!(has_window_in_layout(&mut reactor, user_space, frame, wid));

    reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));

    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(user_space));
    assert!(
        !has_window_in_layout(&mut reactor, user_space, frame, wid),
        "temporary removal should clear active layout membership before the appearance event"
    );

    SpaceEventHandler::handle_window_server_appeared(
        &mut reactor,
        wsid,
        user_space,
        SpaceEventKind::User,
    );

    assert!(
        has_window_in_layout(&mut reactor, user_space, frame, wid),
        "same-space appearance should heal active layout membership even when workspace assignment already matches"
    );
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

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space1)]));
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
        reactor.state.windows.track_window_server_id(wsid, wid);
        reactor.state.windows.set_window_server_space(wsid, Some(space));
        reactor.state.windows.insert_window(wid, WindowState {
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

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space1,
                WindowId::new(pid, 1),
                space1_workspace
            )
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space1,
                WindowId::new(pid, 2),
                space1_workspace
            )
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space2,
                WindowId::new(pid, 3),
                space2_workspace
            )
    );

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space2)]));
    reactor.state.windows.clear_visible_windows();
    reactor.state.windows.mark_window_visible(WindowServerId::new(103));
    reactor.mission_control_manager.pending_mission_control_refresh.insert(pid);

    reactor.on_windows_discovered_with_app_info(pid, vec![], vec![WindowId::new(pid, 3)], None);

    let space1_workspaces = reactor.query_workspaces(Some(space1));
    let space2_workspaces = reactor.query_workspaces(Some(space2));
    let space1_count: usize = space1_workspaces.iter().map(|ws| ws.window_count).sum();
    let space2_count: usize = space2_workspaces.iter().map(|ws| ws.window_count).sum();

    assert_eq!(
        space1_count, 2,
        "inactive native space windows must stay on space1"
    );
    assert_eq!(
        space2_count, 1,
        "only the visible window should belong to space2"
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space1, WindowId::new(pid, 1))
            .is_some()
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space1, WindowId::new(pid, 2))
            .is_some()
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space2, WindowId::new(pid, 1))
            .is_none()
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space2, WindowId::new(pid, 2))
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(old_space)]));
    reactor.handle_event(Event::MissionControlNativeEntered);
    reactor.handle_event(space_state_event(vec![screen], vec![Some(new_space)]));

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
fn mission_control_exit_does_not_restore_cached_space_without_authoritative_snapshot() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let stale_space = SpaceId::new(1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(stale_space)]));
    reactor.handle_event(Event::MissionControlNativeEntered);
    reactor.handle_event(space_state_event(vec![screen], vec![None]));
    reactor.handle_event(Event::MissionControlNativeExited);

    assert_eq!(reactor.workspace_command_space(), None);
    assert_eq!(reactor.space_state.screens[0].space, None);
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(pid, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, space, screen, moved));
    assert!(has_window_in_layout(&mut reactor, space, screen, retained));

    apps.windows.remove(&moved);
    let retained_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 2);
    reactor.refresh_windows_after_mission_control_with_active_windows(vec![(
        retained_wsid,
        Some(space),
    )]);
    apps.simulate_until_quiet(&mut reactor);

    assert!(
        !has_window_in_layout(&mut reactor, space, screen, moved),
        "window moved to another native space during Mission Control should be removed from the origin layout immediately"
    );
    assert!(has_window_in_layout(&mut reactor, space, screen, retained));
}

#[test]
fn mission_control_refresh_known_visible_fallback_does_not_restore_window_moved_to_other_space() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let pid: pid_t = 45;
    let moved = WindowId::new(pid, 1);
    let retained = WindowId::new(pid, 2);
    let retained_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(pid, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    let _ = reactor.layout_manager.layout_engine.handle_virtual_workspace_command(
        &mut reactor.state.windows,
        space,
        &LayoutCommand::CreateWorkspace,
    );

    reactor.refresh_windows_after_mission_control_with_active_windows(vec![(
        retained_wsid,
        Some(space),
    )]);
    apps.simulate_until_quiet(&mut reactor);

    assert!(
        !has_window_in_layout(&mut reactor, space, screen, moved),
        "known_visible fallback must not recreate a layout ghost for a window missing from the authoritative active-space snapshot"
    );

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::SwitchToWorkspace(1),
    )));
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::SwitchToWorkspace(0),
    )));

    assert!(
        !has_window_in_layout(&mut reactor, space, screen, moved),
        "workspace switching must not re-project a window that Mission Control moved to another native space"
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.state.windows.insert_window(wid, WindowState {
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

    assert!(matches!(
        reactor.drag_manager.drag_state,
        DragState::Active { .. }
    ));

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
    reactor.handle_event(space_state_event(vec![full_screen], vec![None]));

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
    reactor.handle_event(space_state_event(vec![screen1, screen2], vec![
        Some(SpaceId::new(1)),
        Some(SpaceId::new(2)),
    ]));

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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(SpaceId::new(1))]));

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
    reactor.handle_event(space_state_event(vec![screen1, screen2], vec![
        Some(SpaceId::new(1)),
        Some(SpaceId::new(2)),
    ]));

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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
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
fn topology_change_clears_stale_pending_hide_target_before_next_workspace_layout() {
    let mut apps = Apps::new();
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    let _ = apps.requests();

    let wsid = reactor
        .state
        .windows
        .window(wid)
        .and_then(|window| window.info.sys_id)
        .expect("tracked window should have a window server id");
    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space)
        .to_vec();
    let hidden_workspace = workspaces[0].0;
    let active_workspace = workspaces[1].0;

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .set_active_workspace(space, active_workspace)
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space, wid, hidden_workspace)
    );

    if let Some(window) = reactor.state.windows.window_mut(wid) {
        window.frame_monotonic = CGRect::new(CGPoint::new(200.0, 200.0), CGSize::new(400.0, 400.0));
    }

    let gaps = reactor.config.settings.layout.gaps.clone();
    let hidden_target = reactor
        .layout_manager
        .layout_engine
        .calculate_layout_with_virtual_workspaces(
            &reactor.state.windows,
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
            |query_wid| {
                reactor.state.windows.window(query_wid).map(|window| window.frame_monotonic)
            },
            &[screen],
        )
        .into_iter()
        .find(|(layout_wid, _)| *layout_wid == wid)
        .map(|(_, frame)| frame)
        .expect("inactive-workspace window should still be laid out to a hidden position");

    let txid = reactor.transaction_manager.generate_next_txid(wsid);
    reactor.transaction_manager.store_txid(wsid, txid, hidden_target);

    assert!(!reactor.update_layout_or_warn(false, true));
    assert!(
        apps.requests().is_empty(),
        "a stale pending target suppresses the hide write before topology invalidation"
    );

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen], vec![Some(space)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [space].into_iter().collect(),
        menu_bar_space: Some(space),
        command_space: Some(space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: true,
        topology_changed: true,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));
    let requests = apps.requests();
    assert!(
        requests.iter().any(|req| {
            matches!(req,
                Request::SetWindowFrame(req_wid, frame, _, true)
                    if *req_wid == wid && frame.same_as(hidden_target)
            ) || matches!(req,
                Request::SetBatchWindowFrame(frames, _, true)
                    if frames.iter().any(|(req_wid, frame)| *req_wid == wid && frame.same_as(hidden_target))
            )
        }),
        "topology invalidation must resend the hidden-window frame write instead of treating the stale target as still pending: {requests:?}"
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
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
        reactor
            .layout_manager
            .layout_engine
            .windows_in_active_workspace(&reactor.state.windows, space),
        vec![WindowId::new(1, 2)],
    );
}

#[test]
fn workspace_query_uses_authoritative_assignment_after_move() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::CreateWorkspace)));
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::MoveWindowToWorkspace {
            workspace: 1,
            window_id: Some(wid.idx.get()),
        },
    )));
    apps.simulate_until_quiet(&mut reactor);

    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space)
        .to_vec();
    let ws1 = workspaces[0].0;
    let ws2 = workspaces[1].0;

    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, wid),
        Some(ws2)
    );

    let queried = reactor.query_workspaces(Some(space));
    assert_eq!(queried[0].window_count, 0);
    assert_eq!(queried[1].window_count, 1);
    assert_eq!(queried[1].windows[0].id, wid);
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_windows(&reactor.state.windows, space, ws1),
        Vec::<WindowId>::new()
    );
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_windows(&reactor.state.windows, space, ws2),
        vec![wid]
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));

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

    reactor.handle_event(space_state_event(vec![CGRect::ZERO], vec![None]));
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));
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
fn login_screen_refresh_preserves_manual_workspace_assignment() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let space = SpaceId::new(1);
    let full_screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let wid1 = WindowId::new(1, 1);
    let wid2 = WindowId::new(1, 2);

    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app_with_opts(1, make_windows(2), Some(wid1), true, true));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
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

    let workspace_before = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager()
        .workspace_for_window(&reactor.state.windows, space, wid2)
        .expect("window should be assigned to workspace 2 before login refresh");
    let other_workspace_before = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager()
        .workspace_for_window(&reactor.state.windows, space, wid1)
        .expect("window should remain assigned to original workspace before login refresh");
    assert_ne!(workspace_before, other_workspace_before);
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .windows_in_active_workspace(&reactor.state.windows, space),
        vec![wid2],
        "switched workspace should show only the moved window before login refresh"
    );

    reactor.handle_event(space_state_event(vec![CGRect::ZERO], vec![None]));
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));
    let requests = apps.requests();
    for request in requests {
        match request {
            Request::GetVisibleWindows => {
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
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, wid2),
        Some(workspace_before),
        "login refresh must preserve the moved window's workspace assignment"
    );
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, wid1),
        Some(other_workspace_before),
        "login refresh must preserve other windows' original workspace assignments"
    );
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .windows_in_active_workspace(&reactor.state.windows, space),
        vec![wid2],
        "active workspace contents must survive login refresh"
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));

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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));

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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));

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

    reactor.handle_event(space_state_event(vec![full_screen], vec![None]));

    // Simulate a native fullscreen transition: space temporarily becomes a fullscreen
    // space id (reactor suppresses it to None), then returns to the original space.
    let fullscreen_space = SpaceId::new(0x400000000 + space.get());
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(
        fullscreen_space,
    )]));

    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(space)]));

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
    reactor.handle_event(space_state_event(vec![right, left], vec![
        Some(SpaceId::new(1)),
        Some(SpaceId::new(2)),
    ]));

    let selected = reactor
        .screen_for_selector(&DisplaySelector::Index(0), None)
        .expect("expected display index 0 to resolve");

    assert_eq!(selected.frame, left);
}

#[test]
fn moving_tiled_window_to_display_applies_destination_layout_after_transfer_frame() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(SpaceId::new(1)),
        Some(SpaceId::new(2)),
    ]));
    reactor.handle_events(apps.make_app(1, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    let moved = WindowId::new(1, 1);
    reactor.handle_event(Event::Command(Command::Reactor(
        ReactorCommand::MoveWindowToDisplay {
            selector: DisplaySelector::Index(1),
            window_id: Some(1),
        },
    )));

    let writes: Vec<CGRect> = apps
        .requests()
        .into_iter()
        .flat_map(|request| match request {
            Request::SetWindowFrame(wid, frame, _, _) if wid == moved => vec![frame],
            Request::SetBatchWindowFrame(frames, _, _) => frames
                .into_iter()
                .filter_map(|(wid, frame)| (wid == moved).then_some(frame))
                .collect(),
            _ => Vec::new(),
        })
        .collect();

    assert!(
        writes.len() >= 2,
        "expected transfer and tiled writes: {writes:?}"
    );
    assert!(
        writes.last().is_some_and(|frame| frame.same_as(right)),
        "the destination layout must supply the final frame: {writes:?}"
    );
    assert!(
        !writes.first().is_some_and(|frame| frame.same_as(right)),
        "the initial transfer frame should preserve the source tile size: {writes:?}"
    );
}

#[test]
fn authoritative_active_window_snapshot_reassigns_window_across_active_displays() {
    let (mut reactor, wid, wsid, space1, space2, _initial_frame, _screen2) =
        reactor_with_window_on_space1_two_displays();

    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(space1));
    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space1));

    reactor.reconcile_authoritative_active_window_snapshot(vec![(wsid, Some(space2))], false);

    assert_eq!(
        reactor.state.windows.window_server_space(wsid),
        Some(space2),
        "authoritative active-space membership should update the tracked native space"
    );
    assert_eq!(
        reactor.assigned_space_for_window_id(wid),
        Some(space2),
        "authoritative active-space membership should reassign the window to the new display"
    );
}

#[test]
fn authoritative_active_window_snapshot_removes_missing_window_from_active_layout() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let pid: pid_t = 42;
    let moved = WindowId::new(pid, 1);
    let retained = WindowId::new(pid, 2);
    let moved_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 1);
    let retained_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 2);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(space)]));
    reactor.handle_events(apps.make_app(pid, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, space, frame, moved));
    assert!(has_window_in_layout(&mut reactor, space, frame, retained));
    reactor.state.windows.set_window_server_space(moved_wsid, Some(space));
    reactor.state.windows.mark_window_visible(moved_wsid);
    reactor.state.windows.set_window_server_space(retained_wsid, Some(space));
    reactor.state.windows.mark_window_visible(retained_wsid);
    reactor
        .reconcile_authoritative_active_window_snapshot(vec![(retained_wsid, Some(space))], false);

    assert!(
        !has_window_in_layout(&mut reactor, space, frame, moved),
        "active-space window missing from the authoritative snapshot must be removed immediately"
    );
    assert!(
        !reactor.state.windows.is_window_visible(moved_wsid),
        "authoritative snapshot reconcile should clear visible state for missing windows"
    );
    assert!(has_window_in_layout(&mut reactor, space, frame, retained));
}

#[test]
fn authoritative_active_window_snapshot_reassigns_missing_window_to_inactive_space() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let active_space = SpaceId::new(1);
    let inactive_space = SpaceId::new(2);
    let pid: pid_t = 43;
    let moved = WindowId::new(pid, 1);
    let retained = WindowId::new(pid, 2);
    let moved_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 1);
    let retained_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 2);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(active_space)]));
    reactor.handle_events(apps.make_app(pid, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    reactor.state.windows.set_window_server_space(moved_wsid, Some(active_space));
    reactor.state.windows.mark_window_visible(moved_wsid);
    reactor.state.windows.set_window_server_space(retained_wsid, Some(active_space));
    reactor.state.windows.mark_window_visible(retained_wsid);
    crate::sys::window_server::set_window_spaces_override(
        moved_wsid,
        Some(vec![inactive_space.get()]),
    );

    reactor.reconcile_authoritative_active_window_snapshot(
        vec![(retained_wsid, Some(active_space))],
        false,
    );

    crate::sys::window_server::set_window_spaces_override(moved_wsid, None);

    assert_eq!(
        reactor.assigned_space_for_window_id(moved),
        Some(inactive_space),
        "missing active-space windows should migrate to their actual inactive native space"
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, active_space, moved)
            .is_none(),
        "window should no longer belong to the old active native space"
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, inactive_space, moved)
            .is_some(),
        "window should now belong to the inactive native space that WindowServer reports"
    );
    assert!(
        !has_window_in_layout(&mut reactor, active_space, frame, moved),
        "window moved onto an inactive native space must be removed from the active layout"
    );
    assert!(has_window_in_layout(&mut reactor, active_space, frame, retained));
    assert_eq!(
        reactor.assigned_space_for_window_id(retained),
        Some(active_space),
        "other visible windows on the active space must remain untouched"
    );
}

#[test]
fn topology_window_delta_reassigns_missing_window_to_inactive_space() {
    let mut apps = Apps::new();
    let mut workspace_settings = crate::common::config::VirtualWorkspaceSettings::default();
    workspace_settings.default_workspace_count = 3;
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_settings,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let active_space = SpaceId::new(1);
    let inactive_space = SpaceId::new(2);
    let pid: pid_t = 44;
    let moved = WindowId::new(pid, 1);
    let retained = WindowId::new(pid, 2);
    let moved_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 1);
    let retained_wsid = WindowServerId::new((pid as u32).saturating_mul(10_000) + 2);

    reactor.handle_event(space_state_event(vec![frame], vec![Some(active_space)]));
    reactor.handle_events(apps.make_app(pid, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    let preserved_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(active_space)[2]
        .0;
    let expected_destination_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(inactive_space)[2]
        .0;
    reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(moved));
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                active_space,
                moved,
                preserved_workspace
            )
    );
    let _ = reactor.layout_manager.layout_engine.handle_virtual_workspace_command(
        &mut reactor.state.windows,
        active_space,
        &LayoutCommand::SwitchToWorkspace(2),
    );
    reactor.send_layout_event(LayoutEvent::WindowAdded(active_space, moved));
    let _ = reactor.layout_manager.layout_engine.handle_virtual_workspace_command(
        &mut reactor.state.windows,
        active_space,
        &LayoutCommand::SwitchToWorkspace(0),
    );

    reactor.state.windows.set_window_server_space(moved_wsid, Some(active_space));
    reactor.state.windows.mark_window_visible(moved_wsid);
    reactor.state.windows.set_window_server_space(retained_wsid, Some(active_space));
    reactor.state.windows.mark_window_visible(retained_wsid);
    crate::sys::window_server::set_window_spaces_override(
        moved_wsid,
        Some(vec![inactive_space.get()]),
    );
    crate::sys::window_server::set_space_window_list_for_space_override(
        active_space.get(),
        Some(vec![retained_wsid.as_u32()]),
    );

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![frame], vec![Some(active_space)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [active_space].into_iter().collect(),
        menu_bar_space: Some(active_space),
        command_space: Some(active_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: Some(crate::actor::spaces::TopologyWindowDelta {
            epoch: 11,
            flags: crate::sys::skylight::DisplayReconfigFlags::MOVED,
            appeared: Vec::new(),
            disappeared: vec![(moved_wsid, active_space)],
        }),
        active_window_spaces: Default::default(),
    }));

    crate::sys::window_server::set_window_spaces_override(moved_wsid, None);
    crate::sys::window_server::set_space_window_list_for_space_override(active_space.get(), None);

    assert_eq!(reactor.assigned_space_for_window_id(moved), Some(inactive_space));
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, active_space, moved)
            .is_none()
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, inactive_space, moved)
            .is_some_and(|workspace| workspace == expected_destination_workspace)
    );
    assert!(!has_window_in_layout(&mut reactor, active_space, frame, moved));
    assert!(has_window_in_layout(&mut reactor, active_space, frame, retained));
}

#[test]
fn topology_window_delta_is_not_ignored_by_command_space_only_short_circuit() {
    let (mut reactor, wid, wsid, space1, space2, _initial_frame, screen2) =
        reactor_with_window_on_space1_two_displays();
    let screen1 = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1440., 900.));

    crate::sys::window_server::set_window_spaces_override(wsid, Some(vec![space2.get()]));
    crate::sys::window_server::set_space_window_list_for_space_override(space1.get(), Some(vec![]));
    crate::sys::window_server::set_space_window_list_for_space_override(
        space2.get(),
        Some(vec![wsid.as_u32()]),
    );

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![screen1, screen2], vec![Some(space1), Some(space2)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [space1, space2].into_iter().collect(),
        menu_bar_space: Some(space1),
        command_space: Some(space1),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: Some(crate::actor::spaces::TopologyWindowDelta {
            epoch: 12,
            flags: crate::sys::skylight::DisplayReconfigFlags::MOVED,
            appeared: vec![(wsid, space2)],
            disappeared: vec![(wsid, space1)],
        }),
        active_window_spaces: Default::default(),
    }));

    crate::sys::window_server::set_window_spaces_override(wsid, None);
    crate::sys::window_server::set_space_window_list_for_space_override(space1.get(), None);
    crate::sys::window_server::set_space_window_list_for_space_override(space2.get(), None);

    assert_eq!(
        reactor.assigned_space_for_window_id(wid),
        Some(space2),
        "topology delta should still be processed even when the forwarded screens snapshot is unchanged"
    );
    assert_eq!(reactor.state.windows.window_server_space(wsid), Some(space2));
}

#[test]
fn forwarded_space_state_does_not_clear_existing_fullscreen_tracks_when_snapshot_has_none() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let tracked_user_space = SpaceId::new(1);
    let current_space = SpaceId::new(2);
    let fullscreen_space = SpaceId::new(0x400000001);
    let window_id = WindowId::new(42, 1);

    let tracked_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(tracked_user_space)
        .first()
        .map(|(id, _)| *id)
        .expect("tracked workspace");
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                tracked_user_space,
                window_id,
                tracked_workspace
            )
    );
    let _ = reactor.state.windows.suspend_window_to_native_fullscreen(
        window_id,
        Some(WindowServerId::new(1)),
        Some(tracked_user_space),
        fullscreen_space,
        NativeFullscreenTransition::Suspended,
    );

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: make_screen_snapshots(vec![frame], vec![Some(current_space)]),
        fullscreen_spaces: Default::default(),
        has_seen_display_set: true,
        active_spaces: [current_space].into_iter().collect(),
        menu_bar_space: Some(current_space),
        command_space: Some(current_space),
        display_space_ids: Default::default(),
        last_user_space_by_display: Default::default(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    assert!(
        reactor
            .state
            .windows
            .native_fullscreen_record_for_window(window_id)
            .is_some_and(|record| record.fullscreen_space == fullscreen_space),
        "empty forwarded fullscreen state must not clear existing fullscreen exit tracking"
    );
}

#[test]
fn non_active_workspace_windows_remain_hidden_even_if_frame_no_longer_matches_corner_geometry() {
    let mut apps = Apps::new();
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    let wsid = reactor
        .state
        .windows
        .window(wid)
        .and_then(|window| window.info.sys_id)
        .expect("tracked window should have a window server id");
    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space);
    let inactive_workspace = workspaces[0].0;
    let active_workspace = workspaces[1].0;

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .set_active_workspace(space, active_workspace)
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(&mut reactor.state.windows, space, wid, inactive_workspace)
    );

    if let Some(window) = reactor.state.windows.window_mut(wid) {
        window.frame_monotonic = CGRect::new(CGPoint::new(200.0, 200.0), CGSize::new(400.0, 400.0));
    }

    assert_eq!(
        reactor.hidden_assigned_space_for_window_id(wid),
        Some(space),
        "workspace-hidden status should follow Rift's workspace assignment, not stale corner geometry"
    );
    assert_eq!(
        reactor.geometry_space_for_window(
            &CGRect::new(CGPoint::new(200.0, 200.0), CGSize::new(400.0, 400.0)),
            Some(wsid),
        ),
        Some(space),
        "topology changes can leave hidden windows at stale coordinates; they must still resolve to their assigned space"
    );
}

#[test]
fn display_churn_quarantines_window_frame_and_membership_events() {
    let reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let space = SpaceId::new(7);
    let wsid = WindowServerId::new(77);
    let _ = crate::sys::display_churn::begin(crate::sys::skylight::DisplayReconfigFlags::ADD);

    let frame_changed = reactor.should_quarantine_during_display_churn(&Event::WindowFrameChanged(
        WindowId::new(99, 1),
        CGRect::new(CGPoint::new(10., 10.), CGSize::new(500., 400.)),
        None,
        Requested(false),
        Some(MouseState::Up),
    ));
    let appeared = reactor.should_quarantine_during_display_churn(&Event::WindowServerAppeared(
        wsid,
        space,
        SpaceEventKind::User,
    ));
    let destroyed = reactor.should_quarantine_during_display_churn(&Event::WindowServerDestroyed(
        wsid,
        space,
        SpaceEventKind::User,
    ));
    let ax_invalidated = reactor
        .should_quarantine_during_display_churn(&Event::WindowDestroyed(WindowId::new(99, 77)));
    let space_created = reactor.should_quarantine_during_display_churn(&Event::SpaceCreated(space));
    let space_destroyed =
        reactor.should_quarantine_during_display_churn(&Event::SpaceDestroyed(space));

    let _ = crate::sys::display_churn::end();
    assert!(
        frame_changed,
        "WindowFrameChanged should be quarantined during churn"
    );
    assert!(
        appeared,
        "WindowServerAppeared should be quarantined during churn"
    );
    assert!(
        destroyed,
        "WindowServerDestroyed should be quarantined during churn"
    );
    assert!(
        !ax_invalidated,
        "AX invalidation must detach live state instead of being dropped during churn"
    );
    assert!(space_created, "SpaceCreated should be quarantined during churn");
    assert!(
        space_destroyed,
        "SpaceDestroyed should be quarantined during churn"
    );
}

#[test]
fn lifecycle_events_are_quarantined_during_sleep_and_session_inactivity() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let space = SpaceId::new(8);

    reactor.refresh_quarantine_manager.sleeping = true;
    assert!(reactor.should_quarantine_space_lifecycle_event(&Event::SpaceCreated(space)));

    reactor.refresh_quarantine_manager.sleeping = false;
    reactor.refresh_quarantine_manager.session_inactive = true;
    assert!(reactor.should_quarantine_space_lifecycle_event(&Event::SpaceDestroyed(space)));
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

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(SpaceId::new(11)),
        Some(SpaceId::new(22)),
    ]));
    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(SpaceId::new(111)),
        Some(SpaceId::new(222)),
    ]));
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

    reactor.space_state.fullscreen_spaces.insert(fullscreen_space);
    reactor.handle_event(space_state_event_from_screens(
        screens_for(user_space)
            .into_iter()
            .map(|mut screen| {
                screen.space = None;
                screen
            })
            .collect(),
    ));
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
fn fullscreen_transition_preserves_other_display_space() {
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

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space_2),
        Some(right_space_1),
    ]));
    reactor.space_state.fullscreen_spaces.insert(right_fullscreen);

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space_2),
        None,
    ]));

    assert_eq!(
        reactor.raw_spaces_for_current_screens(),
        vec![Some(left_space_2), None],
        "fullscreen transitions on one display must not accept a transient user-space change on another display"
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

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space_2),
        Some(right_space_1),
    ]));
    reactor.space_state.fullscreen_spaces.insert(right_fullscreen);
    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space_2),
        None,
    ]));

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(left_space_1),
        None,
    ]));

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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(user_space)]));
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
    reactor.space_state.fullscreen_spaces.insert(fullscreen_space);
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(user_space)]));
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

#[test]
fn fullscreen_startup_applies_app_rules_to_hidden_user_space_windows() {
    let mut workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    workspace_cfg.app_rules = vec![crate::common::config::AppWorkspaceRule {
        app_id: Some("com.testapp1".to_string()),
        workspace: Some(crate::common::config::WorkspaceSelector::Index(1)),
        floating: false,
        manage: true,
        app_name: None,
        title_regex: None,
        title_substring: None,
        ax_role: None,
        ax_subrole: None,
    }];
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let pid = 1;
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new(10_001);
    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let display_uuid = "test-display-0".to_string();

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: vec![ScreenInfo {
            id: crate::sys::screen::ScreenId::new(0),
            frame: screen,
            space: None,
            display_uuid: display_uuid.clone(),
            name: None,
        }],
        fullscreen_spaces: [fullscreen_space].into_iter().collect(),
        has_seen_display_set: true,
        active_spaces: Default::default(),
        menu_bar_space: None,
        command_space: None,
        display_space_ids: Default::default(),
        last_user_space_by_display: [(display_uuid, user_space)].into_iter().collect(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    let (app_tx, _app_rx) = crate::actor::channel();
    reactor.app_manager.apps.insert(pid, AppState {
        info: AppInfo {
            bundle_id: Some("com.testapp1".to_string()),
            localized_name: Some("TestApp1".to_string()),
        },
        handle: AppThreadHandle::new_for_test(app_tx),
    });

    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame: screen,
            min_frame: screen.size,
            max_frame: screen.size,
        });
    reactor.state.windows.set_window_server_space(wsid, Some(user_space));

    reactor.handle_event(Event::WindowsDiscovered {
        pid,
        new: vec![(wid, WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Window".to_string(),
            frame: screen,
            sys_id: Some(wsid),
            bundle_id: Some("com.testapp1".to_string()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        })],
        known_visible: vec![wid],
    });

    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(user_space)
        .to_vec();
    let target_workspace = workspaces[1].0;

    assert_eq!(reactor.assigned_space_for_window_id(wid), Some(user_space));
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, user_space, wid),
        Some(target_workspace),
        "fullscreen startup should still apply app rules to the hidden user-space window"
    );
}

#[test]
fn fullscreen_startup_discovery_preserves_existing_hidden_assignment_without_app_rules() {
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let pid = 1;
    let wid = WindowId::new(pid, 1);
    let wsid = WindowServerId::new(10_001);
    let user_space = SpaceId::new(1);
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let display_uuid = "test-display-0".to_string();

    reactor.handle_event(Event::SpaceStateChanged(ForwardedSpaceState {
        screens: vec![ScreenInfo {
            id: crate::sys::screen::ScreenId::new(0),
            frame: screen,
            space: None,
            display_uuid: display_uuid.clone(),
            name: None,
        }],
        fullscreen_spaces: [fullscreen_space].into_iter().collect(),
        has_seen_display_set: true,
        active_spaces: Default::default(),
        menu_bar_space: None,
        command_space: None,
        display_space_ids: Default::default(),
        last_user_space_by_display: [(display_uuid, user_space)].into_iter().collect(),
        space_remaps: Vec::new(),
        display_set_changed: false,
        topology_changed: false,
        allow_space_remap: false,
        should_force_refresh_layout: false,
        releases_lifecycle_refresh_quarantine: false,
        releases_display_churn_refresh_quarantine: false,
        resized_spaces: Vec::new(),
        topology_window_delta: None,
        active_window_spaces: Default::default(),
    }));

    let (app_tx, _app_rx) = crate::actor::channel();
    reactor.app_manager.apps.insert(pid, AppState {
        info: AppInfo {
            bundle_id: Some("com.testapp1".to_string()),
            localized_name: Some("TestApp1".to_string()),
        },
        handle: AppThreadHandle::new_for_test(app_tx),
    });

    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(user_space)
        .to_vec();
    let default_workspace = workspaces[0].0;
    let secondary_workspace = workspaces[1].0;
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                user_space,
                wid,
                secondary_workspace
            )
    );

    reactor
        .state
        .windows
        .track_window_server_info(crate::sys::window_server::WindowServerInfo {
            id: wsid,
            pid,
            layer: 0,
            frame: screen,
            min_frame: screen.size,
            max_frame: screen.size,
        });
    reactor.state.windows.set_window_server_space(wsid, Some(user_space));

    reactor.handle_event(Event::WindowsDiscovered {
        pid,
        new: vec![(wid, WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Window".to_string(),
            frame: screen,
            sys_id: Some(wsid),
            bundle_id: Some("com.testapp1".to_string()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        })],
        known_visible: vec![wid],
    });

    assert_ne!(secondary_workspace, default_workspace);
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, user_space, wid),
        Some(secondary_workspace),
        "fullscreen startup discovery must preserve the existing hidden assignment instead of defaulting it"
    );
}

// Helper: check whether any window owned by `pid` appears in the layout tree for `space`.
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
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
        reactor.state.windows.window(wid).is_some_and(|window| window.info.is_minimized),
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

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
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
            .state
            .windows
            .window(wid)
            .is_some_and(|window| !window.info.is_minimized),
        "reactor state must clear the minimized flag after restore"
    );
}

#[test]
fn discovery_manageability_loss_removes_window_from_layout() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, space, screen, wid));

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(wid, WindowInfo {
            is_root: false,
            ..make_window(1)
        })],
        known_visible: vec![wid],
    });

    assert!(
        !has_window_in_layout(&mut reactor, space, screen, wid),
        "window must be removed from layout when discovery marks it unmanageable"
    );
    assert!(
        reactor
            .state
            .windows
            .window(wid)
            .is_some_and(|window| !window.matches_filter(WindowFilter::Manageable)),
        "reactor state must keep the window marked unmanageable"
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
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(user_space)]));
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
    let _ = reactor.state.windows.suspend_window_to_native_fullscreen(
        window_id,
        Some(WindowServerId::new(1)),
        Some(user_space),
        fullscreen_space,
        NativeFullscreenTransition::Suspended,
    );

    // Transition to fullscreen space.
    reactor.handle_event(space_state_event(vec![full_screen], vec![None]));
    apps.simulate_until_quiet(&mut reactor);

    // Exit fullscreen (return to user space).
    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(user_space)]));

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
        reactor.state.windows.native_fullscreen_record_for_window(window_id).is_none(),
        "Fullscreen track should be removed from space manager"
    );
}

#[test]
fn fullscreen_exit_space_restore_does_not_revive_stale_pre_rekey_window() {
    let (mut reactor, old_wid, wsid, user_space, _other_space, full_screen) =
        reactor_with_window_on_space1();
    let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());
    let new_wid = WindowId::new(old_wid.pid, 99);

    reactor.send_layout_event(LayoutEvent::WindowAdded(user_space, old_wid));
    assert!(has_window_in_layout(
        &mut reactor,
        user_space,
        full_screen,
        old_wid
    ));

    reactor.space_state.fullscreen_spaces.insert(fullscreen_space);
    let _ = reactor.state.windows.suspend_window_to_native_fullscreen(
        old_wid,
        Some(wsid),
        Some(user_space),
        fullscreen_space,
        NativeFullscreenTransition::Suspended,
    );
    reactor.send_layout_event(LayoutEvent::WindowRemovedPreserveFloating(old_wid));

    let old_info = reactor
        .state
        .windows
        .window(old_wid)
        .expect("old window should exist before rekey")
        .info
        .clone();
    reactor.handle_event(Event::WindowsDiscovered {
        pid: old_wid.pid,
        new: vec![(new_wid, WindowInfo {
            sys_id: old_info.sys_id,
            ..old_info
        })],
        known_visible: vec![new_wid],
    });
    assert!(
        reactor.state.windows.window(old_wid).is_none(),
        "rekey should retire the old AX id before the fullscreen exit snapshot arrives"
    );

    reactor.handle_event(space_state_event(vec![full_screen], vec![Some(user_space)]));

    assert!(
        !has_window_in_layout(&mut reactor, user_space, full_screen, old_wid),
        "fullscreen exit must not recreate a stale layout-only ghost for the old AX window id"
    );
}

#[test]
fn display_churn_snapshot_ack_triggers_visible_window_refresh() {
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));

    reactor.handle_event(space_state_event(vec![screen], vec![Some(SpaceId::new(1))]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::DisplayChurnBegin);
    let Event::SpaceStateChanged(mut snapshot) =
        space_state_event(vec![screen], vec![Some(SpaceId::new(1))])
    else {
        unreachable!("space_state_event must produce a space-state event");
    };
    snapshot.releases_display_churn_refresh_quarantine = true;
    reactor.handle_event(Event::SpaceStateChanged(snapshot));

    assert!(
        apps.requests()
            .into_iter()
            .any(|request| matches!(request, Request::GetVisibleWindows)),
        "the snapshot acknowledgement should release churn and request visible windows"
    );
}

#[test]
fn display_churn_end_refresh_is_idempotent_without_topology_change() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    assert!(has_window_in_layout(&mut reactor, space, screen, wid));

    reactor.handle_event(Event::DisplayChurnEnd);
    apps.simulate_until_quiet(&mut reactor);

    assert!(
        has_window_in_layout(&mut reactor, space, screen, wid),
        "recovery refresh should preserve existing workspace membership when topology is unchanged"
    );
    assert!(
        apps.requests().is_empty(),
        "idempotent churn-end refresh should not trigger follow-up frame writes when nothing moved"
    );
}

#[test]
fn display_churn_end_refresh_preserves_non_default_workspace_without_app_rules() {
    let mut apps = Apps::new();
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space)
        .to_vec();
    let default_workspace = workspaces[0].0;
    let secondary_workspace = workspaces[1].0;

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space,
                wid,
                secondary_workspace
            )
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .set_active_workspace(space, secondary_workspace)
    );
    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![wid],
    });

    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, wid),
        Some(secondary_workspace)
    );
    assert_ne!(secondary_workspace, default_workspace);
    assert!(has_window_in_layout(&mut reactor, space, screen, wid));

    reactor.handle_event(Event::DisplayChurnEnd);
    apps.simulate_until_quiet(&mut reactor);

    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, wid),
        Some(secondary_workspace),
        "visibility refresh must preserve an existing non-default assignment when no app rule matches"
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.active_workspace(space),
        Some(secondary_workspace),
        "refresh must not switch the active workspace back to default"
    );
    assert!(
        has_window_in_layout(&mut reactor, space, screen, wid),
        "window should remain in the visible layout of its non-default workspace after refresh"
    );
}

#[test]
fn session_gate_ignores_discovery_and_replays_one_refresh_after_unlock() {
    let mut apps = Apps::new();
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space)
        .to_vec();
    let secondary_workspace = workspaces[1].0;

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space,
                wid,
                secondary_workspace
            )
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .set_active_workspace(space, secondary_workspace)
    );

    assert!(apps.requests().is_empty());

    reactor.handle_event(Event::SessionDidResignActive);
    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![],
    });
    reactor.handle_event(Event::ApplicationGloballyActivated(1));

    assert!(
        apps.requests().is_empty(),
        "locked-session discovery and activation should defer refreshes instead of querying apps"
    );
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, wid),
        Some(secondary_workspace),
        "ignored lock-session discovery must not reassign the window back to the default workspace"
    );

    reactor.handle_event(Event::SessionDidBecomeActive);
    assert!(
        apps.requests().is_empty(),
        "unlock should stay quarantined until the spaces actor publishes a fresh post-unlock snapshot"
    );
    let stale_snapshot = match space_state_event(vec![screen], vec![Some(space)]) {
        Event::SpaceStateChanged(mut state) => {
            state.releases_lifecycle_refresh_quarantine = false;
            Event::SpaceStateChanged(state)
        }
        other => panic!("unexpected event: {other:?}"),
    };
    reactor.handle_event(stale_snapshot);
    assert!(
        apps.requests().is_empty(),
        "an older queued WM snapshot must not release the unlock quarantine"
    );

    let fresh_snapshot = match space_state_event(vec![screen], vec![Some(space)]) {
        Event::SpaceStateChanged(mut state) => {
            state.releases_lifecycle_refresh_quarantine = true;
            Event::SpaceStateChanged(state)
        }
        other => panic!("unexpected event: {other:?}"),
    };
    reactor.handle_event(fresh_snapshot);

    let requests = apps.requests();
    assert_eq!(
        requests
            .into_iter()
            .filter(|request| matches!(request, Request::GetVisibleWindows))
            .count(),
        1,
        "the first fresh post-unlock snapshot should flush exactly one deferred visibility refresh"
    );
}

#[test]
fn wake_gate_waits_for_fresh_space_snapshot_before_refresh() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    assert!(apps.requests().is_empty());

    reactor.handle_event(Event::SystemWillSleep);
    reactor.handle_event(Event::SystemWoke);
    reactor.handle_event(Event::ApplicationGloballyActivated(1));

    assert!(
        apps.requests().is_empty(),
        "wake should remain quarantined until the spaces actor publishes a fresh post-wake snapshot"
    );

    let stale_snapshot = match space_state_event(vec![screen], vec![Some(space)]) {
        Event::SpaceStateChanged(mut state) => {
            state.releases_lifecycle_refresh_quarantine = false;
            Event::SpaceStateChanged(state)
        }
        other => panic!("unexpected event: {other:?}"),
    };
    reactor.handle_event(stale_snapshot);
    assert!(
        apps.requests().is_empty(),
        "an older queued WM snapshot must not release the wake quarantine"
    );

    let fresh_snapshot = match space_state_event(vec![screen], vec![Some(space)]) {
        Event::SpaceStateChanged(mut state) => {
            state.releases_lifecycle_refresh_quarantine = true;
            Event::SpaceStateChanged(state)
        }
        other => panic!("unexpected event: {other:?}"),
    };
    reactor.handle_event(fresh_snapshot);

    let requests = apps.requests();
    assert_eq!(
        requests
            .into_iter()
            .filter(|request| matches!(request, Request::GetVisibleWindows))
            .count(),
        1,
        "the first fresh post-wake snapshot should flush exactly one deferred visibility refresh"
    );
}

#[test]
fn partial_post_wake_snapshot_preserves_manual_workspace_assignment() {
    let mut apps = Apps::new();
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let kept = WindowId::new(1, 1);
    let omitted = WindowId::new(1, 2);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(2)));
    apps.simulate_until_quiet(&mut reactor);

    let secondary_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space)[1]
        .0;
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space,
                omitted,
                secondary_workspace
            )
    );

    reactor.handle_event(Event::SystemWillSleep);
    reactor.handle_event(Event::SystemWoke);

    let mut fresh_state = match space_state_event(vec![screen], vec![Some(space)]) {
        Event::SpaceStateChanged(state) => state,
        other => panic!("unexpected event: {other:?}"),
    };
    fresh_state.releases_lifecycle_refresh_quarantine = true;
    fresh_state
        .active_window_spaces
        .insert(WindowServerId::new(kept.idx.get()), space);
    reactor.handle_event(Event::SpaceStateChanged(fresh_state));

    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, omitted),
        Some(secondary_workspace),
        "a partial recovery snapshot must not erase a manual workspace assignment"
    );

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![],
        known_visible: vec![kept, omitted],
    });

    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, omitted),
        Some(secondary_workspace),
        "post-wake discovery without an app rule must retain the manual workspace"
    );
}

#[test]
fn ax_invalidation_after_quarantine_release_preserves_workspace_assignment() {
    let mut apps = Apps::new();
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    let rediscovered_info = reactor
        .state
        .windows
        .window(wid)
        .expect("window should initially be tracked")
        .info
        .clone();

    let secondary_workspace = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space)[1]
        .0;
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space,
                wid,
                secondary_workspace,
            )
    );

    // Reproduce the ordering from the dock-wake log: the topology snapshot has already
    // released every timing-based quarantine, then the old AX element is invalidated while
    // its WindowServer window remains alive.
    assert!(!reactor.refreshes_blocked());
    let outcome = window_workflow::handle_window_invalidated(
        &mut reactor.state,
        &reactor.transaction_manager,
        &mut reactor.drag_manager,
        window_workflow::WindowInvalidatedPayload { window: wid },
    )
    .expect("AX invalidation should be handled");
    reactor.apply_event_outcome(outcome);

    assert!(
        reactor.state.windows.window(wid).is_none(),
        "the invalid AX-backed state must not remain as a live ghost",
    );
    assert_eq!(
        reactor
            .state
            .windows
            .workspace_info_for_window(wid)
            .map(|info| info.workspace_id),
        Some(secondary_workspace),
        "detached recovery metadata must retain the virtual-workspace assignment",
    );
    assert!(
        !reactor
            .state
            .windows
            .workspace_windows(space, secondary_workspace)
            .contains(&wid),
        "detached metadata must not appear in live workspace membership",
    );
    let requests = apps.requests();
    assert!(
        requests
            .into_iter()
            .any(|request| matches!(request, Request::GetVisibleWindows)),
        "Rift should reacquire the replacement AX element after preserving model state",
    );

    reactor.handle_event(Event::WindowsDiscovered {
        pid: wid.pid,
        new: vec![(wid, rediscovered_info)],
        known_visible: vec![wid],
    });

    assert!(reactor.state.windows.window(wid).is_some());
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, wid),
        Some(secondary_workspace),
        "rediscovery must restore the detached assignment instead of defaulting to WS1",
    );
}

#[test]
fn ax_invalidation_during_refresh_quarantine_detaches_live_state() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    reactor.refresh_quarantine_manager.display_churn_active = true;

    reactor.handle_event(Event::WindowDestroyed(wid));

    assert!(
        reactor.state.windows.window(wid).is_none(),
        "an invalid AX handle must leave live/layout state even while native evidence is unstable",
    );
    assert!(
        reactor.state.windows.record(wid).is_some(),
        "the unstable native snapshot must not erase recovery identity",
    );
}

#[test]
fn authoritative_destruction_removes_window_server_backed_state() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    let wsid = reactor
        .state
        .windows
        .window(wid)
        .and_then(|window| window.info.sys_id)
        .expect("test window should have a WindowServer identity");

    let outcome = window_workflow::handle_window_destroyed(
        &mut reactor.state,
        &reactor.transaction_manager,
        &mut reactor.drag_manager,
        window_workflow::WindowDestroyedPayload { window: wid },
    )
    .expect("authoritative destruction should be handled");
    reactor.apply_event_outcome(outcome);

    assert!(reactor.state.windows.record(wid).is_none());
    assert_eq!(reactor.state.windows.tracked_window_id(wsid), None);
    assert_eq!(reactor.state.windows.workspace_info_for_window(wid), None);
}

#[test]
fn window_server_disappearance_retires_detached_ax_recovery_state() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);
    let wsid = reactor
        .state
        .windows
        .window(wid)
        .and_then(|window| window.info.sys_id)
        .expect("test window should have a WindowServer identity");

    let invalidated = window_workflow::handle_window_invalidated(
        &mut reactor.state,
        &reactor.transaction_manager,
        &mut reactor.drag_manager,
        window_workflow::WindowInvalidatedPayload { window: wid },
    )
    .expect("AX invalidation should be handled");
    reactor.apply_event_outcome(invalidated);
    assert!(reactor.state.windows.window(wid).is_none());
    assert!(reactor.state.windows.record(wid).is_some());

    let disappeared = topology_workflow::handle_window_server_destroyed(
        &mut reactor.state,
        &reactor.transaction_manager,
        &mut reactor.drag_manager,
        topology_workflow::WindowServerLifecyclePayload {
            window_server_id: wsid,
            space,
            kind: SpaceEventKind::User,
        },
        topology_workflow::WindowServerDestroyedObservations {
            resolved_space: Some(space),
            active_spaces: [space].into_iter().collect(),
            mission_control_active: false,
            ordered_in: true,
            assigned_space: Some(space),
            last_known_user_space: Some(space),
        },
    )
    .expect("WindowServer disappearance should be handled");
    reactor.apply_event_outcome(disappeared);

    assert!(reactor.state.windows.record(wid).is_none());
    assert_eq!(reactor.state.windows.tracked_window_id(wsid), None);
}

#[test]
fn authoritative_active_space_membership_comes_from_space_window_ids_directly() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wsid_a = WindowServerId::new(41);
    let wsid_b = WindowServerId::new(42);

    crate::sys::window_server::set_space_window_list_for_connection_override(Some(vec![
        wsid_a.as_u32(),
        wsid_b.as_u32(),
    ]));

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    let snapshot = reactor.authoritative_active_space_windows();

    crate::sys::window_server::set_space_window_list_for_connection_override(None);

    let ids: Vec<_> = snapshot.into_iter().map(|(wsid, _)| wsid).collect();
    assert_eq!(
        ids,
        vec![wsid_a, wsid_b],
        "active-space membership should be built from the space's own WS ids rather than the lagging global visible-window list"
    );
}

#[test]
fn authoritative_active_space_membership_queries_each_active_space_independently() {
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
    let space1 = SpaceId::new(1);
    let space2 = SpaceId::new(2);
    let wsid_left = WindowServerId::new(41);
    let wsid_right = WindowServerId::new(42);

    crate::sys::window_server::set_space_window_list_for_space_override(
        space1.get(),
        Some(vec![wsid_left.as_u32()]),
    );
    crate::sys::window_server::set_space_window_list_for_space_override(
        space2.get(),
        Some(vec![wsid_right.as_u32()]),
    );
    crate::sys::window_server::set_window_spaces_override(wsid_left, Some(vec![space1.get()]));
    crate::sys::window_server::set_window_spaces_override(wsid_right, Some(vec![space2.get()]));

    reactor.handle_event(space_state_event(vec![left, right], vec![
        Some(space1),
        Some(space2),
    ]));
    let mut snapshot = reactor.authoritative_active_space_windows();

    crate::sys::window_server::set_space_window_list_for_space_override(space1.get(), None);
    crate::sys::window_server::set_space_window_list_for_space_override(space2.get(), None);
    crate::sys::window_server::set_window_spaces_override(wsid_left, None);
    crate::sys::window_server::set_window_spaces_override(wsid_right, None);

    snapshot.sort_unstable_by_key(|(wsid, _)| wsid.as_u32());
    assert_eq!(
        snapshot,
        vec![(wsid_left, Some(space1)), (wsid_right, Some(space2))],
        "multi-display active-space membership should be collected per active space so stale union snapshots do not keep windows visible after topology changes"
    );
}

#[test]
fn empty_active_space_membership_during_wake_race_does_not_blank_known_active_windows() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let wid = WindowId::new(1, 1);
    let wsid = WindowServerId::new(10001);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    reactor.state.windows.set_window_server_space(wsid, Some(space));
    reactor.state.windows.mark_window_visible(wsid);

    crate::sys::window_server::set_space_window_list_for_connection_override(Some(vec![]));
    reactor.refresh_window_server_snapshot_for_active_spaces();
    crate::sys::window_server::set_space_window_list_for_connection_override(None);

    assert!(
        reactor.state.windows.is_window_visible(wsid),
        "a transient empty active-space WS-id result after wake must not blank windows we already know belong to the active space"
    );
    assert!(
        has_window_in_layout(&mut reactor, space, screen, wid),
        "preserving the visibility basis must also preserve the active workspace layout until discovery catches up"
    );
}

#[test]
fn wsid_rekey_preserves_non_default_workspace_without_app_rules() {
    let mut apps = Apps::new();
    let workspace_cfg = crate::common::config::VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..crate::common::config::VirtualWorkspaceSettings::default()
    };
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &workspace_cfg,
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let old_wid = WindowId::new(1, 1);
    let new_wid = WindowId::new(1, 99);

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app(1, make_windows(1)));
    apps.simulate_until_quiet(&mut reactor);

    let workspaces = reactor
        .layout_manager
        .layout_engine
        .virtual_workspace_manager_mut()
        .list_workspaces(space)
        .to_vec();
    let secondary_workspace = workspaces[1].0;

    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .assign_window_to_workspace(
                &mut reactor.state.windows,
                space,
                old_wid,
                secondary_workspace
            )
    );
    assert!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager_mut()
            .set_active_workspace(space, secondary_workspace)
    );

    let old_info = reactor
        .state
        .windows
        .window(old_wid)
        .expect("old window should exist")
        .info
        .clone();

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(new_wid, WindowInfo {
            sys_id: old_info.sys_id,
            ..old_info
        })],
        known_visible: vec![new_wid],
    });

    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_for_window(&reactor.state.windows, space, new_wid),
        Some(secondary_workspace),
        "AX id churn for the same WindowServer window must preserve its workspace assignment"
    );
    assert_eq!(
        reactor
            .layout_manager
            .layout_engine
            .virtual_workspace_manager()
            .workspace_info_for_window_any(&reactor.state.windows, old_wid),
        None,
        "old AX window id should relinquish its assignment after rekey"
    );
}

#[test]
fn wsid_rekey_preserves_floating_membership_and_position() {
    let mut apps = Apps::new();
    let mut reactor = Reactor::new_for_test(LayoutEngine::new(
        &crate::common::config::VirtualWorkspaceSettings::default(),
        &crate::common::config::LayoutSettings::default(),
        None,
    ));
    let screen = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
    let space = SpaceId::new(1);
    let old_wid = WindowId::new(1, 1);
    let new_wid = WindowId::new(1, 99);
    let stored_position = CGRect::new(CGPoint::new(320., 180.), CGSize::new(240., 200.));

    reactor.handle_event(space_state_event(vec![screen], vec![Some(space)]));
    reactor.handle_events(apps.make_app_with_opts(1, make_windows(1), Some(old_wid), true, true));
    reactor.handle_event(Event::ApplicationGloballyActivated(1));
    apps.simulate_until_quiet(&mut reactor);

    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::ToggleWindowFloating,
    )));
    apps.simulate_until_quiet(&mut reactor);
    assert!(reactor.layout_manager.layout_engine.is_window_floating(old_wid));

    let active_workspace = reactor
        .layout_manager
        .layout_engine
        .active_workspace(space)
        .expect("active workspace");
    reactor.layout_manager.layout_engine.store_floating_position(
        space,
        active_workspace,
        old_wid,
        stored_position,
    );

    let old_info = reactor
        .state
        .windows
        .window(old_wid)
        .expect("old window should exist")
        .info
        .clone();

    reactor.handle_event(Event::WindowsDiscovered {
        pid: 1,
        new: vec![(new_wid, WindowInfo {
            sys_id: old_info.sys_id,
            ..old_info
        })],
        known_visible: vec![new_wid],
    });

    assert!(!reactor.layout_manager.layout_engine.is_window_floating(old_wid));
    assert!(reactor.layout_manager.layout_engine.is_window_floating(new_wid));
    assert_eq!(
        reactor.layout_manager.layout_engine.get_floating_position(
            space,
            active_workspace,
            old_wid
        ),
        None
    );
    assert_eq!(
        reactor.layout_manager.layout_engine.get_floating_position(
            space,
            active_workspace,
            new_wid
        ),
        Some(stored_position)
    );
}

#[test]
fn native_space_resolution_policy_table() {
    let mut cases = Vec::new();

    // A direct observation from the old space is stale while Rift's target is
    // still pending.
    {
        let (reactor, _wid, wsid, space1, space2, _) = reactor_with_window_moved_to_space2();
        cases.push((
            "stale origin",
            reactor.resolve_native_space(wsid, Some(space1)),
            Some(space2),
        ));
    }

    // A direct observation of the target confirms the pending move.
    {
        let (reactor, _wid, wsid, _space1, space2, _) = reactor_with_window_moved_to_space2();
        let resolved = reactor.resolve_native_space(wsid, Some(space2));
        reactor.clear_pending_target_if_confirmed_space(wsid, space2);
        cases.push(("confirmed target", resolved, Some(space2)));
    }

    // With no pending Rift move, a live WindowServer observation is an external move.
    {
        let (reactor, _wid, wsid, _space1, space2, _) = reactor_with_window_on_space1();
        crate::sys::window_server::set_window_spaces_override(wsid, Some(vec![space2.get()]));
        let resolved = reactor.resolve_native_space(wsid, Some(space2));
        crate::sys::window_server::set_window_spaces_override(wsid, None);
        cases.push(("newer external move", resolved, Some(space2)));
    }

    // With only an accepted prior observation, a partial sample keeps it.
    {
        let (reactor, _wid, wsid, space1, _space2, _) = reactor_with_window_on_space1();
        cases.push((
            "partial observation",
            reactor.resolve_native_space(wsid, None),
            Some(space1),
        ));
    }

    // Geometry is used only when no native or prior WindowServer state exists.
    {
        let mut reactor = Reactor::new_for_test(LayoutEngine::new(
            &crate::common::config::VirtualWorkspaceSettings::default(),
            &crate::common::config::LayoutSettings::default(),
            None,
        ));
        let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1000., 1000.));
        let right = CGRect::new(CGPoint::new(1000., 0.), CGSize::new(1000., 1000.));
        let space2 = SpaceId::new(2);
        reactor.handle_event(space_state_event(vec![left, right], vec![
            Some(SpaceId::new(1)),
            Some(space2),
        ]));
        let frame = CGRect::new(CGPoint::new(1200., 100.), CGSize::new(400., 400.));
        cases.push((
            "geometry fallback",
            reactor.best_space_for_window(&frame, Some(WindowServerId::new(9999))),
            Some(space2),
        ));
    }

    for (case, resolved, expected) in cases {
        assert_eq!(resolved, expected, "resolver case: {case}");
    }
}

fn laid_out_frame(
    reactor: &mut Reactor,
    space: SpaceId,
    screen: CGRect,
    wid: WindowId,
) -> Option<CGRect> {
    let gaps = reactor.config.settings.layout.gaps.clone();
    reactor
        .layout_manager
        .layout_engine
        .calculate_layout_with_virtual_workspaces(
            &reactor.state.windows,
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
            |q| reactor.state.windows.window(q).map(|w| w.frame_monotonic),
            &[screen],
        )
        .into_iter()
        .find(|(w, _)| *w == wid)
        .map(|(_, f)| f)
}

#[test]
fn floating_window_toggles_to_fullscreen() {
    let (mut reactor, wid, space1, screen, _floating_frame) = reactor_with_floating_window();
    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::ToggleFullscreen)));
    let laid_out = laid_out_frame(&mut reactor, space1, screen, wid).expect("window laid out");
    assert!(
        laid_out.same_as(screen),
        "expected fullscreen {screen:?}, got {laid_out:?}"
    );
}

#[test]
fn floating_window_toggle_off_restore_previous_frame() {
    let (mut reactor, wid, space1, screen, floating_frame) = reactor_with_floating_window();
    // Turn on
    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::ToggleFullscreen)));
    // Turn off
    reactor.handle_event(Event::Command(Command::Layout(LayoutCommand::ToggleFullscreen)));
    let laid_out = laid_out_frame(&mut reactor, space1, screen, wid).expect("window laid out");
    assert!(
        laid_out.same_as(floating_frame),
        "expected restore to {floating_frame:?}, got {laid_out:?}"
    );
}

#[test]
fn floating_window_toggles_to_fullscreen_within_gaps() {
    let (mut reactor, wid, space1, screen, _floating_frame) = reactor_with_floating_window();
    // Assymetric gaps to prevent swapped left/right or swapped width/height bugs from passing
    reactor.config.settings.layout.gaps.outer = OuterGaps {
        top: 10.,
        left: 20.,
        bottom: 30.,
        right: 40.,
    };
    reactor.handle_event(Event::Command(Command::Layout(
        LayoutCommand::ToggleFullscreenWithinGaps,
    )));
    let expected = CGRect::new(
        CGPoint::new(screen.origin.x + 20., screen.origin.y + 10.),
        CGSize::new(screen.size.width - 20. - 40., screen.size.height - 10. - 30.),
    );
    let laid_out = laid_out_frame(&mut reactor, space1, screen, wid).expect("window laid out");
    assert!(
        laid_out.same_as(expected),
        "expected {expected:?}, got {laid_out:?}"
    );
}
