use tracing::debug;

use super::super::{Changes, Presentation, Reactor};
use crate::actor::app::{AppInfo, WindowId, pid_t};
use crate::actor::reactor::LayoutEvent;
use crate::common::collections::{BTreeMap, HashMap, HashSet};
use crate::layout_engine::ResolvedWindow;
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
            self.assign_observed_window(
                changes,
                wid,
                crate::model::virtual_workspace::WorkspaceContext {
                    space,
                    visible: &active_spaces,
                },
                app_info.as_ref(),
                false,
            );
        }

        let mut app_windows: BTreeMap<SpaceId, Vec<WindowId>> = BTreeMap::new();
        let mut included: HashSet<WindowId> = HashSet::default();
        let visible: Vec<_> = self
            .state
            .windows
            .window_ids_for_pid(pid)
            .filter(|wid| self.state.windows.can_reconcile_admission(*wid))
            .filter(|wid| {
                self.state
                    .windows
                    .record(*wid)
                    .and_then(|record| record.window_server_id())
                    .is_some_and(|wsid| self.state.windows.is_window_visible(wsid))
            })
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
                let Some(effects) = self.assign_observed_window(
                    changes,
                    wid,
                    crate::model::virtual_workspace::WorkspaceContext {
                        space,
                        visible: &active_spaces,
                    },
                    app_info.as_ref(),
                    false,
                ) else {
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
