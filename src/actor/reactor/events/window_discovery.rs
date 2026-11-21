use tracing::debug;

use super::super::{Changes, Presentation, Reactor};
use crate::actor::app::{AppInfo, WindowId, pid_t};
use crate::actor::reactor::LayoutEvent;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::screen::SpaceId;

pub(crate) struct EmitLayoutPayload<'a> {
    pub(crate) pid: pid_t,
    pub(crate) known_visible: &'a [WindowId],
    pub(crate) resumed: &'a HashSet<WindowId>,
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
            resumed,
            app_info,
            window_spaces,
            active_spaces,
            focused_window,
        } = payload;
        if self.state.windows.window_ids_for_pid(pid).next().is_none() {
            return;
        }

        let native_visible = |wid| {
            self.state
                .windows
                .record(wid)
                .and_then(|record| record.window_server_id())
                .is_some_and(|id| self.state.windows.is_window_visible(id))
        };
        let has_visible = self
            .state
            .windows
            .window_ids_for_pid(pid)
            .any(|wid| native_visible(wid) && self.state.windows.can_reconcile_admission(wid));
        let known_visible: HashSet<_> = known_visible.iter().copied().collect();
        let mut observations: Vec<_> = self
            .state
            .windows
            .window_ids_for_pid(pid)
            .filter_map(|wid| {
                let (native, discovery) = *window_spaces.get(&wid)?;
                let window = self.state.windows.window(wid)?;
                // Minimized windows can still claim persisted slots before pruning.
                if window.info.is_minimized && !self.state.windows.is_claimed(wid) {
                    return Some((native.or(discovery)?, wid));
                }
                if !self.state.windows.can_reconcile_admission(wid) {
                    return None;
                }
                let space = discovery?;
                if !resumed.contains(&wid)
                    && (self.window_in_non_active_workspace(space, wid)
                        || !(native_visible(wid)
                            || (known_visible.contains(&wid)
                                && (!has_visible
                                    || native.is_some_and(|s| active_spaces.contains(&s))))))
                {
                    return None;
                }
                Some((space, wid))
            })
            .collect();
        observations.sort_unstable();
        for (space, wid) in observations {
            let active = active_spaces.contains(&space);
            if !active
                && resumed.contains(&wid)
                && self.assigned_space_for_window_id(wid) == Some(space)
            {
                self.reconcile_window_membership(changes, wid, Some(space));
                continue;
            }
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
            if active {
                self.project_observed_window(changes, space, wid, effects);
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
