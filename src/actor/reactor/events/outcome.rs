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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseWindowRequest {
    Focused,
    Window(WindowServerId),
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
    pub(crate) recover_after_mission_control: bool,
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
    pub(crate) close_window: Option<CloseWindowRequest>,
    pub(crate) service_config_update: Option<Config>,
    pub(crate) stdout_lines: Vec<String>,
    pub(crate) reapply_app_rules: Vec<WindowId>,
    pub(crate) finalize_created_windows: Vec<WindowId>,
    pub(crate) window_title_broadcasts: Vec<WindowTitleBroadcast>,
    pub(crate) focused_window_broadcast: Option<WindowId>,
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
        self.recover_after_mission_control |= other.recover_after_mission_control;
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
        self.broadcast_selection_changed |= other.broadcast_selection_changed;
        self.layout_events.append(&mut other.layout_events);
        self.layout_responses.append(&mut other.layout_responses);
        if other.arrange.passes > 0 {
            self.arrange.space_scope = if self.arrange.passes > 0 {
                match (self.arrange.space_scope, other.arrange.space_scope) {
                    (Some(existing), Some(other)) if existing == other => Some(existing),
                    _ => None,
                }
            } else {
                other.arrange.space_scope
            };
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
            arrange: ArrangeRequest {
                passes: 1,
                is_resize,
                window_was_destroyed: false,
                space_scope: None,
            },
            refresh_layout_mode: true,
            ..Self::default()
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

    pub(crate) fn with_arrange_passes(mut self, passes: u8) -> Self {
        self.arrange.passes = passes;
        self
    }

    pub(crate) fn with_arrange_space_scope(mut self, space_scope: Option<SpaceId>) -> Self {
        self.arrange.space_scope = space_scope;
        self
    }

    pub(crate) fn with_created_window_finalization(mut self, window: WindowId) -> Self {
        self.finalize_created_windows.push(window);
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

    pub(crate) fn with_raise_request(mut self, request: raise_manager::Event) -> Self {
        self.raise_requests.push(request);
        self
    }

    pub(crate) fn with_mouse_warp(mut self, point: CGPoint) -> Self {
        self.mouse_warps.push(point);
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

    pub(crate) fn with_close_window(mut self, window_server_id: Option<WindowServerId>) -> Self {
        self.close_window = Some(match window_server_id {
            Some(window) => CloseWindowRequest::Window(window),
            None => CloseWindowRequest::Focused,
        });
        self
    }

    pub(crate) fn with_stdout_line(mut self, line: String) -> Self {
        self.stdout_lines.push(line);
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
    }

    #[test]
    fn zero_pass_arrange_does_not_merge_resize_or_scope() {
        let space = SpaceId::new(1);
        let mut outcome = EventOutcome::layout_changed(false).with_arrange_space_scope(Some(space));
        outcome.absorb(
            EventOutcome::layout_changed(true)
                .with_arrange_space_scope(None)
                .with_arrange_passes(0),
        );

        assert_eq!(outcome.arrange.passes, 1);
        assert_eq!(outcome.arrange.space_scope, Some(space));
        assert!(!outcome.arrange.is_resize);
    }

    #[test]
    fn close_target_intent_is_named() {
        let window = WindowServerId::new(7);
        assert_eq!(
            EventOutcome::no_change().with_close_window(None).close_window,
            Some(CloseWindowRequest::Focused)
        );
        assert_eq!(
            EventOutcome::no_change().with_close_window(Some(window)).close_window,
            Some(CloseWindowRequest::Window(window))
        );
    }
}
