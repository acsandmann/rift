use objc2_core_foundation::CGSize;

use super::*;
use crate::actor::app::WindowInfo;
use crate::common::config::LayoutMode;
use crate::layout_engine::{LayoutEvent, LayoutSystemKind};
use crate::model::VirtualWorkspace;
use crate::model::reactor::WindowState;
use crate::sys::window_server::WindowServerId;

fn test_engine() -> LayoutEngine {
    LayoutEngine::new(
        &VirtualWorkspaceSettings::default(),
        &LayoutSettings::default(),
        None,
    )
}

#[test]
fn identity_transfer_preserves_window_tree_position_and_fingerprint() {
    let mut window_store = WindowStore::default();
    let mut engine = test_engine();
    let space = SpaceId::new(77);
    let old = WindowId::new(10, 1);
    let sibling = WindowId::new(10, 2);
    let replacement = WindowId::new(20, 9);

    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, old));
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, sibling));
    engine.persistence.windows.insert(old, WindowFingerprint {
        window_server_id: Some(42),
        title: Some("Editor".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.editor".into()),
    });
    engine.persistence.pending_windows.insert(old);
    let workspace = engine.active_workspace(space).unwrap();
    let layout = engine.workspace_layouts.active(space, workspace).unwrap();
    let other_workspace = engine
        .virtual_workspace_manager
        .list_workspaces(space)
        .into_iter()
        .map(|(workspace, _)| workspace)
        .find(|candidate| *candidate != workspace)
        .unwrap();
    let other_layout = engine.workspace_layouts.active(space, other_workspace).unwrap();
    // Model a provisional live projection created before the restored identity is matched.
    engine
        .workspace_tree_mut(other_workspace)
        .add_window_after_selection(other_layout, replacement);
    let before = engine.workspace_tree(workspace).visible_windows_in_layout(layout);

    engine.transfer_persistent_window_identity(old, replacement);

    let after = engine.workspace_tree(workspace).visible_windows_in_layout(layout);
    assert_eq!(
        after,
        before
            .into_iter()
            .map(|window| if window == old { replacement } else { window })
            .collect::<Vec<_>>()
    );
    assert!(!engine.persistence.windows.contains_key(&old));
    assert_eq!(
        engine.persistence.windows[&replacement].window_server_id,
        Some(42)
    );
    assert!(!engine.persistence.pending_windows.contains(&old));
    assert!(engine.persistence.pending_windows.contains(&replacement));
    assert!(
        !engine
            .workspace_tree(other_workspace)
            .contains_window(other_layout, replacement),
        "identity replacement must not leave the live id in its provisional workspace"
    );
}

#[test]
fn save_and_load_arms_fingerprint_reconciliation() {
    let mut engine = test_engine();
    let window = WindowId::new(42, 7);
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(123);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, window));
    engine.persistence.windows.insert(window, WindowFingerprint {
        window_server_id: Some(9001),
        title: Some("Project".into()),
        width: 900.0,
        height: 700.0,
        app_id: Some("com.example.editor".into()),
    });
    let path = std::env::temp_dir().join(format!(
        "rift-layout-restore-test-{}-{}.ron",
        std::process::id(),
        window.idx.get()
    ));

    engine.save(path.clone()).unwrap();
    let loaded = LayoutEngine::load(path.clone()).unwrap();
    let _ = std::fs::remove_file(path);

    assert_eq!(loaded.persistence.windows[&window].window_server_id, Some(9001));
    assert!(loaded.persistence.pending_windows.contains(&window));
    assert!(loaded.restored_location_for_window(window).is_some());
}

#[test]
fn full_save_records_floating_window_in_its_inactive_workspace() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(122);
    let size = CGSize::new(1200.0, 800.0);
    let frame = objc2_core_foundation::CGRect::new(
        objc2_core_foundation::CGPoint::new(40.0, 50.0),
        CGSize::new(640.0, 480.0),
    );
    let window = WindowId::new(41, 6);
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, size));
    let active_workspace = engine.active_workspace(space).unwrap();
    let inactive_workspace = engine
        .virtual_workspace_manager
        .list_workspaces(space)
        .into_iter()
        .map(|(workspace, _)| workspace)
        .find(|workspace| *workspace != active_workspace)
        .unwrap();
    window_store.insert_window(window, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Inactive floating".into(),
            frame,
            sys_id: Some(WindowServerId::new(4106)),
            bundle_id: Some("com.example.floating".into()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: true,
        ignore_app_rule: false,
    });
    assert!(engine.virtual_workspace_manager.assign_window_to_workspace(
        &mut window_store,
        space,
        window,
        inactive_workspace,
    ));
    engine.floating.add_floating(window);
    let path = std::env::temp_dir().join(format!(
        "rift-inactive-floating-save-test-{}-{}.ron",
        std::process::id(),
        space.get(),
    ));

    engine.save_current_layout(path.clone(), &window_store, Some(space)).unwrap();
    let loaded = LayoutEngine::load(path.clone()).unwrap();
    let _ = std::fs::remove_file(path);

    assert_eq!(
        loaded.restored_location_for_window(window),
        Some((space, inactive_workspace)),
    );
    assert_eq!(
        loaded.floating_positions.get(space, inactive_workspace, window),
        Some(frame),
    );
    assert!(loaded.floating.is_floating(window));
}

#[test]
fn full_save_removes_stale_floating_frame_from_a_tiled_window() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(124);
    let size = CGSize::new(1200.0, 800.0);
    let frame = objc2_core_foundation::CGRect::new(
        objc2_core_foundation::CGPoint::new(80.0, 90.0),
        CGSize::new(700.0, 500.0),
    );
    let window = WindowId::new(41, 7);
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, size));
    let workspace = engine.active_workspace(space).unwrap();
    window_store.insert_window(window, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Tiled".into(),
            frame,
            sys_id: Some(WindowServerId::new(4107)),
            bundle_id: Some("com.example.tiled".into()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: true,
        ignore_app_rule: false,
    });
    assert!(engine.virtual_workspace_manager.assign_window_to_workspace(
        &mut window_store,
        space,
        window,
        workspace,
    ));
    engine.add_window_to_layout(&mut window_store, space, window);
    // Model stale state left behind by an earlier floating-to-tiled transition.
    engine.floating_positions.store(space, workspace, window, frame);
    let path = std::env::temp_dir().join(format!(
        "rift-stale-floating-save-test-{}-{}.ron",
        std::process::id(),
        space.get(),
    ));

    engine.save_current_layout(path.clone(), &window_store, Some(space)).unwrap();
    let loaded = LayoutEngine::load(path.clone()).unwrap();
    let _ = std::fs::remove_file(path);

    assert_eq!(
        loaded.restored_location_for_window(window),
        Some((space, workspace))
    );
    assert_eq!(loaded.floating_positions.get(space, workspace, window), None);
    assert!(!loaded.floating.is_floating(window));
}

#[test]
fn load_does_not_arm_locationless_fingerprints() {
    let mut engine = test_engine();
    let orphan = WindowId::new(42, 8);
    engine.persistence.windows.insert(orphan, WindowFingerprint {
        window_server_id: None,
        title: Some("Untitled".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.orphan".into()),
    });

    let loaded = LayoutEngine::deserialize_from_str(&engine.serialize_to_string()).unwrap();

    assert!(loaded.persistence.windows.contains_key(&orphan));
    assert!(!loaded.persistence.pending_windows.contains(&orphan));
}

#[test]
fn load_removes_serialized_window_state_without_a_fingerprint() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(123);
    let ghost = WindowId::new(42, 9);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, ghost));
    let workspace = engine.active_workspace(space).unwrap();
    engine.floating.add_floating(ghost);
    engine.floating_positions.store(
        space,
        workspace,
        ghost,
        objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(10.0, 20.0),
            CGSize::new(700.0, 500.0),
        ),
    );
    engine
        .virtual_workspace_manager
        .set_last_focused_window(space, workspace, Some(ghost));
    assert!(!engine.persistence.windows.contains_key(&ghost));

    let loaded = LayoutEngine::deserialize_from_str(&engine.serialize_to_string()).unwrap();
    let layout = loaded.workspace_layouts.active(space, workspace).unwrap();

    assert!(!loaded.workspace_tree(workspace).contains_window(layout, ghost));
    assert!(!loaded.floating.is_floating(ghost));
    assert_eq!(loaded.floating_positions.get(space, workspace, ghost), None);
    assert_eq!(
        loaded.virtual_workspace_manager.last_focused_window(space, workspace),
        None
    );
}

#[test]
fn startup_validation_preserves_stale_ids_when_the_app_can_still_fuzzy_match() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(129);
    let closed = WindowId::new(33419, 82684);
    let still_open = WindowId::new(1430, 97361);
    let restarted_app = WindowId::new(40000, 70000);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    for window in [closed, still_open, restarted_app] {
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, window));
        engine.persistence.windows.insert(window, WindowFingerprint {
            window_server_id: Some(window.idx.get()),
            title: Some(format!("window-{}", window.idx.get())),
            width: 600.0,
            height: 800.0,
            app_id: Some(format!("com.example.{}", window.pid)),
        });
        engine.persistence.pending_windows.insert(window);
    }

    let discarded = engine.discard_unmatchable_startup_candidates(
        |window, id| window.pid == still_open.pid && id == still_open.idx.get(),
        |app_id| app_id == format!("com.example.{}", restarted_app.pid),
    );
    let workspace = engine.active_workspace(space).unwrap();
    let layout = engine.workspace_layouts.active(space, workspace).unwrap();

    assert_eq!(discarded, 1);
    assert!(!engine.workspace_tree(workspace).contains_window(layout, closed));
    assert!(!engine.persistence.windows.contains_key(&closed));
    assert!(engine.workspace_tree(workspace).contains_window(layout, still_open));
    assert!(engine.persistence.pending_windows.contains(&still_open));
    assert!(engine.workspace_tree(workspace).contains_window(layout, restarted_app));
    assert!(engine.persistence.pending_windows.contains(&restarted_app));
}

#[test]
fn workspace_restore_discards_unmatched_scoped_windows_and_floating_state() {
    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    let space = SpaceId::new(124);
    let tiled = WindowId::new(10, 1);
    let floating = WindowId::new(10, 2);
    let out_of_scope = WindowId::new(10, 3);
    let size = CGSize::new(1200.0, 800.0);
    let _ = snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(space, size));
    let workspaces = snapshot.virtual_workspace_manager.list_workspaces(space);
    let source_workspace = workspaces[0].0;
    let other_workspace = workspaces[1].0;
    let source_layout = snapshot.workspace_layouts.active(space, source_workspace).unwrap();
    let other_layout = snapshot.workspace_layouts.active(space, other_workspace).unwrap();
    snapshot
        .workspace_tree_mut(source_workspace)
        .add_window_after_selection(source_layout, tiled);
    snapshot
        .workspace_tree_mut(other_workspace)
        .add_window_after_selection(other_layout, out_of_scope);
    let floating_frame = objc2_core_foundation::CGRect::new(
        objc2_core_foundation::CGPoint::new(20.0, 30.0),
        CGSize::new(500.0, 400.0),
    );
    snapshot.floating.add_floating(floating);
    snapshot
        .floating_positions
        .store(space, source_workspace, floating, floating_frame);
    for window in [tiled, floating, out_of_scope] {
        snapshot.persistence.windows.insert(window, WindowFingerprint {
            window_server_id: Some(window.idx.get()),
            title: Some(format!("window-{}", window.idx.get())),
            width: 500.0,
            height: 400.0,
            app_id: Some("com.example.restore".into()),
        });
    }
    let path = std::env::temp_dir().join(format!(
        "rift-scoped-layout-restore-test-{}-{}.ron",
        std::process::id(),
        space.get(),
    ));
    snapshot.save(path.clone()).unwrap();

    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, size));
    let target_workspace = engine.active_workspace(space).unwrap();
    let report = engine
        .restore_layout(
            path.clone(),
            RestoreRequest::new(RestoreScope::Workspace, space),
            &mut window_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap();
    let _ = std::fs::remove_file(path);

    assert!(!engine.persistence.windows.contains_key(&tiled));
    assert!(!engine.persistence.windows.contains_key(&floating));
    assert!(!engine.persistence.windows.contains_key(&out_of_scope));
    assert!(!engine.floating.is_floating(floating));
    assert_eq!(report.workspaces_replaced, 1);
    assert_eq!(report.unmatched, 2);
    assert_eq!(report.warnings, vec![RestoreWarning::UnmatchedWindows(2)]);
    assert_eq!(
        engine.floating_positions.get(space, target_workspace, floating),
        None,
    );
}

#[test]
fn workspace_restore_keeps_current_windows_absent_from_snapshot() {
    let space = SpaceId::new(129);
    let size = CGSize::new(1200.0, 800.0);
    let frame = objc2_core_foundation::CGRect::new(
        objc2_core_foundation::CGPoint::new(10.0, 20.0),
        CGSize::new(700.0, 500.0),
    );
    let saved = WindowId::new(70, 1);
    let live = WindowId::new(71, 1);
    let live_floating = WindowId::new(72, 1);

    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    let _ = snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(space, size));
    let snapshot_workspace = snapshot.active_workspace(space).unwrap();
    let snapshot_layout = snapshot.workspace_layouts.active(space, snapshot_workspace).unwrap();
    snapshot
        .workspace_tree_mut(snapshot_workspace)
        .add_window_after_selection(snapshot_layout, saved);
    snapshot.persistence.windows.insert(saved, WindowFingerprint {
        window_server_id: Some(7001),
        title: Some("Saved".into()),
        width: 700.0,
        height: 500.0,
        app_id: Some("com.example.saved".into()),
    });
    let path = std::env::temp_dir().join(format!(
        "rift-live-window-restore-test-{}-{}.ron",
        std::process::id(),
        space.get(),
    ));
    snapshot.save(path.clone()).unwrap();

    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, size));
    let target_workspace = engine.active_workspace(space).unwrap();
    let live_state = |title: &str, bundle_id: &str, window_server_id: u32| WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: title.into(),
            frame,
            sys_id: Some(WindowServerId::new(window_server_id)),
            bundle_id: Some(bundle_id.into()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: true,
        ignore_app_rule: false,
    };
    window_store.insert_window(live, live_state("Live", "com.example.live", 7101));
    window_store.insert_window(
        live_floating,
        live_state("Live floating", "com.example.live-floating", 7201),
    );
    for window in [live, live_floating] {
        assert!(engine.virtual_workspace_manager.assign_window_to_workspace(
            &mut window_store,
            space,
            window,
            target_workspace,
        ));
    }
    engine.add_window_to_layout(&mut window_store, space, live);
    engine.floating.add_floating(live_floating);
    engine.floating_positions.store(space, target_workspace, live_floating, frame);
    engine.focused_window = Some(live);

    engine
        .restore_layout(
            path.clone(),
            RestoreRequest::new(RestoreScope::Workspace, space),
            &mut window_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap();
    let _ = std::fs::remove_file(path);

    let target_layout = engine.workspace_layouts.active(space, target_workspace).unwrap();
    assert!(engine.workspace_tree(target_workspace).contains_window(target_layout, live));
    assert!(!engine.workspace_tree(target_workspace).contains_window(target_layout, saved));
    assert_eq!(
        window_store.workspace_for_window(space, live),
        Some(target_workspace)
    );
    assert!(engine.floating.is_floating(live_floating));
    assert_eq!(
        engine.floating_positions.get(space, target_workspace, live_floating),
        Some(frame)
    );
    assert_eq!(engine.focused_window, Some(live));
    assert_eq!(
        engine.virtual_workspace_manager.last_focused_window(space, target_workspace),
        Some(live)
    );
}

#[test]
fn scoped_restore_does_not_consume_same_id_live_window_on_another_space() {
    let target_space = SpaceId::new(130);
    let external_space = SpaceId::new(131);
    let size = CGSize::new(1200.0, 800.0);
    let frame = objc2_core_foundation::CGRect::new(
        objc2_core_foundation::CGPoint::new(10.0, 20.0),
        CGSize::new(700.0, 500.0),
    );
    let reused_id = WindowId::new(73, 1);

    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    let _ = snapshot.handle_event(
        &mut snapshot_store,
        LayoutEvent::SpaceExposed(target_space, size),
    );
    let snapshot_workspace = snapshot.active_workspace(target_space).unwrap();
    let snapshot_layout =
        snapshot.workspace_layouts.active(target_space, snapshot_workspace).unwrap();
    snapshot
        .workspace_tree_mut(snapshot_workspace)
        .add_window_after_selection(snapshot_layout, reused_id);
    snapshot.persistence.windows.insert(reused_id, WindowFingerprint {
        window_server_id: Some(7300),
        title: Some("Old saved window".into()),
        width: 700.0,
        height: 500.0,
        app_id: Some("com.example.old".into()),
    });
    let path = std::env::temp_dir().join(format!(
        "rift-cross-space-id-collision-test-{}-{}.ron",
        std::process::id(),
        target_space.get(),
    ));
    snapshot.save(path.clone()).unwrap();

    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    for space in [target_space, external_space] {
        let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, size));
    }
    let external_workspace = engine.active_workspace(external_space).unwrap();
    let external_layout =
        engine.workspace_layouts.active(external_space, external_workspace).unwrap();
    window_store.insert_window(reused_id, WindowState {
        info: WindowInfo {
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: "Current external window".into(),
            frame,
            sys_id: Some(WindowServerId::new(7310)),
            bundle_id: Some("com.example.current".into()),
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        frame_monotonic: frame,
        is_manageable: true,
        ignore_app_rule: false,
    });
    assert!(engine.virtual_workspace_manager.assign_window_to_workspace(
        &mut window_store,
        external_space,
        reused_id,
        external_workspace,
    ));
    engine
        .workspace_tree_mut(external_workspace)
        .add_window_after_selection(external_layout, reused_id);
    engine.floating.add_floating(reused_id);
    engine.floating.add_active(external_space, reused_id.pid, reused_id);

    engine
        .restore_layout(
            path.clone(),
            RestoreRequest::new(RestoreScope::Workspace, target_space),
            &mut window_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap();
    let _ = std::fs::remove_file(path);

    let target_workspace = engine.active_workspace(target_space).unwrap();
    let target_layout = engine.workspace_layouts.active(target_space, target_workspace).unwrap();
    assert!(
        engine
            .workspace_tree(external_workspace)
            .contains_window(external_layout, reused_id)
    );
    assert!(
        !engine
            .workspace_tree(target_workspace)
            .contains_window(target_layout, reused_id)
    );
    assert_eq!(
        window_store.workspace_for_window(external_space, reused_id),
        Some(external_workspace)
    );
    assert!(engine.floating.is_floating(reused_id));
    assert!(engine.floating.active_flat(external_space).contains(&reused_id));
}

#[test]
fn completed_app_discovery_discards_unmatched_startup_ghosts() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(126);
    let ghost = WindowId::new(55, 1);
    let inactive_space = SpaceId::new(128);
    let inactive_ghost = WindowId::new(55, 2);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, ghost));
    engine.persistence.windows.insert(ghost, WindowFingerprint {
        window_server_id: Some(9000),
        title: Some("Closed window".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.closed-window".into()),
    });
    engine.persistence.pending_windows.insert(ghost);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(inactive_space, CGSize::new(1200.0, 800.0)),
    );
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::WindowAdded(inactive_space, inactive_ghost),
    );
    engine.persistence.windows.insert(inactive_ghost, WindowFingerprint {
        window_server_id: Some(9001),
        title: Some("Inactive-space window".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.closed-window".into()),
    });
    engine.persistence.pending_windows.insert(inactive_ghost);
    let workspace = engine.active_workspace(space).unwrap();
    let layout = engine.workspace_layouts.active(space, workspace).unwrap();
    engine.focused_window = Some(ghost);
    engine
        .virtual_workspace_manager
        .set_last_focused_window(space, workspace, Some(ghost));
    engine.floating.add_floating(ghost);
    engine.floating.set_last_focus(Some(ghost));
    assert!(engine.workspace_tree(workspace).contains_window(layout, ghost));

    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::WindowDiscoveryCompleted(ghost.pid, None, vec![space]),
    );

    assert!(!engine.workspace_tree(workspace).contains_window(layout, ghost));
    assert!(!engine.persistence.windows.contains_key(&ghost));
    assert!(!engine.persistence.pending_windows.contains(&ghost));
    assert_eq!(engine.focused_window, None);
    assert_eq!(
        engine.virtual_workspace_manager.last_focused_window(space, workspace),
        None
    );
    assert!(!engine.floating.is_floating(ghost));
    assert_ne!(engine.floating.last_focus(), Some(ghost));
    assert!(engine.persistence.pending_windows.contains(&inactive_ghost));
    assert!(engine.restored_location_for_window(inactive_ghost).is_some());
}

#[test]
fn persisted_layout_schema_is_versioned_and_legacy_files_still_load() {
    let engine = test_engine();
    let serialized = engine.serialize_to_string();
    assert!(serialized.contains("\"schema_version\":2"), "{serialized}");

    let legacy = serialized.replacen("\"schema_version\":2,", "", 1);
    LayoutEngine::deserialize_from_str(&legacy).unwrap();

    let future = serialized.replacen("\"schema_version\":2", "\"schema_version\":3", 1);
    let error = match LayoutEngine::deserialize_from_str(&future) {
        Ok(_) => panic!("future schema version should be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("newer than supported"));
}

#[test]
fn malformed_active_layout_configuration_is_rejected_at_load_boundary() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(600);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let mut serialized = engine.serialize_to_string();
    let active_size = serialized.find("active_size").unwrap_or_else(|| {
        panic!("serialized workspace must contain an active size: {serialized}")
    });
    let width = serialized[active_size..]
        .find("1200")
        .map(|offset| active_size + offset)
        .expect("serialized active size must contain the display width");
    serialized.replace_range(width..width + 4, "9999");

    let error = match LayoutEngine::deserialize_from_str(&serialized) {
        Ok(_) => panic!("invalid active layout configuration should be rejected"),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("no configuration for its active display size"),
        "{error}"
    );
}

#[test]
fn invalid_persisted_window_geometry_is_rejected() {
    let mut engine = test_engine();
    let window = WindowId::new(60, 1);
    engine.persistence.windows.insert(window, WindowFingerprint {
        window_server_id: Some(6001),
        title: Some("Invalid geometry".into()),
        width: -1.0,
        height: 500.0,
        app_id: Some("com.example.invalid".into()),
    });

    let error = match LayoutEngine::deserialize_from_str(&engine.serialize_to_string()) {
        Ok(_) => panic!("invalid persisted window geometry should be rejected"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("invalid persisted size"), "{error}");
}

#[test]
fn invalid_persisted_floating_frame_is_rejected() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(601);
    let window = WindowId::new(60, 2);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let workspace = engine.active_workspace(space).unwrap();
    engine.floating_positions.store(
        space,
        workspace,
        window,
        objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(10.0, 20.0),
            CGSize::new(-1.0, 500.0),
        ),
    );

    let error = match LayoutEngine::deserialize_from_str(&engine.serialize_to_string()) {
        Ok(_) => panic!("invalid persisted floating frame should be rejected"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("invalid floating frame"), "{error}");
}

#[test]
fn portable_restore_rejects_ambiguous_legacy_multi_space_files() {
    let source_a = SpaceId::new(603);
    let source_b = SpaceId::new(604);
    let target_space = SpaceId::new(605);
    let size = CGSize::new(1200.0, 800.0);
    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    for space in [source_a, source_b] {
        let _ = snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(space, size));
    }
    // A direct snapshot models a legacy file, which has no saved-active-space hint.
    let path = std::env::temp_dir().join(format!(
        "rift-ambiguous-portable-restore-test-{}-{}.ron",
        std::process::id(),
        target_space.get(),
    ));
    snapshot.save(path.clone()).unwrap();

    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(target_space, size));
    let target_workspace = engine.active_workspace(target_space).unwrap();
    let before_name = engine
        .virtual_workspace_manager
        .workspace_info(target_space, target_workspace)
        .unwrap()
        .name
        .clone();

    let error = engine
        .restore_layout(
            path.clone(),
            RestoreRequest::new(RestoreScope::Workspace, target_space),
            &mut window_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap_err();
    let _ = std::fs::remove_file(path);

    assert!(error.to_string().contains("cannot choose a source"), "{error}");
    assert_eq!(
        engine
            .virtual_workspace_manager
            .workspace_info(target_space, target_workspace)
            .unwrap()
            .name,
        before_name,
    );
}

#[test]
fn portable_restore_uses_the_space_that_was_active_when_saved() {
    let source_a = SpaceId::new(601);
    let source_b = SpaceId::new(602);
    // The target id also exists in the file. Portable restore must still use the saved origin,
    // while master-file restore deliberately prefers this matching current-space entry.
    let target_space = source_a;
    let size = CGSize::new(1200.0, 800.0);
    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    for space in [source_a, source_b] {
        let _ = snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(space, size));
    }
    let source_a_workspace = snapshot.active_workspace(source_a).unwrap();
    let source_b_workspace = snapshot.active_workspace(source_b).unwrap();
    assert!(snapshot.switch_workspace_layout_mode(
        &snapshot_store,
        source_a,
        source_a_workspace,
        LayoutMode::Bsp,
    ));
    assert!(snapshot.switch_workspace_layout_mode(
        &snapshot_store,
        source_b,
        source_b_workspace,
        LayoutMode::Scrolling,
    ));
    let path = std::env::temp_dir().join(format!(
        "rift-portable-source-restore-test-{}-{}.ron",
        std::process::id(),
        target_space.get(),
    ));
    snapshot
        .save_current_layout(path.clone(), &snapshot_store, Some(source_b))
        .unwrap();

    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(target_space, size));
    let target_workspace = engine.active_workspace(target_space).unwrap();
    let target_name = engine
        .virtual_workspace_manager
        .workspace_info(target_space, target_workspace)
        .unwrap()
        .name
        .clone();
    engine
        .restore_layout(
            path.clone(),
            RestoreRequest::new(RestoreScope::Workspace, target_space),
            &mut window_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap();

    assert_eq!(engine.active_layout_mode_at(target_space), LayoutMode::Scrolling);
    assert_eq!(
        engine
            .virtual_workspace_manager
            .workspace_info(target_space, target_workspace)
            .unwrap()
            .name,
        target_name,
    );

    let mut master_target = test_engine();
    let mut master_store = WindowStore::default();
    let _ = master_target
        .handle_event(&mut master_store, LayoutEvent::SpaceExposed(target_space, size));
    let master_workspace = master_target.active_workspace(target_space).unwrap();
    master_target
        .restore_layout(
            path.clone(),
            RestoreRequest::from_master_file(RestoreScope::Workspace, target_space),
            &mut master_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap();
    let _ = std::fs::remove_file(path);
    assert_eq!(
        master_target.active_layout_mode_at(target_space),
        LayoutMode::Bsp
    );
    assert_eq!(
        master_target
            .virtual_workspace_manager
            .workspace_info(target_space, master_workspace)
            .unwrap()
            .name,
        target_name,
    );
}

#[test]
fn master_workspace_restore_uses_target_ordinal_and_preserves_configured_name() {
    let mut workspace_settings = VirtualWorkspaceSettings::default();
    workspace_settings.default_workspace_count = 6;
    workspace_settings.workspace_names =
        ["B", "C", "E", "T", "X", "S"].into_iter().map(str::to_owned).collect();
    workspace_settings.default_workspace = 3;
    let layout_settings = LayoutSettings::default();
    let space = SpaceId::new(609);
    let size = CGSize::new(1200.0, 800.0);
    let mut snapshot = LayoutEngine::new(&workspace_settings, &layout_settings, None);
    let mut snapshot_store = WindowStore::default();
    let _ = snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(space, size));
    let saved_workspaces = snapshot.virtual_workspace_manager.existing_workspaces(space);
    let saved_t = saved_workspaces[3].0;
    let saved_s = saved_workspaces[5].0;
    assert_eq!(snapshot.active_workspace(space), Some(saved_t));
    assert!(snapshot.switch_workspace_layout_mode(
        &snapshot_store,
        space,
        saved_t,
        LayoutMode::Scrolling,
    ));
    assert!(snapshot.switch_workspace_layout_mode(
        &snapshot_store,
        space,
        saved_s,
        LayoutMode::Stack,
    ));
    let path = std::env::temp_dir().join(format!(
        "rift-master-workspace-ordinal-test-{}-{}.ron",
        std::process::id(),
        space.get(),
    ));
    snapshot
        .save_current_layout(path.clone(), &snapshot_store, Some(space))
        .unwrap();

    let mut engine = LayoutEngine::new(&workspace_settings, &layout_settings, None);
    let mut window_store = WindowStore::default();
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, size));
    let target_s = engine.virtual_workspace_manager.existing_workspaces(space)[5].0;
    assert!(engine.virtual_workspace_manager.set_active_workspace(space, target_s));

    engine
        .restore_layout(
            path.clone(),
            RestoreRequest::from_master_file(RestoreScope::Workspace, space),
            &mut window_store,
            &workspace_settings,
            &layout_settings,
        )
        .unwrap();
    let _ = std::fs::remove_file(path);

    let restored = engine.virtual_workspace_manager.workspace_info(space, target_s).unwrap();
    assert_eq!(restored.name, "S");
    assert_eq!(restored.layout_mode(), LayoutMode::Stack);
    assert_eq!(engine.active_layout_mode_at(space), LayoutMode::Stack);
}

#[test]
fn startup_restore_reapplies_configured_workspace_names() {
    let mut saved_settings = VirtualWorkspaceSettings::default();
    saved_settings.default_workspace_count = 2;
    saved_settings.workspace_names = vec!["Old A".into(), "Old B".into()];
    let layout_settings = LayoutSettings::default();
    let space = SpaceId::new(608);
    let mut snapshot = LayoutEngine::new(&saved_settings, &layout_settings, None);
    let mut window_store = WindowStore::default();
    let _ = snapshot.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let mut restored = LayoutEngine::deserialize_from_str(&snapshot.serialize_to_string()).unwrap();
    let mut current_settings = saved_settings;
    current_settings.workspace_names = vec!["A".into(), "S".into()];

    restored.finish_loading(&current_settings, &layout_settings, None);

    let names = restored
        .virtual_workspace_manager
        .existing_workspaces(space)
        .into_iter()
        .map(|(_, name)| name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["A", "S"]);
}

#[test]
fn startup_restore_remaps_saved_space_by_display_identity_once() {
    let saved_space = SpaceId::new(610);
    let current_space = SpaceId::new(611);
    let later_space = SpaceId::new(612);
    let size = CGSize::new(1200.0, 800.0);
    let display = "display-a".to_string();
    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    let _ =
        snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(saved_space, size));
    snapshot.update_space_display(saved_space, Some(display.clone()));
    let path = std::env::temp_dir().join(format!(
        "rift-startup-space-remap-test-{}-{}.ron",
        std::process::id(),
        saved_space.get(),
    ));
    snapshot.save(path.clone()).unwrap();

    let mut restored = LayoutEngine::load_for_startup_restore(path.clone()).unwrap();
    let _ = std::fs::remove_file(path);
    let mut window_store = WindowStore::default();
    restored.reconcile_startup_spaces(&mut window_store, &[(current_space, display.clone())]);

    assert!(!restored.workspace_layouts.spaces().contains(&saved_space));
    assert!(restored.workspace_layouts.spaces().contains(&current_space));

    // The repair is startup-only. A later ordinary native-space switch must not migrate state.
    restored.reconcile_startup_spaces(&mut window_store, &[(later_space, display)]);
    assert!(restored.workspace_layouts.spaces().contains(&current_space));
    assert!(!restored.workspace_layouts.spaces().contains(&later_space));
}

#[test]
fn pure_matcher_reports_duplicate_identities_without_mutating_candidates() {
    use super::matcher::{RestoreCandidate, choose_match};

    let stale = WindowId::new(1, 1);
    let preferred = WindowId::new(1, 2);
    let live = WindowId::new(2, 1);
    let space = SpaceId::new(500);
    let stale_workspace = crate::model::VirtualWorkspaceId::default();
    let preferred_workspace = crate::model::VirtualWorkspaceId::default();
    let fingerprint = WindowFingerprint {
        window_server_id: Some(77),
        title: Some("Editor".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.editor".into()),
    };
    let candidates = vec![
        RestoreCandidate {
            window: stale,
            fingerprint: &fingerprint,
            location: Some((SpaceId::new(499), stale_workspace)),
        },
        RestoreCandidate {
            window: preferred,
            fingerprint: &fingerprint,
            location: Some((space, preferred_workspace)),
        },
    ];

    let decision = choose_match(
        live,
        space,
        &fingerprint,
        Some((space, preferred_workspace)),
        &candidates,
    )
    .unwrap();

    assert_eq!(decision.selected, preferred);
    assert_eq!(decision.duplicate_identities, vec![stale]);
    assert_eq!(candidates.len(), 2);
}

#[test]
fn direct_window_identity_never_consumes_another_candidate() {
    use super::matcher::{RestoreCandidate, choose_match};

    let live = WindowId::new(42, 7);
    let other = WindowId::new(42, 8);
    let space = SpaceId::new(501);
    let workspace = crate::model::VirtualWorkspaceId::default();
    let direct_fingerprint = WindowFingerprint {
        window_server_id: Some(10),
        title: Some("Direct".into()),
        width: 600.0,
        height: 800.0,
        app_id: Some("com.example.editor".into()),
    };
    let other_fingerprint = WindowFingerprint {
        window_server_id: Some(20),
        title: Some("Other".into()),
        width: 600.0,
        height: 800.0,
        app_id: Some("com.example.editor".into()),
    };
    let live_fingerprint = WindowFingerprint {
        window_server_id: Some(20),
        title: Some("Other".into()),
        width: 600.0,
        height: 800.0,
        app_id: Some("com.example.editor".into()),
    };
    let candidates = [
        RestoreCandidate {
            window: live,
            fingerprint: &direct_fingerprint,
            location: Some((space, workspace)),
        },
        RestoreCandidate {
            window: other,
            fingerprint: &other_fingerprint,
            location: Some((space, workspace)),
        },
    ];

    let decision = choose_match(live, space, &live_fingerprint, None, &candidates).unwrap();

    assert_eq!(decision.selected, live);
    assert!(decision.exact_identity);
    assert!(decision.duplicate_identities.is_empty());
}

#[test]
fn fuzzy_match_requires_window_specific_evidence() {
    use super::matcher::{RestoreCandidate, choose_match};

    let saved = WindowId::new(42, 7);
    let live = WindowId::new(99, 1);
    let space = SpaceId::new(502);
    let saved_fingerprint = WindowFingerprint {
        window_server_id: None,
        title: Some("Music".into()),
        width: 500.0,
        height: 500.0,
        app_id: Some("com.example.app".into()),
    };
    let unrelated_live = WindowFingerprint {
        window_server_id: None,
        title: Some("Preferences".into()),
        width: 900.0,
        height: 700.0,
        app_id: Some("com.example.app".into()),
    };
    let candidate = [RestoreCandidate {
        window: saved,
        fingerprint: &saved_fingerprint,
        location: Some((space, crate::model::VirtualWorkspaceId::default())),
    }];

    assert!(choose_match(live, space, &unrelated_live, None, &candidate).is_none());

    let title_match = WindowFingerprint {
        title: Some("Music".into()),
        ..unrelated_live
    };
    assert_eq!(
        choose_match(live, space, &title_match, None, &candidate).map(|decision| decision.selected),
        Some(saved)
    );
}

#[test]
fn rejected_fuzzy_candidate_is_removed_when_discovery_finishes() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let space = SpaceId::new(503);
    let ghost = WindowId::new(42, 7);
    let live = WindowId::new(99, 8);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    for window in [ghost, live] {
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, window));
    }
    engine.persistence.windows.insert(ghost, WindowFingerprint {
        window_server_id: None,
        title: Some("Music".into()),
        width: 500.0,
        height: 500.0,
        app_id: Some("com.example.app".into()),
    });
    engine.persistence.pending_windows.insert(ghost);

    let outcome =
        engine.reconcile_restored_window(&mut window_store, space, live, &WindowFingerprint {
            window_server_id: None,
            title: Some("Preferences".into()),
            width: 900.0,
            height: 700.0,
            app_id: Some("com.example.app".into()),
        });
    assert!(!outcome.matched);
    assert!(engine.persistence.pending_windows.contains(&ghost));

    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::WindowDiscoveryCompleted(live.pid, Some("com.example.app".into()), vec![
            space,
        ]),
    );
    let workspace = engine.active_workspace(space).unwrap();
    let layout = engine.workspace_layouts.active(space, workspace).unwrap();

    assert!(!engine.workspace_tree(workspace).contains_window(layout, ghost));
    assert!(!engine.persistence.windows.contains_key(&ghost));
    assert!(engine.workspace_tree(workspace).contains_window(layout, live));
}

#[test]
fn space_restore_rejects_workspace_count_mismatch_before_mutating_layouts() {
    let space = SpaceId::new(125);
    let size = CGSize::new(1200.0, 800.0);
    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    let _ = snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(space, size));
    let path = std::env::temp_dir().join(format!(
        "rift-space-count-restore-test-{}-{}.ron",
        std::process::id(),
        space.get(),
    ));
    snapshot.save(path.clone()).unwrap();

    let mut target_settings = VirtualWorkspaceSettings::default();
    target_settings.default_workspace_count = 3;
    let mut engine = LayoutEngine::new(&target_settings, &LayoutSettings::default(), None);
    let mut window_store = WindowStore::default();
    let sentinel = WindowId::new(11, 1);
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, size));
    let target_workspace = engine.active_workspace(space).unwrap();
    let target_layout = engine.workspace_layouts.active(space, target_workspace).unwrap();
    engine
        .workspace_tree_mut(target_workspace)
        .add_window_after_selection(target_layout, sentinel);

    let error = engine
        .restore_saved_layout(
            path.clone(),
            RestoreScope::Space,
            space,
            &mut window_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap_err();
    let _ = std::fs::remove_file(path);

    assert!(error.to_string().contains("different workspace counts"));
    assert!(
        engine.workspace_tree(target_workspace).contains_window(target_layout, sentinel),
        "a rejected restore must leave every existing workspace layout untouched"
    );
}

#[test]
fn runtime_restore_cleans_unmatched_windows_from_inactive_size_configurations() {
    let space = SpaceId::new(504);
    let small = CGSize::new(1200.0, 800.0);
    let large = CGSize::new(1600.0, 1000.0);
    let ghost = WindowId::new(33419, 82684);
    let mut snapshot = test_engine();
    let mut snapshot_store = WindowStore::default();
    let _ = snapshot.handle_event(&mut snapshot_store, LayoutEvent::SpaceExposed(space, large));
    let workspace = snapshot.active_workspace(space).unwrap();
    let large_layout = snapshot.workspace_layouts.active(space, workspace).unwrap();
    let small_layout = snapshot.virtual_workspace_manager.workspaces[workspace]
        .layout_system
        .create_layout();
    snapshot.workspace_layouts.insert_layout_configuration_for_test(
        space,
        workspace,
        small,
        small_layout,
    );
    assert_ne!(small_layout, large_layout);
    snapshot
        .workspace_tree_mut(workspace)
        .add_window_after_selection(small_layout, ghost);
    snapshot.persistence.windows.insert(ghost, WindowFingerprint {
        window_server_id: Some(82684),
        title: Some("Music".into()),
        width: 1512.0,
        height: 944.0,
        app_id: Some("com.apple.Music".into()),
    });
    let path = std::env::temp_dir().join(format!(
        "rift-runtime-inactive-size-restore-test-{}-{}.ron",
        std::process::id(),
        space.get(),
    ));
    snapshot.save(path.clone()).unwrap();

    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let _ = engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, large));
    let report = engine
        .restore_layout(
            path.clone(),
            RestoreRequest::new(RestoreScope::Space, space),
            &mut window_store,
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
        )
        .unwrap();
    let _ = std::fs::remove_file(path);

    assert_eq!(report.unmatched, 1);
    for (_, restored_workspace, layout) in engine.workspace_layouts.all_layouts() {
        assert!(
            !engine.workspace_tree(restored_workspace).contains_window(layout, ghost),
            "unmatched runtime-restore candidate survived in a dormant size configuration"
        );
    }
    assert!(!engine.persistence.windows.contains_key(&ghost));
}

#[test]
fn every_layout_system_round_trips_through_ron() {
    let settings = LayoutSettings::default();
    for mode in [
        LayoutMode::Traditional,
        LayoutMode::Bsp,
        LayoutMode::Stack,
        LayoutMode::MasterStack,
        LayoutMode::Scrolling,
    ] {
        let system = VirtualWorkspace::create_layout_system(mode, &settings);
        let serialized = ron::ser::to_string(&system).unwrap();
        let restored: LayoutSystemKind = ron::from_str(&serialized)
            .unwrap_or_else(|error| panic!("{mode:?} failed to round-trip: {error}"));
        assert_eq!(
            std::mem::discriminant(&system),
            std::mem::discriminant(&restored)
        );
    }
}

#[test]
fn legacy_internally_tagged_layout_systems_are_migrated() {
    let settings = LayoutSettings::default();
    for (mode, variant) in [
        (LayoutMode::Traditional, "traditional"),
        (LayoutMode::Bsp, "bsp"),
        (LayoutMode::Stack, "stack"),
        (LayoutMode::MasterStack, "master_stack"),
        (LayoutMode::Scrolling, "scrolling"),
    ] {
        let system = VirtualWorkspace::create_layout_system(mode, &settings);
        let current = ron::ser::to_string(&system).unwrap();
        let prefix = format!("{variant}((");
        assert!(current.starts_with(&prefix) && current.ends_with("))"));
        let fields = &current[prefix.len()..current.len() - 2];
        let legacy = format!("(kind:\"{variant}\",{fields})");
        let migrated = super::storage::migrate_legacy_layout_system_tags(&legacy).unwrap();
        let restored: LayoutSystemKind = ron::from_str(&migrated)
            .unwrap_or_else(|error| panic!("{mode:?} migration failed: {error}"));
        assert_eq!(
            std::mem::discriminant(&system),
            std::mem::discriminant(&restored)
        );
    }
}

#[test]
fn restored_window_server_id_beats_title_fallback() {
    let mut window_store = WindowStore::default();
    let mut engine = test_engine();
    let space = SpaceId::new(88);
    let titled_match = WindowId::new(1, 1);
    let id_match = WindowId::new(1, 2);
    let live = WindowId::new(99, 1);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, titled_match));
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, id_match));
    engine.persistence.windows.insert(titled_match, WindowFingerprint {
        window_server_id: Some(10),
        title: Some("Current title".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.one".into()),
    });
    engine.persistence.windows.insert(id_match, WindowFingerprint {
        window_server_id: Some(20),
        title: Some("Old title".into()),
        width: 500.0,
        height: 400.0,
        app_id: Some("com.example.two".into()),
    });
    engine.persistence.pending_windows.extend([titled_match, id_match]);

    engine.reconcile_restored_window(&mut window_store, space, live, &WindowFingerprint {
        window_server_id: Some(20),
        title: Some("Current title".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.one".into()),
    });

    let workspace = engine.active_workspace(space).unwrap();
    let layout = engine.workspace_layouts.active(space, workspace).unwrap();
    let windows = engine.workspace_tree(workspace).visible_windows_in_layout(layout);
    assert!(windows.contains(&titled_match));
    assert!(windows.contains(&live));
    assert!(!windows.contains(&id_match));
    assert_eq!(window_store.workspace_for_window(space, live), Some(workspace));
}

#[test]
fn duplicate_window_server_fingerprints_choose_live_assignment_and_are_healed() {
    let mut window_store = WindowStore::default();
    let mut engine = test_engine();
    let space = SpaceId::new(91);
    let stale = WindowId::new(1, 1);
    let preferred = WindowId::new(1, 2);
    let live = WindowId::new(99, 1);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let workspaces = engine.virtual_workspace_manager.list_workspaces(space);
    let stale_workspace = workspaces[0].0;
    let preferred_workspace = workspaces[1].0;
    let stale_layout = engine.workspace_layouts.active(space, stale_workspace).unwrap();
    let preferred_layout = engine.workspace_layouts.active(space, preferred_workspace).unwrap();
    engine
        .workspace_tree_mut(stale_workspace)
        .add_window_after_selection(stale_layout, stale);
    engine
        .workspace_tree_mut(preferred_workspace)
        .add_window_after_selection(preferred_layout, preferred);
    assert!(engine.virtual_workspace_manager.assign_window_to_workspace(
        &mut window_store,
        space,
        live,
        preferred_workspace,
    ));
    let fingerprint = |title: &str| WindowFingerprint {
        window_server_id: Some(42),
        title: Some(title.into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.editor".into()),
    };
    engine.persistence.windows.insert(stale, fingerprint("stale"));
    engine.persistence.windows.insert(preferred, fingerprint("preferred"));
    engine.persistence.pending_windows.extend([stale, preferred]);

    engine.reconcile_restored_window(&mut window_store, space, live, &fingerprint("live"));

    assert_eq!(
        window_store.workspace_for_window(space, live),
        Some(preferred_workspace),
    );
    assert!(!engine.persistence.windows.contains_key(&stale));
    assert!(!engine.workspace_tree(stale_workspace).contains_window(stale_layout, stale));
    assert!(
        engine
            .workspace_tree(preferred_workspace)
            .contains_window(preferred_layout, live)
    );
}

#[test]
fn duplicate_restored_identity_prefers_live_workspace_assignment() {
    let mut window_store = WindowStore::default();
    let mut engine = test_engine();
    let space = SpaceId::new(90);
    let live = WindowId::new(99, 7);

    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let workspaces = engine.virtual_workspace_manager.list_workspaces(space);
    let stale_workspace = workspaces[0].0;
    let preferred_workspace = workspaces[1].0;
    let stale_layout = engine.workspace_layouts.active(space, stale_workspace).unwrap();
    let preferred_layout = engine.workspace_layouts.active(space, preferred_workspace).unwrap();
    engine
        .workspace_tree_mut(stale_workspace)
        .add_window_after_selection(stale_layout, live);
    engine
        .workspace_tree_mut(preferred_workspace)
        .add_window_after_selection(preferred_layout, live);
    assert!(engine.virtual_workspace_manager.assign_window_to_workspace(
        &mut window_store,
        space,
        live,
        preferred_workspace,
    ));

    let fingerprint = WindowFingerprint {
        window_server_id: Some(700),
        title: Some("Editor".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("dev.zed.Zed".into()),
    };
    engine.persistence.windows.insert(live, fingerprint.clone());
    engine.persistence.pending_windows.insert(live);

    engine.reconcile_restored_window(&mut window_store, space, live, &fingerprint);

    assert_eq!(
        window_store.workspace_for_window(space, live),
        Some(preferred_workspace),
    );
    assert!(!engine.workspace_tree(stale_workspace).contains_window(stale_layout, live));
    assert!(
        engine
            .workspace_tree(preferred_workspace)
            .contains_window(preferred_layout, live)
    );
}

#[test]
fn restore_fallback_never_crosses_known_app_identity() {
    let mut window_store = WindowStore::default();
    let mut engine = test_engine();
    let space = SpaceId::new(89);
    let title_match = WindowId::new(1, 1);
    let size_and_app_match = WindowId::new(1, 2);
    let live = WindowId::new(99, 1);
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
    );
    let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, title_match));
    let _ = engine.handle_event(
        &mut window_store,
        LayoutEvent::WindowAdded(space, size_and_app_match),
    );
    engine.persistence.windows.insert(title_match, WindowFingerprint {
        window_server_id: None,
        title: Some("Project".into()),
        width: 400.0,
        height: 300.0,
        app_id: Some("com.example.other".into()),
    });
    engine.persistence.windows.insert(size_and_app_match, WindowFingerprint {
        window_server_id: None,
        title: Some("Other".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.editor".into()),
    });
    engine.persistence.pending_windows.extend([title_match, size_and_app_match]);

    engine.reconcile_restored_window(&mut window_store, space, live, &WindowFingerprint {
        window_server_id: None,
        title: Some("Project".into()),
        width: 800.0,
        height: 600.0,
        app_id: Some("com.example.editor".into()),
    });

    let workspace = engine.active_workspace(space).unwrap();
    let layout = engine.workspace_layouts.active(space, workspace).unwrap();
    let windows = engine.workspace_tree(workspace).visible_windows_in_layout(layout);
    assert!(windows.contains(&title_match));
    assert!(windows.contains(&live));
    assert!(!windows.contains(&size_and_app_match));
}

#[test]
fn app_close_removes_saved_fingerprints() {
    let mut engine = test_engine();
    let mut window_store = WindowStore::default();
    let window = WindowId::new(42, 7);
    engine.persistence.windows.insert(window, WindowFingerprint {
        window_server_id: Some(9),
        title: Some("Closed".into()),
        width: 400.0,
        height: 300.0,
        app_id: Some("com.example.closed".into()),
    });
    engine.persistence.pending_windows.insert(window);

    let _ = engine.handle_event(&mut window_store, LayoutEvent::AppClosed(window.pid));

    assert!(!engine.persistence.windows.contains_key(&window));
    assert!(!engine.persistence.pending_windows.contains(&window));
}
