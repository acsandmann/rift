use std::sync::Arc;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use rift_protocol::{FloatingWindowSize, FloatingWindowSizePreset, ToggleWindowFloatingOptions};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::{Direction, FloatingManager, LayoutId, LayoutSystemKind, ResizeOrientation};
#[cfg(test)]
use crate::actor::app::AppInfo;
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{LayoutMode, LayoutSettings, WorkspaceSelector};
use crate::layout_engine::LayoutSystem;
use crate::layout_engine::floating::FloatingFullscreenKind;
use crate::layout_engine::systems::{
    Focus, GroupEdit, LayoutEdit, WindowChange, WindowLayoutConstraints, WindowQuery,
};
use crate::model::app_rules::{AppRuleFrame, AppRuleOutcome, AppRuleResize, AppRuleWorkspaceFocus};
use crate::model::virtual_workspace::{VirtualWorkspaceId, WorkspaceContext, WorkspaceStore};
use crate::model::{
    AppRuleEffects, AppRuleEngine, AppRuleResult, FloatingPositionStore, WindowRuleContext,
    WindowStore, WindowVisibility,
};
use crate::sys::screen::SpaceId;

mod persistence;

use persistence::PersistenceState;
pub use persistence::{RestoreReport, RestoreRequest, RestoreScope, RestoreSource};
pub use rift_protocol::LayoutCommand;

const SMART_FLOATING_WIDTH_RATIO: f64 = 0.8;
const SMART_FLOATING_HEIGHT_RATIO: f64 = 0.93;

#[derive(Clone, Copy)]
pub(crate) struct DropPreview {
    pub frame: CGRect,
    pub same_slot: bool,
}

impl DropPreview {
    pub(crate) fn action(
        self,
        action: crate::layout_engine::WindowDropAction,
        center: crate::common::config::MouseDropAction,
    ) -> crate::layout_engine::WindowDropAction {
        if self.same_slot && matches!(action, crate::layout_engine::WindowDropAction::Insert(_)) {
            center.into()
        } else {
            action
        }
    }
}

fn interval_gap(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> f64 {
    (b_min - a_max).max(a_min - b_max).max(0.0)
}

fn requested_floating_frame(
    mut frame: CGRect,
    screen: CGRect,
    center: bool,
    size: Option<FloatingWindowSize>,
) -> CGRect {
    if let Some(size) = size {
        frame.size = match size {
            FloatingWindowSize::Dimensions { w, h } => CGSize::new(w, h),
            FloatingWindowSize::Preset(FloatingWindowSizePreset::Smart) => CGSize::new(
                screen.size.width * SMART_FLOATING_WIDTH_RATIO,
                screen.size.height * SMART_FLOATING_HEIGHT_RATIO,
            ),
        };
    }
    if center {
        frame.origin = CGPoint::new(
            screen.mid().x - frame.size.width / 2.0,
            screen.mid().y - frame.size.height / 2.0,
        );
    }
    frame
}

#[derive(Debug, Clone)]
pub struct GroupContainerInfo {
    pub node_id: crate::model::tree::NodeId,
    pub container_kind: super::LayoutKind,
    pub frame: CGRect,
    pub total_count: usize,
    pub selected_index: usize,
    pub window_ids: Vec<crate::actor::app::WindowId>,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct WindowLayoutInfo {
    pub(crate) window_id: WindowId,
    pub(crate) title: Option<String>,
    pub(crate) ax_role: Option<String>,
    pub(crate) ax_subrole: Option<String>,
    pub(crate) is_resizable: bool,
    pub(crate) current_size: CGSize,
    pub(crate) min_size: Option<CGSize>,
    pub(crate) max_size: Option<CGSize>,
}

#[derive(Debug, Clone)]
pub struct ResolvedWindow {
    pub(crate) info: WindowLayoutInfo,
    pub(crate) effects: AppRuleEffects,
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum LayoutEvent {
    /// Admit or update one window without making claims about any sibling window.
    WindowObserved(SpaceId, ResolvedWindow),
    #[cfg(test)]
    #[allow(private_interfaces)]
    TestWindowsObserved(SpaceId, Vec<WindowLayoutInfo>, Option<AppInfo>),
    AppClosed(pid_t),
    WindowAdded(SpaceId, WindowId),
    WindowRemoved(WindowId),
    WindowRemovedPreserveFloating(WindowId),
    /// Suspend projection only; lifecycle state is owned by the window store.
    WindowSuspended(WindowId),
    WindowFocused(SpaceId, WindowId),
    WindowResized {
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screens: Arc<[(SpaceId, CGRect, Option<String>)]>,
    },
    SpaceExposed(SpaceId, CGSize),
}

#[cfg(test)]
impl LayoutEvent {
    fn windows_observed(
        space: SpaceId,
        _pid: pid_t,
        windows: Vec<WindowLayoutInfo>,
        app: Option<AppInfo>,
    ) -> Self {
        Self::TestWindowsObserved(space, windows, app)
    }
}

#[must_use]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventResponse {
    /// Whether handling the request changed layout-engine state.
    #[serde(default)]
    pub changed: bool,
    pub raise_windows: Vec<WindowId>,
    pub focus_window: Option<WindowId>,
    pub boundary_hit: Option<Direction>,
    /// Destination presentation context, resolved once by the command.
    #[serde(default)]
    pub workspace_space: Option<SpaceId>,
    /// Logical relocations needing native placement before arrangement.
    #[serde(default)]
    pub transferred_windows: Vec<WindowId>,
    /// Space edited by a command whose target may differ from the command display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_space: Option<SpaceId>,
}

#[must_use]
pub struct LayoutEventOutcome {
    pub response: EventResponse,
    pub(crate) app_rules: AppRuleOutcome,
}

impl std::ops::Deref for LayoutEventOutcome {
    type Target = EventResponse;

    fn deref(&self) -> &Self::Target { &self.response }
}

#[derive(Default)]
struct LayoutConstraints {
    windows: HashMap<WindowId, WindowLayoutConstraints>,
    revision: u64,
}

impl LayoutConstraints {
    fn insert(&mut self, window: WindowId, constraints: WindowLayoutConstraints) {
        if self.windows.insert(window, constraints) != Some(constraints) {
            self.revision += 1;
        }
    }

    fn remove(&mut self, window: &WindowId) -> Option<WindowLayoutConstraints> {
        let removed = self.windows.remove(window);
        self.revision += u64::from(removed.is_some());
        removed
    }

    fn set_width(&mut self, window: WindowId, width: f64) {
        if let Some(constraints) = self.windows.get_mut(&window)
            && constraints.locked_width != width
        {
            constraints.locked_width = width;
            self.revision += 1;
        }
    }

    fn prepare(
        &self,
        system: &mut LayoutSystemKind,
        layout: LayoutId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) {
        if let LayoutSystemKind::Scrolling(scrolling) = system {
            scrolling.prepare(layout, screen, &self.windows, Some(self.revision), gaps);
        }
    }
}

enum WindowRelocation {
    Workspace,
    Observed,
    Drop(Option<LayoutSystemKind>),
    Directional(Direction),
}

pub struct LayoutEngine {
    floating: FloatingManager,
    floating_positions: FloatingPositionStore,
    app_rules: AppRuleEngine,
    focused_window: Option<WindowId>,
    window_layout_constraints: LayoutConstraints,
    workspaces: WorkspaceStore,
    layout_settings: LayoutSettings,
    space_display_map: HashMap<SpaceId, Option<String>>,
    display_last_space: HashMap<String, SpaceId>,
    persistence: PersistenceState,
    /// Legacy strip commands report boundary excess; workspace arbitration lives here.
    scroll_boundary: Option<((VirtualWorkspaceId, LayoutId, Direction), f64)>,
    // Display-relocation recovery only; temporary absence never snapshots topology.
    native_layouts:
        HashMap<VirtualWorkspaceId, (LayoutSystemKind, super::workspaces::WorkspaceLayoutState)>,
    native_reconcile_batch: Option<HashSet<VirtualWorkspaceId>>,
    // Windows a display departure moved to another display, by where they came from.
    evicted: HashMap<WindowId, EvictedWindow>,
}

/// Where a display departure moved a window from, so the display's return can put it back
/// at the same workspace position instead of the active one.
#[derive(Debug)]
struct EvictedWindow {
    display: String,
    ordinal: usize,
    /// The assignment the departure gave the window. If the window has since been placed
    /// anywhere else, the user (or another change) decided and the record no longer applies.
    landed: VirtualWorkspaceId,
}

pub(crate) struct WorkspaceLayoutQuerySnapshot {
    pub workspace_id: VirtualWorkspaceId,
    pub workspace_index: usize,
    pub is_active: bool,
    pub mode: LayoutMode,
    pub selected_window: Option<WindowId>,
    pub container_tree: rift_protocol::ContainerTreeNode,
}

impl LayoutEngine {
    pub fn focused_window(&self) -> Option<WindowId> { self.focused_window }

    pub(crate) fn apply_window_drop(
        &mut self,
        request: crate::layout_engine::WindowDropRequest,
    ) -> bool {
        if self.active_layout_mode_at(request.space) == LayoutMode::Floating {
            return false;
        }
        let Some((workspace, layout)) = self.workspaces.active_layout_for_space(request.space)
        else {
            return false;
        };
        self.forget_native_layout_for_window_id(request.source);
        self.native_layouts.remove(&workspace);
        self.workspaces[workspace]
            .layout_system
            .edit(layout, LayoutEdit::Drop {
                source: request.source,
                target: request.target,
                action: request.action,
            })
            .changed()
    }

    /// Validate a prospective drop on a workspace-local tree before touching membership.
    /// The regular relocation and target-drop primitives perform the committed operation.
    pub(crate) fn relocate_window_with_drop(
        &mut self,
        windows: &mut WindowStore,
        drop: &crate::layout_engine::WorkspaceDropRequest,
    ) -> bool {
        let Some(source) = windows.workspace_for_window(drop.window) else {
            return false;
        };
        if !self.workspaces.workspaces.contains_key(source) {
            return false;
        }
        let destination = drop.workspace;
        let Some(space) = self.workspaces.workspaces.get(destination).map(|ws| ws.space) else {
            return false;
        };
        let floating = self.is_window_floating(drop.window);
        let Some(layout) = self.workspaces.active_layout(space, destination) else {
            return false;
        };
        let different = source != destination;
        let prospective = if floating {
            let Some(frame) = drop.frame else { return false };
            if ![
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            ]
            .iter()
            .all(|v| v.is_finite())
                || frame.size.width <= 0.0
                || frame.size.height <= 0.0
            {
                return false;
            }
            if drop.target.is_some()
                || (!different
                    && self.get_floating_position(destination, drop.window) == Some(frame))
            {
                return false;
            }
            None
        } else {
            if drop.frame.is_some() {
                return false;
            }

            if let Some((target, action)) = drop.target {
                if (target == drop.window
                    && !matches!(
                        action,
                        crate::layout_engine::WindowDropAction::Move(_)
                            | crate::layout_engine::WindowDropAction::ConsumeOrExpel(_)
                    ))
                    || windows.workspace_for_window(target) != Some(destination)
                    || self.is_window_floating(target)
                {
                    return false;
                }
                let Some((mut tree, before)) =
                    self.prospective_drop_tree(destination, layout, drop.window, target, action)
                else {
                    return false;
                };
                if before.is_some() && before == tree.window_slot(layout, drop.window) {
                    return false;
                }
                tree.focus(layout, Focus::Select(drop.window)).handled;
                Some(tree)
            } else if different {
                None // Generic relocation already inserts a rendered member.
            } else {
                return false;
            }
        };
        if different {
            if !self.relocate_window_to_workspace(
                windows,
                source,
                (space, destination),
                drop.window,
                WindowRelocation::Drop(prospective),
            ) {
                return false;
            }
        } else if let Some(tree) = prospective {
            self.native_layouts.remove(&destination);
            self.workspaces[destination].layout_system = tree;
        }
        if self.focused_window == Some(drop.window) {
            self.focused_window = None;
        }
        if let Some(frame) = drop.frame {
            windows.clear_floating_display_geometry(drop.window);
            self.store_floating_position(destination, drop.window, frame);
        }
        self.workspaces.set_last_focused_window(space, destination, Some(drop.window));
        true
    }

    /// Return the visible logical tiles eligible for drag targeting.
    /// Hidden members of stacked/grouped containers are excluded.
    pub(crate) fn drop_scene_windows(&self, space: SpaceId, source: WindowId) -> Vec<WindowId> {
        if self.active_layout_mode_at(space) == LayoutMode::Floating {
            return Vec::new();
        }
        let Some((workspace, layout)) = self.workspaces.active_layout_for_space(space) else {
            return Vec::new();
        };
        let system = &self.workspaces[workspace].layout_system;
        let mut windows = system.windows(layout, WindowQuery::Visible);
        for window in system.windows(layout, WindowQuery::Stack(source)) {
            if !windows.contains(&window) {
                windows.push(window);
            }
        }
        windows
    }

    pub(crate) fn drop_action_override(
        &self,
        space: SpaceId,
    ) -> Option<crate::layout_engine::WindowDropAction> {
        let workspace = self.workspaces.active_workspace(space)?;
        matches!(
            self.workspaces[workspace].layout_system,
            LayoutSystemKind::Stack(_)
        )
        .then_some(crate::layout_engine::WindowDropAction::Swap)
    }

    pub(crate) fn drop_preview_frame(
        &self,
        space: SpaceId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
        screen: CGRect,
        display_uuid: Option<&str>,
        stack_line: &crate::common::config::StackLineSettings,
    ) -> Option<DropPreview> {
        let (workspace, layout) = self.workspaces.active_layout_for_space(space)?;
        let system = &self.workspaces[workspace].layout_system;
        if !system.window_participates(source)
            || !system.window_participates(target)
            || !system.contains_window(layout, target)
        {
            return None;
        }
        let gaps = self.layout_settings.gaps.effective_for_display(display_uuid);
        let (mut system, before) =
            self.prospective_drop_tree(workspace, layout, source, target, action)?;
        self.window_layout_constraints.prepare(&mut system, layout, screen, &gaps);
        let frame = self
            .project_arrangement(&system, layout, screen, &gaps, stack_line.reserve())
            .into_iter()
            .find_map(|(window, frame)| (window == source).then_some(frame))?;
        Some(DropPreview {
            frame,
            same_slot: before.is_some() && system.window_slot(layout, source) == before,
        })
    }

    /// The layout a drop of `source` on `target` would leave, and `source`'s slot before it.
    /// A window from elsewhere (no slot) joins the layout first, as the drop adds it.
    fn prospective_drop_tree(
        &self,
        workspace: VirtualWorkspaceId,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
    ) -> Option<(LayoutSystemKind, Option<Vec<usize>>)> {
        let mut tree = self.workspaces[workspace].layout_system.preview_clone()?;
        let before = tree.window_slot(layout, source);
        if before.is_none() {
            tree.update(WindowChange::Add(layout, source, None));
        }
        let changed = tree.edit(layout, LayoutEdit::Drop { source, target, action }).changed();
        changed.then_some((tree, before))
    }

    pub(crate) fn source_move_neighbors(&self, space: SpaceId, source: WindowId) -> [bool; 4] {
        let Some((workspace, layout)) = self.workspaces.active_layout_for_space(space) else {
            return [false; 4];
        };
        let baseline = &self.workspaces[workspace].layout_system;
        let directions = [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ];
        directions.map(|direction| match baseline {
            LayoutSystemKind::Traditional(system) => {
                system.can_move_window(layout, source, direction)
            }
            LayoutSystemKind::Bsp(system) => system.can_move_window(layout, source, direction),
            LayoutSystemKind::MasterStack(system) => {
                system.can_move_window(layout, source, direction)
            }
            LayoutSystemKind::Scrolling(system) => {
                system.can_move_window(layout, source, direction)
            }
            LayoutSystemKind::Stack(_) | LayoutSystemKind::Floating(_) => false,
        })
    }

    /// Resolve an optional workspace index and snapshot its layout for read-only consumers.
    pub(crate) fn query_workspace_layout(
        &self,
        target: crate::model::virtual_workspace::WorkspaceTarget,
    ) -> Option<WorkspaceLayoutQuerySnapshot> {
        let workspace_id = target.id;
        let workspace = self.workspaces.workspaces.get(workspace_id)?;
        if workspace.space != target.space {
            return None;
        }
        let active = self.workspaces.active_workspace(target.space)?;
        let workspace_index = self.workspaces.workspace_index(target.space, workspace_id)?;
        let layout = workspace.active_layout()?;
        let selected_window = workspace.layout_system.selected_window(layout);
        let container_tree = workspace.layout_system.container_tree(layout);

        Some(WorkspaceLayoutQuerySnapshot {
            workspace_id,
            workspace_index,
            is_active: workspace_id == active,
            mode: workspace.layout_mode,
            selected_window,
            container_tree,
        })
    }

    pub(crate) fn calculate_workspace_layout(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let Some(layout) = self.workspaces.active_layout(space, workspace_id) else {
            return Vec::new();
        };
        self.calculate_arrangement(workspace_id, layout, screen, gaps, stack_line)
    }

    fn calculate_arrangement(
        &self,
        workspace_id: VirtualWorkspaceId,
        layout: LayoutId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        self.project_arrangement(
            &self.workspaces[workspace_id].layout_system,
            layout,
            screen,
            gaps,
            stack_line,
        )
    }

    fn project_arrangement(
        &self,
        system: &LayoutSystemKind,
        layout: LayoutId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        if let LayoutSystemKind::Scrolling(scrolling) = system {
            return scrolling.calculate_frames(
                layout,
                screen,
                &self.window_layout_constraints.windows,
                Some(self.window_layout_constraints.revision),
                gaps,
                true,
            );
        }
        system.calculate_layout(
            layout,
            screen,
            self.layout_settings.stack.stack_offset,
            &self.window_layout_constraints.windows,
            gaps,
            stack_line,
        )
    }

    fn switch_workspace_layout_mode(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        mode: LayoutMode,
    ) -> bool {
        let (current_mode, selected_window, mut window_order) = {
            let Some(workspace) = self.workspaces.workspace_info(space, workspace_id) else {
                return false;
            };
            if workspace.layout_mode == mode {
                return false;
            }
            let old_layout = workspace.active_layout();
            let selected =
                old_layout.and_then(|layout| workspace.layout_system.selected_window(layout));
            let mut ordered = old_layout
                .map(|layout| workspace.layout_system.windows(layout, WindowQuery::All))
                .unwrap_or_default();
            // Keep windows hidden by stack/group selection when rebuilding into a new mode.
            let mut hidden_windows: Vec<_> = window_store
                .workspace_windows(workspace_id)
                .into_iter()
                .filter(|wid| !ordered.contains(wid))
                .collect();
            hidden_windows.sort();
            ordered.extend(hidden_windows);
            (workspace.layout_mode, selected, ordered)
        };
        self.forget_native_layout_for_workspace(workspace_id);

        if current_mode == LayoutMode::Floating || mode == LayoutMode::Floating {
            for &wid in &window_order {
                self.floating.remove_floating(wid);
                self.floating_positions.remove_window(wid);
            }
        }
        window_order.retain(|wid| !self.floating.is_floating(*wid));

        if mode == LayoutMode::Scrolling && self.layout_settings.scrolling.preserve_window_sizes {
            for &wid in &window_order {
                self.preserve_scrolling_window_width(window_store, wid);
            }
        }

        let Some(workspace) = self.workspaces.workspaces.get_mut(workspace_id) else {
            return false;
        };
        let new_layout = workspace.replace_layout_mode(mode, &self.layout_settings);

        for wid in window_order {
            workspace.layout_system.update(WindowChange::Add(new_layout, wid, None));
            if window_store.is_temporarily_unavailable(wid) {
                workspace.layout_system.set_window_participation(wid, false);
            }
        }

        if let LayoutSystemKind::Floating(system) = &mut workspace.layout_system {
            system.spread_initial_windows();
        }

        if let Some(selected) = selected_window.filter(|wid| !self.floating.is_floating(*wid)) {
            let _ = workspace.layout_system.focus(new_layout, Focus::Select(selected)).handled;
        }

        if mode == LayoutMode::Scrolling {
            self.sync_scrolling_widths_for_space(space);
        }

        true
    }

    fn response_for_raised_windows(raise_windows: Vec<WindowId>) -> EventResponse {
        if raise_windows.is_empty() {
            EventResponse::default()
        } else {
            EventResponse {
                changed: true,
                raise_windows,

                ..EventResponse::default()
            }
        }
    }

    fn toggle_orientation_for_system<S: LayoutSystem>(
        system: &mut S,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> EventResponse {
        if system.parent_of_selection_is_stacked(layout) {
            let toggled_windows = system
                .edit(layout, LayoutEdit::Group(GroupEdit::Stack(default_orientation)))
                .into_windows();
            return Self::response_for_raised_windows(toggled_windows);
        }
        system.edit(layout, LayoutEdit::ToggleOrientation).discard();
        EventResponse::default()
    }

    fn toggle_stack_for_workspace(
        &mut self,
        workspace_id: VirtualWorkspaceId,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> EventResponse {
        let system = &mut self.workspaces[workspace_id].layout_system;
        let selected_window = system.selected_window(layout);
        let unstacked_windows = system
            .edit(
                layout,
                LayoutEdit::Group(GroupEdit::Unstack(default_orientation)),
            )
            .into_windows();
        let raised_windows = if !unstacked_windows.is_empty() {
            unstacked_windows
        } else {
            let stacked_windows = system
                .edit(layout, LayoutEdit::Group(GroupEdit::Stack(default_orientation)))
                .into_windows();
            if !stacked_windows.is_empty() {
                stacked_windows
            } else {
                system.windows(layout, WindowQuery::Visible)
            }
        };

        let mut response = Self::response_for_raised_windows(raised_windows);
        if response.changed
            && let Some(selected_window) = selected_window
            && system.focus(layout, Focus::Select(selected_window)).handled
        {
            response.focus_window = Some(selected_window);
        }
        response
    }

    fn collect_group_containers_for_space(
        &self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        selection_path_only: bool,
    ) -> Vec<GroupContainerInfo> {
        let Some((ws_id, layout_id)) = self.workspaces.active_layout_for_space(space) else {
            return Vec::new();
        };
        let stack_offset = self.layout_settings.stack.stack_offset;
        self.workspaces[ws_id].layout_system.collect_group_containers(
            layout_id,
            screen,
            stack_offset,
            &self.window_layout_constraints.windows,
            gaps,
            stack_line,
            selection_path_only,
        )
    }
}

impl LayoutEngine {
    pub fn set_layout_settings(&mut self, settings: &LayoutSettings) {
        if &self.layout_settings != settings {
            self.apply_layout_settings(settings);
        }
    }

    // Hydrated systems carry saved settings even though the runtime settings
    // field starts at its default. Loading must apply the current configuration.
    fn apply_layout_settings(&mut self, settings: &LayoutSettings) {
        self.native_layouts.clear();
        self.layout_settings = settings.clone();

        for (_, ws) in self.workspaces.workspaces.iter_mut() {
            let display = self.space_display_map.get(&ws.space).and_then(Option::as_deref);
            ws.apply_layout_settings(settings, display);
        }
    }

    pub(crate) fn update_virtual_workspace_settings(
        &mut self,
        window_store: &WindowStore,
        settings: &crate::common::config::VirtualWorkspaceSettings,
        plan: crate::model::virtual_workspace::WorkspaceSettingsPlan,
    ) {
        let mode_changed = self.workspaces.settings.mode != settings.mode;
        let previous_modes: HashMap<_, _> = self
            .workspaces
            .catalog()
            .filter_map(|id| {
                let workspace = &self.workspaces[id];
                let index = self.workspaces.workspace_index(workspace.space, id)?;
                Some((
                    id,
                    self.workspaces.resolve_layout_mode_for_workspace(index, &workspace.name),
                ))
            })
            .collect();
        self.app_rules = AppRuleEngine::new(&settings.app_rules);
        self.workspaces.apply_settings(settings, &self.layout_settings, plan);

        // Changing namespace policy must preserve every existing layout tree.
        let ids: Vec<_> = self.workspaces.catalog().collect();
        for id in ids {
            let workspace = &self.workspaces[id];
            let space = workspace.space;
            let Some(index) = self.workspaces.workspace_index(space, id) else {
                continue;
            };
            let desired = self.workspaces.resolve_layout_mode_for_workspace(index, &workspace.name);
            if !mode_changed
                && workspace.layout_mode() != desired
                && previous_modes.get(&id).is_some_and(|previous| *previous != desired)
            {
                self.switch_workspace_layout_mode(window_store, space, id, desired);
            }
        }
        for space in self.workspaces.initialized_spaces() {
            self.sync_scrolling_widths_for_space(space);
        }
    }

    pub fn active_layout_mode_at(&self, space: SpaceId) -> LayoutMode {
        self.workspaces
            .active_workspace(space)
            .map(|id| self.workspaces[id].layout_mode())
            .unwrap_or_default()
    }

    pub(crate) fn focus_changes_geometry(&self, space: SpaceId) -> bool {
        self.workspaces
            .active_layout_for_space(space)
            .is_some_and(|(workspace, layout)| {
                let system = &self.workspaces[workspace].layout_system;
                matches!(
                    system,
                    LayoutSystemKind::Scrolling(_) | LayoutSystemKind::Floating(_)
                ) || system.parent_of_selection_is_stacked(layout)
            })
    }

    pub fn layout_specific_animate_settings(&self, space: SpaceId) -> Option<bool> {
        if let Some(ws_id) = self.workspaces.active_workspace(space) {
            match &self.workspaces[ws_id].layout_system {
                LayoutSystemKind::Scrolling(_) => {
                    Some(self.layout_settings.scrolling.animate.unwrap_or(true))
                }
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
            .active_windows(space)
            .filter(|&window| self.is_active_workspace_window(window_store, space, window))
            .collect()
    }

    fn preferred_focus_for_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        preferred_focus_window: Option<WindowId>,
    ) -> Option<WindowId> {
        let belongs = |wid: &WindowId| {
            window_store.workspace_for_window(*wid) == Some(workspace_id)
                && !window_store.is_temporarily_unavailable(*wid)
        };
        let workspace = &self.workspaces[workspace_id];
        preferred_focus_window
            .filter(belongs)
            .or_else(|| workspace.last_focused().filter(belongs))
            .or_else(|| {
                let layout = workspace.active_layout()?;
                workspace.layout_system.selected_window(layout).filter(belongs).or_else(|| {
                    workspace
                        .layout_system
                        .windows(layout, WindowQuery::Visible)
                        .into_iter()
                        .find(belongs)
                })
            })
            .or_else(|| {
                let floating = self.active_floating_windows_in_workspace(window_store, space);
                self.floating
                    .last_focus()
                    .filter(|wid| floating.contains(wid))
                    .or_else(|| floating.first().copied())
            })
    }

    pub fn commit_workspace_focus(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        focus_window: Option<WindowId>,
    ) {
        let Some(workspace_id) = self.workspaces.active_workspace(space) else {
            self.focused_window = None;
            return;
        };

        let focus_window = focus_window.filter(|wid| {
            window_store.workspace_for_window(*wid) == Some(workspace_id)
                && !window_store.is_temporarily_unavailable(*wid)
        });

        if let Some(wid) = focus_window {
            self.focused_window = Some(wid);
            self.workspaces.set_last_focused_window(space, workspace_id, Some(wid));
            if self.floating.is_floating(wid) {
                self.floating.set_last_focus(Some(wid));
            } else if let Some(layout) = self.workspaces.active_layout(space, workspace_id) {
                let _ = self.workspaces[workspace_id]
                    .layout_system
                    .focus(layout, Focus::Select(wid))
                    .handled;
            }
        } else {
            self.focused_window = None;
            self.workspaces.set_last_focused_window(space, workspace_id, None);
        }
    }

    pub(crate) fn activate_workspace(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        preferred_focus_window: Option<WindowId>,
    ) -> EventResponse {
        if !self.workspaces.set_active_workspace(space, workspace_id) {
            return EventResponse::default();
        }
        self.update_active_floating_windows(window_store, space);

        EventResponse {
            changed: true,
            workspace_space: Some(space),
            focus_window: self.preferred_focus_for_workspace(
                window_store,
                space,
                workspace_id,
                preferred_focus_window,
            ),
            raise_windows: vec![],

            ..EventResponse::default()
        }
    }

    fn switch_to_workspace_id(
        &mut self,
        window_store: &mut WindowStore,
        context: WorkspaceContext<'_>,
        id: VirtualWorkspaceId,
        preferred_focus_window: Option<WindowId>,
    ) -> EventResponse {
        let command_space = context.space;
        if self.workspaces.active_workspace(command_space) == Some(id) {
            return if self.workspaces.settings.workspace_auto_back_and_forth {
                self.switch_to_previous_workspace(window_store, context)
            } else {
                EventResponse::default()
            };
        }
        self.navigate_to_workspace(window_store, command_space, id, preferred_focus_window)
    }

    fn switch_to_previous_workspace(
        &mut self,
        window_store: &mut WindowStore,
        context: WorkspaceContext<'_>,
    ) -> EventResponse {
        self.workspaces
            .previous_target(context)
            .map(|target| self.navigate_to_workspace(window_store, context.space, target.id, None))
            .unwrap_or_default()
    }

    fn navigate_to_workspace(
        &mut self,
        window_store: &mut WindowStore,
        command_space: SpaceId,
        id: VirtualWorkspaceId,
        preferred_focus_window: Option<WindowId>,
    ) -> EventResponse {
        let space = self.workspaces[id].space;
        let was_active = self.workspaces.active_workspace(space) == Some(id);
        if !self.workspaces.set_active_workspace_from(command_space, id) {
            return EventResponse::default();
        }
        if was_active && command_space != space {
            return EventResponse {
                workspace_space: Some(space),
                focus_window: self.preferred_focus_for_workspace(
                    window_store,
                    space,
                    id,
                    preferred_focus_window,
                ),
                ..EventResponse::default()
            };
        }
        self.activate_workspace(window_store, space, id, preferred_focus_window)
    }

    fn filter_active_workspace_windows(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        mut windows: Vec<WindowId>,
    ) -> Vec<WindowId> {
        windows.retain(|&window| self.is_active_workspace_window(window_store, space, window));
        windows
    }

    fn filter_active_workspace_window(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        window: Option<WindowId>,
    ) -> Option<WindowId> {
        window.filter(|&window| self.is_active_workspace_window(window_store, space, window))
    }

    fn is_active_workspace_window(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        window: WindowId,
    ) -> bool {
        !window_store.is_temporarily_unavailable(window)
            && self.workspaces.is_window_in_active_workspace(window_store, space, window)
    }

    pub fn resize_selection(
        &mut self,
        ws_id: VirtualWorkspaceId,
        layout: LayoutId,
        resize_amount: f64,
    ) {
        self.forget_native_layout_for_workspace(ws_id);
        self.workspaces[ws_id]
            .layout_system
            .edit(layout, LayoutEdit::Resize {
                amount: resize_amount,
                orientation: ResizeOrientation::Horizontal,
            })
            .discard();
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
                let _ =
                    self.workspaces[ws_id].layout_system.focus(layout, Focus::Select(wid)).handled;
                self.workspaces.set_last_focused_window(space, ws_id, Some(wid));
            }
        }
    }

    fn move_focus_internal(
        &mut self,
        window_store: &mut WindowStore,
        space: SpaceId,
        visible_spaces: &[SpaceId],
        visible_space_frames: &HashMap<SpaceId, CGRect>,
        direction: Direction,
        is_floating: bool,
    ) -> EventResponse {
        let Some((ws_id, layout)) = self.workspaces.active_layout_for_space(space) else {
            warn!(
                "No active workspace/layout for space {:?}; move_focus ignored",
                space
            );
            return EventResponse::default();
        };

        let commit = |engine: &mut Self,
                      window_store: &mut WindowStore,
                      (space, ws, layout),
                      window,
                      raise_windows| {
            let response = EventResponse {
                changed: true,
                focus_window: Some(window),
                raise_windows,
                ..Default::default()
            };
            engine.apply_focus_response(window_store, space, ws, layout, &response);
            response
        };
        let current = (space, ws_id, layout);
        let visible =
            |engine: &Self,
             windows: &WindowStore,
             (space, ws, layout): (SpaceId, VirtualWorkspaceId, LayoutId)| {
                engine.filter_active_workspace_windows(
                    windows,
                    space,
                    engine.workspaces[ws].layout_system.windows(layout, WindowQuery::Visible),
                )
            };

        if is_floating {
            let windows = self.active_floating_windows_in_workspace(window_store, space);
            if matches!(direction, Direction::Left | Direction::Right)
                && windows.len() > 1
                && let Some(index) = windows.iter().position(|&w| Some(w) == self.focused_window)
            {
                let step = if direction == Direction::Left {
                    windows.len() - 1
                } else {
                    1
                };
                return commit(
                    self,
                    window_store,
                    current,
                    windows[(index + step) % windows.len()],
                    Vec::new(),
                );
            }
            let tiled = visible(self, window_store, current);
            return match tiled.first().copied() {
                Some(window) => commit(self, window_store, current, window, tiled),
                None => EventResponse::default(),
            };
        }

        let system = &mut self.workspaces[ws_id].layout_system;
        let previous = system.selected_window(layout);
        let focus = system.focus(layout, Focus::Direction(direction));
        if let Some(window) = self.filter_active_workspace_window(window_store, space, focus.window)
        {
            let raise = self.filter_active_workspace_windows(window_store, space, focus.raise);
            return commit(self, window_store, current, window, raise);
        }
        if let Some(window) = previous {
            self.workspaces[ws_id].layout_system.focus(layout, Focus::Select(window));
        }
        if matches!(direction, Direction::Left | Direction::Right)
            && matches!(
                self.workspaces[ws_id].layout_system,
                LayoutSystemKind::Scrolling(_)
            )
        {
            return EventResponse::default();
        }

        if let Some(destination) =
            self.next_space_for_direction(space, direction, visible_spaces, visible_space_frames)
        {
            let Some((ws, layout)) = self.workspaces.active_layout_for_space(destination) else {
                return EventResponse::default();
            };
            let target = (destination, ws, layout);
            let raise = visible(self, window_store, target);
            let directional =
                self.workspaces[ws].layout_system.window_in_direction(layout, direction);
            if let Some(window) = self
                .filter_active_workspace_window(window_store, destination, directional)
                .or_else(|| raise.first().copied())
            {
                self.workspaces[ws].layout_system.focus(layout, Focus::Select(window));
                return commit(self, window_store, target, window, raise);
            }
        }

        let fallback = self
            .active_floating_windows_in_workspace(window_store, space)
            .first()
            .copied()
            .or_else(|| self.filter_active_workspace_window(window_store, space, previous))
            .or_else(|| visible(self, window_store, current).first().copied());
        match fallback {
            Some(window) => commit(self, window_store, current, window, Vec::new()),
            None => EventResponse::default(),
        }
    }

    fn next_space_for_direction(
        &self,
        current_space: SpaceId,
        direction: Direction,
        visible_spaces: &[SpaceId],
        space_frames: &HashMap<SpaceId, CGRect>,
    ) -> Option<SpaceId> {
        let current = *space_frames.get(&current_space)?;
        let mut best: Option<(f64, f64, SpaceId)> = None;
        for &space in visible_spaces {
            if space == current_space {
                continue;
            }
            let Some(candidate) = space_frames.get(&space) else {
                continue;
            };
            let (primary_gap, orth_gap) = match direction {
                Direction::Left => (
                    current.min().x - candidate.max().x,
                    interval_gap(
                        current.min().y,
                        current.max().y,
                        candidate.min().y,
                        candidate.max().y,
                    ),
                ),
                Direction::Right => (
                    candidate.min().x - current.max().x,
                    interval_gap(
                        current.min().y,
                        current.max().y,
                        candidate.min().y,
                        candidate.max().y,
                    ),
                ),
                Direction::Up => (
                    current.min().y - candidate.max().y,
                    interval_gap(
                        current.min().x,
                        current.max().x,
                        candidate.min().x,
                        candidate.max().x,
                    ),
                ),
                Direction::Down => (
                    candidate.min().y - current.max().y,
                    interval_gap(
                        current.min().x,
                        current.max().x,
                        candidate.min().x,
                        candidate.max().x,
                    ),
                ),
            };
            if primary_gap < 0.0 {
                continue;
            }
            // Prefer aligned displays, then the nearest edge in the requested direction.
            if best.as_ref().is_none_or(|&(orth, primary, _)| {
                orth_gap < orth || (orth_gap == orth && primary_gap < primary)
            }) {
                best = Some((orth_gap, primary_gap, space));
            }
        }
        best.map(|(_, _, space)| space)
    }

    // Explicit placement edits revoke recovery even when logical membership is unchanged.
    // Native reconciliation deliberately does not use this boundary.
    fn forget_native_layout_for_window_id(&mut self, wid: WindowId) {
        self.evicted.remove(&wid);
        for (system, _) in self.native_layouts.values_mut() {
            system.update(WindowChange::Remove(wid));
        }
        self.prune_native_layouts();
    }

    fn forget_native_layout_for_workspace(&mut self, workspace: VirtualWorkspaceId) {
        self.native_layouts.remove(&workspace);
        let edited: Vec<_> = self
            .evicted
            .iter()
            .filter_map(|(&window, record)| (record.landed == workspace).then_some(window))
            .collect();
        for window in edited {
            self.forget_native_layout_for_window_id(window);
        }
    }

    fn prune_native_layouts(&mut self) {
        self.native_layouts.retain(|_, (system, state)| {
            state
                .all_layouts()
                .any(|layout| !system.windows(layout, WindowQuery::All).is_empty())
        });
    }

    pub(crate) fn forget_native_layout_for_window(&mut self, windows: &WindowStore, wid: WindowId) {
        self.forget_native_layout_for_window_id(wid);
        if let Some(assignment) = windows.workspace_for_window(wid) {
            self.native_layouts.remove(&assignment);
        }
    }

    pub(crate) fn remember_native_departure(&mut self, windows: &WindowStore, wid: WindowId) {
        let Some(assignment) = windows.workspace_for_window(wid) else {
            return;
        };
        let workspace = assignment;
        if self.floating.is_floating(wid) || self.native_layouts.contains_key(&workspace) {
            return;
        }
        let current = &self.workspaces[workspace];
        if let Some(system) = current.layout_system.preview_clone() {
            self.native_layouts.insert(workspace, (system, current.layout_state.clone()));
        }
    }

    pub(crate) fn begin_native_reconcile(
        &mut self,
        windows: &WindowStore,
        moves: &[(WindowId, SpaceId)],
    ) {
        // Capture every source before the first arrival can modify another source tree.
        for &(window, target) in moves {
            if windows
                .workspace_for_window(window)
                .is_some_and(|source| self.workspaces[source].space != target)
            {
                self.remember_native_departure(windows, window);
            }
        }
        self.native_reconcile_batch = Some(HashSet::default());
    }

    pub(crate) fn finish_native_reconcile(&mut self, windows: &WindowStore) {
        let workspaces = self.native_reconcile_batch.take().unwrap_or_default();
        for workspace in workspaces {
            self.restore_native_layout(windows, workspace, None);
        }
        // Forget windows that were closed, rekeyed or placed somewhere else since.
        let stale: Vec<_> = self
            .evicted
            .iter()
            .filter_map(|(&wid, record)| {
                (windows.workspace_for_window(wid) != Some(record.landed)).then_some(wid)
            })
            .collect();
        for wid in stale {
            self.forget_native_layout_for_window_id(wid);
        }
        self.prune_native_layouts();
    }

    /// Records the display and workspace position `wid` was moved from because its display
    /// departed. Call after the window has been reassigned; `from` is its previous assignment.
    pub(crate) fn note_eviction(
        &mut self,
        windows: &WindowStore,
        wid: WindowId,
        from: VirtualWorkspaceId,
    ) {
        let (Some(landed), Some(display)) = (
            windows.workspace_for_window(wid),
            self.display_uuid_for_space(self.workspaces[from].space),
        ) else {
            return;
        };
        if self.display_uuid_for_space(self.workspaces[landed].space).as_deref() == Some(display) {
            return;
        }
        let Some(ordinal) = self
            .workspaces
            .workspaces_on(self.workspaces[from].space)
            .position(|id| id == from)
        else {
            return;
        };
        let display = display.to_owned();
        self.evicted.insert(wid, EvictedWindow { display, ordinal, landed });
    }

    /// The workspace position `wid` was evicted from, if the display now showing `space` is the
    /// one it was evicted from and nothing has placed the window elsewhere since.
    pub(crate) fn eviction_ordinal(
        &self,
        windows: &WindowStore,
        wid: WindowId,
        space: SpaceId,
    ) -> Option<usize> {
        let record = self.evicted.get(&wid)?;
        if windows.workspace_for_window(wid) != Some(record.landed) {
            return None;
        }
        if self.display_uuid_for_space(space).as_deref() != Some(record.display.as_str()) {
            return None;
        }
        Some(record.ordinal)
    }

    /// Consume recovery only after native reassignment has succeeded, including a fallback.
    pub(crate) fn take_eviction(&mut self, wid: WindowId) -> Option<usize> {
        self.evicted.remove(&wid).map(|record| record.ordinal)
    }

    fn restore_native_layout(
        &mut self,
        windows: &WindowStore,
        workspace: VirtualWorkspaceId,
        returning: Option<WindowId>,
    ) {
        let Some((saved, state)) = self.native_layouts.get(&workspace) else {
            return;
        };
        if returning.is_some_and(|window| {
            !state.all_layouts().any(|layout| saved.contains_window(layout, window))
        }) {
            return;
        }
        if let Some(workspaces) = self.native_reconcile_batch.as_mut() {
            workspaces.insert(workspace);
            return;
        }
        let original: HashSet<_> = state
            .all_layouts()
            .flat_map(|layout| saved.windows(layout, WindowQuery::All))
            .collect();
        let current = &self.workspaces[workspace];
        let Some(size) = current.layout_state.active_size() else {
            return;
        };
        let mut desired = HashSet::default();
        let ordered: Vec<_> = current
            .layout_state
            .all_layouts()
            .flat_map(|layout| current.layout_system.windows(layout, WindowQuery::All))
            .filter(|window| {
                windows
                    .workspace_for_window(*window)
                    .or_else(|| windows.claim(*window).and_then(|claim| claim.workspace()))
                    == Some(workspace)
                    && (windows.is_admitted(*window) || windows.is_temporarily_unavailable(*window))
                    && !self.floating.is_floating(*window)
                    && desired.insert(*window)
            })
            .collect();
        if original.is_disjoint(&desired) {
            return;
        }
        let Some(mut system) = saved.preview_clone() else {
            return;
        };
        let mut state = state.clone();
        // Prune only a working copy: partial returns must share one original arrangement.
        for window in original.difference(&desired) {
            system.update(WindowChange::Remove(*window));
        }
        for layout in state.all_layouts() {
            for &window in &ordered {
                if !system.contains_window(layout, window) {
                    system.update(WindowChange::Add(layout, window, None));
                }
            }
        }
        // Display recovery restores topology, never historical eligibility.
        for &window in &ordered {
            system.set_window_participation(window, !windows.is_temporarily_unavailable(window));
        }
        state.ensure_active(size, &mut system);
        if let Some(focused) = self.focused_window.filter(|window| desired.contains(window))
            && let Some(layout) = state.active()
        {
            system.focus(layout, Focus::Select(focused)).handled;
        }
        let current = &mut self.workspaces[workspace];
        current.layout_system = system;
        current.layout_state = state;
        if original.is_subset(&desired) {
            self.native_layouts.remove(&workspace);
        }
    }

    fn suspend_window(&mut self, window_store: &WindowStore, wid: WindowId) -> bool {
        if let Some(assignment) = window_store.workspace_for_window(wid)
            && let Some(workspace) = self.workspaces.workspaces.get_mut(assignment)
        {
            let projected = workspace
                .active_layout()
                .is_some_and(|layout| workspace.layout_system.contains_window(layout, wid));
            let changed = (workspace.layout_system.set_window_participation(wid, false)
                && projected)
                || self.floating.is_active(workspace.space, wid);
            self.floating.remove_active_for_window(wid);
            if self.focused_window == Some(wid) {
                self.focused_window = None;
            }
            return changed;
        }
        false
    }

    fn remove_window_internal(
        &mut self,
        window_store: &mut WindowStore,
        wid: WindowId,
        preserve_floating: bool,
    ) -> bool {
        let removal = self.remove_window_layout_membership(window_store, wid);

        if preserve_floating {
            self.floating.remove_active_for_window(wid);
        } else {
            self.floating.remove_floating(wid);
        }

        if !preserve_floating {
            self.workspaces.remove_window(window_store, wid);
            self.floating_positions.remove_window(wid);
            self.forget_persisted_window(wid);
            self.evicted.remove(&wid);
        }

        if self.focused_window == Some(wid) {
            self.focused_window = None;
        }

        removal.is_some()
    }

    fn remove_window_layout_membership(
        &mut self,
        window_store: &WindowStore,
        wid: WindowId,
    ) -> Option<SpaceId> {
        if let Some(assignment) = window_store.workspace_for_window(wid)
            && let Some(workspace) = self.workspaces.workspaces.get_mut(assignment)
        {
            let projected = workspace
                .active_layout()
                .is_some_and(|layout| workspace.layout_system.contains_window(layout, wid));
            workspace.layout_system.update(WindowChange::Remove(wid));
            if workspace.last_focused() == Some(wid) {
                workspace.set_last_focused(None);
            }
            return (projected || self.floating.is_active(workspace.space, wid))
                .then_some(workspace.space);
        }
        let active_space = self.space_with_window(wid);

        // The store may already have dropped the record (for example after
        // WindowDestroyed). Layout membership is only a projection, so scrub
        // every tree when its authoritative assignment is unavailable.
        for workspace in self.workspaces.workspaces.values_mut() {
            workspace.layout_system.update(WindowChange::RemoveAndRebalance(wid));
            if workspace.last_focused() == Some(wid) {
                workspace.set_last_focused(None);
            }
        }
        active_space
    }

    fn add_window_to_layout(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        wid: WindowId,
    ) -> bool {
        if !window_store.is_admitted(wid) || window_store.is_window_native_fullscreen_suspended(wid)
        {
            return false;
        }
        let Some(assigned_workspace) =
            self.workspaces.workspace_for_window(window_store, space, wid)
        else {
            return false;
        };

        let active_space_before = self.projected_space_in(assigned_workspace, wid);
        let mut inserted = false;

        if matches!(
            self.workspaces[assigned_workspace].layout_system,
            LayoutSystemKind::Floating(_)
        ) {
            self.floating.remove_floating(wid);
        }
        let should_be_floating = self.floating.is_floating(wid);
        let resumed = self.workspaces[assigned_workspace]
            .layout_system
            .set_window_participation(wid, true);

        if should_be_floating {
            self.floating.add_active(space, wid.pid, wid);
        } else if let Some(layout) = self.workspaces.active_layout(space, assigned_workspace) {
            if !self.workspaces[assigned_workspace].layout_system.contains_window(layout, wid) {
                if matches!(
                    self.workspaces[assigned_workspace].layout_system,
                    LayoutSystemKind::Scrolling(_)
                ) && self.layout_settings.scrolling.preserve_window_sizes
                {
                    self.preserve_scrolling_window_width(window_store, wid);
                }
                self.workspaces[assigned_workspace]
                    .layout_system
                    .update(WindowChange::Add(layout, wid, None));
                inserted = true;
            }
        } else {
            warn!(
                "No active layout for workspace {:?} on space {:?}; window {:?} not added to tree",
                assigned_workspace, space, wid
            );
        }

        if inserted {
            self.restore_native_layout(window_store, assigned_workspace, Some(wid));
        }
        resumed || self.projected_space_in(assigned_workspace, wid) != active_space_before
    }

    fn preserve_scrolling_window_width(&mut self, window_store: &WindowStore, wid: WindowId) {
        if let Some(window) = window_store.window(wid) {
            let mut constraints =
                self.window_layout_constraints.windows.get(&wid).copied().unwrap_or_else(|| {
                    WindowLayoutConstraints {
                        is_resizable: window.info.is_resizable,
                        locked_height: window.frame_monotonic.size.height,
                        min_width: window.info.min_size.map_or(0.0, |size| size.width),
                        min_height: window.info.min_size.map_or(0.0, |size| size.height),
                        max_width: window.info.max_size.map_or(0.0, |size| size.width),
                        max_height: window.info.max_size.map_or(0.0, |size| size.height),
                        ..Default::default()
                    }
                    .normalized()
                });
            constraints.locked_width = window.frame_monotonic.size.width;
            self.window_layout_constraints.insert(wid, constraints);
        }
    }

    fn remove_window_from_all_tiling_trees(&mut self, wid: WindowId) {
        for workspace in self.workspaces.workspaces.values_mut() {
            workspace.layout_system.update(WindowChange::Remove(wid));
        }
    }

    fn workspace_contains_window(&self, workspace_id: VirtualWorkspaceId, wid: WindowId) -> bool {
        let workspace = &self.workspaces[workspace_id];
        workspace
            .layout_state
            .all_layouts()
            .any(|layout| workspace.layout_system.contains_window(layout, wid))
    }

    fn active_workspace_contains_window(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
        wid: WindowId,
    ) -> bool {
        self.workspaces.active_layout(space, workspace_id).is_some_and(|layout| {
            self.workspaces[workspace_id].layout_system.contains_window(layout, wid)
        })
    }

    fn projected_space_in(
        &self,
        workspace: VirtualWorkspaceId,
        window: WindowId,
    ) -> Option<SpaceId> {
        let current = self.workspaces.workspaces.get(workspace)?;
        let visible = self.workspaces.active_workspace(current.space) == Some(workspace)
            && current
                .active_layout()
                .is_some_and(|layout| current.layout_system.contains_window(layout, window));
        (visible || self.floating.is_active(current.space, window)).then_some(current.space)
    }

    pub(crate) fn space_with_window(&self, wid: WindowId) -> Option<SpaceId> {
        self.workspaces.catalog().find_map(|id| self.projected_space_in(id, wid))
    }

    fn active_workspace_id_and_name(
        &self,
        space_id: SpaceId,
    ) -> Option<(crate::model::VirtualWorkspaceId, String)> {
        let workspace_id = self.workspaces.active_workspace(space_id)?;
        Some((workspace_id, self.workspaces[workspace_id].name.clone()))
    }

    pub fn update_space_display(&mut self, space: SpaceId, display_uuid: Option<&str>) {
        // An unchanged topology mapping is a no-op.
        if display_uuid.is_none() && !self.space_display_map.contains_key(&space) {
            return;
        }
        if let Some(uuid) = display_uuid
            && self.display_uuid_for_space(space) == Some(uuid)
            && self.display_last_space.get(uuid) == Some(&space)
            && !self
                .workspaces
                .workspaces
                .values()
                .any(|ws| ws.space == space && ws.preferred_display.is_none())
        {
            return;
        }
        if let Some(uuid) = display_uuid {
            for workspace in self
                .workspaces
                .workspaces
                .values_mut()
                .filter(|workspace| workspace.space == space)
            {
                if workspace.preferred_display.is_none() {
                    workspace.preferred_display = Some(uuid.to_owned());
                    workspace.home_space = Some(space);
                }
            }
            self.space_display_map.insert(space, Some(uuid.to_owned()));
            // An unresolved saved Space still needs this display's evidence.
            if !self.saved_space_is_unresolved(self.display_last_space.get(uuid).copied()) {
                self.display_last_space.insert(uuid.to_owned(), space);
            }
        } else {
            self.space_display_map.remove(&space);
        }
        self.sync_scrolling_widths_for_space(space);
    }

    fn sync_scrolling_widths_for_space(&mut self, space: SpaceId) {
        let display = self.space_display_map.get(&space).and_then(Option::as_deref);
        let widths = self.layout_settings.scrolling.widths_for_display(display);
        for (_, ws) in self.workspaces.workspaces.iter_mut() {
            if ws.space == space {
                if let LayoutSystemKind::Scrolling(system) = &mut ws.layout_system {
                    system.update_width_settings(widths);
                }
            }
        }
    }

    pub fn last_space_for_display_uuid(&self, display_uuid: &str) -> Option<SpaceId> {
        self.display_last_space.get(display_uuid).copied()
    }

    pub(crate) fn display_uuid_for_space(&self, space: SpaceId) -> Option<&str> {
        self.space_display_map.get(&space).and_then(Option::as_deref)
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

        self.floating.remap_space(old_space, new_space);

        // Saved native arrangements are keyed by workspace; one for a deleted workspace
        // would no longer resolve.
        for deleted in self.workspaces.remap_space(window_store, old_space, new_space) {
            self.forget_workspace(deleted);
        }

        if let Some(uuid) = self.space_display_map.remove(&old_space) {
            self.space_display_map.insert(new_space, uuid);
        }

        for (_uuid, space) in self.display_last_space.iter_mut() {
            if *space == old_space {
                *space = new_space;
            }
        }
    }

    /// Drop everything keyed by a workspace that no longer exists; one left behind would no
    /// longer resolve.
    fn forget_workspace(&mut self, workspace: VirtualWorkspaceId) {
        self.native_layouts.remove(&workspace);
        if let Some(batch) = self.native_reconcile_batch.as_mut() {
            batch.remove(&workspace);
        }
        self.floating_positions.remove_workspace(workspace);
        self.forget_persisted_workspace(workspace);
    }

    pub fn new(
        virtual_workspace_config: &crate::common::config::VirtualWorkspaceSettings,
        layout_settings: &LayoutSettings,
    ) -> Self {
        let workspaces = WorkspaceStore::new_with_config(virtual_workspace_config, layout_settings);

        LayoutEngine {
            floating: FloatingManager::new(),
            floating_positions: FloatingPositionStore::default(),
            app_rules: AppRuleEngine::new(&virtual_workspace_config.app_rules),
            focused_window: None,
            window_layout_constraints: LayoutConstraints::default(),
            workspaces,
            layout_settings: layout_settings.clone(),
            space_display_map: HashMap::default(),
            display_last_space: HashMap::default(),
            persistence: PersistenceState::default(),
            scroll_boundary: None,
            native_layouts: HashMap::default(),
            native_reconcile_batch: None,
            evicted: HashMap::default(),
        }
    }

    fn apply_app_rule_outcome(
        &mut self,
        window: WindowId,
        space: SpaceId,
        was_floating: bool,
        effects: AppRuleEffects,
        app_rule_outcome: &mut AppRuleOutcome,
    ) -> Option<(WindowId, VirtualWorkspaceId)> {
        let should_float = effects.should_float(was_floating);
        if should_float {
            self.floating.add_floating(window);
            self.floating.add_active(space, window.pid, window);
        } else if was_floating {
            self.floating.remove_floating(window);
        }

        app_rule_outcome.frame =
            effects.floating_placement(window, space).map(AppRuleFrame::Place).or_else(|| {
                effects.tiled_resize(window, space, was_floating).map(AppRuleFrame::Resize)
            });

        effects.focus.then_some((window, effects.workspace_id))
    }

    pub(crate) fn apply_app_rule_resize(
        &mut self,
        resize: AppRuleResize,
        old_frame: CGRect,
        new_frame: CGRect,
        screen_frame: CGRect,
        display_uuid: Option<&str>,
    ) {
        self.window_layout_constraints.set_width(resize.window, new_frame.size.width);
        let Some(layout) = self.workspaces.active_layout(resize.space, resize.workspace_id) else {
            return;
        };
        let gaps = self.layout_settings.gaps.effective_for_display(display_uuid);
        let previous_selection =
            self.workspaces[resize.workspace_id].layout_system.selected_window(layout);
        let tree = &mut self.workspaces[resize.workspace_id].layout_system;
        let _ = tree.focus(layout, Focus::Select(resize.window)).handled;
        tree.update(WindowChange::Resize {
            layout,
            window: resize.window,
            old: old_frame,
            new: new_frame,
            screen: screen_frame,
            gaps: &gaps,
        });
        if let Some(previous) = previous_selection.filter(|window| *window != resize.window) {
            let _ = tree.focus(layout, Focus::Select(previous)).handled;
        }
    }

    pub fn debug_tree(&self, space: SpaceId) { self.debug_tree_desc(space, "", false); }

    pub fn debug_tree_desc(&self, space: SpaceId, desc: &'static str, print: bool) {
        if let Some(workspace_id) = self.workspaces.active_workspace(space) {
            if let Some(layout) = self.workspaces.active_layout(space, workspace_id) {
                if print {
                    println!(
                        "Tree {desc}\n{}",
                        self.workspaces[workspace_id].layout_system.draw_tree(layout).trim()
                    );
                } else {
                    debug!(
                        "Tree {desc}\n{}",
                        self.workspaces[workspace_id].layout_system.draw_tree(layout).trim()
                    );
                }
            } else {
                debug!("No layout for workspace {workspace_id:?} on space {space:?}");
            }
        } else {
            debug!("No active workspace for space {space:?}");
        }
    }

    /// Lifecycle additions restore participation, never logical ownership.
    fn normalize_event(&self, windows: &WindowStore, event: LayoutEvent) -> LayoutEvent {
        match event {
            LayoutEvent::WindowAdded(space, window) => LayoutEvent::WindowAdded(
                windows
                    .workspace_for_window(window)
                    .and_then(|workspace| self.workspaces.workspace_space(workspace))
                    .unwrap_or(space),
                window,
            ),
            event => event,
        }
    }

    pub fn handle_event(
        &mut self,
        window_store: &mut WindowStore,
        event: LayoutEvent,
    ) -> LayoutEventOutcome {
        let event = self.normalize_event(window_store, event);
        let mut app_rules = AppRuleOutcome::default();
        let response = self.handle_event_inner(window_store, event, &mut app_rules);
        LayoutEventOutcome { response, app_rules }
    }

    fn handle_event_inner(
        &mut self,
        window_store: &mut WindowStore,
        event: LayoutEvent,
        app_rule_outcome: &mut AppRuleOutcome,
    ) -> EventResponse {
        debug!(?event);
        // Claims keep a nonparticipating canonical slot until release. Ordinary
        // observations cannot reactivate it; destruction ends the claim first.
        if let LayoutEvent::WindowAdded(_, wid)
        | LayoutEvent::WindowSuspended(wid)
        | LayoutEvent::WindowRemoved(wid)
        | LayoutEvent::WindowObserved(
            _,
            ResolvedWindow {
                info: WindowLayoutInfo { window_id: wid, .. },
                ..
            },
        ) = &event
            && window_store.is_claimed(*wid)
        {
            return EventResponse::default();
        }
        match event {
            LayoutEvent::SpaceExposed(space, size) => {
                self.debug_tree(space);

                self.ensure_workspace_layouts(space, size);
            }
            #[cfg(test)]
            LayoutEvent::TestWindowsObserved(space, windows, app_info) => {
                let (app_bundle_id, app_name) = app_info.as_ref().map_or((None, None), |app| {
                    (app.bundle_id.as_deref(), app.localized_name.as_deref())
                });
                let mut response = EventResponse::default();
                for info in windows {
                    if window_store.window(info.window_id).is_none() {
                        let _ = self.observe_window_for_persistence(
                            window_store,
                            space,
                            info.window_id,
                            info.title.as_deref(),
                            info.current_size,
                            app_bundle_id,
                        );
                    }
                    let Ok(Some(AppRuleResult::Managed(effects))) = self
                        .assign_window_with_app_info(
                            window_store,
                            info.window_id,
                            space,
                            WindowRuleContext {
                                app_bundle_id,
                                app_name,
                                window_title: info.title.as_deref(),
                                ax_role: info.ax_role.as_deref(),
                                ax_subrole: info.ax_subrole.as_deref(),
                                window_tags: None,
                            },
                            false,
                        )
                    else {
                        continue;
                    };
                    let next = self.handle_event_inner(
                        window_store,
                        LayoutEvent::WindowObserved(space, ResolvedWindow { info, effects }),
                        &mut AppRuleOutcome::default(),
                    );
                    response.changed |= next.changed;
                    response.raise_windows.extend(next.raise_windows);
                    response.focus_window = next.focus_window.or(response.focus_window);
                    response.boundary_hit = next.boundary_hit.or(response.boundary_hit);
                }
                return response;
            }
            LayoutEvent::WindowObserved(_, ResolvedWindow { info, effects }) => {
                let space = self.workspaces[effects.workspace_id].space;
                self.debug_tree(space);
                let wid = info.window_id;
                self.window_layout_constraints.insert(
                    wid,
                    WindowLayoutConstraints {
                        is_resizable: info.is_resizable,
                        locked_width: info.current_size.width,
                        locked_height: info.current_size.height,
                        min_width: info.min_size.map_or(0.0, |s| s.width),
                        min_height: info.min_size.map_or(0.0, |s| s.height),
                        max_width: info.max_size.map_or(0.0, |s| s.width),
                        max_height: info.max_size.map_or(0.0, |s| s.height),
                    }
                    .normalized(),
                );

                let target_workspace = effects.workspace_id;
                let source_workspace = effects.source_workspace;
                let should_focus = effects.focus;
                let projected_space_before = source_workspace.map_or_else(
                    || self.space_with_window(wid),
                    |id| self.projected_space_in(id, wid),
                );
                let was_floating = self.floating.is_floating(wid);
                let focus_request = self.apply_app_rule_outcome(
                    wid,
                    space,
                    was_floating,
                    effects,
                    app_rule_outcome,
                );

                // Workspace policy owns floating behavior without a global flag.
                // Rule reapplication must not remove these windows from their groups.
                if matches!(
                    self.workspaces[target_workspace].layout_system,
                    LayoutSystemKind::Floating(_)
                ) {
                    self.floating.remove_floating(wid);
                }
                let should_float = self.floating.is_floating(wid);
                let relocated = source_workspace
                    .filter(|&source| source != target_workspace)
                    .is_some_and(|source| {
                        self.relocate_window_to_workspace(
                            window_store,
                            source,
                            (space, target_workspace),
                            wid,
                            WindowRelocation::Observed,
                        )
                    });
                let mut membership_changed = relocated;
                if source_workspace.is_some() {
                    if should_float && !relocated {
                        membership_changed |= self.workspace_contains_window(target_workspace, wid);
                        self.workspaces[target_workspace]
                            .layout_system
                            .update(WindowChange::Remove(wid));
                    }
                } else {
                    // Initial discovery and snapshot recovery have no authoritative source
                    // projection. Keep the defensive scrub at that boundary only.
                    let ids: Vec<_> = self.workspaces.catalog().collect();
                    for id in ids {
                        if !should_float && id == target_workspace {
                            continue;
                        }
                        membership_changed |= self.workspace_contains_window(id, wid);
                        self.workspaces[id].layout_system.update(WindowChange::Remove(wid));
                    }
                }

                if !relocated {
                    self.floating.remove_active_for_window(wid);
                    let previous_selection = self
                        .workspaces
                        .active_layout(space, target_workspace)
                        .filter(|_| !should_float && !should_focus)
                        .and_then(|layout| {
                            self.workspaces[target_workspace].layout_system.selected_window(layout)
                        });
                    let contained =
                        self.active_workspace_contains_window(space, target_workspace, wid);
                    membership_changed |= self.add_window_to_layout(window_store, space, wid);
                    membership_changed |= !contained
                        && self.active_workspace_contains_window(space, target_workspace, wid);
                    if let (Some(previous), Some(layout)) = (
                        previous_selection,
                        self.workspaces.active_layout(space, target_workspace),
                    ) && self.workspaces[target_workspace]
                        .layout_system
                        .contains_window(layout, previous)
                    {
                        self.workspaces[target_workspace]
                            .layout_system
                            .focus(layout, Focus::Select(previous))
                            .handled;
                    }
                }
                membership_changed |=
                    self.projected_space_in(target_workspace, wid) != projected_space_before;

                if let Some((window, workspace)) = focus_request {
                    if self.workspaces.active_workspace(space) != Some(workspace) {
                        app_rule_outcome.workspace_focus =
                            Some(AppRuleWorkspaceFocus { window, space, workspace });
                        return EventResponse {
                            changed: membership_changed,
                            ..EventResponse::default()
                        };
                    }
                    self.commit_workspace_focus(window_store, space, Some(window));
                    return EventResponse {
                        changed: membership_changed,
                        raise_windows: vec![window],
                        focus_window: Some(window),

                        ..EventResponse::default()
                    };
                }
                if membership_changed {
                    return EventResponse {
                        changed: true,
                        ..EventResponse::default()
                    };
                }
            }
            LayoutEvent::AppClosed(pid) => {
                for (system, _) in self.native_layouts.values_mut() {
                    system.update(WindowChange::CloseApp(pid));
                }
                let changed = self.workspaces.layout_spaces().into_iter().any(|space| {
                    self.workspaces.active_layout_for_space(space).is_some_and(
                        |(workspace, layout)| {
                            self.workspaces[workspace]
                                .layout_system
                                .has_windows_for_app(layout, pid)
                        },
                    )
                });
                if self.focused_window.is_some_and(|wid| wid.pid == pid) {
                    self.focused_window = None;
                }
                for (_, ws) in self.workspaces.workspaces.iter_mut() {
                    ws.layout_system.update(WindowChange::CloseApp(pid));
                    if ws.last_focused().is_some_and(|wid| wid.pid == pid) {
                        ws.set_last_focused(None);
                    }
                }
                self.evicted.retain(|wid, _| wid.pid != pid);
                self.prune_native_layouts();
                self.floating.remove_all_for_pid(pid);
                self.forget_persisted_app(pid);

                self.floating_positions.remove_app(pid);
                return EventResponse {
                    changed,
                    ..EventResponse::default()
                };
            }
            LayoutEvent::WindowAdded(space, wid) => {
                self.debug_tree(space);
                return EventResponse {
                    changed: self.add_window_to_layout(window_store, space, wid),
                    ..EventResponse::default()
                };
            }
            LayoutEvent::WindowRemoved(wid) => {
                self.forget_native_layout_for_window_id(wid);
                return EventResponse {
                    changed: self.remove_window_internal(window_store, wid, false),
                    ..EventResponse::default()
                };
            }
            LayoutEvent::WindowSuspended(wid) => {
                return EventResponse {
                    changed: self.suspend_window(window_store, wid),
                    ..EventResponse::default()
                };
            }
            LayoutEvent::WindowRemovedPreserveFloating(wid) => {
                return EventResponse {
                    changed: self.remove_window_internal(window_store, wid, true),
                    ..EventResponse::default()
                };
            }
            LayoutEvent::WindowFocused(space, wid) => {
                if window_store.is_temporarily_unavailable(wid) {
                    return EventResponse::default();
                }
                if self.floating.is_floating(wid) {
                    self.focused_window = Some(wid);
                    self.floating.set_last_focus(Some(wid));
                } else if let Some((ws_id, layout)) = self.workspaces.active_layout_for_space(space)
                {
                    if !self.workspaces[ws_id].layout_system.contains_window(layout, wid) {
                        warn!(
                            "WindowFocused ignored: wid={:?} not in active layout for space {:?}",
                            wid, space
                        );
                        return EventResponse::default();
                    }
                    let preserve_selection = self.focused_window == Some(wid)
                        && self.workspaces[ws_id]
                            .layout_system
                            .selection_contains_window(layout, wid);
                    self.focused_window = Some(wid);
                    let selection_changed =
                        self.workspaces[ws_id].layout_system.selected_window(layout) != Some(wid);
                    if !preserve_selection {
                        let _ = self.workspaces[ws_id]
                            .layout_system
                            .focus(layout, Focus::Select(wid))
                            .handled;
                    }
                    self.workspaces.set_last_focused_window(space, ws_id, Some(wid));
                    return EventResponse {
                        changed: selection_changed
                            && matches!(
                                self.active_layout_mode_at(space),
                                LayoutMode::Scrolling | LayoutMode::Floating
                            ),
                        ..EventResponse::default()
                    };
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
                // Native departure no longer suspends logical availability. Its
                // ordered-out transition frames still cannot edit the saved split.
                if window_store
                    .record(wid)
                    .is_some_and(|r| r.visibility() == WindowVisibility::Hidden)
                {
                    return EventResponse::default();
                }
                self.window_layout_constraints.set_width(wid, new_frame.size.width);
                for (space, screen_frame, display_uuid) in screens.iter() {
                    let Some((ws_id, layout)) = self.workspaces.active_layout_for_space(*space)
                    else {
                        debug!(
                            "No active workspace/layout for resized window {:?} on space {:?}; skipping",
                            wid, space
                        );
                        continue;
                    };
                    let gaps =
                        self.layout_settings.gaps.effective_for_display(display_uuid.as_deref());
                    self.workspaces[ws_id].layout_system.update(WindowChange::Resize {
                        layout,
                        window: wid,
                        old: old_frame,
                        new: new_frame,
                        screen: *screen_frame,
                        gaps: &gaps,
                    });
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
        visible_space_frames: &HashMap<SpaceId, CGRect>,
        command: LayoutCommand,
    ) -> EventResponse {
        if !matches!(
            command,
            LayoutCommand::NextWindow
                | LayoutCommand::PrevWindow
                | LayoutCommand::MoveFocus(_)
                | LayoutCommand::ToggleFocusFloating
                | LayoutCommand::Ascend
                | LayoutCommand::Descend
                | LayoutCommand::ScrollStrip { .. }
                | LayoutCommand::MoveStackTab { .. }
                | LayoutCommand::NextWorkspace(_)
                | LayoutCommand::PrevWorkspace(_)
                | LayoutCommand::SwitchToWorkspace(_)
                | LayoutCommand::SwitchToLastWorkspace
                | LayoutCommand::CreateWorkspace
                | LayoutCommand::SetWorkspaceLayout { .. }
                | LayoutCommand::SnapStrip
                | LayoutCommand::CenterSelection
                | LayoutCommand::RotateMasterStackNext
                | LayoutCommand::RotateMasterStackPrev
        ) && let Some(workspace) =
            space.and_then(|space| self.workspaces.active_workspace(space))
        {
            self.forget_native_layout_for_workspace(workspace);
        }
        if !matches!(command, LayoutCommand::ScrollStrip { .. }) {
            self.scroll_boundary = None;
        }
        if let Some(space) = space {
            if let Some(ws_id) = self.workspaces.active_workspace(space) {
                if let Some(layout) = self.workspaces.active_layout(space, ws_id) {
                    debug!(
                        "Tree:\n{}",
                        self.workspaces[ws_id].layout_system.draw_tree(layout).trim()
                    );
                    debug!(selection_window = ?self.workspaces[ws_id].layout_system.selected_window(layout));
                } else {
                    debug!("No active layout for workspace {:?} on space {:?}", ws_id, space);
                }
            } else {
                debug!("No active workspace for space {:?}", space);
            }
        }
        if let LayoutCommand::MoveStackTab { window, target } = command {
            // The stack line belongs to whichever display it was dragged on, not the
            // command space, so find the layout that holds the window.
            let window = crate::actor::app::WindowId::new(window.pid, window.idx);
            let target = crate::actor::app::WindowId::new(target.pid, target.idx);
            self.forget_native_layout_for_window(window_store, window);
            let changed = visible_spaces.iter().any(|&space| {
                self.workspaces.active_layout_for_space(space).is_some_and(|(ws_id, layout)| {
                    let system = &mut self.workspaces[ws_id].layout_system;
                    system.contains_window(layout, window)
                        && system.edit(layout, LayoutEdit::ReorderTab { window, target }).changed()
                })
            });
            return EventResponse {
                changed,
                ..EventResponse::default()
            };
        }
        let is_floating = if let Some(focus) = self.focused_window {
            self.floating.is_floating(focus)
        } else {
            false
        };
        debug!(?self.focused_window, last_floating_focus=?self.floating.last_focus(), ?is_floating);

        let floating_options = match &command {
            LayoutCommand::ToggleWindowFloating => Some(ToggleWindowFloatingOptions::default()),
            LayoutCommand::ToggleWindowFloatingWithOptions(options) => Some(*options),
            _ => None,
        };
        if let Some(options) = floating_options {
            if let Some(window) = self.focused_window {
                window_store.clear_floating_display_geometry(window);
            }
            if space.is_some_and(|space| self.active_layout_mode_at(space) == LayoutMode::Floating)
            {
                return EventResponse::default();
            }
            let Some(wid) = self.focused_window else {
                return EventResponse::default();
            };
            if is_floating {
                if let Some(space) = space {
                    let Some(assigned_workspace) = self
                        .workspaces()
                        .workspace_for_window(&window_store, space, wid)
                        .or_else(|| self.workspaces.active_workspace(space))
                    else {
                        return EventResponse::default();
                    };

                    if let Some(layout) = self.workspaces.active_layout(space, assigned_workspace) {
                        self.workspaces[assigned_workspace]
                            .layout_system
                            .update(WindowChange::Add(layout, wid, None));
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
                    if let Some((ws_id, _)) = self.workspaces.active_layout_for_space(space) {
                        self.workspaces[ws_id].layout_system.update(WindowChange::Remove(wid));
                        if options != ToggleWindowFloatingOptions::default()
                            && let (Some(center), Some(size), Some(current)) = (
                                visible_space_frames.get(&space).map(|frame| frame.mid()),
                                self.workspaces[ws_id].layout_state.active_size(),
                                window_store.window(wid).map(|window| window.frame_monotonic),
                            )
                        {
                            let screen = CGRect::new(
                                CGPoint::new(
                                    center.x - size.width / 2.0,
                                    center.y - size.height / 2.0,
                                ),
                                size,
                            );
                            let frame = requested_floating_frame(
                                current,
                                screen,
                                options.center,
                                options.size,
                            );
                            self.floating_positions.store(ws_id, wid, frame);
                        }
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
                        .workspaces()
                        .workspace_for_window(&window_store, space, wid)
                        .or_else(|| self.workspaces.active_workspace(space));
                    if let (Some(ws), Some(frame)) =
                        (ws, window_store.window(wid).map(|w| w.frame_monotonic))
                    {
                        self.floating_positions.store(ws, wid, frame);
                    }
                }
                self.floating.set_fullscreen(wid, Some(target));
            }
            return EventResponse {
                changed: true,
                raise_windows: vec![wid],
                focus_window: Some(wid),

                ..EventResponse::default()
            };
        }

        let Some(space) = space else {
            return EventResponse::default();
        };
        let workspace_id = match self.workspaces.active_workspace(space) {
            Some(id) => id,
            None => {
                warn!("No active virtual workspace for space {:?}", space);
                return EventResponse::default();
            }
        };
        let layout = match self.workspaces.active_layout(space, workspace_id) {
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
            let (mut raise_windows, selection) = if is_floating {
                let system = &self.workspaces[workspace_id].layout_system;
                (
                    system.windows(layout, WindowQuery::Visible),
                    system.selected_window(layout),
                )
            } else {
                let selection = self.floating.last_focus();
                let windows = self
                    .active_floating_windows_in_workspace(window_store, space)
                    .into_iter()
                    .filter(|wid| Some(*wid) != selection)
                    .collect();
                (windows, selection)
            };
            let focus_window = selection.or_else(|| raise_windows.pop());
            let response = EventResponse {
                changed: true,
                raise_windows,
                focus_window,
                ..Default::default()
            };
            self.apply_focus_response(window_store, space, workspace_id, layout, &response);
            return response;
        }

        match command {
            LayoutCommand::ToggleWindowFloating
            | LayoutCommand::ToggleWindowFloatingWithOptions(_) => {
                unreachable!()
            }
            LayoutCommand::ToggleFocusFloating => unreachable!(),
            LayoutCommand::MoveStackTab { .. } => unreachable!(),

            LayoutCommand::SwapWindows(a, b) => {
                let a = crate::actor::app::WindowId::new(a.pid, a.idx);
                let b = crate::actor::app::WindowId::new(b.pid, b.idx);
                self.forget_native_layout_for_window_id(a);
                self.forget_native_layout_for_window_id(b);
                let _ = self.workspaces[workspace_id]
                    .layout_system
                    .edit(layout, LayoutEdit::Swap(a, b))
                    .changed();

                EventResponse::default()
            }
            LayoutCommand::NextWindow | LayoutCommand::PrevWindow => {
                let forward = matches!(command, LayoutCommand::NextWindow);
                let windows = if let LayoutSystemKind::Floating(system) =
                    &mut self.workspaces[workspace_id].layout_system
                {
                    system.cycle_windows(layout)
                } else if is_floating {
                    self.active_floating_windows_in_workspace(window_store, space)
                } else {
                    self.filter_active_workspace_windows(
                        window_store,
                        space,
                        self.workspaces[workspace_id]
                            .layout_system
                            .windows(layout, WindowQuery::Visible),
                    )
                };
                let focus_window = if let Some(idx) =
                    windows.iter().position(|&w| Some(w) == self.focused_window)
                {
                    let next = if forward {
                        (idx + 1) % windows.len()
                    } else {
                        (idx + windows.len() - 1) % windows.len()
                    };
                    Some(windows[next])
                } else {
                    self.workspaces[workspace_id]
                        .layout_system
                        .selected_window(layout)
                        .filter(|wid| windows.contains(wid))
                        .or_else(|| windows.first().copied())
                };
                let response = EventResponse {
                    changed: true,
                    focus_window,
                    raise_windows: focus_window.into_iter().collect(),
                    ..Default::default()
                };
                self.apply_focus_response(window_store, space, workspace_id, layout, &response);
                return response;
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
                    visible_space_frames,
                    direction,
                    is_floating,
                );
            }
            LayoutCommand::Ascend | LayoutCommand::Descend => {
                let focus = if matches!(command, LayoutCommand::Descend) {
                    Focus::Descend
                } else if is_floating {
                    return EventResponse::default();
                } else {
                    Focus::Ascend
                };
                let changed =
                    self.workspaces[workspace_id].layout_system.focus(layout, focus).handled;
                EventResponse { changed, ..Default::default() }
            }
            LayoutCommand::MoveNode(direction) => {
                let adjacent = self
                    .next_space_for_direction(
                        space,
                        direction,
                        visible_spaces,
                        visible_space_frames,
                    )
                    .and_then(|space| {
                        let (workspace, layout) = self.workspaces.active_layout_for_space(space)?;
                        Some((space, workspace, layout))
                    });
                let changed = self.workspaces[workspace_id]
                    .layout_system
                    .edit(layout, LayoutEdit::Move {
                        direction,
                        has_display_neighbor: adjacent.is_some(),
                    })
                    .changed();
                let mut response = EventResponse {
                    changed,
                    ..EventResponse::default()
                };
                if !changed && let Some((space, target_workspace, _)) = adjacent {
                    let mut windows = self.workspaces[workspace_id]
                        .layout_system
                        .windows(layout, WindowQuery::Selection);
                    let target_system = &self.workspaces[target_workspace].layout_system;
                    // Preserve subtree order when the destination prepends members.
                    if matches!(target_system, LayoutSystemKind::Traditional(_))
                        && matches!(direction, Direction::Right | Direction::Down)
                    {
                        windows.reverse();
                    }
                    for wid in windows {
                        if self.relocate_window_to_workspace(
                            window_store,
                            workspace_id,
                            (space, target_workspace),
                            wid,
                            WindowRelocation::Directional(direction),
                        ) {
                            response.transferred_windows.push(wid);
                        }
                    }
                }
                response
            }
            LayoutCommand::ToggleFullscreen | LayoutCommand::ToggleFullscreenWithinGaps => {
                let within_gaps = matches!(command, LayoutCommand::ToggleFullscreenWithinGaps);
                let windows = self.workspaces[workspace_id]
                    .layout_system
                    .edit(layout, LayoutEdit::Fullscreen { within_gaps })
                    .into_windows();
                Self::response_for_raised_windows(windows)
            }
            // handled by upper reactor
            LayoutCommand::NextWorkspace(_)
            | LayoutCommand::PrevWorkspace(_)
            | LayoutCommand::SwitchToWorkspace(_)
            | LayoutCommand::MoveWindowToWorkspace { .. }
            | LayoutCommand::SetWorkspaceLayout { .. }
            | LayoutCommand::CreateWorkspace
            | LayoutCommand::SwitchToLastWorkspace => EventResponse::default(),
            LayoutCommand::JoinWindow(_)
            | LayoutCommand::ConsumeOrExpelWindow(_)
            | LayoutCommand::UnjoinWindows
            | LayoutCommand::Balance => {
                let edit = match command {
                    LayoutCommand::JoinWindow(direction) => {
                        LayoutEdit::Group(GroupEdit::Join(direction))
                    }
                    LayoutCommand::ConsumeOrExpelWindow(direction) => {
                        LayoutEdit::Group(GroupEdit::ConsumeOrExpel(direction))
                    }
                    LayoutCommand::UnjoinWindows => LayoutEdit::Group(GroupEdit::Unjoin),
                    _ => LayoutEdit::Rebalance,
                };
                self.workspaces[workspace_id].layout_system.edit(layout, edit).discard();
                EventResponse::default()
            }
            LayoutCommand::ToggleStack => self.toggle_stack_for_workspace(
                workspace_id,
                layout,
                self.layout_settings.stack.default_orientation,
            ),
            LayoutCommand::ToggleOrientation => {
                let default_orientation = self.layout_settings.stack.default_orientation;
                Self::toggle_orientation_for_system(
                    &mut self.workspaces[workspace_id].layout_system,
                    layout,
                    default_orientation,
                )
            }
            LayoutCommand::ResizeWindowGrow(_)
            | LayoutCommand::ResizeWindowShrink(_)
            | LayoutCommand::ResizeWindowBy { .. } => {
                if is_floating {
                    return EventResponse::default();
                }
                let (amount, orientation) = match command {
                    LayoutCommand::ResizeWindowGrow(orientation) => (0.05, orientation),
                    LayoutCommand::ResizeWindowShrink(orientation) => (-0.05, orientation),
                    LayoutCommand::ResizeWindowBy { amount } => {
                        (amount, ResizeOrientation::Horizontal)
                    }
                    _ => unreachable!(),
                };
                self.workspaces[workspace_id]
                    .layout_system
                    .edit(layout, LayoutEdit::Resize { amount, orientation })
                    .discard();
                EventResponse::default()
            }
            LayoutCommand::AdjustMasterRatio(_)
            | LayoutCommand::AdjustMasterCount { .. }
            | LayoutCommand::PromoteToMaster
            | LayoutCommand::SwapMasterStack => {
                if let LayoutSystemKind::MasterStack(system) =
                    &mut self.workspaces[workspace_id].layout_system
                {
                    match command {
                        LayoutCommand::AdjustMasterRatio(delta) => {
                            system.adjust_master_ratio(layout, delta)
                        }
                        LayoutCommand::AdjustMasterCount { delta } => {
                            system.adjust_master_count(layout, delta)
                        }
                        LayoutCommand::PromoteToMaster => system.promote_to_master(layout),
                        LayoutCommand::SwapMasterStack => system.swap_master_stack(layout),
                        _ => unreachable!(),
                    }
                }
                EventResponse::default()
            }
            LayoutCommand::RotateMasterStackNext | LayoutCommand::RotateMasterStackPrev => {
                let backwards = matches!(command, LayoutCommand::RotateMasterStackPrev);
                let changed = match &mut self.workspaces[workspace_id].layout_system {
                    LayoutSystemKind::MasterStack(system) => {
                        system.rotate_master_stack(layout, backwards)
                    }
                    _ => false,
                };
                if changed {
                    self.forget_native_layout_for_workspace(workspace_id);
                }
                EventResponse {
                    changed,
                    ..EventResponse::default()
                }
            }
            LayoutCommand::ScrollStrip { .. }
            | LayoutCommand::SnapStrip
            | LayoutCommand::SwitchPresetColumnWidth { .. }
            | LayoutCommand::CenterSelection => {
                let LayoutSystemKind::Scrolling(system) =
                    &mut self.workspaces[workspace_id].layout_system
                else {
                    return EventResponse::default();
                };
                let mut response = EventResponse::default();
                match command {
                    LayoutCommand::ScrollStrip { delta } => {
                        if let Some((direction, excess)) = system.scroll_by_delta(layout, delta) {
                            let key = (workspace_id, layout, direction);
                            let accumulated = self
                                .scroll_boundary
                                .filter(|(previous, _)| *previous == key)
                                .map_or(excess, |(_, amount)| amount + excess);
                            let boundary = accumulated
                                >= self
                                    .layout_settings
                                    .scrolling
                                    .gestures
                                    .workspace_switch_threshold;
                            self.scroll_boundary = (!boundary).then_some((key, accumulated));
                            response.boundary_hit = boundary.then_some(direction);
                        } else {
                            self.scroll_boundary = None;
                        }
                    }
                    LayoutCommand::SnapStrip => {
                        response.focus_window = system.snap_to_nearest_column(layout);
                        response.changed = response.focus_window.is_some();
                    }
                    LayoutCommand::SwitchPresetColumnWidth { backwards } => {
                        response.changed = system.switch_preset_column_width(layout, backwards);
                    }
                    LayoutCommand::CenterSelection => system.center_selected_column(layout),
                    _ => unreachable!(),
                }
                response
            }
        }
    }

    pub fn calculate_layout(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let Some((workspace_id, layout)) = self.workspaces.active_layout_for_space(space) else {
            return Vec::new();
        };
        self.window_layout_constraints.prepare(
            &mut self.workspaces[workspace_id].layout_system,
            layout,
            screen,
            gaps,
        );
        self.calculate_arrangement(workspace_id, layout, screen, gaps, stack_line)
    }

    pub fn calculate_layout_with_virtual_workspaces<F>(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        get_window_frame: F,
        all_screens: &[CGRect],
    ) -> Vec<(WindowId, CGRect)>
    where
        F: Fn(WindowId) -> Option<CGRect>,
    {
        self.calculate_layout_with_groups(
            window_store,
            space,
            screen,
            gaps,
            stack_line,
            get_window_frame,
            all_screens,
            None,
            false,
        )
    }

    pub(crate) fn calculate_layout_with_groups<F>(
        &mut self,
        window_store: &WindowStore,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        get_window_frame: F,
        all_screens: &[CGRect],
        groups: Option<&mut Vec<GroupContainerInfo>>,
        finish_scrolling: bool,
    ) -> Vec<(WindowId, CGRect)>
    where
        F: Fn(WindowId) -> Option<CGRect>,
    {
        use crate::model::{HiddenWindowPlacement, HideCorner};
        let mut positions = HashMap::default();
        let center = screen.mid();
        let floating_frame = |window, candidate: Option<CGRect>| {
            candidate
                .filter(|rect| !HiddenWindowPlacement::is_hidden(screen, *rect, all_screens))
                .unwrap_or_else(|| {
                    let size = get_window_frame(window)
                        .map_or(CGSize::new(500.0, 500.0), |frame| frame.size);
                    CGRect::new(
                        CGPoint::new(center.x - size.width / 2.0, center.y - size.height / 2.0),
                        size,
                    )
                })
        };

        if let Some(active_workspace_id) = self.workspaces.active_workspace(space) {
            if let Some(layout) = self.workspaces.active_layout(space, active_workspace_id) {
                let constraints = &self.window_layout_constraints.windows;
                if let LayoutSystemKind::Floating(system) =
                    &mut self.workspaces[active_workspace_id].layout_system
                {
                    system.initialize_frames(layout, screen, constraints, &|wid| {
                        get_window_frame(wid).filter(|frame| {
                            !HiddenWindowPlacement::is_hidden(screen, *frame, all_screens)
                        })
                    });
                }
                let system = &self.workspaces[active_workspace_id].layout_system;
                let frames = groups.and_then(|groups| {
                    let offset = self.layout_settings.stack.stack_offset;
                    if let Some(frames) = system.project_groups(
                        layout,
                        screen,
                        offset,
                        constraints,
                        gaps,
                        stack_line,
                        true,
                        groups,
                        false,
                    ) {
                        return Some(frames);
                    }
                    *groups = system.collect_group_containers(
                        layout,
                        screen,
                        offset,
                        constraints,
                        gaps,
                        stack_line,
                        false,
                    );
                    None
                });
                positions.extend(frames.unwrap_or_else(|| {
                    let system = &mut self.workspaces[active_workspace_id].layout_system;
                    self.window_layout_constraints.prepare(system, layout, screen, gaps);
                    if finish_scrolling && let LayoutSystemKind::Scrolling(scrolling) = system {
                        scrolling.finish_viewport(layout);
                    }
                    self.calculate_arrangement(
                        active_workspace_id,
                        layout,
                        screen,
                        gaps,
                        stack_line,
                    )
                }));
            }

            // Look up this workspace's frames directly. Scanning the entire
            // floating-position store makes every switch depend on windows in
            // unrelated workspaces (and displays).
            for window_id in window_store.workspace_windows(active_workspace_id) {
                if self.floating.is_floating(window_id)
                    && let Some(stored_position) =
                        self.floating_positions.get(active_workspace_id, window_id)
                {
                    let frame = floating_frame(window_id, Some(stored_position));
                    positions.insert(window_id, frame);
                    self.floating_positions.store(active_workspace_id, window_id, frame);
                }
            }

            let floating_windows = self.active_floating_windows_in_workspace(window_store, space);
            for wid in floating_windows {
                let frame = floating_frame(wid, positions.get(&wid).copied());
                positions.insert(wid, frame);
                self.floating_positions.store(active_workspace_id, wid, frame);
            }

            for (wid, frame) in &mut positions {
                if let Some(kind) = self.floating.fullscreen_kind(*wid) {
                    *frame = match kind {
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
                }
            }
        }

        let hidden_windows = self.workspaces.windows_in_inactive_workspaces(window_store, space);
        for wid in hidden_windows {
            let original_frame = get_window_frame(wid);

            if self.floating.is_floating(wid)
                && let Some(workspace_id) =
                    self.workspaces().workspace_for_window(window_store, space, wid)
                && self.floating_positions.get(workspace_id, wid).is_none()
            {
                let candidate = original_frame.or_else(|| positions.get(&wid).copied());
                self.floating_positions.store_if_absent(
                    workspace_id,
                    wid,
                    floating_frame(wid, candidate),
                );
            }

            let reference_frame = original_frame
                .unwrap_or_else(|| CGRect::new(screen.origin, CGSize::new(500.0, 500.0)));
            let hidden_rect = HiddenWindowPlacement::calculate(
                screen,
                reference_frame,
                HideCorner::BottomRight,
                all_screens,
            );
            positions.insert(wid, hidden_rect);
        }

        // Membership survives native fullscreen; geometry never targets it.
        positions
            .into_iter()
            .filter(|(wid, _)| !window_store.is_temporarily_unavailable(*wid))
            .collect()
    }

    pub fn collect_group_containers_in_selection_path(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<GroupContainerInfo> {
        self.collect_group_containers_for_space(space, screen, gaps, stack_line, true)
    }

    pub fn active_workspace_for_space_has_fullscreen(&mut self, space: SpaceId) -> bool {
        let Some((ws_id, layout_id)) = self.workspaces.active_layout_for_space(space) else {
            return false;
        };
        self.workspaces[ws_id].layout_system.has_any_fullscreen_node(layout_id)
    }

    pub fn workspace_has_fullscreen(
        &self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
        windows: &[WindowId],
    ) -> bool {
        if windows.is_empty() {
            return false;
        }
        self.workspaces.active_layout(space, workspace).is_some_and(|layout_id| {
            self.workspaces[workspace].layout_system.has_any_fullscreen_node(layout_id)
        }) || windows.iter().any(|wid| self.floating.fullscreen_kind(*wid).is_some())
    }

    pub fn collect_group_containers(
        &mut self,
        space: SpaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<GroupContainerInfo> {
        self.collect_group_containers_for_space(space, screen, gaps, stack_line, false)
    }

    /// Scrolling strips expose their calculated coordinates before native offscreen parking.
    pub(crate) fn logical_window_frames(
        &self,
        space: SpaceId,
        workspace: VirtualWorkspaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) -> HashMap<WindowId, CGRect> {
        match (
            &self.workspaces[workspace].layout_system,
            self.workspaces.active_layout(space, workspace),
        ) {
            (LayoutSystemKind::Scrolling(system), Some(layout)) => system
                .calculate_frames(
                    layout,
                    screen,
                    &self.window_layout_constraints.windows,
                    Some(self.window_layout_constraints.revision),
                    gaps,
                    false,
                )
                .into_iter()
                .collect(),
            _ => HashMap::default(),
        }
    }

    pub fn calculate_layout_for_workspace(
        &self,
        window_store: &WindowStore,
        space: SpaceId,
        workspace_id: crate::model::VirtualWorkspaceId,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let mut positions: HashMap<_, _> = self
            .calculate_workspace_layout(space, workspace_id, screen, gaps, stack_line)
            .into_iter()
            .collect();

        let floating_positions = self.floating_positions.workspace_positions(workspace_id);
        for (window_id, stored_position) in floating_positions {
            if self.floating.is_floating(window_id)
                && window_store.workspace_for_window(window_id) == Some(workspace_id)
            {
                positions.insert(window_id, stored_position);
            }
        }

        positions.into_iter().collect()
    }

    pub(crate) fn selected_window(&mut self, space: SpaceId) -> Option<WindowId> {
        let (ws_id, layout) = self.workspaces.active_layout_for_space(space)?;
        self.workspaces[ws_id].layout_system.selected_window(layout)
    }

    pub(crate) fn handle_virtual_workspace_command<'a>(
        &mut self,
        window_store: &mut WindowStore,
        context: impl Into<crate::model::virtual_workspace::WorkspaceContext<'a>>,
        command: &LayoutCommand,
    ) -> EventResponse {
        let context = context.into();
        let space = context.space;
        match command {
            LayoutCommand::NextWorkspace(skip_empty) | LayoutCommand::PrevWorkspace(skip_empty) => {
                let Some(current) = self.workspaces.active_workspace(space) else {
                    return EventResponse::default();
                };
                let direction = if matches!(command, LayoutCommand::NextWorkspace(_)) {
                    Direction::Right
                } else {
                    Direction::Left
                };
                let target = self.workspaces.step_workspace(
                    window_store,
                    context,
                    current,
                    *skip_empty,
                    direction,
                );
                // Global targets on another display are shown there and focus follows.
                if let Some(target) = target.filter(|&target| target != current) {
                    return self.switch_to_workspace_id(window_store, context, target, None);
                }
                EventResponse::default()
            }
            LayoutCommand::SwitchToWorkspace(workspace_index) => {
                let Some(target) = self
                    .workspaces
                    .resolve_workspace(context, &WorkspaceSelector::Index(*workspace_index))
                else {
                    return EventResponse::default();
                };
                self.switch_to_workspace_id(window_store, context, target.id, None)
            }
            LayoutCommand::MoveWindowToWorkspace {
                workspace,
                follow,
                window_id: maybe_id,
            } => {
                let window = match maybe_id {
                    Some(index) => self.workspaces.resolve_window_index(
                        window_store,
                        Some(space),
                        |space| context.visible.contains(&space),
                        *index,
                    ),
                    None => self.focused_window,
                };
                let Some(focused_window) =
                    window.filter(|&window| window_store.can_move_assigned_window(window))
                else {
                    return EventResponse::default();
                };
                let current_workspace_id =
                    window_store.workspace_for_window(focused_window).unwrap();
                let Some(op_space) = self.workspaces.workspace_space(current_workspace_id) else {
                    return EventResponse::default();
                };
                let target_workspace_id = match workspace {
                    WorkspaceSelector::Name(name) if name == "next" || name == "prev" => {
                        let direction = if name == "next" {
                            Direction::Right
                        } else {
                            Direction::Left
                        };
                        self.workspaces.step_workspace(
                            window_store,
                            WorkspaceContext {
                                space: op_space,
                                visible: context.visible,
                            },
                            current_workspace_id,
                            None,
                            direction,
                        )
                    }
                    selector => {
                        self.workspaces.resolve_workspace(context, selector).map(|target| target.id)
                    }
                };
                let Some(target_workspace_id) = target_workspace_id else {
                    return EventResponse::default();
                };

                if current_workspace_id == target_workspace_id {
                    return EventResponse::default();
                }
                let target_space = self.workspaces[target_workspace_id].space;
                if !self.relocate_window_to_workspace(
                    window_store,
                    current_workspace_id,
                    (target_space, target_workspace_id),
                    focused_window,
                    WindowRelocation::Workspace,
                ) {
                    return EventResponse::default();
                }
                let placeable = window_store.can_place_window(focused_window);
                let is_floating = self.floating.is_floating(focused_window);
                if *follow || target_space != op_space {
                    let mut response = if *follow {
                        self.activate_workspace(
                            window_store,
                            target_space,
                            target_workspace_id,
                            placeable.then_some(focused_window),
                        )
                    } else {
                        EventResponse {
                            changed: true,
                            ..Default::default()
                        }
                    };
                    response.changed = true;
                    response.layout_space = Some(op_space);
                    if target_space != op_space {
                        response.transferred_windows.push(focused_window);
                    }
                    return response;
                }

                if !placeable {
                    return EventResponse {
                        changed: true,
                        layout_space: Some(op_space),
                        ..EventResponse::default()
                    };
                }
                let active_workspace = self.workspaces.active_workspace(op_space);

                if Some(target_workspace_id) == active_workspace {
                    if is_floating {
                        self.floating.add_active(op_space, focused_window.pid, focused_window);
                    }
                    return EventResponse {
                        changed: true,
                        layout_space: Some(op_space),
                        focus_window: Some(focused_window),
                        raise_windows: vec![],

                        ..EventResponse::default()
                    };
                } else if Some(current_workspace_id) == active_workspace {
                    self.focused_window = None;
                    self.workspaces.set_last_focused_window(op_space, current_workspace_id, None);

                    let remaining_windows =
                        self.workspaces.windows_in_active_workspace(window_store, op_space);
                    if let Some(&new_focus) = remaining_windows.first() {
                        return EventResponse {
                            changed: true,
                            layout_space: Some(op_space),
                            focus_window: Some(new_focus),
                            raise_windows: vec![],

                            ..EventResponse::default()
                        };
                    }
                }

                self.workspaces.set_last_focused_window(
                    op_space,
                    target_workspace_id,
                    Some(focused_window),
                );

                EventResponse {
                    changed: true,
                    layout_space: Some(op_space),
                    ..EventResponse::default()
                }
            }
            LayoutCommand::CreateWorkspace => match self.workspaces.create_workspace(space, None) {
                Ok(_workspace_id) => {
                    self.sync_scrolling_widths_for_space(space);
                    EventResponse {
                        changed: true,
                        ..EventResponse::default()
                    }
                }
                Err(e) => {
                    warn!("Failed to create new workspace: {:?}", e);
                    EventResponse::default()
                }
            },
            LayoutCommand::SwitchToLastWorkspace => {
                self.switch_to_previous_workspace(window_store, context)
            }
            LayoutCommand::SetWorkspaceLayout { workspace, mode } => {
                let workspace_id = if let Some(index) = workspace {
                    self.workspaces
                        .resolve_workspace(context, &WorkspaceSelector::Index(*index))
                        .map(|target| target.id)
                } else {
                    self.workspaces.active_workspace(space)
                };
                let Some(workspace_id) = workspace_id else {
                    return EventResponse::default();
                };
                let space = self.workspaces[workspace_id].space;

                if !self.switch_workspace_layout_mode(window_store, space, workspace_id, *mode) {
                    return EventResponse::default();
                }

                let is_active_workspace =
                    self.workspaces.active_workspace(space) == Some(workspace_id);
                let raise_windows = if is_active_workspace {
                    self.workspaces.windows_in_active_workspace(window_store, space)
                } else {
                    Vec::new()
                };

                EventResponse {
                    changed: true,
                    layout_space: Some(space),
                    raise_windows,
                    focus_window: if is_active_workspace {
                        self.focused_window
                    } else {
                        None
                    },

                    ..EventResponse::default()
                }
            }
            _ => EventResponse::default(),
        }
    }

    pub fn workspaces(&self) -> &WorkspaceStore { &self.workspaces }

    pub fn workspaces_mut(&mut self) -> &mut WorkspaceStore { &mut self.workspaces }

    pub(crate) fn assign_window_with_app_info<'a>(
        &mut self,
        window_store: &mut WindowStore,
        window_id: WindowId,
        workspace_context: impl Into<crate::model::virtual_workspace::WorkspaceContext<'a>>,
        context: WindowRuleContext<'_>,
        reapply_workspace_rule: bool,
    ) -> Result<Option<AppRuleResult>, crate::model::virtual_workspace::WorkspaceError> {
        let mut decision = {
            let window = window_store.window(window_id).map(|window| &window.info);
            let context = WindowRuleContext {
                window_title: context
                    .window_title
                    .or_else(|| window.map(|info| info.title.as_str())),
                ax_role: context
                    .ax_role
                    .or_else(|| window.and_then(|info| info.ax_role.as_deref())),
                ax_subrole: context
                    .ax_subrole
                    .or_else(|| window.and_then(|info| info.ax_subrole.as_deref())),
                ..context
            };
            self.app_rules.evaluate_observation(context, reapply_workspace_rule, || {
                window
                    .and_then(|info| info.sys_id)
                    .and_then(crate::sys::window_server::window_tags)
            })
        };
        if window_store.is_claimed(window_id) {
            if let Some(window) = window_store.window_mut(window_id) {
                window.manage_override =
                    decision.as_ref().and_then(|rule| rule.management_override());
            }
            return Ok(None);
        }
        let workspace_context = workspace_context.into();
        let space = workspace_context.space;
        let observation =
            window_store.window(window_id).filter(|_| self.is_restoring()).map(|window| {
                (
                    context.window_title.unwrap_or(&window.info.title).to_owned(),
                    window.frame_monotonic.size,
                )
            });
        let restored = observation.is_some_and(|(title, size)| {
            self.observe_window_for_persistence(
                window_store,
                space,
                window_id,
                Some(&title),
                size,
                context.app_bundle_id,
            )
        });
        // A persistence match is an explicit restoration of the user's previous
        // workspace. App rules still control admission and other effects, but
        // their default placement must not relocate the window during restore.
        if restored && let Some(decision) = &mut decision {
            decision.workspace = None;
        }
        if reapply_workspace_rule
            && decision.as_ref().is_some_and(|decision| decision.workspace.is_some())
        {
            self.forget_native_layout_for_window(window_store, window_id);
        }
        self.workspaces
            .apply_app_rule_decision(
                window_store,
                window_id,
                workspace_context,
                decision,
                !reapply_workspace_rule,
            )
            .map(Some)
    }

    pub fn ensure_active_workspace_info(
        &mut self,
        space: SpaceId,
    ) -> Option<(crate::model::VirtualWorkspaceId, String)> {
        if self.workspaces.active_workspace(space).is_none() {
            if let Some(id) = self.workspaces.workspace_id_at(space, Some(0)) {
                self.workspaces.set_active_workspace(space, id);
            }
        }
        self.active_workspace_id_and_name(space)
    }

    pub fn move_window_to_space(
        &mut self,
        window_store: &mut WindowStore,
        source_space: SpaceId,
        target_space: SpaceId,
        target_screen_size: CGSize,
        window_id: WindowId,
    ) -> EventResponse {
        let Some(source_workspace_id) =
            self.workspaces.workspace_for_window(window_store, source_space, window_id)
        else {
            return EventResponse::default();
        };

        let Some(target_workspace_id) = self.workspaces.active_workspace(target_space) else {
            debug!(
                ?window_id,
                ?target_space,
                "Window move ignored: destination has no workspace"
            );
            return EventResponse::default();
        };

        self.ensure_workspace_layouts(target_space, target_screen_size);
        if !self.relocate_window_to_workspace(
            window_store,
            source_workspace_id,
            (target_space, target_workspace_id),
            window_id,
            WindowRelocation::Workspace,
        ) {
            return EventResponse::default();
        }

        if !window_store.can_place_window(window_id) {
            return EventResponse {
                changed: true,
                ..EventResponse::default()
            };
        }
        self.workspaces
            .set_last_focused_window(target_space, target_workspace_id, Some(window_id));
        self.focused_window = Some(window_id);

        EventResponse {
            changed: true,
            raise_windows: vec![window_id],
            focus_window: Some(window_id),

            ..EventResponse::default()
        }
    }

    /// Transfer the workspace object; no window changes logical workspace identity.
    pub fn move_active_workspace_to_space(
        &mut self,
        window_store: &mut WindowStore,
        source_space: SpaceId,
        target_space: SpaceId,
        target_screen_size: CGSize,
    ) -> EventResponse {
        let Some(id) = self.workspaces.active_workspace(source_space) else {
            return EventResponse::default();
        };
        if self.workspaces.effective_mode() == crate::common::config::WorkspaceMode::PerDisplay {
            if source_space == target_space {
                return EventResponse::default();
            }
            let Some(index) = self.workspaces.active_workspace_idx(source_space) else {
                return EventResponse::default();
            };
            let Some(destination) =
                self.workspaces.workspace_id_at(target_space, Some(index as usize))
            else {
                return EventResponse::default();
            };
            let focus = self.preferred_focus_for_workspace(window_store, source_space, id, None);
            let members = window_store.workspace_windows(id);
            self.ensure_workspace_layouts(target_space, target_screen_size);
            let moved: Vec<_> = members
                .into_iter()
                .filter(|&window| {
                    self.relocate_window_to_workspace(
                        window_store,
                        id,
                        (target_space, destination),
                        window,
                        WindowRelocation::Workspace,
                    )
                })
                .collect();
            let changed = self.workspaces.active_workspace(target_space) != Some(destination);
            self.workspaces.set_active_workspace(target_space, destination);
            self.update_active_floating_windows(window_store, target_space);
            return EventResponse {
                changed: changed || !moved.is_empty(),
                workspace_space: Some(target_space),
                focus_window: focus
                    .filter(|window| moved.contains(window))
                    .or_else(|| moved.first().copied()),
                transferred_windows: moved.clone(),
                raise_windows: moved,
                ..EventResponse::default()
            };
        }
        let display = self.space_display_map.get(&target_space).cloned().flatten();
        self.transfer_workspace(
            window_store,
            id,
            target_space,
            target_screen_size,
            display.as_deref(),
            crate::model::virtual_workspace::WorkspaceTransferReason::Explicit,
        )
    }

    pub(crate) fn transfer_workspace(
        &mut self,
        windows: &mut WindowStore,
        id: VirtualWorkspaceId,
        target: SpaceId,
        size: CGSize,
        display: Option<&str>,
        reason: crate::model::virtual_workspace::WorkspaceTransferReason,
    ) -> EventResponse {
        let source = self.workspaces[id].space;
        let source_size = self.workspaces[id].layout_state.active_size();
        let members = windows.workspace_windows(id);
        let focus = self.preferred_focus_for_workspace(windows, source, id, None);
        if !self.workspaces.transfer_workspace(id, target, display, reason) {
            return EventResponse::default();
        }
        self.native_layouts.remove(&id);
        if reason == crate::model::virtual_workspace::WorkspaceTransferReason::Explicit
            && let Some(source_size) = source_size
        {
            self.ensure_workspace_layouts(source, source_size);
        }
        if reason == crate::model::virtual_workspace::WorkspaceTransferReason::Explicit {
            let display = self.display_uuid_for_space(source).map(str::to_owned);
            self.update_space_display(source, display.as_deref());
        }
        self.sync_scrolling_widths_for_space(target);
        self.workspaces[id].ensure_layout_for_size(size);
        for &window in &members {
            self.evicted.remove(&window);
            if self.floating.is_floating(window) {
                self.floating.remove_active_for_window(window);
                self.floating.add_active(target, window.pid, window);
            }
        }

        EventResponse {
            changed: true,
            workspace_space: Some(target),
            raise_windows: members.clone(),
            transferred_windows: if source == target {
                Vec::new()
            } else {
                members
            },
            focus_window: focus,
            ..EventResponse::default()
        }
    }

    fn ensure_workspace_layouts(&mut self, space: SpaceId, screen_size: CGSize) {
        self.workspaces.ensure_layouts_for_size(space, screen_size);
        self.sync_scrolling_widths_for_space(space);
    }

    fn relocate_window_to_workspace(
        &mut self,
        windows: &mut WindowStore,
        source: VirtualWorkspaceId,
        (space, destination): (SpaceId, VirtualWorkspaceId),
        window: WindowId,
        policy: WindowRelocation,
    ) -> bool {
        let Some(source_workspace) = self.workspaces.workspaces.get(source) else {
            return false;
        };
        let Some(target_workspace) = self.workspaces.workspaces.get(destination) else {
            return false;
        };
        let observed = matches!(policy, WindowRelocation::Observed);
        let suspended = windows.is_temporarily_unavailable(window)
            || (matches!(policy, WindowRelocation::Workspace) && !windows.can_place_window(window));
        let owner = windows.workspace_for_window(window);
        // App rules can commit assignment before observation applies the layout change.
        if target_workspace.space != space
            || owner.is_some_and(|owner| owner != source && !(observed && owner == destination))
        {
            return false;
        }

        let source_floating_layout =
            matches!(source_workspace.layout_system, LayoutSystemKind::Floating(_));
        let target_floating_layout =
            matches!(target_workspace.layout_system, LayoutSystemKind::Floating(_));
        let directional = match policy {
            WindowRelocation::Directional(direction) => Some(direction),
            _ => None,
        };
        let preserve_floating = observed || matches!(policy, WindowRelocation::Drop(_));
        let was_floating = if preserve_floating {
            self.is_window_floating(window)
        } else {
            self.floating.is_floating(window)
                && (directional.is_some() || !(source_floating_layout || target_floating_layout))
        };
        let target_layout = self.workspaces.active_layout(space, destination);
        if (directional.is_some() && target_layout.is_none())
            || (owner != Some(destination)
                && !self.workspaces.assign_window_to_workspace(windows, space, window, destination))
        {
            return false;
        }

        self.forget_native_layout_for_window_id(window);
        self.native_layouts.remove(&source);
        self.native_layouts.remove(&destination);
        self.workspaces[source].layout_system.update(WindowChange::Remove(window));
        let project_as_floating = directional.is_none() && was_floating && !target_floating_layout;
        if directional.is_none() {
            self.floating.remove_active_for_window(window);
            if project_as_floating {
                self.floating.add_floating(window);
                self.floating.add_active(space, window.pid, window);
                if observed {
                    self.workspaces[destination].layout_system.update(WindowChange::Remove(window));
                }
            } else {
                self.floating.remove_floating(window);
            }
            if was_floating {
                self.floating_positions.remove_window(window);
                self.floating.set_last_focus(Some(window));
            }
        }

        if !project_as_floating {
            if (!was_floating || directional.is_some())
                && matches!(
                    self.workspaces[destination].layout_system,
                    LayoutSystemKind::Scrolling(_)
                )
                && self.layout_settings.scrolling.preserve_window_sizes
                && target_layout.is_some()
            {
                self.preserve_scrolling_window_width(windows, window);
            }
            if let WindowRelocation::Drop(Some(edited)) = policy {
                self.workspaces[destination].layout_system = edited;
            } else if let Some(layout) = target_layout {
                self.workspaces[destination].layout_system.update(WindowChange::Add(
                    layout,
                    window,
                    directional,
                ));
            }
        }

        if suspended {
            self.suspend_window(windows, window);
        } else if observed {
            self.workspaces[destination]
                .layout_system
                .set_window_participation(window, true);
        }
        if directional.is_none() && self.focused_window == Some(window) {
            self.focused_window = None;
        }
        true
    }

    pub fn workspace_name(
        &self,
        space: SpaceId,
        workspace_id: crate::model::VirtualWorkspaceId,
    ) -> Option<String> {
        self.workspaces.workspace_info(space, workspace_id).map(|ws| ws.name.clone())
    }

    pub fn is_window_floating(&self, window_id: WindowId) -> bool {
        self.floating.is_floating(window_id)
            || self.workspaces.workspaces.values().any(|ws| {
                matches!(&ws.layout_system, LayoutSystemKind::Floating(system) if system.contains_floating_window(window_id))
            })
    }

    /// Requested geometry is presentation intent, never native membership evidence.
    pub(crate) fn prepare_window_placement(
        &mut self,
        windows: &mut WindowStore,
        window: WindowId,
        frame: CGRect,
    ) {
        if self.is_window_floating(window)
            && let Some(assignment) = windows.workspace_for_window(window)
        {
            self.store_floating_position(assignment, window, frame);
        }
    }

    pub fn store_floating_position(
        &mut self,
        workspace: VirtualWorkspaceId,
        window: WindowId,
        frame: CGRect,
    ) {
        if let LayoutSystemKind::Floating(system) = &mut self.workspaces[workspace].layout_system {
            system.store_frame(window, frame);
            return;
        }
        self.floating_positions.store(workspace, window, frame);
    }

    pub fn get_floating_position(
        &self,
        workspace: VirtualWorkspaceId,
        window: WindowId,
    ) -> Option<CGRect> {
        if let LayoutSystemKind::Floating(system) = &self.workspaces[workspace].layout_system {
            return system.frame(window);
        }
        self.floating_positions.get(workspace, window)
    }

    pub fn workspace_floating_positions(
        &self,
        workspace: VirtualWorkspaceId,
    ) -> Vec<(WindowId, CGRect)> {
        self.floating_positions.workspace_positions(workspace)
    }

    pub fn remove_floating_position(&mut self, window: WindowId) {
        self.floating_positions.remove_window(window);
    }

    /// A window's size constraints live as long as the window, so only its retirement drops
    /// them. Leaving a layout doesn't: a move between Spaces arrives as a layout removal.
    pub(crate) fn retire_window(&mut self, wid: WindowId) {
        self.window_layout_constraints.remove(&wid);
    }

    #[cfg(test)]
    pub(crate) fn has_window_constraints(&self, wid: WindowId) -> bool {
        self.window_layout_constraints.windows.contains_key(&wid)
    }

    pub(crate) fn transfer_persistent_window_identity(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }

        self.evicted.remove(&to);
        if let Some(eviction) = self.evicted.remove(&from) {
            self.evicted.insert(to, eviction);
        }
        for (system, _) in self.native_layouts.values_mut() {
            system.update(WindowChange::Remove(to));
            system.update(WindowChange::Replace(from, to));
        }
        // A live `to` identity can already be provisionally present when a saved `from` identity
        // is matched. Remove that projection before replacement; LayoutSystem::replace_window is
        // not required to deduplicate and otherwise the same window can survive in two workspaces.
        self.remove_window_from_all_tiling_trees(to);
        for (_, workspace) in self.workspaces.workspaces.iter_mut() {
            workspace.layout_system.update(WindowChange::Replace(from, to));
        }
        self.workspaces.transfer_window_identity(from, to);
        self.floating_positions.transfer_window_identity(from, to);
        self.floating.transfer_window_identity(from, to);
        if let Some(constraints) = self.window_layout_constraints.remove(&from) {
            self.window_layout_constraints.insert(to, constraints);
        }
        if self.focused_window == Some(from) {
            self.focused_window = Some(to);
        }
    }

    fn update_active_floating_windows(&mut self, window_store: &WindowStore, space: SpaceId) {
        let windows_in_workspace = self
            .workspaces
            .windows_in_active_workspace(window_store, space)
            .into_iter()
            .filter(|wid| !window_store.is_temporarily_unavailable(*wid))
            .collect();
        self.floating.rebuild_active_for_workspace(space, windows_in_workspace);
    }

    pub fn store_floating_window_positions(
        &mut self,
        space: SpaceId,
        floating_positions: &[(WindowId, CGRect)],
    ) {
        if let Some(workspace) = self.workspaces.active_workspace(space) {
            for &(window, frame) in floating_positions {
                self.store_floating_position(workspace, window, frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};

    use super::*;
    use crate::common::collections::HashMap;
    use crate::common::config::{
        LayoutMode, LayoutSettings, VirtualWorkspaceSettings, WorkspaceSelector,
    };
    use crate::layout_engine::systems::{Focus, WindowQuery};

    #[test]
    fn simultaneous_suspensions_keep_workspace_owners_through_inactive_returns_and_mode_changes() {
        for mode in [
            LayoutMode::Traditional,
            LayoutMode::Bsp,
            LayoutMode::Stack,
            LayoutMode::MasterStack,
            LayoutMode::Scrolling,
        ] {
            let mut settings = LayoutSettings::default();
            settings.mode = mode;
            let mut engine = LayoutEngine::new(&VirtualWorkspaceSettings::default(), &settings);
            let mut store = WindowStore::default();
            let space = SpaceId::new(91);
            let _ = engine.handle_event(
                &mut store,
                LayoutEvent::SpaceExposed(space, CGSize::new(1200.0, 800.0)),
            );
            let owner = engine.workspaces.active_workspace(space).unwrap();
            let other = engine.workspaces.list_workspaces(space)[1].0;
            let windows = [WindowId::new(1, 1), WindowId::new(2, 1)];
            for &window in &windows {
                store.insert_window(window, live_window(CGSize::new(500.0, 500.0)));
                assert!(
                    engine.workspaces.assign_window_to_workspace(&mut store, space, window, owner)
                );
                let _ = engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, window));
            }
            assert!(engine.relocate_window_to_workspace(
                &mut store,
                owner,
                (space, other),
                windows[1],
                WindowRelocation::Observed
            ));
            for &window in &windows {
                store.invalidate_ax_identity(window);
                let _ = engine.handle_event(&mut store, LayoutEvent::WindowSuspended(window));
            }
            let _ = engine.handle_event(&mut store, LayoutEvent::WindowFocused(space, windows[0]));
            assert_eq!(
                engine.focused_window(),
                None,
                "stale focus must not reactivate a suspended member"
            );
            assert!(
                engine.native_layouts.is_empty(),
                "suspension never snapshots topology"
            );
            engine.workspaces.set_active_workspace(space, other);
            store.validate_ax_identity(windows[0]);
            let _ = engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, windows[0]));
            assert_eq!(engine.workspaces.active_workspace(space), Some(other));
            assert_eq!(store.workspace_for_window(windows[0]), Some(owner));
            assert!(
                engine.workspaces[owner].layout_system.window_participates(windows[0]),
                "a confirmed AX return restores its slot without selecting its workspace"
            );
            assert!(!engine.workspaces[other].layout_system.window_participates(windows[1]));
            let next_mode = if mode == LayoutMode::Bsp {
                LayoutMode::Traditional
            } else {
                LayoutMode::Bsp
            };
            engine.switch_workspace_layout_mode(&store, space, other, next_mode);
            assert!(!engine.workspaces[other].layout_system.window_participates(windows[1]));
            store.validate_ax_identity(windows[1]);
            let _ = engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, windows[1]));
            assert_eq!(store.workspace_for_window(windows[1]), Some(other));
            store.remove_window(windows[0]);
            let _ = engine.handle_event(&mut store, LayoutEvent::WindowRemoved(windows[0]));
            assert!(
                engine.workspaces[owner]
                    .layout_system
                    .windows(
                        engine.workspaces[owner].active_layout().unwrap(),
                        WindowQuery::All
                    )
                    .is_empty()
            );
            let _ = engine.handle_event(&mut store, LayoutEvent::AppClosed(windows[1].pid));
            assert!(
                engine.workspaces[other]
                    .layout_system
                    .windows(
                        engine.workspaces[other].active_layout().unwrap(),
                        WindowQuery::All
                    )
                    .is_empty()
            );
        }
    }

    fn test_engine() -> LayoutEngine {
        LayoutEngine::new(&VirtualWorkspaceSettings::default(), &LayoutSettings::default())
    }

    fn live_window(size: CGSize) -> crate::model::reactor::WindowState {
        let frame = CGRect::new(CGPoint::ZERO, size);
        let mut state: crate::model::reactor::WindowState =
            crate::actor::reactor::testing::make_window_info(frame, None, "Window", None).into();
        state.is_manageable = true;
        state
    }

    fn window_layout_info(window_id: WindowId, current_size: CGSize) -> WindowLayoutInfo {
        WindowLayoutInfo {
            window_id,
            title: None,
            ax_role: None,
            ax_subrole: None,
            is_resizable: true,
            current_size,
            min_size: None,
            max_size: None,
        }
    }

    fn test_display_frame(center: CGPoint) -> CGRect {
        CGRect::new(
            CGPoint::new(center.x - 500.0, center.y - 500.0),
            CGSize::new(1000.0, 1000.0),
        )
    }

    #[test]
    fn move_node_crosses_display_edge_and_enters_at_near_edge() {
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            for stacked in [false, true] {
                let mut engine = test_engine();
                let mut store = WindowStore::default();
                let source = SpaceId::new(1);
                let target = SpaceId::new(2);
                let size = CGSize::new(1000.0, 1000.0);
                let moving = WindowId::new(1, 2);
                for (space, ids) in [(source, vec![1, 2]), (target, vec![3, 4, 5])] {
                    for &idx in &ids {
                        store.insert_window(WindowId::new(1, idx), live_window(size));
                    }
                    let _ = engine.handle_event(&mut store, LayoutEvent::SpaceExposed(space, size));
                    let _ = engine.handle_event(
                        &mut store,
                        LayoutEvent::windows_observed(
                            space,
                            1,
                            ids.into_iter()
                                .map(|idx| window_layout_info(WindowId::new(1, idx), size))
                                .collect(),
                            None,
                        ),
                    );
                    let (ws, layout) = engine.workspaces.active_layout_for_space(space).unwrap();
                    let LayoutSystemKind::Traditional(system) =
                        &mut engine.workspaces[ws].layout_system
                    else {
                        panic!("expected traditional layout");
                    };
                    if stacked {
                        system
                            .edit(
                                layout,
                                LayoutEdit::Group(GroupEdit::Stack(
                                    crate::common::config::StackDefaultOrientation::Vertical,
                                )),
                            )
                            .discard();
                    } else if matches!(direction, Direction::Up | Direction::Down) {
                        assert!(system.edit(layout, LayoutEdit::ToggleOrientation).changed());
                    }
                    // Select the far side on the destination, so selection-based insertion is wrong.
                    let selected = if space == source {
                        moving
                    } else {
                        WindowId::new(1, 5)
                    };
                    system.focus(layout, Focus::Select(selected)).handled;
                    if space == source
                        && (direction == Direction::Up
                            || (direction == Direction::Left && !stacked))
                    {
                        system.focus(layout, Focus::Select(WindowId::new(1, 1))).handled;
                    }
                }
                let moving =
                    if direction == Direction::Up || (direction == Direction::Left && !stacked) {
                        WindowId::new(1, 1)
                    } else {
                        moving
                    };
                let _ = engine.handle_event(&mut store, LayoutEvent::WindowFocused(source, moving));
                let offset = match direction {
                    Direction::Left => CGPoint::new(-1000.0, 0.0),
                    Direction::Right => CGPoint::new(1000.0, 0.0),
                    Direction::Up => CGPoint::new(0.0, -1000.0),
                    Direction::Down => CGPoint::new(0.0, 1000.0),
                };
                let centers = HashMap::from_iter([
                    (source, test_display_frame(CGPoint::new(0.0, 0.0))),
                    (target, test_display_frame(offset)),
                ]);
                let _ = engine.handle_command(
                    &mut store,
                    Some(source),
                    &[source, target],
                    &centers,
                    LayoutCommand::MoveNode(direction),
                );
                let (source_ws, source_layout) =
                    engine.workspaces.active_layout_for_space(source).unwrap();
                let (target_ws, target_layout) =
                    engine.workspaces.active_layout_for_space(target).unwrap();
                assert!(
                    !engine.workspaces[source_ws]
                        .layout_system
                        .contains_window(source_layout, moving),
                    "{direction:?}, stacked={stacked}"
                );
                assert!(
                    engine.workspaces[target_ws]
                        .layout_system
                        .contains_window(target_layout, moving)
                );
                let gaps = engine.layout_settings.gaps.clone();
                let frames: HashMap<_, _> = engine
                    .calculate_layout(
                        target,
                        CGRect::new(CGPoint::new(0.0, 0.0), size),
                        &gaps,
                        Default::default(),
                    )
                    .into_iter()
                    .collect();
                for other in [3, 4, 5].map(|idx| WindowId::new(1, idx)) {
                    let a = frames[&moving];
                    let b = frames[&other];
                    assert!(
                        match direction {
                            Direction::Right => a.max().x <= b.min().x,
                            Direction::Left => a.min().x >= b.max().x,
                            Direction::Down => a.max().y <= b.min().y,
                            Direction::Up => a.min().y >= b.max().y,
                        },
                        "{direction:?}, stacked={stacked}: {a:?} vs {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn observing_one_window_cannot_rewrite_sibling_topology() {
        let mut window_store = WindowStore::default();
        let mut engine = test_engine();
        let space = SpaceId::new(94);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 1000.0));
        let pid: pid_t = 5153;
        let w1 = WindowId::new(pid, 1);
        let w2 = WindowId::new(pid, 2);
        let size = CGSize::new(500.0, 500.0);
        for window in [w1, w2] {
            window_store.insert_window(window, live_window(size));
        }
        let info = |wid| window_layout_info(wid, size);

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::windows_observed(space, pid, vec![info(w1), info(w2)], None),
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
        let before = engine.calculate_layout(space, screen, &gaps, Default::default());

        // A partial AX inventory observes w1 but says nothing about w2.
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::windows_observed(space, pid, vec![info(w1)], None),
        );

        assert_eq!(
            engine.calculate_layout(space, screen, &gaps, Default::default()),
            before,
            "partial discovery must not remove an assigned window or reset its split"
        );
    }

    #[test]
    fn drag_preview_matches_committed_geometry_with_asymmetric_constraints() {
        use crate::layout_engine::{WindowDropAction, WindowDropRequest};

        for mode in [
            LayoutMode::Traditional,
            LayoutMode::Bsp,
            LayoutMode::MasterStack,
            LayoutMode::Scrolling,
            LayoutMode::Stack,
        ] {
            for action in [
                WindowDropAction::Swap,
                WindowDropAction::Insert(Direction::Down),
            ] {
                let settings = LayoutSettings {
                    mode,
                    ..LayoutSettings::default()
                };
                let mut engine = LayoutEngine::new(&VirtualWorkspaceSettings::default(), &settings);
                let mut store = WindowStore::default();
                let space = SpaceId::new(96);
                let screen = CGRect::new(CGPoint::ZERO, CGSize::new(1200.0, 800.0));
                let gaps = settings.gaps.effective_for_display(None);
                let line = crate::common::config::StackLineSettings::default();
                let source = WindowId::new(1, 1);
                let target = WindowId::new(1, 2);
                let _ =
                    engine.handle_event(&mut store, LayoutEvent::SpaceExposed(space, screen.size));
                let owner = engine.workspaces.active_workspace(space).unwrap();
                for window in [source, target, WindowId::new(1, 3)] {
                    store.insert_window(window, live_window(screen.size));
                    assert!(
                        engine
                            .workspaces
                            .assign_window_to_workspace(&mut store, space, window, owner)
                    );
                    let _ =
                        engine.handle_event(&mut store, LayoutEvent::WindowAdded(space, window));
                }
                engine.window_layout_constraints.insert(source, WindowLayoutConstraints {
                    is_resizable: true,
                    max_width: 180.0,
                    ..Default::default()
                });
                let before = engine.calculate_layout(space, screen, &gaps, line.reserve());
                let preview = engine
                    .drop_preview_frame(space, source, target, action, screen, None, &line)
                    .unwrap_or_else(|| panic!("missing {mode:?} {action:?} preview"));

                assert_eq!(
                    engine.calculate_layout(space, screen, &gaps, line.reserve()),
                    before
                );
                assert!(engine.apply_window_drop(WindowDropRequest {
                    source,
                    target,
                    space,
                    action
                }));
                let after = engine.calculate_layout(space, screen, &gaps, line.reserve());
                assert_eq!(
                    after.iter().find(|(window, _)| *window == source).unwrap().1,
                    preview.frame,
                    "{mode:?} {action:?}"
                );
            }
        }
    }

    fn overview_fixture() -> (LayoutEngine, WindowStore, crate::actor::reactor::OverviewDrop) {
        let mut engine = test_engine();
        let mut store = WindowStore::default();
        let space = SpaceId::new(95);
        let _ = engine.handle_event(
            &mut store,
            LayoutEvent::SpaceExposed(space, CGSize::new(1000.0, 800.0)),
        );
        let window = WindowId::new(6001, 1);
        store.insert_window(window, live_window(CGSize::new(500.0, 500.0)));
        let _ = engine.handle_event(
            &mut store,
            LayoutEvent::windows_observed(
                space,
                window.pid,
                vec![window_layout_info(window, CGSize::new(500.0, 500.0))],
                None,
            ),
        );
        let workspaces = engine.workspaces.list_workspaces(space);
        let drop = crate::actor::reactor::OverviewDrop {
            window,
            workspace: workspaces[1].0,
            target: None,
            frame: None,
        };
        (engine, store, drop)
    }

    #[test]
    fn overview_drop_moves_membership_once_without_switching_workspace() {
        let (mut engine, mut store, drop) = overview_fixture();
        let active = engine.workspaces().active_workspace(SpaceId::new(95));
        assert!(engine.relocate_window_with_drop(&mut store, &drop));
        assert_eq!(engine.workspaces().active_workspace(SpaceId::new(95)), active);
        let destination = engine
            .workspaces()
            .workspace_for_window(&store, SpaceId::new(95), drop.window)
            .unwrap();
        assert_eq!(destination, drop.workspace);
        let layout = engine.workspaces.active_layout(SpaceId::new(95), destination).unwrap();
        assert_eq!(
            engine.workspaces[destination].layout_system.selected_window(layout),
            Some(drop.window)
        );
        assert!(
            !engine.relocate_window_with_drop(&mut store, &drop),
            "stale source rejects duplicate mouse-up"
        );
    }

    #[test]
    fn window_added_projects_only_admitted_windows_with_canonical_ownership() {
        let mut engine = test_engine();
        let mut store = WindowStore::default();
        let space = SpaceId::new(95);
        let screen = test_display_frame(CGPoint::new(500., 500.));
        let window = WindowId::new(1, 1);
        store.insert_window(window, live_window(screen.size));
        let _ = engine.handle_event(&mut store, LayoutEvent::SpaceExposed(space, screen.size));

        assert!(
            !engine
                .handle_event(&mut store, LayoutEvent::WindowAdded(space, window))
                .response
                .changed
        );
        assert!(store.workspace_for_window(window).is_none());

        let workspace = engine.workspaces.active_workspace(space).unwrap();
        assert!(
            engine
                .workspaces
                .assign_window_to_workspace(&mut store, space, window, workspace)
        );
        store.window_mut(window).unwrap().manage_override = Some(false);
        assert!(
            !engine
                .handle_event(&mut store, LayoutEvent::WindowAdded(space, window))
                .response
                .changed
        );
        store.window_mut(window).unwrap().manage_override = None;
        assert!(
            engine
                .handle_event(&mut store, LayoutEvent::WindowAdded(space, window))
                .response
                .changed
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

        let mut window = crate::model::reactor::WindowState::from(
            crate::actor::reactor::testing::make_window_info(screen, None, "Window", None),
        );
        window.is_manageable = true;
        window_store.insert_window(wid, window);

        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::windows_observed(
                space,
                pid,
                vec![window_layout_info(wid, CGSize::new(500.0, 500.0))],
                None,
            ),
        );

        let _ = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::CreateWorkspace,
        );
        let workspaces = engine.workspaces_mut().list_workspaces(space).to_vec();
        let ws1 = workspaces[0].0;
        let ws2 = workspaces[1].0;

        let layout = engine.workspaces.active_layout(space, ws1).unwrap();
        let before = engine.workspaces[ws1].layout_system.container_tree(layout);
        assert!(!engine.relocate_window_to_workspace(
            &mut window_store,
            ws1,
            (SpaceId::new(96), ws2),
            wid,
            WindowRelocation::Workspace,
        ));
        assert_eq!(window_store.workspace_for_window(wid), Some(ws1));
        assert_eq!(
            engine.workspaces[ws1].layout_system.container_tree(layout),
            before
        );

        let _ = engine.handle_virtual_workspace_command(
            &mut window_store,
            space,
            &LayoutCommand::MoveWindowToWorkspace {
                workspace: WorkspaceSelector::Index(1),
                follow: false,
                window_id: Some(wid.idx.get()),
            },
        );

        assert!(
            window_store.workspace_windows(ws1).is_empty(),
            "source workspace must be empty after a same-space workspace move"
        );
        assert_eq!(
            engine.workspaces().workspace_for_window(&window_store, space, wid),
            Some(ws2)
        );
        assert_eq!(window_store.workspace_windows(ws2), vec![wid]);

        for (target, expected) in [("next", workspaces[2].0), ("prev", ws2)] {
            let _ = engine.handle_virtual_workspace_command(
                &mut window_store,
                space,
                &LayoutCommand::MoveWindowToWorkspace {
                    workspace: WorkspaceSelector::Name(target.into()),
                    follow: false,
                    window_id: Some(wid.idx.get()),
                },
            );
            assert_eq!(
                engine.workspaces().workspace_for_window(&window_store, space, wid),
                Some(expected)
            );
        }
        window_store.invalidate_ax_identity(wid);
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowSuspended(wid));
        assert!(engine.relocate_window_to_workspace(
            &mut window_store,
            ws2,
            (space, ws1),
            wid,
            WindowRelocation::Observed
        ));
        assert!(window_store.is_temporarily_unavailable(wid));
        assert!(!engine.workspaces[ws1].layout_system.window_participates(wid));
        window_store.validate_ax_identity(wid);
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, wid));
        assert_eq!(window_store.workspace_for_window(wid), Some(ws1));
        for (id, workspace) in engine.workspaces.workspaces.iter() {
            let layout = workspace.active_layout().unwrap();
            assert_eq!(workspace.layout_system.contains_window(layout, wid), id == ws1);
        }
    }

    #[test]
    fn moving_a_window_between_spaces_keeps_its_size_constraints() {
        let mut engine = test_engine();
        let mut window_store = WindowStore::default();
        let space = SpaceId::new(93);
        let screen = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1200.0, 800.0));
        let _ =
            engine.handle_event(&mut window_store, LayoutEvent::SpaceExposed(space, screen.size));
        let (fixed, other) = (WindowId::new(7, 1), WindowId::new(8, 1));
        let mut state = live_window(CGSize::new(300.0, 800.0));
        state.info.is_resizable = false;
        let fixed_info = state.layout_info(fixed);
        window_store.insert_window(fixed, state);
        window_store.insert_window(other, live_window(CGSize::new(500.0, 500.0)));
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::windows_observed(space, fixed.pid, vec![fixed_info], None),
        );
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::windows_observed(
                space,
                other.pid,
                vec![window_layout_info(other, CGSize::new(500.0, 500.0))],
                None,
            ),
        );
        let width = |engine: &mut LayoutEngine| {
            let frames =
                engine.calculate_layout(space, screen, &Default::default(), Default::default());
            frames
                .into_iter()
                .find(|(w, _)| *w == fixed)
                .expect("fixed window is tiled")
                .1
                .size
                .width
        };
        assert_eq!(width(&mut engine), 300.0);

        // A display move: the window leaves the layout and comes back.
        let _ = engine.handle_event(
            &mut window_store,
            LayoutEvent::WindowRemovedPreserveFloating(fixed),
        );
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowAdded(space, fixed));
        assert_eq!(width(&mut engine), 300.0);

        // Leaving the layout for good still isn't the window's end.
        let _ = engine.handle_event(&mut window_store, LayoutEvent::WindowRemoved(fixed));
        assert!(engine.window_layout_constraints.windows.contains_key(&fixed));
    }
}
