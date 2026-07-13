use tracing::{debug, trace};

use crate::actor::app::{Request, WindowId};
use crate::actor::reactor::events::{EventOutcome, window};
use crate::actor::reactor::managers::{DragManager, MissionControlManager};
use crate::actor::reactor::{DragState, LayoutEvent, MissionControlState, SpaceEventKind};
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::WmEvent;
use crate::common::collections::HashSet;
use crate::model::RiftState;
use crate::model::space_activation::{SpaceActivationConfig, SpaceActivationPolicy};
use crate::model::window_store::NativeFullscreenTransition;
use crate::sys::app::AppInfo;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

#[derive(Debug)]
pub(crate) struct SpaceSnapshotAnalysis {
    pub(crate) spaces: Vec<Option<SpaceId>>,
    pub(crate) authoritative_spaces: Vec<Option<SpaceId>>,
    pub(crate) command_space_only_update: bool,
    pub(crate) invalidates_pending_targets: bool,
}

pub(crate) fn analyze_space_snapshot(
    current: &ForwardedSpaceState,
    current_effective_active_spaces: &HashSet<SpaceId>,
    activation_policy: &SpaceActivationPolicy,
    activation_config: SpaceActivationConfig,
    incoming: &ForwardedSpaceState,
) -> SpaceSnapshotAnalysis {
    let active_window_membership_changed =
        current.active_window_spaces != incoming.active_window_spaces;
    let spaces = incoming.screens.iter().map(|screen| screen.space).collect();
    let display_uuids: Vec<Option<String>> =
        incoming.screens.iter().map(|screen| screen.display_uuid_owned()).collect();
    let authoritative_spaces: Vec<Option<SpaceId>> = incoming
        .screens
        .iter()
        .map(|screen| screen.space.filter(|space| incoming.active_spaces.contains(space)))
        .collect();
    let effective_active_spaces = activation_policy
        .compute_active_spaces(activation_config, &authoritative_spaces, &display_uuids)
        .into_iter()
        .flatten()
        .collect();
    let command_space_only_update = !incoming.display_set_changed
        && !incoming.should_force_refresh_layout
        && incoming.space_remaps.is_empty()
        && incoming.resized_spaces.is_empty()
        && incoming.topology_window_delta.is_none()
        && current.screens == incoming.screens
        && current.fullscreen_spaces == incoming.fullscreen_spaces
        && current_effective_active_spaces == &effective_active_spaces
        && current.display_space_ids == incoming.display_space_ids
        && current.last_user_space_by_display == incoming.last_user_space_by_display
        && !active_window_membership_changed;
    let invalidates_pending_targets = incoming.display_set_changed
        || incoming.should_force_refresh_layout
        || !incoming.space_remaps.is_empty()
        || !incoming.resized_spaces.is_empty()
        || incoming.topology_window_delta.is_some();
    SpaceSnapshotAnalysis {
        spaces,
        authoritative_spaces,
        command_space_only_update,
        invalidates_pending_targets,
    }
}

// spacewindowappeared/destroyed happen a lot when a display is connected/disconnected
// since they are literally when a window enters or leaves a space and each display has its own space(s)
// this is functionally a connection dropping to the window server
#[derive(Debug, Clone, Copy)]
pub struct WindowServerLifecyclePayload {
    pub window_server_id: WindowServerId,
    pub space: SpaceId,
    pub kind: SpaceEventKind,
}

#[derive(Debug)]
pub struct WindowServerDestroyedObservations {
    pub resolved_space: Option<SpaceId>,
    pub active_spaces: HashSet<SpaceId>,
    pub mission_control_active: bool,
    pub ordered_in: bool,
    pub assigned_space: Option<SpaceId>,
    pub last_known_user_space: Option<SpaceId>,
}

#[derive(Debug)]
pub struct WindowServerAppearedObservations {
    pub resolved_space: Option<SpaceId>,
    pub active_spaces: HashSet<SpaceId>,
    pub mission_control_active: bool,
    pub assigned_space: Option<SpaceId>,
    pub last_known_user_space: Option<SpaceId>,
    pub window_server_info: Option<crate::sys::window_server::WindowServerInfo>,
    pub app_known: bool,
    pub running_app_info: Option<AppInfo>,
}

pub fn handle_window_server_destroyed(
    state: &mut RiftState,
    transactions: &crate::actor::reactor::transaction_manager::TransactionManager,
    drag: &mut DragManager,
    payload: WindowServerLifecyclePayload,
    observations: WindowServerDestroyedObservations,
) -> anyhow::Result<EventOutcome> {
    let WindowServerLifecyclePayload {
        window_server_id: wsid,
        space: sid,
        kind,
    } = payload;
    let WindowServerDestroyedObservations {
        resolved_space,
        active_spaces,
        mission_control_active,
        ordered_in,
        assigned_space,
        last_known_user_space,
    } = observations;
    let mut outcome = EventOutcome::default();
    if matches!(kind, SpaceEventKind::Fullscreen) {
        let mut layout_changed = false;
        let (_pid, window_id) = if let Some(wid) = state.windows.tracked_window_id(wsid) {
            (wid.pid, Some(wid))
        } else if let Some(info) = state.windows.get_window_server_info(wsid) {
            (info.pid, None)
        } else {
            // We don't know who owned this fullscreen window.
            return Ok(EventOutcome::default());
        };

        record_fullscreen_window(
            state,
            sid,
            Some(_pid),
            window_id,
            Some(wsid),
            last_known_user_space,
        );
        if let (Some(wid), Some(user_space)) = (window_id, last_known_user_space)
            && assigned_space == Some(user_space)
        {
            outcome = outcome.with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
            layout_changed = active_spaces.contains(&user_space);
        }
        if layout_changed && !mission_control_active {
            outcome = outcome.with_arrange_passes(1);
        }

        if let Some(wid) = window_id {
            outcome = outcome.with_app_request(wid.pid, Request::WindowMaybeDestroyed(wid));
        }

        return Ok(outcome);
    } else if matches!(kind, SpaceEventKind::User) {
        if resolved_space.is_some_and(|space| space != sid) {
            let current_space = resolved_space.expect("checked above");
            state.windows.set_window_server_space(wsid, Some(current_space));
            if active_spaces.contains(&current_space) {
                state.windows.mark_window_visible(wsid);
            } else {
                state.windows.mark_window_hidden(wsid);
            }
            if let Some(wid) = state.windows.tracked_window_id(wsid) {
                outcome = outcome
                    .with_topology_reassignment(wid, current_space, false)
                    .with_arrange_passes((!mission_control_active) as u8);
            }
            debug!(
                ?wsid,
                reported_space = ?sid,
                resolved_space = ?current_space,
                "Resolved user-space disappearance to newer native membership"
            );
            return Ok(outcome);
        }

        if let Some(wid) = state.windows.tracked_window_id(wsid) {
            if !ordered_in {
                // since the connection has dropped it wont be shown in space_windows_list
                // so ordered in can be authorative because it doesnt consider
                // ghost windows that sometimes remain
                debug!(
                    ?wid,
                    ?wsid,
                    reported_space = ?sid,
                    "Promoting WindowServer disappearance to immediate WindowDestroyed"
                );
                if let Ok(destroyed_outcome) = window::handle_window_destroyed(
                    state,
                    transactions,
                    drag,
                    window::WindowDestroyedPayload {
                        window: wid,
                        suppress_if_window_alive: false,
                        platform_window_alive: false,
                    },
                ) {
                    outcome.absorb(destroyed_outcome);
                }
                return Ok(outcome);
            }

            state.windows.set_window_server_space(wsid, Some(sid));
            state.windows.mark_window_hidden(wsid);
            let layout_changed = assigned_space == Some(sid);
            if layout_changed {
                outcome =
                    outcome.with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
            }
            if layout_changed && !mission_control_active {
                outcome = outcome.with_arrange_passes(1);
            }
            outcome = outcome.with_app_request(wid.pid, Request::WindowMaybeDestroyed(wid));
        } else {
            state.windows.set_window_server_space(wsid, Some(sid));
            state.windows.mark_window_hidden(wsid);
            debug!(
                ?wsid,
                "Received WindowServerDestroyed for unknown window - ignoring"
            );
        }
        return Ok(outcome);
    }
    Ok(outcome)
}

pub fn handle_window_server_appeared(
    state: &mut RiftState,
    payload: WindowServerLifecyclePayload,
    observations: WindowServerAppearedObservations,
) -> anyhow::Result<EventOutcome> {
    let WindowServerLifecyclePayload {
        window_server_id: wsid,
        space: sid,
        kind,
    } = payload;
    let WindowServerAppearedObservations {
        resolved_space,
        active_spaces,
        mission_control_active,
        assigned_space,
        last_known_user_space,
        window_server_info,
        app_known,
        running_app_info,
    } = observations;
    let mut outcome = EventOutcome::default();
    if matches!(kind, SpaceEventKind::User) {
        if let Some(resolved_space) = resolved_space {
            if resolved_space != sid {
                state.windows.set_window_server_space(wsid, Some(resolved_space));
                if active_spaces.contains(&resolved_space) {
                    state.windows.mark_window_visible(wsid);
                } else {
                    state.windows.mark_window_hidden(wsid);
                }
                if let Some(wid) = state.windows.tracked_window_id(wsid) {
                    outcome = outcome
                        .with_topology_reassignment(wid, resolved_space, false)
                        .with_arrange_passes((!mission_control_active) as u8);
                }
                debug!(
                    ?wsid,
                    reported_space = ?sid,
                    resolved_space = ?resolved_space,
                    "Resolved user-space appearance to stronger native membership"
                );
                return Ok(outcome);
            }

            state.windows.set_window_server_space(wsid, Some(resolved_space));
            state.windows.mark_window_visible(wsid);
            outcome = outcome.with_confirmed_window_space(wsid, resolved_space);
        }
    }

    if state.windows.knows_window_server_id(wsid) || state.windows.is_window_server_observed(wsid) {
        if !mission_control_active {
            match kind {
                SpaceEventKind::User => {
                    if let Some(wid) = state.windows.tracked_window_id(wsid) {
                        outcome = outcome
                            .with_fullscreen_restoration(wsid, sid, wid)
                            .with_arrange_passes(1);
                    } else if let Some(pid) =
                        state.windows.pending_native_fullscreen_pid_for_window_server_id(wsid)
                    {
                        outcome = outcome.with_app_request(pid, Request::GetVisibleWindows);
                    }
                }
                SpaceEventKind::Fullscreen => {
                    let mut layout_changed = false;
                    let tracked_window_id = state.windows.tracked_window_id(wsid);
                    let owner_pid = tracked_window_id.map(|wid| wid.pid).or_else(|| {
                        state.windows.get_window_server_info(wsid).map(|info| info.pid)
                    });
                    record_fullscreen_window(
                        state,
                        sid,
                        owner_pid,
                        tracked_window_id,
                        Some(wsid),
                        last_known_user_space,
                    );
                    if tracked_window_id.is_none()
                        && let Some(pid) = owner_pid
                    {
                        outcome = outcome.with_app_request(pid, Request::GetVisibleWindows);
                    }
                    if let Some(wid) = tracked_window_id {
                        if let Some(user_space) = last_known_user_space
                            && assigned_space == Some(user_space)
                        {
                            outcome = outcome
                                .with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                            layout_changed = active_spaces.contains(&user_space);
                        }
                    }
                    if layout_changed {
                        outcome = outcome.with_arrange_passes(1);
                    }
                }
            }
        }
        debug!(
            ?wsid,
            "Received WindowServerAppeared for known window - ignoring"
        );
        return Ok(outcome);
    }

    state.windows.mark_window_server_observed(wsid);
    // TODO: figure out why this is happening, we should really know about this app,
    // why dont we get notifications that its being launched?
    if let Some(window_server_info) = window_server_info {
        if window_server_info.layer != 0 {
            trace!(
                ?wsid,
                layer = window_server_info.layer,
                "Ignoring non-normal window"
            );
            return Ok(outcome);
        }

        // Filter out very small windows (likely tooltips or similar UI elements)
        // that shouldn't be managed by the window manager
        const MIN_MANAGEABLE_WINDOW_SIZE: f64 = 50.0;
        if window_server_info.frame.size.width < MIN_MANAGEABLE_WINDOW_SIZE
            || window_server_info.frame.size.height < MIN_MANAGEABLE_WINDOW_SIZE
        {
            trace!(
                ?wsid,
                "Ignoring tiny window ({}x{}) - likely tooltip",
                window_server_info.frame.size.width,
                window_server_info.frame.size.height
            );
            return Ok(outcome);
        }

        if matches!(kind, SpaceEventKind::Fullscreen) {
            let window_id = state.windows.tracked_window_id(wsid);
            record_fullscreen_window(
                state,
                sid,
                Some(window_server_info.pid),
                window_id,
                Some(wsid),
                last_known_user_space,
            );
            outcome = outcome.with_app_request(window_server_info.pid, Request::GetVisibleWindows);

            return Ok(outcome);
        }

        outcome = outcome.with_window_server_updates(vec![window_server_info]);

        if !app_known {
            if let Some(app_info) = running_app_info {
                outcome =
                    outcome.with_wm_event(WmEvent::AppLaunch(window_server_info.pid, app_info));
            }
        } else {
            outcome = outcome.with_app_request(window_server_info.pid, Request::GetVisibleWindows);
        }
    }
    Ok(outcome)
}

pub fn handle_mission_control_native_entered(
    mission_control: &mut MissionControlManager,
    drag: &mut DragManager,
) -> anyhow::Result<EventOutcome> {
    drag.reset();
    drag.drag_state = DragState::Inactive;
    drag.skip_layout_for_window = None;
    let changed = !matches!(
        mission_control.mission_control_state,
        MissionControlState::Active
    );
    mission_control.mission_control_state = MissionControlState::Active;
    let outcome = EventOutcome::finalized_event(None, false, false, false);
    Ok(if changed {
        outcome.with_focus_follows_mouse_refresh()
    } else {
        outcome
    })
}

pub fn handle_mission_control_native_exited(
    mission_control: &mut MissionControlManager,
) -> anyhow::Result<EventOutcome> {
    let changed = matches!(
        mission_control.mission_control_state,
        MissionControlState::Active
    );
    mission_control.mission_control_state = MissionControlState::Inactive;
    let outcome =
        EventOutcome::finalized_event(None, false, false, false).with_mission_control_recovery();
    Ok(if changed {
        outcome.with_focus_follows_mouse_refresh()
    } else {
        outcome
    })
}

#[derive(Debug, Clone, Copy)]
pub struct SpaceLifecyclePayload {
    pub space: SpaceId,
    pub created: bool,
}

pub fn handle_space_lifecycle(
    policy: &mut SpaceActivationPolicy,
    payload: SpaceLifecyclePayload,
) -> anyhow::Result<EventOutcome> {
    if payload.created {
        policy.on_space_created(payload.space);
    } else {
        policy.on_space_destroyed(payload.space);
    }
    Ok(EventOutcome::finalized_event(None, false, false, false).with_active_space_recompute())
}
pub(crate) fn resolve_last_known_user_space(
    window_space: Option<SpaceId>,
    fallback_space: Option<SpaceId>,
) -> Option<SpaceId> {
    window_space.or(fallback_space)
}

fn record_fullscreen_window(
    state: &mut RiftState,
    sid: SpaceId,
    pid: Option<i32>,
    window_id: Option<WindowId>,
    window_server_id: Option<WindowServerId>,
    last_known_user_space: Option<SpaceId>,
) {
    let resolved_window_id = window_id
        .or_else(|| window_server_id.and_then(|wsid| state.windows.tracked_window_id(wsid)));
    if let Some(window_id) = resolved_window_id {
        let _ = state.windows.suspend_window_to_native_fullscreen(
            window_id,
            window_server_id,
            last_known_user_space,
            sid,
            NativeFullscreenTransition::Suspended,
        );
    } else if let (Some(pid), Some(wsid)) = (pid, window_server_id) {
        let _ = state.windows.suspend_window_server_to_native_fullscreen(
            pid,
            wsid,
            last_known_user_space,
            sid,
            NativeFullscreenTransition::Suspended,
        );
    }
}

#[cfg(test)]
mod workflow_tests {
    use super::*;

    #[test]
    fn last_known_user_space_prefers_window_observation() {
        let observed = SpaceId::new(2);
        let fallback = SpaceId::new(1);
        assert_eq!(
            resolve_last_known_user_space(Some(observed), Some(fallback)),
            Some(observed)
        );
        assert_eq!(
            resolve_last_known_user_space(None, Some(fallback)),
            Some(fallback)
        );
    }
}
