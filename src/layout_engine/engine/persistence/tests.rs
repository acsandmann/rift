use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use super::matcher::MatchScope;
use super::*;
use crate::actor::app::WindowInfo;
use crate::common::config::LayoutMode;
use crate::layout_engine::systems::{Focus, WindowQuery};
use crate::layout_engine::{LayoutEvent, LayoutSystemKind};
use crate::model::VirtualWorkspace;
use crate::model::virtual_workspace::WorkspaceTransferReason;
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
        frame_monotonic: frame,
        is_manageable: true,
        manage_override: None,
    }
}

fn fresh(
    settings: &VirtualWorkspaceSettings,
    spaces: &[(SpaceId, &str)],
) -> (LayoutEngine, WindowStore) {
    let mut engine = LayoutEngine::new(settings, &LayoutSettings::default());
    let mut store = WindowStore::default();
    let all: Vec<_> = spaces.iter().map(|(space, _)| *space).collect();
    engine.workspaces.ensure_visible_spaces(&all, all.len(), |_| None);
    for (space, display) in spaces {
        let size = CGSize::new(1200.0, 800.0);
        let _ = engine.handle_event(&mut store, LayoutEvent::SpaceExposed(*space, size));
        engine.update_space_display(*space, Some(*display));
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
    engine.finish_loading(settings, &LayoutSettings::default()).unwrap();
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
        .flat_map(|layout| entry.layout_system.windows(layout, WindowQuery::All))
        .collect()
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

#[test]
fn restores_legacy_slots_and_atomically_installs_workspace_contents() {
    let per_display = VirtualWorkspaceSettings::default();
    let shared = VirtualWorkspaceSettings {
        mode: WorkspaceMode::Global,
        default_workspace_count: 2,
        workspace_names: vec!["One".into(), "Two".into()],
        ..Default::default()
    };
    let [a, b, c] = [900, 901, 902].map(SpaceId::new);
    let (w1, w2, w3) = (win(7, 1), win(7, 2), win(7, 3));
    let other = win(8, 4);
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
        assert_eq!(tree(&loaded, id).len(), 4);
        for layout in loaded.workspaces[id].layout_state.all_layouts() {
            assert!(
                loaded.workspaces[id]
                    .layout_system
                    .windows(layout, WindowQuery::Visible)
                    .is_empty()
            );
        }
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
    assert_rejected(
        &LEGACY_V2.replacen(
            "selected_child:(idx:3,version:1)",
            "selected_child:(idx:999999,version:1)",
            1,
        ),
        "layout selection references an invalid child",
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
}

#[test]
fn single_display_restore_keeps_the_surviving_desktops_catalog() {
    let settings = VirtualWorkspaceSettings {
        mode: WorkspaceMode::Global,
        default_workspace_count: 2,
        workspace_names: vec!["1".into(), "2".into()],
        ..Default::default()
    };
    let (old, current) = (SpaceId::new(801), SpaceId::new(802));
    for exposed in [true, false] {
        let mut engine = LayoutEngine::new(&settings, &LayoutSettings::default());
        engine.workspaces.ensure_visible_spaces(&[current], 1, |_| None);
        engine.workspaces.ensure_space_initialized(old);
        if exposed {
            engine.update_space_display(old, Some("display"));
        }
        engine.update_space_display(current, Some("display"));
        let expected = engine.workspaces.workspace_ids(current);
        let saved = engine.serialize_to_string(&WindowStore::default());
        let (mut engine, mut store) = boot(&settings, &saved);
        resolve(&mut engine, &mut store, &[(current, "display")], &[(
            "display",
            &[current],
        )]);
        assert_eq!(engine.workspaces.workspace_ids(current), expected);
    }
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

#[test]
fn checkpoint_keeps_unavailable_tile_dormant_until_readmission() {
    let settings = VirtualWorkspaceSettings::default();
    let space = SpaceId::new(900);
    let (ardia, mail) = (win(7, 1), win(8, 2));
    let (mut engine, mut store) = fresh(&settings, &[(space, "display")]);
    let workspace = ws(&engine, space, 0);
    engine.switch_workspace_layout_mode(&store, space, workspace, LayoutMode::Traditional);
    open(&mut engine, &mut store, workspace, ardia);
    open(&mut engine, &mut store, workspace, mail);
    let screen = CGRect::new(CGPoint::ZERO, CGSize::new(1920.0, 1080.0));
    let frames = |engine: &mut LayoutEngine| {
        engine.calculate_layout(space, screen, &Default::default(), Default::default())
    };
    let split = frames(&mut engine);
    assert_eq!(split.len(), 2);
    assert!(split.iter().all(|(_, frame)| frame.size.width == 960.0));
    let checkpoint = engine.serialize_to_string(&store);

    store.window_mut(mail).unwrap().info.is_minimized = true;
    let _ = engine.handle_event(&mut store, LayoutEvent::WindowSuspended(mail));
    assert_eq!(frames(&mut engine), [(ardia, screen)]);
    for _ in 0..2 {
        let report = restore(
            &mut engine,
            &mut store,
            &checkpoint,
            RestoreScope::Space,
            space,
            RestoreSource::CurrentSpace,
        )
        .unwrap();
        assert_eq!(report.matched, 2);
        assert_eq!(frames(&mut engine), [(ardia, screen)]);
        assert_eq!(tree(&engine, workspace), [ardia, mail]);
        assert!(!engine.workspaces[workspace].layout_system.window_participates(mail));
    }

    store.window_mut(mail).unwrap().info.is_minimized = false;
    let _ = engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, mail));
    assert_eq!(frames(&mut engine), split);
}

#[test]
fn startup_slots_stay_dormant_until_matched_and_admitted() {
    let settings = VirtualWorkspaceSettings::default();
    let space = SpaceId::new(900);
    let (first, saved, restarted) = (win(7, 1), win(8, 2), win(9, 3));
    for mode in [
        LayoutMode::Traditional,
        LayoutMode::Bsp,
        LayoutMode::Stack,
        LayoutMode::MasterStack,
        LayoutMode::Scrolling,
        LayoutMode::Floating,
    ] {
        let (mut engine, mut store) = fresh(&settings, &[(space, "display")]);
        let workspace = ws(&engine, space, 0);
        engine.switch_workspace_layout_mode(&store, space, workspace, mode);
        open(&mut engine, &mut store, workspace, first);
        open(&mut engine, &mut store, workspace, saved);
        let expected = tree(&engine, workspace)
            .into_iter()
            .map(|window| if window == saved { restarted } else { window })
            .collect::<Vec<_>>();
        let checkpoint = engine.serialize_to_string(&store);
        let (mut engine, mut store) = boot(&settings, &checkpoint);
        let displays = [(space, "display")];
        resolve(&mut engine, &mut store, &displays, &[("display", &[space])]);
        let screen = CGRect::new(CGPoint::ZERO, CGSize::new(1200.0, 800.0));
        let frames = |engine: &mut LayoutEngine| {
            engine.calculate_layout(space, screen, &Default::default(), Default::default())
        };
        assert!(frames(&mut engine).is_empty(), "{mode:?}");

        let mut live = live_state(saved);
        live.info.sys_id = live_state(restarted).info.sys_id;
        live.info.is_minimized = true;
        let fingerprint = WindowFingerprint::of(restarted, &live);
        store.insert_window(restarted, live);
        assert!(observe(&mut engine, &mut store, space, restarted, &fingerprint));
        let _ = engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, restarted));
        assert_eq!(tree(&engine, workspace), expected, "{mode:?}");
        assert!(frames(&mut engine).is_empty(), "{mode:?}");
        assert!(!engine.workspaces[workspace].layout_system.window_participates(restarted));

        assert_eq!(
            engine.discard_unmatched_candidates_for_app(first.pid, None, &[space]),
            1
        );
        assert_eq!(tree(&engine, workspace), [restarted], "{mode:?}");

        store.window_mut(restarted).unwrap().info.is_minimized = false;
        let _ = engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, restarted));
        assert!(engine.workspaces[workspace].layout_system.window_participates(restarted));
        assert_eq!(frames(&mut engine)[0].0, restarted, "{mode:?}");
    }
}

#[test]
fn scrolling_checkpoint_preserves_selection_and_viewport_through_readmission() {
    let settings = VirtualWorkspaceSettings::default();
    let space = SpaceId::new(900);
    let windows = [win(7, 1), win(8, 2), win(9, 3)];
    let selected = windows[2];
    let (mut engine, mut store) = fresh(&settings, &[(space, "display")]);
    let workspace = ws(&engine, space, 0);
    engine.switch_workspace_layout_mode(&store, space, workspace, LayoutMode::Scrolling);
    for window in windows {
        open(&mut engine, &mut store, workspace, window);
    }
    let layout = engine.workspaces[workspace].active_layout().unwrap();
    let screen = CGRect::new(CGPoint::ZERO, CGSize::new(1200.0, 800.0));
    let frames = |engine: &mut LayoutEngine| {
        engine.calculate_layout(space, screen, &Default::default(), Default::default())
    };
    let _ = frames(&mut engine);
    engine.workspaces[workspace]
        .layout_system
        .focus(layout, Focus::Select(selected))
        .handled;
    let expected = frames(&mut engine);
    assert_ne!(expected[0].1.origin.x, screen.origin.x);
    let checkpoint = engine.serialize_to_string(&store);

    for minimize_selection in [false, true] {
        store.window_mut(selected).unwrap().info.is_minimized = minimize_selection;
        restore(
            &mut engine,
            &mut store,
            &checkpoint,
            RestoreScope::Space,
            space,
            RestoreSource::CurrentSpace,
        )
        .unwrap();
        assert_eq!(tree(&engine, workspace), windows);
        if minimize_selection {
            let visible = frames(&mut engine);
            assert_eq!(
                visible.iter().map(|&(window, _)| window).collect::<Vec<_>>(),
                windows[..2]
            );
            assert!(visible.iter().any(|(_, frame)| frame.origin.x >= screen.origin.x
                && frame.origin.x < screen.max().x - 1.0));
            store.window_mut(selected).unwrap().info.is_minimized = false;
            let _ = engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, selected));
        }
        assert_eq!(
            engine.workspaces[workspace].layout_system.selected_window(layout),
            Some(selected)
        );
        assert_eq!(frames(&mut engine), expected);
    }
}
