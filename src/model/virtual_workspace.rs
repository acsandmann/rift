use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use slotmap::{SlotMap, new_key_type};
use tracing::{error, warn};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{AppWorkspaceRule, VirtualWorkspaceSettings, WorkspaceSelector};
use crate::common::log::trace_misc;
use crate::layout_engine::Direction;
use crate::sys::app::pid_t;
use crate::sys::geometry::CGRectDef;
use crate::sys::screen::SpaceId;

new_key_type! {
    pub struct VirtualWorkspaceId;
}

impl std::fmt::Display for VirtualWorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dbg = format!("{:?}", self);
        let digits: String = dbg.chars().filter(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            write!(f, "{:08}", n)
        } else {
            write!(f, "{}", dbg)
        }
    }
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
    pub space: SpaceId,
    windows: HashSet<WindowId>,
    last_focused: Option<WindowId>,
}

impl VirtualWorkspace {
    fn new(name: String, space: SpaceId) -> Self {
        Self {
            name,
            space,
            windows: HashSet::default(),
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
    workspaces_by_space: HashMap<SpaceId, Vec<VirtualWorkspaceId>>,
    pub active_workspace_per_space:
        HashMap<SpaceId, (Option<VirtualWorkspaceId>, VirtualWorkspaceId)>,
    pub window_to_workspace: HashMap<(SpaceId, WindowId), VirtualWorkspaceId>,
    #[serde(skip)]
    window_rule_floating: HashMap<(SpaceId, WindowId), bool>,
    floating_positions: HashMap<(SpaceId, VirtualWorkspaceId), FloatingWindowPositions>,
    workspace_counter: usize,
    #[serde(skip)]
    app_rules: Vec<AppWorkspaceRule>,
    #[serde(skip)]
    max_workspaces: usize,
    #[serde(skip)]
    default_workspace_count: usize,
    #[serde(skip)]
    default_workspace_names: Vec<String>,
    #[serde(skip)]
    workspace_auto_back_and_forth: bool,
}

impl Default for VirtualWorkspaceManager {
    fn default() -> Self { Self::new() }
}

impl VirtualWorkspaceManager {
    pub fn new() -> Self { Self::new_with_config(&VirtualWorkspaceSettings::default()) }

    pub fn new_with_rules(app_rules: Vec<AppWorkspaceRule>) -> Self {
        let mut cfg = VirtualWorkspaceSettings::default();
        cfg.app_rules = app_rules;
        Self::new_with_config(&cfg)
    }

    pub fn new_with_config(config: &VirtualWorkspaceSettings) -> Self {
        Self {
            workspaces: SlotMap::default(),
            workspaces_by_space: HashMap::default(),
            active_workspace_per_space: HashMap::default(),
            window_to_workspace: HashMap::default(),
            window_rule_floating: HashMap::default(),
            floating_positions: HashMap::default(),
            workspace_counter: 1,
            app_rules: config.app_rules.clone(),
            max_workspaces: 32,
            default_workspace_count: config.default_workspace_count,
            default_workspace_names: config.workspace_names.clone(),
            workspace_auto_back_and_forth: config.workspace_auto_back_and_forth,
        }
    }

    fn ensure_space_initialized(&mut self, space: SpaceId) {
        if self.workspaces_by_space.contains_key(&space) {
            return;
        }

        let mut ids = Vec::new();
        let count = self.default_workspace_count.max(1).min(self.max_workspaces);
        for i in 0..count {
            let name = self
                .default_workspace_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("Workspace {}", i + 1));
            let ws = VirtualWorkspace::new(name, space);
            let id = self.workspaces.insert(ws);
            ids.push(id);
        }
        self.workspaces_by_space.insert(space, ids.clone());

        if let Some(&first_id) = ids.first() {
            self.active_workspace_per_space.insert(space, (None, first_id));
        }
    }

    pub fn create_workspace(
        &mut self,
        space: SpaceId,
        name: Option<String>,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        self.ensure_space_initialized(space);
        let count = self.workspaces_by_space.get(&space).map(|v| v.len()).unwrap_or(0);
        if count >= self.max_workspaces {
            return Err(WorkspaceError::InconsistentState(format!(
                "Maximum workspace limit ({}) reached for space {:?}",
                self.max_workspaces, space
            )));
        }

        let name = name.unwrap_or_else(|| {
            let name = format!("Workspace {}", self.workspace_counter);
            self.workspace_counter += 1;
            name
        });

        let workspace = VirtualWorkspace::new(name, space);
        let workspace_id = self.workspaces.insert(workspace);
        self.workspaces_by_space.entry(space).or_default().push(workspace_id);

        Ok(workspace_id)
    }

    pub fn last_workspace(&self, space: SpaceId) -> Option<VirtualWorkspaceId> {
        self.active_workspace_per_space.get(&space)?.0
    }

    pub fn active_workspace(&self, space: SpaceId) -> Option<VirtualWorkspaceId> {
        self.active_workspace_per_space.get(&space).map(|tuple| tuple.1)
    }

    pub fn workspace_auto_back_and_forth(&self) -> bool { self.workspace_auto_back_and_forth }

    pub fn set_active_workspace(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> bool {
        trace_misc("set_active_workspace", || {
            let active = self.active_workspace_per_space.get(&space).map(|tuple| tuple.1);

            let result = if self.workspaces.contains_key(workspace_id)
                && self.workspaces.get(workspace_id).map(|w| w.space) == Some(space)
            {
                self.active_workspace_per_space.insert(space, (active, workspace_id));
                true
            } else {
                error!(
                    "Attempted to set non-existent or foreign workspace {:?} as active for {:?}",
                    workspace_id, space
                );
                false
            };

            result
        })
    }

    fn filtered_workspace_ids(
        &self,
        space: SpaceId,
        skip_empty: Option<bool>,
    ) -> Vec<VirtualWorkspaceId> {
        let ids = match self.workspaces_by_space.get(&space) {
            Some(v) => v,
            None => return Vec::new(),
        };

        let require_non_empty = skip_empty == Some(true);

        ids.iter()
            .copied()
            .filter(|id| {
                if let Some(ws) = self.workspaces.get(*id) {
                    !(require_non_empty && ws.windows.is_empty())
                } else {
                    false
                }
            })
            .collect()
    }

    fn step_workspace(
        &self,
        space: SpaceId,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
        dir: Direction,
    ) -> Option<VirtualWorkspaceId> {
        let base_ids: Vec<VirtualWorkspaceId> = if skip_empty == Some(true) {
            self.filtered_workspace_ids(space, Some(true))
        } else {
            self.workspaces_by_space.get(&space).cloned().unwrap_or_default()
        };

        if base_ids.is_empty() {
            return None;
        }

        if let Some(pos) = base_ids.iter().position(|&id| id == current) {
            let i = dir.step(pos, base_ids.len());
            return Some(base_ids[i]);
        }

        let fallback_ids = self.filtered_workspace_ids(space, Some(false));
        if fallback_ids.is_empty() {
            return None;
        }
        let start = fallback_ids.iter().position(|&id| id == current)?;
        let require_non_empty = skip_empty == Some(true);

        let mut i = dir.step(start, fallback_ids.len());
        if !require_non_empty {
            return Some(fallback_ids[i]);
        }

        for _ in 0..fallback_ids.len() {
            let id = fallback_ids[i];
            if self.workspaces.get(id).map_or(false, |ws| !ws.windows.is_empty()) {
                return Some(id);
            }
            i = dir.step(i, fallback_ids.len());
        }
        None
    }

    pub fn next_workspace(
        &self,
        space: SpaceId,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
    ) -> Option<VirtualWorkspaceId> {
        self.step_workspace(space, current, skip_empty, Direction::Right)
    }

    pub fn prev_workspace(
        &self,
        space: SpaceId,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
    ) -> Option<VirtualWorkspaceId> {
        self.step_workspace(space, current, skip_empty, Direction::Left)
    }

    pub fn assign_window_to_workspace(
        &mut self,
        space: SpaceId,
        window_id: WindowId,
        workspace_id: VirtualWorkspaceId,
    ) -> bool {
        trace_misc("assign_window_to_workspace", || {
            if !self.workspaces.contains_key(workspace_id)
                || self.workspaces.get(workspace_id).map(|w| w.space) != Some(space)
            {
                error!(
                    "Attempted to assign window to non-existent/foreign workspace {:?} for space {:?}",
                    workspace_id, space
                );
                return false;
            }

            let existing_mapping: Option<(SpaceId, VirtualWorkspaceId)> =
                self.window_to_workspace.iter().find_map(|(&(existing_space, wid), &ws_id)| {
                    if wid == window_id {
                        Some((existing_space, ws_id))
                    } else {
                        None
                    }
                });

            if let Some((existing_space, old_workspace_id)) = existing_mapping {
                if existing_space != space {
                    if let Some(old_workspace) = self.workspaces.get_mut(old_workspace_id) {
                        old_workspace.remove_window(window_id);
                    }
                    self.window_to_workspace.remove(&(existing_space, window_id));
                    self.window_rule_floating.remove(&(existing_space, window_id));
                } else {
                    if let Some(old_workspace) = self.workspaces.get_mut(old_workspace_id) {
                        old_workspace.remove_window(window_id);
                    }
                    self.window_to_workspace.remove(&(existing_space, window_id));
                    self.window_rule_floating.remove(&(existing_space, window_id));
                }
            }

            if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.add_window(window_id);
                self.window_to_workspace.insert((space, window_id), workspace_id);
                true
            } else {
                error!(
                    "Failed to get workspace {:?} for window assignment",
                    workspace_id
                );
                false
            }
        })
    }

    pub fn workspace_for_window(
        &self,
        space: SpaceId,
        window_id: WindowId,
    ) -> Option<VirtualWorkspaceId> {
        self.window_to_workspace.get(&(space, window_id)).copied()
    }

    pub fn remove_window(&mut self, window_id: WindowId) {
        let keys: Vec<(SpaceId, WindowId)> = self
            .window_to_workspace
            .keys()
            .copied()
            .filter(|(_, wid)| *wid == window_id)
            .collect();
        for (space, wid) in keys {
            if let Some(workspace_id) = self.window_to_workspace.remove(&(space, wid)) {
                if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                    workspace.remove_window(wid);
                }
                self.window_rule_floating.remove(&(space, wid));
            }
        }
    }

    pub fn remove_windows_for_app(&mut self, pid: pid_t) {
        let windows_to_remove: Vec<_> = self
            .window_to_workspace
            .keys()
            .filter_map(|(space, wid)| {
                if wid.pid == pid {
                    Some((*space, *wid))
                } else {
                    None
                }
            })
            .collect();

        for (space, window_id) in windows_to_remove {
            if let Some(ws_id) = self.window_to_workspace.remove(&(space, window_id)) {
                if let Some(workspace) = self.workspaces.get_mut(ws_id) {
                    workspace.remove_window(window_id);
                }
                self.window_rule_floating.remove(&(space, window_id));
            }
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
            .filter(|(id, workspace)| workspace.space == space && Some(*id) != active_workspace_id)
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
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        window_id: Option<WindowId>,
    ) {
        if self.workspaces.get(workspace_id).map(|w| w.space) == Some(space) {
            if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
                workspace.set_last_focused(window_id);
            }
        }
    }

    pub fn last_focused_window(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> Option<WindowId> {
        if self.workspaces.get(workspace_id).map(|w| w.space) == Some(space) {
            self.workspaces.get(workspace_id)?.last_focused()
        } else {
            None
        }
    }

    pub fn workspace_info(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> Option<&VirtualWorkspace> {
        if self.workspaces.get(workspace_id).map(|w| w.space) == Some(space) {
            self.workspaces.get(workspace_id)
        } else {
            None
        }
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

    pub fn list_workspaces(&mut self, space: SpaceId) -> Vec<(VirtualWorkspaceId, String)> {
        self.ensure_space_initialized(space);
        let ids = self.workspaces_by_space.get(&space).cloned().unwrap_or_default();
        let workspaces: Vec<_> = ids
            .into_iter()
            .filter_map(|id| self.workspaces.get(id).map(|ws| (id, ws.name.clone())))
            .collect();
        //workspaces.sort_by(|a, b| a.1.cmp(&b.1));
        workspaces
    }

    pub fn rename_workspace(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        new_name: String,
    ) -> bool {
        if self.workspaces.get(workspace_id).map(|w| w.space) != Some(space) {
            return false;
        }
        if let Some(workspace) = self.workspaces.get_mut(workspace_id) {
            workspace.name = new_name;

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
        if self.assign_window_to_workspace(space, window_id, default_workspace_id) {
            self.window_rule_floating.remove(&(space, window_id));
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
        window_title: Option<&str>,
        ax_role: Option<&str>,
        ax_subrole: Option<&str>,
    ) -> Result<(VirtualWorkspaceId, bool), WorkspaceError> {
        self.ensure_space_initialized(space);
        if self.workspaces_by_space.get(&space).map(|v| v.is_empty()).unwrap_or(true) {
            return Err(WorkspaceError::NoWorkspacesAvailable);
        }

        if let Some(existing_ws) = self.window_to_workspace.get(&(space, window_id)).copied() {
            let should_float =
                self.window_rule_floating.get(&(space, window_id)).copied().unwrap_or(false);
            return Ok((existing_ws, should_float));
        }

        let rule_match = self
            .find_matching_app_rule(app_bundle_id, app_name, window_title, ax_role, ax_subrole)
            .cloned();

        if let Some(rule) = rule_match {
            let target_workspace_id = if let Some(ref ws_sel) = rule.workspace {
                let maybe_idx: Option<usize> = match ws_sel {
                    WorkspaceSelector::Index(i) => Some(*i),
                    WorkspaceSelector::Name(name) => {
                        let workspaces = self.list_workspaces(space);
                        match workspaces.iter().position(|(_, n)| n == name) {
                            Some(idx) => Some(idx),
                            None => {
                                tracing::warn!(
                                    "App rule references workspace name '{}' which could not be resolved for space {:?}; falling back to default workspace",
                                    name,
                                    space
                                );
                                None
                            }
                        }
                    }
                };

                if let Some(workspace_idx) = maybe_idx {
                    let len = self.workspaces_by_space.get(&space).map(|v| v.len()).unwrap_or(0);
                    if workspace_idx >= len {
                        tracing::warn!(
                            "App rule references non-existent workspace index {}, falling back to active workspace",
                            workspace_idx
                        );
                        self.get_default_workspace(space)?
                    } else {
                        let workspaces = self.list_workspaces(space);
                        if let Some((workspace_id, _)) = workspaces.get(workspace_idx) {
                            *workspace_id
                        } else {
                            tracing::warn!(
                                "App rule references invalid workspace index {}, falling back to active workspace",
                                workspace_idx
                            );
                            self.get_default_workspace(space)?
                        }
                    }
                } else {
                    self.get_default_workspace(space)?
                }
            } else {
                self.get_default_workspace(space)?
            };

            if self.assign_window_to_workspace(space, window_id, target_workspace_id) {
                if rule.floating {
                    self.window_rule_floating.insert((space, window_id), true);
                } else {
                    self.window_rule_floating.remove(&(space, window_id));
                }
                return Ok((target_workspace_id, rule.floating));
            } else {
                error!("Failed to assign window to workspace from app rule");
            }
        }

        let default_workspace_id = self.get_default_workspace(space)?;
        if self.assign_window_to_workspace(space, window_id, default_workspace_id) {
            self.window_rule_floating.remove(&(space, window_id));
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
        self.ensure_space_initialized(space);
        if let Some(active_workspace_id) = self.active_workspace(space) {
            if self.workspaces.contains_key(active_workspace_id) {
                return Ok(active_workspace_id);
            } else {
                warn!("Active workspace no longer exists, clearing reference");
                self.active_workspace_per_space.remove(&space);
            }
        }

        let first_id = self
            .workspaces_by_space
            .get(&space)
            .and_then(|v| v.first().copied())
            .ok_or_else(|| {
                WorkspaceError::InconsistentState("No workspaces for space".to_string())
            })?;

        if self.set_active_workspace(space, first_id) {
            Ok(first_id)
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
        window_title: Option<&str>,
        ax_role: Option<&str>,
        ax_subrole: Option<&str>,
    ) -> Option<&AppWorkspaceRule> {
        let mut matches: Vec<(usize, &AppWorkspaceRule, usize)> = Vec::new();

        for (idx, rule) in self.app_rules.iter().enumerate() {
            if let Some(ref rule_app_id) = rule.app_id {
                match app_bundle_id {
                    Some(bundle_id) if rule_app_id == bundle_id => {}
                    _ => continue,
                }
            }

            if let Some(ref rule_name) = rule.app_name {
                match app_name {
                    Some(name) => {
                        if !(name.contains(rule_name) || rule_name.contains(name)) {
                            continue;
                        }
                    }
                    None => continue,
                }
            }

            if let Some(ref rule_re) = rule.title_regex {
                if rule_re.is_empty() {
                    continue;
                }
                match window_title {
                    Some(title) => match Regex::new(rule_re) {
                        Ok(re) => {
                            if !re.is_match(title) {
                                continue;
                            }
                        }
                        Err(e) => {
                            warn!("Invalid title_regex '{}' in app rule: {}", rule_re, e);
                            continue;
                        }
                    },
                    None => continue,
                }
            }

            if let Some(ref title_sub) = rule.title_substring {
                if title_sub.is_empty() {
                    continue;
                }
                match window_title {
                    Some(title) => {
                        if !title.contains(title_sub) {
                            continue;
                        }
                    }
                    None => continue,
                }
            }

            if let Some(ref rule_ax_role) = rule.ax_role {
                if rule_ax_role.is_empty() {
                    continue;
                }
                match ax_role {
                    Some(r) => {
                        if r != rule_ax_role.as_str() {
                            continue;
                        }
                    }
                    None => continue,
                }
            }

            if let Some(ref rule_ax_sub) = rule.ax_subrole {
                if rule_ax_sub.is_empty() {
                    continue;
                }
                match ax_subrole {
                    Some(sr) => {
                        if sr != rule_ax_sub.as_str() {
                            continue;
                        }
                    }
                    None => continue,
                }
            }

            let mut score = 0usize;
            if rule.app_id.as_ref().map_or(false, |s| !s.is_empty()) {
                score += 1;
            }
            if rule.app_name.as_ref().map_or(false, |s| !s.is_empty()) {
                score += 1;
            }
            if rule.title_regex.as_ref().map_or(false, |s| !s.is_empty()) {
                score += 1;
            }
            if rule.title_substring.as_ref().map_or(false, |s| !s.is_empty()) {
                score += 1;
            }
            if rule.ax_role.as_ref().map_or(false, |s| !s.is_empty()) {
                score += 1;
            }
            if rule.ax_subrole.as_ref().map_or(false, |s| !s.is_empty()) {
                score += 1;
            }
            if rule.workspace.is_some() {
                score += 1;
            }

            matches.push((idx, rule, score));
        }

        if matches.is_empty() {
            return None;
        }

        if matches.len() == 1 {
            return Some(matches[0].1);
        }

        let mut groups: HashMap<&str, Vec<&(usize, &AppWorkspaceRule, usize)>> = HashMap::default();
        for entry in &matches {
            if let Some(ref app_id) = entry.1.app_id {
                if !app_id.is_empty() {
                    groups.entry(app_id.as_str()).or_default().push(entry);
                }
            }
        }

        if !groups.is_empty() {
            let mut candidate_group_key: Option<&str> = None;
            let mut candidate_group_first_idx: Option<usize> = None;

            for (key, vec_entries) in groups.iter() {
                if vec_entries.len() > 1 {
                    let first_idx = vec_entries.iter().map(|e| e.0).min().unwrap_or(usize::MAX);
                    if candidate_group_key.is_none()
                        || first_idx < candidate_group_first_idx.unwrap()
                    {
                        candidate_group_key = Some(*key);
                        candidate_group_first_idx = Some(first_idx);
                    }
                }
            }

            if let Some(key) = candidate_group_key {
                if let Some(vec_entries) = groups.get(key) {
                    let best = vec_entries
                        .iter()
                        .copied()
                        .max_by(|a, b| a.2.cmp(&b.2).then_with(|| b.0.cmp(&a.0)));
                    if let Some(best_entry) = best {
                        return Some(best_entry.1);
                    }
                }
            }
        }

        let best_overall = matches.iter().max_by(|a, b| a.2.cmp(&b.2).then_with(|| b.0.cmp(&a.0)));

        best_overall.map(|(_, rule, _)| *rule)
    }

    pub fn get_stats(&self) -> WorkspaceStats {
        let mut stats = WorkspaceStats {
            total_workspaces: self.workspaces.len(),
            total_windows: self.window_to_workspace.len(),
            active_spaces: self.active_workspace_per_space.len(),
            workspace_window_counts: HashMap::default(),
        };

        for (workspace_id, workspace) in &self.workspaces {
            stats.workspace_window_counts.insert(workspace_id, workspace.window_count());
        }

        stats
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FloatingWindowPositions {
    #[serde_as(as = "HashMap<_, CGRectDef>")]
    positions: HashMap<WindowId, CGRect>,
}

impl FloatingWindowPositions {
    pub fn store_position(&mut self, window_id: WindowId, position: CGRect) {
        self.positions.insert(window_id, position);
    }

    pub fn get_position(&self, window_id: WindowId) -> Option<CGRect> {
        self.positions.get(&window_id).copied()
    }

    pub fn remove_position(&mut self, window_id: WindowId) -> Option<CGRect> {
        self.positions.remove(&window_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::app::WindowId;
    use crate::sys::screen::SpaceId;

    #[test]
    fn test_virtual_workspace_creation() {
        let mut manager = VirtualWorkspaceManager::new();

        let space = SpaceId::new(1);
        assert_eq!(
            manager.list_workspaces(space).len(),
            manager.workspaces_by_space.get(&space).map(|v| v.len()).unwrap_or(0)
        );

        let ws_id = manager.create_workspace(space, Some("Test Workspace".to_string())).unwrap();
        assert!(
            manager
                .list_workspaces(space)
                .iter()
                .any(|(id, name)| *id == ws_id && name == "Test Workspace")
        );

        let workspace = manager.workspace_info(space, ws_id).unwrap();
        assert_eq!(workspace.name, "Test Workspace");
    }

    #[test]
    fn test_window_assignment() {
        let mut manager = VirtualWorkspaceManager::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();

        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        assert!(manager.assign_window_to_workspace(space, window1, ws1_id));
        assert!(manager.assign_window_to_workspace(space, window2, ws2_id));

        assert_eq!(manager.workspace_for_window(space, window1), Some(ws1_id));
        assert_eq!(manager.workspace_for_window(space, window2), Some(ws2_id));

        let ws1 = manager.workspace_info(space, ws1_id).unwrap();
        let ws2 = manager.workspace_info(space, ws2_id).unwrap();

        assert!(ws1.contains_window(window1));
        assert!(!ws1.contains_window(window2));
        assert!(ws2.contains_window(window2));
        assert!(!ws2.contains_window(window1));
    }

    #[test]
    fn test_active_workspace_switching() {
        let mut manager = VirtualWorkspaceManager::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();

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
            let window_workspace = wm.workspace_for_window(space, window_id);
            let active_workspace = wm.active_workspace(space);

            match (window_workspace, active_workspace) {
                (Some(window_ws), Some(active_ws)) => window_ws == active_ws,
                _ => true,
            }
        }
        let mut manager = VirtualWorkspaceManager::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        manager.set_active_workspace(space, ws1_id);
        manager.assign_window_to_workspace(space, window1, ws1_id);
        manager.assign_window_to_workspace(space, window2, ws2_id);

        assert!(is_window_visible(&manager, window1, space));
        assert!(!is_window_visible(&manager, window2, space));

        manager.set_active_workspace(space, ws2_id);
        assert!(!is_window_visible(&manager, window1, space));
        assert!(is_window_visible(&manager, window2, space));
    }

    #[test]
    fn test_workspace_navigation() {
        let mut manager = VirtualWorkspaceManager::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let ws3_id = manager.create_workspace(space, Some("WS3".to_string())).unwrap();

        assert_eq!(manager.next_workspace(space, ws1_id, None), Some(ws2_id));
        assert_eq!(manager.next_workspace(space, ws2_id, None), Some(ws3_id));

        assert_eq!(manager.prev_workspace(space, ws2_id, None), Some(ws1_id));
        assert_eq!(manager.prev_workspace(space, ws3_id, None), Some(ws2_id));
    }

    #[test]
    fn test_app_rule_floating_state_persists() {
        let mut settings = VirtualWorkspaceSettings::default();
        settings.app_rules = vec![AppWorkspaceRule {
            app_id: Some("com.example.test".to_string()),
            workspace: None,
            floating: true,
            app_name: None,
            title_regex: None,
            title_substring: None,
            ax_role: None,
            ax_subrole: None,
        }];
        let mut manager = VirtualWorkspaceManager::new_with_config(&settings);
        let space = SpaceId::new(1);
        let window = WindowId::new(42, 7);

        let (_, should_float) = manager
            .assign_window_with_app_info(
                window,
                space,
                Some("com.example.test"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(should_float);
        assert_eq!(manager.window_rule_floating.get(&(space, window)), Some(&true));

        let (_, still_floats) = manager
            .assign_window_with_app_info(
                window,
                space,
                Some("com.example.test"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(still_floats);

        manager.remove_window(window);
        assert!(!manager.window_rule_floating.contains_key(&(space, window)));

        let (_, floats_again) = manager
            .assign_window_with_app_info(
                window,
                space,
                Some("com.example.test"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(floats_again);
    }
}
