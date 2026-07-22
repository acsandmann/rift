use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use objc2_core_foundation::CGSize;
use serde::{Deserialize, Serialize};

use super::LayoutEngine;
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{LayoutSettings, VirtualWorkspaceSettings};
use crate::layout_engine::LayoutSystem;
use crate::model::broadcast::BroadcastSender;
use crate::model::{AppRuleEngine, VirtualWorkspaceId, WindowStore};
use crate::sys::screen::SpaceId;

static SAVE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreScope {
    Workspace,
    Space,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WindowFingerprint {
    #[serde(default)]
    window_server_id: Option<u32>,
    title: Option<String>,
    width: f64,
    height: f64,
    app_id: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct PersistenceState {
    #[serde(default, rename = "persisted_windows")]
    windows: HashMap<WindowId, WindowFingerprint>,
    #[serde(skip)]
    pending_windows: HashSet<WindowId>,
}

impl LayoutEngine {
    pub(super) fn observe_window_for_persistence(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        window: WindowId,
        title: Option<&str>,
        size: CGSize,
        app_id: Option<&str>,
    ) {
        let fingerprint = WindowFingerprint {
            window_server_id: window_store
                .window(window)
                .and_then(|window| window.info.sys_id)
                .map(|id| id.as_u32()),
            title: title.filter(|title| !title.trim().is_empty()).map(str::to_owned),
            width: size.width,
            height: size.height,
            app_id: app_id.filter(|app_id| !app_id.trim().is_empty()).map(str::to_owned),
        };
        self.reconcile_restored_window(window_store, space, window, &fingerprint);
        self.persistence.windows.insert(window, fingerprint);
    }

    pub(super) fn forget_persisted_window(&mut self, window: WindowId) {
        self.persistence.windows.remove(&window);
        self.persistence.pending_windows.remove(&window);
    }

    pub(super) fn forget_persisted_app(&mut self, pid: pid_t) {
        self.persistence.windows.retain(|window, _| window.pid != pid);
        self.persistence.pending_windows.retain(|window| window.pid != pid);
    }

    pub(super) fn transfer_persisted_window_identity(&mut self, from: WindowId, to: WindowId) {
        if let Some(fingerprint) = self.persistence.windows.remove(&from) {
            self.persistence.windows.insert(to, fingerprint);
        }
        if self.persistence.pending_windows.remove(&from) {
            self.persistence.pending_windows.insert(to);
        }
    }

    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let mut buf = String::new();
        File::open(path)?.read_to_string(&mut buf)?;
        Self::deserialize_from_str(&buf)
    }

    pub(crate) fn deserialize_from_str(buf: &str) -> anyhow::Result<Self> {
        let mut engine: Self = match ron::from_str(buf) {
            Ok(engine) => engine,
            Err(original_error) => {
                let Some(migrated) = migrate_legacy_layout_system_tags(buf) else {
                    return Err(original_error.into());
                };
                ron::from_str(&migrated).map_err(|migration_error| {
                    anyhow::anyhow!(
                        "could not parse layout file ({original_error}); compatibility migration also failed ({migration_error})"
                    )
                })?
            }
        };
        engine.persistence.pending_windows = engine.persistence.windows.keys().copied().collect();
        Ok(engine)
    }

    pub fn save(&self, path: PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let serialized = self.serialize_to_string();
        let (temporary, mut file) = loop {
            let sequence = SAVE_TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
            let temporary_extension = path
                .extension()
                .map(|extension| {
                    format!(
                        "{}.{}.{}.tmp",
                        extension.to_string_lossy(),
                        std::process::id(),
                        sequence
                    )
                })
                .unwrap_or_else(|| format!("{}.{}.tmp", std::process::id(), sequence));
            let temporary = path.with_extension(temporary_extension);
            match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary) {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = (|| {
            file.write_all(serialized.as_bytes())?;
            drop(file);
            fs::rename(&temporary, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn finish_loading(
        &mut self,
        virtual_workspace_config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
        broadcast_tx: Option<BroadcastSender>,
    ) {
        self.broadcast_tx = broadcast_tx;
        self.set_layout_settings(layout_settings);
        self.app_rules = AppRuleEngine::new(&virtual_workspace_config.app_rules);
        self.virtual_workspace_manager
            .update_settings(virtual_workspace_config, layout_settings);
    }

    pub fn restore_saved_layout(
        &mut self,
        path: PathBuf,
        scope: RestoreScope,
        active_space: SpaceId,
        window_store: &mut WindowStore,
        virtual_workspace_config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) -> anyhow::Result<usize> {
        let mut snapshot = Self::load(path)?;
        snapshot.finish_loading(
            virtual_workspace_config,
            layout_settings,
            self.broadcast_tx.clone(),
        );

        self.refresh_window_fingerprints(window_store);
        let live_windows = self.persistence.windows.clone();
        let source_space = if snapshot.workspace_layouts.spaces().contains(&active_space) {
            active_space
        } else {
            snapshot
                .workspace_layouts
                .spaces()
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("saved layout contains no macOS spaces"))?
        };

        match scope {
            RestoreScope::Space => {
                let source_workspaces =
                    snapshot.virtual_workspace_manager.list_workspaces(source_space);
                let target_workspaces =
                    self.virtual_workspace_manager.list_workspaces(active_space);
                let source_active =
                    snapshot.virtual_workspace_manager.active_workspace(source_space);
                let mut target_active = None;
                for ((source_id, _), (target_id, _)) in
                    source_workspaces.into_iter().zip(target_workspaces)
                {
                    if let Some(mut workspace) =
                        snapshot.virtual_workspace_manager.workspaces.remove(source_id)
                    {
                        workspace.space = active_space;
                        if let Some(target) =
                            self.virtual_workspace_manager.workspaces.get_mut(target_id)
                        {
                            *target = workspace;
                        }
                        let _ = self.workspace_layouts.replace_workspace_from(
                            &snapshot.workspace_layouts,
                            source_space,
                            source_id,
                            active_space,
                            target_id,
                        );
                        if source_active == Some(source_id) {
                            target_active = Some(target_id);
                        }
                    }
                }
                if let Some(target_active) = target_active {
                    self.virtual_workspace_manager
                        .active_workspace_per_space
                        .insert(active_space, (None, target_active));
                }
                self.persistence.windows.extend(snapshot.persistence.windows);
            }
            RestoreScope::Workspace => {
                let source_id =
                    snapshot
                        .virtual_workspace_manager
                        .active_workspace(source_space)
                        .ok_or_else(|| anyhow::anyhow!("saved space has no active workspace"))?;
                let target_id = self
                    .virtual_workspace_manager
                    .active_workspace(active_space)
                    .ok_or_else(|| anyhow::anyhow!("current space has no active workspace"))?;
                let mut workspace = snapshot
                    .virtual_workspace_manager
                    .workspaces
                    .remove(source_id)
                    .ok_or_else(|| anyhow::anyhow!("saved workspace is missing"))?;
                workspace.space = active_space;
                self.virtual_workspace_manager.workspaces[target_id] = workspace;
                let _ = self.workspace_layouts.replace_workspace_from(
                    &snapshot.workspace_layouts,
                    source_space,
                    source_id,
                    active_space,
                    target_id,
                );
                self.persistence.windows.extend(snapshot.persistence.windows);
            }
        }

        self.persistence.pending_windows = self
            .persistence
            .windows
            .keys()
            .copied()
            .filter(
                |window| match (scope, self.restored_location_for_window(*window)) {
                    (RestoreScope::Space, Some((space, _))) => space == active_space,
                    (RestoreScope::Workspace, Some((space, workspace))) => {
                        space == active_space
                            && self.virtual_workspace_manager.active_workspace(active_space)
                                == Some(workspace)
                    }
                    _ => false,
                },
            )
            .collect();
        let pending_before = self.persistence.pending_windows.len();
        for (live, fingerprint) in live_windows {
            if !window_store.contains_window(live) {
                continue;
            }
            let live_space = window_store
                .current_window_server_space_for_window(live)
                .or_else(|| window_store.workspace_info_for_window(live).map(|w| w.space))
                .unwrap_or(active_space);
            if live_space != active_space {
                continue;
            }
            self.reconcile_restored_window(window_store, live_space, live, &fingerprint);
            self.persistence.windows.insert(live, fingerprint);
        }
        Ok(pending_before - self.persistence.pending_windows.len())
    }

    pub(crate) fn refresh_window_fingerprints(&mut self, window_store: &WindowStore) {
        for (window_id, window) in window_store.iter_windows() {
            let app_id = self
                .persistence
                .windows
                .get(&window_id)
                .and_then(|fingerprint| fingerprint.app_id.clone());
            self.persistence.windows.insert(window_id, WindowFingerprint {
                window_server_id: window.info.sys_id.map(|id| id.as_u32()),
                title: (!window.info.title.trim().is_empty()).then(|| window.info.title.clone()),
                width: window.frame_monotonic.size.width,
                height: window.frame_monotonic.size.height,
                app_id,
            });
        }
    }

    fn restored_location_for_window(
        &self,
        window: WindowId,
    ) -> Option<(SpaceId, VirtualWorkspaceId)> {
        for space in self.workspace_layouts.spaces() {
            for (workspace, layout) in self.workspace_layouts.active_layouts_for_space(space) {
                if self.workspace_tree(workspace).contains_window(layout, window) {
                    return Some((space, workspace));
                }
                if self
                    .floating_positions
                    .workspace_positions(space, workspace)
                    .iter()
                    .any(|(candidate, _)| *candidate == window)
                {
                    return Some((space, workspace));
                }
            }
        }
        None
    }

    fn reconcile_restored_window(
        &mut self,
        window_store: &mut WindowStore,
        live_space: SpaceId,
        live: WindowId,
        fingerprint: &WindowFingerprint,
    ) {
        if self.persistence.pending_windows.is_empty() {
            return;
        }

        let exact = fingerprint
            .window_server_id
            .and_then(|window_server_id| {
                self.persistence.pending_windows.iter().copied().find(|old| {
                    self.persistence.windows.get(old).and_then(|saved| saved.window_server_id)
                        == Some(window_server_id)
                })
            })
            .or_else(|| self.persistence.pending_windows.contains(&live).then_some(live));
        let matched_exact_id = exact.is_some();
        let candidate = exact.or_else(|| {
            let mut candidates: Vec<_> = self
                .persistence
                .pending_windows
                .iter()
                .copied()
                .filter(|old| {
                    self.restored_location_for_window(*old)
                        .is_none_or(|(space, _)| space == live_space)
                })
                .collect();
            if candidates.is_empty() {
                candidates.extend(self.persistence.pending_windows.iter().copied());
            }
            candidates.into_iter().max_by(|a, b| {
                let fa = &self.persistence.windows[a];
                let fb = &self.persistence.windows[b];
                let score = |saved: &WindowFingerprint| {
                    let title = (saved.title.is_some() && saved.title == fingerprint.title) as u8;
                    let size_delta = (saved.width - fingerprint.width).abs()
                        + (saved.height - fingerprint.height).abs();
                    let app = (saved.app_id.is_some() && saved.app_id == fingerprint.app_id) as u8;
                    (title, size_delta, app)
                };
                let (title_a, size_a, app_a) = score(fa);
                let (title_b, size_b, app_b) = score(fb);
                title_a
                    .cmp(&title_b)
                    .then_with(|| size_b.partial_cmp(&size_a).unwrap_or(Ordering::Equal))
                    .then_with(|| app_a.cmp(&app_b))
                    .then_with(|| b.cmp(a))
            })
        });

        let Some(old) = candidate else { return };
        let Some(saved) = self.persistence.windows.get(&old) else {
            return;
        };
        let title_matches = saved.title.is_some() && saved.title == fingerprint.title;
        let size_delta =
            (saved.width - fingerprint.width).abs() + (saved.height - fingerprint.height).abs();
        let app_matches = saved.app_id.is_some() && saved.app_id == fingerprint.app_id;
        if old != live && !matched_exact_id && !title_matches && size_delta > 8.0 && !app_matches {
            return;
        }

        let restored_location = self.restored_location_for_window(old);
        self.persistence.pending_windows.remove(&old);
        if old != live {
            self.transfer_persistent_window_identity(old, live);
            self.persistence.windows.remove(&old);
        }
        if let Some((space, workspace)) = restored_location {
            let _ = self.virtual_workspace_manager.assign_window_to_workspace(
                window_store,
                space,
                live,
                workspace,
            );
        }
    }
}

fn migrate_legacy_layout_system_tags(input: &str) -> Option<String> {
    const TAGS: [(&str, &str); 5] = [
        ("(kind:\"traditional\",", "traditional(("),
        ("(kind:\"bsp\",", "bsp(("),
        ("(kind:\"master_stack\",", "master_stack(("),
        ("(kind:\"scrolling\",", "scrolling(("),
        ("(kind:\"stack\",", "stack(("),
    ];

    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut changed = false;
    while cursor < input.len() {
        let Some((start, needle, replacement)) = TAGS
            .iter()
            .filter_map(|(needle, replacement)| {
                input[cursor..]
                    .find(needle)
                    .map(|offset| (cursor + offset, *needle, *replacement))
            })
            .min_by_key(|(start, _, _)| *start)
        else {
            output.push_str(&input[cursor..]);
            break;
        };

        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (offset, ch) in input[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        output.push_str(&input[cursor..start]);
        output.push_str(replacement);
        output.push_str(&input[start + needle.len()..end]);
        output.push_str("))");
        cursor = end + 1;
        changed = true;
    }
    changed.then_some(output)
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CGSize;

    use super::*;
    use crate::common::config::LayoutMode;
    use crate::layout_engine::{LayoutEvent, LayoutSystemKind};
    use crate::model::VirtualWorkspace;

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
            let migrated = migrate_legacy_layout_system_tags(&legacy).unwrap();
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
        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, titled_match));
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
    fn restored_title_beats_size_and_app_fallbacks() {
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
        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, title_match));
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
        assert!(windows.contains(&live));
        assert!(windows.contains(&size_and_app_match));
        assert!(!windows.contains(&title_match));
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
}
