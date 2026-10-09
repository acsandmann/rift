use tracing::{debug, warn};

use super::window;
use crate::actor::app::{AppInfo, WindowId, WindowInfo, pid_t};
use crate::actor::reactor::transaction_manager::TransactionManager;
use crate::actor::reactor::{LayoutEvent, WindowState, utils};
use crate::common::collections::{BTreeMap, HashMap, HashSet};
use crate::layout_engine::ResolvedWindow;
use crate::model::virtual_workspace::WorkspaceError;
use crate::model::{AppRuleEffects, AppRuleResult, WindowRuleContext};
use crate::sys::geometry::SameAs;
use crate::sys::screen::SpaceId;

/// Handler for window discovery events, responsible for processing newly discovered windows
/// and managing the lifecycle of window state in the reactor.
fn sync_existing_window_state(
    state: &mut crate::model::RiftState,
    transactions: &TransactionManager,
    wid: WindowId,
    mut info: WindowInfo,
    active_space: Option<SpaceId>,
) -> anyhow::Result<crate::actor::reactor::events::EventOutcome> {
    let was_minimized = state.windows.window(wid).is_some_and(|window| window.info.is_minimized);
    let was_manageable = state.windows.window(wid).is_some_and(WindowState::is_admitted);

    let is_minimized = info.is_minimized;
    let rift_owns_geometry =
        info.sys_id.is_some_and(|wsid| transactions.get_target_frame(wsid).is_some());
    let needs_arrange;
    if let Some(existing) = state.windows.window_mut(wid) {
        let valid_frame = info.frame.size.width != 0.0 || info.frame.size.height != 0.0;
        let geometry_changed =
            !rift_owns_geometry && valid_frame && !existing.frame_monotonic.same_as(info.frame);
        let constraints_changed = existing.info.is_resizable != info.is_resizable
            || existing.info.min_size != info.min_size
            || existing.info.max_size != info.max_size;
        needs_arrange = geometry_changed || constraints_changed;
        if !rift_owns_geometry && valid_frame {
            existing.frame_monotonic = info.frame;
        }
        // Preserve the observed frame and minimize transition until their handlers run.
        info.frame = existing.info.frame;
        info.is_minimized = was_minimized;
        existing.info = info;
    } else {
        return Ok(crate::actor::reactor::events::EventOutcome::default());
    }

    let mut admission_changed = false;
    let mut outcome = match (was_minimized, is_minimized) {
        (_, true) => window::handle_window_minimized(state, wid)?,
        (true, false) => {
            window::handle_window_deminiaturized(state, window::WindowDeminiaturizedPayload {
                window: wid,
                active_space,
            })?
        }
        _ => {
            let is_admitted = utils::refresh_heuristic(state, wid)
                .is_some_and(|transition| transition.is_admitted);
            admission_changed = was_manageable != is_admitted;
            if was_manageable && !is_admitted {
                crate::actor::reactor::events::EventOutcome::default()
                    .with_layout_event(LayoutEvent::WindowRemoved(wid))
            } else {
                crate::actor::reactor::events::EventOutcome::default()
            }
        }
    };
    if needs_arrange && was_minimized == is_minimized && !admission_changed {
        outcome.absorb(crate::actor::reactor::events::EventOutcome::layout_changed(
            false,
        ));
    }

    if was_minimized != is_minimized {
        debug!(
            ?wid,
            was_minimized,
            is_minimized = is_minimized,
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
    let assigned_workspace = state.windows.workspace_for_window(space, wid);
    let active_workspace = engine.workspaces().active_workspace(space);

    match (assigned_workspace, active_workspace) {
        (Some(assigned), Some(active)) => assigned == active,
        _ => true,
    }
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
    transactions: &TransactionManager,
    observed: Vec<ObservedWindow>,
) -> (
    Vec<(WindowId, WindowInfo)>,
    crate::actor::reactor::events::EventOutcome,
) {
    let mut new_windows = Vec::new();
    let mut outcome = crate::actor::reactor::events::EventOutcome::default();

    for window in observed {
        let ObservedWindow {
            wid,
            info,
            current_native_space,
            active_space,
        } = window;
        if let Some(previous) =
            state.windows.reconcile_ax_identity(wid, info.sys_id, current_native_space)
        {
            layout.layout_engine.transfer_persistent_window_identity(previous, wid);
            outcome =
                outcome.with_layout_event(LayoutEvent::WindowRemovedPreserveFloating(previous));
        }
        if state.windows.contains_window(wid) {
            if let Ok(existing_outcome) =
                sync_existing_window_state(state, transactions, wid, info, active_space)
            {
                outcome.absorb(existing_outcome);
            }
        } else {
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
        let state: WindowState = info.into();
        rift_state.windows.insert_window(wid, state);
        let _ = utils::refresh_heuristic(rift_state, wid);
    }
}

/// Assignment and rejection handling shared by discovery and rule reapplication.
/// Callers decide whether successful, unchanged membership needs a layout refresh.
pub(crate) fn assign_window(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    wid: WindowId,
    space: SpaceId,
    app_info: Option<&AppInfo>,
    reapply: bool,
) -> (Option<AppRuleEffects>, Option<LayoutEvent>) {
    let result = if let Some(window) = state.windows.window(wid) {
        let (title, role, subrole) = (
            window.info.title.clone(),
            window.info.ax_role.clone(),
            window.info.ax_subrole.clone(),
        );
        layout.layout_engine.assign_window_with_app_info(
            &mut state.windows,
            wid,
            space,
            WindowRuleContext {
                app_bundle_id: app_info.and_then(|app| app.bundle_id.as_deref()),
                app_name: app_info.and_then(|app| app.localized_name.as_deref()),
                window_title: Some(&title),
                ax_role: role.as_deref(),
                ax_subrole: subrole.as_deref(),
            },
            reapply,
        )
    } else {
        Err(WorkspaceError::AssignmentFailed)
    };
    match result {
        Ok(AppRuleResult::Managed(effects)) => (Some(effects), None),
        Ok(AppRuleResult::Rejected(_)) => (
            None,
            utils::rejection_needs_removal(state, layout, wid, space)
                .then_some(LayoutEvent::WindowRemoved(wid)),
        ),
        Err(error) => {
            warn!(?wid, ?error, "Failed to assign window to workspace");
            utils::clear_rule_admission(state, wid);
            (None, None)
        }
    }
}

pub(crate) struct EmitLayoutPayload<'a> {
    pub(crate) pid: pid_t,
    pub(crate) known_visible: &'a [WindowId],
    pub(crate) app_info: &'a Option<AppInfo>,
    // Authoritative ownership and location allowing geometry fallback, respectively.
    pub(crate) window_spaces: HashMap<WindowId, (Option<SpaceId>, Option<SpaceId>)>,
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
        window_spaces,
        active_spaces,
        focused_window,
    } = payload;
    let mut outcome = crate::actor::reactor::events::EventOutcome::default();
    if state.windows.window_ids_for_pid(pid).next().is_none() {
        return outcome;
    }

    let mut app_windows: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
    let mut included: HashSet<WindowId> = HashSet::default();
    let visible: Vec<_> = state
        .windows
        .iter_visible_window_server_ids()
        .filter_map(|wsid| state.windows.tracked_window_id(wsid))
        .filter(|wid| wid.pid == pid)
        .filter(|wid| state.windows.window(*wid).is_some_and(WindowState::can_reconcile_admission))
        .collect();
    let has_visible_window_server_windows = !visible.is_empty();

    for (wid, native_visible) in visible.into_iter().map(|wid| (wid, true)).chain(
        known_visible
            .iter()
            .copied()
            .filter(|wid| wid.pid == pid)
            .map(|wid| (wid, false)),
    ) {
        if included.contains(&wid)
            || !state.windows.window(wid).is_some_and(WindowState::can_reconcile_admission)
        {
            continue;
        }
        // AX fallback must not resurrect an omitted window via geometry when native
        // membership already identifies other visible windows of this application.
        if !native_visible
            && has_visible_window_server_windows
            && window_spaces
                .get(&wid)
                .and_then(|(native, _)| *native)
                .is_none_or(|space| !active_spaces.contains(&space))
        {
            continue;
        }
        let Some(space) = window_spaces.get(&wid).and_then(|(_, discovery)| *discovery) else {
            continue;
        };
        if should_emit_window_for_space(state, layout, space, wid) {
            included.insert(wid);
            app_windows.entry(space).or_default().push(wid);
        }
    }

    for (space, mut windows_for_space) in app_windows {
        windows_for_space.sort_unstable();
        for wid in windows_for_space {
            let (effects, removal) =
                assign_window(state, layout, wid, space, app_info.as_ref(), false);
            if let Some(event) = removal {
                outcome = outcome.with_layout_event(event);
            }
            let Some(effects) = effects else {
                continue;
            };
            let Some(window) = state.windows.window(wid).filter(|window| window.is_admitted())
            else {
                continue;
            };
            if active_spaces.contains(&space) {
                outcome =
                    outcome.with_layout_event(LayoutEvent::WindowObserved(space, ResolvedWindow {
                        info: window.layout_info(wid),
                        effects,
                    }));
            }
        }
    }

    // Matching is allowed to observe every native-space slice for this application before stale
    // saved identities are discarded. Cleaning after an individual slice would incorrectly drop
    // windows that are discovered on a later space in the same batch.
    outcome = outcome.with_layout_event(LayoutEvent::WindowDiscoveryCompleted(
        pid,
        app_info.as_ref().and_then(|info| info.bundle_id.clone()),
        active_spaces,
    ));

    if let Some((space, main_window)) = focused_window {
        outcome = outcome.with_layout_event(LayoutEvent::WindowFocused(space, main_window));
    }
    outcome
}
