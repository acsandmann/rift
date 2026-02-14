use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{
    Direction, FloatingManager, LayoutId, LayoutSystemKind, WindowConstraint, WorkspaceLayouts,
};
use crate::actor::app::{AppInfo, WindowId, pid_t};
use crate::actor::broadcast::{BroadcastEvent, BroadcastSender};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{LayoutMode, LayoutSettings};
use crate::layout_engine::LayoutSystem;
use crate::model::virtual_workspace::{
    AppRuleAssignment, AppRuleResult, VirtualWorkspace, VirtualWorkspaceId, VirtualWorkspaceManager,
};
use crate::sys::screen::SpaceId;

#[derive(Debug, Clone)]
pub struct GroupContainerInfo {
    pub node_id: crate::model::tree::NodeId,
    pub container_kind: super::LayoutKind,
    pub frame: CGRect,
    pub total_count: usize,
    pub selected_index: usize,
    pub window_ids: Vec<crate::actor::app::WindowId>,
}

#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutCommand {
    NextWindow,
    PrevWindow,
    MoveFocus(#[serde(rename = "direction")] Direction),
    Ascend,
    Descend,
    MoveNode(Direction),

    JoinWindow(Direction),
    ToggleStack,
    ToggleOrientation,
    UnjoinWindows,
    ToggleFocusFloating,
    ToggleWindowFloating,
    ToggleFullscreen,
    ToggleFullscreenWithinGaps,

    ResizeWindowGrow,
    ResizeWindowShrink,
    ResizeWindowBy {
        amount: f64,
    },

    /// Scroll the strip by a normalized delta (scaled by column step width)
    ScrollStrip {
        delta: f64,
    },
    /// Snap the strip to the nearest column boundary
    SnapStrip,
    /// Toggle centering for the selected column without changing alignment settings.
    /// The center override is cleared when focus moves to a different window.
    CenterSelection,

    NextWorkspace(Option<bool>),
    PrevWorkspace(Option<bool>),
    SwitchToWorkspace(usize),
    MoveWindowToWorkspace {
        workspace: usize,
        window_id: Option<u32>,
    },
    SetWorkspaceLayout {
        workspace: Option<usize>,
        mode: LayoutMode,
    },
    CreateWorkspace,
    SwitchToLastWorkspace,

    SwapWindows(crate::actor::app::WindowId, crate::actor::app::WindowId),

    AdjustMasterRatio {
        delta: f64,
    },
    AdjustMasterCount {
        delta: i32,
    },
    PromoteToMaster,
    SwapMasterStack,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum LayoutEvent {
    WindowsOnScreenUpdated(
        SpaceId,
        pid_t,
        Vec<(WindowId, Option<String>, Option<String>, Option<String>)>,
        Option<AppInfo>,
    ),
    AppClosed(pid_t),
    WindowAdded(SpaceId, WindowId),
    WindowRemoved(WindowId),
    WindowRemovedPreserveFloating(WindowId),
    WindowFocused(SpaceId, WindowId),
    WindowResized {
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screens: Vec<(SpaceId, CGRect, Option<String>)>,
    },
    SpaceExposed(SpaceId, CGSize),
}

#[must_use]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventResponse {
    pub raise_windows: Vec<WindowId>,
    pub focus_window: Option<WindowId>,
    pub boundary_hit: Option<Direction>,
}

#[derive(Serialize, Deserialize)]
pub struct LayoutEngine {
    workspace_layouts: WorkspaceLayouts,
    floating: FloatingManager,
    #[serde(skip)]
    focused_window: Option<WindowId>,
    virtual_workspace_manager: VirtualWorkspaceManager,
    #[serde(skip)]
    layout_settings: LayoutSettings,
    #[serde(skip)]
    broadcast_tx: Option<BroadcastSender>,
    #[serde(skip)]
    space_display_map: HashMap<SpaceId, Option<String>>,
    #[serde(skip)]
    display_last_space: HashMap<String, SpaceId>,
}

impl LayoutEngine {
    /// Get the active workspace ID for a space, ensuring initialization.
    fn active_workspace_id(&self, space: SpaceId) -> Option<VirtualWorkspaceId> {
        self.virtual_workspace_manager.active_workspace(space)
    }

    /// Get mutable access to a workspace's layout system.
    fn workspace_tree_mut(&mut self, ws_id: VirtualWorkspaceId) -> &mut LayoutSystemKind {
        &mut self.virtual_workspace_manager.workspaces[ws_id].layout_system
    }

    /// Get immutable access to a workspace's layout system.
    fn workspace_tree(&self, ws_id: VirtualWorkspaceId) -> &LayoutSystemKind {
        &self.virtual_workspace_manager.workspaces[ws_id].layout_system
    }

    /// Get the active workspace and layout for a space.
    fn workspace_and_layout(&self, space: SpaceId) -> (VirtualWorkspaceId, LayoutId) {
        let ws_id = self.active_workspace_id(space).expect("No active workspace for space");
        let layout = self
            .workspace_layouts
            .active(space, ws_id)
            .expect("No active layout for workspace");
        (ws_id, layout)
    }

    fn workspace_id_for_index(
        &mut self,
        space: SpaceId,
        workspace: Option<usize>,
    ) -> Option<VirtualWorkspaceId> {
        if let Some(index) = workspace {
            let workspaces = self.virtual_workspace_manager.list_workspaces(space);
            workspaces.get(index).map(|(workspace_id, _)| *workspace_id)
        } else {
            self.virtual_workspace_manager.active_workspace(space)
        }
    }

    fn switch_workspace_layout_mode(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        mode: LayoutMode,
    ) -> bool {
        let old_layout = self.workspace_layouts.active(space, workspace_id);
        let (current_mode, selected_window, mut window_order) = {
            let Some(workspace) =
                self.virtual_workspace_manager.workspace_info(space, workspace_id)
            else {
                return false;
            };
            let selected =
                old_layout.and_then(|layout| workspace.layout_system.selected_window(layout));
            let mut ordered = old_layout
                .map(|layout| workspace.layout_system.visible_windows_in_layout(layout))
                .unwrap_or_default();
            // Keep windows hidden by stack/group selection when rebuilding into a new mode.
            let mut hidden_windows: Vec<_> =
                workspace.windows().filter(|wid| !ordered.contains(wid)).collect();
            hidden_windows.sort();
            ordered.extend(hidden_windows);
            (workspace.layout_mode, selected, ordered)
        };

        if current_mode == mode {
            return false;
        }

        window_order.retain(|wid| !self.floating.is_floating(*wid));

        let Some(workspace) = self.virtual_workspace_manager.workspaces.get_mut(workspace_id)
        else {
            return false;
        };
        workspace.layout_mode = mode;
        workspace.layout_system =
            VirtualWorkspace::create_layout_system(mode, &self.layout_settings);

        let new_layout = workspace.layout_system.create_layout();
        self.workspace_layouts
            .replace_layouts_for_workspace(space, workspace_id, new_layout);

        for wid in window_order {
            workspace.layout_system.add_window_after_selection(new_layout, wid);
        }

        if let Some(selected) = selected_window.filter(|wid| !self.floating.is_floating(*wid)) {
            let _ = workspace.layout_system.select_window(new_layout, selected);
        }

        true
    }
}

impl LayoutEngine {
    pub fn set_layout_settings(&mut self, settings: &LayoutSettings) {
        self.layout_settings = settings.clone();

        for (_, ws) in self.virtual_workspace_manager.workspaces.iter_mut() {
            match &mut ws.layout_system {
                LayoutSystemKind::MasterStack(system) => {
                    system.update_settings(settings.master_stack.clone());
                }
                LayoutSystemKind::Scrolling(system) => {
                    system.update_settings(&settings.scrolling);
                }
                _ => {}
            }
        }
    }

    pub fn set_window_constraint(&mut self, wid: WindowId, constraint: WindowConstraint) {
        let ws_ids = self.virtual_workspace_manager.workspaces_for_window(wid);
        let ws_ids = if ws_ids.is_empty() {
            self.virtual_workspace_manager.workspaces.keys().collect()
        } else {
            ws_ids
        };

        for ws_id in ws_ids {
            match self.workspace_tree_mut(ws_id) {
                LayoutSystemKind::Traditional(system) => {
                    system.set_window_constraint(wid, constraint);
                }
                LayoutSystemKind::Bsp(system) => {
                    system.set_window_constraint(wid, constraint);
                }
                LayoutSystemKind::MasterStack(system) => {
                    system.set_window_constraint(wid, constraint);
                }
                LayoutSystemKind::Scrolling(_) => {}
            }
        }
    }

    pub fn clear_window_constraint(&mut self, wid: WindowId) {
        let ws_ids = self.virtual_workspace_manager.workspaces_for_window(wid);
        let ws_ids = if ws_ids.is_empty() {
            self.virtual_workspace_manager.workspaces.keys().collect()
        } else {
            ws_ids
        };

        for ws_id in ws_ids {
            match self.workspace_tree_mut(ws_id) {
                LayoutSystemKind::Traditional(system) => system.clear_window_constraint(wid),
                LayoutSystemKind::Bsp(system) => system.clear_window_constraint(wid),
                LayoutSystemKind::MasterStack(system) => system.clear_window_constraint(wid),
                LayoutSystemKind::Scrolling(_) => {}
            }
        }
    }

    pub fn window_constraint(&self, wid: WindowId) -> Option<WindowConstraint> {
        let ws_ids = self.virtual_workspace_manager.workspaces_for_window(wid);
        let ws_ids = if ws_ids.is_empty() {
            self.virtual_workspace_manager.workspaces.keys().collect()
        } else {
            ws_ids
        };

        for ws_id in ws_ids {
            let constraint = match self.workspace_tree(ws_id) {
                LayoutSystemKind::Traditional(system) => system.window_constraint(wid),
                LayoutSystemKind::Bsp(system) => system.window_constraint(wid),
                LayoutSystemKind::MasterStack(system) => system.window_constraint(wid),
                LayoutSystemKind::Scrolling(_) => None,
            };
            if constraint.is_some() {
                return constraint;
            }
        }
        None
    }

    pub fn update_virtual_workspace_settings(
        &mut self,
        settings: &crate::common::config::VirtualWorkspaceSettings,
    ) {
        self.virtual_workspace_manager.update_settings(settings, &self.layout_settings);

        // Re-apply workspace layout rules to already-existing workspaces on hot reload.
        let spaces = self.virtual_workspace_manager.initialized_spaces();
        for space in spaces {
            let workspaces = self.virtual_workspace_manager.list_workspaces(space).to_vec();
            for (index, (workspace_id, name)) in workspaces.iter().enumerate() {
                let desired_mode =
                    self.virtual_workspace_manager.desired_layout_mode_for_workspace(index, name);
                let current_mode = self
                    .virtual_workspace_manager
                    .workspace_info(space, *workspace_id)
                    .map(|ws| ws.layout_mode())
                    .unwrap_or_default();
                if current_mode != desired_mode {
                    let _ = self.switch_workspace_layout_mode(space, *workspace_id, desired_mode);
                }
            }
        }
    }

    pub fn layout_mode_at(&self, space: SpaceId) -> &'static str {
        if let Some(ws_id) = self.virtual_workspace_manager.active_workspace(space) {
            match self.workspace_tree(ws_id) {
                LayoutSystemKind::Traditional(_) => "traditional",
                LayoutSystemKind::Bsp(_) => "bsp",
                LayoutSystemKind::MasterStack(_) => "master_stack",
                LayoutSystemKind::Scrolling(_) => "scrolling",
            }
        } else {
            "none"
        }
    }

    pub fn active_layout_mode_at(&self, space: SpaceId) -> crate::common::config::LayoutMode {
        if let Some(ws_id) = self.virtual_workspace_manager.active_workspace(space) {
            match self.workspace_tree(ws_id) {
                LayoutSystemKind::Traditional(_) => crate::common::config::LayoutMode::Traditional,
                LayoutSystemKind::Bsp(_) => crate::common::config::LayoutMode::Bsp,
                LayoutSystemKind::MasterStack(_) => crate::common::config::LayoutMode::MasterStack,
                LayoutSystemKind::Scrolling(_) => crate::common::config::LayoutMode::Scrolling,
            }
        } else {
            crate::common::config::LayoutMode::default()
        }
    }

    pub fn layout_specific_animate_settings(&self, space: SpaceId) -> Option<bool> {
        if let Some(ws_id) = self.virtual_workspace_manager.active_workspace(space) {
            match self.workspace_tree(ws_id) {
                LayoutSystemKind::Scrolling(_) => self.layout_settings.scrolling.animate,
                _ => None,
            }
        } else {
            None
        }
    }

    fn active_floating_windows_in_workspace(&self, space: SpaceId) -> Vec<WindowId> {
        self.floating
            .active_flat(space)
            .into_iter()
            .filter(|wid| self.is_window_in_active_workspace(space, *wid))
            .collect()
    }

    fn refocus_workspace(
        &mut self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> EventResponse {
        let mut focus_window = self
            .virtual_workspace_manager
            .last_focused_window(space, workspace_id)
            .filter(|wid| {
                self.virtual_workspace_manager.workspace_for_window(space, *wid)
                    == Some(workspace_id)
            });

        if focus_window.is_none() {
            if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
                let selected =
                    self.workspace_tree(workspace_id).selected_window(layout).filter(|wid| {
                        self.virtual_workspace_manager.workspace_for_window(space, *wid)
                            == Some(workspace_id)
                    });
                let visible = self
                    .workspace_tree(workspace_id)
                    .visible_windows_in_layout(layout)
                    .into_iter()
                    .find(|wid| {
                        self.virtual_workspace_manager.workspace_for_window(space, *wid)
                            == Some(workspace_id)
                    });
                focus_window = selected.or(visible);
            }
        }

        if focus_window.is_none() {
            let floating_windows = self.active_floating_windows_in_workspace(space);
            let floating_focus =
                self.floating.last_focus().filter(|wid| floating_windows.contains(wid));
            focus_window = floating_focus.or_else(|| floating_windows.first().copied());
        }

        if let Some(wid) = focus_window {
            self.focused_window = Some(wid);
            self.virtual_workspace_manager
                .set_last_focused_window(space, workspace_id, Some(wid));
            if self.floating.is_floating(wid) {
                self.floating.set_last_focus(Some(wid));
            } else if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
                let _ = self.workspace_tree_mut(workspace_id).select_window(layout, wid);
            }
        } else {
            self.focused_window = None;
            self.virtual_workspace_manager
                .set_last_focused_window(space, workspace_id, None);
        }

        EventResponse {
            focus_window,
            raise_windows: vec![],
            boundary_hit: None,
        }
    }

    fn filter_active_workspace_windows(
        &self,
        space: SpaceId,
        windows: Vec<WindowId>,
    ) -> Vec<WindowId> {
        windows
            .into_iter()
            .filter(|wid| self.is_window_in_active_workspace(space, *wid))
            .collect()
    }

    fn filter_active_workspace_window(
        &self,
        space: SpaceId,
        window: Option<WindowId>,
    ) -> Option<WindowId> {
        window.filter(|wid| self.is_window_in_active_workspace(space, *wid))
    }

    pub fn resize_selection(
        &mut self,
        ws_id: VirtualWorkspaceId,
        layout: LayoutId,
        resize_amount: f64,
    ) {
        self.workspace_tree_mut(ws_id).resize_selection_by(layout, resize_amount);
    }

    fn apply_focus_response(
        &mut self,
        space: SpaceId,
        ws_id: VirtualWorkspaceId,
        layout: LayoutId,
        response: &EventResponse,
    ) {
        if let Some(wid) = response.focus_window {
            self.focused_window = Some(wid);
            if self.floating.is_floating(wid) {
                self.floating.set_last_focus(Some(wid));
            } else {
                let _ = self.workspace_tree_mut(ws_id).select_window(layout, wid);
                self.virtual_workspace_manager.set_last_focused_window(space, ws_id, Some(wid));
            }
        }
    }

    fn move_focus_internal(
        &mut self,
        space: SpaceId,
        visible_spaces: &[SpaceId],
        visible_space_centers: &HashMap<SpaceId, CGPoint>,
        direction: Direction,
        is_floating: bool,
    ) -> EventResponse {
        let (ws_id, layout) = self.workspace_and_layout(space);

        if is_floating {
            let floating_windows = self.active_floating_windows_in_workspace(space);
            debug!(
                "Floating navigation: found {} floating windows: {:?}",
                floating_windows.len(),
                floating_windows
            );

            match direction {
                Direction::Left | Direction::Right => {
                    if floating_windows.len() > 1 {
                        debug!(
                            "Multiple floating windows found, looking for current window: {:?}",
                            self.focused_window
                        );

                        if let Some(current_idx) =
                            floating_windows.iter().position(|&w| Some(w) == self.focused_window)
                        {
                            debug!("Found current window at index {}", current_idx);
                            let next_idx = match direction {
                                Direction::Left => {
                                    if current_idx == 0 {
                                        floating_windows.len() - 1
                                    } else {
                                        current_idx - 1
                                    }
                                }
                                Direction::Right => (current_idx + 1) % floating_windows.len(),
                                _ => unreachable!(),
                            };
                            debug!(
                                "Moving to index {}, window: {:?}",
                                next_idx, floating_windows[next_idx]
                            );
                            let focus_window = Some(floating_windows[next_idx]);
                            let response = EventResponse {
                                focus_window,
                                raise_windows: vec![],
                                boundary_hit: None,
                            };
                            self.apply_focus_response(space, ws_id, layout, &response);
                            return response;
                        } else {
                            debug!("Could not find current window in floating windows list");
                        }
                    } else {
                        debug!(
                            "Not enough floating windows for horizontal navigation (len: {})",
                            floating_windows.len()
                        );
                    }
                }
                Direction::Up | Direction::Down => {
                    debug!("Vertical navigation - switching to tiled windows");
                }
            }

            let tiled_windows = self.filter_active_workspace_windows(
                space,
                self.workspace_tree(ws_id).visible_windows_in_layout(layout),
            );
            debug!("Trying tiled windows: {:?}", tiled_windows);
            if !tiled_windows.is_empty() {
                let response = EventResponse {
                    focus_window: tiled_windows.first().copied(),
                    raise_windows: tiled_windows,
                    boundary_hit: None,
                };
                self.apply_focus_response(space, ws_id, layout, &response);
                return response;
            }

            debug!("No windows to navigate to, returning default");
            return EventResponse::default();
        }

        let previous_selection = self.workspace_tree(ws_id).selected_window(layout);

        let (focus_window_raw, raise_windows) =
            self.workspace_tree_mut(ws_id).move_focus(layout, direction);
        let focus_window = self.filter_active_workspace_window(space, focus_window_raw);
        let raise_windows = self.filter_active_workspace_windows(space, raise_windows);
        if focus_window.is_some() {
            let response = EventResponse {
                focus_window,
                raise_windows,
                boundary_hit: None,
            };
            self.apply_focus_response(space, ws_id, layout, &response);
            response
        } else {
            if let Some(prev_wid) = previous_selection {
                let _ = self.workspace_tree_mut(ws_id).select_window(layout, prev_wid);
            }
            if let Some(new_space) = self.next_space_for_direction(
                space,
                direction,
                visible_spaces,
                visible_space_centers,
            ) {
                let (new_ws_id, new_layout) = self.workspace_and_layout(new_space);
                let windows_in_new_space = self.filter_active_workspace_windows(
                    new_space,
                    self.workspace_tree(new_ws_id).visible_windows_in_layout(new_layout),
                );
                if let Some(target_window) = self
                    .filter_active_workspace_window(
                        new_space,
                        self.workspace_tree(new_ws_id).window_in_direction(new_layout, direction),
                    )
                    .or_else(|| windows_in_new_space.first().copied())
                {
                    let _ =
                        self.workspace_tree_mut(new_ws_id).select_window(new_layout, target_window);
                    let response = EventResponse {
                        focus_window: Some(target_window),
                        raise_windows: windows_in_new_space,
                        boundary_hit: None,
                    };
                    self.apply_focus_response(new_space, new_ws_id, new_layout, &response);
                    return response;
                }
            }

            let floating_windows = self.active_floating_windows_in_workspace(space);

            if let Some(&first_floating) = floating_windows.first() {
                let focus_window = Some(first_floating);
                let response = EventResponse {
                    focus_window,
                    raise_windows: vec![],
                    boundary_hit: None,
                };
                self.apply_focus_response(space, ws_id, layout, &response);
                return response;
            }

            let visible_windows = self.filter_active_workspace_windows(
                space,
                self.workspace_tree(ws_id).visible_windows_in_layout(layout),
            );

            if let Some(fallback_focus) = self
                .filter_active_workspace_window(space, previous_selection)
                .or_else(|| visible_windows.first().copied())
            {
                let response = EventResponse {
                    focus_window: Some(fallback_focus),
                    raise_windows: visible_windows,
                    boundary_hit: None,
                };
                self.apply_focus_response(space, ws_id, layout, &response);
                return response;
            }

            EventResponse::default()
        }
    }

    fn next_space_for_direction(
        &self,
        current_space: SpaceId,
        direction: Direction,
        visible_spaces: &[SpaceId],
        space_centers: &HashMap<SpaceId, CGPoint>,
    ) -> Option<SpaceId> {
        if visible_spaces.len() <= 1 {
            return None;
        }

        let current_center = space_centers.get(&current_space)?;
        let mut candidates = Vec::new();
        for &candidate_space in visible_spaces {
            if candidate_space == current_space {
                continue;
            }
            if let Some(candidate_center) = space_centers.get(&candidate_space) {
                if let Some(delta) =
                    Self::directional_delta(direction, current_center, candidate_center)
                {
                    candidates.push((candidate_space, delta));
                }
            }
        }

        if !candidates.is_empty() {
            candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
            return Some(candidates[0].0);
        }

        match direction {
            Direction::Left => {
                visible_spaces.iter().rev().copied().find(|&space| space != current_space)
            }
            Direction::Right => {
                visible_spaces.iter().copied().find(|&space| space != current_space)
            }
            Direction::Up | Direction::Down => None,
        }
    }

    fn directional_delta(
        direction: Direction,
        current: &CGPoint,
        candidate: &CGPoint,
    ) -> Option<f64> {
        match direction {
            Direction::Left => {
                let delta = current.x - candidate.x;
                if delta > 0.0 { Some(delta) } else { None }
            }
            Direction::Right => {
                let delta = candidate.x - current.x;
                if delta > 0.0 { Some(delta) } else { None }
            }
            Direction::Up => {
                let delta = candidate.y - current.y;
                if delta > 0.0 { Some(delta) } else { None }
            }
            Direction::Down => {
                let delta = current.y - candidate.y;
                if delta > 0.0 { Some(delta) } else { None }
            }
        }
    }

    fn remove_window_internal(&mut self, wid: WindowId, preserve_floating: bool) {
        let affected_space: Option<SpaceId> = self.space_with_window(wid);

        let ws_ids = self.virtual_workspace_manager.workspaces_for_window(wid);
        if !ws_ids.is_empty() {
            for ws_id in ws_ids {
                self.workspace_tree_mut(ws_id).remove_window(wid);
            }
        } else {
            // Fallback: search all workspaces
            let ws_ids: Vec<_> = self.virtual_workspace_manager.workspaces.keys().collect();
            for ws_id in ws_ids {
                self.workspace_tree_mut(ws_id).remove_window(wid);
            }
        }
        self.clear_window_constraint(wid);

        if preserve_floating {
            self.floating.remove_active_for_window(wid);
        } else {
            self.floating.remove_floating(wid);
        }

        self.virtual_workspace_manager.remove_window(wid);
        if !preserve_floating {
            self.virtual_workspace_manager.remove_floating_position(wid);
        }

        if self.focused_window == Some(wid) {
            self.focused_window = None;
        }

        if let Some(space) = affected_space {
            self.broadcast_windows_changed(space);
        }

        self.rebalance_all_layouts();
    }

    fn space_with_window(&self, wid: WindowId) -> Option<SpaceId> {
        for space in self.workspace_layouts.spaces() {
            if let Some(ws_id) = self.virtual_workspace_manager.active_workspace(space) {
                if let Some(layout) = self.workspace_layouts.active(space, ws_id) {
                    if self.workspace_tree(ws_id).contains_window(layout, wid) {
                        return Some(space);
                    }
                }
            }

            if self.floating.active_flat(space).contains(&wid) {
                return Some(space);
            }
        }
        None
    }

    fn active_workspace_id_and_name(
        &self,
        space_id: SpaceId,
    ) -> Option<(crate::model::VirtualWorkspaceId, String)> {
        let workspace_id = self.virtual_workspace_manager.active_workspace(space_id)?;
        let workspace_name = self
            .virtual_workspace_manager
            .workspace_info(space_id, workspace_id)
            .map(|ws| ws.name.clone())
            .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));
        Some((workspace_id, workspace_name))
    }

    pub fn update_space_display(&mut self, space: SpaceId, display_uuid: Option<String>) {
        if let Some(uuid) = display_uuid {
            self.space_display_map.insert(space, Some(uuid.clone()));
            self.display_last_space.insert(uuid, space);
        } else {
            self.space_display_map.remove(&space);
        }
    }

    pub fn last_space_for_display_uuid(&self, display_uuid: &str) -> Option<SpaceId> {
        self.display_last_space.get(display_uuid).copied()
    }

    pub fn display_seen_before(&self, display_uuid: &str) -> bool {
        self.display_last_space.contains_key(display_uuid)
    }

    fn display_uuid_for_space(&self, space: SpaceId) -> Option<String> {
        self.space_display_map.get(&space).and_then(|uuid| uuid.clone())
    }

    /// Returns the last known space associated with the given display UUID.
    /// Useful when the OS recreates spaces (e.g. after sleep/resume) and we
    /// want to migrate layout state to the new space id.
    pub fn space_for_display_uuid(&self, display_uuid: &str) -> Option<SpaceId> {
        self.space_display_map.iter().find_map(|(space, uuid_opt)| match uuid_opt {
            Some(uuid) if uuid == display_uuid => Some(*space),
            _ => None,
        })
    }

    /// Move all per-space layout state from `old_space` to `new_space`.
    pub fn remap_space(&mut self, old_space: SpaceId, new_space: SpaceId) {
        if old_space == new_space {
            return;
        }

        self.workspace_layouts.remap_space(old_space, new_space);
        self.floating.remap_space(old_space, new_space);
        self.virtual_workspace_manager.remap_space(old_space, new_space);

        if let Some(uuid) = self.space_display_map.remove(&old_space) {
            self.space_display_map.insert(new_space, uuid);
        }

        for (_uuid, space) in self.display_last_space.iter_mut() {
            if *space == old_space {
                *space = new_space;
            }
        }
    }

    pub fn prune_display_state(&mut self, active_display_uuids: &[String]) {
        let active: HashSet<&str> = active_display_uuids.iter().map(|s| s.as_str()).collect();

        self.display_last_space.retain(|uuid, _| active.contains(uuid.as_str()));

        self.space_display_map.retain(|_, uuid_opt| {
            uuid_opt.as_ref().map(|uuid| active.contains(uuid.as_str())).unwrap_or(false)
        });
    }

    pub fn new(
        virtual_workspace_config: &crate::common::config::VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
        broadcast_tx: Option<BroadcastSender>,
    ) -> Self {
        let virtual_workspace_manager =
            VirtualWorkspaceManager::new_with_config(virtual_workspace_config, layout_settings);

        LayoutEngine {
            workspace_layouts: WorkspaceLayouts::default(),
            floating: FloatingManager::new(),
            focused_window: None,
            virtual_workspace_manager,
            layout_settings: layout_settings.clone(),
            broadcast_tx,
            space_display_map: HashMap::default(),
            display_last_space: HashMap::default(),
        }
    }

    pub fn debug_tree(&self, space: SpaceId) { self.debug_tree_desc(space, "", false); }

    pub fn debug_tree_desc(&self, space: SpaceId, desc: &'static str, print: bool) {
        if let Some(workspace_id) = self.virtual_workspace_manager.active_workspace(space) {
            if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
                if print {
                    println!(
                        "Tree {desc}\n{}",
                        self.workspace_tree(workspace_id).draw_tree(layout).trim()
                    );
                } else {
                    debug!(
                        "Tree {desc}\n{}",
                        self.workspace_tree(workspace_id).draw_tree(layout).trim()
                    );
                }
            } else {
                debug!("No layout for workspace {workspace_id:?} on space {space:?}");
            }
        } else {
            debug!("No active workspace for space {space:?}");
        }
    }

    pub fn handle_event(&mut self, event: LayoutEvent) -> EventResponse {
        debug!(?event);
        match event {
            LayoutEvent::SpaceExposed(space, size) => {
                self.debug_tree(space);

                let workspaces =
                    self.virtual_workspace_manager_mut().list_workspaces(space).to_vec();
                for (id, _) in workspaces {
                    let tree = &mut self.virtual_workspace_manager.workspaces[id].layout_system;
                    self.workspace_layouts.ensure_active_for_workspace(space, size, id, tree);
                }
            }
            LayoutEvent::WindowsOnScreenUpdated(space, pid, windows_with_titles, app_info) => {
                self.debug_tree(space);
                self.floating.clear_active_for_app(space, pid);

                let mut windows_by_workspace: HashMap<
                    crate::model::VirtualWorkspaceId,
                    Vec<WindowId>,
                > = HashMap::default();

                let (app_bundle_id, app_name) = match app_info.as_ref() {
                    Some(info) => (info.bundle_id.as_deref(), info.localized_name.as_deref()),
                    None => (None, None),
                };

                for (wid, title_opt, ax_role_opt, ax_subrole_opt) in windows_with_titles {
                    let title_ref = title_opt.as_deref();
                    let ax_role_ref = ax_role_opt.as_deref();
                    let ax_subrole_ref = ax_subrole_opt.as_deref();

                    let was_floating = self.floating.is_floating(wid);
                    let assignment = match self
                        .virtual_workspace_manager
                        .assign_window_with_app_info(
                            wid,
                            space,
                            app_bundle_id,
                            app_name,
                            title_ref,
                            ax_role_ref,
                            ax_subrole_ref,
                        ) {
                        Ok(AppRuleResult::Managed(decision)) => Some(decision),
                        Ok(AppRuleResult::Unmanaged) => None,
                        Err(_) => {
                            match self.virtual_workspace_manager.auto_assign_window(wid, space) {
                                Ok(ws) => Some(AppRuleAssignment {
                                    workspace_id: ws,
                                    floating: was_floating,
                                    prev_rule_decision: false,
                                }),
                                Err(_) => {
                                    warn!(
                                        "Could not determine workspace for window {:?} on space {:?}; skipping assignment",
                                        wid, space
                                    );
                                    continue;
                                }
                            }
                        }
                    };

                    let AppRuleAssignment {
                        workspace_id: assigned_workspace,
                        floating: rule_says_float,
                        prev_rule_decision,
                    } = match assignment {
                        Some(assign) => assign,
                        None => continue,
                    };

                    let should_float = rule_says_float || (!prev_rule_decision && was_floating);

                    if should_float {
                        self.floating.add_floating(wid);
                        self.floating.add_active(space, pid, wid);
                    } else if was_floating {
                        self.floating.remove_floating(wid);
                    }

                    if !self.floating.is_floating(wid) {
                        windows_by_workspace.entry(assigned_workspace).or_default().push(wid);
                    }

                    self.virtual_workspace_manager_mut().set_last_rule_decision(
                        space,
                        wid,
                        rule_says_float,
                    );
                }

                // `windows_by_workspace` already excludes floating windows.
                let tiled_by_workspace = windows_by_workspace;

                let total_tiled_count: usize = tiled_by_workspace.values().map(|v| v.len()).sum();

                for (ws_id, layout) in self.workspace_layouts.active_layouts_for_space(space) {
                    let mut desired = tiled_by_workspace.get(&ws_id).cloned().unwrap_or_default();
                    for wid in self.virtual_workspace_manager.workspace_windows(space, ws_id) {
                        if wid.pid != pid
                            || self.floating.is_floating(wid)
                            || desired.contains(&wid)
                        {
                            continue;
                        }
                        desired.push(wid);
                    }

                    if desired.is_empty() && total_tiled_count == 0 {
                        if self.workspace_tree(ws_id).has_windows_for_app(layout, pid) {
                            continue;
                        }
                    }

                    self.workspace_tree_mut(ws_id).set_windows_for_app(layout, pid, desired);
                }

                self.broadcast_windows_changed(space);

                self.rebalance_all_layouts();
            }
            LayoutEvent::AppClosed(pid) => {
                for (_, ws) in self.virtual_workspace_manager.workspaces.iter_mut() {
                    ws.layout_system.remove_windows_for_app(pid);
                }
                self.floating.remove_all_for_pid(pid);

                self.virtual_workspace_manager.remove_windows_for_app(pid);
                self.virtual_workspace_manager.remove_app_floating_positions(pid);
            }
            LayoutEvent::WindowAdded(space, wid) => {
                self.debug_tree(space);

                let assigned_workspace =
                    match self.virtual_workspace_manager.workspace_for_window(space, wid) {
                        Some(workspace_id) => workspace_id,
                        None => match self.virtual_workspace_manager.auto_assign_window(wid, space)
                        {
                            Ok(workspace_id) => workspace_id,
                            Err(e) => {
                                warn!("Failed to auto-assign window to workspace: {:?}", e);
                                self.virtual_workspace_manager
                                    .active_workspace(space)
                                    .expect("No active workspace available")
                            }
                        },
                    };

                let should_be_floating = self.floating.is_floating(wid);

                if should_be_floating {
                    self.floating.add_active(space, wid.pid, wid);
                } else if let Some(layout) =
                    self.workspace_layouts.active(space, assigned_workspace)
                {
                    if !self.workspace_tree(assigned_workspace).contains_window(layout, wid) {
                        self.workspace_tree_mut(assigned_workspace)
                            .add_window_after_selection(layout, wid);
                    }
                } else {
                    warn!(
                        "No active layout for workspace {:?} on space {:?}; window {:?} not added to tree",
                        assigned_workspace, space, wid
                    );
                }

                self.broadcast_windows_changed(space);
            }
            LayoutEvent::WindowRemoved(wid) => {
                self.remove_window_internal(wid, false);
            }
            LayoutEvent::WindowRemovedPreserveFloating(wid) => {
                self.remove_window_internal(wid, true);
            }
            LayoutEvent::WindowFocused(space, wid) => {
                self.focused_window = Some(wid);
                if self.floating.is_floating(wid) {
                    self.floating.set_last_focus(Some(wid));
                } else {
                    let (ws_id, layout) = self.workspace_and_layout(space);
                    let _ = self.workspace_tree_mut(ws_id).select_window(layout, wid);
                    self.virtual_workspace_manager.set_last_focused_window(space, ws_id, Some(wid));
                }
            }
            LayoutEvent::WindowResized {
                wid,
                old_frame,
                new_frame,
                screens,
            } => {
                for (space, screen_frame, display_uuid) in screens {
                    let (ws_id, layout) = self.workspace_and_layout(space);
                    let gaps =
                        self.layout_settings.gaps.effective_for_display(display_uuid.as_deref());
                    self.workspace_tree_mut(ws_id).on_window_resized(
                        layout,
                        wid,
                        old_frame,
                        new_frame,
                        screen_frame,
                        &gaps,
                    );

                    self.workspace_layouts.mark_last_saved(space, ws_id, layout);
                }
            }
        }
        EventResponse::default()
    }

    pub fn handle_command(
        &mut self,
        space: Option<SpaceId>,
        visible_spaces: &[SpaceId],
        visible_space_centers: &HashMap<SpaceId, CGPoint>,
        command: LayoutCommand,
    ) -> EventResponse {
        if let Some(space) = space {
            if let Some(ws_id) = self.virtual_workspace_manager.active_workspace(space) {
                if let Some(layout) = self.workspace_layouts.active(space, ws_id) {
                    debug!("Tree:\n{}", self.workspace_tree(ws_id).draw_tree(layout).trim());
                    debug!(selection_window = ?self.workspace_tree(ws_id).selected_window(layout));
                } else {
                    debug!("No active layout for workspace {:?} on space {:?}", ws_id, space);
                }
            } else {
                debug!("No active workspace for space {:?}", space);
            }
        }
        let is_floating = if let Some(focus) = self.focused_window {
            self.floating.is_floating(focus)
        } else {
            false
        };
        debug!(?self.focused_window, last_floating_focus=?self.floating.last_focus(), ?is_floating);

        if let LayoutCommand::ToggleWindowFloating = &command {
            let Some(wid) = self.focused_window else {
                return EventResponse::default();
            };
            if is_floating {
                if let Some(space) = space {
                    let assigned_workspace = self
                        .virtual_workspace_manager
                        .workspace_for_window(space, wid)
                        .unwrap_or_else(|| {
                            self.virtual_workspace_manager
                                .active_workspace(space)
                                .expect("No active workspace available")
                        });

                    if let Some(layout) = self.workspace_layouts.active(space, assigned_workspace) {
                        self.workspace_tree_mut(assigned_workspace)
                            .add_window_after_selection(layout, wid);
                        debug!(
                            "Re-added floating window {:?} to tiling tree in workspace {:?}",
                            wid, assigned_workspace
                        );
                    }

                    self.floating.remove_active(space, wid.pid, wid);
                }
                self.floating.remove_floating(wid);
                self.floating.set_last_focus(None);
            } else {
                if let Some(space) = space {
                    self.floating.add_active(space, wid.pid, wid);
                    let (ws_id, _) = self.workspace_and_layout(space);
                    self.workspace_tree_mut(ws_id).remove_window(wid);
                }
                self.floating.add_floating(wid);
                self.floating.set_last_focus(Some(wid));
                debug!("Removed window {:?} from tiling tree, now floating", wid);
            }
            return EventResponse::default();
        }

        let Some(space) = space else {
            return EventResponse::default();
        };
        let workspace_id = match self.virtual_workspace_manager.active_workspace(space) {
            Some(id) => id,
            None => {
                warn!("No active virtual workspace for space {:?}", space);
                return EventResponse::default();
            }
        };
        let layout = match self.workspace_layouts.active(space, workspace_id) {
            Some(id) => id,
            None => {
                warn!(
                    "No active layout for workspace {:?} on space {:?}; command ignored",
                    workspace_id, space
                );
                return EventResponse::default();
            }
        };

        if let LayoutCommand::ToggleFocusFloating = &command {
            if is_floating {
                let selection = self.workspace_tree(workspace_id).selected_window(layout);
                let mut raise_windows =
                    self.workspace_tree(workspace_id).visible_windows_in_layout(layout);
                let focus_window = selection.or_else(|| raise_windows.pop());
                let response = EventResponse {
                    raise_windows,
                    focus_window,
                    boundary_hit: None,
                };
                self.apply_focus_response(space, workspace_id, layout, &response);
                return response;
            } else {
                let floating_windows: Vec<WindowId> =
                    self.active_floating_windows_in_workspace(space);
                let mut raise_windows: Vec<_> = floating_windows
                    .iter()
                    .copied()
                    .filter(|wid| Some(*wid) != self.floating.last_focus())
                    .collect();
                let focus_window = self.floating.last_focus().or_else(|| raise_windows.pop());
                let response = EventResponse {
                    raise_windows,
                    focus_window,
                    boundary_hit: None,
                };
                self.apply_focus_response(space, workspace_id, layout, &response);
                return response;
            }
        }

        match command {
            LayoutCommand::ToggleWindowFloating => unreachable!(),
            LayoutCommand::ToggleFocusFloating => unreachable!(),

            LayoutCommand::SwapWindows(a, b) => {
                let (ws_id, layout) = self.workspace_and_layout(space);
                let _ = self.workspace_tree_mut(ws_id).swap_windows(layout, a, b);

                EventResponse::default()
            }

            LayoutCommand::NextWindow => self.move_focus_internal(
                space,
                visible_spaces,
                visible_space_centers,
                Direction::Right,
                is_floating,
            ),
            LayoutCommand::PrevWindow => self.move_focus_internal(
                space,
                visible_spaces,
                visible_space_centers,
                Direction::Left,
                is_floating,
            ),
            LayoutCommand::MoveFocus(direction) => {
                debug!(
                    "MoveFocus command received, direction: {:?}, is_floating: {}",
                    direction, is_floating
                );
                return self.move_focus_internal(
                    space,
                    visible_spaces,
                    visible_space_centers,
                    direction,
                    is_floating,
                );
            }
            LayoutCommand::Ascend => {
                if is_floating {
                    return EventResponse::default();
                }
                self.workspace_tree_mut(workspace_id).ascend_selection(layout);
                EventResponse::default()
            }
            LayoutCommand::Descend => {
                self.workspace_tree_mut(workspace_id).descend_selection(layout);
                EventResponse::default()
            }
            LayoutCommand::MoveNode(direction) => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                if !self.workspace_tree_mut(workspace_id).move_selection(layout, direction) {
                    if let Some(new_space) = self.next_space_for_direction(
                        space,
                        direction,
                        visible_spaces,
                        visible_space_centers,
                    ) {
                        let (new_ws_id, new_layout) = self.workspace_and_layout(new_space);
                        let windows = self
                            .workspace_tree(workspace_id)
                            .visible_windows_under_selection(layout);
                        for wid in windows {
                            self.workspace_tree_mut(workspace_id).remove_window(wid);
                            self.workspace_tree_mut(new_ws_id)
                                .add_window_after_selection(new_layout, wid);
                            self.virtual_workspace_manager
                                .assign_window_to_workspace(new_space, wid, new_ws_id);
                        }
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::ToggleFullscreen => {
                let raise_windows =
                    self.workspace_tree_mut(workspace_id).toggle_fullscreen_of_selection(layout);
                if raise_windows.is_empty() {
                    EventResponse::default()
                } else {
                    EventResponse {
                        raise_windows,
                        focus_window: None,
                        boundary_hit: None,
                    }
                }
            }
            LayoutCommand::ToggleFullscreenWithinGaps => {
                let raise_windows = self
                    .workspace_tree_mut(workspace_id)
                    .toggle_fullscreen_within_gaps_of_selection(layout);
                if raise_windows.is_empty() {
                    EventResponse::default()
                } else {
                    EventResponse {
                        raise_windows,
                        focus_window: None,
                        boundary_hit: None,
                    }
                }
            }
            // handled by upper reactor
            LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::MoveWindowToWorkspace { .. }
            | LayoutCommand::SetWorkspaceLayout { .. }
            | LayoutCommand::CreateWorkspace
            | LayoutCommand::SwitchToLastWorkspace => EventResponse::default(),
            LayoutCommand::JoinWindow(direction) => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                self.workspace_tree_mut(workspace_id)
                    .join_selection_with_direction(layout, direction);
                EventResponse::default()
            }
            LayoutCommand::ToggleStack => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let default_orientation: crate::common::config::StackDefaultOrientation =
                    self.layout_settings.stack.default_orientation;
                let unstacked_windows = self
                    .workspace_tree_mut(workspace_id)
                    .unstack_parent_of_selection(layout, default_orientation);

                if !unstacked_windows.is_empty() {
                    return EventResponse {
                        raise_windows: unstacked_windows,
                        focus_window: None,
                        boundary_hit: None,
                    };
                }

                let stacked_windows = self
                    .workspace_tree_mut(workspace_id)
                    .apply_stacking_to_parent_of_selection(layout, default_orientation);
                if !stacked_windows.is_empty() {
                    return EventResponse {
                        raise_windows: stacked_windows,
                        focus_window: None,
                        boundary_hit: None,
                    };
                }

                let visible_windows =
                    self.workspace_tree(workspace_id).visible_windows_in_layout(layout);
                if !visible_windows.is_empty() {
                    EventResponse {
                        raise_windows: visible_windows,
                        focus_window: None,
                        boundary_hit: None,
                    }
                } else {
                    EventResponse::default()
                }
            }
            LayoutCommand::UnjoinWindows => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                self.workspace_tree_mut(workspace_id).unjoin_selection(layout);
                EventResponse::default()
            }
            LayoutCommand::ToggleOrientation => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);

                let default_orientation = self.layout_settings.stack.default_orientation;
                let tree = self.workspace_tree_mut(workspace_id);
                let resp = match tree {
                    LayoutSystemKind::Traditional(s) => {
                        if s.parent_of_selection_is_stacked(layout) {
                            let toggled_windows = s
                                .apply_stacking_to_parent_of_selection(layout, default_orientation);
                            if !toggled_windows.is_empty() {
                                EventResponse {
                                    raise_windows: toggled_windows,
                                    focus_window: None,
                                    boundary_hit: None,
                                }
                            } else {
                                EventResponse::default()
                            }
                        } else {
                            s.toggle_tile_orientation(layout);
                            EventResponse::default()
                        }
                    }
                    LayoutSystemKind::Bsp(s) => {
                        s.toggle_tile_orientation(layout);
                        EventResponse::default()
                    }
                    LayoutSystemKind::MasterStack(s) => {
                        if s.parent_of_selection_is_stacked(layout) {
                            let toggled_windows = s
                                .apply_stacking_to_parent_of_selection(layout, default_orientation);
                            if !toggled_windows.is_empty() {
                                EventResponse {
                                    raise_windows: toggled_windows,
                                    focus_window: None,
                                    boundary_hit: None,
                                }
                            } else {
                                EventResponse::default()
                            }
                        } else {
                            s.toggle_tile_orientation(layout);
                            EventResponse::default()
                        }
                    }
                    LayoutSystemKind::Scrolling(s) => {
                        s.toggle_tile_orientation(layout);
                        EventResponse::default()
                    }
                };

                resp
            }
            LayoutCommand::ResizeWindowGrow => {
                if is_floating {
                    return EventResponse::default();
                }

                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let resize_amount = 0.05;
                self.workspace_tree_mut(workspace_id).resize_selection_by(layout, resize_amount);
                EventResponse::default()
            }
            LayoutCommand::ResizeWindowShrink => {
                if is_floating {
                    return EventResponse::default();
                }

                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let resize_amount = -0.05;
                self.workspace_tree_mut(workspace_id).resize_selection_by(layout, resize_amount);
                EventResponse::default()
            }
            LayoutCommand::ResizeWindowBy { amount } => {
                if is_floating {
                    return EventResponse::default();
                }

                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                self.workspace_tree_mut(workspace_id).resize_selection_by(layout, amount);
                EventResponse::default()
            }
            LayoutCommand::AdjustMasterRatio { delta } => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                if let LayoutSystemKind::MasterStack(s) = self.workspace_tree_mut(workspace_id) {
                    s.adjust_master_ratio(layout, delta);
                }
                EventResponse::default()
            }
            LayoutCommand::AdjustMasterCount { delta } => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                if let LayoutSystemKind::MasterStack(s) = self.workspace_tree_mut(workspace_id) {
                    s.adjust_master_count(layout, delta);
                }
                EventResponse::default()
            }
            LayoutCommand::PromoteToMaster => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                if let LayoutSystemKind::MasterStack(s) = self.workspace_tree_mut(workspace_id) {
                    s.promote_to_master(layout);
                }
                EventResponse::default()
            }
            LayoutCommand::SwapMasterStack => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                if let LayoutSystemKind::MasterStack(s) = self.workspace_tree_mut(workspace_id) {
                    s.swap_master_stack(layout);
                }
                EventResponse::default()
            }
            LayoutCommand::ScrollStrip { delta } => {
                let mut resp = EventResponse::default();
                if let LayoutSystemKind::Scrolling(system) = self.workspace_tree_mut(workspace_id) {
                    resp.boundary_hit = system.scroll_by_delta(layout, delta);
                }
                resp
            }
            LayoutCommand::SnapStrip => {
                if let LayoutSystemKind::Scrolling(system) = self.workspace_tree_mut(workspace_id) {
                    system.snap_to_nearest_column(layout);
                }
                EventResponse::default()
            }
            LayoutCommand::CenterSelection => {
                if let LayoutSystemKind::Scrolling(system) = self.workspace_tree_mut(workspace_id) {
                    system.center_selected_column(layout);
                }
                EventResponse::default()
            }
        }
    }

    pub fn calculate_layout(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let (ws_id, layout) = self.workspace_and_layout(space);
        self.workspace_tree(ws_id).calculate_layout(
            layout,
            screen,
            self.layout_settings.stack.stack_offset,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        )
    }

    pub fn calculate_layout_with_virtual_workspaces<F>(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
        get_window_frame: F,
    ) -> Vec<(WindowId, CGRect)>
    where
        F: Fn(WindowId) -> Option<CGRect>,
    {
        use crate::model::HideCorner;

        let mut positions = HashMap::default();
        let window_size = |wid| {
            get_window_frame(wid)
                .map(|f| f.size)
                .unwrap_or_else(|| CGSize::new(500.0, 500.0))
        };
        let center_rect = |size: CGSize| {
            let center = screen.mid();
            let origin = CGPoint::new(center.x - size.width / 2.0, center.y - size.height / 2.0);
            CGRect::new(origin, size)
        };

        fn ensure_visible_floating(
            engine: &mut LayoutEngine,
            positions: &mut HashMap<WindowId, CGRect>,
            space: SpaceId,
            workspace_id: crate::model::VirtualWorkspaceId,
            wid: WindowId,
            candidate: Option<CGRect>,
            store_if_absent: bool,
            screen: &CGRect,
            center_rect: &impl Fn(CGSize) -> CGRect,
            window_size: &impl Fn(WindowId) -> CGSize,
        ) {
            let existing = positions.get(&wid).copied();
            let bundle_id = engine.get_app_bundle_id_for_window(wid);
            let visible = candidate.or(existing).filter(|rect| {
                !engine.virtual_workspace_manager.is_hidden_position(
                    screen,
                    rect,
                    bundle_id.as_deref(),
                )
            });
            let rect = visible.unwrap_or_else(|| center_rect(window_size(wid)));
            positions.insert(wid, rect);
            if store_if_absent {
                engine.virtual_workspace_manager.store_floating_position_if_absent(
                    space,
                    workspace_id,
                    wid,
                    rect,
                );
            } else {
                engine.virtual_workspace_manager.store_floating_position(
                    space,
                    workspace_id,
                    wid,
                    rect,
                );
            }
        }

        if let Some(active_workspace_id) = self.virtual_workspace_manager.active_workspace(space) {
            if let Some(layout) = self.workspace_layouts.active(space, active_workspace_id) {
                let tiled_positions = self.workspace_tree(active_workspace_id).calculate_layout(
                    layout,
                    screen,
                    self.layout_settings.stack.stack_offset,
                    gaps,
                    stack_line_thickness,
                    stack_line_horiz,
                    stack_line_vert,
                );
                for (wid, rect) in tiled_positions {
                    positions.insert(wid, rect);
                }
            }

            let floating_positions = self
                .virtual_workspace_manager
                .get_workspace_floating_positions(space, active_workspace_id);
            for (window_id, stored_position) in floating_positions {
                if self.floating.is_floating(window_id) {
                    ensure_visible_floating(
                        self,
                        &mut positions,
                        space,
                        active_workspace_id,
                        window_id,
                        Some(stored_position),
                        false,
                        &screen,
                        &center_rect,
                        &window_size,
                    );
                }
            }

            let floating_windows = self.active_floating_windows_in_workspace(space);
            for wid in floating_windows {
                ensure_visible_floating(
                    self,
                    &mut positions,
                    space,
                    active_workspace_id,
                    wid,
                    None,
                    false,
                    &screen,
                    &center_rect,
                    &window_size,
                );
            }
        }

        let hidden_windows = self.virtual_workspace_manager.windows_in_inactive_workspaces(space);
        for (index, wid) in hidden_windows.into_iter().enumerate() {
            let original_frame = get_window_frame(wid);

            if self.floating.is_floating(wid) {
                if let Some(workspace_id) =
                    self.virtual_workspace_manager.workspace_for_window(space, wid)
                {
                    ensure_visible_floating(
                        self,
                        &mut positions,
                        space,
                        workspace_id,
                        wid,
                        original_frame,
                        true,
                        &screen,
                        &center_rect,
                        &window_size,
                    );
                }
            }

            let original_size =
                original_frame.map(|f| f.size).unwrap_or_else(|| CGSize::new(500.0, 500.0));
            let app_bundle_id = self.get_app_bundle_id_for_window(wid);
            let hidden_rect = self.virtual_workspace_manager.calculate_hidden_position(
                screen,
                index,
                original_size,
                HideCorner::BottomRight,
                app_bundle_id.as_deref(),
            );
            positions.insert(wid, hidden_rect);
        }

        positions.into_iter().collect()
    }

    pub fn collect_group_containers_in_selection_path(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<GroupContainerInfo> {
        let (ws_id, layout_id) = self.workspace_and_layout(space);
        match self.workspace_tree(ws_id) {
            LayoutSystemKind::Traditional(s) => s.collect_group_containers_in_selection_path(
                layout_id,
                screen,
                self.layout_settings.stack.stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
            LayoutSystemKind::MasterStack(s) => s.collect_group_containers_in_selection_path(
                layout_id,
                screen,
                self.layout_settings.stack.stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
            _ => Vec::new(),
        }
    }

    pub fn active_workspace_for_space_has_fullscreen(&mut self, space: SpaceId) -> bool {
        let (ws_id, layout_id) = self.workspace_and_layout(space);
        self.workspace_tree(ws_id).has_any_fullscreen_node(layout_id)
    }

    pub fn collect_group_containers(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<GroupContainerInfo> {
        let (ws_id, layout_id) = self.workspace_and_layout(space);
        match self.workspace_tree(ws_id) {
            LayoutSystemKind::Traditional(s) => s.collect_group_containers(
                layout_id,
                screen,
                self.layout_settings.stack.stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
            LayoutSystemKind::MasterStack(s) => s.collect_group_containers(
                layout_id,
                screen,
                self.layout_settings.stack.stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
            _ => Vec::new(),
        }
    }

    pub fn calculate_layout_for_workspace(
        &self,
        space: SpaceId,
        workspace_id: crate::model::VirtualWorkspaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let mut positions = HashMap::default();

        if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
            let tiled_positions = self.workspace_tree(workspace_id).calculate_layout(
                layout,
                screen,
                self.layout_settings.stack.stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            );
            for (wid, rect) in tiled_positions {
                positions.insert(wid, rect);
            }
        }

        let floating_positions = self
            .virtual_workspace_manager
            .get_workspace_floating_positions(space, workspace_id);
        for (window_id, stored_position) in floating_positions {
            if self.floating.is_floating(window_id) {
                positions.insert(window_id, stored_position);
            }
        }

        positions.into_iter().collect()
    }

    fn get_app_bundle_id_for_window(&self, _window_id: WindowId) -> Option<String> {
        // The bundle ID is stored in the app info, which we can access via the PID
        // Note: This would need to be available from the reactor state, but since
        // we're in the layout engine, we don't have direct access to that.
        // For now, we'll return None, but this could be improved by passing
        // app information through the layout calculation or storing it separately.

        None
    }

    pub fn layout(&mut self, space: SpaceId) -> LayoutId {
        let workspace_id = self
            .virtual_workspace_manager
            .active_workspace(space)
            .expect("No active workspace for space");

        if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
            layout
        } else {
            let workspaces = self.virtual_workspace_manager_mut().list_workspaces(space).to_vec();
            let default_size = CGSize::new(1000.0, 1000.0);
            for (id, _) in workspaces {
                let tree = &mut self.virtual_workspace_manager.workspaces[id].layout_system;
                self.workspace_layouts
                    .ensure_active_for_workspace(space, default_size, id, tree);
            }

            self.workspace_layouts
                .active(space, workspace_id)
                .expect("Failed to create an active layout for the workspace")
        }
    }

    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let mut buf = String::new();
        File::open(path)?.read_to_string(&mut buf)?;
        Ok(ron::from_str(&buf)?)
    }

    pub fn save(&self, path: PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create(path)?.write_all(self.serialize_to_string().as_bytes())?;
        Ok(())
    }

    pub fn serialize_to_string(&self) -> String { ron::ser::to_string(&self).unwrap() }

    #[cfg(test)]
    pub(crate) fn selected_window(&mut self, space: SpaceId) -> Option<WindowId> {
        let (ws_id, layout) = self.workspace_and_layout(space);
        self.workspace_tree(ws_id).selected_window(layout)
    }

    pub fn handle_virtual_workspace_command(
        &mut self,
        space: SpaceId,
        command: &LayoutCommand,
    ) -> EventResponse {
        match command {
            LayoutCommand::NextWorkspace(skip_empty) => {
                if let Some(current_workspace) =
                    self.virtual_workspace_manager.active_workspace(space)
                {
                    if let Some(next_workspace) = self.virtual_workspace_manager.next_workspace(
                        space,
                        current_workspace,
                        *skip_empty,
                    ) {
                        self.virtual_workspace_manager.set_active_workspace(space, next_workspace);

                        self.update_active_floating_windows(space);

                        self.broadcast_workspace_changed(space);
                        self.broadcast_windows_changed(space);

                        return self.refocus_workspace(space, next_workspace);
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::PrevWorkspace(skip_empty) => {
                if let Some(current_workspace) =
                    self.virtual_workspace_manager.active_workspace(space)
                {
                    if let Some(prev_workspace) = self.virtual_workspace_manager.prev_workspace(
                        space,
                        current_workspace,
                        *skip_empty,
                    ) {
                        self.virtual_workspace_manager.set_active_workspace(space, prev_workspace);

                        self.update_active_floating_windows(space);

                        self.broadcast_workspace_changed(space);
                        self.broadcast_windows_changed(space);

                        return self.refocus_workspace(space, prev_workspace);
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::SwitchToWorkspace(workspace_index) => {
                let workspaces = self.virtual_workspace_manager_mut().list_workspaces(space);
                if let Some((workspace_id, _)) = workspaces.get(*workspace_index) {
                    let workspace_id = *workspace_id;
                    if self.virtual_workspace_manager.active_workspace(space) == Some(workspace_id)
                    {
                        // Check if workspace_auto_back_and_forth is enabled
                        if self.virtual_workspace_manager.workspace_auto_back_and_forth() {
                            // Switch to last workspace instead
                            if let Some(last_workspace) =
                                self.virtual_workspace_manager.last_workspace(space)
                            {
                                self.virtual_workspace_manager
                                    .set_active_workspace(space, last_workspace);
                                self.update_active_floating_windows(space);
                                self.broadcast_workspace_changed(space);
                                self.broadcast_windows_changed(space);
                                return self.refocus_workspace(space, last_workspace);
                            }
                        }
                        return EventResponse::default();
                    }
                    self.virtual_workspace_manager.set_active_workspace(space, workspace_id);

                    self.update_active_floating_windows(space);

                    self.broadcast_workspace_changed(space);
                    self.broadcast_windows_changed(space);

                    return self.refocus_workspace(space, workspace_id);
                }
                EventResponse::default()
            }
            LayoutCommand::MoveWindowToWorkspace {
                workspace: workspace_index,
                window_id: maybe_id,
            } => {
                let focused_window = if let Some(spec_u32) = maybe_id {
                    match self.virtual_workspace_manager.find_window_by_idx(space, *spec_u32) {
                        Some(w) => w,
                        None => return EventResponse::default(),
                    }
                } else {
                    match self.focused_window {
                        Some(wid) => wid,
                        None => return EventResponse::default(),
                    }
                };

                let inferred_space = self.space_with_window(focused_window);
                let op_space = if inferred_space == Some(space) {
                    space
                } else {
                    inferred_space.unwrap_or(space)
                };

                let workspaces = self.virtual_workspace_manager_mut().list_workspaces(op_space);
                let Some((target_workspace_id, _)) = workspaces.get(*workspace_index) else {
                    return EventResponse::default();
                };
                let target_workspace_id = *target_workspace_id;

                let Some(current_workspace_id) =
                    self.virtual_workspace_manager.workspace_for_window(op_space, focused_window)
                else {
                    return EventResponse::default();
                };

                if current_workspace_id == target_workspace_id {
                    return EventResponse::default();
                }

                let is_floating = self.floating.is_floating(focused_window);

                if is_floating {
                    self.floating.remove_active(op_space, focused_window.pid, focused_window);
                } else if let Some(_layout) =
                    self.workspace_layouts.active(op_space, current_workspace_id)
                {
                    self.workspace_tree_mut(current_workspace_id).remove_window(focused_window);
                }

                let assigned = self.virtual_workspace_manager.assign_window_to_workspace(
                    op_space,
                    focused_window,
                    target_workspace_id,
                );
                if !assigned {
                    if is_floating {
                        self.floating.add_active(op_space, focused_window.pid, focused_window);
                    } else if let Some(prev_layout) =
                        self.workspace_layouts.active(op_space, current_workspace_id)
                    {
                        self.workspace_tree_mut(current_workspace_id)
                            .add_window_after_selection(prev_layout, focused_window);
                    }
                    return EventResponse::default();
                }

                if !is_floating {
                    if let Some(target_layout) =
                        self.workspace_layouts.active(op_space, target_workspace_id)
                    {
                        self.workspace_tree_mut(target_workspace_id)
                            .add_window_after_selection(target_layout, focused_window);
                    }
                }

                let active_workspace = self.virtual_workspace_manager.active_workspace(op_space);

                if Some(target_workspace_id) == active_workspace {
                    if is_floating {
                        self.floating.add_active(op_space, focused_window.pid, focused_window);
                    }
                    return EventResponse {
                        focus_window: Some(focused_window),
                        raise_windows: vec![],
                        boundary_hit: None,
                    };
                } else if Some(current_workspace_id) == active_workspace {
                    self.focused_window = None;
                    self.virtual_workspace_manager.set_last_focused_window(
                        op_space,
                        current_workspace_id,
                        None,
                    );

                    let remaining_windows =
                        self.virtual_workspace_manager.windows_in_active_workspace(op_space);
                    if let Some(&new_focus) = remaining_windows.first() {
                        return EventResponse {
                            focus_window: Some(new_focus),
                            raise_windows: vec![],
                            boundary_hit: None,
                        };
                    }
                }

                self.virtual_workspace_manager.set_last_focused_window(
                    op_space,
                    target_workspace_id,
                    Some(focused_window),
                );

                self.broadcast_windows_changed(op_space);
                EventResponse::default()
            }
            LayoutCommand::CreateWorkspace => {
                match self.virtual_workspace_manager.create_workspace(space, None) {
                    Ok(_workspace_id) => {
                        self.broadcast_workspace_changed(space);
                    }
                    Err(e) => {
                        warn!("Failed to create new workspace: {:?}", e);
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::SwitchToLastWorkspace => {
                if let Some(last_workspace) = self.virtual_workspace_manager.last_workspace(space) {
                    self.virtual_workspace_manager.set_active_workspace(space, last_workspace);

                    self.update_active_floating_windows(space);

                    self.broadcast_workspace_changed(space);
                    self.broadcast_windows_changed(space);

                    return self.refocus_workspace(space, last_workspace);
                }
                EventResponse::default()
            }
            LayoutCommand::SetWorkspaceLayout { workspace, mode } => {
                let Some(workspace_id) = self.workspace_id_for_index(space, *workspace) else {
                    return EventResponse::default();
                };

                if !self.switch_workspace_layout_mode(space, workspace_id, *mode) {
                    return EventResponse::default();
                }

                let is_active_workspace =
                    self.virtual_workspace_manager.active_workspace(space) == Some(workspace_id);
                let raise_windows = if is_active_workspace {
                    self.windows_in_active_workspace(space)
                } else {
                    Vec::new()
                };
                self.broadcast_workspace_changed(space);
                self.broadcast_windows_changed(space);

                EventResponse {
                    raise_windows,
                    focus_window: if is_active_workspace {
                        self.focused_window
                    } else {
                        None
                    },
                    boundary_hit: None,
                }
            }
            _ => EventResponse::default(),
        }
    }

    pub fn virtual_workspace_manager(&self) -> &VirtualWorkspaceManager {
        &self.virtual_workspace_manager
    }

    pub fn virtual_workspace_manager_mut(&mut self) -> &mut VirtualWorkspaceManager {
        &mut self.virtual_workspace_manager
    }

    pub fn active_workspace(&self, space: SpaceId) -> Option<crate::model::VirtualWorkspaceId> {
        self.virtual_workspace_manager.active_workspace(space)
    }

    pub fn active_workspace_idx(&self, space: SpaceId) -> Option<u64> {
        self.virtual_workspace_manager.active_workspace_idx(space)
    }

    pub fn move_window_to_space(
        &mut self,
        source_space: SpaceId,
        target_space: SpaceId,
        target_screen_size: CGSize,
        window_id: WindowId,
    ) -> EventResponse {
        if source_space == target_space {
            return EventResponse {
                raise_windows: vec![window_id],
                focus_window: Some(window_id),
                boundary_hit: None,
            };
        }

        let _ = self.virtual_workspace_manager.list_workspaces(source_space);
        let _ = self.virtual_workspace_manager.list_workspaces(target_space);

        let source_workspace = self
            .virtual_workspace_manager
            .workspace_for_window(source_space, window_id)
            .or_else(|| {
                self.virtual_workspace_manager.workspace_for_window(target_space, window_id)
            });

        let Some(source_workspace_id) = source_workspace else {
            return EventResponse::default();
        };

        let mut target_workspace_id = self.virtual_workspace_manager.active_workspace(target_space);
        if target_workspace_id.is_none() {
            if let Some((id, _)) =
                self.virtual_workspace_manager.list_workspaces(target_space).first()
            {
                self.virtual_workspace_manager.set_active_workspace(target_space, *id);
                target_workspace_id = Some(*id);
            }
        }

        let Some(target_workspace_id) = target_workspace_id else {
            return EventResponse::default();
        };

        let was_floating = self.floating.is_floating(window_id);

        if was_floating {
            self.floating.remove_active(source_space, window_id.pid, window_id);
        } else {
            self.workspace_tree_mut(source_workspace_id).remove_window(window_id);
        }

        let assigned = self.virtual_workspace_manager.assign_window_to_workspace(
            target_space,
            window_id,
            target_workspace_id,
        );

        if !assigned {
            if was_floating {
                self.floating.add_active(source_space, window_id.pid, window_id);
            } else if let Some(src_layout) =
                self.workspace_layouts.active(source_space, source_workspace_id)
            {
                self.workspace_tree_mut(source_workspace_id)
                    .add_window_after_selection(src_layout, window_id);
            }
            return EventResponse::default();
        }

        {
            let workspace_ids = self.virtual_workspace_manager.list_workspaces(target_space);
            for (id, _) in workspace_ids {
                let tree = &mut self.virtual_workspace_manager.workspaces[id].layout_system;
                self.workspace_layouts.ensure_active_for_workspace(
                    target_space,
                    target_screen_size,
                    id,
                    tree,
                );
            }
        }

        if was_floating {
            self.floating.add_active(target_space, window_id.pid, window_id);
            self.floating.set_last_focus(Some(window_id));
        } else if let Some(target_layout) =
            self.workspace_layouts.active(target_space, target_workspace_id)
        {
            self.workspace_tree_mut(target_workspace_id)
                .add_window_after_selection(target_layout, window_id);
        }

        if self.focused_window == Some(window_id) {
            self.focused_window = None;
        }

        if let Some(active_ws) = self.virtual_workspace_manager.active_workspace(source_space) {
            if active_ws == source_workspace_id {
                self.virtual_workspace_manager.set_last_focused_window(
                    source_space,
                    source_workspace_id,
                    None,
                );
            }
        }

        self.virtual_workspace_manager.set_last_focused_window(
            target_space,
            target_workspace_id,
            Some(window_id),
        );
        self.focused_window = Some(window_id);

        if source_space != target_space {
            self.broadcast_windows_changed(source_space);
        }
        self.broadcast_windows_changed(target_space);

        EventResponse {
            raise_windows: vec![window_id],
            focus_window: Some(window_id),
            boundary_hit: None,
        }
    }

    pub fn workspace_name(
        &self,
        space: SpaceId,
        workspace_id: crate::model::VirtualWorkspaceId,
    ) -> Option<String> {
        self.virtual_workspace_manager
            .workspace_info(space, workspace_id)
            .map(|ws| ws.name.clone())
    }

    pub fn windows_in_active_workspace(&self, space: SpaceId) -> Vec<WindowId> {
        self.virtual_workspace_manager.windows_in_active_workspace(space)
    }

    pub fn get_workspace_stats(&self) -> crate::model::virtual_workspace::WorkspaceStats {
        self.virtual_workspace_manager.get_stats()
    }

    pub fn is_window_floating(&self, window_id: WindowId) -> bool {
        self.floating.is_floating(window_id)
    }

    fn update_active_floating_windows(&mut self, space: SpaceId) {
        let windows_in_workspace =
            self.virtual_workspace_manager.windows_in_active_workspace(space);
        self.floating.rebuild_active_for_workspace(space, windows_in_workspace);
    }

    pub fn store_floating_window_positions(
        &mut self,
        space: SpaceId,
        floating_positions: &[(WindowId, CGRect)],
    ) {
        self.virtual_workspace_manager
            .store_current_floating_positions(space, floating_positions);
    }

    fn broadcast_workspace_changed(&self, space_id: SpaceId) {
        if let Some(ref broadcast_tx) = self.broadcast_tx {
            if let Some((active_workspace_id, active_workspace_name)) =
                self.active_workspace_id_and_name(space_id)
            {
                let display_uuid = self.display_uuid_for_space(space_id);
                let _ = broadcast_tx.send(BroadcastEvent::WorkspaceChanged {
                    workspace_id: active_workspace_id,
                    workspace_name: active_workspace_name.clone(),
                    space_id,
                    display_uuid,
                });
            }
        }
    }

    fn broadcast_windows_changed(&self, space_id: SpaceId) {
        if let Some(ref broadcast_tx) = self.broadcast_tx {
            if let Some((workspace_id, workspace_name)) =
                self.active_workspace_id_and_name(space_id)
            {
                let windows = self
                    .virtual_workspace_manager
                    .windows_in_active_workspace(space_id)
                    .iter()
                    .map(|window_id| window_id.to_debug_string())
                    .collect();

                let display_uuid = self.display_uuid_for_space(space_id);
                let event = BroadcastEvent::WindowsChanged {
                    workspace_id,
                    workspace_name,
                    windows,
                    space_id,
                    display_uuid,
                };

                let _ = broadcast_tx.send(event);
            }
        }
    }

    pub fn debug_log_workspace_stats(&self) {
        let stats = self.virtual_workspace_manager.get_stats();
        info!(
            "Workspace Stats: {} workspaces, {} windows, {} active spaces",
            stats.total_workspaces, stats.total_windows, stats.active_spaces
        );

        for (workspace_id, window_count) in &stats.workspace_window_counts {
            info!("  - '{:?}': {} windows", workspace_id, window_count);
        }
    }

    pub fn debug_log_workspace_state(&self, space: SpaceId) {
        if let Some(active_workspace) = self.virtual_workspace_manager.active_workspace(space) {
            if let Some(workspace) =
                self.virtual_workspace_manager.workspace_info(space, active_workspace)
            {
                let active_windows =
                    self.virtual_workspace_manager.windows_in_active_workspace(space);
                let inactive_windows =
                    self.virtual_workspace_manager.windows_in_inactive_workspaces(space);

                info!(
                    "Space {:?}: Active workspace '{}' with {} windows",
                    space,
                    workspace.name,
                    active_windows.len()
                );
                info!("  Active windows: {:?}", active_windows);
                info!("  Inactive windows: {} total", inactive_windows.len());
                if !inactive_windows.is_empty() {
                    info!("  Inactive window IDs: {:?}", inactive_windows);
                }
            }
        } else {
            warn!("Space {:?}: No active workspace set", space);
        }
    }

    fn rebalance_all_layouts(&mut self) {
        let active_layouts = self.workspace_layouts.active_layouts_with_workspace();
        for (ws_id, layout) in active_layouts {
            self.workspace_tree_mut(ws_id).rebalance(layout);
        }
    }

    pub fn is_window_in_active_workspace(&self, space: SpaceId, window_id: WindowId) -> bool {
        self.virtual_workspace_manager.is_window_in_active_workspace(space, window_id)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::AssertUnwindSafe;

    use objc2_core_foundation::CGPoint;

    use super::*;
    use crate::common::collections::HashMap;
    use crate::common::config::{
        LayoutMode, LayoutSettings, VirtualWorkspaceSettings, WorkspaceLayoutRule,
        WorkspaceSelector,
    };

    fn test_engine() -> LayoutEngine {
        LayoutEngine::new(
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
            None,
        )
    }

    fn build_three_spaces() -> (
        Vec<SpaceId>,
        HashMap<SpaceId, CGPoint>,
        SpaceId,
        SpaceId,
        SpaceId,
    ) {
        let left = SpaceId::new(1);
        let right = SpaceId::new(2);
        let middle = SpaceId::new(3);

        let mut centers = HashMap::default();
        centers.insert(left, CGPoint::new(0.0, 0.0));
        centers.insert(right, CGPoint::new(4000.0, 0.0));
        centers.insert(middle, CGPoint::new(2000.0, 0.0));

        (vec![left, right, middle], centers, left, middle, right)
    }

    #[test]
    fn next_space_for_direction_respects_physical_layout() {
        let engine = test_engine();
        let (visible_spaces, centers, left, middle, right) = build_three_spaces();

        assert_eq!(
            engine.next_space_for_direction(middle, Direction::Right, &visible_spaces, &centers),
            Some(right)
        );
        assert_eq!(
            engine.next_space_for_direction(middle, Direction::Left, &visible_spaces, &centers),
            Some(left)
        );
        assert_eq!(
            engine.next_space_for_direction(middle, Direction::Up, &visible_spaces, &centers),
            None
        );
    }

    #[test]
    fn handle_command_does_not_panic_before_layout_initialization() {
        let mut engine = test_engine();
        let space = SpaceId::new(42);
        let visible_spaces = vec![space];
        let visible_space_centers = HashMap::default();

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            engine.handle_command(
                Some(space),
                &visible_spaces,
                &visible_space_centers,
                LayoutCommand::NextWindow,
            )
        }));

        assert!(
            result.is_ok(),
            "handle_command should not panic before SpaceExposed"
        );
    }

    #[test]
    fn update_virtual_workspace_settings_reapplies_workspace_rules() {
        let mut engine = test_engine();
        let space = SpaceId::new(7);
        let workspace_list = engine.virtual_workspace_manager_mut().list_workspaces(space);
        let (workspace_id, workspace_name) = workspace_list[0].clone();
        assert_eq!(
            engine
                .virtual_workspace_manager()
                .workspace_info(space, workspace_id)
                .map(|ws| ws.layout_mode()),
            Some(LayoutMode::Traditional)
        );

        let mut settings = VirtualWorkspaceSettings::default();
        settings.workspace_rules = vec![WorkspaceLayoutRule {
            workspace: WorkspaceSelector::Name(workspace_name),
            layout: LayoutMode::Scrolling,
        }];

        engine.update_virtual_workspace_settings(&settings);

        assert_eq!(
            engine
                .virtual_workspace_manager()
                .workspace_info(space, workspace_id)
                .map(|ws| ws.layout_mode()),
            Some(LayoutMode::Scrolling)
        );
    }

    #[test]
    fn set_workspace_layout_for_inactive_workspace_does_not_raise_active_windows() {
        let mut engine = test_engine();
        let space = SpaceId::new(8);
        let window_id = WindowId::new(999, 1);

        let _ = engine.virtual_workspace_manager_mut().list_workspaces(space);
        let _ = engine.virtual_workspace_manager_mut().auto_assign_window(window_id, space);

        let response =
            engine.handle_virtual_workspace_command(space, &LayoutCommand::SetWorkspaceLayout {
                workspace: Some(1),
                mode: LayoutMode::Bsp,
            });

        assert!(response.raise_windows.is_empty());
        assert_eq!(response.focus_window, None);
    }
}
