use tracing::{debug, trace, warn};

use super::window;
use crate::actor::app::{AppInfo, WindowId, WindowInfo, pid_t};
use crate::actor::reactor::{LayoutEvent, WindowFilter, WindowState, utils};
use crate::common::collections::{BTreeMap, HashMap, HashSet};
use crate::model::virtual_workspace::{AppRuleResult, WorkspaceError};
use crate::sys::screen::SpaceId;
use crate::sys::window_server::WindowServerId;

/// Handler for window discovery events, responsible for processing newly discovered windows
/// and managing the lifecycle of window state in the reactor.
fn sync_existing_window_state(
    state: &mut crate::model::RiftState,
    wid: WindowId,
    info: &WindowInfo,
    active_space: Option<SpaceId>,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    let was_minimized = state.windows.window(wid).is_some_and(|window| window.info.is_minimized);
    let was_manageable = state
        .windows
        .window(wid)
        .is_some_and(|window| window.matches_filter(WindowFilter::EffectivelyManageable));

    if let Some(existing) = state.windows.window_mut(wid) {
        existing.info.title = info.title.clone();
        if info.frame.size.width != 0.0 || info.frame.size.height != 0.0 {
            existing.frame_monotonic = info.frame;
        }
        existing.info.is_standard = info.is_standard;
        existing.info.is_root = info.is_root;
        existing.info.is_resizable = info.is_resizable;
        existing.info.min_size = info.min_size;
        existing.info.max_size = info.max_size;
        existing.info.sys_id = info.sys_id;
        existing.info.bundle_id = info.bundle_id.clone();
        existing.info.path = info.path.clone();
        existing.info.ax_role = info.ax_role.clone();
        existing.info.ax_subrole = info.ax_subrole.clone();
    } else {
        return Ok(crate::actor::reactor::events::EventOutcome::default());
    }

    let outcome = match (was_minimized, info.is_minimized) {
        (false, true) => window::handle_window_minimized(state, wid)?,
        (true, false) => {
            window::handle_window_deminiaturized(state, window::WindowDeminiaturizedPayload {
                window: wid,
                active_space,
            })?
        }
        _ => {
            let manageable = utils::compute_window_manageability(
                info.sys_id,
                info.is_minimized,
                info.is_standard,
                info.is_root,
                |wsid| state.windows.get_window_server_info(wsid),
            );
            if let Some(existing) = state.windows.window_mut(wid) {
                existing.info.is_minimized = info.is_minimized;
                existing.is_manageable = manageable;
            }
            if was_manageable && !manageable {
                crate::actor::reactor::events::EventOutcome::default()
                    .with_layout_event(LayoutEvent::WindowRemoved(wid))
            } else {
                crate::actor::reactor::events::EventOutcome::default()
            }
        }
    };

    if was_minimized != info.is_minimized {
        debug!(
            ?wid,
            was_minimized,
            is_minimized = info.is_minimized,
            "Window minimize state reconciled from discovery"
        );
    }
    Ok(outcome)
}

fn should_emit_window_for_space(
    state: &crate::model::RiftState,
    layout: &crate::actor::reactor::managers::LayoutManager,
    space: SpaceId,
    wid: WindowId,
) -> bool {
    let engine = &layout.layout_engine;
    let assigned_workspace =
        engine
            .virtual_workspace_manager()
            .workspace_for_window(&state.windows, space, wid);
    let active_workspace = engine.active_workspace(space);

    match (assigned_workspace, active_workspace) {
        (Some(assigned), Some(active)) => assigned == active,
        _ => true,
    }
}

fn sync_window_server_id_mapping(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    wid: WindowId,
    old_sys_id: Option<WindowServerId>,
    new_sys_id: Option<WindowServerId>,
    current_native_space: Option<SpaceId>,
) -> crate::actor::reactor::events::EventOutcome {
    let mut outcome = crate::actor::reactor::events::EventOutcome::default();
    if old_sys_id != new_sys_id
        && let Some(old_wsid) = old_sys_id
        && state.windows.tracked_window_id(old_wsid) == Some(wid)
    {
        state.windows.remove_window_server_state(old_wsid);
    }

    if let Some(new_wsid) = new_sys_id {
        if let Some(previous_wid) = state.windows.track_window_server_id(new_wsid, wid)
            && previous_wid != wid
        {
            state.windows.transfer_persistent_window_metadata(previous_wid, wid);
            layout.layout_engine.transfer_persistent_window_identity(previous_wid, wid);
            outcome =
                outcome.with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(previous_wid));
            state.windows.remove_window(previous_wid);
        }
        if let (Some(record), Some(current_space)) = (
            state.windows.native_fullscreen_record_for_window(wid),
            current_native_space,
        ) {
            let target_user_space = record
                .workspace
                .map(|workspace| workspace.space)
                .or(record.last_known_user_space);
            if current_space != record.fullscreen_space && Some(current_space) == target_user_space
            {
                let _ = state.windows.restore_window_from_native_fullscreen(wid);
            }
        }
    }
    outcome
}

/// Identify windows that should be removed as stale.
#[derive(Debug)]
pub(crate) struct StaleCleanupSnapshot {
    pub(crate) pending_refresh: bool,
    pub(crate) suppressed: bool,
    pub(crate) mission_control_active: bool,
    pub(crate) drag_active: bool,
    pub(crate) inactive_windows: HashSet<WindowId>,
    pub(crate) server_observations: HashMap<WindowServerId, StaleWindowObservation>,
}

#[derive(Debug)]
pub(crate) struct StaleWindowObservation {
    pub(crate) info: Option<crate::sys::window_server::WindowServerInfo>,
    pub(crate) suitable: bool,
    pub(crate) ordered_in: bool,
}

pub(crate) fn identify_stale_windows(
    state: &crate::model::RiftState,
    pid: pid_t,
    known_visible: &[WindowId],
    snapshot: &StaleCleanupSnapshot,
) -> (Vec<WindowId>, bool) {
    const MIN_REAL_WINDOW_DIMENSION: f64 = 2.0;

    let known_visible_set: HashSet<WindowId> = known_visible.iter().cloned().collect();
    let pending_refresh = snapshot.pending_refresh;

    let has_window_server_visibles_without_ax = {
        let known_visible_set = &known_visible_set;
        state
            .windows
            .iter_visible_window_server_ids()
            .filter_map(|wsid| state.windows.tracked_window_id(wsid))
            .any(|wid| wid.pid == pid && !known_visible_set.contains(&wid))
    };
    // TODO: Rewrite it
    let has_visible_window_server_ids = state
        .windows
        .iter_visible_window_server_ids()
        .any(|wsid| state.windows.tracked_window_id(wsid).is_some_and(|wid| wid.pid == pid));
    let skip_stale_cleanup = snapshot.suppressed
        || pending_refresh
        || snapshot.mission_control_active
        || snapshot.drag_active
        || (known_visible_set.is_empty() && !has_visible_window_server_ids)
        || has_window_server_visibles_without_ax;

    if skip_stale_cleanup {
        return (Vec::new(), false);
    }

    let stale_windows = state
        .windows
        .iter_windows()
        .filter_map(|(wid, window_state)| {
            if wid.pid != pid || known_visible_set.contains(&wid) {
                return None;
            }

            if window_state.info.is_minimized {
                return None;
            }

            let Some(ws_id) = window_state.info.sys_id else {
                trace!(
                    ?wid,
                    "Skipping stale cleanup for window without window server id"
                );
                return None;
            };

            if snapshot.inactive_windows.contains(&wid) {
                trace!(
                    ?wid,
                    ws_id = ?ws_id,
                    "Skipping stale cleanup; window is on a known inactive space"
                );
                return None;
            }

            let observation = snapshot.server_observations.get(&ws_id)?;
            let info = match observation.info.as_ref() {
                Some(info) => info,
                None => {
                    trace!(
                        ?wid,
                        ws_id = ?ws_id,
                        "Skipping stale cleanup for window without server info"
                    );
                    return None;
                }
            };

            let width = info.frame.size.width.abs();
            let height = info.frame.size.height.abs();

            let unsuitable = !observation.suitable;
            let invalid_layer = info.layer != 0;
            let too_small = width < MIN_REAL_WINDOW_DIMENSION || height < MIN_REAL_WINDOW_DIMENSION;
            let ordered_in = observation.ordered_in;
            let visible_in_snapshot = state.windows.is_window_visible(ws_id);

            if unsuitable || invalid_layer || too_small || (!ordered_in && !visible_in_snapshot) {
                Some(wid)
            } else {
                None
            }
        })
        .collect();

    (stale_windows, pending_refresh)
}

/// Remove stale windows and send events.
pub(crate) fn cleanup_stale_windows(
    state: &mut crate::model::RiftState,
    transactions: &crate::actor::reactor::transaction_manager::TransactionManager,
    drag: &mut crate::actor::reactor::managers::DragManager,
    mission_control: &mut crate::actor::reactor::managers::MissionControlManager,
    pid: pid_t,
    stale_windows: Vec<WindowId>,
    pending_refresh: bool,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    let mut outcome = crate::actor::reactor::events::EventOutcome::default();
    for wid in stale_windows {
        outcome.absorb(window::handle_window_destroyed(
            state,
            transactions,
            drag,
            window::WindowDestroyedPayload {
                window: wid,
                platform_window_alive: false,
            },
        )?);
    }
    if pending_refresh {
        mission_control.pending_mission_control_refresh.remove(&pid);
    }
    Ok(outcome)
}

/// Process new and updated windows, returning lists of new and updated windows.
#[derive(Debug)]
pub(crate) struct ObservedWindow {
    pub(crate) wid: WindowId,
    pub(crate) info: WindowInfo,
    pub(crate) current_native_space: Option<SpaceId>,
    pub(crate) active_space: Option<SpaceId>,
}

pub(crate) fn process_window_list(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    observed: Vec<ObservedWindow>,
    app_info: &Option<AppInfo>,
) -> (
    Vec<(WindowId, WindowInfo)>,
    crate::actor::reactor::events::EventOutcome,
) {
    const APP_RULE_TTL_MS: u64 = 1000;

    let mut new_windows = Vec::new();
    let mut outcome = crate::actor::reactor::events::EventOutcome::default();

    state.windows.purge_expired(APP_RULE_TTL_MS);

    let any_recent = observed.iter().any(|window| {
        let info = &window.info;
        info.sys_id
            .map_or(false, |wsid| state.windows.is_wsid_recent(wsid, APP_RULE_TTL_MS))
    });

    if any_recent && app_info.is_none() && !observed.is_empty() {
        // Update state for any newly reported windows, but do not early-return;
        // proceed to emit WindowsOnScreenUpdated so existing mappings are respected
        // without reapplying app rules.
        for window in &observed {
            let wid = window.wid;
            let info = &window.info;
            if state.windows.contains_window(wid) {
                let old_sys_id = state.windows.window(wid).and_then(|window| window.info.sys_id);
                outcome.absorb(sync_window_server_id_mapping(
                    state,
                    layout,
                    wid,
                    old_sys_id,
                    info.sys_id,
                    window.current_native_space,
                ));
                if let Ok(existing_outcome) =
                    sync_existing_window_state(state, wid, info, window.active_space)
                {
                    outcome.absorb(existing_outcome);
                }
            } else {
                let mut window_state: WindowState = WindowState::from((*info).clone());
                let manageable = utils::compute_window_manageability(
                    window_state.info.sys_id,
                    window_state.info.is_minimized,
                    window_state.info.is_standard,
                    window_state.info.is_root,
                    |wsid| state.windows.get_window_server_info(wsid),
                );
                window_state.is_manageable = manageable;
                state.windows.insert_window(wid, window_state);
            }
            outcome.absorb(sync_window_server_id_mapping(
                state,
                layout,
                wid,
                None,
                info.sys_id,
                window.current_native_space,
            ));
        }
        // fall through
    }

    // Process all new windows
    for window in observed {
        let ObservedWindow {
            wid,
            info,
            current_native_space,
            active_space,
        } = window;
        if state.windows.contains_window(wid) {
            let old_sys_id = state.windows.window(wid).and_then(|window| window.info.sys_id);
            outcome.absorb(sync_window_server_id_mapping(
                state,
                layout,
                wid,
                old_sys_id,
                info.sys_id,
                current_native_space,
            ));
            if let Ok(existing_outcome) =
                sync_existing_window_state(state, wid, &info, active_space)
            {
                outcome.absorb(existing_outcome);
            }
        } else {
            outcome.absorb(sync_window_server_id_mapping(
                state,
                layout,
                wid,
                None,
                info.sys_id,
                current_native_space,
            ));
            new_windows.push((wid, info));
        }
    }

    (new_windows, outcome)
}

/// Inserts the newly discovered window snapshots into domain state.
pub(crate) fn update_window_states(
    rift_state: &mut crate::model::RiftState,
    new_windows: Vec<(WindowId, WindowInfo)>,
) {
    // Update or insert window states
    for (wid, info) in new_windows {
        let mut state: WindowState = info.into();
        let manageable = utils::compute_window_manageability(
            state.info.sys_id,
            state.info.is_minimized,
            state.info.is_standard,
            state.info.is_root,
            |wsid| rift_state.windows.get_window_server_info(wsid),
        );
        state.is_manageable = manageable;
        rift_state.windows.insert_window(wid, state);
    }
}

/// Send layout events for discovered windows.
fn assign_discovered_window_to_space(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    wid: WindowId,
    space: SpaceId,
    app_info: &Option<AppInfo>,
) -> Result<AppRuleResult, WorkspaceError> {
    let Some(window) = state.windows.window(wid) else {
        return Err(WorkspaceError::AssignmentFailed);
    };
    let title = window.info.title.clone();
    let ax_role = window.info.ax_role.clone();
    let ax_subrole = window.info.ax_subrole.clone();

    layout.layout_engine.assign_window_with_app_info(
        &mut state.windows,
        wid,
        space,
        app_info.as_ref().and_then(|a| a.bundle_id.as_deref()),
        app_info.as_ref().and_then(|a| a.localized_name.as_deref()),
        Some(title.as_str()),
        ax_role.as_deref(),
        ax_subrole.as_deref(),
    )
}

fn apply_assignment_result(
    state: &mut crate::model::RiftState,
    layout: &crate::actor::reactor::managers::LayoutManager,
    wid: WindowId,
    space: SpaceId,
    assign_result: Result<AppRuleResult, WorkspaceError>,
) -> crate::actor::reactor::events::EventOutcome {
    let mut outcome = crate::actor::reactor::events::EventOutcome::default();
    match assign_result {
        Ok(AppRuleResult::Managed(_)) => {
            if let Some(window) = state.windows.window_mut(wid) {
                window.ignore_app_rule = false;
            }
        }
        Ok(AppRuleResult::Unmanaged) => {
            if let Some(window) = state.windows.window_mut(wid) {
                window.ignore_app_rule = true;
            }
            let needs_removal = {
                let engine = &layout.layout_engine;
                engine
                    .virtual_workspace_manager()
                    .workspace_for_window(&state.windows, space, wid)
                    .is_some()
                    || engine.is_window_floating(wid)
            };
            if needs_removal {
                outcome = outcome.with_layout_event(LayoutEvent::WindowRemoved(wid));
            }
        }
        Err(e) => warn!("Failed to assign window {:?} to workspace: {:?}", wid, e),
    }
    outcome
}

pub(crate) struct EmitLayoutPayload<'a> {
    pub(crate) pid: pid_t,
    pub(crate) known_visible: &'a [WindowId],
    pub(crate) app_info: &'a Option<AppInfo>,
    pub(crate) discovery_spaces: HashMap<WindowId, SpaceId>,
    pub(crate) authoritative_spaces: HashMap<WindowId, SpaceId>,
    pub(crate) active_spaces: Vec<SpaceId>,
    pub(crate) focused_window: Option<(SpaceId, WindowId)>,
}

pub(crate) fn emit_layout_events(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    payload: EmitLayoutPayload<'_>,
) -> crate::actor::reactor::events::EventOutcome {
    let EmitLayoutPayload {
        pid,
        known_visible,
        app_info,
        discovery_spaces,
        authoritative_spaces,
        active_spaces,
        focused_window,
    } = payload;
    let mut outcome = crate::actor::reactor::events::EventOutcome::default();
    if !state.windows.iter_windows().any(|(wid, _)| wid.pid == pid) {
        return outcome;
    }

    let mut app_windows: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
    let mut included: HashSet<WindowId> = HashSet::default();
    let has_visible_window_server_windows = state
        .windows
        .iter_visible_window_server_ids()
        .filter_map(|wsid| state.windows.tracked_window_id(wsid))
        .any(|wid| {
            wid.pid == pid
                && state.windows.window(wid).is_some_and(|window| {
                    window.matches_filter(WindowFilter::EffectivelyManageable)
                })
        });

    // Collect windows from visible window server IDs
    for wid in state
        .windows
        .iter_visible_window_server_ids()
        .filter_map(|wsid| state.windows.tracked_window_id(wsid))
        .filter(|wid| wid.pid == pid)
        .filter(|wid| {
            state
                .windows
                .window(*wid)
                .is_some_and(|window| window.matches_filter(WindowFilter::EffectivelyManageable))
        })
    {
        let Some(space) = discovery_spaces.get(&wid).copied() else {
            continue;
        };
        if !should_emit_window_for_space(state, layout, space, wid) {
            continue;
        }
        included.insert(wid);
        app_windows.entry(space).or_default().push(wid);
    }

    // If we have no visible WSIDs (e.g., SpaceChanged provided empty ws_info),
    // fall back to the app-reported known_visible list for this pid.
    for wid in known_visible.iter().copied().filter(|wid| wid.pid == pid) {
        if included.contains(&wid)
            || !state
                .windows
                .window(wid)
                .is_some_and(|window| window.matches_filter(WindowFilter::EffectivelyManageable))
        {
            continue;
        }
        if has_visible_window_server_windows
            && authoritative_spaces
                .get(&wid)
                .is_none_or(|space| !active_spaces.contains(space))
        {
            // Once the active-space snapshot already contains some windows for
            // this app, do not let AX-only fallback resurrect other windows on
            // the current desktop via geometry inference alone.
            continue;
        }
        let Some(_state) = state.windows.window(wid) else {
            continue;
        };
        let Some(space) = discovery_spaces.get(&wid).copied() else {
            continue;
        };
        if !should_emit_window_for_space(state, layout, space, wid) {
            continue;
        }
        included.insert(wid);
        app_windows.entry(space).or_default().push(wid);
    }

    // Pre-pass: update the VWM for all windows definitively assigned to a space before
    // processing any per-space layout events. Without this, the ordering of space events
    // determines whether a window removed from one space's tree gets re-added by the
    // loop in sync_tiled_windows_for_app (which reads the VWM state at event time).
    // By updating the VWM upfront, the guard logic in sync_tiled_windows_for_app can
    // correctly identify cross-space moves regardless of event ordering.
    let mut assignment_results = BTreeMap::new();
    for (&space, windows_for_space) in &app_windows {
        for &wid in windows_for_space {
            assignment_results.insert(
                (space, wid),
                assign_discovered_window_to_space(state, layout, wid, space, app_info),
            );
        }
    }

    for space in active_spaces {
        let windows_for_space = app_windows.remove(&space).unwrap_or_default();

        if !windows_for_space.is_empty() {
            for &wid in &windows_for_space {
                let assign_result = assignment_results.remove(&(space, wid)).unwrap_or_else(|| {
                    assign_discovered_window_to_space(state, layout, wid, space, app_info)
                });
                let apply_outcome =
                    apply_assignment_result(state, layout, wid, space, assign_result);
                outcome.absorb(apply_outcome);
            }
        }

        let windows_with_titles: Vec<(
            WindowId,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
            objc2_core_foundation::CGSize,
            Option<objc2_core_foundation::CGSize>,
            Option<objc2_core_foundation::CGSize>,
        )> = windows_for_space
            .iter()
            .filter_map(|&wid| {
                let window = state.windows.window(wid)?;
                if !window.matches_filter(WindowFilter::EffectivelyManageable) {
                    return None;
                }
                Some((
                    wid,
                    Some(window.info.title.clone()),
                    window.info.ax_role.clone(),
                    window.info.ax_subrole.clone(),
                    window.info.is_resizable,
                    window.frame_monotonic.size,
                    window.info.min_size,
                    window.info.max_size,
                ))
            })
            .collect();

        outcome = outcome.with_layout_event(LayoutEvent::WindowsOnScreenUpdated(
            space,
            pid,
            windows_with_titles.clone(),
            app_info.clone(),
        ));
    }

    if let Some((space, main_window)) = focused_window {
        outcome = outcome.with_layout_event(LayoutEvent::WindowFocused(space, main_window));
    }
    outcome
}
