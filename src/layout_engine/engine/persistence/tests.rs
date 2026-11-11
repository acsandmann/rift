use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::matcher::MatchScope;
use super::*;
use crate::actor::app::WindowInfo;
use crate::common::config::{AppWorkspaceRule, LayoutMode, WorkspaceSelector};
use crate::layout_engine::{LayoutEvent, LayoutSystemKind};
use crate::model::virtual_workspace::WorkspaceTransferReason;
use crate::model::{AppRuleResult, VirtualWorkspace};
use crate::sys::window_server::WindowServerId;

type Inventory<'a> = &'a [(&'a str, &'a [SpaceId])];

const LEGACY_V2: &str = r#"{"workspace_layouts":(map:{((901),(idx:3,version:1)):(configurations:{(width:1200,height:800):(idx:1,version:1)},active_size:(width:1200,height:800),last_saved:None),((901),(idx:1,version:1)):(configurations:{(width:1600,height:1000):(idx:2,version:1),(width:1200,height:800):(idx:1,version:1)},active_size:(width:1200,height:800),last_saved:Some((idx:1,version:1))),((901),(idx:2,version:1)):(configurations:{(width:1200,height:800):(idx:1,version:1)},active_size:(width:1200,height:800),last_saved:None),((901),(idx:4,version:1)):(configurations:{(width:1200,height:800):(idx:1,version:1)},active_size:(width:1200,height:800),last_saved:None)}),"schema_version":2,"floating":(floating_windows:[(pid:9,idx:3)],last_floating_focus:None),"floating_positions":(positions:{((901),(idx:1,version:1),(pid:9,idx:3)):(origin:(x:12.0,y:34.0),size:(width:400.0,height:300.0))}),"virtual_workspace_manager":(workspaces:[(value:None,version:0),(value:Some((name:"Main",space:(901),last_focused:Some((pid:9,idx:2)),layout_system:stack((inner:(tree:(map:(map:[(value:None,version:0),(value:Some((parent:None,prev_sibling:None,next_sibling:None,first_child:Some((idx:2,version:3)),last_child:Some((idx:3,version:1)))),version:1),(value:Some((parent:Some((idx:1,version:1)),prev_sibling:None,next_sibling:Some((idx:3,version:1)),first_child:None,last_child:None)),version:3),(value:Some((parent:Some((idx:1,version:1)),prev_sibling:Some((idx:2,version:3)),next_sibling:None,first_child:None,last_child:None)),version:1),(value:Some((parent:None,prev_sibling:None,next_sibling:None,first_child:Some((idx:6,version:3)),last_child:Some((idx:5,version:3)))),version:1),(value:Some((parent:Some((idx:4,version:1)),prev_sibling:Some((idx:6,version:3)),next_sibling:None,first_child:None,last_child:None)),version:3),(value:Some((parent:Some((idx:4,version:1)),prev_sibling:None,next_sibling:Some((idx:5,version:3)),first_child:None,last_child:None)),version:3)]),data:(selection:(nodes:[(value:None,version:0),(value:Some((selected_child:(idx:3,version:1),stop_here:false)),version:1),(value:None,version:0),(value:None,version:0),(value:Some((selected_child:(idx:6,version:3),stop_here:false)),version:1)]),layout:(info:[(value:None,version:0),(value:Some((size:0.0,total:1.0,kind:vertical_stack,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:1),(value:Some((size:0.5,total:0.0,kind:horizontal,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:3),(value:Some((size:0.5,total:0.0,kind:horizontal,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:1),(value:Some((size:0.0,total:2.0,kind:vertical_stack,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:1),(value:Some((size:1.0,total:0.0,kind:horizontal,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:3),(value:Some((size:1.0,total:0.0,kind:horizontal,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:3)]),window:(windows:[(value:None,version:0),(value:None,version:0),(value:Some((pid:9,idx:1)),version:3),(value:Some((pid:9,idx:2)),version:1),(value:None,version:0),(value:Some((pid:9,idx:2)),version:3),(value:Some((pid:9,idx:1)),version:3)],window_nodes:{(pid:9,idx:1):([(layout:(idx:1,version:1),node:(idx:2,version:3)),(layout:(idx:2,version:1),node:(idx:6,version:3))]),(pid:9,idx:2):([(layout:(idx:1,version:1),node:(idx:3,version:1)),(layout:(idx:2,version:1),node:(idx:5,version:3))])}))),layout_roots:[(value:None,version:0),(value:Some((Some((idx:1,version:1)),"layout_root")),version:1),(value:Some((Some((idx:4,version:1)),"layout_root")),version:1)]),default_orientation:perpendicular)),layout_mode:stack)),version:1),(value:Some((name:"Development",space:(901),last_focused:None,layout_system:stack((inner:(tree:(map:(map:[(value:None,version:0),(value:Some((parent:None,prev_sibling:None,next_sibling:None,first_child:None,last_child:None)),version:1)]),data:(selection:(nodes:[(value:None,version:0)]),layout:(info:[(value:None,version:0),(value:Some((size:0.0,total:0.0,kind:vertical_stack,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:1)]),window:(windows:[(value:None,version:0)],window_nodes:{}))),layout_roots:[(value:None,version:0),(value:Some((Some((idx:1,version:1)),"layout_root")),version:1)]),default_orientation:perpendicular)),layout_mode:stack)),version:1),(value:Some((name:"Communication",space:(901),last_focused:None,layout_system:stack((inner:(tree:(map:(map:[(value:None,version:0),(value:Some((parent:None,prev_sibling:None,next_sibling:None,first_child:None,last_child:None)),version:1)]),data:(selection:(nodes:[(value:None,version:0)]),layout:(info:[(value:None,version:0),(value:Some((size:0.0,total:0.0,kind:vertical_stack,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:1)]),window:(windows:[(value:None,version:0)],window_nodes:{}))),layout_roots:[(value:None,version:0),(value:Some((Some((idx:1,version:1)),"layout_root")),version:1)]),default_orientation:perpendicular)),layout_mode:stack)),version:1),(value:Some((name:"Utilities",space:(901),last_focused:None,layout_system:stack((inner:(tree:(map:(map:[(value:None,version:0),(value:Some((parent:None,prev_sibling:None,next_sibling:None,first_child:None,last_child:None)),version:1)]),data:(selection:(nodes:[(value:None,version:0)]),layout:(info:[(value:None,version:0),(value:Some((size:0.0,total:0.0,kind:vertical_stack,last_ungrouped_kind:horizontal,is_fullscreen:false,is_fullscreen_within_gaps:false)),version:1)]),window:(windows:[(value:None,version:0)],window_nodes:{}))),layout_roots:[(value:None,version:0),(value:Some((Some((idx:1,version:1)),"layout_root")),version:1)]),default_orientation:perpendicular)),layout_mode:stack)),version:1)],workspaces_by_space:{(901):[(idx:1,version:1),(idx:2,version:1),(idx:3,version:1),(idx:4,version:1)]},active_workspace_per_space:{(901):(None,(idx:1,version:1))},workspace_counter:1),"space_display_map":{},"display_last_space":{},"persisted_windows":{(pid:9,idx:2):(window_server_id:Some(2),title:Some("WindowId { pid: 9, idx: 2 }"),width:400.0,height:300.0,app_id:Some("test.app")),(pid:9,idx:1):(window_server_id:Some(1),title:Some("WindowId { pid: 9, idx: 1 }"),width:400.0,height:300.0,app_id:Some("test.app")),(pid:9,idx:3):(window_server_id:Some(3),title:Some("WindowId { pid: 9, idx: 3 }"),width:400.0,height:300.0,app_id:Some("test.app"))},"saved_active_space":None}"#;

fn win(pid: pid_t, idx: u32) -> WindowId { WindowId::new(pid, idx) }

fn ws(engine: &LayoutEngine, space: SpaceId, index: usize) -> VirtualWorkspaceId {
    engine.workspaces.workspace_ids(space)[index]
}

fn evidence(window: WindowId) -> WindowFingerprint {
    WindowFingerprint {
        window_server_id: Some(10_000 + window.idx.get()),
        title: Some(format!("window {}", window.idx.get())),
        width: 800.0,
        height: 600.0,
        app_id: Some(format!("com.example.app{}", window.pid)),
    }
}

fn live_state(window: WindowId) -> WindowState {
    let saved = evidence(window);
    let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(saved.width, saved.height));
    WindowState {
        native_transient: false,
        info: WindowInfo {
            has_native_tabs: false,
            is_standard: true,
            is_root: true,
            is_minimized: false,
            is_resizable: true,
            min_size: None,
            max_size: None,
            title: saved.title.unwrap(),
            frame,
            sys_id: saved.window_server_id.map(WindowServerId::new),
            bundle_id: saved.app_id,
            path: None,
            ax_role: None,
            ax_subrole: None,
        },
        presented_frame: frame,
        is_manageable: true,
        manage_override: None,
    }
}

fn fresh(
    settings: &VirtualWorkspaceSettings,
    spaces: &[(SpaceId, &str)],
) -> (LayoutEngine, WindowStore) {
    let mut engine = LayoutEngine::new(settings, &LayoutSettings::default(), None);
    let mut store = WindowStore::default();
    let all: Vec<_> = spaces.iter().map(|(space, _)| *space).collect();
    engine.workspaces.ensure_visible_spaces(&all, all.len(), |_| None);
    for (space, display) in spaces {
        let size = CGSize::new(1200.0, 800.0);
        let _ = engine.handle_event(&mut store, LayoutEvent::SpaceExposed(*space, size));
        engine.update_space_display(*space, Some((*display).into()));
    }
    (engine, store)
}

fn open(
    engine: &mut LayoutEngine,
    store: &mut WindowStore,
    id: VirtualWorkspaceId,
    window: WindowId,
) {
    let space = engine.workspaces[id].space;
    store.insert_window(window, live_state(window));
    assert!(engine.workspaces.assign_window_to_workspace(store, space, window, id));
    engine.add_window_to_layout(store, space, window);
}

/// A file saved by a session with `windows` tiled on each Space's first workspace.
fn session(settings: &VirtualWorkspaceSettings, spaces: &[(SpaceId, &str, &[WindowId])]) -> String {
    let displays: Vec<_> = spaces.iter().map(|(space, display, _)| (*space, *display)).collect();
    let (mut engine, mut store) = fresh(settings, &displays);
    for (space, _, windows) in spaces {
        let id = ws(&engine, *space, 0);
        for window in *windows {
            open(&mut engine, &mut store, id, *window);
        }
    }
    engine.serialize_to_string(&store)
}

fn boot(settings: &VirtualWorkspaceSettings, saved: &str) -> (LayoutEngine, WindowStore) {
    let mut engine = LayoutEngine::deserialize_from_str(saved).unwrap();
    engine.await_topology();
    engine.finish_loading(settings, &LayoutSettings::default(), None).unwrap();
    (engine, WindowStore::default())
}

fn resolve(
    engine: &mut LayoutEngine,
    store: &mut WindowStore,
    current: &[(SpaceId, &str)],
    inventory: Inventory<'_>,
) {
    let size = CGSize::new(1200.0, 800.0);
    let current: Vec<_> = current.iter().map(|(s, d)| (*s, (*d).to_owned(), size)).collect();
    let inventory = inventory.iter().map(|(d, s)| ((*d).to_owned(), s.to_vec())).collect();
    engine.resolve_saved_spaces(store, &current, current.len(), &inventory);
}

fn observe(
    engine: &mut LayoutEngine,
    store: &mut WindowStore,
    space: SpaceId,
    window: WindowId,
    live: &WindowFingerprint,
) -> bool {
    engine.is_restoring()
        && engine.reconcile_saved_window(store, window, live, MatchScope::Space(space))
}

fn restore(
    engine: &mut LayoutEngine,
    store: &mut WindowStore,
    file: &str,
    scope: RestoreScope,
    active_space: SpaceId,
    source: RestoreSource,
) -> anyhow::Result<RestoreReport> {
    restore_on(engine, store, file, scope, active_space, source, &[active_space])
}

fn restore_on(
    engine: &mut LayoutEngine,
    store: &mut WindowStore,
    file: &str,
    scope: RestoreScope,
    active_space: SpaceId,
    source: RestoreSource,
    visible: &[SpaceId],
) -> anyhow::Result<RestoreReport> {
    let path = std::env::temp_dir().join(format!(
        "rift-persistence-test-{}-{}.ron",
        std::process::id(),
        SAVE_TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed),
    ));
    fs::write(&path, file).unwrap();
    let request = RestoreRequest { scope, active_space, source };
    let result =
        engine.restore_layout(path.clone(), request, visible, store, &LayoutSettings::default());
    let _ = fs::remove_file(path);
    result
}

fn tree(engine: &LayoutEngine, id: VirtualWorkspaceId) -> Vec<WindowId> {
    let entry = &engine.workspaces[id];
    (entry.layout_state.all_layouts())
        .flat_map(|layout| entry.layout_system.all_windows_in_layout(layout))
        .collect()
}

fn in_any_tree(engine: &LayoutEngine, window: WindowId) -> bool {
    engine.workspaces.catalog().any(|id| tree(engine, id).contains(&window))
}

fn pending(engine: &LayoutEngine, window: WindowId) -> bool {
    engine.persistence.pending.contains_key(&window)
}

fn with_active_space(file: String, space: SpaceId) -> String {
    file.replace(
        r#""saved_active_space":None"#,
        &format!(r#""saved_active_space":Some({})"#, space.get()),
    )
}

/// A file carrying raw floating state, as older versions wrote it.
fn with_floating(
    file: String,
    marked: &[WindowId],
    frames: &[(VirtualWorkspaceId, WindowId, CGRect)],
) -> String {
    let mut floating = FloatingManager::new();
    marked.iter().for_each(|window| floating.add_floating(*window));
    let mut positions = FloatingPositionStore::default();
    frames
        .iter()
        .for_each(|(id, window, frame)| positions.store(*id, *window, *frame));
    file.replacen(
        r#""floating":(floating_windows:[],last_floating_focus:None),"floating_positions":(positions:{})"#,
        &format!(
            r#""floating":{},"floating_positions":{}"#,
            ron::ser::to_string(&floating).unwrap(),
            ron::ser::to_string(&positions).unwrap(),
        ),
        1,
    )
}

fn assert_rejected(file: &str, message: &str) {
    let error = LayoutEngine::deserialize_from_str(file).err().expect("file must be rejected");
    assert!(error.to_string().contains(message), "{error}");
}

/// One run through everything persistence promises, in the order a layout file meets it.
#[test]
fn persistence_preserves_layouts_through_every_lifecycle() {
    let per_display = VirtualWorkspaceSettings::default();
    let shared = VirtualWorkspaceSettings {
        mode: WorkspaceMode::Global,
        default_workspace_count: 2,
        workspace_names: vec!["One".into(), "Two".into()],
        ..Default::default()
    };
    let spaces: Vec<_> = (0..12).map(|index| SpaceId::new(900 + index)).collect();
    let (a, b, c, d) = (spaces[0], spaces[1], spaces[2], spaces[3]);
    let (w1, w2, w3) = (win(7, 1), win(7, 2), win(7, 3));
    let other = win(8, 4);
    let frame = CGRect::new(CGPoint::new(30.0, 40.0), CGSize::new(500.0, 400.0));
    let settings = LayoutSettings::default();

    // Format: every layout system keeps its exact structure, in files of every age.
    for (mode, tag) in [
        (LayoutMode::Traditional, "traditional"),
        (LayoutMode::Bsp, "bsp"),
        (LayoutMode::Stack, "stack"),
        (LayoutMode::MasterStack, "master_stack"),
        (LayoutMode::Scrolling, "scrolling"),
    ] {
        let (mut engine, mut store) = fresh(&per_display, &[(a, "x")]);
        let id = ws(&engine, a, 0);
        let _ = engine.switch_workspace_layout_mode(&store, a, id, mode);
        [w1, w2, w3].iter().for_each(|w| open(&mut engine, &mut store, id, *w));
        let saved = engine.serialize_to_string(&store);
        let loaded = LayoutEngine::deserialize_from_str(&saved).unwrap();
        let ron_of = |engine: &LayoutEngine| ron::ser::to_string(&engine.workspaces[id]).unwrap();
        assert_eq!(ron_of(&loaded), ron_of(&engine));

        let system = VirtualWorkspace::create_layout_system(mode, &settings);
        let current = ron::ser::to_string(&system).unwrap();
        let prefix = format!("{tag}((");
        let legacy = format!("(kind:\"{tag}\",{})", &current[prefix.len()..current.len() - 2]);
        let migrated = super::snapshot::migrate_legacy_layout_system_tags(&legacy).unwrap();
        let _: LayoutSystemKind = ron::from_str(&migrated).unwrap();
    }
    let empty = session(&per_display, &[(a, "x", &[])]);
    LayoutEngine::deserialize_from_str(&empty.replacen("\"schema_version\":5,", "", 1)).unwrap();
    assert_rejected(
        &empty.replacen("\"schema_version\":5", "\"schema_version\":6", 1),
        "newer than supported",
    );
    for version in [None, Some(1), Some(2)] {
        let header = version.map(|v| format!("\"schema_version\":{v},")).unwrap_or_default();
        let old = LEGACY_V2.replacen("\"schema_version\":2,", &header, 1);
        let mut loaded = LayoutEngine::deserialize_from_str(&old).unwrap();
        let id = loaded.workspaces.active_workspace(SpaceId::new(901)).unwrap();
        assert_eq!(loaded.workspaces[id].layout_mode, LayoutMode::Stack);
        assert_eq!(loaded.workspaces[id].layout_state.all_layouts().count(), 2);
        assert_eq!(loaded.workspaces[id].last_focused(), Some(win(9, 2)));
        loaded.workspaces[id].ensure_layout_for_size(CGSize::new(1600.0, 1000.0));
        assert_eq!(loaded.workspaces[id].layout_state.all_layouts().count(), 1);
        let canonical = loaded.serialize_to_string(&WindowStore::default());
        assert!(!canonical.contains("\"workspace_layouts\":"));
        let reloaded = LayoutEngine::deserialize_from_str(&canonical).unwrap();
        assert_eq!(reloaded.persistence.pending[&win(9, 3)].workspace, id);
        assert_eq!(
            reloaded.persistence.pending[&win(9, 3)].frame,
            Some(CGRect::new(CGPoint::new(12.0, 34.0), CGSize::new(400.0, 300.0))),
        );
    }
    let (mut engine, _) = fresh(&shared, &[(a, "x"), (b, "y")]);
    let moved = engine.workspaces.active_workspace(a).unwrap();
    engine
        .workspaces
        .transfer_workspace(moved, b, None, WorkspaceTransferReason::DisplayDeparture);
    let replacement = engine.workspaces.active_workspace(a);
    let current = ron::ser::to_string(&engine.workspaces.active_workspace_per_space).unwrap();
    let old = ron::ser::to_string(&HashMap::from_iter([
        (a, (None::<VirtualWorkspaceId>, moved)),
        (b, (None, engine.workspaces.active_workspace(b).unwrap())),
    ]))
    .unwrap();
    let file = engine.serialize_to_string(&WindowStore::default()).replacen(&current, &old, 1);
    let loaded = LayoutEngine::deserialize_from_str(&file).unwrap();
    assert_eq!(loaded.workspaces.active_workspace(a), replacement);
    assert_eq!(
        loaded.workspaces.active_workspace_per_space[&a].remembered,
        Some(moved)
    );

    // Validation: a bad file fails whole and leaves the live engine exactly as it was.
    let good = session(&per_display, &[(a, "x", &[w1])]);
    let id = LayoutEngine::deserialize_from_str(&good).unwrap().workspaces.workspace_ids(a)[0];
    let negative = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(-1.0, 1.0));
    assert_rejected(
        &good.replacen("width:800.0", "width:-1.0", 1),
        "invalid persisted size",
    );
    assert_rejected(
        &with_floating(good.clone(), &[], &[(id, w1, negative)]),
        "invalid floating frame",
    );
    assert_rejected(
        &LEGACY_V2.replacen("((901),", "((902),", 1),
        "invalid workspace layouts",
    );
    assert_rejected(
        &LEGACY_V2.replacen("(idx:3,version:1)", "(idx:999999,version:1)", 1),
        "invalid workspace layouts",
    );
    let layout = good.find("layout:Some((idx:").unwrap();
    let version = layout + good[layout..].find("version:1").unwrap();
    let mut dangling = good.clone();
    dangling.replace_range(version..version + 9, "version:9999");
    assert_rejected(&dangling, "references missing layout");
    let (mut engine, mut store) = boot(&per_display, &good);
    resolve(&mut engine, &mut store, &[(a, "x")], &[("x", &[a])]);
    let before = engine.serialize_to_string(&store);
    for bad in [
        good[..good.len() - 20].to_owned(),
        good.replacen("\"schema_version\":5", "\"schema_version\":99", 1),
        good.replacen("width:800.0", "width:-1.0", 1),
    ] {
        for scope in [RestoreScope::Space, RestoreScope::Workspace] {
            let source = RestoreSource::CurrentSpace;
            assert!(restore(&mut engine, &mut store, &bad, scope, a, source).is_err());
            assert_eq!(engine.serialize_to_string(&store), before);
        }
    }

    // Loading gives each saved window one location; disagreeing or unmatchable state goes.
    let (mut engine, mut store) = fresh(&per_display, &[(a, "x")]);
    let (first, second) = (ws(&engine, a, 0), ws(&engine, a, 1));
    for window in [w1, w2, w3, other] {
        open(&mut engine, &mut store, first, window);
    }
    let layout = engine.workspaces[second].active_layout().unwrap();
    engine.workspaces[second].layout_system.add_window_after_selection(layout, w1);
    let ghost = win(9, 9);
    let tiled_ghost = engine.workspaces[first].active_layout().unwrap();
    engine.workspaces[first]
        .layout_system
        .add_window_after_selection(tiled_ghost, ghost);
    engine.workspaces.set_last_focused_window(a, second, Some(ghost));
    let orphan = format!(
        r#"{}:{}"#,
        ron::ser::to_string(&win(9, 10)).unwrap(),
        ron::ser::to_string(&evidence(win(9, 10))).unwrap(),
    );
    let file = with_floating(engine.serialize_to_string(&store), &[w2, w3], &[
        (first, w3, frame),
        (second, w3, frame),
        (first, other, frame),
    ])
    .replacen(
        r#""persisted_windows":{"#,
        &format!(r#""persisted_windows":{{{orphan},"#),
        1,
    );
    let loaded = LayoutEngine::deserialize_from_str(&file).unwrap();
    let saved = &loaded.persistence.pending;
    assert_eq!(tree(&loaded, second), [], "a duplicate keeps its first location");
    assert_eq!(saved[&w1].workspace, first);
    assert!(
        saved[&w2].frame.is_none() && in_any_tree(&loaded, w2),
        "a marker needs a frame"
    );
    assert_eq!((saved[&w3].frame, saved[&w3].workspace), (Some(frame), first));
    assert!(
        !in_any_tree(&loaded, w3),
        "an agreed floating window holds no slot"
    );
    assert!(
        saved[&other].frame.is_none() && in_any_tree(&loaded, other),
        "a frame needs a marker"
    );
    assert!(
        !in_any_tree(&loaded, ghost) && !pending(&loaded, ghost),
        "no fingerprint, no slot"
    );
    assert!(!pending(&loaded, win(9, 10)), "no location, no candidate");
    assert_eq!(loaded.workspaces.last_focused_window(a, second), None);

    // Saving observes: repeating it changes nothing, and a parked frame is not a placement.
    let (mut engine, mut store) = fresh(&per_display, &[(a, "x")]);
    engine.workspaces.ensure_space_initialized(d);
    let (shown, hidden) = (ws(&engine, a, 0), ws(&engine, a, 1));
    open(&mut engine, &mut store, shown, w1);
    let minimized = win(7, 5);
    open(&mut engine, &mut store, shown, minimized);
    for (id, window) in [(shown, w2), (hidden, w3)] {
        store.insert_window(window, live_state(window));
        assert!(engine.workspaces.assign_window_to_workspace(&mut store, a, window, id));
        engine.floating.add_floating(window);
    }
    engine.floating_positions.store(hidden, w3, frame);
    engine.floating_positions.store(shown, w1, frame);
    store.window_mut(w3).unwrap().presented_frame.origin.x = 99_999.0;
    store.observe_minimized(minimized);
    let _ = engine.handle_event(&mut store, LayoutEvent::WindowSuspended(minimized));
    let path = std::env::temp_dir().join(format!("rift-save-test-{}.ron", std::process::id()));
    engine.save_current_layout(path.clone(), &store, Some(a)).unwrap();
    let file = fs::read_to_string(&path).unwrap();
    let _ = fs::remove_file(path);
    assert_eq!(with_active_space(engine.serialize_to_string(&store), a), file);
    assert_eq!(engine.floating_positions.get(hidden, w3), Some(frame));
    let loaded = LayoutEngine::deserialize_from_str(&file).unwrap();
    let saved = &loaded.persistence.pending;
    assert_eq!(
        saved[&w1].frame, None,
        "a tiled window's stale frame stays behind"
    );
    assert_eq!(saved[&w2].frame, Some(store.window(w2).unwrap().presented_frame));
    assert_eq!((saved[&w3].frame, saved[&w3].workspace), (Some(frame), hidden));
    assert!(
        saved.contains_key(&minimized),
        "a minimized live window retains its canonical saved slot"
    );
    assert!(
        !loaded.workspaces.workspace_ids(d).is_empty(),
        "a never-shown Space still saves"
    );

    // Matching: the strongest evidence wins, and nothing is guessed.
    let saved_file = session(&per_display, &[(a, "x", &[w1, w2, w3])]);
    let start = || {
        let (mut engine, mut store) = boot(&per_display, &saved_file);
        resolve(&mut engine, &mut store, &[(a, "x")], &[("x", &[a])]);
        (engine, store)
    };
    let (ref mut engine, ref mut store) = start();
    assert!(
        observe(engine, store, a, win(70, 1), &evidence(w1)),
        "a server id survives rekeying"
    );
    let id = ws(engine, a, 0);
    assert_eq!(
        tree(engine, id),
        [win(70, 1), w2, w3],
        "the slot is kept exactly"
    );
    assert_eq!(store.workspace_for_window(win(70, 1)), Some(id));
    let other_app = WindowFingerprint {
        app_id: Some("com.example.other".into()),
        ..evidence(w2)
    };
    assert!(
        !observe(engine, store, a, win(71, 1), &other_app),
        "never across applications"
    );
    assert!(pending(engine, w2));
    // A reused process-local id names another window: it may not inherit that slot.
    let reused = WindowFingerprint {
        window_server_id: Some(1),
        ..evidence(win(7, 50))
    };
    assert!(!observe(engine, store, a, w3, &reused));
    assert!(!pending(engine, w3) && !in_any_tree(engine, w3));

    let (mut engine, mut store) = start();
    let restarted = |window: WindowId, width: f64| WindowFingerprint {
        window_server_id: None,
        width,
        ..evidence(window)
    };
    assert!(!observe(
        &mut engine,
        &mut store,
        a,
        win(70, 1),
        &WindowFingerprint {
            title: Some("Preferences".into()),
            ..restarted(w1, 800.0)
        }
    ));
    assert!(
        observe(&mut engine, &mut store, a, win(70, 2), &restarted(w1, 300.0)),
        "unique title"
    );
    for window in [w2, w3] {
        engine.persistence.pending.get_mut(&window).unwrap().fingerprint.title =
            Some("same".into());
    }
    let same = |width| WindowFingerprint {
        title: Some("same".into()),
        ..restarted(w2, width)
    };
    assert!(
        !observe(&mut engine, &mut store, a, win(70, 3), &same(800.0)),
        "an exact tie"
    );
    engine.persistence.pending.get_mut(&w3).unwrap().fingerprint.width = 500.0;
    assert!(
        observe(&mut engine, &mut store, a, win(70, 4), &same(520.0)),
        "the closest size"
    );
    assert!(pending(&engine, w2) && !pending(&engine, w3));

    let (mut engine, mut store) = start();
    engine.persistence.pending.get_mut(&w2).unwrap().fingerprint = evidence(w1);
    let id = ws(&engine, a, 0);
    assert!(observe(&mut engine, &mut store, a, win(70, 1), &evidence(w1)));
    assert!(
        engine.persistence.pending.len() == 1 && !in_any_tree(&engine, w2),
        "duplicates heal"
    );
    assert_eq!(tree(&engine, id), [win(70, 1), w3]);

    // Lifecycle: discovery, closing and app exit each retire what they prove stale.
    let (mut engine, mut store) = start();
    let id = ws(&engine, a, 0);
    assert!(observe(&mut engine, &mut store, a, w1, &evidence(w1)));
    let _ = engine.handle_event(&mut store, LayoutEvent::WindowRemoved(w2));
    assert!(!pending(&engine, w2) && !in_any_tree(&engine, w2));
    let finished = LayoutEvent::WindowDiscoveryCompleted(w3.pid, None, vec![b]);
    let _ = engine.handle_event(&mut store, finished);
    assert!(pending(&engine, w3), "another Space's discovery proves nothing");
    let finished = LayoutEvent::WindowDiscoveryCompleted(w3.pid, None, vec![a]);
    assert!(engine.handle_event(&mut store, finished).response.changed);
    assert_eq!(
        tree(&engine, id),
        [w1],
        "no ghost, and survivors keep their place"
    );
    let (mut engine, mut store) = start();
    let _ = engine.handle_event(&mut store, LayoutEvent::AppClosed(w1.pid));
    assert!(engine.persistence.pending.is_empty() && !in_any_tree(&engine, w1));
    let (mut engine, _) = start();
    let running = engine.discard_unmatchable_startup_candidates(
        |window, _| window == w1,
        |app_id| app_id == "nothing running",
    );
    assert_eq!(running, 2, "a live server id alone keeps a candidate");
    assert!(pending(&engine, w1));
    let (mut engine, _) = start();
    let mut lookups = 0;
    engine.discard_unmatchable_startup_candidates(
        |_, _| panic!("needless query"),
        |_| {
            lookups += 1;
            true
        },
    );
    assert_eq!(lookups, 1, "one lookup per application");

    // A restored window keeps its workspace over an app rule that would place a new one.
    let ruled = VirtualWorkspaceSettings {
        app_rules: vec![AppWorkspaceRule {
            app_id: evidence(w1).app_id,
            workspace: Some(WorkspaceSelector::Index(1)),
            manage: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    let (mut engine, mut store) = boot(&ruled, &saved_file);
    resolve(&mut engine, &mut store, &[(a, "x")], &[("x", &[a])]);
    let id = ws(&engine, a, 0);
    store.insert_window(w1, live_state(w1));
    let title = evidence(w1).title;
    let result = engine.assign_window_with_app_info(
        &mut store,
        w1,
        a,
        crate::model::WindowRuleContext {
            app_bundle_id: evidence(w1).app_id.as_deref(),
            app_name: None,
            window_title: title.as_deref(),
            ax_role: None,
            ax_subrole: None,
        },
        false,
    );
    assert!(matches!(result, Ok(AppRuleResult::Managed(effects)) if effects.workspace_id == id));

    // Topology: a saved Space is nobody's destination until the inventory proves it.
    let (mut engine, mut store) = boot(&per_display, &session(&per_display, &[(a, "x", &[w1])]));
    let saved_ws = ws(&engine, a, 0);
    assert!(!observe(&mut engine, &mut store, c, w1, &evidence(w1)));
    let _ = engine.handle_event(
        &mut store,
        LayoutEvent::WindowDiscoveryCompleted(w1.pid, None, vec![a]),
    );
    assert!(
        pending(&engine, w1),
        "evidence seen before topology is not consumed"
    );
    engine.resolve_saved_spaces(&mut store, &[], 1, &HashMap::default());
    resolve(&mut engine, &mut store, &[(c, "x")], &[("x", &[a])]);
    assert!(
        engine.persistence.awaiting_topology,
        "a contradictory inventory proves nothing"
    );
    engine.update_space_display(c, Some("x".into()));
    resolve(&mut engine, &mut store, &[(c, "x")], &[("x", &[c])]);
    assert_eq!(
        engine.workspaces[saved_ws].space, c,
        "the display's evidence survived the wait"
    );
    assert!(
        observe(&mut engine, &mut store, c, w1, &evidence(w1)),
        "re-observation adopts"
    );
    resolve(&mut engine, &mut store, &[(d, "x")], &[("x", &[d])]);
    assert_eq!(
        engine.workspaces[saved_ws].space, c,
        "only unresolved Spaces ever remap"
    );

    // A Space that is gone reserves nothing, even for an identical window of a running app.
    let file = session(&per_display, &[(a, "x", &[w1]), (b, "x", &[])]);
    let (mut engine, mut store) = boot(&per_display, &file);
    resolve(&mut engine, &mut store, &[(b, "x")], &[("x", &[b])]);
    assert!(engine.persistence.pending.is_empty() && !in_any_tree(&engine, w1));
    assert!(engine.workspaces.workspace_ids(a).is_empty());
    assert!(!engine.is_restoring());
    LayoutEngine::deserialize_from_str(&engine.serialize_to_string(&store)).unwrap();

    // A Space that still exists keeps its layout while hidden, against identical live windows.
    let file = session(&per_display, &[(b, "x", &[]), (a, "x", &[w1, w2])]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let hidden = ws(&engine, a, 0);
    resolve(&mut engine, &mut store, &[(b, "x")], &[("x", &[a, b])]);
    assert_eq!(
        engine.workspaces[hidden].space, a,
        "switching desktops renames nothing"
    );
    let lookalike = WindowFingerprint {
        window_server_id: Some(1),
        ..evidence(w1)
    };
    assert!(!observe(&mut engine, &mut store, b, win(7, 9), &lookalike));
    let _ = engine.handle_event(
        &mut store,
        LayoutEvent::WindowDiscoveryCompleted(w1.pid, None, vec![b]),
    );
    assert_eq!(engine.persistence.pending.len(), 2);
    let restarted = WindowFingerprint {
        window_server_id: None,
        ..evidence(w1)
    };
    assert!(observe(&mut engine, &mut store, a, win(70, 1), &restarted));
    assert_eq!(tree(&engine, hidden), [win(70, 1), w2]);
    // Exact evidence on another Space leaves the window there and frees the slot.
    assert!(!observe(&mut engine, &mut store, b, w2, &evidence(w2)));
    assert_eq!(tree(&engine, hidden), [win(70, 1)]);

    // A display that is away keeps its state through changes, and resumes when it returns.
    let file = session(&per_display, &[(a, "x", &[]), (b, "away", &[w1, w2, w3])]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let away = ws(&engine, b, 0);
    resolve(&mut engine, &mut store, &[(a, "x")], &[("x", &[a])]);
    assert!(!engine.is_restoring() || engine.saved_space_is_unresolved(Some(b)));
    let lookalike = WindowFingerprint {
        window_server_id: None,
        ..evidence(w1)
    };
    assert!(
        !observe(&mut engine, &mut store, a, win(70, 1), &lookalike),
        "no stolen windows"
    );
    // WindowServer can still name a departed desktop as a carried-off window's Space.
    assert!(
        !observe(&mut engine, &mut store, b, win(70, 1), &lookalike),
        "not a destination"
    );
    assert!(
        !observe(&mut engine, &mut store, a, w2, &evidence(w2)),
        "carried off, it stays"
    );
    let _ = engine.handle_event(&mut store, LayoutEvent::WindowRemoved(w3));
    let during = LayoutEngine::deserialize_from_str(&engine.serialize_to_string(&store)).unwrap();
    assert_eq!(during.persistence.pending[&w1].workspace, away);
    resolve(&mut engine, &mut store, &[(a, "x"), (d, "away")], &[
        ("x", &[a]),
        ("away", &[d]),
    ]);
    assert!(!engine.saved_space_is_unresolved(Some(b)));
    assert_eq!(engine.workspaces[away].space, d);
    assert!(observe(&mut engine, &mut store, d, w1, &evidence(w1)));
    assert_eq!(tree(&engine, away), [w1]);

    // Id churn: display identity remaps only when it is unambiguous and overwrites nothing.
    let names = |space, display| (space, display, &[] as &[WindowId]);
    let file = session(&per_display, &[names(a, "x"), names(b, "y")]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let (on_a, on_b) = (ws(&engine, a, 0), ws(&engine, b, 0));
    resolve(&mut engine, &mut store, &[(b, "x"), (a, "y")], &[
        ("x", &[b]),
        ("y", &[a]),
    ]);
    assert_eq!(
        (engine.workspaces[on_a].space, engine.workspaces[on_b].space),
        (a, b),
        "a main-display change moves desktops, it doesn't rename them"
    );

    let (mut engine, mut store) = boot(&per_display, &session(&per_display, &[names(a, "x")]));
    let saved_ws = ws(&engine, a, 0);
    engine.workspaces.ensure_space_initialized(c);
    resolve(&mut engine, &mut store, &[(c, "x")], &[("x", &[c])]);
    assert_eq!(ws(&engine, c, 0), saved_ws, "auto-created workspaces make way");

    let file = session(&per_display, &[names(a, "x"), names(b, "x")]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let kept = ws(&engine, b, 0);
    engine.display_last_space.insert("x".into(), a);
    resolve(&mut engine, &mut store, &[(b, "x")], &[("x", &[b])]);
    assert_eq!(
        ws(&engine, b, 0),
        kept,
        "a destination with saved state is not overwritten"
    );
    assert!(engine.workspaces.workspace_ids(a).is_empty());

    let file = session(&per_display, &[names(a, "x"), names(b, "y"), names(d, "y")]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let kept = ws(&engine, d, 0);
    engine.space_display_map.remove(&b);
    engine.workspaces.workspace_ids(b).iter().for_each(|id| {
        engine.workspaces[*id].preferred_display = None;
    });
    engine.display_last_space.insert("x".into(), a);
    engine.display_last_space.insert("y".into(), a);
    resolve(&mut engine, &mut store, &[(c, "x"), (d, "y")], &[
        ("x", &[c]),
        ("y", &[c, d]),
    ]);
    assert!(
        engine.workspaces.workspace_ids(a).is_empty(),
        "two claimants prove nothing"
    );
    assert!(
        engine.workspaces.workspace_ids(b).is_empty(),
        "nor does a missing identity"
    );
    assert!(engine.workspaces.workspace_ids(c).is_empty());
    assert_eq!(ws(&engine, d, 0), kept);

    // Remapping over a workspace leaves nothing that still names it.
    let (mut engine, mut store) = fresh(&per_display, &[(a, "x"), (b, "y")]);
    let replaced = ws(&engine, b, 0);
    engine.floating_positions.store(replaced, w1, frame);
    engine.remap_space(&mut store, a, b);
    assert_eq!(engine.floating_positions.get(replaced, w1), None);

    // Shared mode: the catalog is global, so a lost desktop's workspaces stay reachable.
    let file = session(&shared, &[(a, "x", &[w1]), (b, "away", &[other])]);
    let (mut engine, mut store) = boot(&shared, &file);
    let catalog: Vec<_> = engine.workspaces.catalog().collect();
    let lost = engine.persistence.pending[&other].workspace;
    let dormant = engine.persistence.pending[&w1].workspace;
    resolve(&mut engine, &mut store, &[(c, "x")], &[("x", &[c, a])]);
    assert_eq!(engine.workspaces.catalog().collect::<Vec<_>>(), catalog);
    assert_eq!(
        engine.workspaces[dormant].space, a,
        "a dormant workspace on a live Space stays"
    );
    assert_eq!(engine.workspaces[lost].space, c);
    assert!(observe(&mut engine, &mut store, c, other, &evidence(other)));
    assert_eq!(store.workspace_for_window(other), Some(lost));
    LayoutEngine::deserialize_from_str(&engine.serialize_to_string(&store)).unwrap();

    // Restore: matched windows take saved slots, current windows stay, the rest leave no trace.
    let portable = with_active_space(
        session(&per_display, &[(a, "x", &[w1, w2]), (b, "y", &[other])]),
        a,
    );
    let (mut engine, mut store) = fresh(&per_display, &[(c, "z")]);
    let target = ws(&engine, c, 0);
    let name = engine.workspaces[target].name.clone();
    let absent = win(9, 1);
    let outsider = win(7, 2);
    open(&mut engine, &mut store, target, absent);
    open(&mut engine, &mut store, target, w1);
    let sibling = ws(&engine, c, 1);
    open(&mut engine, &mut store, sibling, outsider);
    let report = restore(
        &mut engine,
        &mut store,
        &portable,
        RestoreScope::Workspace,
        c,
        RestoreSource::SavedActiveSpace,
    )
    .unwrap();
    assert_eq!(
        (report.matched, report.unmatched, report.workspaces_replaced),
        (1, 1, 1)
    );
    assert_eq!(tree(&engine, target), [w1, absent]);
    assert_eq!(
        tree(&engine, sibling),
        [outsider],
        "a sibling's live window is not consumed"
    );
    assert_eq!(
        engine.workspaces[target].name, name,
        "contents replace, identity stays"
    );
    assert_eq!(engine.workspaces[target].space, c);
    assert!(engine.persistence.pending.is_empty());

    // Scoped: another Space's windows, pending restores and caches are all left alone.
    let file = with_active_space(session(&per_display, &[(a, "x", &[other]), (b, "y", &[w1])]), b);
    let (mut engine, mut store) = boot(&per_display, &file);
    resolve(&mut engine, &mut store, &[(a, "x"), (b, "y")], &[
        ("x", &[a]),
        ("y", &[b]),
    ]);
    let untouched = ws(&engine, a, 0);
    let before = ron::ser::to_string(&engine.workspaces[untouched]).unwrap();
    let cached = VirtualWorkspace::create_layout_system(LayoutMode::Traditional, &settings);
    engine.native_layouts.insert(untouched, (cached, Default::default()));
    let target = ws(&engine, b, 0);
    open(&mut engine, &mut store, target, w1);
    let scope = RestoreScope::Workspace;
    let source = RestoreSource::SavedActiveSpace;
    assert_eq!(
        restore(&mut engine, &mut store, &file, scope, b, source).unwrap().matched,
        1
    );
    assert_eq!(
        ron::ser::to_string(&engine.workspaces[untouched]).unwrap(),
        before
    );
    assert!(pending(&engine, other) && engine.native_layouts.contains_key(&untouched));

    // Source selection: the saved origin, then the display, and never a guess or a partial space.
    let two_spaces = session(&per_display, &[(a, "x", &[]), (b, "y", &[w1])]);
    let (mut engine, mut store) = fresh(&per_display, &[(c, "z")]);
    let before = engine.serialize_to_string(&store);
    let result = restore(&mut engine, &mut store, &two_spaces, scope, c, source);
    assert!(result.unwrap_err().to_string().contains("cannot choose a source"));
    assert_eq!(engine.serialize_to_string(&store), before);

    let few = VirtualWorkspaceSettings {
        default_workspace_count: 2,
        ..Default::default()
    };
    let (mut engine, mut store) = fresh(&few, &[(c, "z")]);
    let before = engine.serialize_to_string(&store);
    let result = restore(
        &mut engine,
        &mut store,
        &portable,
        RestoreScope::Space,
        c,
        source,
    );
    assert!(result.unwrap_err().to_string().contains("different workspace counts"));
    assert_eq!(engine.serialize_to_string(&store), before);

    let (mut saved, mut saved_store) = fresh(&per_display, &[(a, "y")]);
    let ordinal = ws(&saved, a, 2);
    saved.workspaces.set_active_workspace(a, ws(&saved, a, 0));
    open(&mut saved, &mut saved_store, ordinal, w1);
    let master = saved.serialize_to_string(&saved_store);
    let (mut engine, mut store) = fresh(&per_display, &[(c, "y")]);
    let target = ws(&engine, c, 2);
    assert!(engine.workspaces.set_active_workspace(c, target));
    open(&mut engine, &mut store, target, w1);
    let source = RestoreSource::CurrentSpace;
    assert_eq!(
        restore(&mut engine, &mut store, &master, scope, c, source).unwrap().matched,
        1
    );
    assert_eq!(
        tree(&engine, target),
        [w1],
        "the old SpaceId resolved through the display"
    );

    // Every display: one master restores each display's Space in a single transaction.
    let master = session(&per_display, &[(a, "x", &[w1, w2]), (b, "y", &[w3])]);
    let loaded = LayoutEngine::deserialize_from_str(&master).unwrap();
    assert_eq!(
        loaded.persistence.pending.len(),
        3,
        "a save covers every display"
    );
    let displays = RestoreScope::Displays;
    let (mut engine, mut store) = fresh(&per_display, &[(a, "x"), (b, "y"), (c, "new")]);
    let (on_a, on_b, on_new) = (ws(&engine, a, 0), ws(&engine, b, 0), ws(&engine, c, 0));
    open(&mut engine, &mut store, on_b, w1);
    open(&mut engine, &mut store, on_a, w3);
    open(&mut engine, &mut store, on_new, other);
    let report = restore_on(&mut engine, &mut store, &master, displays, a, source, &[a, b, c]);
    let report = report.unwrap();
    assert_eq!(
        (report.matched, report.unmatched, report.workspaces_replaced),
        (2, 1, 8)
    );
    let mut moved = report.moved;
    moved.sort_unstable_by_key(|(window, ..)| *window);
    assert_eq!(moved, [(w1, b, a), (w3, a, b)], "swapped windows go home");
    assert_eq!((tree(&engine, on_a), tree(&engine, on_b)), (vec![w1], vec![w3]));
    assert_eq!(
        tree(&engine, on_new),
        [other],
        "a display the file doesn't know is left alone"
    );

    let (mut engine, mut store) = fresh(&per_display, &[(a, "x"), (b, "y")]);
    engine.workspaces.create_workspace(b, Some("extra".into())).unwrap();
    let before = engine.serialize_to_string(&store);
    let result = restore_on(&mut engine, &mut store, &master, displays, a, source, &[a, b]);
    assert!(result.unwrap_err().to_string().contains("different workspace counts"));
    assert_eq!(
        engine.serialize_to_string(&store),
        before,
        "one bad display stops them all"
    );
    let (mut engine, mut store) = fresh(&per_display, &[(c, "new")]);
    let result = restore_on(&mut engine, &mut store, &master, displays, c, source, &[c]);
    assert!(result.unwrap_err().to_string().contains("no entry for any connected display"));

    // A renumbered display is found by identity, and a shared catalog pairs by position even
    // after a workspace changed displays.
    let (mut engine, mut store) = fresh(&per_display, &[(c, "x"), (b, "y")]);
    let on_new = ws(&engine, c, 0);
    open(&mut engine, &mut store, on_new, w1);
    let report = restore_on(&mut engine, &mut store, &master, displays, c, source, &[c, b]);
    assert_eq!(report.unwrap().moved, []);
    assert_eq!(tree(&engine, on_new), [w1]);
    let master = session(&shared, &[(a, "x", &[w1]), (b, "y", &[w3])]);
    let (mut engine, mut store) = fresh(&shared, &[(a, "x"), (b, "y")]);
    let (first, second) = (ws(&engine, a, 0), ws(&engine, b, 0));
    engine
        .workspaces
        .transfer_workspace(second, a, None, WorkspaceTransferReason::Explicit);
    open(&mut engine, &mut store, first, w1);
    open(&mut engine, &mut store, second, w3);
    let report = restore_on(&mut engine, &mut store, &master, displays, a, source, &[a, b]);
    assert_eq!(report.unwrap().matched, 2);
    assert_eq!(
        (tree(&engine, first), tree(&engine, second)),
        (vec![w1], vec![w3])
    );

    // A shared catalog needs no display evidence, and a moved window is matched only once: left
    // to meet its neighbor's slot again, it would take it from the same-titled window.
    let file = session(&shared, &[(a, "gone", &[w1])]);
    let (mut engine, mut store) = fresh(&shared, &[(c, "new"), (d, "second")]);
    let report = restore_on(&mut engine, &mut store, &file, displays, c, source, &[c, d]);
    assert_eq!(report.unwrap().matched, 0);
    assert!(!in_any_tree(&engine, w1) && engine.persistence.pending.is_empty());

    let terminal = |store: &mut WindowStore, window: WindowId, title: &str| {
        let mut state = live_state(window);
        state.info.title = title.into();
        state.info.bundle_id = Some("com.example.terminal".into());
        store.insert_window(window, state);
    };
    let (mut saved, mut saved_store) = fresh(&per_display, &[(a, "x")]);
    let id = ws(&saved, a, 1);
    for (window, title) in [(w1, "zsh"), (w2, "zsh"), (w3, "logs")] {
        terminal(&mut saved_store, window, title);
        assert!(saved.workspaces.assign_window_to_workspace(&mut saved_store, a, window, id));
        saved.add_window_to_layout(&mut saved_store, a, window);
    }
    let file = saved.serialize_to_string(&saved_store);
    let (mut engine, mut store) = fresh(&per_display, &[(a, "x")]);
    let (first, second) = (ws(&engine, a, 0), ws(&engine, a, 1));
    for (window, title, id) in [
        (w1, "zsh", first),
        (w2, "zsh", second),
        (w3, "logs", second),
    ] {
        terminal(&mut store, window, title);
        assert!(engine.workspaces.assign_window_to_workspace(&mut store, a, window, id));
        engine.add_window_to_layout(&mut store, a, window);
    }
    restore(&mut engine, &mut store, &file, RestoreScope::Space, a, source).unwrap();
    assert_eq!(tree(&engine, second), [w1, w2, w3]);

    // A desktop that moved to another display keeps its slots, not just its name.
    let file = session(&per_display, &[(a, "x", &[w1, w3]), (b, "y", &[w2])]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let on_a = ws(&engine, a, 0);
    resolve(&mut engine, &mut store, &[(b, "x"), (a, "y")], &[
        ("x", &[b]),
        ("y", &[a]),
    ]);
    store.insert_window(w1, live_state(w1));
    assert!(observe(&mut engine, &mut store, a, w1, &evidence(w1)));
    assert_eq!(tree(&engine, on_a), [w1, w3]);

    // A late display's renumbering leaves a Space outside the remap alone.
    let (s3, s5, s8, s9) = (
        SpaceId::new(3),
        SpaceId::new(5),
        SpaceId::new(8),
        SpaceId::new(9),
    );
    let file = session(&per_display, &[
        (s9, "x", &[w1]),
        (s3, "x", &[]),
        (s5, "y", &[w2]),
    ]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let hidden = ws(&engine, s9, 0);
    resolve(&mut engine, &mut store, &[(s3, "x")], &[("x", &[s3, s9])]);
    resolve(&mut engine, &mut store, &[(s3, "x"), (s8, "y")], &[
        ("x", &[s3, s9]),
        ("y", &[s8]),
    ]);
    assert_eq!(tree(&engine, ws(&engine, s9, 0)), [w1]);
    assert_eq!(ws(&engine, s9, 0), hidden);
    assert_eq!(tree(&engine, ws(&engine, s8, 0)), [w2]);

    // A fullscreen display has no current Space. It blocks nothing elsewhere, keeps what its
    // inventory proves, and holds back only a Space it may yet show renumbered.
    let inventory: HashMap<String, Vec<SpaceId>> =
        [("x".to_owned(), vec![a]), ("y".to_owned(), vec![d])].into_iter().collect();
    let size = CGSize::new(1200.0, 800.0);
    let file = session(&per_display, &[(a, "x", &[w1]), (b, "y", &[w2])]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let on_y = ws(&engine, b, 0);
    let current = [(a, "x".to_owned(), size)];
    engine.resolve_saved_spaces(&mut store, &current, 2, &inventory);
    assert!(engine.is_restoring() && engine.saved_space_is_unresolved(Some(b)));
    store.insert_window(w1, live_state(w1));
    assert!(observe(&mut engine, &mut store, a, w1, &evidence(w1)));
    let current = [(a, "x".to_owned(), size), (d, "y".to_owned(), size)];
    engine.resolve_saved_spaces(&mut store, &current, 2, &inventory);
    assert_eq!(engine.workspaces[on_y].space, d);

    let file = session(&shared, &[(a, "x", &[w1]), (b, "y", &[w2])]);
    let (mut engine, mut store) = boot(&shared, &file);
    let on_y = engine.persistence.pending[&w2].workspace;
    let current = [(a, "x".to_owned(), size)];
    engine.resolve_saved_spaces(&mut store, &current, 2, &inventory);
    assert_eq!(engine.workspaces[on_y].space, b, "shared mode waits for it too");
    engine.resolve_saved_spaces(&mut store, &[], 2, &inventory);
    assert!(
        engine.saved_space_is_unresolved(Some(b)),
        "with nowhere to go, nothing is lost"
    );

    // Shared rehoming fits the workspace to the display it lands on.
    let file = session(&shared, &[(a, "x", &[w1]), (b, "away", &[other])]);
    let (mut engine, mut store) = boot(&shared, &file);
    let lost = engine.persistence.pending[&other].workspace;
    let wide = CGSize::new(1600.0, 900.0);
    let inventory: HashMap<String, Vec<SpaceId>> =
        [("x".to_owned(), vec![a])].into_iter().collect();
    engine.resolve_saved_spaces(&mut store, &[(a, "x".to_owned(), wide)], 1, &inventory);
    assert_eq!(engine.workspaces[lost].space, a);
    assert_eq!(engine.workspaces[lost].layout_state.active_size(), Some(wide));

    // Windows that arrived during the wait are not forgotten when it ends: they take their slots,
    // finished discoveries clear ghosts, and no snapshot is written from the overlap.
    let file = session(&per_display, &[(a, "x", &[w1, w2]), (b, "x", &[])]);
    let (mut engine, mut store) = boot(&per_display, &file);
    let (home, elsewhere) = (ws(&engine, a, 0), ws(&engine, a, 1));
    open(&mut engine, &mut store, elsewhere, w1);
    let _ = engine.handle_event(
        &mut store,
        LayoutEvent::WindowDiscoveryCompleted(w1.pid, None, vec![a]),
    );
    let path = std::env::temp_dir().join(format!("rift-waiting-{}.ron", std::process::id()));
    assert!(engine.save_current_layout(path.clone(), &store, None).is_err());
    assert!(!path.exists(), "the master on disk outlives the wait");
    resolve(&mut engine, &mut store, &[(a, "x")], &[("x", &[a, b])]);
    assert_eq!(store.workspace_for_window(w1), Some(home));
    assert_eq!(tree(&engine, home), [w1], "the closed window's ghost is gone");
    assert!(tree(&engine, elsewhere).is_empty());
    assert!(
        engine.persistence.pending.is_empty() && engine.persistence.awaited_discoveries.is_empty()
    );
    engine.save_current_layout(path.clone(), &store, None).unwrap();
    let _ = fs::remove_file(path);

    // Configured names apply over saved ones, and duplicates never cost a workspace.
    let mut named = VirtualWorkspaceSettings::default();
    named.workspace_names = vec!["Fresh".into()];
    let (mut engine, _) = fresh(&per_display, &[(a, "x")]);
    let first = ws(&engine, a, 0);
    assert!(engine.workspaces.rename_workspace(a, first, "Old".into()));
    let extra = engine.workspaces.create_workspace(a, Some("Fresh".into())).unwrap();
    let file = engine.serialize_to_string(&WindowStore::default());
    let (loaded, _) = boot(&named, &file);
    assert_eq!(loaded.workspaces[first].name, "Fresh");
    assert_ne!(loaded.workspaces[extra].name, "Fresh");

    // Identity transfer outside restore keeps the tree position of a live window.
    let (mut engine, mut store) = fresh(&per_display, &[(a, "x")]);
    let id = ws(&engine, a, 0);
    [w1, w2].iter().for_each(|w| open(&mut engine, &mut store, id, *w));
    engine.transfer_persistent_window_identity(w1, win(70, 1));
    assert_eq!(tree(&engine, id), [win(70, 1), w2]);
}

#[test]
fn global_single_display_restore_stays_in_the_windows_native_space() {
    let settings = VirtualWorkspaceSettings {
        mode: WorkspaceMode::Global,
        ..Default::default()
    };
    let (a, b) = (SpaceId::new(801), SpaceId::new(802));
    let window = win(80, 1);
    let file = session(&settings, &[(a, "display", &[window])]);
    for current in [a, b] {
        let (mut engine, mut store) = boot(&settings, &file);
        resolve(&mut engine, &mut store, &[(current, "display")], &[(
            "display",
            &[a, b],
        )]);
        engine.workspaces.ensure_visible_spaces(&[current], 1, |_| None);
        let target = ws(&engine, current, 0);
        open(&mut engine, &mut store, target, window);
        assert_eq!(
            observe(&mut engine, &mut store, current, window, &evidence(window)),
            current == a
        );
        assert_eq!(store.workspace_for_window(window), Some(target));
    }
}
