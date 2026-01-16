use tracing::{trace, warn};

use crate::actor::app::{AppInfo, WindowId, WindowInfo, pid_t};
use crate::actor::reactor::{Event, LayoutEvent, Reactor, WindowState, utils};
use crate::common::collections::{BTreeMap, HashSet};
use crate::model::virtual_workspace::AppRuleResult;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{self, WindowServerId};

/// Handler for window discovery events, responsible for processing newly discovered windows
/// and managing the lifecycle of window state in the reactor.
pub struct WindowDiscoveryHandler;

impl WindowDiscoveryHandler {
    /// Handle a windows discovered event with app info.
    pub fn handle_discovery(
        reactor: &mut Reactor,
        pid: pid_t,
        new: Vec<(WindowId, WindowInfo)>,
        known_visible: Vec<WindowId>,
        // pending_refresh: bool,
        app_info: Option<AppInfo>,
    ) {
        // If app_info wasn't provided, try to look it up from our running app state so
        // we can apply workspace rules immediately on first discovery.
        let app_info =
            app_info.or_else(|| reactor.app_manager.apps.get(&pid).map(|app| app.info.clone()));

        let (stale_windows, pending_refresh) =
            Self::identify_stale_windows(reactor, pid, &known_visible);
        Self::cleanup_stale_windows(reactor, pid, stale_windows, pending_refresh);
        let (new_windows, updated_windows) =
            Self::process_window_list(reactor, pid, new, &known_visible, &app_info);
        Self::update_window_states(reactor, pid, new_windows, updated_windows, &app_info);

        Self::emit_layout_events(reactor, pid, &known_visible, &app_info);
    }

    /// Identify windows that should be removed as stale.
    fn identify_stale_windows(
        reactor: &Reactor,
        pid: pid_t,
        known_visible: &[WindowId],
    ) -> (Vec<WindowId>, bool) {
        const MIN_REAL_WINDOW_DIMENSION: f64 = 2.0;

        let known_visible_set: HashSet<WindowId> = known_visible.iter().cloned().collect();
        // TODO: maybe remove pid here, and use pending_refresh bool
        let pending_refresh =
            reactor.mission_control_manager.pending_mission_control_refresh.contains(&pid);

        let has_window_server_visibles_without_ax = {
            let known_visible_set = &known_visible_set;
            reactor
                .window_manager
                .visible_windows
                .iter()
                .filter_map(|wsid| reactor.window_manager.window_ids.get(wsid))
                .any(|wid| wid.pid == pid && !known_visible_set.contains(wid))
        };
        // TODO: Rewrite it
        let skip_stale_cleanup = matches!(
            reactor.refocus_manager.stale_cleanup_state,
            crate::actor::reactor::StaleCleanupState::Suppressed
        ) || pending_refresh
            || reactor.is_mission_control_active()
            || reactor.is_in_drag()
            || reactor.pid_has_changing_screens(pid)
            || reactor.get_active_drag_session().map_or(false, |s| s.window.pid == pid)
            || (known_visible_set.is_empty()
                && !reactor.has_visible_window_server_ids_for_pid(pid))
            || has_window_server_visibles_without_ax;

        let active_space_windows: Option<HashSet<WindowServerId>> = if skip_stale_cleanup {
            None
        } else {
            let active_space_ids = reactor.active_space_ids();

            if active_space_ids.is_empty() {
                None
            } else {
                let window_ids = crate::sys::window_server::space_window_list_for_connection(
                    &active_space_ids,
                    0,
                    true,
                );
                let mut set = HashSet::default();
                set.extend(window_ids.into_iter().map(WindowServerId::new));
                Some(set)
            }
        };

        match skip_stale_cleanup {
            true => return (Vec::new(), false),
            false => {
                return (
                    reactor
                        .window_manager
                        .windows
                        .iter()
                        .filter_map(|(&wid, state)| {
                            if wid.pid != pid || known_visible_set.contains(&wid) {
                                return None;
                            }

                            if state.is_minimized {
                                return None;
                            }

                            let Some(ws_id) = state.window_server_id else {
                                trace!(
                                    ?wid,
                                    "Skipping stale cleanup for window without window server id"
                                );
                                return None;
                            };

                            if let Some(active_windows) = active_space_windows.as_ref() {
                                if !active_windows.contains(&ws_id) {
                                    trace!(
                                        ?wid,
                                        ws_id = ?ws_id,
                                        "Skipping stale cleanup; window is not on an active space"
                                    );
                                    return None;
                                }
                            }

                            let server_info = reactor
                                .window_server_info_manager
                                .window_server_info
                                .get(&ws_id)
                                .cloned()
                                .or_else(|| window_server::get_window(ws_id));

                            let info = match server_info {
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

                            let unsuitable = !window_server::app_window_suitable(ws_id);
                            let invalid_layer = info.layer != 0;
                            let too_small = width < MIN_REAL_WINDOW_DIMENSION
                                || height < MIN_REAL_WINDOW_DIMENSION;
                            let ordered_in = window_server::window_is_ordered_in(ws_id);
                            let visible_in_snapshot =
                                reactor.window_manager.visible_windows.contains(&ws_id);

                            let is_on_active_space = active_space_windows
                                .as_ref()
                                .map_or(false, |set| set.contains(&ws_id));

                            if unsuitable
                                || invalid_layer
                                || too_small
                                || (is_on_active_space && !ordered_in && !visible_in_snapshot)
                            {
                                Some(wid)
                            } else {
                                None
                            }
                        })
                        .collect(),
                    pending_refresh,
                );
            }
        }
    }

    /// Remove stale windows and send events.
    fn cleanup_stale_windows(
        reactor: &mut Reactor,
        pid: pid_t,
        stale_windows: Vec<WindowId>,
        pending_refresh: bool,
    ) {
        for wid in stale_windows {
            reactor.handle_event(Event::WindowDestroyed(wid));
        }
        if pending_refresh {
            reactor.mission_control_manager.pending_mission_control_refresh.remove(&pid);
        }
    }

    /// Process new and updated windows, returning lists of new and updated windows.
    fn process_window_list(
        reactor: &mut Reactor,
        _pid: pid_t,
        new: Vec<(WindowId, WindowInfo)>,
        _known_visible: &[WindowId],
        app_info: &Option<AppInfo>,
    ) -> (Vec<(WindowId, WindowInfo)>, Vec<(WindowId, WindowInfo)>) {
        const APP_RULE_TTL_MS: u64 = 1000;

        let mut new_windows = Vec::new();
        let mut updated_windows = Vec::new();

        reactor.app_manager.purge_expired(APP_RULE_TTL_MS);

        let any_recent = new.iter().any(|(_, info)| {
            info.sys_id.map_or(false, |wsid| {
                reactor.app_manager.is_wsid_recent(wsid, APP_RULE_TTL_MS)
            })
        });

        if any_recent && app_info.is_none() && !new.is_empty() {
            // Update state for any newly reported windows, but do not early-return;
            // proceed to emit WindowsOnScreenUpdated so existing mappings are respected
            // without reapplying app rules.
            for (wid, info) in &new {
                if reactor.window_manager.windows.contains_key(wid) {
                    let manageable = utils::compute_window_manageability(
                        info.sys_id,
                        info.is_minimized,
                        info.is_standard,
                        info.is_root,
                        &reactor.window_server_info_manager.window_server_info,
                    );
                    if let Some(existing) = reactor.window_manager.windows.get_mut(wid) {
                        existing.title = info.title.clone();
                        if info.frame.size.width != 0.0 || info.frame.size.height != 0.0 {
                            existing.frame_monotonic = info.frame;
                        }
                        existing.is_ax_standard = info.is_standard;
                        existing.is_ax_root = info.is_root;
                        existing.is_minimized = info.is_minimized;
                        existing.window_server_id = info.sys_id;
                        existing.bundle_id = info.bundle_id.clone();
                        existing.bundle_path = info.path.clone();
                        existing.ax_role = info.ax_role.clone();
                        existing.ax_subrole = info.ax_subrole.clone();
                        existing.is_manageable = manageable;
                    }
                } else {
                    let mut state: WindowState = WindowState {
                        title: info.title.clone(),
                        frame_monotonic: info.frame,
                        anim_state: None,
                        is_ax_standard: info.is_standard,
                        is_ax_root: info.is_root,
                        is_minimized: info.is_minimized,
                        is_manageable: false,
                        ignore_app_rule: false,
                        window_server_id: info.sys_id,
                        bundle_id: info.bundle_id.clone(),
                        bundle_path: info.path.clone(),
                        ax_role: info.ax_role.clone(),
                        ax_subrole: info.ax_subrole.clone(),
                    };
                    let manageable = utils::compute_window_manageability(
                        state.window_server_id,
                        state.is_minimized,
                        state.is_ax_standard,
                        state.is_ax_root,
                        &reactor.window_server_info_manager.window_server_info,
                    );
                    state.is_manageable = manageable;
                    reactor.window_manager.windows.insert(*wid, state);
                }
                if let Some(wsid) = info.sys_id {
                    reactor.window_manager.window_ids.insert(wsid, *wid);
                }
            }
            // fall through
        }

        // Process all new windows
        for (wid, info) in new {
            if reactor.window_manager.windows.contains_key(&wid) {
                updated_windows.push((wid, info));
            } else {
                new_windows.push((wid, info));
            }
        }

        (new_windows, updated_windows)
    }

    /// Update window states in Reactor.
    fn update_window_states(
        reactor: &mut Reactor,
        _pid: pid_t,
        new_windows: Vec<(WindowId, WindowInfo)>,
        updated_windows: Vec<(WindowId, WindowInfo)>,
        _app_info: &Option<AppInfo>,
    ) {
        // Update window IDs for new windows
        for (wid, info) in &new_windows {
            if let Some(wsid) = info.sys_id {
                reactor.window_manager.window_ids.insert(wsid, *wid);
            }
        }
        for (wid, info) in &updated_windows {
            if let Some(wsid) = info.sys_id {
                reactor.window_manager.window_ids.insert(wsid, *wid);
            }
        }

        // Update or insert window states
        for (wid, info) in new_windows {
            let mut state: WindowState = info.into();
            let manageable = utils::compute_window_manageability(
                state.window_server_id,
                state.is_minimized,
                state.is_ax_standard,
                state.is_ax_root,
                &reactor.window_server_info_manager.window_server_info,
            );
            state.is_manageable = manageable;
            reactor.window_manager.windows.insert(wid, state);
        }
    }

    /// Send layout events for discovered windows.
    fn emit_layout_events(
        reactor: &mut Reactor,
        pid: pid_t,
        known_visible: &[WindowId],
        app_info: &Option<AppInfo>,
    ) {
        if !reactor.window_manager.windows.iter().any(|(wid, _)| wid.pid == pid) {
            return;
        }

        let mut app_windows: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
        let mut included: HashSet<WindowId> = HashSet::default();

        // Collect windows from visible window server IDs
        for wid in reactor
            .window_manager
            .visible_windows
            .iter()
            .flat_map(|wsid| reactor.window_manager.window_ids.get(wsid))
            .copied()
            .filter(|wid| wid.pid == pid)
            .filter(|wid| reactor.window_is_standard(*wid))
        {
            let Some(space) = reactor.best_space_for_window_id(wid) else {
                continue;
            };
            included.insert(wid);
            app_windows.entry(space).or_default().push(wid);
        }

        // If we have no visible WSIDs (e.g., SpaceChanged provided empty ws_info),
        // fall back to the app-reported known_visible list for this pid.
        for wid in known_visible.iter().copied().filter(|wid| wid.pid == pid) {
            if included.contains(&wid) || !reactor.window_is_standard(wid) {
                continue;
            }
            let Some(state) = reactor.window_manager.windows.get(&wid) else {
                continue;
            };
            let Some(space) =
                reactor.best_space_for_window(&state.frame_monotonic, state.window_server_id)
            else {
                continue;
            };
            included.insert(wid);
            app_windows.entry(space).or_default().push(wid);
        }

        // For now, we'll assume known_visible is handled elsewhere or we need to pass it.
        // Looking back, the original method processes known_visible in the main logic.
        // Actually, the emit_layout_events should be called after processing, and we need to collect all windows.

        let screens = reactor.space_manager.screens.clone();
        for screen in screens {
            let Some(space) = screen.space else {
                continue;
            };
            if !reactor.is_space_active(space) {
                continue;
            }
            let windows_for_space = app_windows.remove(&space).unwrap_or_default();

            if !windows_for_space.is_empty() {
                for wid in &windows_for_space {
                    let title_opt =
                        reactor.window_manager.windows.get(wid).map(|w| w.title.clone());
                    let assign_result = reactor
                        .layout_manager
                        .layout_engine
                        .virtual_workspace_manager_mut()
                        .assign_window_with_app_info(
                            *wid,
                            space,
                            app_info.as_ref().and_then(|a| a.bundle_id.as_deref()),
                            app_info.as_ref().and_then(|a| a.localized_name.as_deref()),
                            title_opt.as_deref(),
                            reactor
                                .window_manager
                                .windows
                                .get(wid)
                                .and_then(|w| w.ax_role.as_deref()),
                            reactor
                                .window_manager
                                .windows
                                .get(wid)
                                .and_then(|w| w.ax_subrole.as_deref()),
                        );

                    match assign_result {
                        Ok(AppRuleResult::Managed(_)) => {
                            if let Some(window) = reactor.window_manager.windows.get_mut(wid) {
                                window.ignore_app_rule = false;
                            }
                        }
                        Ok(AppRuleResult::Unmanaged) => {
                            if let Some(window) = reactor.window_manager.windows.get_mut(wid) {
                                window.ignore_app_rule = true;
                            }
                            let needs_removal = {
                                let engine = &reactor.layout_manager.layout_engine;
                                engine
                                    .virtual_workspace_manager()
                                    .workspace_for_window(space, *wid)
                                    .is_some()
                                    || engine.is_window_floating(*wid)
                            };
                            if needs_removal {
                                reactor.send_layout_event(LayoutEvent::WindowRemoved(*wid));
                            }
                        }
                        Err(e) => warn!("Failed to assign window {:?} to workspace: {:?}", wid, e),
                    }
                }
            }

            let windows_with_titles: Vec<(
                WindowId,
                Option<String>,
                Option<String>,
                Option<String>,
            )> = windows_for_space
                .iter()
                .filter_map(|&wid| {
                    let window = reactor.window_manager.windows.get(&wid)?;
                    if !window.is_effectively_manageable() {
                        return None;
                    }
                    Some((
                        wid,
                        Some(window.title.clone()),
                        window.ax_role.clone(),
                        window.ax_subrole.clone(),
                    ))
                })
                .collect();

            reactor.send_layout_event(LayoutEvent::WindowsOnScreenUpdated(
                space,
                pid,
                windows_with_titles.clone(),
                app_info.clone(),
            ));
        }

        if let Some(main_window) = reactor.main_window() {
            if main_window.pid == pid {
                if let Some(space) = reactor.main_window_space() {
                    if reactor.is_space_active(space) {
                        reactor.send_layout_event(LayoutEvent::WindowFocused(space, main_window));
                    }
                }
            }
        }
    }
}
