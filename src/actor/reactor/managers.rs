use std::time::Instant;

use objc2_core_foundation::{CGPoint, CGRect};
use tracing::trace;

use super::replay::Record;
use super::{
    AppState, Event, FullscreenSpaceTrack, PendingSpaceChange, ScreenInfo, WindowState,
    WorkspaceSwitchOrigin, WorkspaceSwitchState,
};
use crate::actor;
use crate::actor::app::{WindowId, pid_t};
use crate::actor::broadcast::{BroadcastEvent, BroadcastSender, StackInfo};
use crate::actor::drag_swap::DragManager as DragSwapManager;
use crate::actor::reactor::Reactor;
use crate::actor::reactor::animation::AnimationManager;
use crate::actor::{event_tap, menu_bar, raise_manager, stack_line, window_notify, wm_controller};
use crate::common::collections::{HashMap, HashSet};
use crate::common::config::{LayoutMode, WindowSnappingSettings};
use crate::layout_engine::LayoutEngine;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

/// Manages window state and lifecycle
pub struct WindowManager {
    pub windows: HashMap<WindowId, WindowState>,
    pub window_ids: HashMap<WindowServerId, WindowId>,
    pub visible_windows: HashSet<WindowServerId>,
    pub observed_window_server_ids: HashSet<WindowServerId>,
}

/// Manages application state and rules
pub struct AppManager {
    pub apps: HashMap<pid_t, AppState>,
    pub app_rules_recent_targets: HashMap<crate::sys::window_server::WindowServerId, Instant>,
}

impl AppManager {
    pub fn new() -> Self {
        AppManager {
            apps: HashMap::default(),
            app_rules_recent_targets: HashMap::default(),
        }
    }

    pub fn mark_wsids_recent<I>(&mut self, wsids: I)
    where I: IntoIterator<Item = crate::sys::window_server::WindowServerId> {
        let now = std::time::Instant::now();
        for ws in wsids {
            self.app_rules_recent_targets.insert(ws, now);
        }
    }

    pub fn is_wsid_recent(
        &self,
        wsid: crate::sys::window_server::WindowServerId,
        ttl_ms: u64,
    ) -> bool {
        if let Some(&ts) = self.app_rules_recent_targets.get(&wsid) {
            return ts.elapsed().as_millis() < (ttl_ms as u128);
        }
        false
    }

    pub fn purge_expired(&mut self, ttl_ms: u64) {
        let now = std::time::Instant::now();
        let mut to_remove = Vec::new();
        for (k, &v) in self.app_rules_recent_targets.iter() {
            if now.duration_since(v).as_millis() >= (ttl_ms as u128) {
                to_remove.push(*k);
            }
        }
        for k in to_remove {
            self.app_rules_recent_targets.remove(&k);
        }
    }
}

/// Manages space and screen state
pub struct SpaceManager {
    pub screens: Vec<ScreenInfo>,
    pub fullscreen_by_space: HashMap<u64, FullscreenSpaceTrack>,
    pub changing_screens: HashSet<WindowServerId>,
    pub has_seen_display_set: bool,
}

impl SpaceManager {
    pub fn screen_by_space(&self, space: SpaceId) -> Option<&ScreenInfo> {
        self.screens.iter().find(|screen| screen.space == Some(space))
    }

    pub fn iter_known_spaces(&self) -> impl Iterator<Item = SpaceId> + '_ {
        self.screens.iter().filter_map(|screen| screen.space)
    }

    pub fn first_known_space(&self) -> Option<SpaceId> { self.iter_known_spaces().next() }
}

/// Manages drag operations and window swapping
pub struct DragManager {
    pub drag_state: super::DragState,
    pub drag_swap_manager: DragSwapManager,
    pub skip_layout_for_window: Option<WindowId>,
}

impl DragManager {
    pub fn reset(&mut self) { self.drag_swap_manager.reset(); }

    pub fn last_target(&self) -> Option<WindowId> { self.drag_swap_manager.last_target() }

    pub fn dragged(&self) -> Option<WindowId> { self.drag_swap_manager.dragged() }

    pub fn origin_frame(&self) -> Option<CGRect> { self.drag_swap_manager.origin_frame() }

    pub fn update_config(&mut self, config: WindowSnappingSettings) {
        self.drag_swap_manager.update_config(config);
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
}

/// Manages Mission Control state
pub struct MissionControlManager {
    pub mission_control_state: super::MissionControlState,
    pub pending_mission_control_refresh: HashSet<pid_t>,
}

/// Manages workspace switching state
pub struct WorkspaceSwitchManager {
    pub workspace_switch_state: super::WorkspaceSwitchState,
    pub workspace_switch_generation: u64,
    pub active_workspace_switch: Option<u64>,
    pub pending_workspace_switch_origin: Option<WorkspaceSwitchOrigin>,
    pub pending_workspace_mouse_warp: Option<WindowId>,
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
    pub event_tap_tx: Option<event_tap::Sender>,
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

fn bound_frame_to_screen(frame: CGRect, screen: CGRect) -> CGRect {
    const WINDOW_HIDDEN_THRESHOLD: f64 = 10.0;

    let screen_left = screen.origin.x;
    let screen_top = screen.origin.y;
    let screen_right = screen.max().x;
    let screen_bottom = screen.max().y;
    let max_y = (screen_bottom - frame.size.height).max(screen_top);
    let x = if frame.max().x <= screen_left {
        screen_left - frame.size.width + WINDOW_HIDDEN_THRESHOLD
    } else if frame.origin.x >= screen_right {
        screen_right - WINDOW_HIDDEN_THRESHOLD
    } else {
        frame.origin.x
    };

    CGRect::new(
        CGPoint::new(x, frame.origin.y.clamp(screen_top, max_y)),
        frame.size,
    )
}

fn bound_scrolling_tiled_frames_to_screen(
    reactor: &Reactor,
    layout: &mut Vec<(WindowId, CGRect)>,
    screen: CGRect,
    active_workspace_windows: &HashSet<WindowId>,
) {
    for (wid, frame) in layout.iter_mut() {
        if !active_workspace_windows.contains(wid)
            || reactor.layout_manager.layout_engine.is_window_floating(*wid)
        {
            continue;
        }
        *frame = bound_frame_to_screen(*frame, screen);
    }
}

impl LayoutManager {
    pub fn update_layout(
        reactor: &mut Reactor,
        is_resize: bool,
        is_workspace_switch: bool,
    ) -> Result<bool, crate::model::reactor::ReactorError> {
        let layout_result = Self::calculate_layout(reactor);
        Self::apply_layout(reactor, layout_result, is_resize, is_workspace_switch)
    }

    fn calculate_layout(reactor: &mut Reactor) -> LayoutResult {
        if reactor.window_manager.windows.is_empty() {
            return LayoutResult::new();
        }
        let screens = reactor.space_manager.screens.clone();
        let active_space_count = screens
            .iter()
            .filter_map(|screen| screen.space)
            .filter(|space| reactor.is_space_active(*space))
            .count();
        let mut layout_result = LayoutResult::new();

        for screen in screens {
            let Some(space) = screen.space else {
                continue;
            };
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
                    space,
                    screen.frame.clone(),
                    &gaps,
                    reactor.config.settings.ui.stack_line.thickness(),
                    reactor.config.settings.ui.stack_line.horiz_placement,
                    reactor.config.settings.ui.stack_line.vert_placement,
                    |wid| reactor.window_manager.windows.get(&wid).map(|w| w.frame_monotonic),
                );
            if active_space_count > 1
                && reactor.layout_manager.layout_engine.active_layout_mode_at(space)
                    == LayoutMode::Scrolling
            {
                let active_workspace_windows: HashSet<WindowId> = reactor
                    .layout_manager
                    .layout_engine
                    .windows_in_active_workspace(space)
                    .into_iter()
                    .collect();
                bound_scrolling_tiled_frames_to_screen(
                    reactor,
                    &mut layout,
                    screen.frame,
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
    ) -> Result<bool, crate::model::reactor::ReactorError> {
        let main_window = reactor.main_window();
        trace!(?main_window);
        let skip_wid = reactor
            .drag_manager
            .skip_layout_for_window
            .take()
            .or(reactor.drag_manager.drag_swap_manager.dragged());
        let mut any_frame_changed = false;

        let active_space = reactor.main_window_space();
        for (space, layout) in layout_result {
            if let Some(screen) = reactor.space_manager.screen_by_space(space) {
                let screen_frame = screen.frame;
                let display_uuid = screen.display_uuid_owned();
                let gaps = reactor
                    .config
                    .settings
                    .layout
                    .gaps
                    .effective_for_display(display_uuid.as_deref());
                let active_workspace_for_space_has_fullscreen = active_space == Some(space)
                    && reactor
                        .layout_manager
                        .layout_engine
                        .active_workspace_for_space_has_fullscreen(space);
                let group_infos = reactor.layout_manager.layout_engine.collect_group_containers(
                    space,
                    screen_frame,
                    &gaps,
                    reactor.config.settings.ui.stack_line.thickness(),
                    reactor.config.settings.ui.stack_line.horiz_placement,
                    reactor.config.settings.ui.stack_line.vert_placement,
                );

                // Keep internal stack-line UI actor fed from the same group snapshot.
                if reactor.config.settings.ui.stack_line.enabled
                    && let Some(tx) = &reactor.communication_manager.stack_line_tx
                {
                    let groups: Vec<crate::actor::stack_line::GroupInfo> = group_infos
                        .iter()
                        .map(|g| crate::actor::stack_line::GroupInfo {
                            node_id: g.node_id,
                            space_id: space,
                            container_kind: g.container_kind,
                            frame: g.frame,
                            total_count: g.total_count,
                            selected_index: g.selected_index,
                            window_ids: g.window_ids.clone(),
                        })
                        .collect();
                    let active_space_ids: Vec<crate::sys::screen::SpaceId> =
                        reactor.iter_active_spaces().collect();

                    if let Err(e) = tx.try_send(crate::actor::stack_line::Event::GroupsUpdated {
                        active_space_ids,
                        space_id: space,
                        groups,
                        active_workspace_for_space_has_fullscreen,
                    }) {
                        tracing::warn!("Failed to send groups update to stack_line: {}", e);
                    }
                }

                if let Some(workspace_id) =
                    reactor.layout_manager.layout_engine.active_workspace(space)
                {
                    let workspace_index =
                        reactor.layout_manager.layout_engine.active_workspace_idx(space);
                    let workspace_name = reactor
                        .layout_manager
                        .layout_engine
                        .workspace_name(space, workspace_id)
                        .unwrap_or_else(|| format!("Workspace {:?}", workspace_id));

                    let stacks: Vec<StackInfo> = group_infos
                        .iter()
                        .map(|g| StackInfo {
                            container_kind: g.container_kind,
                            total_count: g.total_count,
                            selected_index: g.selected_index,
                            windows: g.window_ids.iter().map(WindowId::to_debug_string).collect(),
                        })
                        .collect();

                    let event = BroadcastEvent::StacksChanged {
                        workspace_id,
                        workspace_index,
                        workspace_name,
                        stacks,
                        active_workspace_has_fullscreen: active_workspace_for_space_has_fullscreen,
                        space_id: space,
                        display_uuid,
                    };
                    let _ = reactor.communication_manager.event_broadcaster.send(event);
                }
            }

            let suppress_animation = is_workspace_switch
                || reactor.workspace_switch_manager.active_workspace_switch.is_some();
            if suppress_animation {
                any_frame_changed |= AnimationManager::instant_layout(reactor, &layout, skip_wid);
            } else {
                any_frame_changed |=
                    AnimationManager::animate_layout(reactor, space, &layout, is_resize, skip_wid);
            }
        }

        reactor.maybe_send_menu_update();
        Ok(any_frame_changed)
    }
}

/// Manages window server information
pub struct WindowServerInfoManager {
    pub window_server_info: HashMap<WindowServerId, WindowServerInfo>,
}

/// Manages pending space changes
pub struct PendingSpaceChangeManager {
    pub pending_space_change: Option<PendingSpaceChange>,
    pub topology_relayout_pending: bool,
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::bound_frame_to_screen;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
    }

    #[test]
    fn bound_frame_to_screen_keeps_partial_overlap_for_strip_behavior() {
        let screen = rect(2000.0, 0.0, 1000.0, 800.0);
        let frame = rect(1500.0, 50.0, 700.0, 400.0);
        let bounded = bound_frame_to_screen(frame, screen);
        assert_eq!(bounded.origin.x, 1500.0);
        assert_eq!(bounded.size.width, 700.0);
    }

    #[test]
    fn bound_frame_to_screen_parks_fully_offscreen_windows_to_hidden_sliver() {
        let screen = rect(2000.0, 0.0, 1000.0, 800.0);
        let frame = rect(1200.0, 80.0, 600.0, 300.0);
        let bounded = bound_frame_to_screen(frame, screen);
        assert_eq!(bounded.origin.x, 1410.0);
        assert_eq!(bounded.size.width, 600.0);
    }

    #[test]
    fn bound_frame_to_screen_parks_right_offscreen_windows_to_hidden_sliver() {
        let screen = rect(2000.0, 0.0, 1000.0, 800.0);
        let frame = rect(3200.0, 80.0, 600.0, 300.0);
        let bounded = bound_frame_to_screen(frame, screen);
        assert_eq!(bounded.origin.x, 2990.0);
        assert_eq!(bounded.size.width, 600.0);
    }

    #[test]
    fn bound_frame_to_screen_does_not_park_partially_visible_right_windows() {
        let screen = rect(2000.0, 0.0, 1000.0, 800.0);
        let frame = rect(2998.0, 80.0, 600.0, 300.0);
        let bounded = bound_frame_to_screen(frame, screen);
        assert_eq!(bounded.origin.x, 2998.0);
        assert_eq!(bounded.size.width, 600.0);
    }
}
