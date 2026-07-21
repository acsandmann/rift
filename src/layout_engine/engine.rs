use std::cmp::Ordering;
use std::path::PathBuf;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{Direction, FloatingManager, LayoutId, LayoutSystemKind, WorkspaceLayouts};
use crate::actor::app::{AppInfo, WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{LayoutMode, LayoutSettings, VirtualWorkspaceSettings};
use crate::layout_engine::LayoutSystem;
use crate::layout_engine::floating::FloatingFullscreenKind;
use crate::layout_engine::systems::WindowLayoutConstraints;
use crate::model::broadcast::{BroadcastEvent, BroadcastSender};
use crate::model::virtual_workspace::{
    AppRuleAssignment, AppRuleResult, VirtualWorkspace, VirtualWorkspaceId, WorkspaceStore,
};
use crate::model::{AppRuleEngine, FloatingPositionStore, WindowRuleContext, WindowStore};
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

#[derive(Debug, Default)]
struct WindowRemovalImpact {
    active_space: Option<SpaceId>,
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
    ConsumeOrExpelWindow(Direction),
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

    AdjustMasterRatio(f64),
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
        Vec<(
            WindowId,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
            CGSize,
            Option<CGSize>,
            Option<CGSize>,
        )>,
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
    floating_positions: FloatingPositionStore,
    #[serde(skip)]
    app_rules: AppRuleEngine,
    #[serde(skip)]
    focused_window: Option<WindowId>,
    #[serde(skip)]
    window_layout_constraints: HashMap<WindowId, WindowLayoutConstraints>,
    virtual_workspace_manager: WorkspaceStore,
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
    pub fn focused_window(&self) -> Option<WindowId> { self.focused_window }

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
    fn workspace_and_layout(&self, space: SpaceId) -> Option<(VirtualWorkspaceId, LayoutId)> {
        let ws_id = self.active_workspace_id(space)?;
        let layout = self.workspace_layouts.active(space, ws_id)?;
        Some((ws_id, layout))
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
        window_store: &WindowStore,
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
            let mut hidden_windows: Vec<_> = self
                .virtual_workspace_manager
                .workspace_windows(window_store, space, workspace_id)
                .into_iter()
                .filter(|wid| !ordered.contains(wid))
                .collect();
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

    fn response_for_raised_windows(raise_windows: Vec<WindowId>) -> EventResponse {
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

    fn toggle_orientation_for_system<S: LayoutSystem>(
        system: &mut S,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> EventResponse {
        if system.parent_of_selection_is_stacked(layout) {
            let toggled_windows =
                system.apply_stacking_to_parent_of_selection(layout, default_orientation);
            return Self::response_for_raised_windows(toggled_windows);
        }
        system.toggle_tile_orientation(layout);
        EventResponse::default()
    }

    fn toggle_stack_for_workspace(
        &mut self,
        workspace_id: VirtualWorkspaceId,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> EventResponse {
        let unstacked_windows = {
            self.workspace_tree_mut(workspace_id)
                .unstack_parent_of_selection(layout, default_orientation)
        };
        if !unstacked_windows.is_empty() {
            return Self::response_for_raised_windows(unstacked_windows);
        }

        let stacked_windows = {
            self.workspace_tree_mut(workspace_id)
                .apply_stacking_to_parent_of_selection(layout, default_orientation)
        };
        if !stacked_windows.is_empty() {
            return Self::response_for_raised_windows(stacked_windows);
        }

        let visible_windows = self.workspace_tree(workspace_id).visible_windows_in_layout(layout);
        Self::response_for_raised_windows(visible_windows)
    }

    fn collect_group_containers_for_space(
        &self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
        selection_path_only: bool,
    ) -> Vec<GroupContainerInfo> {
        let Some((ws_id, layout_id)) = self.workspace_and_layout(space) else {
            return Vec::new();
        };
        let stack_offset = self.layout_settings.stack.stack_offset;
        match self.workspace_tree(ws_id) {
            LayoutSystemKind::Traditional(s) => {
                if selection_path_only {
                    s.collect_group_containers_in_selection_path(
                        layout_id,
                        screen,
                        stack_offset,
                        gaps,
                        stack_line_thickness,
                        stack_line_horiz,
                        stack_line_vert,
                    )
                } else {
                    s.collect_group_containers(
                        layout_id,
                        screen,
                        stack_offset,
                        gaps,
                        stack_line_thickness,
                        stack_line_horiz,
                        stack_line_vert,
                    )
                }
            }
            LayoutSystemKind::Stack(s) => {
                if selection_path_only {
                    s.collect_group_containers_in_selection_path(
                        layout_id,
                        screen,
                        stack_offset,
                        gaps,
                        stack_line_thickness,
                        stack_line_horiz,
                        stack_line_vert,
                    )
                } else {
                    s.collect_group_containers(
                        layout_id,
                        screen,
                        stack_offset,
                        gaps,
                        stack_line_thickness,
                        stack_line_horiz,
                        stack_line_vert,
                    )
                }
            }
            LayoutSystemKind::MasterStack(s) => {
                if selection_path_only {
                    s.collect_group_containers_in_selection_path(
                        layout_id,
                        screen,
                        stack_offset,
                        gaps,
                        stack_line_thickness,
                        stack_line_horiz,
                        stack_line_vert,
                    )
                } else {
                    s.collect_group_containers(
                        layout_id,
                        screen,
                        stack_offset,
                        gaps,
                        stack_line_thickness,
                        stack_line_horiz,
                        stack_line_vert,
                    )
                }
            }
            _ => Vec::new(),
        }
    }
}

impl LayoutEngine {
    pub fn set_layout_settings(&mut self, settings: &LayoutSettings) {
        self.layout_settings = settings.clone();

        for (_, ws) in self.virtual_workspace_manager.workspaces.iter_mut() {
            match &mut ws.layout_system {
                LayoutSystemKind::Stack(system) => {
                    system.update_settings(settings.stack.default_orientation);
                }
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

    pub fn update_virtual_workspace_settings(
        &mut self,
        window_store: &WindowStore,
        settings: &crate::common::config::VirtualWorkspaceSettings,
    ) {
        self.app_rules = AppRuleEngine::new(&settings.app_rules);
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
                    let _ = self.switch_workspace_layout_mode(
                        window_store,
                        space,
                        *workspace_id,
                        desired_mode,
                    );
                }
            }
        }
    }

    pub fn layout_mode_at(&self, space: SpaceId) -> &'static str {
        if let Some(ws_id) = self.virtual_workspace_manager.active_workspace(space) {
            match self.workspace_tree(ws_id) {
                LayoutSystemKind::Traditional(_) => "traditional",
                LayoutSystemKind::Bsp(_) => "bsp",
                LayoutSystemKind::Stack(_) => "stack",
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
                LayoutSystemKind::Stack(_) => crate::common::config::LayoutMode::Stack,
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

    fn active_floating_windows_in_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
    ) -> Vec<WindowId> {
        self.floating
            .active_flat(space)
            .into_iter()
            .filter(|wid| self.is_window_in_active_workspace(window_store, space, *wid))
            .collect()
    }

    fn preferred_focus_for_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        preferred_focus_window: Option<WindowId>,
    ) -> Option<WindowId> {
        let mut focus_window = preferred_focus_window.filter(|wid| {
            self.virtual_workspace_manager.workspace_for_window(window_store, space, *wid)
                == Some(workspace_id)
        });

        if focus_window.is_none() {
            focus_window = self
                .virtual_workspace_manager
                .last_focused_window(space, workspace_id)
                .filter(|wid| {
                    self.virtual_workspace_manager.workspace_for_window(window_store, space, *wid)
                        == Some(workspace_id)
                });
        }

        if focus_window.is_none() {
            if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
                let selected =
                    self.workspace_tree(workspace_id).selected_window(layout).filter(|wid| {
                        self.virtual_workspace_manager.workspace_for_window(
                            window_store,
                            space,
                            *wid,
                        ) == Some(workspace_id)
                    });
                let visible = self
                    .workspace_tree(workspace_id)
                    .visible_windows_in_layout(layout)
                    .into_iter()
                    .find(|wid| {
                        self.virtual_workspace_manager.workspace_for_window(
                            window_store,
                            space,
                            *wid,
                        ) == Some(workspace_id)
                    });
                focus_window = selected.or(visible);
            }
        }

        if focus_window.is_none() {
            let floating_windows = self.active_floating_windows_in_workspace(window_store, space);
            let floating_focus =
                self.floating.last_focus().filter(|wid| floating_windows.contains(wid));
            focus_window = floating_focus.or_else(|| floating_windows.first().copied());
        }

        focus_window
    }

    pub fn commit_workspace_focus(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        focus_window: Option<WindowId>,
    ) {
        let Some(workspace_id) = self.virtual_workspace_manager.active_workspace(space) else {
            self.focused_window = None;
            return;
        };

        let focus_window = focus_window.filter(|wid| {
            self.virtual_workspace_manager.workspace_for_window(window_store, space, *wid)
                == Some(workspace_id)
        });

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
    }

    fn activate_workspace(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        preferred_focus_window: Option<WindowId>,
    ) -> EventResponse {
        self.virtual_workspace_manager.set_active_workspace(space, workspace_id);
        self.update_active_floating_windows(window_store, space);
        self.broadcast_workspace_changed(space);
        self.broadcast_windows_changed(window_store, space);

        EventResponse {
            focus_window: self.preferred_focus_for_workspace(
                window_store,
                space,
                workspace_id,
                preferred_focus_window,
            ),
            raise_windows: vec![],
            boundary_hit: None,
        }
    }

    fn switch_to_workspace(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_index: usize,
        preferred_focus_window: Option<WindowId>,
    ) -> EventResponse {
        let workspaces = self.virtual_workspace_manager_mut().list_workspaces(space);
        if let Some((workspace_id, _)) = workspaces.get(workspace_index) {
            let workspace_id = *workspace_id;
            if self.virtual_workspace_manager.active_workspace(space) == Some(workspace_id) {
                // Check if workspace_auto_back_and_forth is enabled
                if self.virtual_workspace_manager.workspace_auto_back_and_forth() {
                    // Switch to last workspace instead
                    if let Some(last_workspace) =
                        self.virtual_workspace_manager.last_workspace(space)
                    {
                        return self.activate_workspace(window_store, space, last_workspace, None);
                    }
                }
                return EventResponse::default();
            }
            return self.activate_workspace(
                window_store,
                space,
                workspace_id,
                preferred_focus_window,
            );
        }
        EventResponse::default()
    }

    fn filter_active_workspace_windows(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        windows: Vec<WindowId>,
    ) -> Vec<WindowId> {
        windows
            .into_iter()
            .filter(|wid| self.is_window_in_active_workspace(window_store, space, *wid))
            .collect()
    }

    fn filter_active_workspace_window(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        window: Option<WindowId>,
    ) -> Option<WindowId> {
        window.filter(|wid| self.is_window_in_active_workspace(window_store, space, *wid))
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
        _window_store: &mut WindowStore,
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
        window_store: &mut WindowStore,
        space: SpaceId,
        visible_spaces: &[SpaceId],
        visible_space_centers: &HashMap<SpaceId, CGPoint>,
        direction: Direction,
        is_floating: bool,
    ) -> EventResponse {
        let Some((ws_id, layout)) = self.workspace_and_layout(space) else {
            warn!(
                "No active workspace/layout for space {:?}; move_focus ignored",
                space
            );
            return EventResponse::default();
        };

        if is_floating {
            let floating_windows = self.active_floating_windows_in_workspace(window_store, space);
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
                            self.apply_focus_response(
                                window_store,
                                space,
                                ws_id,
                                layout,
                                &response,
                            );
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
                window_store,
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
                self.apply_focus_response(window_store, space, ws_id, layout, &response);
                return response;
            }

            debug!("No windows to navigate to, returning default");
            return EventResponse::default();
        }

        let previous_selection = self.workspace_tree(ws_id).selected_window(layout);

        let (focus_window_raw, raise_windows) =
            self.workspace_tree_mut(ws_id).move_focus(layout, direction);
        let focus_window =
            self.filter_active_workspace_window(window_store, space, focus_window_raw);
        let raise_windows =
            self.filter_active_workspace_windows(window_store, space, raise_windows);
        if focus_window.is_some() {
            let response = EventResponse {
                focus_window,
                raise_windows,
                boundary_hit: None,
            };
            self.apply_focus_response(window_store, space, ws_id, layout, &response);
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
                let Some((new_ws_id, new_layout)) = self.workspace_and_layout(new_space) else {
                    debug!(
                        "No active workspace/layout for adjacent space {:?}; skipping cross-space focus",
                        new_space
                    );
                    return EventResponse::default();
                };
                let windows_in_new_space = self.filter_active_workspace_windows(
                    window_store,
                    new_space,
                    self.workspace_tree(new_ws_id).visible_windows_in_layout(new_layout),
                );
                if let Some(target_window) = self
                    .filter_active_workspace_window(
                        window_store,
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
                    self.apply_focus_response(
                        window_store,
                        new_space,
                        new_ws_id,
                        new_layout,
                        &response,
                    );
                    return response;
                }
            }

            let floating_windows = self.active_floating_windows_in_workspace(window_store, space);

            if let Some(&first_floating) = floating_windows.first() {
                let focus_window = Some(first_floating);
                let response = EventResponse {
                    focus_window,
                    raise_windows: vec![],
                    boundary_hit: None,
                };
                self.apply_focus_response(window_store, space, ws_id, layout, &response);
                return response;
            }

            let visible_windows = self.filter_active_workspace_windows(
                window_store,
                space,
                self.workspace_tree(ws_id).visible_windows_in_layout(layout),
            );

            if let Some(fallback_focus) = self
                .filter_active_workspace_window(window_store, space, previous_selection)
                .or_else(|| visible_windows.first().copied())
            {
                let response = EventResponse {
                    focus_window: Some(fallback_focus),
                    raise_windows: vec![],
                    boundary_hit: None,
                };
                self.apply_focus_response(window_store, space, ws_id, layout, &response);
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

    fn remove_window_internal(
        &mut self,
        window_store: &mut WindowStore,
        wid: WindowId,
        preserve_floating: bool,
    ) {
        let removal = self.remove_window_layout_membership(window_store, wid);

        if preserve_floating {
            self.floating.remove_active_for_window(wid);
        } else {
            self.floating.remove_floating(wid);
        }

        if !preserve_floating {
            self.virtual_workspace_manager.remove_window(window_store, wid);
            self.floating_positions.remove_window(wid);
        }

        if self.focused_window == Some(wid) {
            self.focused_window = None;
        }
        self.window_layout_constraints.remove(&wid);

        if let Some(space) = removal.active_space {
            self.broadcast_windows_changed(window_store, space);
        }
    }

    fn remove_window_layout_membership(
        &mut self,
        window_store: &WindowStore,
        wid: WindowId,
    ) -> WindowRemovalImpact {
        let active_space = self.space_with_window(wid);
        let tiled_workspaces =
            self.virtual_workspace_manager.workspaces_for_window(window_store, wid);

        if !tiled_workspaces.is_empty() {
            for ws_id in &tiled_workspaces {
                self.workspace_tree_mut(*ws_id).remove_window(wid);
            }
            return WindowRemovalImpact { active_space };
        }

        // The store may already have dropped the record (for example after
        // WindowDestroyed). Layout membership is only a projection, so scrub
        // every tree when its authoritative assignment is unavailable.
        let ws_ids: Vec<_> = self.virtual_workspace_manager.workspaces.keys().collect();
        for ws_id in ws_ids {
            self.workspace_tree_mut(ws_id).remove_window_and_rebalance_parent(wid);
        }
        WindowRemovalImpact { active_space }
    }

    fn add_window_to_layout(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        wid: WindowId,
    ) -> bool {
        let active_space_before = self.space_with_window(wid);

        let assigned_workspace =
            match self.virtual_workspace_manager.workspace_for_window(window_store, space, wid) {
                Some(workspace_id) => workspace_id,
                None => match self.virtual_workspace_manager.auto_assign_window(
                    window_store,
                    wid,
                    space,
                ) {
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
        } else if let Some(layout) = self.workspace_layouts.active(space, assigned_workspace) {
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

        self.space_with_window(wid) != active_space_before
    }

    fn remove_window_from_all_tiling_trees(&mut self, wid: WindowId) {
        let ws_ids: Vec<_> = self.virtual_workspace_manager.workspaces.keys().collect();
        for ws_id in ws_ids {
            self.workspace_tree_mut(ws_id).remove_window(wid);
        }
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

    fn window_no_longer_assigned_to_space(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        wid: WindowId,
    ) -> bool {
        self.virtual_workspace_manager
            .workspace_for_window(window_store, space, wid)
            .is_none()
    }

    fn sync_tiled_windows_for_app(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        pid: pid_t,
        tiled_by_workspace: &HashMap<crate::model::VirtualWorkspaceId, Vec<WindowId>>,
    ) -> Vec<(crate::model::VirtualWorkspaceId, LayoutId)> {
        let total_tiled_count: usize = tiled_by_workspace.values().map(|v| v.len()).sum();
        let mut changed_layouts = Vec::new();

        for (ws_id, layout) in self.workspace_layouts.active_layouts_for_space(space) {
            let mut desired = tiled_by_workspace.get(&ws_id).cloned().unwrap_or_default();
            for wid in self.virtual_workspace_manager.workspace_windows(window_store, space, ws_id)
            {
                let authoritative_native_space =
                    window_store.current_window_server_space_for_window(wid);
                // Skip re-adding if the VWM no longer assigns this window to this space
                // (it was moved to another space during this discovery cycle).
                if wid.pid != pid
                    || self.floating.is_floating(wid)
                    || desired.contains(&wid)
                    || authoritative_native_space.is_some_and(|native_space| native_space != space)
                    || self.window_no_longer_assigned_to_space(window_store, space, wid)
                {
                    continue;
                }
                desired.push(wid);
            }

            if desired.is_empty() && total_tiled_count == 0 {
                // Empty discovery can mean AX temporarily omitted the app. Preserve
                // windows still assigned to this workspace, but allow moved windows
                // to be removed from this layout tree.
                let tree_windows = self.workspace_tree(ws_id).windows_for_app(layout, pid);
                desired = tree_windows
                    .into_iter()
                    .filter(|wid| {
                        self.virtual_workspace_manager.workspace_for_window(
                            window_store,
                            space,
                            *wid,
                        ) == Some(ws_id)
                    })
                    .collect();
            }

            desired.sort_unstable();
            let mut current = self.workspace_tree(ws_id).windows_for_app(layout, pid);
            current.sort_unstable();

            // AX/window-server discovery can temporarily omit windows. Keep windows that are
            // still assigned to this workspace so a partial snapshot does not tear them out
            // of the tree and cause their sibling weights to be rebuilt.
            for wid in current.iter().copied() {
                if !desired.contains(&wid)
                    && !self.floating.is_floating(wid)
                    && self.virtual_workspace_manager.workspace_for_window(window_store, space, wid)
                        == Some(ws_id)
                {
                    desired.push(wid);
                }
            }
            desired.sort_unstable();
            if desired == current {
                continue;
            }

            // Per-app membership reconciliation is not a focus operation.
            // Several layout systems select newly inserted windows as part of
            // their normal insertion semantics, so preserve the selection
            // explicitly across discovery-driven synchronization.
            let selected_window = self.workspace_tree(ws_id).selected_window(layout);
            self.workspace_tree_mut(ws_id).set_windows_for_app(layout, pid, desired);
            if let Some(selected_window) = selected_window
                && self.workspace_tree(ws_id).contains_window(layout, selected_window)
            {
                let _ = self.workspace_tree_mut(ws_id).select_window(layout, selected_window);
            }
            changed_layouts.push((ws_id, layout));
        }

        changed_layouts
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
    pub fn remap_space(
        &mut self,
        window_store: &mut WindowStore,
        old_space: SpaceId,
        new_space: SpaceId,
    ) {
        if old_space == new_space {
            return;
        }

        self.workspace_layouts.remap_space(old_space, new_space);
        self.floating.remap_space(old_space, new_space);
        self.floating_positions.remap_space(old_space, new_space);
        self.virtual_workspace_manager.remap_space(window_store, old_space, new_space);

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
            WorkspaceStore::new_with_config(virtual_workspace_config, layout_settings);

        LayoutEngine {
            workspace_layouts: WorkspaceLayouts::default(),
            floating: FloatingManager::new(),
            floating_positions: FloatingPositionStore::default(),
            app_rules: AppRuleEngine::new(&virtual_workspace_config.app_rules),
            focused_window: None,
            window_layout_constraints: HashMap::default(),
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

    pub fn handle_event(
        &mut self,
        window_store: &mut WindowStore,
        event: LayoutEvent,
    ) -> EventResponse {
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

                for (
                    wid,
                    title_opt,
                    ax_role_opt,
                    ax_subrole_opt,
                    is_resizable,
                    size_hint,
                    min_size,
                    max_size,
                ) in windows_with_titles
                {
                    self.window_layout_constraints.insert(
                        wid,
                        WindowLayoutConstraints {
                            is_resizable,
                            locked_width: size_hint.width,
                            locked_height: size_hint.height,
                            min_width: min_size.map_or(0.0, |s| s.width),
                            min_height: min_size.map_or(0.0, |s| s.height),
                            max_width: max_size.map_or(0.0, |s| s.width),
                            max_height: max_size.map_or(0.0, |s| s.height),
                        }
                        .normalized(),
                    );

                    let title_ref = title_opt.as_deref();
                    let ax_role_ref = ax_role_opt.as_deref();
                    let ax_subrole_ref = ax_subrole_opt.as_deref();

                    let was_floating = self.floating.is_floating(wid);
                    let assignment = match self.assign_window_with_app_info(
                        window_store,
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
                            match self.virtual_workspace_manager.auto_assign_window(
                                window_store,
                                wid,
                                space,
                            ) {
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
                        window_store,
                        space,
                        wid,
                        rule_says_float,
                    );
                }

                // `windows_by_workspace` already excludes floating windows.
                let tiled_by_workspace = windows_by_workspace;
                let changed_layouts =
                    self.sync_tiled_windows_for_app(window_store, space, pid, &tiled_by_workspace);
                if !changed_layouts.is_empty() {
                    self.broadcast_windows_changed(window_store, space);
                }
            }
            LayoutEvent::AppClosed(pid) => {
                for (_, ws) in self.virtual_workspace_manager.workspaces.iter_mut() {
                    ws.layout_system.remove_windows_for_app(pid);
                }
                self.floating.remove_all_for_pid(pid);
                self.window_layout_constraints.retain(|wid, _| wid.pid != pid);

                self.virtual_workspace_manager.remove_windows_for_app(window_store, pid);
                self.floating_positions.remove_app(pid);
            }
            LayoutEvent::WindowAdded(space, wid) => {
                self.debug_tree(space);
                if self.add_window_to_layout(window_store, space, wid) {
                    self.broadcast_windows_changed(window_store, space);
                }
            }
            LayoutEvent::WindowRemoved(wid) => {
                self.remove_window_internal(window_store, wid, false);
            }
            LayoutEvent::WindowRemovedPreserveFloating(wid) => {
                self.remove_window_internal(window_store, wid, true);
            }
            LayoutEvent::WindowFocused(space, wid) => {
                if self.floating.is_floating(wid) {
                    self.focused_window = Some(wid);
                    self.floating.set_last_focus(Some(wid));
                } else if let Some((ws_id, layout)) = self.workspace_and_layout(space) {
                    if !self.workspace_tree(ws_id).contains_window(layout, wid) {
                        warn!(
                            "WindowFocused ignored: wid={:?} not in active layout for space {:?}",
                            wid, space
                        );
                        return EventResponse::default();
                    }
                    self.focused_window = Some(wid);
                    let _ = self.workspace_tree_mut(ws_id).select_window(layout, wid);
                    self.virtual_workspace_manager.set_last_focused_window(space, ws_id, Some(wid));
                } else {
                    warn!(
                        "No active workspace/layout for focused window {:?} on space {:?}",
                        wid, space
                    );
                }
            }
            LayoutEvent::WindowResized {
                wid,
                old_frame,
                new_frame,
                screens,
            } => {
                for (space, screen_frame, display_uuid) in screens {
                    let Some((ws_id, layout)) = self.workspace_and_layout(space) else {
                        debug!(
                            "No active workspace/layout for resized window {:?} on space {:?}; skipping",
                            wid, space
                        );
                        continue;
                    };
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
        window_store: &mut WindowStore,
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
                        .workspace_for_window(window_store, space, wid)
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
                    if let Some((ws_id, _)) = self.workspace_and_layout(space) {
                        self.workspace_tree_mut(ws_id).remove_window(wid);
                    } else {
                        debug!(
                            "No active workspace/layout for space {:?}; leaving window {:?} out of tiling removal",
                            space, wid
                        );
                    }
                }
                self.floating.add_floating(wid);
                self.floating.set_last_focus(Some(wid));
                debug!("Removed window {:?} from tiling tree, now floating", wid);
            }
            return EventResponse::default();
        }

        if let LayoutCommand::ToggleFullscreen | LayoutCommand::ToggleFullscreenWithinGaps =
            &command
            && is_floating
        {
            let Some(wid) = self.focused_window else {
                return EventResponse::default();
            };
            let target = match command {
                LayoutCommand::ToggleFullscreenWithinGaps => FloatingFullscreenKind::WithinGaps,
                _ => FloatingFullscreenKind::Full,
            };
            if self.floating.fullscreen_kind(wid) == Some(target) {
                self.floating.set_fullscreen(wid, None);
            } else {
                // Only save the pre-fullscreen frame when switching from a non-fullscreen state,
                // and _not_ when switching between fullscreen kinds.
                if self.floating.fullscreen_kind(wid).is_none()
                    && let Some(space) = space
                {
                    let ws = self
                        .virtual_workspace_manager
                        .workspace_for_window(window_store, space, wid)
                        .or_else(|| self.virtual_workspace_manager.active_workspace(space));
                    if let (Some(ws), Some(frame)) =
                        (ws, window_store.window(wid).map(|w| w.frame_monotonic))
                    {
                        self.floating_positions.store(space, ws, wid, frame);
                    }
                }
                self.floating.set_fullscreen(wid, Some(target));
            }
            return EventResponse {
                raise_windows: vec![wid],
                focus_window: Some(wid),
                boundary_hit: None,
            };
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
                self.apply_focus_response(window_store, space, workspace_id, layout, &response);
                return response;
            } else {
                let floating_windows: Vec<WindowId> =
                    self.active_floating_windows_in_workspace(window_store, space);
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
                self.apply_focus_response(window_store, space, workspace_id, layout, &response);
                return response;
            }
        }

        match command {
            LayoutCommand::ToggleWindowFloating => unreachable!(),
            LayoutCommand::ToggleFocusFloating => unreachable!(),

            LayoutCommand::SwapWindows(a, b) => {
                let _ = self.workspace_tree_mut(workspace_id).swap_windows(layout, a, b);

                EventResponse::default()
            }
            LayoutCommand::NextWindow | LayoutCommand::PrevWindow => {
                let forward = matches!(command, LayoutCommand::NextWindow);
                let windows = if is_floating {
                    self.active_floating_windows_in_workspace(window_store, space)
                } else {
                    self.filter_active_workspace_windows(
                        window_store,
                        space,
                        self.workspace_tree(workspace_id).visible_windows_in_layout(layout),
                    )
                };
                if let Some(idx) = windows.iter().position(|&w| Some(w) == self.focused_window) {
                    let next = if forward {
                        (idx + 1) % windows.len()
                    } else {
                        (idx + windows.len() - 1) % windows.len()
                    };
                    let response = EventResponse {
                        focus_window: Some(windows[next]),
                        raise_windows: vec![windows[next]],
                        boundary_hit: None,
                    };
                    self.apply_focus_response(window_store, space, workspace_id, layout, &response);
                    return response;
                } else {
                    let focus_window = self
                        .workspace_tree(workspace_id)
                        .selected_window(layout)
                        .filter(|wid| windows.contains(wid))
                        .or_else(|| windows.first().copied());
                    let raise_windows = focus_window.into_iter().collect();
                    let response = EventResponse {
                        focus_window,
                        raise_windows,
                        boundary_hit: None,
                    };
                    self.apply_focus_response(window_store, space, workspace_id, layout, &response);
                    return response;
                }
            }
            LayoutCommand::MoveFocus(direction) => {
                debug!(
                    "MoveFocus command received, direction: {:?}, is_floating: {}",
                    direction, is_floating
                );
                return self.move_focus_internal(
                    window_store,
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
                        let Some((new_ws_id, new_layout)) = self.workspace_and_layout(new_space)
                        else {
                            debug!(
                                "No active workspace/layout for adjacent space {:?}; skipping cross-space move",
                                new_space
                            );
                            return EventResponse::default();
                        };
                        let windows = self
                            .workspace_tree(workspace_id)
                            .visible_windows_under_selection(layout);
                        for wid in windows {
                            self.workspace_tree_mut(workspace_id).remove_window(wid);
                            self.workspace_tree_mut(new_ws_id)
                                .add_window_after_selection(new_layout, wid);
                            self.virtual_workspace_manager.assign_window_to_workspace(
                                window_store,
                                new_space,
                                wid,
                                new_ws_id,
                            );
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
            LayoutCommand::ConsumeOrExpelWindow(direction) => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                self.workspace_tree_mut(workspace_id)
                    .consume_or_expel_selection(layout, direction);
                EventResponse::default()
            }
            LayoutCommand::ToggleStack => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let default_orientation: crate::common::config::StackDefaultOrientation =
                    self.layout_settings.stack.default_orientation;
                self.toggle_stack_for_workspace(workspace_id, layout, default_orientation)
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
                match tree {
                    LayoutSystemKind::Traditional(s) => {
                        Self::toggle_orientation_for_system(s, layout, default_orientation)
                    }
                    LayoutSystemKind::Bsp(s) => {
                        Self::toggle_orientation_for_system(s, layout, default_orientation)
                    }
                    LayoutSystemKind::Stack(s) => {
                        Self::toggle_orientation_for_system(s, layout, default_orientation)
                    }
                    LayoutSystemKind::MasterStack(s) => {
                        Self::toggle_orientation_for_system(s, layout, default_orientation)
                    }
                    LayoutSystemKind::Scrolling(s) => {
                        Self::toggle_orientation_for_system(s, layout, default_orientation)
                    }
                }
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
            LayoutCommand::AdjustMasterRatio(delta) => {
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
        let Some((ws_id, layout)) = self.workspace_and_layout(space) else {
            return Vec::new();
        };
        self.workspace_tree(ws_id).calculate_layout(
            layout,
            screen,
            self.layout_settings.stack.stack_offset,
            &self.window_layout_constraints,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        )
    }

    pub fn calculate_layout_with_virtual_workspaces<F>(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
        get_window_frame: F,
        all_screens: &[CGRect],
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
            all_screens: &[CGRect],
            center_rect: &impl Fn(CGSize) -> CGRect,
            window_size: &impl Fn(WindowId) -> CGSize,
        ) {
            let existing = positions.get(&wid).copied();
            let bundle_id = engine.get_app_bundle_id_for_window(wid);
            let visible = candidate.or(existing).filter(|rect| {
                !engine.virtual_workspace_manager.is_hidden_position_multi(
                    screen,
                    rect,
                    bundle_id.as_deref(),
                    all_screens,
                )
            });
            let rect = visible.unwrap_or_else(|| center_rect(window_size(wid)));
            positions.insert(wid, rect);
            if store_if_absent {
                engine.floating_positions.store_if_absent(space, workspace_id, wid, rect);
            } else {
                engine.floating_positions.store(space, workspace_id, wid, rect);
            }
        }

        if let Some(active_workspace_id) = self.virtual_workspace_manager.active_workspace(space) {
            if let Some(layout) = self.workspace_layouts.active(space, active_workspace_id) {
                let tiled_positions = self.workspace_tree(active_workspace_id).calculate_layout(
                    layout,
                    screen,
                    self.layout_settings.stack.stack_offset,
                    &self.window_layout_constraints,
                    gaps,
                    stack_line_thickness,
                    stack_line_horiz,
                    stack_line_vert,
                );

                for (wid, rect) in tiled_positions {
                    positions.insert(wid, rect);
                }
            }

            let floating_positions =
                self.floating_positions.workspace_positions(space, active_workspace_id);
            for (window_id, stored_position) in floating_positions {
                if self.floating.is_floating(window_id)
                    && self.virtual_workspace_manager.workspace_for_window(
                        window_store,
                        space,
                        window_id,
                    ) == Some(active_workspace_id)
                {
                    ensure_visible_floating(
                        self,
                        &mut positions,
                        space,
                        active_workspace_id,
                        window_id,
                        Some(stored_position),
                        false,
                        &screen,
                        all_screens,
                        &center_rect,
                        &window_size,
                    );
                }
            }

            let floating_windows = self.active_floating_windows_in_workspace(window_store, space);
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
                    all_screens,
                    &center_rect,
                    &window_size,
                );
            }

            let fullscreen: Vec<(WindowId, FloatingFullscreenKind)> = positions
                .keys()
                .copied()
                .filter_map(|w| self.floating.fullscreen_kind(w).map(|k| (w, k)))
                .collect();
            for (w, kind) in fullscreen {
                let rect = match kind {
                    FloatingFullscreenKind::Full => screen,
                    FloatingFullscreenKind::WithinGaps => {
                        let o = &gaps.outer;
                        CGRect::new(
                            CGPoint::new(screen.origin.x + o.left, screen.origin.y + o.top),
                            CGSize::new(
                                screen.size.width - o.left - o.right,
                                screen.size.height - o.top - o.bottom,
                            ),
                        )
                    }
                };
                positions.insert(w, rect);
            }
        }

        let hidden_windows = self
            .virtual_workspace_manager
            .windows_in_inactive_workspaces(window_store, space);
        for wid in hidden_windows {
            let original_frame = get_window_frame(wid);

            if self.floating.is_floating(wid) {
                if let Some(workspace_id) =
                    self.virtual_workspace_manager.workspace_for_window(window_store, space, wid)
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
                        all_screens,
                        &center_rect,
                        &window_size,
                    );
                }
            }

            let original_size =
                original_frame.map(|f| f.size).unwrap_or_else(|| CGSize::new(500.0, 500.0));
            let reference_frame = original_frame.unwrap_or_else(|| {
                CGRect::new(CGPoint::new(screen.origin.x, screen.origin.y), original_size)
            });
            let app_bundle_id = self.get_app_bundle_id_for_window(wid);
            let hidden_rect = self.virtual_workspace_manager.calculate_hidden_position_multi(
                screen,
                reference_frame,
                HideCorner::BottomRight,
                app_bundle_id.as_deref(),
                all_screens,
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
        self.collect_group_containers_for_space(
            space,
            screen,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
            true,
        )
    }

    pub fn active_workspace_for_space_has_fullscreen(&mut self, space: SpaceId) -> bool {
        let Some((ws_id, layout_id)) = self.workspace_and_layout(space) else {
            return false;
        };
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
        self.collect_group_containers_for_space(
            space,
            screen,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
            false,
        )
    }

    pub fn calculate_layout_for_workspace(
        &self,
        window_store: &WindowStore,
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
                &self.window_layout_constraints,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            );
            for (wid, rect) in tiled_positions {
                positions.insert(wid, rect);
            }
        }

        let floating_positions = self.floating_positions.workspace_positions(space, workspace_id);
        for (window_id, stored_position) in floating_positions {
            if self.floating.is_floating(window_id)
                && self.virtual_workspace_manager.workspace_for_window(
                    window_store,
                    space,
                    window_id,
                ) == Some(workspace_id)
            {
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

    pub fn load(_path: PathBuf) -> anyhow::Result<Self> {
        Ok(Self::new(
            &VirtualWorkspaceSettings::default(),
            &LayoutSettings::default(),
            None,
        ))
    }

    pub fn save(&self, _path: PathBuf) -> std::io::Result<()> { Ok(()) }

    pub fn serialize_to_string(&self) -> String { ron::ser::to_string(&self).unwrap() }

    #[cfg(test)]
    pub(crate) fn selected_window(&mut self, space: SpaceId) -> Option<WindowId> {
        let (ws_id, layout) = self.workspace_and_layout(space)?;
        self.workspace_tree(ws_id).selected_window(layout)
    }

    pub fn handle_virtual_workspace_command(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        command: &LayoutCommand,
    ) -> EventResponse {
        match command {
            LayoutCommand::NextWorkspace(skip_empty) => {
                if let Some(current_workspace) =
                    self.virtual_workspace_manager.active_workspace(space)
                {
                    if let Some(next_workspace) = self.virtual_workspace_manager.next_workspace(
                        window_store,
                        space,
                        current_workspace,
                        *skip_empty,
                    ) {
                        return self.activate_workspace(window_store, space, next_workspace, None);
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::PrevWorkspace(skip_empty) => {
                if let Some(current_workspace) =
                    self.virtual_workspace_manager.active_workspace(space)
                {
                    if let Some(prev_workspace) = self.virtual_workspace_manager.prev_workspace(
                        window_store,
                        space,
                        current_workspace,
                        *skip_empty,
                    ) {
                        return self.activate_workspace(window_store, space, prev_workspace, None);
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::SwitchToWorkspace(workspace_index) => {
                self.switch_to_workspace(window_store, space, *workspace_index, None)
            }
            LayoutCommand::MoveWindowToWorkspace {
                workspace: workspace_index,
                window_id: maybe_id,
            } => {
                let focused_window = if let Some(spec_u32) = maybe_id {
                    match self.virtual_workspace_manager.find_window_by_idx(
                        window_store,
                        space,
                        *spec_u32,
                    ) {
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

                let Some(current_workspace_id) = self
                    .virtual_workspace_manager
                    .workspace_for_window(window_store, op_space, focused_window)
                else {
                    return EventResponse::default();
                };

                if current_workspace_id == target_workspace_id {
                    return EventResponse::default();
                }

                let is_floating = self.floating.is_floating(focused_window);

                if is_floating {
                    self.floating.remove_active_for_window(focused_window);
                } else {
                    self.remove_window_from_all_tiling_trees(focused_window);
                }

                let assigned = self.virtual_workspace_manager.assign_window_to_workspace(
                    window_store,
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
                    self.broadcast_windows_changed(window_store, op_space);
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

                    let remaining_windows = self
                        .virtual_workspace_manager
                        .windows_in_active_workspace(window_store, op_space);
                    if let Some(&new_focus) = remaining_windows.first() {
                        self.broadcast_windows_changed(window_store, op_space);
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

                self.broadcast_windows_changed(window_store, op_space);
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
                    return self.activate_workspace(window_store, space, last_workspace, None);
                }
                EventResponse::default()
            }
            LayoutCommand::SetWorkspaceLayout { workspace, mode } => {
                let Some(workspace_id) = self.workspace_id_for_index(space, *workspace) else {
                    return EventResponse::default();
                };

                if !self.switch_workspace_layout_mode(window_store, space, workspace_id, *mode) {
                    return EventResponse::default();
                }

                let is_active_workspace =
                    self.virtual_workspace_manager.active_workspace(space) == Some(workspace_id);
                let raise_windows = if is_active_workspace {
                    self.windows_in_active_workspace(window_store, space)
                } else {
                    Vec::new()
                };
                self.broadcast_workspace_changed(space);
                self.broadcast_windows_changed(window_store, space);

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

    pub fn switch_to_workspace_with_focus(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_index: usize,
        focus_window: WindowId,
    ) -> EventResponse {
        self.switch_to_workspace(window_store, space, workspace_index, Some(focus_window))
    }

    pub fn virtual_workspace_manager(&self) -> &WorkspaceStore { &self.virtual_workspace_manager }

    pub fn virtual_workspace_manager_mut(&mut self) -> &mut WorkspaceStore {
        &mut self.virtual_workspace_manager
    }

    pub fn active_workspace(&self, space: SpaceId) -> Option<crate::model::VirtualWorkspaceId> {
        self.virtual_workspace_manager.active_workspace(space)
    }

    pub fn assign_window_with_app_info(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        space: SpaceId,
        app_bundle_id: Option<&str>,
        app_name: Option<&str>,
        window_title: Option<&str>,
        ax_role: Option<&str>,
        ax_subrole: Option<&str>,
    ) -> Result<AppRuleResult, crate::model::virtual_workspace::WorkspaceError> {
        let decision = self.app_rules.evaluate(WindowRuleContext {
            app_bundle_id,
            app_name,
            window_title,
            ax_role,
            ax_subrole,
        });
        self.virtual_workspace_manager.apply_app_rule_decision(
            window_store,
            window_id,
            space,
            decision,
        )
    }

    pub fn ensure_active_workspace_info(
        &mut self,
        space: SpaceId,
    ) -> Option<(crate::model::VirtualWorkspaceId, String)> {
        if let Some(workspace_id) = self.virtual_workspace_manager.active_workspace(space) {
            let workspace_name = self
                .workspace_name(space, workspace_id)
                .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));
            return Some((workspace_id, workspace_name));
        }

        let first_workspace = self
            .virtual_workspace_manager
            .list_workspaces(space)
            .first()
            .map(|(workspace_id, _)| *workspace_id)?;

        self.virtual_workspace_manager.set_active_workspace(space, first_workspace);

        let workspace_name = self
            .workspace_name(space, first_workspace)
            .unwrap_or_else(|| format!("Workspace {:?}", first_workspace));

        Some((first_workspace, workspace_name))
    }

    pub fn active_workspace_idx(&self, space: SpaceId) -> Option<u64> {
        self.virtual_workspace_manager.active_workspace_idx(space)
    }

    pub fn move_window_to_space(
        &mut self,
        window_store: &mut WindowStore,
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
            .workspace_for_window(window_store, source_space, window_id)
            .or_else(|| self.virtual_workspace_manager.active_workspace(source_space));

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
            self.floating.remove_active_for_window(window_id);
        } else {
            self.remove_window_from_all_tiling_trees(window_id);
        }

        let assigned = self.virtual_workspace_manager.assign_window_to_workspace(
            window_store,
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

        if was_floating {
            self.floating_positions.remove_window(window_id);
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
            self.broadcast_windows_changed(window_store, source_space);
        }
        self.broadcast_windows_changed(window_store, target_space);

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

    pub fn windows_in_active_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
    ) -> Vec<WindowId> {
        self.virtual_workspace_manager.windows_in_active_workspace(window_store, space)
    }

    pub fn get_workspace_stats(
        &self,
        window_store: &WindowStore,
    ) -> crate::model::virtual_workspace::WorkspaceStats {
        self.virtual_workspace_manager.get_stats(window_store)
    }

    pub fn is_window_floating(&self, window_id: WindowId) -> bool {
        self.floating.is_floating(window_id)
    }

    pub fn store_floating_position(
        &mut self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
        window: WindowId,
        frame: CGRect,
    ) {
        self.floating_positions.store(space, workspace, window, frame);
    }

    pub fn get_floating_position(
        &self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
        window: WindowId,
    ) -> Option<CGRect> {
        self.floating_positions.get(space, workspace, window)
    }

    pub fn workspace_floating_positions(
        &self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
    ) -> Vec<(WindowId, CGRect)> {
        self.floating_positions.workspace_positions(space, workspace)
    }

    pub fn remove_floating_position(&mut self, window: WindowId) {
        self.floating_positions.remove_window(window);
    }

    pub fn transfer_persistent_window_identity(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }

        self.virtual_workspace_manager.transfer_window_identity(from, to);
        self.floating_positions.transfer_window_identity(from, to);
        self.floating.transfer_window_identity(from, to);
        if self.focused_window == Some(from) {
            self.focused_window = Some(to);
        }
    }

    fn update_active_floating_windows(&mut self, window_store: &WindowStore, space: SpaceId) {
        let windows_in_workspace =
            self.virtual_workspace_manager.windows_in_active_workspace(window_store, space);
        self.floating.rebuild_active_for_workspace(space, windows_in_workspace);
    }

    pub fn store_floating_window_positions(
        &mut self,
        space: SpaceId,
        floating_positions: &[(WindowId, CGRect)],
    ) {
        if let Some(workspace) = self.active_workspace(space) {
            for &(window, frame) in floating_positions {
                self.floating_positions.store(space, workspace, window, frame);
            }
        }
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

    fn broadcast_windows_changed(&self, window_store: &WindowStore, space_id: SpaceId) {
        if let Some(ref broadcast_tx) = self.broadcast_tx {
            if let Some((workspace_id, workspace_name)) =
                self.active_workspace_id_and_name(space_id)
            {
                let windows = self
                    .virtual_workspace_manager
                    .windows_in_active_workspace(window_store, space_id)
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

    pub fn debug_log_workspace_stats(&self, window_store: &WindowStore) {
        let stats = self.virtual_workspace_manager.get_stats(window_store);
        info!(
            "Workspace Stats: {} workspaces, {} windows, {} active spaces",
            stats.total_workspaces, stats.total_windows, stats.active_spaces
        );

        for (workspace_id, window_count) in &stats.workspace_window_counts {
            info!("  - '{:?}': {} windows", workspace_id, window_count);
        }
    }

    pub fn debug_log_workspace_state(&self, window_store: &WindowStore, space: SpaceId) {
        if let Some(active_workspace) = self.virtual_workspace_manager.active_workspace(space) {
            if let Some(workspace) =
                self.virtual_workspace_manager.workspace_info(space, active_workspace)
            {
                let active_windows =
                    self.virtual_workspace_manager.windows_in_active_workspace(window_store, space);
                let inactive_windows = self
                    .virtual_workspace_manager
                    .windows_in_inactive_workspaces(window_store, space);

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

    pub fn is_window_in_active_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        window_id: WindowId,
    ) -> bool {
        self.virtual_workspace_manager
            .is_window_in_active_workspace(window_store, space, window_id)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::AssertUnwindSafe;

    use objc2_core_foundation::{CGPoint, CGSize};

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
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(42);
        let visible_spaces = vec![space];
        let visible_space_centers = HashMap::default();

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            engine.handle_command(
                &mut window_store,
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
    fn tiled_membership_sync_does_not_rebalance_other_spaces() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space_a = SpaceId::new(101);
        let space_b = SpaceId::new(202);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let visible_spaces = vec![space_a, space_b];
        let visible_space_centers = HashMap::default();
        let window_a = WindowId::new(1, 1);
        let window_b = WindowId::new(1, 2);
        let window_c = WindowId::new(2, 1);
        let window_info = |wid| (wid, None, None, None, true, CGSize::new(0.0, 0.0), None, None);

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::SpaceExposed(space_a, screen.size),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(
                space_a,
                1,
                vec![window_info(window_a), window_info(window_b)],
                None,
            ),
        );
        let _ = engine.handle_command(
            &mut window_store,
            Some(space_a),
            &visible_spaces,
            &visible_space_centers,
            LayoutCommand::ResizeWindowBy { amount: 0.2 },
        );

        let resized_layout = engine.calculate_layout(
            space_a,
            screen,
            &LayoutSettings::default().gaps,
            0.0,
            Default::default(),
            Default::default(),
        );

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::SpaceExposed(space_b, screen.size),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space_b, 2, vec![window_info(window_c)], None),
        );

        let after_other_space_sync = engine.calculate_layout(
            space_a,
            screen,
            &LayoutSettings::default().gaps,
            0.0,
            Default::default(),
            Default::default(),
        );
        assert_eq!(
            resized_layout, after_other_space_sync,
            "membership sync on one space must not rebalance saved layouts on another space"
        );
    }

    #[test]
    fn window_removed_preserve_floating_keeps_workspace_assignment() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(303);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let pid: pid_t = 42;
        let wid = WindowId::new(pid, 1);
        let window_info = |wid| (wid, None, None, None, true, CGSize::new(0.0, 0.0), None, None);

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space, pid, vec![window_info(wid)], None),
        );

        let assigned_workspace = engine
            .virtual_workspace_manager()
            .workspace_for_window(&window_store, space, wid)
            .expect("window should have a workspace assignment");

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowRemovedPreserveFloating(wid),
        );

        assert_eq!(
            engine
                .virtual_workspace_manager()
                .workspace_for_window(&window_store, space, wid),
            Some(assigned_workspace),
            "temporary layout removal must not clear workspace ownership"
        );

        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, wid));

        assert_eq!(
            engine
                .virtual_workspace_manager()
                .workspace_for_window(&window_store, space, wid),
            Some(assigned_workspace),
            "window should reappear in the same workspace after a temporary hide"
        );
    }

    #[test]
    fn moving_floating_window_to_space_clears_source_floating_state() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let source_space = SpaceId::new(304);
        let target_space = SpaceId::new(305);
        let source_screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 800.0));
        let target_screen = CGRect::new(CGPoint::new(1000.0, 0.0), CGSize::new(1000.0, 800.0));
        let pid: pid_t = 43;
        let wid = WindowId::new(pid, 1);
        let source_position = CGRect::new(CGPoint::new(120.0, 140.0), CGSize::new(260.0, 220.0));
        let window_info = |wid| (wid, None, None, None, true, CGSize::new(0.0, 0.0), None, None);

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::SpaceExposed(source_space, source_screen.size),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::SpaceExposed(target_space, target_screen.size),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(source_space, pid, vec![window_info(wid)], None),
        );

        let source_workspace = engine
            .virtual_workspace_manager()
            .active_workspace(source_space)
            .expect("source workspace");
        let target_workspace = engine
            .virtual_workspace_manager()
            .active_workspace(target_space)
            .expect("target workspace");

        engine.remove_window_from_all_tiling_trees(wid);
        engine.floating.add_floating(wid);
        engine.floating.add_active(source_space, pid, wid);
        engine.store_floating_position(source_space, source_workspace, wid, source_position);

        let response = engine.move_window_to_space(
            &mut window_store,
            source_space,
            target_space,
            target_screen.size,
            wid,
        );

        assert_eq!(response.focus_window, Some(wid));
        assert_eq!(
            engine.virtual_workspace_manager().workspace_for_window(
                &window_store,
                target_space,
                wid
            ),
            Some(target_workspace)
        );
        assert_eq!(
            engine.get_floating_position(source_space, source_workspace, wid),
            None,
            "cross-space moves must clear the source workspace's saved floating frame"
        );
        assert!(
            !engine
                .calculate_layout_for_workspace(
                    &window_store,
                    source_space,
                    source_workspace,
                    source_screen,
                    &LayoutSettings::default().gaps,
                    0.0,
                    Default::default(),
                    Default::default(),
                )
                .into_iter()
                .any(|(window_id, _)| window_id == wid),
            "source workspace layout must not keep emitting the moved floating window"
        );
    }

    #[test]
    fn move_focus_to_uninitialized_adjacent_space_does_not_panic() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let current_space = SpaceId::new(50);
        let adjacent_space = SpaceId::new(51);
        let screen_size = CGSize::new(1920.0, 1080.0);
        let visible_spaces = vec![current_space, adjacent_space];
        let mut visible_space_centers = HashMap::default();
        visible_space_centers.insert(current_space, CGPoint::new(0.0, 0.0));
        visible_space_centers.insert(adjacent_space, CGPoint::new(1920.0, 0.0));

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::SpaceExposed(current_space, screen_size),
        );

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            engine.handle_command(
                &mut window_store,
                Some(current_space),
                &visible_spaces,
                &visible_space_centers,
                LayoutCommand::MoveFocus(Direction::Right),
            )
        }));

        assert!(
            result.is_ok(),
            "cross-space move focus should not panic when adjacent space is not initialized"
        );
    }

    #[test]
    fn update_virtual_workspace_settings_reapplies_workspace_rules() {
        let window_store = WindowStore::default();
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

        engine.update_virtual_workspace_settings(&window_store, &settings);

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
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(8);
        let window_id = WindowId::new(999, 1);

        let _ = engine.virtual_workspace_manager_mut().list_workspaces(space);
        let _ = engine.virtual_workspace_manager_mut().auto_assign_window(
            &mut window_store,
            window_id,
            space,
        );

        let response = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::SetWorkspaceLayout {
                workspace: Some(1),
                mode: LayoutMode::Bsp,
            },
        );

        assert!(response.raise_windows.is_empty());
        assert_eq!(response.focus_window, None);
    }

    #[test]
    fn locked_tiled_windows_stay_within_screen_bounds() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(90);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let pid: pid_t = 4242;

        let locked = WindowId::new(pid, 100);
        let other_a = WindowId::new(pid, 101);
        let other_b = WindowId::new(pid, 102);

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                vec![
                    (
                        locked,
                        None,
                        None,
                        None,
                        false,
                        // Intentionally impossible size for this screen; layout should still keep
                        // tiled results bounded instead of force-applying this at the end.
                        CGSize::new(1600.0, 900.0),
                        None,
                        None,
                    ),
                    (
                        other_a,
                        None,
                        None,
                        None,
                        true,
                        CGSize::new(600.0, 600.0),
                        None,
                        None,
                    ),
                    (
                        other_b,
                        None,
                        None,
                        None,
                        true,
                        CGSize::new(600.0, 600.0),
                        None,
                        None,
                    ),
                ],
                None,
            ),
        );

        let gaps = engine.layout_settings.gaps.effective_for_display(None);
        let positions = engine.calculate_layout_with_virtual_workspaces(
            &window_store,
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
            |_| None,
            &[screen],
        );
        let frames: HashMap<WindowId, CGRect> = positions.into_iter().collect();
        let locked_frame = frames
            .get(&locked)
            .copied()
            .expect("locked tiled window should have a calculated frame");

        let epsilon = 0.5;
        let max_x = screen.origin.x + screen.size.width + epsilon;
        let max_y = screen.origin.y + screen.size.height + epsilon;
        assert!(locked_frame.origin.x >= screen.origin.x - epsilon);
        assert!(locked_frame.origin.y >= screen.origin.y - epsilon);
        assert!(locked_frame.origin.x + locked_frame.size.width <= max_x);
        assert!(locked_frame.origin.y + locked_frame.size.height <= max_y);
    }

    #[test]
    fn repeated_windows_on_screen_update_does_not_rebalance_unchanged_tiled_layout() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(91);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let pid: pid_t = 5150;

        let windows = vec![
            (
                WindowId::new(pid, 1),
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            ),
            (
                WindowId::new(pid, 2),
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            ),
            (
                WindowId::new(pid, 3),
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            ),
        ];

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space, pid, windows.clone(), None),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowFocused(space, WindowId::new(pid, 1)),
        );
        let gaps = engine.layout_settings.gaps.clone();

        let baseline = engine.calculate_layout(
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
        );

        let _ = engine.handle_command(
            &mut window_store,
            Some(space),
            &[space],
            &HashMap::default(),
            LayoutCommand::MoveNode(Direction::Up),
        );

        let modified = engine.calculate_layout(
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
        );
        assert_ne!(baseline, modified);

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space, pid, windows, None),
        );

        assert_eq!(
            engine.calculate_layout(
                space,
                screen,
                &gaps,
                0.0,
                Default::default(),
                Default::default(),
            ),
            modified
        );
    }

    #[test]
    fn partial_windows_on_screen_update_preserves_assigned_tiled_windows() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(94);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let pid: pid_t = 5153;
        let w1 = WindowId::new(pid, 1);
        let w2 = WindowId::new(pid, 2);
        let info = |wid| {
            (
                wid,
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            )
        };

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space, pid, vec![info(w1), info(w2)], None),
        );
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowFocused(space, w1));
        let _ = engine.handle_command(
            &mut window_store,
            Some(space),
            &[space],
            &HashMap::default(),
            LayoutCommand::ResizeWindowBy { amount: 0.2 },
        );

        let gaps = engine.layout_settings.gaps.clone();
        let before = engine.calculate_layout(
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
        );

        // Simulate a discovery snapshot that temporarily omitted w2.
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space, pid, vec![info(w1)], None),
        );

        assert_eq!(
            engine.calculate_layout(
                space,
                screen,
                &gaps,
                0.0,
                Default::default(),
                Default::default(),
            ),
            before,
            "partial discovery must not remove an assigned window or reset its split"
        );
    }

    #[test]
    fn removing_a_window_does_not_rebalance_other_workspaces() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space_a = SpaceId::new(95);
        let space_b = SpaceId::new(96);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let info = |wid| {
            (
                wid,
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            )
        };
        let a1 = WindowId::new(5154, 1);
        let a2 = WindowId::new(5154, 2);
        let b1 = WindowId::new(5155, 1);

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::SpaceExposed(space_a, screen.size),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::SpaceExposed(space_b, screen.size),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space_a, a1.pid, vec![info(a1), info(a2)], None),
        );
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowFocused(space_a, a1));
        let _ = engine.handle_command(
            &mut window_store,
            Some(space_a),
            &[space_a, space_b],
            &HashMap::default(),
            LayoutCommand::ResizeWindowBy { amount: 0.2 },
        );

        let gaps = engine.layout_settings.gaps.clone();
        let before = engine.calculate_layout(
            space_a,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
        );

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space_b, b1.pid, vec![info(b1)], None),
        );
        let _ = window_store.remove_window_assignment(b1);
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowRemoved(b1));

        assert_eq!(
            engine.calculate_layout(
                space_a,
                screen,
                &gaps,
                0.0,
                Default::default(),
                Default::default(),
            ),
            before,
            "removing a window must not rebalance layouts in other workspaces"
        );
    }

    #[test]
    fn removing_unassigned_window_rebalances_only_its_immediate_split() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(97);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let pid: pid_t = 5156;
        let w1 = WindowId::new(pid, 1);
        let w2 = WindowId::new(pid, 2);
        let w3 = WindowId::new(pid, 3);
        let info = |wid| {
            (
                wid,
                None,
                None,
                None,
                true,
                CGSize::new(400.0, 800.0),
                None,
                None,
            )
        };

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                vec![info(w1), info(w2), info(w3)],
                None,
            ),
        );
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowFocused(space, w1));
        let _ = engine.handle_command(
            &mut window_store,
            Some(space),
            &[space],
            &HashMap::default(),
            LayoutCommand::ResizeWindowBy { amount: 0.2 },
        );

        let gaps = engine.layout_settings.gaps.clone();
        let before: HashMap<_, _> = engine
            .calculate_layout(space, screen, &gaps, 0.0, Default::default(), Default::default())
            .into_iter()
            .collect();
        let ratio_before = before[&w1].size.width / before[&w2].size.width;
        assert!(
            (ratio_before - 1.0).abs() > 0.1,
            "test must start with a manual resize"
        );

        // WindowDestroyed can remove store state before layout membership is scrubbed.
        let _ = window_store.remove_window_assignment(w3);
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowRemoved(w3));

        let after: HashMap<_, _> = engine
            .calculate_layout(space, screen, &gaps, 0.0, Default::default(), Default::default())
            .into_iter()
            .collect();
        let ratio_after = after[&w1].size.width / after[&w2].size.width;
        assert!((ratio_after - 1.0).abs() < 0.0001);
    }

    #[test]
    fn removing_unknown_window_does_not_rebalance_layout() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(92);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let pid: pid_t = 5151;

        let windows = vec![
            (
                WindowId::new(pid, 1),
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            ),
            (
                WindowId::new(pid, 2),
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            ),
            (
                WindowId::new(pid, 3),
                None,
                None,
                None,
                true,
                CGSize::new(500.0, 500.0),
                None,
                None,
            ),
        ];

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(space, pid, windows, None),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowFocused(space, WindowId::new(pid, 1)),
        );
        let gaps = engine.layout_settings.gaps.clone();

        let _ = engine.handle_command(
            &mut window_store,
            Some(space),
            &[space],
            &HashMap::default(),
            LayoutCommand::MoveNode(Direction::Up),
        );

        let modified = engine.calculate_layout(
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
        );

        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowRemoved(WindowId::new(9999, 1)),
        );

        assert_eq!(
            engine.calculate_layout(
                space,
                screen,
                &gaps,
                0.0,
                Default::default(),
                Default::default(),
            ),
            modified
        );
    }

    #[test]
    fn duplicate_window_added_is_treated_as_noop_for_active_layout() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(93);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let pid: pid_t = 5152;
        let wid = WindowId::new(pid, 1);

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                vec![(
                    wid,
                    None,
                    None,
                    None,
                    true,
                    CGSize::new(500.0, 500.0),
                    None,
                    None,
                )],
                None,
            ),
        );
        let gaps = engine.layout_settings.gaps.clone();
        let before = engine.calculate_layout(
            space,
            screen,
            &gaps,
            0.0,
            Default::default(),
            Default::default(),
        );

        assert!(!engine.add_window_to_layout(&mut window_store, space, wid));
        assert_eq!(
            engine.calculate_layout(
                space,
                screen,
                &gaps,
                0.0,
                Default::default(),
                Default::default(),
            ),
            before
        );
    }

    #[test]
    fn workspace_switch_only_commits_focus_after_authoritative_commit() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(94);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let pid: pid_t = 5153;
        let wid1 = WindowId::new(pid, 1);
        let wid2 = WindowId::new(pid, 2);

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                vec![
                    (
                        wid1,
                        None,
                        None,
                        None,
                        true,
                        CGSize::new(500.0, 500.0),
                        None,
                        None,
                    ),
                    (
                        wid2,
                        None,
                        None,
                        None,
                        true,
                        CGSize::new(500.0, 500.0),
                        None,
                        None,
                    ),
                ],
                None,
            ),
        );
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowFocused(space, wid1));

        let _ = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::CreateWorkspace,
        );
        let workspaces = engine.virtual_workspace_manager_mut().list_workspaces(space).to_vec();
        let workspace_two = workspaces[1].0;

        let _ = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::MoveWindowToWorkspace {
                workspace: 1,
                window_id: Some(wid2.idx.get()),
            },
        );

        let response = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::SwitchToWorkspace(1),
        );

        assert_eq!(engine.active_workspace(space), Some(workspace_two));
        assert_eq!(response.focus_window, Some(wid2));
        assert_ne!(engine.focused_window, Some(wid2));

        engine.commit_workspace_focus(&mut window_store, space, response.focus_window);

        assert_eq!(engine.focused_window, Some(wid2));
        assert_eq!(
            engine.virtual_workspace_manager().last_focused_window(space, workspace_two),
            Some(wid2)
        );
    }

    #[test]
    fn move_window_to_workspace_updates_authoritative_workspace_membership() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(95);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let pid: pid_t = 6001;
        let wid = WindowId::new(pid, 1);

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                vec![(
                    wid,
                    None,
                    None,
                    None,
                    true,
                    CGSize::new(500.0, 500.0),
                    None,
                    None,
                )],
                None,
            ),
        );

        let _ = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::CreateWorkspace,
        );
        let workspaces = engine.virtual_workspace_manager_mut().list_workspaces(space).to_vec();
        let ws1 = workspaces[0].0;
        let ws2 = workspaces[1].0;

        let _ = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::MoveWindowToWorkspace {
                workspace: 1,
                window_id: Some(wid.idx.get()),
            },
        );

        assert!(
            engine
                .virtual_workspace_manager
                .workspace_windows(&window_store, space, ws1)
                .is_empty(),
            "source workspace must be empty after a same-space workspace move"
        );
        assert_eq!(
            engine.virtual_workspace_manager.workspace_for_window(&window_store, space, wid),
            Some(ws2)
        );
        assert_eq!(
            engine.virtual_workspace_manager.workspace_windows(&window_store, space, ws2),
            vec![wid]
        );
    }
}
