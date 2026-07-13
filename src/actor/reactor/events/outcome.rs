use objc2_core_foundation::{CGPoint, CGRect};

use crate::actor::app::{AppInfo, Request, WindowId, WindowInfo, pid_t};
use crate::actor::raise_manager;
use crate::actor::wm_controller::{WmCmd, WmEvent};
use crate::common::config::Config;
use crate::layout_engine::{Direction, EventResponse, LayoutEvent};
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

#[derive(Debug)]
pub(crate) struct WindowDiscoveryRequest {
    pub(crate) pid: pid_t,
    pub(crate) new: Vec<(WindowId, WindowInfo)>,
    pub(crate) known_visible: Vec<WindowId>,
    pub(crate) app_info: Option<AppInfo>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WindowFrameWriteRequest {
    pub(crate) window: WindowId,
    pub(crate) frame: CGRect,
    pub(crate) requested: bool,
}

#[derive(Debug)]
pub(crate) struct WindowTitleBroadcast {
    pub(crate) window: WindowId,
    pub(crate) previous_title: String,
    pub(crate) new_title: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TopologyReassignment {
    pub(crate) window: WindowId,
    pub(crate) space: SpaceId,
    pub(crate) preserve_workspace_ordinal: bool,
}

/// Follow-up work requested by an event workflow.
///
/// Workflows mutate reactor-owned domain state synchronously, then describe the
/// ordered integration work which must happen after the mutation.  Keeping the
/// description small and concrete makes it possible to test policy without
/// turning platform operations into a generic effect system.
#[derive(Debug, Default)]
pub(crate) struct EventOutcome {
    pub(crate) window_server_updates: Vec<WindowServerInfo>,
    pub(crate) discoveries: Vec<WindowDiscoveryRequest>,
    pub(crate) activate_application: Option<pid_t>,
    pub(crate) recompute_active_spaces: bool,
    pub(crate) repair_spaces_after_mission_control: bool,
    pub(crate) refresh_after_mission_control: bool,
    pub(crate) force_refresh_all_windows: bool,
    pub(crate) switch_native_space: Option<Direction>,
    pub(crate) wm_commands: Vec<WmCmd>,
    pub(crate) wm_events: Vec<WmEvent>,
    pub(crate) app_requests: Vec<(pid_t, Request)>,
    pub(crate) topology_reassignments: Vec<TopologyReassignment>,
    pub(crate) confirmed_window_spaces: Vec<(WindowServerId, SpaceId)>,
    pub(crate) fullscreen_restorations: Vec<(WindowServerId, SpaceId, WindowId)>,
    pub(crate) raise_requests: Vec<raise_manager::Event>,
    pub(crate) make_key_windows: Vec<(pid_t, WindowServerId)>,
    pub(crate) mouse_warps: Vec<CGPoint>,
    pub(crate) window_frame_writes: Vec<WindowFrameWriteRequest>,
    pub(crate) drag_swap_evaluations: Vec<(WindowId, CGRect)>,
    pub(crate) dispatch_mouse_up: bool,
    pub(crate) close_window: Option<Option<WindowServerId>>,
    pub(crate) service_config_update: Option<(Config, bool)>,
    pub(crate) stdout_lines: Vec<String>,
    pub(crate) reapply_app_rules: Vec<WindowId>,
    pub(crate) finalize_created_windows: Vec<WindowId>,
    pub(crate) window_title_broadcasts: Vec<WindowTitleBroadcast>,
    pub(crate) layout_events: Vec<LayoutEvent>,
    pub(crate) layout_responses: Vec<(EventResponse, Option<SpaceId>)>,
    pub(crate) arrange: ArrangeRequest,
    pub(crate) focused_window: Option<WindowId>,
    pub(crate) refresh_window_notifications: bool,
    pub(crate) refresh_focus_follows_mouse: bool,
    pub(crate) refresh_layout_mode: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ArrangeRequest {
    pub(crate) requested: bool,
    pub(crate) passes: u8,
    pub(crate) is_resize: bool,
    pub(crate) window_was_destroyed: bool,
}

impl EventOutcome {
    /// Combines follow-up work produced by nested reducers while preserving
    /// reducer order for every queued operation.
    pub(crate) fn absorb(&mut self, mut other: Self) {
        self.window_server_updates.append(&mut other.window_server_updates);
        self.discoveries.append(&mut other.discoveries);
        self.activate_application = other.activate_application.or(self.activate_application);
        self.recompute_active_spaces |= other.recompute_active_spaces;
        self.repair_spaces_after_mission_control |= other.repair_spaces_after_mission_control;
        self.refresh_after_mission_control |= other.refresh_after_mission_control;
        self.force_refresh_all_windows |= other.force_refresh_all_windows;
        self.switch_native_space = other.switch_native_space.or(self.switch_native_space);
        self.wm_commands.append(&mut other.wm_commands);
        self.wm_events.append(&mut other.wm_events);
        self.app_requests.append(&mut other.app_requests);
        self.topology_reassignments.append(&mut other.topology_reassignments);
        self.confirmed_window_spaces.append(&mut other.confirmed_window_spaces);
        self.fullscreen_restorations.append(&mut other.fullscreen_restorations);
        self.raise_requests.append(&mut other.raise_requests);
        self.make_key_windows.append(&mut other.make_key_windows);
        self.mouse_warps.append(&mut other.mouse_warps);
        self.window_frame_writes.append(&mut other.window_frame_writes);
        self.drag_swap_evaluations.append(&mut other.drag_swap_evaluations);
        self.dispatch_mouse_up |= other.dispatch_mouse_up;
        self.close_window = other.close_window.or(self.close_window);
        self.service_config_update =
            other.service_config_update.or(self.service_config_update.take());
        self.stdout_lines.append(&mut other.stdout_lines);
        self.reapply_app_rules.append(&mut other.reapply_app_rules);
        self.finalize_created_windows.append(&mut other.finalize_created_windows);
        self.window_title_broadcasts.append(&mut other.window_title_broadcasts);
        self.layout_events.append(&mut other.layout_events);
        self.layout_responses.append(&mut other.layout_responses);
        if other.arrange.requested {
            self.arrange.requested = true;
            self.arrange.passes = self.arrange.passes.saturating_add(other.arrange.passes).max(1);
            self.arrange.is_resize |= other.arrange.is_resize;
            self.arrange.window_was_destroyed |= other.arrange.window_was_destroyed;
        }
        self.focused_window = other.focused_window.or(self.focused_window);
        self.refresh_window_notifications |= other.refresh_window_notifications;
        self.refresh_focus_follows_mouse |= other.refresh_focus_follows_mouse;
        self.refresh_layout_mode |= other.refresh_layout_mode;
    }

    pub(crate) fn finalized_event(
        focused_window: Option<WindowId>,
        is_resize: bool,
        window_was_destroyed: bool,
        refresh_window_notifications: bool,
    ) -> Self {
        Self {
            window_server_updates: Vec::new(),
            discoveries: Vec::new(),
            activate_application: None,
            recompute_active_spaces: false,
            repair_spaces_after_mission_control: false,
            refresh_after_mission_control: false,
            force_refresh_all_windows: false,
            switch_native_space: None,
            wm_commands: Vec::new(),
            wm_events: Vec::new(),
            app_requests: Vec::new(),
            topology_reassignments: Vec::new(),
            confirmed_window_spaces: Vec::new(),
            fullscreen_restorations: Vec::new(),
            raise_requests: Vec::new(),
            make_key_windows: Vec::new(),
            mouse_warps: Vec::new(),
            window_frame_writes: Vec::new(),
            drag_swap_evaluations: Vec::new(),
            dispatch_mouse_up: false,
            close_window: None,
            service_config_update: None,
            stdout_lines: Vec::new(),
            reapply_app_rules: Vec::new(),
            finalize_created_windows: Vec::new(),
            window_title_broadcasts: Vec::new(),
            layout_events: Vec::new(),
            layout_responses: Vec::new(),
            arrange: ArrangeRequest {
                requested: true,
                passes: 1,
                is_resize,
                window_was_destroyed,
            },
            focused_window,
            refresh_window_notifications,
            refresh_focus_follows_mouse: false,
            refresh_layout_mode: true,
        }
    }

    pub(crate) fn with_focus_follows_mouse_refresh(mut self) -> Self {
        self.refresh_focus_follows_mouse = true;
        self
    }

    pub(crate) fn window_notification_refresh() -> Self {
        Self {
            refresh_window_notifications: true,
            ..Self::default()
        }
    }

    pub(crate) fn with_layout_event(mut self, event: LayoutEvent) -> Self {
        self.layout_events.push(event);
        self
    }

    pub(crate) fn with_layout_response(
        mut self,
        response: EventResponse,
        workspace_switch_space: Option<SpaceId>,
    ) -> Self {
        self.layout_responses.push((response, workspace_switch_space));
        self
    }

    pub(crate) fn with_application_activation(mut self, pid: pid_t) -> Self {
        self.activate_application = Some(pid);
        self
    }

    pub(crate) fn with_active_space_recompute(mut self) -> Self {
        self.recompute_active_spaces = true;
        self
    }

    pub(crate) fn with_mission_control_recovery(mut self) -> Self {
        self.repair_spaces_after_mission_control = true;
        self.refresh_after_mission_control = true;
        self
    }

    pub(crate) fn with_force_window_refresh(mut self) -> Self {
        self.force_refresh_all_windows = true;
        self
    }

    pub(crate) fn with_arrange_passes(mut self, passes: u8) -> Self {
        self.arrange.requested = passes > 0;
        self.arrange.passes = passes;
        self
    }

    pub(crate) fn with_window_server_updates(mut self, updates: Vec<WindowServerInfo>) -> Self {
        self.window_server_updates = updates;
        self
    }

    pub(crate) fn with_discovery(mut self, request: WindowDiscoveryRequest) -> Self {
        self.discoveries.push(request);
        self
    }

    pub(crate) fn with_native_space_switch(mut self, direction: Direction) -> Self {
        self.switch_native_space = Some(direction);
        self
    }

    pub(crate) fn with_wm_command(mut self, command: WmCmd) -> Self {
        self.wm_commands.push(command);
        self
    }

    pub(crate) fn with_wm_event(mut self, event: WmEvent) -> Self {
        self.wm_events.push(event);
        self
    }

    pub(crate) fn with_app_request(mut self, pid: pid_t, request: Request) -> Self {
        self.app_requests.push((pid, request));
        self
    }

    pub(crate) fn with_topology_reassignment(
        mut self,
        window: WindowId,
        space: SpaceId,
        preserve_workspace_ordinal: bool,
    ) -> Self {
        self.topology_reassignments.push(TopologyReassignment {
            window,
            space,
            preserve_workspace_ordinal,
        });
        self
    }

    pub(crate) fn with_confirmed_window_space(
        mut self,
        window_server_id: WindowServerId,
        space: SpaceId,
    ) -> Self {
        self.confirmed_window_spaces.push((window_server_id, space));
        self
    }

    pub(crate) fn with_fullscreen_restoration(
        mut self,
        window_server_id: WindowServerId,
        space: SpaceId,
        window: WindowId,
    ) -> Self {
        self.fullscreen_restorations.push((window_server_id, space, window));
        self
    }

    pub(crate) fn with_raise_request(mut self, request: raise_manager::Event) -> Self {
        self.raise_requests.push(request);
        self
    }

    pub(crate) fn with_make_key_window(mut self, pid: pid_t, window: WindowServerId) -> Self {
        self.make_key_windows.push((pid, window));
        self
    }

    pub(crate) fn with_mouse_warp(mut self, point: CGPoint) -> Self {
        self.mouse_warps.push(point);
        self
    }

    pub(crate) fn with_window_frame_write(
        mut self,
        window: WindowId,
        frame: CGRect,
        requested: bool,
    ) -> Self {
        self.window_frame_writes
            .push(WindowFrameWriteRequest { window, frame, requested });
        self
    }

    pub(crate) fn with_drag_swap_evaluation(mut self, window: WindowId, frame: CGRect) -> Self {
        self.drag_swap_evaluations.push((window, frame));
        self
    }

    pub(crate) fn with_mouse_up_dispatch(mut self) -> Self {
        self.dispatch_mouse_up = true;
        self
    }

    pub(crate) fn with_close_window(mut self, window_server_id: Option<WindowServerId>) -> Self {
        self.close_window = Some(window_server_id);
        self
    }

    pub(crate) fn with_service_config_update(mut self, config: Config, keys_changed: bool) -> Self {
        self.service_config_update = Some((config, keys_changed));
        self
    }

    pub(crate) fn with_stdout_line(mut self, line: String) -> Self {
        self.stdout_lines.push(line);
        self
    }

    pub(crate) fn with_app_rule_reapply(mut self, window: WindowId) -> Self {
        self.reapply_app_rules.push(window);
        self
    }

    pub(crate) fn with_created_window_finalization(mut self, window: WindowId) -> Self {
        self.finalize_created_windows.push(window);
        self
    }

    pub(crate) fn with_window_title_broadcast(
        mut self,
        window: WindowId,
        previous_title: String,
        new_title: String,
    ) -> Self {
        self.window_title_broadcasts.push(WindowTitleBroadcast {
            window,
            previous_title,
            new_title,
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalized_events_explicitly_request_all_legacy_follow_up_work() {
        let outcome = EventOutcome::finalized_event(None, true, false, true);

        assert!(outcome.arrange.requested);
        assert!(outcome.arrange.is_resize);
        assert!(!outcome.arrange.window_was_destroyed);
        assert!(outcome.refresh_window_notifications);
        assert!(!outcome.refresh_focus_follows_mouse);
        assert!(outcome.refresh_layout_mode);
    }
}
