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
    pub(crate) recompute_active_spaces: bool,
    pub(crate) repair_spaces_after_mission_control: bool,
    pub(crate) refresh_after_mission_control: bool,
    pub(crate) refresh_window_inventories: bool,
    pub(crate) switch_native_space: Option<Direction>,
    pub(crate) wm_commands: Vec<WmCmd>,
    pub(crate) wm_events: Vec<WmEvent>,
    pub(crate) app_requests: Vec<(pid_t, Request)>,
    pub(crate) window_inventory_requests: Vec<pid_t>,
    pub(crate) topology_reassignments: Vec<TopologyReassignment>,
    pub(crate) confirmed_window_spaces: Vec<(WindowServerId, SpaceId)>,
    pub(crate) fullscreen_restorations: Vec<(WindowServerId, SpaceId, WindowId)>,
    pub(crate) raise_requests: Vec<raise_manager::Event>,
    pub(crate) make_key_windows: Vec<(pid_t, WindowServerId)>,
    pub(crate) mouse_warps: Vec<CGPoint>,
    pub(crate) post_arrange_mouse_warp: Option<WindowId>,
    pub(crate) pre_layout_window_frame_writes: Vec<WindowFrameWriteRequest>,
    pub(crate) drag_swap_evaluations: Vec<(WindowId, CGRect)>,
    pub(crate) dispatch_mouse_up: bool,
    pub(crate) close_window: Option<Option<WindowServerId>>,
    pub(crate) service_config_update: Option<Config>,
    pub(crate) stdout_lines: Vec<String>,
    pub(crate) reapply_app_rules: Vec<WindowId>,
    pub(crate) finalize_created_windows: Vec<WindowId>,
    pub(crate) window_title_broadcasts: Vec<WindowTitleBroadcast>,
    pub(crate) focused_window_broadcast: Option<WindowId>,
    pub(crate) broadcast_layout_changed: bool,
    pub(crate) broadcast_selection_changed: bool,
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
    pub(crate) space_scope: Option<SpaceId>,
}

impl EventOutcome {
    /// The event was observed, but it does not require any follow-up work.
    pub(crate) fn no_change() -> Self { Self::default() }

    /// Combines follow-up work produced by nested reducers while preserving
    /// reducer order for every queued operation.
    pub(crate) fn absorb(&mut self, mut other: Self) {
        self.window_server_updates.append(&mut other.window_server_updates);
        self.discoveries.append(&mut other.discoveries);
        self.recompute_active_spaces |= other.recompute_active_spaces;
        self.repair_spaces_after_mission_control |= other.repair_spaces_after_mission_control;
        self.refresh_after_mission_control |= other.refresh_after_mission_control;
        self.refresh_window_inventories |= other.refresh_window_inventories;
        self.switch_native_space = other.switch_native_space.or(self.switch_native_space);
        self.wm_commands.append(&mut other.wm_commands);
        self.wm_events.append(&mut other.wm_events);
        self.app_requests.append(&mut other.app_requests);
        self.window_inventory_requests.append(&mut other.window_inventory_requests);
        self.topology_reassignments.append(&mut other.topology_reassignments);
        self.confirmed_window_spaces.append(&mut other.confirmed_window_spaces);
        self.fullscreen_restorations.append(&mut other.fullscreen_restorations);
        self.raise_requests.append(&mut other.raise_requests);
        self.make_key_windows.append(&mut other.make_key_windows);
        self.mouse_warps.append(&mut other.mouse_warps);
        self.post_arrange_mouse_warp =
            other.post_arrange_mouse_warp.or(self.post_arrange_mouse_warp);
        self.pre_layout_window_frame_writes
            .append(&mut other.pre_layout_window_frame_writes);
        self.drag_swap_evaluations.append(&mut other.drag_swap_evaluations);
        self.dispatch_mouse_up |= other.dispatch_mouse_up;
        self.close_window = other.close_window.or(self.close_window);
        self.service_config_update =
            other.service_config_update.or(self.service_config_update.take());
        self.stdout_lines.append(&mut other.stdout_lines);
        self.reapply_app_rules.append(&mut other.reapply_app_rules);
        self.finalize_created_windows.append(&mut other.finalize_created_windows);
        self.window_title_broadcasts.append(&mut other.window_title_broadcasts);
        self.focused_window_broadcast =
            other.focused_window_broadcast.or(self.focused_window_broadcast);
        self.broadcast_layout_changed |= other.broadcast_layout_changed;
        self.broadcast_selection_changed |= other.broadcast_selection_changed;
        self.layout_events.append(&mut other.layout_events);
        self.layout_responses.append(&mut other.layout_responses);
        if other.arrange.requested {
            self.arrange.space_scope = if self.arrange.requested {
                match (self.arrange.space_scope, other.arrange.space_scope) {
                    (Some(existing), Some(other)) if existing == other => Some(existing),
                    _ => None,
                }
            } else {
                other.arrange.space_scope
            };
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

    /// The event changed geometry or layout state and requires one arrange pass.
    pub(crate) fn layout_changed(is_resize: bool) -> Self {
        Self {
            window_server_updates: Vec::new(),
            discoveries: Vec::new(),
            recompute_active_spaces: false,
            repair_spaces_after_mission_control: false,
            refresh_after_mission_control: false,
            refresh_window_inventories: false,
            switch_native_space: None,
            wm_commands: Vec::new(),
            wm_events: Vec::new(),
            app_requests: Vec::new(),
            window_inventory_requests: Vec::new(),
            topology_reassignments: Vec::new(),
            confirmed_window_spaces: Vec::new(),
            fullscreen_restorations: Vec::new(),
            raise_requests: Vec::new(),
            make_key_windows: Vec::new(),
            mouse_warps: Vec::new(),
            post_arrange_mouse_warp: None,
            pre_layout_window_frame_writes: Vec::new(),
            drag_swap_evaluations: Vec::new(),
            dispatch_mouse_up: false,
            close_window: None,
            service_config_update: None,
            stdout_lines: Vec::new(),
            reapply_app_rules: Vec::new(),
            finalize_created_windows: Vec::new(),
            window_title_broadcasts: Vec::new(),
            focused_window_broadcast: None,
            broadcast_layout_changed: true,
            broadcast_selection_changed: false,
            layout_events: Vec::new(),
            layout_responses: Vec::new(),
            arrange: ArrangeRequest {
                requested: true,
                passes: 1,
                is_resize,
                window_was_destroyed: false,
                space_scope: None,
            },
            focused_window: None,
            refresh_window_notifications: false,
            refresh_focus_follows_mouse: false,
            refresh_layout_mode: true,
        }
    }

    /// A window entered, left, or changed its membership in the managed set.
    pub(crate) fn window_membership_changed(
        window_was_destroyed: bool,
        refresh_window_notifications: bool,
    ) -> Self {
        let mut outcome = Self::layout_changed(false);
        outcome.arrange.window_was_destroyed = window_was_destroyed;
        outcome.refresh_window_notifications = refresh_window_notifications;
        outcome
    }

    /// Focus changed without changing window membership.
    pub(crate) fn focus_changed(
        focused_window: Option<WindowId>,
        refresh_window_notifications: bool,
    ) -> Self {
        Self {
            focused_window,
            refresh_window_notifications,
            ..Self::default()
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

    pub(crate) fn with_window_inventory_request(mut self, pid: pid_t) -> Self {
        self.window_inventory_requests.push(pid);
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

    pub(crate) fn with_active_space_recompute(mut self) -> Self {
        self.recompute_active_spaces = true;
        self
    }

    pub(crate) fn with_mission_control_recovery(mut self) -> Self {
        self.repair_spaces_after_mission_control = true;
        self.refresh_after_mission_control = true;
        self
    }

    pub(crate) fn with_window_inventory_refresh(mut self) -> Self {
        self.refresh_window_inventories = true;
        self
    }

    pub(crate) fn with_arrange_passes(mut self, passes: u8) -> Self {
        self.arrange.requested = passes > 0;
        self.arrange.passes = passes;
        self
    }

    pub(crate) fn with_arrange_space_scope(mut self, space_scope: Option<SpaceId>) -> Self {
        self.arrange.space_scope = space_scope;
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

    pub(crate) fn with_post_arrange_mouse_warp(mut self, window: WindowId) -> Self {
        self.post_arrange_mouse_warp = Some(window);
        self
    }

    pub(crate) fn with_pre_layout_window_frame_write(
        mut self,
        window: WindowId,
        frame: CGRect,
        requested: bool,
    ) -> Self {
        self.pre_layout_window_frame_writes.push(WindowFrameWriteRequest {
            window,
            frame,
            requested,
        });
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

    pub(crate) fn with_service_config_update(mut self, config: Config) -> Self {
        self.service_config_update = Some(config);
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

    pub(crate) fn with_focused_window_broadcast(mut self, window: WindowId) -> Self {
        self.focused_window_broadcast = Some(window);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_no_change_request_no_follow_up_work() {
        for outcome in [EventOutcome::default(), EventOutcome::no_change()] {
            assert!(!outcome.arrange.requested);
            assert_eq!(outcome.arrange.passes, 0);
            assert!(!outcome.refresh_window_notifications);
            assert!(!outcome.refresh_layout_mode);
            assert!(!outcome.broadcast_layout_changed);
        }
    }

    #[test]
    fn explicit_change_constructors_request_their_follow_up_work() {
        let outcome = EventOutcome::layout_changed(true);

        assert!(outcome.arrange.requested);
        assert!(outcome.arrange.is_resize);
        assert!(!outcome.arrange.window_was_destroyed);
        assert!(!outcome.refresh_window_notifications);
        assert!(!outcome.refresh_focus_follows_mouse);
        assert!(outcome.refresh_layout_mode);
        assert!(outcome.broadcast_layout_changed);

        let outcome = EventOutcome::window_membership_changed(true, true);
        assert!(outcome.arrange.requested);
        assert!(outcome.arrange.window_was_destroyed);
        assert!(outcome.refresh_window_notifications);

        let focused = WindowId::new(42, 7);
        let outcome = EventOutcome::focus_changed(Some(focused), false);
        assert!(!outcome.arrange.requested);
        assert_eq!(outcome.focused_window, Some(focused));

        let outcome = EventOutcome::no_change().with_focused_window_broadcast(focused);
        assert_eq!(outcome.focused_window_broadcast, Some(focused));
    }

    #[test]
    fn absorbed_arrange_requests_keep_only_a_common_space_scope() {
        let first_space = SpaceId::new(1);
        let second_space = SpaceId::new(2);
        let mut outcome =
            EventOutcome::layout_changed(false).with_arrange_space_scope(Some(first_space));

        outcome.absorb(
            EventOutcome::layout_changed(false).with_arrange_space_scope(Some(first_space)),
        );
        assert_eq!(outcome.arrange.space_scope, Some(first_space));

        outcome.absorb(
            EventOutcome::layout_changed(false).with_arrange_space_scope(Some(second_space)),
        );
        assert_eq!(outcome.arrange.space_scope, None);
        assert!(outcome.broadcast_layout_changed);
    }
}
