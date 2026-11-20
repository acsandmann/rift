use tracing::debug;

use super::super::{Changes, Reactor};
use crate::actor::reactor::SpaceEventKind;
use crate::actor::spaces::ForwardedSpaceState;
use crate::actor::wm_controller::WmEvent;
use crate::common::collections::HashSet;
use crate::model::space_activation::{SpaceActivationConfig, SpaceActivationPolicy};
use crate::model::window_store::NativeDeparture;
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
        && current.screens == incoming.screens
        && current.fullscreen_spaces == incoming.fullscreen_spaces
        && current_effective_active_spaces == &effective_active_spaces
        && current.display_space_ids == incoming.display_space_ids
        && current.membership_complete == incoming.membership_complete
        && incoming.destroyed_spaces.is_empty()
        && !active_window_membership_changed;
    let invalidates_pending_targets = incoming.display_set_changed
        || incoming.should_force_refresh_layout
        || !incoming.space_remaps.is_empty()
        || !incoming.resized_spaces.is_empty();
    SpaceSnapshotAnalysis {
        spaces,
        authoritative_spaces,
        command_space_only_update,
        invalidates_pending_targets,
    }
}

/// Prepends an older sample's deltas to a newer one; geometry and membership stay the newer's.
pub(crate) fn merge_space_deltas(
    mut older: ForwardedSpaceState,
    mut newer: ForwardedSpaceState,
) -> ForwardedSpaceState {
    older.space_remaps.retain(|remap| !newer.space_remaps.contains(remap));
    older.space_remaps.append(&mut newer.space_remaps);
    newer.space_remaps = older.space_remaps;
    older
        .resized_spaces
        .retain(|(space, _)| !newer.resized_spaces.iter().any(|(next, _)| space == next));
    older.resized_spaces.append(&mut newer.resized_spaces);
    newer.resized_spaces = older.resized_spaces;
    newer.destroyed_spaces.extend(older.destroyed_spaces);
    newer.should_force_refresh_layout |= older.should_force_refresh_layout;
    newer.display_set_changed |= older.display_set_changed;
    newer
}

#[derive(Debug)]
pub struct WindowServerDestroyedObservations {
    pub resolved_space: Option<SpaceId>,
    pub ordered_in: Option<bool>,
    pub suitable: Option<bool>,
}

#[derive(Debug)]
pub struct WindowServerAppearedObservations {
    pub resolved_space: Option<SpaceId>,
    pub last_known_user_space: Option<SpaceId>,
    pub window_server_info: Option<crate::sys::window_server::WindowServerInfo>,
}

impl Reactor {
    pub(in crate::actor::reactor) fn reconcile_native_departure(
        &mut self,
        changes: &mut Changes,
        wsid: WindowServerId,
        sid: SpaceId,
        kind: SpaceEventKind,
        observations: WindowServerDestroyedObservations,
    ) {
        let WindowServerDestroyedObservations {
            resolved_space,
            ordered_in,
            suitable,
        } = observations;
        if matches!(kind, SpaceEventKind::Fullscreen) {
            // Departure alone cannot establish entry. Startup recovery uses positive
            // fullscreen appearance/discovery, even if this notification arrives late.
            if let Some(wid) = self.state.windows.tracked_window_id(wsid) {
                changes.inventory(wid.pid);
            }
        } else {
            match self.state.windows.observe_native_departure(
                wsid,
                sid,
                resolved_space,
                &self.active_spaces,
                ordered_in,
                suitable,
            ) {
                NativeDeparture::Moved(Some(wid), space) => {
                    self.reassign_window_to_authoritative_space(changes, wid, space, false);
                }
                NativeDeparture::Closed(wid) => {
                    self.retire_window(changes, wid);
                }
                NativeDeparture::Hidden(Some(wid)) => {
                    self.reconcile_window_membership(changes, wid, None);
                    changes.inventory(wid.pid);
                }
                NativeDeparture::Moved(None, _) | NativeDeparture::Hidden(None) => {}
            }
        }
    }

    pub(in crate::actor::reactor) fn reconcile_native_appearance(
        &mut self,
        changes: &mut Changes,
        wsid: WindowServerId,
        sid: SpaceId,
        kind: SpaceEventKind,
        observations: WindowServerAppearedObservations,
    ) {
        let WindowServerAppearedObservations {
            resolved_space,
            last_known_user_space,
            window_server_info,
        } = observations;
        if matches!(kind, SpaceEventKind::User)
            && let Some(resolved_space) = resolved_space
        {
            if resolved_space != sid {
                self.state.windows.observe_native_space(
                    wsid,
                    resolved_space,
                    self.active_spaces.contains(&resolved_space),
                );
                if let Some(wid) = self.state.windows.tracked_window_id(wsid) {
                    self.reassign_window_to_authoritative_space(
                        changes,
                        wid,
                        resolved_space,
                        false,
                    );
                }
                debug!(
                    ?wsid,
                    reported_space = ?sid,
                    resolved_space = ?resolved_space,
                    "Resolved user-space appearance to stronger native membership"
                );
                return;
            }

            self.state.windows.observe_native_space(wsid, resolved_space, true);
            self.state.windows.note_window_server_fullscreen_exit(wsid, resolved_space);
        }

        let tracked = self.state.windows.tracked_window_id(wsid);
        let known = tracked.is_some()
            || self.state.windows.knows_window_server_id(wsid)
            || self.state.windows.is_window_server_observed(wsid);
        if known && self.is_mission_control_active() {
            return;
        }
        if !known {
            self.state.windows.mark_window_server_observed(wsid);
            if let Some(info) = window_server_info
                && (info.layer != 0
                    || info.frame.size.width < 50.0
                    || info.frame.size.height < 50.0)
            {
                self.state.windows.clear_window_server_observed(wsid);
                return;
            }
        }

        if matches!(kind, SpaceEventKind::Fullscreen) {
            let pid = tracked.map(|wid| wid.pid).or_else(|| {
                self.state
                    .windows
                    .get_window_server_info(wsid)
                    .or(window_server_info)
                    .map(|info| info.pid)
            });
            if pid.is_none() {
                return;
            }
            if let Some(wid) =
                self.state
                    .windows
                    .observe_native_fullscreen(wsid, sid, last_known_user_space, pid)
            {
                self.reconcile_window_membership(changes, wid, None);
            }
            if tracked.is_none()
                && let Some(pid) = pid
            {
                changes.inventory(pid);
            }
            return;
        }
        if let Some(wid) = tracked {
            if self.state.windows.native_fullscreen_record_for_window(wid).is_some() {
                changes.inventory(wid.pid);
            }
            self.reconcile_native_presence(changes, wsid, sid, wid);
        } else if known {
            if let Some(pid) =
                self.state.windows.pending_native_fullscreen_pid_for_window_server_id(wsid)
            {
                changes.inventory(pid);
            }
        } else if let Some(window_server_info) = window_server_info {
            self.update_partial_window_server_info(changes, [window_server_info]);
            let pid = window_server_info.pid;
            if self.app_manager.apps.contains_key(&pid) {
                changes.inventory(pid);
            } else if let Some(wm) = &self.communication_manager.wm_sender
                && let Some(app) =
                    objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(
                        pid,
                    )
            {
                wm.send(WmEvent::AppLaunch(
                    pid,
                    AppInfo::from(&*app),
                    crate::actor::wm_controller::AppDiscoverySource::WindowServer,
                ));
            }
        }
    }
}
