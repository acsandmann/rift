use tracing::{debug, warn};

use super::super::{Changes, Presentation, Reactor};
use crate::actor::app::{AppInfo, WindowId, WindowInfo, pid_t};
use crate::actor::reactor::{LayoutEvent, WindowState, utils};
use crate::common::collections::{BTreeMap, HashMap, HashSet};
use crate::layout_engine::ResolvedWindow;
use crate::model::virtual_workspace::WorkspaceError;
use crate::model::{AppRuleEffects, AppRuleResult, WindowRuleContext};
use crate::sys::geometry::SameAs;
use crate::sys::screen::SpaceId;

/// Handler for window discovery events, responsible for processing newly discovered windows
/// and managing the lifecycle of window state in the reactor.
fn should_emit_window_for_space(
    state: &crate::model::RiftState,
    layout: &crate::actor::reactor::managers::LayoutManager,
    space: SpaceId,
    wid: WindowId,
) -> bool {
    let engine = &layout.layout_engine;
    let assigned_workspace =
        layout
            .layout_engine
            .workspaces()
            .workspace_for_window(&state.windows, space, wid);
    let active_workspace = engine.workspaces().active_workspace(space);

    match (assigned_workspace, active_workspace) {
        (Some(assigned), Some(active)) => assigned == active,
        _ => true,
    }
}

/// Assignment and rejection handling shared by discovery and rule reapplication.
/// Callers decide whether successful, unchanged membership needs a layout refresh.
pub(crate) fn assign_window<'a>(
    state: &mut crate::model::RiftState,
    layout: &mut crate::actor::reactor::managers::LayoutManager,
    wid: WindowId,
    context: impl Into<crate::model::virtual_workspace::WorkspaceContext<'a>>,
    app_info: Option<&AppInfo>,
    reapply: bool,
) -> (Option<AppRuleEffects>, Option<LayoutEvent>) {
    let context = context.into();
    let space = context.space;
    let result = if state.windows.contains_window(wid) {
        layout.layout_engine.assign_window_with_app_info(
            &mut state.windows,
            wid,
            context,
            WindowRuleContext {
                app_bundle_id: app_info.and_then(|app| app.bundle_id.as_deref()),
                app_name: app_info.and_then(|app| app.localized_name.as_deref()),
                ..Default::default()
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

impl Reactor {
    pub(in crate::actor::reactor) fn reconcile_window_list(
        &mut self,
        changes: &mut Changes,
        observed: Vec<(WindowId, WindowInfo)>,
    ) -> Vec<WindowId> {
        let mut new_windows = Vec::new();
        for (wid, info) in observed {
            if let Some(previous) = self.state.windows.reconcile_ax_identity(wid, info.sys_id) {
                self.layout_manager
                    .layout_engine
                    .transfer_persistent_window_identity(previous, wid);
                self.reconcile_layout_event(
                    changes,
                    LayoutEvent::WindowRemovedPreserveFloating(previous),
                );
            }
            if self.state.windows.contains_window(wid) {
                self.sync_existing_window_state(changes, wid, info);
            } else {
                self.state.windows.insert_window(wid, info.into());
                let _ = utils::refresh_heuristic(&mut self.state, wid);
                new_windows.push(wid);
            }
        }
        new_windows
    }

    fn sync_existing_window_state(
        &mut self,
        changes: &mut Changes,
        wid: WindowId,
        mut info: WindowInfo,
    ) {
        let ax_returned = self.state.windows.validate_ax_identity(wid);
        let was_minimized =
            self.state.windows.window(wid).is_some_and(|window| window.info.is_minimized);
        let was_manageable = self.state.windows.window(wid).is_some_and(WindowState::is_admitted);

        let is_minimized = info.is_minimized;
        let rift_owns_geometry = info
            .sys_id
            .is_some_and(|wsid| self.transaction_manager.get_target_frame(wsid).is_some());
        let needs_arrange;
        let claimed = self.state.windows.is_claimed(wid);
        if let Some(existing) = self.state.windows.window_mut(wid) {
            let valid_frame = info.frame.size.width != 0.0 || info.frame.size.height != 0.0;
            let geometry_changed =
                !rift_owns_geometry && valid_frame && !existing.frame_monotonic.same_as(info.frame);
            let constraints_changed = existing.info.is_resizable != info.is_resizable
                || existing.info.min_size != info.min_size
                || existing.info.max_size != info.max_size;
            // A claimed window's owner has geometry authority.
            needs_arrange = (geometry_changed || constraints_changed) && !claimed;
            if !rift_owns_geometry && valid_frame {
                existing.frame_monotonic = info.frame;
            }
            // Preserve the observed frame and minimize transition until their handlers run.
            info.frame = existing.info.frame;
            info.is_minimized = was_minimized;
            existing.info = info;
        } else {
            return;
        }

        if was_minimized || is_minimized {
            self.reconcile_window_visibility(changes, wid, !is_minimized);
        } else {
            let admitted = utils::refresh_heuristic(&mut self.state, wid)
                .is_some_and(|transition| transition.is_admitted);
            if was_manageable && !admitted {
                self.reconcile_layout_event(
                    changes,
                    if self.state.windows.is_temporarily_unavailable(wid) {
                        LayoutEvent::WindowSuspended(wid)
                    } else {
                        LayoutEvent::WindowRemoved(wid)
                    },
                );
            } else if admitted && !claimed && (!was_manageable || ax_returned) {
                self.finalize_created_window(changes, wid);
            }
            if admitted
                && (needs_arrange || !was_manageable || ax_returned)
                && let Some(space) = self
                    .assigned_space_for_window_id(wid)
                    .or_else(|| self.best_space_for_window_id(wid))
            {
                changes.arrange(Some(space), Presentation::Animated);
            }
        }

        if was_minimized != is_minimized {
            debug!(
                ?wid,
                was_minimized,
                is_minimized = is_minimized,
                "Window minimize state reconciled from discovery"
            );
        }
    }

    pub(in crate::actor::reactor) fn reconcile_discovered_windows(
        &mut self,
        changes: &mut Changes,
        payload: EmitLayoutPayload<'_>,
    ) {
        let EmitLayoutPayload {
            pid,
            known_visible,
            app_info,
            window_spaces,
            active_spaces,
            focused_window,
        } = payload;
        if self.state.windows.window_ids_for_pid(pid).next().is_none() {
            return;
        }

        // Minimized windows still match saved membership. Match before the app's
        // discovery-completed event prunes truly missing identities; admission and
        // geometry remain suspended until a positive return.
        let minimized: Vec<_> = self
            .state
            .windows
            .window_ids_for_pid(pid)
            .filter(|wid| {
                !self.state.windows.is_claimed(*wid)
                    && self.state.windows.window(*wid).is_some_and(|w| w.info.is_minimized)
            })
            .collect();
        for wid in minimized {
            let Some(space) =
                window_spaces.get(&wid).and_then(|(native, discovery)| native.or(*discovery))
            else {
                continue;
            };
            let (_, removal) = assign_window(
                &mut self.state,
                &mut self.layout_manager,
                wid,
                crate::model::virtual_workspace::WorkspaceContext {
                    space,
                    visible: &active_spaces,
                },
                app_info.as_ref(),
                false,
            );
            if let Some(event) = removal {
                self.reconcile_layout_event(changes, event);
            } else if self.state.windows.workspace_for_window(wid).is_some() {
                self.reconcile_layout_event(changes, LayoutEvent::WindowSuspended(wid));
            }
        }

        let mut app_windows: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
        let mut included: HashSet<WindowId> = HashSet::default();
        let visible: Vec<_> = self
            .state
            .windows
            .iter_visible_window_server_ids()
            .filter_map(|wsid| self.state.windows.tracked_window_id(wsid))
            .filter(|wid| wid.pid == pid)
            .filter(|wid| self.state.windows.can_reconcile_admission(*wid))
            .collect();
        let has_visible_window_server_windows = !visible.is_empty();

        for (wid, native_visible) in visible.into_iter().map(|wid| (wid, true)).chain(
            known_visible
                .iter()
                .copied()
                .filter(|wid| wid.pid == pid)
                .map(|wid| (wid, false)),
        ) {
            if included.contains(&wid) || !self.state.windows.can_reconcile_admission(wid) {
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
            if should_emit_window_for_space(&self.state, &self.layout_manager, space, wid) {
                included.insert(wid);
                app_windows.entry(space).or_default().push(wid);
            }
        }

        for (space, mut windows_for_space) in app_windows {
            windows_for_space.sort_unstable();
            for wid in windows_for_space {
                let (effects, removal) = assign_window(
                    &mut self.state,
                    &mut self.layout_manager,
                    wid,
                    crate::model::virtual_workspace::WorkspaceContext {
                        space,
                        visible: &active_spaces,
                    },
                    app_info.as_ref(),
                    false,
                );
                if let Some(event) = removal {
                    self.reconcile_layout_event(changes, event);
                }
                let Some(effects) = effects else {
                    continue;
                };
                let Some(window) =
                    self.state.windows.window(wid).filter(|_| self.state.windows.is_admitted(wid))
                else {
                    continue;
                };
                if active_spaces.contains(&space) {
                    self.reconcile_layout_event(
                        changes,
                        LayoutEvent::WindowObserved(space, ResolvedWindow {
                            info: window.layout_info(wid),
                            effects,
                        }),
                    );
                }
            }
        }

        // Matching is allowed to observe every native-space slice for this application before stale
        // saved identities are discarded. Cleaning after an individual slice would incorrectly drop
        // windows that are discovered on a later space in the same batch.
        let ignored = self.layout_manager.layout_engine.discard_unmatched_candidates_for_app(
            pid,
            app_info.as_ref().and_then(|info| info.bundle_id.as_deref()),
            &active_spaces,
        );
        if ignored > 0 {
            debug!(
                pid,
                ignored, "Ignored unmatched persisted windows after application discovery"
            );
            for space in active_spaces {
                changes.membership_for(Some(space));
                changes.arrange(Some(space), Presentation::Animated);
            }
        }

        if let Some((space, main_window)) = focused_window {
            self.reconcile_layout_event(changes, LayoutEvent::WindowFocused(space, main_window));
        }
    }
}
