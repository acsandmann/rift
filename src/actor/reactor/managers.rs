use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2_core_foundation::CGRect;
use rift_protocol::StackInfo;
use tracing::trace;

use super::replay::Record;
use super::{AppState, Event, WorkspaceSwitchOrigin, WorkspaceSwitchState};
use crate::actor;
use crate::actor::app::{AppThreadHandle, WindowId, WindowInventoryToken, pid_t};
use crate::actor::reactor::Reactor;
use crate::actor::reactor::animation::AnimationManager;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::{input, menu_bar, raise_manager, stack_line, window_notify, wm_controller};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{DragDropSettings, LayoutMode};
use crate::layout_engine::LayoutEngine;
use crate::model::broadcast::{BroadcastEvent, BroadcastSender, protocol_workspace_id};
use crate::sys::geometry::SameAs;
use crate::sys::screen::SpaceId;

/// Manages application state and rules
pub struct AppManager {
    pub apps: HashMap<pid_t, AppState>,
}

impl AppManager {
    pub fn new() -> Self { AppManager { apps: HashMap::default() } }

    pub fn reject_duplicate(&self, pid: pid_t, handle: &AppThreadHandle) -> bool {
        let Some(existing) = self.apps.get(&pid) else {
            return false;
        };
        tracing::error!(pid, "Duplicate app actor registration; retaining original actor");
        if !existing.handle.same_actor(handle) {
            _ = handle.send(crate::actor::app::Request::Terminate);
        }
        true
    }
}

/// Manages drag operations and window swapping
pub struct DragManager {
    pub actor: crate::actor::drag::DragActor,
    pub native_down: Option<(WindowId, CGRect, objc2_core_foundation::CGPoint)>,
    pub native_motion_active: Arc<AtomicBool>,
    pub externally_controlled_window: Option<WindowId>,
    pub(super) preview: Option<crate::ui::drag_preview::DragPreview>,
    pub(super) preview_enabled: bool,
    pub(super) preview_suppressed: bool,
    pub(super) haptics_enabled: bool,
}

impl DragManager {
    /// Native intent belongs to the pressed window, and resize intent starts at
    /// its border. A button held elsewhere cannot authorize layout mutations.
    pub fn native_geometry_intent(
        &self,
        wid: WindowId,
        new: CGRect,
    ) -> Option<crate::actor::drag::DragKind> {
        use crate::actor::drag::DragKind;
        if let Some(source) = self.actor.source() {
            return (source.window == wid).then(|| self.actor.kind()).flatten();
        }
        let (window, anchor, point) = self.native_down?;
        if window != wid {
            return None;
        }
        let in_x = point.x >= anchor.origin.x - 8.0 && point.x <= anchor.max().x + 8.0;
        let in_y = point.y >= anchor.origin.y - 8.0 && point.y <= anchor.max().y + 8.0;
        if !in_x || !in_y {
            return None;
        }
        let pointer = crate::sys::event::current_cursor_location().ok()?;
        if pointer.same_as(point) {
            return None;
        }
        let near = |a: f64, b: f64| (a - b).abs() <= 8.0;
        let follows_edge = |press: f64, cursor: f64, before: f64, after: f64| {
            near(press, before) && (before - after).abs() > 0.1 && near(cursor, after)
        };
        if !anchor.size.same_as(new.size)
            && (follows_edge(point.x, pointer.x, anchor.origin.x, new.origin.x)
                || follows_edge(point.x, pointer.x, anchor.max().x, new.max().x)
                || follows_edge(point.y, pointer.y, anchor.origin.y, new.origin.y)
                || follows_edge(point.y, pointer.y, anchor.max().y, new.max().y))
        {
            Some(DragKind::NativeResize)
        } else if point.y <= anchor.origin.y + 32.0
            && near(pointer.x - point.x, new.origin.x - anchor.origin.x)
            && near(pointer.y - point.y, new.origin.y - anchor.origin.y)
        {
            Some(DragKind::NativeMove)
        } else {
            None
        }
    }

    pub fn sync_motion_gate(&self) {
        let active = self.actor.is_active()
            && matches!(
                self.actor.kind(),
                Some(
                    crate::actor::drag::DragKind::NativeMove
                        | crate::actor::drag::DragKind::NativeResize
                )
            );
        self.native_motion_active.store(active, Ordering::Release);
    }

    /// A window leaving Rift's control can no longer be dragged by it.
    pub fn forget_window(&mut self, wid: WindowId) {
        if self.native_down.is_some_and(|(window, _, _)| window == wid) {
            self.native_down = None;
        }
        if self.actor.window_removed(wid) && self.externally_controlled_window == Some(wid) {
            self.externally_controlled_window = None;
        }
        self.sync_motion_gate();
        self.sync_preview();
    }

    pub fn reset(&mut self) {
        self.native_down = None;
        self.actor.cancel();
        self.sync_motion_gate();
        self.release_preview();
        self.externally_controlled_window = None;
    }

    pub fn update_config(&mut self, config: DragDropSettings) {
        self.haptics_enabled = config.haptics_enabled;
        self.actor.update_config(config);
        self.preview_enabled = config.enabled && config.preview;
        if !config.enabled || !config.preview {
            self.release_preview();
        }
        if !config.enabled {
            self.externally_controlled_window = None;
        }
        self.sync_preview();
    }

    pub fn sync_preview(&mut self) {
        if cfg!(test) {
            return;
        }
        let Some(target) = self
            .actor
            .preview_target()
            .filter(|_| self.preview_enabled && !self.preview_suppressed)
        else {
            self.hide_preview();
            return;
        };
        let result = if let Some(preview) = &mut self.preview {
            preview.show(target)
        } else {
            crate::ui::drag_preview::DragPreview::new(target).and_then(|mut preview| {
                preview.show(target)?;
                self.preview = Some(preview);
                Ok(())
            })
        };
        if let Err(error) = result {
            tracing::warn!(?error, "failed to update drag preview");
            self.release_preview();
        }
    }

    pub fn hide_preview(&mut self) {
        if let Some(preview) = &mut self.preview {
            preview.hide();
        }
    }

    pub fn release_preview(&mut self) { drop(self.preview.take()); }

    pub fn suppress_preview(&mut self, suppressed: bool) {
        self.preview_suppressed = suppressed;
        if suppressed {
            self.hide_preview();
        }
    }
}

/// Manages window notifications
pub struct NotificationManager {
    pub last_sls_notification_ids: Vec<u32>,
    pub last_layout_modes_by_space: HashMap<SpaceId, crate::common::config::LayoutMode>,
    pub _window_notify_tx: Option<window_notify::Sender>,
}

/// Manages menu state and interactions
pub struct MenuManager {
    pub menu_state: super::MenuState,
    pub menu_tx: Option<menu_bar::Sender>,
    pub last_projection_signature: Option<u64>,
}

/// Manages Mission Control state
pub struct MissionControlManager {
    pub mission_control_state: super::MissionControlState,
}

/// Owns ordering and coalescing for asynchronous AX window inventories.
pub struct WindowInventoryManager {
    pub next_request_id: u64,
    pub in_flight: HashMap<pid_t, WindowInventoryToken>,
    pub pending: HashSet<pid_t>,
    pub refocus_after_refresh: HashMap<pid_t, WindowId>,
}

/// Manages workspace switching state
pub struct WorkspaceSwitchManager {
    pub workspace_switch_state: super::WorkspaceSwitchState,
    pub workspace_switch_generation: u64,
    pub active_workspace_switch: Option<u64>,
    pub pending_workspace_switch_origin: Option<WorkspaceSwitchOrigin>,
    pub pending_focus_mouse_warp: Option<WindowId>,
    pub pending_display_warp: Option<crate::sys::screen::SpaceId>,
}

impl WorkspaceSwitchManager {
    pub fn start_workspace_switch(&mut self, origin: WorkspaceSwitchOrigin) {
        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        self.active_workspace_switch = Some(self.workspace_switch_generation);
        self.workspace_switch_state = WorkspaceSwitchState::Active;
        self.pending_workspace_switch_origin = Some(origin);
    }

    pub fn manual_switch_in_progress(&self) -> bool {
        self.workspace_switch_state == WorkspaceSwitchState::Active
            && self.pending_workspace_switch_origin == Some(WorkspaceSwitchOrigin::Manual)
    }

    pub fn mark_workspace_switch_inactive(&mut self) {
        self.workspace_switch_state = WorkspaceSwitchState::Inactive;
        self.pending_workspace_switch_origin = None;
    }
}

/// Manages refocus and cleanup state
pub struct RefocusManager {
    pub stale_cleanup_state: super::StaleCleanupState,
    pub refocus_state: super::RefocusState,
}

/// Manages communication channels to other actors
pub struct CommunicationManager {
    pub input_tx: Option<input::Sender>,
    pub stack_line_tx: Option<stack_line::Sender>,
    pub raise_manager_tx: raise_manager::Sender,
    pub event_broadcaster: BroadcastSender,
    pub wm_sender: Option<wm_controller::Sender>,
    pub events_tx: Option<actor::Sender<Event>>,
}

/// Manages recording state
pub struct RecordingManager {
    pub record: Record,
}

/// Manages layout engine state
pub struct LayoutManager {
    pub layout_engine: LayoutEngine,
}

pub type LayoutResult = Vec<(SpaceId, Vec<(WindowId, CGRect)>)>;

pub(super) use crate::model::hidden_window_placement::ScrollingDisplayBounds;

fn bound_scrolling_tiled_frames_to_screen(
    reactor: &Reactor,
    layout: &mut Vec<(WindowId, CGRect)>,
    bounds: ScrollingDisplayBounds,
    active_workspace_windows: &HashSet<WindowId>,
) {
    for (wid, frame) in layout.iter_mut() {
        if !active_workspace_windows.contains(wid)
            || reactor.layout_manager.layout_engine.is_window_floating(*wid)
        {
            continue;
        }
        *frame = bounds.apply(*frame);
    }
}

impl LayoutManager {
    fn publish_stack_tabs(
        reactor: &Reactor,
        space: SpaceId,
        group_infos: &[crate::layout_engine::engine::GroupContainerInfo],
        active_workspace_for_space_has_fullscreen: bool,
        reorder: bool,
    ) {
        if reactor.config.settings.ui.stack_line.enabled
            && let Some(tx) = &reactor.communication_manager.stack_line_tx
        {
            let groups: Vec<crate::actor::stack_line::GroupInfo> = group_infos
                .iter()
                .map(|g| crate::actor::stack_line::GroupInfo {
                    node_id: g.node_id,
                    space_id: space,
                    frame: g.frame,
                    horizontal: g.container_kind != crate::layout_engine::LayoutKind::VerticalStack,
                    selected_index: g.selected_index,
                    tabs: g
                        .window_ids
                        .iter()
                        .map(|&window_id| {
                            let info = reactor.state.windows.window(window_id).map(|w| &w.info);
                            crate::actor::stack_line::TabInfo {
                                window_id,
                                title: info.map(|i| i.title.clone()).unwrap_or_default(),
                                bundle_id: info.and_then(|i| i.bundle_id.clone()),
                                window_server_id: reactor
                                    .state
                                    .windows
                                    .record(window_id)
                                    .and_then(|record| record.window_server_id()),
                            }
                        })
                        .collect(),
                })
                .collect();
            let active_space_ids: Vec<crate::sys::screen::SpaceId> =
                reactor.iter_active_spaces().collect();

            if let Err(e) = tx.try_send(crate::actor::stack_line::Event::GroupsUpdated {
                active_space_ids,
                space_id: space,
                groups,
                active_workspace_for_space_has_fullscreen,
                reorder,
            }) {
                tracing::warn!("Failed to send groups update to stack_line: {}", e);
            }
        }
    }

    pub(super) fn refresh_stack_tabs(reactor: &mut Reactor, space: SpaceId) {
        if !reactor.config.settings.ui.stack_line.enabled
            || reactor.communication_manager.stack_line_tx.is_none()
            || !reactor.is_space_active(space)
        {
            return;
        }
        let Some(screen) = reactor.space_state.screen_by_space(space) else {
            return;
        };
        let frame = screen.frame;
        let gaps = reactor
            .config
            .settings
            .layout
            .gaps
            .effective_for_display(screen.display_uuid_owned().as_deref());
        let groups = reactor.layout_manager.layout_engine.collect_group_containers(
            space,
            frame,
            &gaps,
            reactor.config.settings.ui.stack_line.reserve(),
        );
        let fullscreen = reactor
            .layout_manager
            .layout_engine
            .active_workspace_for_space_has_fullscreen(space);
        Self::publish_stack_tabs(reactor, space, &groups, fullscreen, true);
    }

    pub fn update_layout(
        reactor: &mut Reactor,
        is_resize: bool,
        is_workspace_switch: bool,
        space_scope: Option<SpaceId>,
    ) -> Result<bool, crate::model::reactor::ReactorError> {
        if reactor.refreshes_blocked() {
            return Ok(false);
        }
        let layout_result = Self::calculate_layout(reactor, space_scope);
        Self::apply_layout(
            reactor,
            layout_result,
            is_resize,
            is_workspace_switch,
            false,
            false,
        )
    }

    pub fn update_layout_instant(
        reactor: &mut Reactor,
        space_scope: Option<SpaceId>,
        force_frames: bool,
    ) -> Result<bool, crate::model::reactor::ReactorError> {
        if reactor.refreshes_blocked() {
            return Ok(false);
        }
        let layout = Self::calculate_layout(reactor, space_scope);
        // Finish the logical scrolling viewport here, without queueing a second
        // animation behind the overview's own transition.
        for &(space, _) in &layout {
            let engine = &mut reactor.layout_manager.layout_engine;
            if let Some((workspace, layout)) = engine.workspaces().active_layout_for_space(space)
                && let crate::layout_engine::LayoutSystemKind::Scrolling(system) =
                    &mut engine.workspaces_mut().workspaces[workspace].layout_system
                && let Some((mut camera, _)) = system.presentation(layout)
            {
                camera.finish();
                system
                    .commit_presented_viewport(layout, &camera.snapshot(std::time::Instant::now()));
            }
        }
        let layout = Self::calculate_layout(reactor, space_scope);
        Self::apply_layout(reactor, layout, false, false, true, force_frames)
    }

    pub(super) fn calculate_layout(
        reactor: &mut Reactor,
        space_scope: Option<SpaceId>,
    ) -> LayoutResult {
        if reactor.state.windows.tracked_window_count() == 0 {
            return LayoutResult::new();
        }
        let all_screen_frames: Vec<CGRect> =
            reactor.space_state.screens.iter().map(|s| s.frame).collect();
        let mut layout_result = LayoutResult::new();

        for index in 0..reactor.space_state.screens.len() {
            let screen = &reactor.space_state.screens[index];
            let (Some(space), frame) = (screen.space, screen.frame) else {
                continue;
            };
            if space_scope.is_some_and(|scope| scope != space) {
                continue;
            }
            if !reactor.is_space_active(space) {
                continue;
            }
            let display_uuid_opt = screen.display_uuid_owned();
            let gaps = reactor
                .config
                .settings
                .layout
                .gaps
                .effective_for_display(display_uuid_opt.as_deref());
            reactor
                .layout_manager
                .layout_engine
                .update_space_display(space, display_uuid_opt.clone());
            let mut layout =
                reactor.layout_manager.layout_engine.calculate_layout_with_virtual_workspaces(
                    &reactor.state.windows,
                    space,
                    frame,
                    &gaps,
                    reactor.config.settings.ui.stack_line.reserve(),
                    |wid| reactor.state.windows.window(wid).map(|w| w.presented_frame),
                    &all_screen_frames,
                );
            if reactor.layout_manager.layout_engine.active_layout_mode_at(space)
                == LayoutMode::Scrolling
                && let Some(bounds) = ScrollingDisplayBounds::for_screen(frame, &all_screen_frames)
            {
                let active_workspace_windows: HashSet<WindowId> = reactor
                    .layout_manager
                    .layout_engine
                    .workspaces()
                    .windows_in_active_workspace(&reactor.state.windows, space)
                    .into_iter()
                    .collect();
                bound_scrolling_tiled_frames_to_screen(
                    reactor,
                    &mut layout,
                    bounds,
                    &active_workspace_windows,
                );
            }
            layout_result.push((space, layout));
        }

        layout_result
    }

    fn apply_layout(
        reactor: &mut Reactor,
        layout_result: LayoutResult,
        is_resize: bool,
        is_workspace_switch: bool,
        instant: bool,
        force_frames: bool,
    ) -> Result<bool, crate::model::reactor::ReactorError> {
        let main_window = reactor.main_window();
        trace!(?main_window);
        let skip_wid = reactor.drag_manager.externally_controlled_window;
        let mut any_frame_changed = false;

        let active_space = reactor.workspace_command_space();
        for (space, layout) in layout_result {
            // Start native workspace moves before collecting stack tabs and building
            // IPC events. App actors can perform AX work while we publish the UI.
            let switch_dispatched = is_workspace_switch && !instant && !force_frames;
            if switch_dispatched {
                any_frame_changed |=
                    AnimationManager::workspace_switch_layout(reactor, space, &layout, skip_wid);
            }
            if let Some(screen) = reactor.space_state.screen_by_space(space) {
                let screen_frame = screen.frame;
                let display_uuid = screen.display_uuid_owned();
                let gaps = reactor
                    .config
                    .settings
                    .layout
                    .gaps
                    .effective_for_display(display_uuid.as_deref());
                let stack_has_fullscreen = reactor
                    .layout_manager
                    .layout_engine
                    .active_workspace_for_space_has_fullscreen(space);
                let active_workspace_for_space_has_fullscreen =
                    active_space == Some(space) && stack_has_fullscreen;
                let group_infos = reactor.layout_manager.layout_engine.collect_group_containers(
                    space,
                    screen_frame,
                    &gaps,
                    reactor.config.settings.ui.stack_line.reserve(),
                );

                Self::publish_stack_tabs(reactor, space, &group_infos, stack_has_fullscreen, false);

                if let Some(workspace_id) =
                    reactor.layout_manager.layout_engine.workspaces().active_workspace(space)
                {
                    let workspace_index = reactor
                        .layout_manager
                        .layout_engine
                        .workspaces()
                        .active_workspace_idx(space);
                    let workspace_name = reactor
                        .layout_manager
                        .layout_engine
                        .workspace_name(space, workspace_id)
                        .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));

                    let stacks: Vec<StackInfo> = group_infos
                        .iter()
                        .map(|g| {
                            // Clients have always received vertical stacks bottom to top.
                            let mut windows: Vec<_> =
                                g.window_ids.iter().map(WindowId::to_debug_string).collect();
                            let mut selected_index = g.selected_index;
                            if g.container_kind == crate::layout_engine::LayoutKind::VerticalStack {
                                windows.reverse();
                                selected_index =
                                    windows.len().saturating_sub(1).saturating_sub(selected_index);
                            }
                            StackInfo {
                                total_count: g.total_count,
                                container_kind: match g.container_kind {
                                    crate::layout_engine::LayoutKind::Horizontal => {
                                        rift_protocol::LayoutKind::Horizontal
                                    }
                                    crate::layout_engine::LayoutKind::Vertical => {
                                        rift_protocol::LayoutKind::Vertical
                                    }
                                    crate::layout_engine::LayoutKind::HorizontalStack => {
                                        rift_protocol::LayoutKind::HorizontalStack
                                    }
                                    crate::layout_engine::LayoutKind::VerticalStack => {
                                        rift_protocol::LayoutKind::VerticalStack
                                    }
                                },
                                selected_index,
                                windows,
                            }
                        })
                        .collect();

                    if stacks.len() > 0 {
                        let event = BroadcastEvent::StacksChanged {
                            workspace_id: protocol_workspace_id(workspace_id),
                            workspace_index,
                            workspace_name,
                            stacks,
                            active_workspace_has_fullscreen:
                                active_workspace_for_space_has_fullscreen,
                            space_id: space.get(),
                            display_uuid,
                        };
                        let _ = reactor.communication_manager.event_broadcaster.send(event);
                    }
                }
            }

            if force_frames {
                any_frame_changed |=
                    AnimationManager::reapply_layout(reactor, space, &layout, skip_wid);
            } else if instant {
                any_frame_changed |=
                    AnimationManager::instant_layout(reactor, space, &layout, skip_wid);
            } else if switch_dispatched {
                // Already submitted before UI publication above.
            } else if reactor.workspace_switch_manager.active_workspace_switch.is_some() {
                any_frame_changed |=
                    AnimationManager::instant_layout(reactor, space, &layout, skip_wid);
            } else {
                any_frame_changed |= AnimationManager::animate_layout(
                    reactor, space, &layout, is_resize, skip_wid, true,
                );
            }
            if let Some(window) = reactor.workspace_switch_manager.pending_focus_mouse_warp
                && reactor.workspace_switch_manager.pending_display_warp.is_none()
                && reactor.layout_manager.layout_engine.active_layout_mode_at(space)
                    == LayoutMode::Scrolling
                && let Some((_, frame)) = layout.iter().find(|(wid, _)| *wid == window)
            {
                reactor.workspace_switch_manager.pending_focus_mouse_warp = None;
                if reactor.screen_for_point(frame.mid()).is_some() {
                    reactor.warp_mouse(frame.mid());
                }
            }
        }

        Ok(any_frame_changed)
    }
}

/// Manages pending space changes
pub struct PendingSpaceChangeManager {
    pub pending_space_change: Option<ForwardedSpaceState>,
}
