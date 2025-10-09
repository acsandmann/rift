use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;

use objc2_core_foundation::{CGRect, CGSize};
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{Direction, FloatingManager, LayoutId, LayoutSystemKind, WorkspaceLayouts};
use crate::actor::app::{AppInfo, WindowId, pid_t};
use crate::actor::broadcast::{BroadcastEvent, BroadcastSender};
use crate::common::collections::{BTreeSet, HashMap};
use crate::common::config::LayoutSettings;
use crate::layout_engine::LayoutSystem;
use crate::model::{VirtualWorkspaceId, VirtualWorkspaceManager};
use crate::sys::screen::SpaceId;

#[derive(Debug, Clone)]
pub struct GroupContainerInfo {
    pub node_id: crate::model::tree::NodeId,
    pub container_kind: super::LayoutKind,
    pub frame: CGRect,
    pub total_count: usize,
    pub selected_index: usize,
}

#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutCommand {
    NextWindow,
    PrevWindow,
    MoveFocus(#[serde(rename = "direction")] Direction),
    Ascend,
    Descend,
    MoveNode(Direction),

    JoinWindow(Direction),
    StackWindows,
    UnstackWindows,
    UnjoinWindows,
    ToggleFocusFloating,
    ToggleWindowFloating,
    ToggleFullscreen,

    ResizeWindowGrow,
    ResizeWindowShrink,

    NextWorkspace(Option<bool>),
    PrevWorkspace(Option<bool>),
    SwitchToWorkspace(usize),
    MoveWindowToWorkspace(usize),
    CreateWorkspace,
    SwitchToLastWorkspace,
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
    WindowFocused(Vec<SpaceId>, WindowId),
    WindowResized {
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screens: Vec<(SpaceId, CGRect)>,
    },
    SpaceExposed(SpaceId, CGSize),
}

#[must_use]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventResponse {
    pub raise_windows: Vec<WindowId>,
    pub focus_window: Option<WindowId>,
}

#[derive(Serialize, Deserialize)]
pub struct LayoutEngine {
    tree: LayoutSystemKind,
    workspace_layouts: WorkspaceLayouts,
    floating: FloatingManager,
    #[serde(skip)]
    focused_window: Option<WindowId>,
    virtual_workspace_manager: VirtualWorkspaceManager,
    #[serde(skip)]
    layout_settings: LayoutSettings,
    #[serde(skip)]
    broadcast_tx: Option<BroadcastSender>,
}

impl LayoutEngine {
    pub fn set_layout_settings(&mut self, settings: &LayoutSettings) {
        self.layout_settings = settings.clone();
    }

    fn active_floating_windows_flat(&self, space: SpaceId) -> Vec<WindowId> {
        self.floating.active_flat(space)
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
        let mut focus_window =
            self.virtual_workspace_manager.last_focused_window(space, workspace_id);

        if focus_window.is_none() {
            if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
                let selected = self.tree.selected_window(layout);
                let visible = self.tree.visible_windows_in_layout(layout);
                focus_window = selected.or_else(|| visible.first().copied());
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
                let _ = self.tree.select_window(layout, wid);
            }
        } else {
            self.focused_window = None;
            self.virtual_workspace_manager
                .set_last_focused_window(space, workspace_id, None);
        }

        EventResponse {
            focus_window,
            raise_windows: vec![],
        }
    }

    #[allow(dead_code)]
    fn resize_selection(&mut self, layout: LayoutId, resize_amount: f64) {
        self.tree.resize_selection_by(layout, resize_amount);
    }

    fn move_focus_internal(
        &mut self,
        space: SpaceId,
        visible_spaces: &[SpaceId],
        direction: Direction,
        is_floating: bool,
    ) -> EventResponse {
        let layout = self.layout(space);

        let next_space = |direction| {
            if visible_spaces.len() <= 1 {
                return None;
            }
            let idx = visible_spaces.iter().enumerate().find(|(_, s)| **s == space)?.0;
            let idx = match direction {
                Direction::Left | Direction::Up => idx as i32 - 1,
                Direction::Right | Direction::Down => idx as i32 + 1,
            };
            let idx = idx.rem_euclid(visible_spaces.len() as i32);
            Some(visible_spaces[idx as usize])
        };

        if is_floating {
            let floating_windows = self.active_floating_windows_flat(space);
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
                            return EventResponse {
                                focus_window,
                                raise_windows: vec![],
                            };
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

            let tiled_windows = self.tree.visible_windows_in_layout(layout);
            debug!("Trying tiled windows: {:?}", tiled_windows);
            if !tiled_windows.is_empty() {
                let focus_window = tiled_windows.first().copied();
                if let Some(wid) = focus_window {
                    let _ = self.tree.select_window(layout, wid);
                }
                debug!("Focusing tiled window: {:?}", focus_window);
                return EventResponse {
                    focus_window,
                    raise_windows: tiled_windows,
                };
            }

            debug!("No windows to navigate to, returning default");
            return EventResponse::default();
        }

        let (focus_window, raise_windows) = self.tree.move_focus(layout, direction);
        if focus_window.is_some() {
            EventResponse { focus_window, raise_windows }
        } else {
            if let Some(new_space) = next_space(direction) {
                let new_layout = self.layout(new_space);
                let windows_in_new_space = self.tree.visible_windows_in_layout(new_layout);
                if let Some(&first_window) = windows_in_new_space.first() {
                    let _ = self.tree.select_window(new_layout, first_window);
                    return EventResponse {
                        focus_window: Some(first_window),
                        raise_windows: windows_in_new_space,
                    };
                }
            }

            let floating_windows = self.active_floating_windows_flat(space);

            if let Some(&first_floating) = floating_windows.first() {
                let focus_window = Some(first_floating);
                return EventResponse {
                    focus_window,
                    raise_windows: vec![],
                };
            }

            EventResponse::default()
        }
    }

    fn spaces_with_window(&self, wid: WindowId) -> Vec<SpaceId> {
        let mut spaces: BTreeSet<SpaceId> = BTreeSet::new();
        for space in self.workspace_layouts.spaces() {
            if let Some(ws_id) = self.virtual_workspace_manager.active_workspace(space) {
                if let Some(layout) = self.workspace_layouts.active(space, ws_id) {
                    if self.tree.contains_window(layout, wid) {
                        spaces.insert(space);
                        continue;
                    }
                }
            }

            if self.floating.active_flat(space).contains(&wid) {
                spaces.insert(space);
            }
        }
        spaces.into_iter().collect()
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

    pub fn new(
        virtual_workspace_config: &crate::common::config::VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
        broadcast_tx: Option<BroadcastSender>,
    ) -> Self {
        let virtual_workspace_manager =
            VirtualWorkspaceManager::new_with_config(virtual_workspace_config);

        let tree = match layout_settings.mode {
            crate::common::config::LayoutMode::Traditional => LayoutSystemKind::Traditional(
                crate::layout_engine::TraditionalLayoutSystem::default(),
            ),
            crate::common::config::LayoutMode::Bsp => {
                LayoutSystemKind::Bsp(crate::layout_engine::BspLayoutSystem::default())
            }
        };

        LayoutEngine {
            tree,
            workspace_layouts: WorkspaceLayouts::default(),
            floating: FloatingManager::new(),
            focused_window: None,
            virtual_workspace_manager,
            layout_settings: layout_settings.clone(),
            broadcast_tx,
        }
    }

    pub fn debug_tree(&self, space: SpaceId) { self.debug_tree_desc(space, "", false); }

    pub fn debug_tree_desc(&self, space: SpaceId, desc: &'static str, print: bool) {
        if let Some(workspace_id) = self.virtual_workspace_manager.active_workspace(space) {
            if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
                if print {
                    println!("Tree {desc}\n{}", self.tree.draw_tree(layout).trim());
                } else {
                    debug!("Tree {desc}\n{}", self.tree.draw_tree(layout).trim());
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
                self.workspace_layouts.ensure_active_for_space(
                    space,
                    size,
                    workspaces.into_iter().map(|(id, _)| id),
                    &mut self.tree,
                );
            }
            LayoutEvent::WindowsOnScreenUpdated(space, pid, mut windows_with_titles, app_info) => {
                self.debug_tree(space);
                self.floating.clear_active_for_app(space, pid);
                let mut floating_active_accum = Vec::new();
                windows_with_titles.retain(|(wid, _, _, _)| {
                    let is_floating = self.floating.is_floating(*wid);
                    if is_floating {
                        floating_active_accum.push(*wid);
                    }
                    !is_floating
                });
                for wid in floating_active_accum {
                    self.floating.add_active(space, pid, wid);
                }

                let mut windows_by_workspace: HashMap<
                    crate::model::VirtualWorkspaceId,
                    Vec<WindowId>,
                > = HashMap::default();

                for (wid, title_opt, ax_role_opt, ax_subrole_opt) in windows_with_titles {
                    let assigned_workspace = if let Some(workspace_id) =
                        self.virtual_workspace_manager.workspace_for_window(space, wid)
                    {
                        workspace_id
                    } else if let Some(ref app_info) = app_info {
                        match self.virtual_workspace_manager.assign_window_with_app_info(
                            wid,
                            space,
                            app_info.bundle_id.as_deref(),
                            app_info.localized_name.as_deref(),
                            title_opt.as_deref(),
                            ax_role_opt.as_deref(),
                            ax_subrole_opt.as_deref(),
                        ) {
                            Ok((workspace_id, should_float)) => {
                                if should_float {
                                    self.floating.add_floating(wid);
                                    self.floating.add_active(space, pid, wid);
                                }
                                workspace_id
                            }
                            Err(_) => self
                                .virtual_workspace_manager
                                .auto_assign_window(wid, space)
                                .unwrap_or_else(|_| {
                                    self.virtual_workspace_manager
                                        .active_workspace(space)
                                        .expect("No active workspace available")
                                }),
                        }
                    } else {
                        self.virtual_workspace_manager
                            .auto_assign_window(wid, space)
                            .unwrap_or_else(|_| {
                                self.virtual_workspace_manager
                                    .active_workspace(space)
                                    .expect("No active workspace available")
                            })
                    };
                    windows_by_workspace.entry(assigned_workspace).or_default().push(wid);
                }

                let mut tiled_by_workspace: HashMap<
                    crate::model::VirtualWorkspaceId,
                    Vec<WindowId>,
                > = HashMap::default();
                for (workspace_id, workspace_windows) in windows_by_workspace {
                    let tiled: Vec<WindowId> = workspace_windows
                        .into_iter()
                        .filter(|wid| !self.floating.is_floating(*wid))
                        .collect();
                    tiled_by_workspace.insert(workspace_id, tiled);
                }

                let total_tiled_count: usize = tiled_by_workspace.values().map(|v| v.len()).sum();

                for (ws_id, layout) in self.workspace_layouts.active_layouts_for_space(space) {
                    let desired = tiled_by_workspace.get(&ws_id).cloned().unwrap_or_default();

                    if desired.is_empty() && total_tiled_count == 0 {
                        if self.tree.has_windows_for_app(layout, pid) {
                            continue;
                        }
                    }

                    self.tree.set_windows_for_app(layout, pid, desired);
                }

                self.broadcast_windows_changed(space);

                self.rebalance_all_layouts();
            }
            LayoutEvent::AppClosed(pid) => {
                self.tree.remove_windows_for_app(pid);
                self.floating.remove_all_for_pid(pid);

                self.virtual_workspace_manager.remove_windows_for_app(pid);
                self.virtual_workspace_manager.remove_app_floating_positions(pid);
            }
            LayoutEvent::WindowAdded(space, wid) => {
                self.debug_tree(space);

                let assigned_workspace =
                    match self.virtual_workspace_manager.auto_assign_window(wid, space) {
                        Ok(workspace_id) => workspace_id,
                        Err(e) => {
                            tracing::warn!("Failed to auto-assign window to workspace: {:?}", e);
                            self.virtual_workspace_manager
                                .active_workspace(space)
                                .expect("No active workspace available")
                        }
                    };

                let should_be_floating = self.floating.is_floating(wid);

                if !should_be_floating {
                    if let Some(layout) = self.workspace_layouts.active(space, assigned_workspace) {
                        self.tree.add_window_after_selection(layout, wid);
                    } else {
                        tracing::error!("No layout found for workspace {:?}", assigned_workspace);
                    }
                    tracing::debug!("Added tiled window {:?} to layout tree", wid);
                } else {
                    self.floating.add_active(space, wid.pid, wid);
                    tracing::debug!("Window {:?} is floating, excluded from layout tree", wid);
                }

                self.broadcast_windows_changed(space);
            }
            LayoutEvent::WindowRemoved(wid) => {
                let affected_spaces: Vec<SpaceId> = self.spaces_with_window(wid);

                self.tree.remove_window(wid);

                self.floating.remove_floating(wid);

                self.virtual_workspace_manager.remove_window(wid);

                self.virtual_workspace_manager.remove_floating_position(wid);

                if self.focused_window == Some(wid) {
                    self.focused_window = None;
                }

                for space in affected_spaces {
                    self.broadcast_windows_changed(space);
                }

                self.rebalance_all_layouts();
            }
            LayoutEvent::WindowFocused(spaces, wid) => {
                self.focused_window = Some(wid);
                if self.floating.is_floating(wid) {
                    self.floating.set_last_focus(Some(wid));
                } else {
                    for space in spaces {
                        let layout = self.layout(space);
                        let _ = self.tree.select_window(layout, wid);
                        if let Some(workspace_id) =
                            self.virtual_workspace_manager.active_workspace(space)
                        {
                            self.virtual_workspace_manager.set_last_focused_window(
                                space,
                                workspace_id,
                                Some(wid),
                            );
                        }
                    }
                }
            }
            LayoutEvent::WindowResized {
                wid,
                old_frame,
                new_frame,
                screens,
            } => {
                for (space, screen) in screens {
                    let layout = self.layout(space);
                    self.tree.on_window_resized(layout, wid, old_frame, new_frame, screen);
                }
            }
        }
        EventResponse::default()
    }

    pub fn handle_command(
        &mut self,
        space: Option<SpaceId>,
        visible_spaces: &[SpaceId],
        command: LayoutCommand,
    ) -> EventResponse {
        if let Some(space) = space {
            let layout = self.layout(space);
            debug!("Tree:\n{}", self.tree.draw_tree(layout).trim());
            debug!(selection_window = ?self.tree.selected_window(layout));
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
                        self.tree.add_window_after_selection(layout, wid);
                        tracing::debug!(
                            "Re-added floating window {:?} to tiling tree in workspace {:?}",
                            wid,
                            assigned_workspace
                        );
                    }

                    self.floating.remove_active(space, wid.pid, wid);
                }
                self.floating.remove_floating(wid);
                self.floating.set_last_focus(None);
            } else {
                if let Some(space) = space {
                    self.floating.add_active(space, wid.pid, wid);
                }
                self.tree.remove_window(wid);
                self.floating.add_floating(wid);
                self.floating.set_last_focus(Some(wid));
                tracing::debug!("Removed window {:?} from tiling tree, now floating", wid);
            }
            return EventResponse::default();
        }

        let Some(space) = space else {
            return EventResponse::default();
        };
        let workspace_id = self
            .virtual_workspace_manager
            .active_workspace(space)
            .expect("No active workspace for space");
        let layout = self.workspace_layouts.active_layout(space, workspace_id);

        if let LayoutCommand::ToggleFocusFloating = &command {
            if is_floating {
                let selection = self.tree.selected_window(layout);
                let mut raise_windows = self.tree.visible_windows_in_layout(layout);
                let focus_window = selection.or_else(|| raise_windows.pop());
                return EventResponse { raise_windows, focus_window };
            } else {
                let floating_windows: Vec<WindowId> =
                    self.active_floating_windows_in_workspace(space);
                let mut raise_windows: Vec<_> = floating_windows
                    .iter()
                    .copied()
                    .filter(|wid| Some(*wid) != self.floating.last_focus())
                    .collect();
                let focus_window = self.floating.last_focus().or_else(|| raise_windows.pop());
                return EventResponse { raise_windows, focus_window };
            }
        }

        let next_space = |direction| {
            if visible_spaces.len() <= 1 {
                return None;
            }
            let idx = visible_spaces.iter().enumerate().find(|(_, s)| **s == space)?.0;
            let idx = match direction {
                Direction::Left | Direction::Up => idx as i32 - 1,
                Direction::Right | Direction::Down => idx as i32 + 1,
            };
            let idx = idx.rem_euclid(visible_spaces.len() as i32);
            Some(visible_spaces[idx as usize])
        };

        match command {
            LayoutCommand::ToggleWindowFloating => unreachable!(),
            LayoutCommand::ToggleFocusFloating => unreachable!(),

            LayoutCommand::NextWindow => {
                self.move_focus_internal(space, visible_spaces, Direction::Left, is_floating)
            }
            LayoutCommand::PrevWindow => {
                self.move_focus_internal(space, visible_spaces, Direction::Right, is_floating)
            }
            LayoutCommand::MoveFocus(direction) => {
                debug!(
                    "MoveFocus command received, direction: {:?}, is_floating: {}",
                    direction, is_floating
                );
                if is_floating {
                    return self.move_focus_internal(space, visible_spaces, direction, true);
                } else {
                    return self.move_focus_internal(space, visible_spaces, direction, false);
                }
            }
            LayoutCommand::Ascend => {
                if is_floating {
                    return EventResponse::default();
                }
                self.tree.ascend_selection(layout);
                EventResponse::default()
            }
            LayoutCommand::Descend => {
                self.tree.descend_selection(layout);
                EventResponse::default()
            }
            LayoutCommand::MoveNode(direction) => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                if !self.tree.move_selection(layout, direction) {
                    if let Some(new_space) = next_space(direction) {
                        let new_layout = self.layout(new_space);
                        self.tree.move_selection_to_layout_after_selection(layout, new_layout);
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::ToggleFullscreen => {
                let raise_windows = self.tree.toggle_fullscreen_of_selection(layout);
                if raise_windows.is_empty() {
                    EventResponse::default()
                } else {
                    EventResponse {
                        raise_windows,
                        focus_window: None,
                    }
                }
            }
            // handled by upper reactor
            LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::MoveWindowToWorkspace(_)
            | LayoutCommand::CreateWorkspace
            | LayoutCommand::SwitchToLastWorkspace => EventResponse::default(),
            LayoutCommand::JoinWindow(direction) => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                self.tree.join_selection_with_direction(layout, direction);
                EventResponse::default()
            }
            LayoutCommand::StackWindows => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let stacked_windows = self.tree.apply_stacking_to_parent_of_selection(layout);
                EventResponse {
                    raise_windows: stacked_windows,
                    focus_window: None,
                }
            }
            LayoutCommand::UnstackWindows => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let unstacked_windows = self.tree.unstack_parent_of_selection(layout);
                EventResponse {
                    raise_windows: unstacked_windows,
                    focus_window: None,
                }
            }
            LayoutCommand::UnjoinWindows => {
                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                self.tree.unjoin_selection(layout);
                EventResponse::default()
            }
            LayoutCommand::ResizeWindowGrow => {
                if is_floating {
                    return EventResponse::default();
                }

                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let resize_amount = 0.05;
                self.tree.resize_selection_by(layout, resize_amount);
                EventResponse::default()
            }
            LayoutCommand::ResizeWindowShrink => {
                if is_floating {
                    return EventResponse::default();
                }

                self.workspace_layouts.mark_last_saved(space, workspace_id, layout);
                let resize_amount = -0.05;
                self.tree.resize_selection_by(layout, resize_amount);
                EventResponse::default()
            }
        }
    }

    pub fn calculate_layout(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let layout = self.layout(space);
        self.tree.calculate_layout(
            layout,
            screen,
            self.layout_settings.stack.stack_offset,
            &self.layout_settings.gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        )
    }

    pub fn calculate_layout_with_virtual_workspaces<F>(
        &self,
        space: SpaceId,
        screen: CGRect,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
        get_window_size: F,
    ) -> Vec<(WindowId, CGRect)>
    where
        F: Fn(WindowId) -> CGSize,
    {
        use crate::model::HideCorner;

        let mut positions = HashMap::default();

        if let Some(active_workspace_id) = self.virtual_workspace_manager.active_workspace(space) {
            if let Some(layout) = self.workspace_layouts.active(space, active_workspace_id) {
                let tiled_positions = self.tree.calculate_layout(
                    layout,
                    screen,
                    self.layout_settings.stack.stack_offset,
                    &self.layout_settings.gaps,
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
                    positions.insert(window_id, stored_position);
                }
            }
        }

        let hidden_windows = self.virtual_workspace_manager.windows_in_inactive_workspaces(space);
        for (index, wid) in hidden_windows.into_iter().enumerate() {
            let original_size = get_window_size(wid);
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
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<GroupContainerInfo> {
        let layout_id = self.layout(space);
        match &self.tree {
            LayoutSystemKind::Traditional(s) => s.collect_group_containers_in_selection_path(
                layout_id,
                screen,
                self.layout_settings.stack.stack_offset,
                &self.layout_settings.gaps,
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
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let mut positions = HashMap::default();

        if let Some(layout) = self.workspace_layouts.active(space, workspace_id) {
            let tiled_positions = self.tree.calculate_layout(
                layout,
                screen,
                self.layout_settings.stack.stack_offset,
                &self.layout_settings.gaps,
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

    fn layout(&mut self, space: SpaceId) -> LayoutId {
        let workspace_id = match self.virtual_workspace_manager.active_workspace(space) {
            Some(ws) => ws,
            None => {
                let list = self.virtual_workspace_manager_mut().list_workspaces(space);
                if let Some((first_id, _)) = list.first() {
                    *first_id
                } else {
                    let _ = self.virtual_workspace_manager.active_workspace(space);
                    self.virtual_workspace_manager_mut()
                        .list_workspaces(space)
                        .first()
                        .map(|(id, _)| *id)
                        .expect("No active workspace for space and none could be created")
                }
            }
        };
        self.workspace_layouts.active_layout(space, workspace_id)
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
        let layout = self.layout(space);
        self.tree.selected_window(layout)
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
            LayoutCommand::MoveWindowToWorkspace(workspace_index) => {
                let focused_window = match self.focused_window {
                    Some(wid) => wid,
                    None => return EventResponse::default(),
                };

                let inferred_spaces = self.spaces_with_window(focused_window);
                let op_space = if inferred_spaces.contains(&space) {
                    space
                } else {
                    match inferred_spaces.first().copied() {
                        Some(s) => s,
                        None => space, // NOTE: this should rarely if not ever occur
                    }
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
                    self.tree.remove_window(focused_window);
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
                        self.tree.add_window_after_selection(prev_layout, focused_window);
                    }
                    return EventResponse::default();
                }

                if !is_floating {
                    if let Some(target_layout) =
                        self.workspace_layouts.active(op_space, target_workspace_id)
                    {
                        self.tree.add_window_after_selection(target_layout, focused_window);
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
                        tracing::warn!("Failed to create new workspace: {:?}", e);
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
                let _ = broadcast_tx.send(BroadcastEvent::WorkspaceChanged {
                    workspace_id: active_workspace_id,
                    workspace_name: active_workspace_name.clone(),
                    space_id,
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

                let event = BroadcastEvent::WindowsChanged {
                    workspace_id,
                    workspace_name,
                    windows,
                };

                let _ = broadcast_tx.send(event);
            }
        }
    }

    pub fn debug_log_workspace_stats(&self) {
        let stats = self.virtual_workspace_manager.get_stats();
        tracing::info!(
            "Workspace Stats: {} workspaces, {} windows, {} active spaces",
            stats.total_workspaces,
            stats.total_windows,
            stats.active_spaces
        );

        for (workspace_id, window_count) in &stats.workspace_window_counts {
            tracing::info!("  - '{:?}': {} windows", workspace_id, window_count);
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

                tracing::info!(
                    "Space {:?}: Active workspace '{}' with {} windows",
                    space,
                    workspace.name,
                    active_windows.len()
                );
                tracing::info!("  Active windows: {:?}", active_windows);
                tracing::info!("  Inactive windows: {} total", inactive_windows.len());
                if !inactive_windows.is_empty() {
                    tracing::info!("  Inactive window IDs: {:?}", inactive_windows);
                }
            }
        } else {
            tracing::warn!("Space {:?}: No active workspace set", space);
        }
    }

    fn rebalance_all_layouts(&mut self) {
        self.workspace_layouts.for_each_active(|layout| self.tree.rebalance(layout));
    }

    pub fn is_window_in_active_workspace(&self, space: SpaceId, window_id: WindowId) -> bool {
        let Some(active_workspace_id) = self.virtual_workspace_manager.active_workspace(space)
        else {
            return true;
        };

        if let Some(workspace_id) =
            self.virtual_workspace_manager.workspace_for_window(space, window_id)
        {
            workspace_id == active_workspace_id
        } else {
            true
        }
    }
}
