use tracing::{debug, trace};

use crate::actor::app::Request;
use crate::actor::reactor::events::{EventOutcome, window};
use crate::actor::reactor::managers::{DragManager, MissionControlManager};
use crate::actor::reactor::{LayoutEvent, MissionControlState, SpaceEventKind};
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::WmEvent;
use crate::common::collections::HashSet;
use crate::model::RiftState;
use crate::model::space_activation::{SpaceActivationConfig, SpaceActivationPolicy};
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
        && current.membership_complete == incoming.membership_complete
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
    pub ordered_in: Option<bool>,
    pub last_known_user_space: Option<SpaceId>,
}

#[derive(Debug)]
pub struct WindowServerAppearedObservations {
    pub resolved_space: Option<SpaceId>,
    pub active_spaces: HashSet<SpaceId>,
    pub mission_control_active: bool,
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
        ordered_in,
        last_known_user_space,
    } = observations;
    let mut outcome = EventOutcome::default();
    if matches!(kind, SpaceEventKind::Fullscreen) {
        if let Some(wid) =
            state.windows.observe_native_fullscreen(wsid, sid, last_known_user_space, None)
        {
            outcome = outcome.with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
        }
        if let Some(wid) = state.windows.tracked_window_id(wsid) {
            outcome = outcome.with_app_request(wid.pid, Request::WindowMaybeDestroyed(wid));
        }

        return Ok(outcome);
    } else if matches!(kind, SpaceEventKind::User) {
        match state.windows.observe_native_departure(
            wsid,
            sid,
            resolved_space,
            &active_spaces,
            ordered_in,
        ) {
            crate::model::window_store::NativeDeparture::Moved(window, space) => {
                if let Some(wid) = window {
                    outcome = outcome.with_topology_reassignment(wid, space, false);
                }
            }
            crate::model::window_store::NativeDeparture::Closed(wid) => {
                outcome.absorb(window::handle_window_destroyed(
                    state,
                    transactions,
                    drag,
                    window::WindowDestroyedPayload { window: wid },
                ));
            }
            crate::model::window_store::NativeDeparture::Hidden { window, remove_projection } => {
                if let Some(wid) = window {
                    if remove_projection {
                        outcome = outcome
                            .with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                    }
                    outcome = outcome.with_app_request(wid.pid, Request::WindowMaybeDestroyed(wid));
                }
            }
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
        last_known_user_space,
        window_server_info,
        app_known,
        running_app_info,
    } = observations;
    let mut outcome = EventOutcome::default();
    if matches!(kind, SpaceEventKind::User) {
        if let Some(resolved_space) = resolved_space {
            if resolved_space != sid {
                state.windows.observe_native_space(
                    wsid,
                    resolved_space,
                    active_spaces.contains(&resolved_space),
                );
                if let Some(wid) = state.windows.tracked_window_id(wsid) {
                    outcome = outcome.with_topology_reassignment(wid, resolved_space, false);
                }
                debug!(
                    ?wsid,
                    reported_space = ?sid,
                    resolved_space = ?resolved_space,
                    "Resolved user-space appearance to stronger native membership"
                );
                return Ok(outcome);
            }

            state.windows.observe_native_space(wsid, resolved_space, true);
            outcome.confirmed_window_spaces.push((wsid, resolved_space));
        }
    }

    if state.windows.knows_window_server_id(wsid) || state.windows.is_window_server_observed(wsid) {
        if !mission_control_active {
            match kind {
                SpaceEventKind::User => {
                    if let Some(wid) = state.windows.tracked_window_id(wsid) {
                        outcome.fullscreen_restorations.push((wsid, sid, wid));
                    } else if let Some(pid) =
                        state.windows.pending_native_fullscreen_pid_for_window_server_id(wsid)
                    {
                        outcome = outcome.with_window_inventory_request(pid);
                    }
                }
                SpaceEventKind::Fullscreen => {
                    let tracked_window_id = state.windows.tracked_window_id(wsid);
                    let owner_pid = tracked_window_id.map(|wid| wid.pid).or_else(|| {
                        state.windows.get_window_server_info(wsid).map(|info| info.pid)
                    });
                    if let Some(wid) = state.windows.observe_native_fullscreen(
                        wsid,
                        sid,
                        last_known_user_space,
                        owner_pid,
                    ) {
                        outcome = outcome
                            .with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(wid));
                    }
                    if tracked_window_id.is_none()
                        && let Some(pid) = owner_pid
                    {
                        outcome = outcome.with_window_inventory_request(pid);
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
            state.windows.clear_window_server_observed(wsid);
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
            state.windows.clear_window_server_observed(wsid);
            trace!(
                ?wsid,
                "Ignoring tiny window ({}x{}) - likely tooltip",
                window_server_info.frame.size.width,
                window_server_info.frame.size.height
            );
            return Ok(outcome);
        }

        if matches!(kind, SpaceEventKind::Fullscreen) {
            state.windows.observe_native_fullscreen(
                wsid,
                sid,
                last_known_user_space,
                Some(window_server_info.pid),
            );
            outcome = outcome.with_window_inventory_request(window_server_info.pid);

            return Ok(outcome);
        }

        outcome = outcome.with_window_server_updates(vec![window_server_info]);

        if !app_known {
            if let Some(app_info) = running_app_info {
                outcome.wm_events.push(WmEvent::AppLaunch(
                    window_server_info.pid,
                    app_info,
                    crate::actor::wm_controller::AppDiscoverySource::WindowServer,
                ));
            }
        } else {
            outcome = outcome.with_window_inventory_request(window_server_info.pid);
        }
    }
    Ok(outcome)
}

pub fn handle_mission_control_native_entered(
    mission_control: &mut MissionControlManager,
    drag: &mut DragManager,
) -> anyhow::Result<EventOutcome> {
    drag.suppress_preview(true);
    drag.reset();
    let changed = !matches!(
        mission_control.mission_control_state,
        MissionControlState::Active
    );
    mission_control.mission_control_state = MissionControlState::Active;
    let outcome = EventOutcome::focus_changed(None, false);
    Ok(if changed {
        outcome.with_focus_follows_mouse_refresh()
    } else {
        outcome
    })
}

pub fn handle_mission_control_native_exited(
    mission_control: &mut MissionControlManager,
    drag: &mut DragManager,
) -> anyhow::Result<EventOutcome> {
    drag.suppress_preview(false);
    let changed = matches!(
        mission_control.mission_control_state,
        MissionControlState::Active
    );
    mission_control.mission_control_state = MissionControlState::Inactive;
    let mut outcome = EventOutcome::layout_changed(false);
    outcome.recover_after_mission_control = true;
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
    Ok(EventOutcome::layout_changed(false).with_active_space_recompute())
}
