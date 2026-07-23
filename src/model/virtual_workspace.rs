use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};
use slotmap::{SlotMap, new_key_type};
use tracing::{error, warn};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
#[cfg(test)]
use crate::common::config::AppWorkspaceRule;
use crate::common::config::{
    LayoutMode, LayoutSettings, MAX_WORKSPACES, VirtualWorkspaceSettings, WorkspaceSelector,
};
use crate::common::log::trace_misc;
use crate::layout_engine::Direction;
use crate::layout_engine::systems::LayoutSystemKind;
use crate::model::app_rules::AppRuleDecision;
use crate::model::hidden_window_placement::{HiddenWindowPlacement, HideCorner};
use crate::model::{WindowStore, WindowWorkspaceInfo};
use crate::sys::app::pid_t;
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

/// Details about an app rule assignment when Rift will manage the window.
#[derive(Debug, Clone, Copy)]
pub struct AppRuleAssignment {
    pub workspace_id: VirtualWorkspaceId,
    pub floating: bool,
    pub prev_rule_decision: bool,
}

/// Result of evaluating app rules for a window.
#[derive(Debug, Clone, Copy)]
pub enum AppRuleResult {
    Managed(AppRuleAssignment),
    Unmanaged,
}

/// Workspace-local configuration and layout state.
///
/// This intentionally does not own the set of member windows. Window-to-
/// workspace assignment is authoritative in `WindowStore`; the layout system
/// here is only the arrangement for the windows currently projected into this
/// workspace.
#[derive(Debug, Serialize, Deserialize)]
pub struct VirtualWorkspace {
    pub name: String,
    pub space: SpaceId,
    last_focused: Option<WindowId>,
    #[serde(default = "default_layout_system_kind")]
    pub layout_system: LayoutSystemKind,
    #[serde(default)]
    pub layout_mode: LayoutMode,
}

fn default_layout_system_kind() -> LayoutSystemKind {
    VirtualWorkspace::create_layout_system(LayoutMode::default(), &LayoutSettings::default())
}

impl VirtualWorkspace {
    fn new(name: String, space: SpaceId, mode: LayoutMode, settings: &LayoutSettings) -> Self {
        let layout_system = Self::create_layout_system(mode, settings);
        Self {
            name,
            space,
            last_focused: None,
            layout_system,
            layout_mode: mode,
        }
    }

    pub fn tree(&self) -> &LayoutSystemKind { &self.layout_system }

    pub fn tree_mut(&mut self) -> &mut LayoutSystemKind { &mut self.layout_system }

    pub fn layout_mode(&self) -> LayoutMode { self.layout_mode }

    pub fn create_layout_system(mode: LayoutMode, settings: &LayoutSettings) -> LayoutSystemKind {
        match mode {
            LayoutMode::Traditional => LayoutSystemKind::Traditional(
                crate::layout_engine::systems::TraditionalLayoutSystem::default(),
            ),
            LayoutMode::Bsp => {
                LayoutSystemKind::Bsp(crate::layout_engine::systems::BspLayoutSystem::default())
            }
            LayoutMode::Stack => {
                LayoutSystemKind::Stack(crate::layout_engine::systems::StackLayoutSystem::new(
                    settings.stack.default_orientation,
                ))
            }
            LayoutMode::MasterStack => LayoutSystemKind::MasterStack(
                crate::layout_engine::systems::MasterStackLayoutSystem::new(
                    settings.master_stack.clone(),
                ),
            ),
            LayoutMode::Scrolling => LayoutSystemKind::Scrolling(
                crate::layout_engine::systems::ScrollingLayoutSystem::new(&settings.scrolling),
            ),
        }
    }

    pub fn set_last_focused(&mut self, window_id: Option<WindowId>) {
        self.last_focused = window_id;
    }

    pub fn last_focused(&self) -> Option<WindowId> { self.last_focused }
}

/// Owns the virtual workspace topology for each native macOS space.
///
/// Membership is single-source-of-truth in `WindowStore`. Any code that
/// needs to answer "which workspace owns this window?" or "which windows belong
/// to this workspace?" must go through the store-backed helpers on this
/// manager. Keeping the mapping out of `VirtualWorkspace` prevents stale
/// duplicated membership from surviving topology churn or discovery refreshes.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceStore {
    pub(crate) workspaces: SlotMap<VirtualWorkspaceId, VirtualWorkspace>,
    workspaces_by_space: HashMap<SpaceId, Vec<VirtualWorkspaceId>>,
    pub active_workspace_per_space:
        HashMap<SpaceId, (Option<VirtualWorkspaceId>, VirtualWorkspaceId)>,
    workspace_counter: usize,
    #[cfg(test)]
    #[serde(skip)]
    test_app_rules: crate::model::AppRuleEngine,
    #[serde(skip)]
    max_workspaces: usize,
    #[serde(skip)]
    default_workspace_count: usize,
    #[serde(skip)]
    default_workspace_names: Vec<String>,
    #[serde(skip)]
    default_workspace: usize,
    #[serde(skip)]
    pub workspace_auto_back_and_forth: bool,
    #[serde(skip)]
    prevent_wrapping: bool,
    #[serde(skip)]
    pub workspace_rules: Vec<crate::common::config::WorkspaceLayoutRule>,
    #[serde(skip)]
    pub default_layout_mode: LayoutMode,
    #[serde(skip)]
    pub layout_settings: LayoutSettings,
}

impl Default for WorkspaceStore {
    fn default() -> Self { Self::new() }
}

impl WorkspaceStore {
    pub fn new() -> Self {
        Self::new_with_config(&VirtualWorkspaceSettings::default(), &LayoutSettings::default())
    }

    pub fn new_with_config(
        config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) -> Self {
        let target_count = config.default_workspace_count.max(1).min(MAX_WORKSPACES);
        let default_workspace = config.default_workspace.min(target_count - 1);

        Self {
            workspaces: SlotMap::default(),
            workspaces_by_space: HashMap::default(),
            active_workspace_per_space: HashMap::default(),
            workspace_counter: 1,
            #[cfg(test)]
            test_app_rules: crate::model::AppRuleEngine::new(&config.app_rules),
            max_workspaces: MAX_WORKSPACES,
            default_workspace_count: config.default_workspace_count,
            default_workspace_names: config.workspace_names.clone(),
            default_workspace,
            workspace_auto_back_and_forth: config.workspace_auto_back_and_forth,
            prevent_wrapping: config.prevent_wrapping,
            workspace_rules: config.workspace_rules.clone(),
            default_layout_mode: layout_settings.mode,
            layout_settings: layout_settings.clone(),
        }
    }

    pub fn update_settings(
        &mut self,
        config: &VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) {
        // Runtime-only limits are skipped by layout snapshots and therefore
        // deserialize to zero. Rehydrate them before doing count arithmetic.
        if self.max_workspaces == 0 {
            self.max_workspaces = MAX_WORKSPACES;
        }
        self.workspace_rules = config.workspace_rules.clone();
        self.default_layout_mode = layout_settings.mode;
        self.layout_settings = layout_settings.clone();
        self.default_workspace_count = config.default_workspace_count;
        self.default_workspace_names = config.workspace_names.clone();
        self.workspace_auto_back_and_forth = config.workspace_auto_back_and_forth;
        self.prevent_wrapping = config.prevent_wrapping;

        let target_count = self.default_workspace_count.max(1).min(self.max_workspaces);
        self.default_workspace = config.default_workspace.min(target_count - 1);

        let spaces: Vec<SpaceId> = self.workspaces_by_space.keys().copied().collect();
        for space in spaces {
            if let Some(workspaces) = self.workspaces_by_space.get_mut(&space) {
                workspaces.sort_unstable();
            }
            // Persisted workspace names are historical display metadata. Explicit names in the
            // current config remain authoritative after startup restore and config reload.
            if let Some(workspaces) = self.workspaces_by_space.get(&space) {
                for (index, &workspace) in workspaces.iter().enumerate() {
                    if let Some(name) = self.default_workspace_names.get(index)
                        && let Some(workspace) = self.workspaces.get_mut(workspace)
                    {
                        workspace.name = name.clone();
                    }
                }
            }
            while self.workspaces_by_space.get(&space).unwrap().len() < target_count {
                let idx = self.workspaces_by_space.get(&space).unwrap().len();
                let name = if let Some(n) = self.default_workspace_names.get(idx) {
                    n.clone()
                } else {
                    let name = format!("Workspace {}", self.workspace_counter);
                    self.workspace_counter += 1;
                    name
                };

                let mode = self.resolve_layout_mode_for_workspace(idx, &name);
                let ws = VirtualWorkspace::new(name, space, mode, &self.layout_settings);
                let id = self.workspaces.insert(ws);
                self.workspaces_by_space.get_mut(&space).unwrap().push(id);
            }
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

            let mode = self.resolve_layout_mode_for_workspace(i, &name);
            let ws = VirtualWorkspace::new(name, space, mode, &self.layout_settings);
            let id = self.workspaces.insert(ws);
            ids.push(id);
        }
        self.workspaces_by_space.insert(space, ids.clone());

        let default_idx = self.default_workspace.min(ids.len() - 1);
        if let Some(&default_id) = ids.get(default_idx) {
            self.active_workspace_per_space.insert(space, (None, default_id));
        }
    }

    fn resolve_layout_mode_for_workspace(&self, index: usize, name: &str) -> LayoutMode {
        // Check workspace_rules (last matching rule wins, like app_rules)
        for rule in self.workspace_rules.iter().rev() {
            match &rule.workspace {
                WorkspaceSelector::Index(idx) if *idx == index => return rule.layout,
                WorkspaceSelector::Name(n) if n == name => return rule.layout,
                _ => continue,
            }
        }
        // Fall back to global default
        self.default_layout_mode
    }

    pub fn desired_layout_mode_for_workspace(&self, index: usize, name: &str) -> LayoutMode {
        self.resolve_layout_mode_for_workspace(index, name)
    }

    pub fn initialized_spaces(&self) -> Vec<SpaceId> {
        let mut spaces = self.workspaces_by_space.keys().copied().collect::<Vec<_>>();
        spaces.sort_unstable();
        spaces
    }

    /// Validate the serialized workspace graph before layout code indexes into slotmaps.
    ///
    /// Persistence files are user-visible and may be old, truncated, or manually edited. Loading
    /// malformed topology must return a useful error instead of panicking later through indexing.
    pub(crate) fn validate_persisted_topology(&self) -> Result<(), String> {
        let mut indexed = HashSet::default();
        for (&space, workspaces) in &self.workspaces_by_space {
            if workspaces.is_empty() {
                return Err(format!("native space {} has no virtual workspaces", space.get()));
            }
            for &workspace in workspaces {
                let Some(entry) = self.workspaces.get(workspace) else {
                    return Err(format!(
                        "native space {} references missing workspace {workspace:?}",
                        space.get()
                    ));
                };
                if entry.space != space {
                    return Err(format!(
                        "workspace {workspace:?} belongs to space {} but is indexed under {}",
                        entry.space.get(),
                        space.get()
                    ));
                }
                if !indexed.insert(workspace) {
                    return Err(format!("workspace {workspace:?} is indexed more than once"));
                }
            }

            let Some(&(previous, active)) = self.active_workspace_per_space.get(&space) else {
                return Err(format!("native space {} has no active workspace", space.get()));
            };
            if !workspaces.contains(&active) {
                return Err(format!(
                    "native space {} has an invalid active workspace",
                    space.get()
                ));
            }
            if previous.is_some_and(|previous| !workspaces.contains(&previous)) {
                return Err(format!(
                    "native space {} has an invalid previous workspace",
                    space.get()
                ));
            }
        }

        for (workspace, entry) in &self.workspaces {
            if !indexed.contains(&workspace) {
                return Err(format!(
                    "workspace {workspace:?} for native space {} is not indexed",
                    entry.space.get()
                ));
            }
        }
        for space in self.active_workspace_per_space.keys() {
            if !self.workspaces_by_space.contains_key(space) {
                return Err(format!(
                    "active workspace state references unknown native space {}",
                    space.get()
                ));
            }
        }
        Ok(())
    }

    pub fn remap_space(
        &mut self,
        window_store: &mut WindowStore,
        old_space: SpaceId,
        new_space: SpaceId,
    ) {
        if old_space == new_space || !self.workspaces_by_space.contains_key(&old_space) {
            return;
        }

        // Remove any auto-created state for the target space; the migrated state
        // should be authoritative.
        let mut deleted_target_workspace_ids = Vec::new();
        if let Some(existing) = self.workspaces_by_space.remove(&new_space) {
            for ws_id in existing {
                if let Some(ws) = self.workspaces.get(ws_id) {
                    if ws.space == new_space {
                        self.workspaces.remove(ws_id);
                        deleted_target_workspace_ids.push(ws_id);
                    }
                }
            }
        }
        self.active_workspace_per_space.remove(&new_space);

        if !deleted_target_workspace_ids.is_empty() {
            let stale_windows: Vec<_> = window_store
                .iter_workspace_assignments()
                .filter_map(|(window_id, assignment)| {
                    deleted_target_workspace_ids
                        .contains(&assignment.workspace_id)
                        .then_some(window_id)
                })
                .collect();
            for window_id in stale_windows {
                let _ = window_store.remove_window_assignment(window_id);
            }
        }

        let ids = self.workspaces_by_space.remove(&old_space).unwrap_or_default();
        for ws_id in &ids {
            if let Some(ws) = self.workspaces.get_mut(*ws_id) {
                ws.space = new_space;
            }
        }
        if !ids.is_empty() {
            self.workspaces_by_space.insert(new_space, ids.clone());
        }

        if let Some((last, active)) = self.active_workspace_per_space.remove(&old_space) {
            self.active_workspace_per_space.insert(new_space, (last, active));
        }

        window_store.remap_space(old_space, new_space);
    }

    pub fn create_workspace(
        &mut self,
        space: SpaceId,
        name: Option<String>,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        self.ensure_space_initialized(space);
        let count = self
            .workspaces_by_space
            .get(&space)
            .map(|v: &Vec<VirtualWorkspaceId>| v.len())
            .unwrap_or(0);
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

        let idx = self
            .workspaces_by_space
            .get(&space)
            .map(|v: &Vec<VirtualWorkspaceId>| v.len())
            .unwrap_or(0);
        let mode = self.resolve_layout_mode_for_workspace(idx, &name);

        let workspace = VirtualWorkspace::new(name, space, mode, &self.layout_settings);
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

    pub fn active_workspace_idx(&self, space: SpaceId) -> Option<u64> {
        self.active_workspace(space).and_then(|active_ws_id| {
            self.ordered_workspace_ids(space)
                .iter()
                .position(|id| *id == active_ws_id)
                .map(|idx| idx as u64)
        })
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

    fn step_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
        dir: Direction,
    ) -> Option<VirtualWorkspaceId> {
        let ids = self.ordered_workspace_ids(space);
        if ids.is_empty() {
            return None;
        }
        let mut index = ids.iter().position(|&id| id == current)?;
        let require_non_empty = skip_empty == Some(true);

        for _ in 0..ids.len() {
            index = match dir {
                Direction::Right if index + 1 < ids.len() => index + 1,
                Direction::Left if index > 0 => index - 1,
                Direction::Right if !self.prevent_wrapping => 0,
                Direction::Left if !self.prevent_wrapping => ids.len() - 1,
                _ => return None,
            };

            let id = ids[index];
            if !require_non_empty || !self.workspace_windows(window_store, space, id).is_empty() {
                return Some(id);
            }
        }
        None
    }

    pub fn next_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
    ) -> Option<VirtualWorkspaceId> {
        self.step_workspace(window_store, space, current, skip_empty, Direction::Right)
    }

    pub fn prev_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        current: VirtualWorkspaceId,
        skip_empty: Option<bool>,
    ) -> Option<VirtualWorkspaceId> {
        self.step_workspace(window_store, space, current, skip_empty, Direction::Left)
    }

    pub fn assign_window_to_workspace(
        &mut self,
        window_store: &mut WindowStore,
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

            let previous_assignment = window_store.workspace_info_for_window(window_id);
            window_store
                .assign_window_to_workspace(window_id, WindowWorkspaceInfo { space, workspace_id });
            if let Some(previous_assignment) = previous_assignment
                && previous_assignment.workspace_id != workspace_id
                && let Some(previous_workspace) =
                    self.workspaces.get_mut(previous_assignment.workspace_id)
                && previous_workspace.space == previous_assignment.space
                && previous_workspace.last_focused() == Some(window_id)
            {
                previous_workspace.set_last_focused(None);
            }
            true
        })
    }

    /// Moves a window to `space` while retaining the ordinal of its current
    /// virtual workspace. This is used for native-space identity churn, where
    /// WindowServer can briefly report a different space without a user moving
    /// the window to the destination's active workspace.
    pub fn assign_window_to_workspace_preserving_ordinal(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        window_id: WindowId,
    ) -> Option<VirtualWorkspaceId> {
        self.ensure_space_initialized(space);

        let existing_assignment = window_store.workspace_info_for_window(window_id)?;
        if existing_assignment.space == space {
            return Some(existing_assignment.workspace_id);
        }

        let source_index = self
            .ordered_workspace_ids(existing_assignment.space)
            .iter()
            .position(|&workspace_id| workspace_id == existing_assignment.workspace_id)?;
        let target_workspace_id = *self.ordered_workspace_ids(space).get(source_index)?;

        self.assign_window_to_workspace(window_store, space, window_id, target_workspace_id)
            .then_some(target_workspace_id)
    }

    pub fn workspace_for_window(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        window_id: WindowId,
    ) -> Option<VirtualWorkspaceId> {
        window_store.workspace_for_window(space, window_id)
    }

    pub fn workspace_for_window_any(
        &self,
        window_store: &WindowStore,
        window_id: WindowId,
    ) -> Option<VirtualWorkspaceId> {
        window_store.workspace_info_for_window(window_id).map(|info| info.workspace_id)
    }

    pub fn workspace_info_for_window_any(
        &self,
        window_store: &WindowStore,
        window_id: WindowId,
    ) -> Option<WindowWorkspaceInfo> {
        window_store.workspace_info_for_window(window_id)
    }

    pub fn workspaces_for_window(
        &self,
        window_store: &WindowStore,
        window_id: WindowId,
    ) -> Vec<VirtualWorkspaceId> {
        window_store.workspaces_for_window(window_id)
    }

    pub fn set_last_rule_decision(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        window_id: WindowId,
        value: bool,
    ) {
        let _ = space;
        window_store.set_last_rule_decision(window_id, value);
    }

    pub fn remove_window(&mut self, window_store: &mut WindowStore, window_id: WindowId) {
        let _ = window_store.remove_window_assignment(window_id);
        window_store.clear_rule_metadata(window_id);
    }

    pub fn remove_windows_for_app(&mut self, window_store: &mut WindowStore, pid: pid_t) {
        let windows_to_remove: Vec<_> = window_store
            .iter_workspace_assignments()
            .map(|(window_id, _)| window_id)
            .filter(|wid| wid.pid == pid)
            .collect();

        for window_id in windows_to_remove {
            let _ = window_store.remove_window_assignment(window_id);
            window_store.clear_rule_metadata(window_id);
        }
    }

    /// Gets all windows in the active virtual workspace for a given native space.
    pub fn windows_in_active_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
    ) -> Vec<WindowId> {
        if let Some(workspace_id) = self.active_workspace(space) {
            return self.workspace_windows(window_store, space, workspace_id);
        }
        Vec::new()
    }

    pub fn is_window_in_active_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        window_id: WindowId,
    ) -> bool {
        if let Some(active_workspace_id) = self.active_workspace(space) {
            if let Some(window_workspace_id) = window_store.workspace_for_window(space, window_id) {
                return window_workspace_id == active_workspace_id;
            }
        }
        true
    }

    pub fn windows_in_inactive_workspaces(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
    ) -> Vec<WindowId> {
        let active_workspace_id = self.active_workspace(space);

        self.workspaces
            .iter()
            .filter(|(id, workspace)| workspace.space == space && Some(*id) != active_workspace_id)
            .flat_map(|(id, _)| self.workspace_windows(window_store, space, id))
            .collect()
    }

    pub fn find_window_by_idx(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        idx: u32,
    ) -> Option<WindowId> {
        window_store
            .iter_workspace_assignments()
            .find_map(|(wid, info)| (info.space == space && wid.idx.get() == idx).then_some(wid))
    }

    pub fn find_window_in_workspace_by_idx(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        idx: u32,
    ) -> Option<WindowId> {
        if self.workspaces.get(workspace_id).map(|w| w.space) != Some(space) {
            return None;
        }

        self.workspaces.get(workspace_id).and_then(|_| {
            self.workspace_windows(window_store, space, workspace_id)
                .into_iter()
                .find(|wid| wid.idx.get() == idx)
        })
    }

    pub fn calculate_hidden_position(
        &self,
        screen_frame: CGRect,
        original_frame: CGRect,
        corner: HideCorner,
        _app_bundle_id: Option<&str>,
    ) -> CGRect {
        HiddenWindowPlacement::calculate(screen_frame, original_frame, corner, &[])
    }

    pub fn calculate_hidden_position_multi(
        &self,
        screen_frame: CGRect,
        original_frame: CGRect,
        corner: HideCorner,
        _app_bundle_id: Option<&str>,
        all_screens: &[CGRect],
    ) -> CGRect {
        let others: Vec<_> =
            all_screens.iter().copied().filter(|screen| *screen != screen_frame).collect();
        HiddenWindowPlacement::calculate(screen_frame, original_frame, corner, &others)
    }

    pub fn is_hidden_position(
        &self,
        screen_frame: &CGRect,
        rect: &CGRect,
        _app_bundle_id: Option<&str>,
    ) -> bool {
        HiddenWindowPlacement::is_hidden(*screen_frame, *rect, &[])
    }

    pub fn is_hidden_position_multi(
        &self,
        screen_frame: &CGRect,
        rect: &CGRect,
        _app_bundle_id: Option<&str>,
        all_screens: &[CGRect],
    ) -> bool {
        let others: Vec<_> =
            all_screens.iter().copied().filter(|screen| *screen != *screen_frame).collect();
        HiddenWindowPlacement::is_hidden(*screen_frame, *rect, &others)
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

    pub fn transfer_window_identity(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }
        for workspace in self.workspaces.values_mut() {
            let contained_from = workspace.last_focused() == Some(from);
            if workspace.last_focused() == Some(to) {
                workspace.set_last_focused(None);
            }
            if contained_from {
                workspace.set_last_focused(Some(to));
            }
        }
    }

    pub(crate) fn forget_window_identity(&mut self, window: WindowId) {
        for workspace in self.workspaces.values_mut() {
            if workspace.last_focused() == Some(window) {
                workspace.set_last_focused(None);
            }
        }
    }

    pub(crate) fn retain_window_focus_location(
        &mut self,
        window: WindowId,
        keep: VirtualWorkspaceId,
    ) {
        for (workspace_id, workspace) in self.workspaces.iter_mut() {
            if workspace_id != keep && workspace.last_focused() == Some(window) {
                workspace.set_last_focused(None);
            }
        }
    }

    pub fn list_workspaces(&mut self, space: SpaceId) -> Vec<(VirtualWorkspaceId, String)> {
        self.ensure_space_initialized(space);
        self.existing_workspaces(space)
    }

    /// Read workspace topology without creating missing state. Validation and restore planning
    /// must use this accessor so a failed transaction cannot initialize part of the live engine.
    pub(crate) fn existing_workspaces(&self, space: SpaceId) -> Vec<(VirtualWorkspaceId, String)> {
        self.ordered_workspace_ids(space)
            .into_iter()
            .filter_map(|id| self.workspaces.get(id).map(|ws| (id, ws.name.clone())))
            .collect()
    }

    /// Workspace index is creation/configuration order, represented by the stable slot-map key.
    /// Serialized vectors are historical implementation detail and may have been reordered by an
    /// older restore. All ordinal behavior must go through this canonical view.
    fn ordered_workspace_ids(&self, space: SpaceId) -> Vec<VirtualWorkspaceId> {
        let mut ids = self.workspaces_by_space.get(&space).cloned().unwrap_or_default();
        ids.retain(|id| self.workspaces.get(*id).is_some_and(|workspace| workspace.space == space));
        ids.sort_unstable();
        ids
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

    pub fn workspace_windows(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> Vec<WindowId> {
        if self.workspaces.get(workspace_id).map(|workspace| workspace.space) == Some(space) {
            return window_store.workspace_windows(space, workspace_id);
        }
        Vec::new()
    }

    pub fn auto_assign_window(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
    ) -> Result<VirtualWorkspaceId, WorkspaceError> {
        let default_workspace_id = self.get_default_workspace(space)?;
        if self.assign_window_to_workspace(window_store, space, window_id, default_workspace_id) {
            window_store.clear_rule_floating(window_id);
            Ok(default_workspace_id)
        } else {
            Err(WorkspaceError::AssignmentFailed)
        }
    }

    fn preserved_workspace_assignment(
        &self,
        window_store: &WindowStore,
        window_id: WindowId,
        space: SpaceId,
    ) -> Option<WindowWorkspaceInfo> {
        let existing_assignment = window_store.workspace_info_for_window(window_id)?;
        if existing_assignment.space == space {
            return Some(existing_assignment);
        }

        // Treat an empty, newly initialized target space as a transient native-space-id churn
        // candidate and preserve workspace ownership by ordinal. Once the target space already
        // has assignments, prefer the normal resolution path so real cross-space moves still
        // follow the destination space.
        if window_store.has_workspace_assignments_in_space(space) {
            return None;
        }

        let source_index = self
            .ordered_workspace_ids(existing_assignment.space)
            .iter()
            .position(|&workspace_id| workspace_id == existing_assignment.workspace_id)?;
        let target_workspace_id = *self.ordered_workspace_ids(space).get(source_index)?;
        Some(WindowWorkspaceInfo {
            space,
            workspace_id: target_workspace_id,
        })
    }

    fn ensure_window_assignment(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        assignment: WindowWorkspaceInfo,
    ) -> bool {
        if window_store.workspace_info_for_window(window_id) == Some(assignment) {
            true
        } else {
            self.assign_window_to_workspace(
                window_store,
                assignment.space,
                window_id,
                assignment.workspace_id,
            )
        }
    }

    pub(crate) fn apply_app_rule_decision(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
        rule_decision: AppRuleDecision,
    ) -> Result<AppRuleResult, WorkspaceError> {
        let prev_rule_decision = window_store.last_rule_decision(window_id);

        self.ensure_space_initialized(space);
        if self
            .workspaces_by_space
            .get(&space)
            .map(|v: &Vec<VirtualWorkspaceId>| v.is_empty())
            .unwrap_or(true)
        {
            return Err(WorkspaceError::NoWorkspacesAvailable);
        }

        let existing_assignment =
            self.preserved_workspace_assignment(window_store, window_id, space);

        if rule_decision == AppRuleDecision::Unmanaged {
            window_store.clear_rule_floating(window_id);
            return Ok(AppRuleResult::Unmanaged);
        }

        if let AppRuleDecision::Managed { workspace, floating } = rule_decision {
            let target_workspace_id = if let Some(ref ws_sel) = workspace {
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
                    let len = self
                        .workspaces_by_space
                        .get(&space)
                        .map(|v: &Vec<VirtualWorkspaceId>| v.len())
                        .unwrap_or(0);
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
                } else if let Some(existing_assignment) = existing_assignment {
                    existing_assignment.workspace_id
                } else {
                    self.get_default_workspace(space)?
                }
            } else {
                if let Some(existing_assignment) = existing_assignment {
                    existing_assignment.workspace_id
                } else {
                    self.get_default_workspace(space)?
                }
            };

            if let Some(existing_assignment) = existing_assignment {
                if !self.ensure_window_assignment(window_store, window_id, existing_assignment) {
                    error!("Failed to preserve window workspace assignment from app rule");
                    return Err(WorkspaceError::AssignmentFailed);
                }
                window_store.set_rule_floating(window_id, floating);
                return Ok(AppRuleResult::Managed(AppRuleAssignment {
                    workspace_id: existing_assignment.workspace_id,
                    floating,
                    prev_rule_decision,
                }));
            }

            if self.assign_window_to_workspace(window_store, space, window_id, target_workspace_id)
            {
                window_store.set_rule_floating(window_id, floating);
                return Ok(AppRuleResult::Managed(AppRuleAssignment {
                    workspace_id: target_workspace_id,
                    floating,
                    prev_rule_decision,
                }));
            } else {
                error!("Failed to assign window to workspace from app rule");
            }
        }

        // No matching app rule: preserve the current workspace assignment if one
        // already exists. Discovery/refresh passes must not silently fall back to
        // the default workspace, or windows on non-default workspaces will appear
        // to "reset" after sleep/display churn.
        if let Some(existing_assignment) = existing_assignment {
            if !self.ensure_window_assignment(window_store, window_id, existing_assignment) {
                error!("Failed to preserve existing window workspace assignment");
                return Err(WorkspaceError::AssignmentFailed);
            }
            window_store.clear_rule_floating(window_id);
            return Ok(AppRuleResult::Managed(AppRuleAssignment {
                workspace_id: existing_assignment.workspace_id,
                floating: false,
                prev_rule_decision,
            }));
        }

        let default_workspace_id = self.get_default_workspace(space)?;
        if self.assign_window_to_workspace(window_store, space, window_id, default_workspace_id) {
            window_store.clear_rule_floating(window_id);
            Ok(AppRuleResult::Managed(AppRuleAssignment {
                workspace_id: default_workspace_id,
                floating: false,
                prev_rule_decision,
            }))
        } else {
            error!("Failed to assign window to default workspace");
            Err(WorkspaceError::AssignmentFailed)
        }
    }

    #[cfg(test)]
    fn assign_window_with_app_info(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
        app_bundle_id: Option<&str>,
        app_name: Option<&str>,
        window_title: Option<&str>,
        ax_role: Option<&str>,
        ax_subrole: Option<&str>,
    ) -> Result<AppRuleResult, WorkspaceError> {
        let decision = self.test_app_rules.evaluate(crate::model::WindowRuleContext {
            app_bundle_id,
            app_name,
            window_title,
            ax_role,
            ax_subrole,
        });
        self.apply_app_rule_decision(window_store, window_id, space, decision)
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

        let first_id = self.ordered_workspace_ids(space).first().copied().ok_or_else(|| {
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

    pub fn get_stats(&self, window_store: &WindowStore) -> WorkspaceStats {
        let mut stats = WorkspaceStats {
            total_workspaces: self.workspaces.len(),
            total_windows: window_store.workspace_assignment_count(),
            active_spaces: self.active_workspace_per_space.len(),
            workspace_window_counts: HashMap::default(),
        };

        for (workspace_id, workspace) in &self.workspaces {
            stats.workspace_window_counts.insert(
                workspace_id,
                window_store.workspace_window_count(workspace.space, workspace_id),
            );
        }

        stats
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
    use objc2_core_foundation::{CGPoint, CGSize};

    use super::*;
    use crate::actor::app::WindowId;
    use crate::sys::screen::SpaceId;

    fn expect_managed(result: Result<AppRuleResult, WorkspaceError>) -> AppRuleAssignment {
        match result {
            Ok(AppRuleResult::Managed(decision)) => decision,
            Ok(AppRuleResult::Unmanaged) => {
                panic!("App rule unexpectedly marked window as unmanaged")
            }
            Err(e) => panic!("assign_window_with_app_info failed: {:?}", e),
        }
    }

    fn assign(
        manager: &mut WorkspaceStore,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
        app_id: Option<&str>,
        app_name: Option<&str>,
        window_title: Option<&str>,
        ax_role: Option<&str>,
        ax_subrole: Option<&str>,
    ) -> AppRuleAssignment {
        expect_managed(manager.assign_window_with_app_info(
            window_store,
            window_id,
            space,
            app_id,
            app_name,
            window_title,
            ax_role,
            ax_subrole,
        ))
    }

    #[test]
    fn test_virtual_workspace_creation() {
        let mut manager = WorkspaceStore::new();

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
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();

        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window1, ws1_id));
        assert!(manager.assign_window_to_workspace(&mut window_store, space, window2, ws2_id));

        assert_eq!(
            manager.workspace_for_window(&window_store, space, window1),
            Some(ws1_id)
        );
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window2),
            Some(ws2_id)
        );

        assert_eq!(manager.workspace_windows(&window_store, space, ws1_id), vec![
            window1
        ]);
        assert_eq!(manager.workspace_windows(&window_store, space, ws2_id), vec![
            window2
        ]);
    }

    #[test]
    fn reassignment_updates_authoritative_workspace_index() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let window = WindowId::new(9, 1);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws1_id));
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window),
            Some(ws1_id)
        );
        assert_eq!(manager.workspace_windows(&window_store, space, ws1_id), vec![
            window
        ]);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws2_id));
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window),
            Some(ws2_id)
        );
        assert!(manager.workspace_windows(&window_store, space, ws1_id).is_empty());
        assert_eq!(manager.workspace_windows(&window_store, space, ws2_id), vec![
            window
        ]);
    }

    #[test]
    fn reassignment_clears_stale_last_focused_on_source_workspace() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let window = WindowId::new(9, 1);

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws1_id));
        manager.set_last_focused_window(space, ws1_id, Some(window));

        assert!(manager.assign_window_to_workspace(&mut window_store, space, window, ws2_id));

        assert_eq!(manager.last_focused_window(space, ws1_id), None);
        assert_eq!(
            manager.workspace_for_window(&window_store, space, window),
            Some(ws2_id)
        );
    }

    #[test]
    fn remap_space_drops_assignments_to_deleted_target_workspaces() {
        let mut window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let migrated_ws = manager.create_workspace(old_space, Some("Old".to_string())).unwrap();
        let transient_ws =
            manager.create_workspace(new_space, Some("Transient".to_string())).unwrap();
        let migrated_window = WindowId::new(10, 1);
        let transient_window = WindowId::new(11, 1);

        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            migrated_window,
            migrated_ws
        ));
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            new_space,
            transient_window,
            transient_ws
        ));

        manager.remap_space(&mut window_store, old_space, new_space);

        assert_eq!(
            manager.workspace_for_window(&window_store, new_space, migrated_window),
            Some(migrated_ws)
        );
        assert_eq!(
            manager.workspace_windows(&window_store, new_space, migrated_ws),
            vec![migrated_window]
        );
        assert_eq!(
            manager.workspace_info_for_window_any(&window_store, transient_window),
            None
        );
        assert!(manager.workspace_windows(&window_store, new_space, transient_ws).is_empty());
        assert!(manager.workspace_info(new_space, transient_ws).is_none());
    }

    #[test]
    fn preserves_workspace_ordinal_across_transient_space_id_churn() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let window = WindowId::new(12, 1);

        let old_workspaces = manager.list_workspaces(old_space);
        let new_workspaces = manager.list_workspaces(new_space);
        let preserved_workspace = old_workspaces[2].0;
        let expected_target_workspace = new_workspaces[2].0;

        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            window,
            preserved_workspace
        ));

        let assignment = assign(
            &mut manager,
            &mut window_store,
            window,
            new_space,
            None,
            None,
            None,
            None,
            None,
        );

        assert_eq!(assignment.workspace_id, expected_target_workspace);
        assert_eq!(
            manager.workspace_info_for_window_any(&window_store, window),
            Some(WindowWorkspaceInfo {
                space: new_space,
                workspace_id: expected_target_workspace,
            })
        );
    }

    #[test]
    fn does_not_preserve_workspace_ordinal_when_target_space_already_has_assignments() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let moved_window = WindowId::new(13, 1);
        let existing_window = WindowId::new(14, 1);

        let old_workspaces = manager.list_workspaces(old_space);
        let new_workspaces = manager.list_workspaces(new_space);

        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            moved_window,
            old_workspaces[2].0
        ));
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            new_space,
            existing_window,
            new_workspaces[1].0
        ));

        let assignment = assign(
            &mut manager,
            &mut window_store,
            moved_window,
            new_space,
            None,
            None,
            None,
            None,
            None,
        );

        assert_eq!(assignment.workspace_id, new_workspaces[0].0);
        assert_eq!(
            manager.workspace_info_for_window_any(&window_store, moved_window),
            Some(WindowWorkspaceInfo {
                space: new_space,
                workspace_id: new_workspaces[0].0,
            })
        );
    }

    #[test]
    fn topology_reassignment_preserves_workspace_ordinal_with_destination_assignments() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let source_space = SpaceId::new(1);
        let destination_space = SpaceId::new(2);
        let moved_window = WindowId::new(15, 1);
        let destination_window = WindowId::new(16, 1);

        let source_workspaces = manager.list_workspaces(source_space);
        let destination_workspaces = manager.list_workspaces(destination_space);
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            source_space,
            moved_window,
            source_workspaces[2].0
        ));
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            destination_space,
            destination_window,
            destination_workspaces[0].0
        ));

        assert_eq!(
            manager.assign_window_to_workspace_preserving_ordinal(
                &mut window_store,
                destination_space,
                moved_window
            ),
            Some(destination_workspaces[2].0)
        );
        assert_eq!(
            manager.workspace_info_for_window_any(&window_store, moved_window),
            Some(WindowWorkspaceInfo {
                space: destination_space,
                workspace_id: destination_workspaces[2].0,
            })
        );
    }

    #[test]
    fn unmanaged_rule_does_not_reassign_window_during_transient_space_id_churn() {
        let mut window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 3;
        settings.app_rules = vec![AppWorkspaceRule {
            app_id: Some("com.example.unmanaged".into()),
            workspace: None,
            floating: false,
            manage: false,
            app_name: None,
            title_regex: None,
            title_substring: None,
            ax_role: None,
            ax_subrole: None,
        }];
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let old_space = SpaceId::new(1);
        let new_space = SpaceId::new(2);
        let window = WindowId::new(15, 1);

        let old_workspaces = manager.list_workspaces(old_space);
        let old_assignment = WindowWorkspaceInfo {
            space: old_space,
            workspace_id: old_workspaces[2].0,
        };
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            old_space,
            window,
            old_assignment.workspace_id
        ));

        let result = manager.assign_window_with_app_info(
            &mut window_store,
            window,
            new_space,
            Some("com.example.unmanaged"),
            None,
            None,
            None,
            None,
        );

        assert!(matches!(result, Ok(AppRuleResult::Unmanaged)));
        assert_eq!(
            manager.workspace_for_window(&window_store, new_space, window),
            None
        );
        assert_eq!(
            manager.workspace_info_for_window_any(&window_store, window),
            Some(old_assignment)
        );
    }

    #[test]
    fn test_active_workspace_switching() {
        let mut manager = WorkspaceStore::new();
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
        let mut window_store = WindowStore::default();
        fn is_window_visible(
            wm: &WorkspaceStore,
            window_store: &WindowStore,
            window_id: WindowId,
            space: SpaceId,
        ) -> bool {
            let window_workspace = wm.workspace_for_window(window_store, space, window_id);
            let active_workspace = wm.active_workspace(space);

            match (window_workspace, active_workspace) {
                (Some(window_ws), Some(active_ws)) => window_ws == active_ws,
                _ => true,
            }
        }
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let window1 = WindowId::new(1, 1);
        let window2 = WindowId::new(1, 2);

        manager.set_active_workspace(space, ws1_id);
        manager.assign_window_to_workspace(&mut window_store, space, window1, ws1_id);
        manager.assign_window_to_workspace(&mut window_store, space, window2, ws2_id);

        assert!(is_window_visible(&manager, &window_store, window1, space));
        assert!(!is_window_visible(&manager, &window_store, window2, space));

        manager.set_active_workspace(space, ws2_id);
        assert!(!is_window_visible(&manager, &window_store, window1, space));
        assert!(is_window_visible(&manager, &window_store, window2, space));
    }

    #[test]
    fn default_workspace_setting_applied() {
        let mut settings = VirtualWorkspaceSettings::default();
        settings.default_workspace_count = 5;
        settings.default_workspace = 3;

        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());

        let space = SpaceId::new(42);
        let workspaces = manager.list_workspaces(space);
        let expected_ws = workspaces.get(settings.default_workspace).unwrap().0;

        assert_eq!(manager.active_workspace(space), Some(expected_ws));
    }

    #[test]
    fn test_workspace_navigation() {
        let window_store = WindowStore::default();
        let mut manager = WorkspaceStore::new();
        let space = SpaceId::new(1);
        let ws1_id = manager.create_workspace(space, Some("WS1".to_string())).unwrap();
        let ws2_id = manager.create_workspace(space, Some("WS2".to_string())).unwrap();
        let ws3_id = manager.create_workspace(space, Some("WS3".to_string())).unwrap();

        assert_eq!(
            manager.next_workspace(&window_store, space, ws1_id, None),
            Some(ws2_id)
        );
        assert_eq!(
            manager.next_workspace(&window_store, space, ws2_id, None),
            Some(ws3_id)
        );

        assert_eq!(
            manager.prev_workspace(&window_store, space, ws2_id, None),
            Some(ws1_id)
        );
        assert_eq!(
            manager.prev_workspace(&window_store, space, ws3_id, None),
            Some(ws2_id)
        );
    }

    #[test]
    fn workspace_navigation_uses_stable_indexes_not_persisted_vector_order() {
        let window_store = WindowStore::default();
        let settings = VirtualWorkspaceSettings {
            default_workspace_count: 3,
            workspace_names: vec!["A".into(), "B".into(), "C".into()],
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(2);
        let indexed = manager.list_workspaces(space);
        manager.workspaces_by_space.get_mut(&space).unwrap().reverse();

        assert_eq!(
            manager
                .list_workspaces(space)
                .iter()
                .map(|(_, name)| name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B", "C"],
        );
        assert_eq!(
            manager.next_workspace(&window_store, space, indexed[0].0, None),
            Some(indexed[1].0),
        );
        assert_eq!(
            manager.prev_workspace(&window_store, space, indexed[2].0, None),
            Some(indexed[1].0),
        );
    }

    #[test]
    fn workspace_navigation_wraps_by_default() {
        let window_store = WindowStore::default();
        let settings = VirtualWorkspaceSettings {
            default_workspace_count: 3,
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(1);
        let workspaces = manager.list_workspaces(space).to_vec();

        assert_eq!(
            manager.next_workspace(&window_store, space, workspaces[2].0, None),
            Some(workspaces[0].0)
        );
        assert_eq!(
            manager.prev_workspace(&window_store, space, workspaces[0].0, None),
            Some(workspaces[2].0)
        );
    }

    #[test]
    fn prevent_wrapping_stops_workspace_navigation_at_boundaries() {
        let mut window_store = WindowStore::default();
        let settings = VirtualWorkspaceSettings {
            default_workspace_count: 4,
            prevent_wrapping: true,
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(1);
        let workspaces = manager.list_workspaces(space).to_vec();

        assert_eq!(
            manager.prev_workspace(&window_store, space, workspaces[0].0, None),
            None
        );
        assert_eq!(
            manager.next_workspace(&window_store, space, workspaces[3].0, None),
            None
        );

        let window = WindowId::new(1, 1);
        assert!(manager.assign_window_to_workspace(
            &mut window_store,
            space,
            window,
            workspaces[3].0
        ));
        assert_eq!(
            manager.next_workspace(&window_store, space, workspaces[1].0, Some(true)),
            Some(workspaces[3].0)
        );
        assert_eq!(
            manager.next_workspace(&window_store, space, workspaces[3].0, Some(true)),
            None
        );
    }

    #[test]
    fn prevent_wrapping_updates_on_config_reload() {
        let window_store = WindowStore::default();
        let mut settings = VirtualWorkspaceSettings {
            default_workspace_count: 2,
            ..VirtualWorkspaceSettings::default()
        };
        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());
        let space = SpaceId::new(1);
        let workspaces = manager.list_workspaces(space).to_vec();

        assert_eq!(
            manager.next_workspace(&window_store, space, workspaces[1].0, None),
            Some(workspaces[0].0)
        );

        settings.prevent_wrapping = true;
        manager.update_settings(&settings, &LayoutSettings::default());
        assert_eq!(
            manager.next_workspace(&window_store, space, workspaces[1].0, None),
            None
        );
    }

    #[test]
    fn app_rules() {
        let mut window_store = WindowStore::default();
        let space1 = SpaceId::new(1);
        let space2 = SpaceId::new(2);

        let mut settings = VirtualWorkspaceSettings::default();

        if settings.workspace_names.len() < 4 {
            while settings.workspace_names.len() < 4 {
                settings
                    .workspace_names
                    .push(format!("Workspace {}", settings.workspace_names.len() + 1));
            }
        }
        settings.workspace_names[1] = "coding".to_string();

        settings.app_rules = vec![
            // Floating by app_id
            AppWorkspaceRule {
                app_id: Some("com.example.test".into()),
                workspace: None,
                floating: true,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: None,
                ax_role: None,
                ax_subrole: None,
            },
            // Match by app_name -> workspace 1
            AppWorkspaceRule {
                app_id: None,
                workspace: Some(WorkspaceSelector::Index(1)),
                floating: false,
                manage: true,
                app_name: Some("Calendar".into()),
                title_regex: None,
                title_substring: None,
                ax_role: None,
                ax_subrole: None,
            },
            // Title substring -> workspace 0
            AppWorkspaceRule {
                app_id: Some("com.example.foo".into()),
                workspace: Some(WorkspaceSelector::Index(0)),
                floating: false,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: Some("Preferences".into()),
                ax_role: None,
                ax_subrole: None,
            },
            // Title regex -> workspace 2
            AppWorkspaceRule {
                app_id: Some("com.example.foo".into()),
                workspace: Some(WorkspaceSelector::Index(2)),
                floating: false,
                manage: true,
                app_name: None,
                title_regex: Some(r"Dialog\s+\d+".into()),
                title_substring: None,
                ax_role: None,
                ax_subrole: None,
            },
            // AX role + subrole floating
            AppWorkspaceRule {
                app_id: Some("com.example.special".into()),
                workspace: None,
                floating: true,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: None,
                ax_role: Some("AXWindow".into()),
                ax_subrole: Some("AXDialog".into()),
            },
            // Workspace by name
            AppWorkspaceRule {
                app_id: Some("com.example.name".into()),
                workspace: Some(WorkspaceSelector::Name("coding".into())),
                floating: false,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: None,
                ax_role: None,
                ax_subrole: None,
            },
            // Specificity tie breaking generic vs substring (generic workspace 0, specific workspace 2)
            AppWorkspaceRule {
                app_id: Some("com.example.tie".into()),
                workspace: Some(WorkspaceSelector::Index(0)),
                floating: false,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: None,
                ax_role: None,
                ax_subrole: None,
            },
            AppWorkspaceRule {
                app_id: Some("com.example.tie".into()),
                workspace: Some(WorkspaceSelector::Index(2)),
                floating: false,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: Some("Editor".into()),
                ax_role: None,
                ax_subrole: None,
            },
            // Reapplication: Bitwarden title becomes floating
            AppWorkspaceRule {
                app_id: Some("app.zen-browser.zen".into()),
                workspace: None,
                floating: true,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: Some("Bitwarden".into()),
                ax_role: None,
                ax_subrole: None,
            },
            AppWorkspaceRule {
                app_id: Some("app.zen-browser.zen".into()),
                workspace: Some(WorkspaceSelector::Index(2)),
                floating: false,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: None,
                ax_role: None,
                ax_subrole: None,
            },
            // Workspace override when specific rule matches different workspace + floating
            AppWorkspaceRule {
                app_id: Some("app.zen-browser.zen".into()),
                workspace: Some(WorkspaceSelector::Index(1)),
                floating: false,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: None,
                ax_role: None,
                ax_subrole: None,
            },
            AppWorkspaceRule {
                app_id: Some("app.zen-browser.zen".into()),
                workspace: Some(WorkspaceSelector::Index(3)),
                floating: true,
                manage: true,
                app_name: None,
                title_regex: None,
                title_substring: Some("bitwarden".into()),
                ax_role: None,
                ax_subrole: None,
            },
        ];

        let mut manager = WorkspaceStore::new_with_config(&settings, &LayoutSettings::default());

        // 1. Floating persistence via app_id (case-insensitive)
        let w_float = WindowId::new(10, 1);
        let assignment = assign(
            &mut manager,
            &mut window_store,
            w_float,
            space1,
            Some("COM.EXAMPLE.Test"),
            None,
            None,
            None,
            None,
        );
        assert!(assignment.floating);

        manager.remove_window(&mut window_store, w_float);

        // After removal, reassign should still float.
        let assignment_again = assign(
            &mut manager,
            &mut window_store,
            w_float,
            space1,
            Some("com.example.test"),
            None,
            None,
            None,
            None,
        );
        assert!(assignment_again.floating);

        // 2. Match by app_name
        let w_name = WindowId::new(20, 2);
        let ws_name = assign(
            &mut manager,
            &mut window_store,
            w_name,
            space1,
            None,
            Some("MyCalendarApp"),
            None,
            None,
            None,
        )
        .workspace_id;
        let coding_idx = 1; // Calendar rule points to workspace index 1
        let expected_ws_name = manager.list_workspaces(space1).get(coding_idx).unwrap().0;
        assert_eq!(ws_name, expected_ws_name);

        // 3. Title substring and regex for same app
        let w_pref = WindowId::new(30, 3);
        let w_dialog = WindowId::new(30, 4);
        let ws_pref = assign(
            &mut manager,
            &mut window_store,
            w_pref,
            space1,
            Some("com.example.foo"),
            None,
            Some("App Preferences"),
            None,
            None,
        )
        .workspace_id;
        let ws_dialog = assign(
            &mut manager,
            &mut window_store,
            w_dialog,
            space1,
            Some("com.example.foo"),
            None,
            Some("Dialog 42"),
            None,
            None,
        )
        .workspace_id;
        let expected_pref = manager.list_workspaces(space1).get(0).unwrap().0;
        let expected_dialog = manager.list_workspaces(space1).get(2).unwrap().0;
        assert_eq!(ws_pref, expected_pref);
        assert_eq!(ws_dialog, expected_dialog);

        // 4. AX role + subrole floating
        let w_ax = WindowId::new(40, 5);
        let ax_assignment = assign(
            &mut manager,
            &mut window_store,
            w_ax,
            space1,
            Some("com.example.special"),
            None,
            None,
            Some("AXWindow"),
            Some("AXDialog"),
        );
        assert!(ax_assignment.floating);

        // 5. Workspace name resolution
        let w_named = WindowId::new(50, 6);
        let ws_named = assign(
            &mut manager,
            &mut window_store,
            w_named,
            space1,
            Some("com.example.name"),
            None,
            None,
            None,
            None,
        )
        .workspace_id;
        let coding_ws =
            manager.list_workspaces(space1).iter().find(|(_, n)| n == "coding").unwrap().0;
        assert_eq!(ws_named, coding_ws);

        // 6. Specificity tie-breaking (generic vs substring)
        let w_tie = WindowId::new(60, 7);
        let ws_tie = assign(
            &mut manager,
            &mut window_store,
            w_tie,
            space1,
            Some("com.example.tie"),
            None,
            Some("Editor - Untitled"),
            None,
            None,
        )
        .workspace_id;
        let expected_specific = manager.list_workspaces(space1).get(2).unwrap().0; // substring rule points to 2
        assert_eq!(ws_tie, expected_specific);

        // 7. Reapplication updates existing window to floating (Bitwarden title)
        let w_bw = WindowId::new(70, 8);
        let bw_initial_assignment = assign(
            &mut manager,
            &mut window_store,
            w_bw,
            space1,
            Some("app.zen-browser.zen"),
            None,
            None,
            None,
            None,
        );
        assert!(!bw_initial_assignment.floating);
        let bw_updated_assignment = assign(
            &mut manager,
            &mut window_store,
            w_bw,
            space1,
            Some("app.zen-browser.zen"),
            None,
            Some("Bitwarden Login"),
            None,
            None,
        );
        assert_eq!(
            bw_initial_assignment.workspace_id,
            bw_updated_assignment.workspace_id
        );
        assert!(bw_updated_assignment.floating);

        // 8. Workspace override + floating with specific substring on different space
        let w_bw2 = WindowId::new(80, 9);
        let bw2_initial_assignment = assign(
            &mut manager,
            &mut window_store,
            w_bw2,
            space2,
            Some("app.zen-browser.zen"),
            None,
            None,
            None,
            None,
        );
        assert!(!bw2_initial_assignment.floating);
        let bw2_updated_assignment = assign(
            &mut manager,
            &mut window_store,
            w_bw2,
            space2,
            Some("app.zen-browser.zen"),
            None,
            Some("Bitwarden Vault"),
            None,
            None,
        );
        // The generic rule with workspace index 1 should apply first.
        // When title matches, the specific rule (index 3, floating) should override.
        let expected_initial = manager.list_workspaces(space2).get(2).unwrap().0; // workspace index 1
        let expected_updated = manager.list_workspaces(space2).get(3).unwrap().0; // workspace index 3
        assert_eq!(bw2_initial_assignment.workspace_id, expected_initial);
        // Workspace may remain same depending on rule ordering; ensure floating toggled and workspace is one of the target candidates.
        assert!(
            bw2_updated_assignment.workspace_id == expected_initial
                || bw2_updated_assignment.workspace_id == expected_updated
        );
        assert!(bw2_updated_assignment.floating);
    }

    #[test]
    fn hidden_position_uses_corner_anchor_while_hiding_offscreen() {
        let manager = WorkspaceStore::new();
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(100.0, 100.0));
        let frame = CGRect::new(CGPoint::new(20.0, 37.0), CGSize::new(30.0, 20.0));

        let hidden = manager.calculate_hidden_position_multi(
            screen,
            frame,
            HideCorner::BottomRight,
            None,
            &[screen],
        );

        assert_eq!(hidden.origin.y, screen.max().y - 1.0);
        assert_eq!(hidden.origin.x, screen.max().x - 1.0);
    }

    #[test]
    fn hidden_position_flips_sides_to_avoid_neighboring_monitor_overlap() {
        let manager = WorkspaceStore::new();
        let primary = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(100.0, 100.0));
        let right_neighbor = CGRect::new(CGPoint::new(100.0, 0.0), CGSize::new(100.0, 100.0));
        let frame = CGRect::new(CGPoint::new(20.0, 25.0), CGSize::new(30.0, 20.0));

        let hidden = manager.calculate_hidden_position_multi(
            primary,
            frame,
            HideCorner::BottomRight,
            None,
            &[primary, right_neighbor],
        );

        assert_eq!(hidden.origin.y, primary.max().y - 1.0);
        assert_eq!(hidden.origin.x, primary.origin.x - frame.size.width + 1.0);
    }
}
