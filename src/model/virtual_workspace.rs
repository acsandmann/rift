use std::collections::{HashMap, HashSet};
use std::time::Instant;

use accessibility_sys::pid_t;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};
use slotmap::{SlotMap, new_key_type};
use tracing::{error, trace, warn};

use crate::actor::app::WindowId;
use crate::common::config::AppWorkspaceRule;
use crate::sys::screen::SpaceId;

new_key_type! {
    pub struct VirtualWorkspaceId;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    NoWorkspacesAvailable,
    AssignmentFailed,
    InvalidWorkspaceId(VirtualWorkspaceId),
    InvalidWorkspaceIndex(usize),
    InconsistentState(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualWorkspace {
    pub name: String,
    windows: HashSet<WindowId>,
    last_focused: Option<WindowId>,
}

impl VirtualWorkspace {
    fn new(name: String) -> Self {
        Self {
            name,
            windows: HashSet::new(),
            last_focused: None,
        }
    }

    pub fn contains_window(&self, window_id: WindowId) -> bool { self.windows.contains(&window_id) }

    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ { self.windows.iter().copied() }

    pub fn add_window(&mut self, window_id: WindowId) { self.windows.insert(window_id); }

    pub fn remove_window(&mut self, window_id: WindowId) -> bool {
        if self.last_focused == Some(window_id) {
            self.last_focused = None;
        }
        self.windows.remove(&window_id)
    }

    pub fn set_last_focused(&mut self, window_id: Option<WindowId>) {
        self.last_focused = window_id;
    }

    pub fn last_focused(&self) -> Option<WindowId> { self.last_focused }

    pub fn window_count(&self) -> usize { self.windows.len() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideCorner {
    BottomLeft,
    BottomRight,
}

impl Default for HideCorner {
    fn default() -> Self { HideCorner::BottomRight }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VirtualWorkspaceManager {
    workspaces: SlotMap<VirtualWorkspaceId, VirtualWorkspace>,
    pub active_workspace_per_space:
        HashMap<SpaceId, (Option<VirtualWorkspaceId>, VirtualWorkspaceId)>,
    pub window_to_workspace: HashMap<WindowId, VirtualWorkspaceId>,
    floating_positions: HashMap<(SpaceId, VirtualWorkspaceId), FloatingWindowPositions>,
    workspace_counter: usize,
    #[serde(skip)]
    app_rules: Vec<AppWorkspaceRule>,
    #[serde(skip)]
    workspace_list_cache: Option<Vec<(VirtualWorkspaceId, String)>>,
    #[serde(skip)]
    max_workspaces: usize,
}

impl Default for VirtualWorkspaceManager {
    fn default() -> Self { Self::new() }
}

impl VirtualWorkspaceManager {
    pub fn new() -> Self { Self::new_with_rules(Vec::new()) }

    pub fn new_with_rules(app_rules: Vec<AppWorkspaceRule>) -> Self {
        let mut manager = Self {
            workspaces: SlotMap::default(),
            active_workspace_per_space: HashMap::new(),
            window_to_workspace: HashMap::new(),
            floating_positions: HashMap::new(),
            workspace_counter: 1,
            app_rules,
            workspace_list_cache: None,
            max_workspaces: 32, // TODO: just like???? should we do this? will any sane person ever have 32 ws', prolly not
        };

        manager.create_workspace(None).expect("Failed to create default workspace");
        manager
    }

    pub fn create_workspace(
        &mut self,
        name: Option<String>,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        // Check workspace limit
        if self.workspaces.len() >= self.max_workspaces {
            return Err(WorkspaceError::InconsistentState(format!(
                "Maximum workspace limit ({}) reached",
                self.max_workspaces
            )));
        }

        let name = name.unwrap_or_else(|| {
            let name = format!("Workspace {}", self.workspace_counter);
            self.workspace_counter += 1;
            name
        });

        let workspace = VirtualWorkspace::new(name);
        let workspace_id = self.workspaces.insert(workspace);

        // Invalidate cache
        self.workspace_list_cache = None;

        Ok(workspace_id)
    }

    pub fn last_workspace(&self, space: SpaceId) -> Option<VirtualWorkspaceId> {
        self.active_workspace_per_space.get(&space)?.0
    }

    pub fn active_workspace(&self, space: SpaceId) -> Option<VirtualWorkspaceId> {
        self.active_workspace_per_space
            .get(&space)
            .map(|tuple| tuple.1)
            .or_else(|| self.workspaces.keys().next())
    }

    pub fn set_active_workspace(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> bool {
        trace_misc("set_active_workspace", || {
            let active = self.active_workspace_per_space.get(&space).map(|tuple| tuple.1);

            let result = if self.workspaces.contains_key(workspace_id) {
                self.active_workspace_per_space.insert(space, (active, workspace_id));
                true
            } else {
                error!(
                    "Attempted to set non-existent workspace {:?} as active",
                    workspace_id
                );
                false
            };

            result
        })
    }

    pub fn next_workspace(
        &self,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
    ) -> Option<VirtualWorkspaceId> {
        let mut workspace_ids: Vec<_> = self
            .workspaces
            .iter()
            .filter(|(_, workspace)| {
                skip_empty.map_or(true, |skip| !skip || !workspace.windows.is_empty())
            })
            .map(|(id, workspace)| (id, workspace.name.as_str()))
            .collect();
        workspace_ids.sort_by(|a, b| a.1.cmp(b.1));

        let workspace_ids: Vec<_> = workspace_ids.into_iter().map(|(id, _)| id).collect();
        let current_pos = workspace_ids.iter().position(|&id| id == current)?;
        let next_pos = (current_pos + 1) % workspace_ids.len();
        workspace_ids.get(next_pos).copied()
    }

    pub fn prev_workspace(
        &self,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
    ) -> Option<VirtualWorkspaceId> {
        let mut workspace_ids: Vec<_> = self
            .workspaces
            .iter()
            .filter(|(_, workspace)| {
                skip_empty.map_or(true, |skip| !skip || !workspace.windows.is_empty())
            })
            .map(|(id, workspace)| (id, workspace.name.as_str()))
            .collect();
        workspace_ids.sort_by(|a, b| a.1.cmp(b.1));

        let workspace_ids: Vec<_> = workspace_ids.into_iter().map(|(id, _)| id).collect();
        let current_pos = workspace_ids.iter().position(|&id| id == current)?;
        let prev_pos = if current_pos == 0 {
            workspace_ids.len() - 1
        } else {
            current_pos - 1
        };
        workspace_ids.get(prev_pos).copied()
    }

    pub fn assign_window_to_workspace(
        &mut self,
        window_id: WindowId,
        workspace_id: VirtualWorkspaceId,
    ) -> bool {
        trace_misc("assign_window_to_workspace", || {
            if !self.workspaces.contains_key(workspace_id) {
                error!(
                    "Attempted to assign window to non-existent workspace {:?}",
                    workspace_id
                );
                return false;
            }

            if let Some(old_workspace_id) = self.window_to_workspace.get(&window_id).copied() {
                if let Some(old_workspace) = self.workspaces.get_mut(old_workspace_id) {
                    old_workspace.remove_window(window_id);
                }
            }

            let result = if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.add_window(window_id);
                self.window_to_workspace.insert(window_id, workspace_id);
                true
            } else {
                error!(
                    "Failed to get workspace {:?} for window assignment",
                    workspace_id
                );
                false
            };

            result
        })
    }

    pub fn workspace_for_window(&self, window_id: WindowId) -> Option<VirtualWorkspaceId> {
        self.window_to_workspace.get(&window_id).copied()
    }

    pub fn remove_window(&mut self, window_id: WindowId) {
        if let Some(workspace_id) = self.window_to_workspace.remove(&window_id) {
            if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.remove_window(window_id);
            }
        }
    }

    pub fn remove_windows_for_app(&mut self, pid: pid_t) {
        let windows_to_remove: Vec<_> =
            self.window_to_workspace.keys().filter(|wid| wid.pid == pid).copied().collect();

        for window_id in windows_to_remove {
            self.remove_window(window_id);
        }
    }

    /// Gets all windows in the active virtual workspace for a given native space.
    pub fn windows_in_active_workspace(&self, space: SpaceId) -> Vec<WindowId> {
        if let Some(workspace_id) = self.active_workspace(space) {
            if let Some(workspace) = self.workspaces.get(workspace_id) {
                return workspace.windows().collect();
            }
        }
        Vec::new()
    }

    pub fn windows_in_inactive_workspaces(&self, space: SpaceId) -> Vec<WindowId> {
        let active_workspace_id = self.active_workspace(space);

        self.workspaces
            .iter()
            .filter(|(id, _)| Some(*id) != active_workspace_id)
            .flat_map(|(_, workspace)| workspace.windows())
            .collect()
    }

    pub fn calculate_hidden_position(
        &self,
        screen_frame: CGRect,
        _window_index: usize,
        original_size: CGSize,
        corner: HideCorner,
        app_bundle_id: Option<&str>,
    ) -> CGRect {
        let one_pixel_offset = if let Some(bundle_id) = app_bundle_id {
            match bundle_id {
                "us.zoom.xos" => CGPoint::new(0.0, 0.0),
                _ => match corner {
                    HideCorner::BottomLeft => CGPoint::new(1.0, -1.0),
                    HideCorner::BottomRight => CGPoint::new(1.0, 1.0),
                },
            }
        } else {
            match corner {
                HideCorner::BottomLeft => CGPoint::new(1.0, -1.0),
                HideCorner::BottomRight => CGPoint::new(1.0, 1.0),
            }
        };

        let hidden_point = match corner {
            HideCorner::BottomLeft => {
                let bottom_left = CGPoint::new(screen_frame.origin.x, screen_frame.max().y);
                CGPoint::new(
                    bottom_left.x + one_pixel_offset.x - original_size.width + 1.0,
                    bottom_left.y + one_pixel_offset.y,
                )
            }
            HideCorner::BottomRight => {
                let bottom_right = CGPoint::new(screen_frame.max().x, screen_frame.max().y);
                CGPoint::new(
                    bottom_right.x - one_pixel_offset.x - 1.0, // -1 to keep 1px visible
                    bottom_right.y - one_pixel_offset.y,
                )
            }
        };

        CGRect::new(hidden_point, original_size)
    }

    pub fn set_last_focused_window(
        &mut self,
        workspace_id: VirtualWorkspaceId,
        window_id: Option<WindowId>,
    ) {
        if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
            workspace.set_last_focused(window_id);
        }
    }

    pub fn last_focused_window(&self, workspace_id: VirtualWorkspaceId) -> Option<WindowId> {
        self.workspaces.get(workspace_id)?.last_focused()
    }

    pub fn workspace_info(&self, workspace_id: VirtualWorkspaceId) -> Option<&VirtualWorkspace> {
        self.workspaces.get(workspace_id)
    }

    pub fn store_floating_position(
        &mut self,
        space: SpaceId,
        window_id: WindowId,
        position: CGRect,
    ) {
        if let Some(workspace_id) = self.active_workspace(space) {
            let key = (space, workspace_id);
            self.floating_positions
                .entry(key)
                .or_default()
                .store_position(window_id, position);
        }
    }

    pub fn get_floating_position(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        window_id: WindowId,
    ) -> Option<CGRect> {
        let key = (space, workspace_id);
        self.floating_positions.get(&key)?.get_position(window_id)
    }

    pub fn store_current_floating_positions(
        &mut self,
        space: SpaceId,
        floating_windows: &[(WindowId, CGRect)],
    ) {
        if let Some(workspace_id) = self.active_workspace(space) {
            let key = (space, workspace_id);
            let positions = self.floating_positions.entry(key).or_default();

            for &(window_id, position) in floating_windows {
                positions.store_position(window_id, position);
            }
        }
    }

    pub fn get_workspace_floating_positions(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> Vec<(WindowId, CGRect)> {
        let key = (space, workspace_id);
        if let Some(positions) = self.floating_positions.get(&key) {
            positions
                .windows()
                .filter_map(|window_id| {
                    positions.get_position(window_id).map(|position| (window_id, position))
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn remove_floating_position(&mut self, window_id: WindowId) {
        for positions in self.floating_positions.values_mut() {
            positions.remove_position(window_id);
        }
    }

    pub fn remove_app_floating_positions(&mut self, pid: pid_t) {
        for positions in self.floating_positions.values_mut() {
            positions.remove_app_windows(pid);
        }
    }

    pub fn list_workspaces(&mut self) -> &[(VirtualWorkspaceId, String)] {
        if self.workspace_list_cache.is_none() {
            let mut workspaces: Vec<_> = self
                .workspaces
                .iter()
                .map(|(id, workspace)| (id, workspace.name.clone()))
                .collect();

            workspaces.sort_by(|a, b| a.1.cmp(&b.1));
            self.workspace_list_cache = Some(workspaces);
        }
        self.workspace_list_cache.as_ref().unwrap()
    }

    pub fn list_workspaces_readonly(&self) -> Vec<(VirtualWorkspaceId, &str)> {
        let mut workspaces: Vec<_> = self
            .workspaces
            .iter()
            .map(|(id, workspace)| (id, workspace.name.as_str()))
            .collect();

        workspaces.sort_by(|a, b| a.1.cmp(b.1));
        workspaces
    }

    pub fn rename_workspace(&mut self, workspace_id: VirtualWorkspaceId, new_name: String) -> bool {
        if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
            workspace.name = new_name;
            self.workspace_list_cache = None;
            true
        } else {
            false
        }
    }

    pub fn auto_assign_window(
        &mut self,
        window_id: WindowId,
        space: SpaceId,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        let default_workspace_id = self.get_default_workspace(space)?;
        if self.assign_window_to_workspace(window_id, default_workspace_id) {
            Ok(default_workspace_id)
        } else {
            Err(WorkspaceError::AssignmentFailed)
        }
    }

    pub fn assign_window_with_app_info(
        &mut self,
        window_id: WindowId,
        space: SpaceId,
        app_bundle_id: Option<&str>,
        app_name: Option<&str>,
    ) -> Result<(VirtualWorkspaceId, bool), WorkspaceError> {
        if self.workspaces.is_empty() {
            return Err(WorkspaceError::NoWorkspacesAvailable);
        }

        let rule_match = self.find_matching_app_rule(app_bundle_id, app_name).cloned();
        if let Some(rule) = rule_match {
            let target_workspace_id = if let Some(workspace_idx) = rule.workspace {
                if workspace_idx >= self.workspaces.len() {
                    tracing::warn!(
                        "App rule references non-existent workspace index {}, falling back to active workspace",
                        workspace_idx
                    );
                    self.get_default_workspace(space)?
                } else {
                    let workspaces = self.list_workspaces();
                    if let Some((workspace_id, _)) = workspaces.get(workspace_idx) {
                        *workspace_id
                    } else {
                        warn!(
                            "App rule references invalid workspace index {}, falling back to active workspace",
                            workspace_idx
                        );
                        self.get_default_workspace(space)?
                    }
                }
            } else {
                self.get_default_workspace(space)?
            };

            if self.assign_window_to_workspace(window_id, target_workspace_id) {
                return Ok((target_workspace_id, rule.floating));
            } else {
                error!("Failed to assign window to workspace from app rule");
            }
        }

        let default_workspace_id = self.get_default_workspace(space)?;
        if self.assign_window_to_workspace(window_id, default_workspace_id) {
            Ok((default_workspace_id, false))
        } else {
            error!("Failed to assign window to default workspace");
            Err(WorkspaceError::AssignmentFailed)
        }
    }

    fn get_default_workspace(
        &mut self,
        space: SpaceId,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        if let Some(active_workspace_id) = self.active_workspace(space) {
            if self.workspaces.contains_key(active_workspace_id) {
                return Ok(active_workspace_id);
            } else {
                warn!("Active workspace no longer exists, clearing reference");
                self.active_workspace_per_space.remove(&space);
            }
        }

        let first_workspace_id = if let Some((first_id, _)) = self.workspaces.iter().next() {
            first_id
        } else {
            warn!("No workspaces exist, creating default workspace");
            let default_id = self.create_workspace(Some("Default".to_string()))?;
            self.set_active_workspace(space, default_id);
            return Ok(default_id);
        };

        if self.set_active_workspace(space, first_workspace_id) {
            Ok(first_workspace_id)
        } else {
            Err(WorkspaceError::InconsistentState(
                "Failed to set default workspace as active".to_string(),
            ))
        }
    }

    fn find_matching_app_rule(
        &self,
        app_bundle_id: Option<&str>,
        app_name: Option<&str>,
    ) -> Option<&AppWorkspaceRule> {
        self.app_rules.iter().find(|rule| {
            if let Some(bundle_id) = app_bundle_id {
                if let Some(ref rule_app_id) = rule.app_id {
                    if rule_app_id == bundle_id {
                        return true;
                    }
                }
            }

            if let (Some(name), Some(rule_name)) = (app_name, &rule.app_name) {
                if name.contains(rule_name) || rule_name.contains(name) {
                    return true;
                }
            }

            false
        })
    }

    pub fn get_stats(&self) -> WorkspaceStats {
        let mut stats = WorkspaceStats {
            total_workspaces: self.workspaces.len(),
            total_windows: self.window_to_workspace.len(),
            active_spaces: self.active_workspace_per_space.len(),
            workspace_window_counts: HashMap::new(),
        };

        for (workspace_id, workspace) in &self.workspaces {
            stats.workspace_window_counts.insert(workspace_id, workspace.window_count());
        }

        stats
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SerializableRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<CGRect> for SerializableRect {
    fn from(rect: CGRect) -> Self {
        Self {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }
}

impl From<SerializableRect> for CGRect {
    fn from(rect: SerializableRect) -> Self {
        CGRect::new(
            CGPoint::new(rect.x, rect.y),
            CGSize::new(rect.width, rect.height),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FloatingWindowPositions {
    positions: HashMap<WindowId, SerializableRect>,
}

impl FloatingWindowPositions {
    pub fn store_position(&mut self, window_id: WindowId, position: CGRect) {
        self.positions.insert(window_id, position.into());
    }

    pub fn get_position(&self, window_id: WindowId) -> Option<CGRect> {
        self.positions.get(&window_id).map(|rect| (*rect).into())
    }

    pub fn remove_position(&mut self, window_id: WindowId) -> Option<CGRect> {
        self.positions.remove(&window_id).map(|rect| rect.into())
    }

    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ { self.positions.keys().copied() }

    pub fn clear(&mut self) { self.positions.clear(); }

    pub fn contains_window(&self, window_id: WindowId) -> bool {
        self.positions.contains_key(&window_id)
    }

    pub fn remove_app_windows(&mut self, pid: pid_t) {
        self.positions.retain(|window_id, _| window_id.pid != pid);
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceStats {
    pub total_workspaces: usize,
    pub total_windows: usize,
    pub active_spaces: usize,
    pub workspace_window_counts: HashMap<VirtualWorkspaceId, usize>,
}

fn trace_misc<T>(desc: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let end = Instant::now();
    trace!(time = ?(end - start), "{desc}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::app::WindowId;
    use crate::sys::screen::SpaceId;

    #[test]
    fn test_virtual_workspace_creation() {
        let mut manager = VirtualWorkspaceManager::new();

        assert_eq!(manager.list_workspaces().len(), 1);

        let ws_id = manager.create_workspace(Some("Test Workspace".to_string())).unwrap();
        assert_eq!(manager.list_workspaces().len(), 2);

        let workspace = manager.workspace_info(ws_id).unwrap();
        assert_eq!(workspace.name, "Test Workspace");
    }

    #[test]
    fn test_window_assignment() {
        let mut manager = VirtualWorkspaceManager::new();
        let ws1_id = manager.create_workspace(Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(Some("WS2".to_string())).unwrap();

        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        assert!(manager.assign_window_to_workspace(window1, ws1_id));
        assert!(manager.assign_window_to_workspace(window2, ws2_id));

        assert_eq!(manager.workspace_for_window(window1), Some(ws1_id));
        assert_eq!(manager.workspace_for_window(window2), Some(ws2_id));

        let ws1 = manager.workspace_info(ws1_id).unwrap();
        let ws2 = manager.workspace_info(ws2_id).unwrap();

        assert!(ws1.contains_window(window1));
        assert!(!ws1.contains_window(window2));
        assert!(ws2.contains_window(window2));
        assert!(!ws2.contains_window(window1));
    }

    #[test]
    fn test_active_workspace_switching() {
        let mut manager = VirtualWorkspaceManager::new();
        let ws1_id = manager.create_workspace(Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(Some("WS2".to_string())).unwrap();

        let space = SpaceId::new(1);

        assert!(manager.set_active_workspace(space, ws1_id));
        assert_eq!(manager.active_workspace(space), Some(ws1_id));

        assert!(manager.set_active_workspace(space, ws2_id));
        assert_eq!(manager.active_workspace(space), Some(ws2_id));
    }

    #[test]
    fn test_window_visibility() {
        fn is_window_visible(
            wm: &VirtualWorkspaceManager,
            window_id: WindowId,
            space: SpaceId,
        ) -> bool {
            let window_workspace = wm.workspace_for_window(window_id);
            let active_workspace = wm.active_workspace(space);

            match (window_workspace, active_workspace) {
                (Some(window_ws), Some(active_ws)) => window_ws == active_ws,
                _ => true,
            }
        }
        let mut manager = VirtualWorkspaceManager::new();
        let ws1_id = manager.create_workspace(Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(Some("WS2".to_string())).unwrap();

        let space = SpaceId::new(1);
        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        manager.set_active_workspace(space, ws1_id);
        manager.assign_window_to_workspace(window1, ws1_id);
        manager.assign_window_to_workspace(window2, ws2_id);

        assert!(is_window_visible(&manager, window1, space));
        assert!(!is_window_visible(&manager, window2, space));

        manager.set_active_workspace(space, ws2_id);
        assert!(!is_window_visible(&manager, window1, space));
        assert!(is_window_visible(&manager, window2, space));
    }

    #[test]
    fn test_workspace_navigation() {
        let mut manager = VirtualWorkspaceManager::new();
        let ws1_id = manager.create_workspace(Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(Some("WS2".to_string())).unwrap();
        let ws3_id = manager.create_workspace(Some("WS3".to_string())).unwrap();

        assert_eq!(manager.next_workspace(ws1_id, None), Some(ws2_id));
        assert_eq!(manager.next_workspace(ws2_id, None), Some(ws3_id));

        assert_eq!(manager.prev_workspace(ws2_id, None), Some(ws1_id));
        assert_eq!(manager.prev_workspace(ws3_id, None), Some(ws2_id));
    }
}
