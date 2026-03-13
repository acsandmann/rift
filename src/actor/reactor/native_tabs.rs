use objc2_core_foundation::CGRect;
use tracing::debug;

use crate::actor::app::{Quiet, Request, WindowId, pid_t};
use crate::actor::reactor::{NativeTabMembership, NativeTabRole, Reactor};
use crate::model::reactor::WindowFilter;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

const NATIVE_TAB_FRAME_TOLERANCE: f64 = 2.0;

pub(crate) fn frames_match(a: CGRect, b: CGRect) -> bool {
    (a.origin.x - b.origin.x).abs() <= NATIVE_TAB_FRAME_TOLERANCE
        && (a.origin.y - b.origin.y).abs() <= NATIVE_TAB_FRAME_TOLERANCE
        && (a.size.width - b.size.width).abs() <= NATIVE_TAB_FRAME_TOLERANCE
        && (a.size.height - b.size.height).abs() <= NATIVE_TAB_FRAME_TOLERANCE
}

impl Reactor {
    fn log_native_tab_state(&self, label: &str, wid: WindowId) { let _ = (label, wid); }

    fn native_tab_group_frame_for(&self, wid: WindowId) -> Option<CGRect> {
        let group_id = self.native_tab_manager.group_for_window(wid)?;
        self.native_tab_manager.groups.get(&group_id).map(|group| group.canonical_frame)
    }

    fn pid_has_native_tab_state(&self, pid: pid_t) -> bool {
        self.native_tab_manager.groups.values().any(|group| group.pid == pid)
            || !self.native_tab_manager.pending_destroys_for_pid(pid).is_empty()
            || !self.native_tab_manager.pending_appearances_for_pid(pid).is_empty()
    }

    fn set_native_tab_role(&mut self, wid: WindowId, group_id: u32, role: NativeTabRole) {
        if let Some(window) = self.window_manager.windows.get_mut(&wid) {
            window.native_tab = Some(NativeTabMembership { group_id, role });
        }
    }

    fn try_activate_native_tab_replacement(&mut self, old: WindowId, new: WindowId) -> bool {
        if self.activate_native_tab_replacement(old, new) {
            self.native_tab_manager.clear_pending_destroy(old);
            return true;
        }
        false
    }

    fn sync_native_tab_group_frame(
        &mut self,
        active_wid: WindowId,
        frame: CGRect,
        membership: Option<NativeTabMembership>,
        sync_window_positions: bool,
    ) {
        let Some(membership) = membership else {
            return;
        };
        if membership.role != NativeTabRole::Active {
            return;
        }
        let Some(group) = self.native_tab_manager.groups.get(&membership.group_id) else {
            return;
        };
        let members: Vec<WindowId> = group.members.iter().copied().collect();
        for member in members {
            if let Some(window) = self.window_manager.windows.get_mut(&member) {
                window.frame_monotonic = frame;
                window.info.frame = frame;
            }
            if !sync_window_positions || member == active_wid {
                continue;
            }
            self.request_native_tab_member_frame_sync(member, frame);
        }
    }

    fn request_native_tab_member_frame_sync(&mut self, wid: WindowId, frame: CGRect) {
        let Some(app) = self.app_manager.apps.get(&wid.pid) else {
            return;
        };
        self.native_tab_manager.set_pending_frame_target(wid, frame);
        let txid = if let Some(wsid) =
            self.window_manager.windows.get(&wid).and_then(|window| window.info.sys_id)
        {
            let txid = self.transaction_manager.generate_next_txid(wsid);
            self.transaction_manager.store_txid(wsid, txid, frame);
            txid
        } else {
            Default::default()
        };
        if let Err(err) = app.handle.send(Request::SetWindowFrame(wid, frame, txid, true)) {
            debug!(?wid, ?err, "Failed to sync native tab member frame");
        }
    }

    fn ensure_active_native_tab_frame(&mut self, wid: WindowId, frame: CGRect) {
        self.request_native_tab_member_frame_sync(wid, frame);
    }

    pub(super) fn retry_pending_native_tab_frame_target(&mut self, wid: WindowId) {
        let Some(frame) = self.native_tab_manager.pending_frame_target(wid) else {
            return;
        };
        self.request_native_tab_member_frame_sync(wid, frame);
    }

    pub(super) fn is_native_tab_suppressed(&self, wid: WindowId) -> bool {
        self.native_tab_manager.is_suppressed(wid)
    }

    pub(super) fn window_is_native_tab_candidate(&self, wid: WindowId) -> bool {
        self.window_manager.windows.get(&wid).is_some_and(|window| {
            !window.info.is_minimized && window.info.is_standard && window.info.is_root
        })
    }

    fn has_other_visible_same_pid_frame(
        &self,
        pid: pid_t,
        frame: CGRect,
        exclude: &[WindowId],
    ) -> bool {
        self.window_manager
            .visible_windows
            .iter()
            .filter_map(|wsid| self.window_manager.window_ids.get(wsid))
            .copied()
            .any(|wid| {
                if wid.pid != pid || exclude.contains(&wid) {
                    return false;
                }
                self.window_manager.windows.get(&wid).is_some_and(|window| {
                    window.matches_filter(WindowFilter::Manageable)
                        && frames_match(window.frame_monotonic, frame)
                })
            })
    }

    fn visible_native_tab_peer_for(
        &self,
        pid: pid_t,
        frame: CGRect,
        exclude: &[WindowId],
    ) -> Option<WindowId> {
        self.window_manager
            .visible_windows
            .iter()
            .filter_map(|wsid| self.window_manager.window_ids.get(wsid))
            .copied()
            .find(|wid| {
                if wid.pid != pid || exclude.contains(wid) {
                    return false;
                }
                self.window_manager.windows.get(wid).is_some_and(|window| {
                    self.window_is_native_tab_candidate(*wid)
                        && !window.is_native_tab_suppressed()
                        && frames_match(window.frame_monotonic, frame)
                        && self.layout_manager.layout_engine.has_window_membership(*wid)
                })
            })
    }

    fn has_matching_pending_native_tab_appearance(&self, wid: WindowId, frame: CGRect) -> bool {
        let Some(window) = self.window_manager.windows.get(&wid) else {
            return false;
        };
        let Some(wsid) = window.info.sys_id else {
            return false;
        };
        let Some(space) = self
            .best_space_for_window(&frame, Some(wsid))
            .or_else(|| self.best_space_for_window_state(window))
        else {
            return false;
        };

        self.native_tab_manager
            .pending_appearances_for_pid(wid.pid)
            .into_iter()
            .any(|pending| {
                pending.wsid == wsid && pending.space == space && frames_match(pending.frame, frame)
            })
    }

    fn visible_native_tab_replacement_peer_for(
        &self,
        new: WindowId,
        frame: CGRect,
    ) -> Option<WindowId> {
        // A same-frame visible sibling is only a replacement candidate if we also
        // observed the matching WindowServer appearance for `new`.
        if !self.has_matching_pending_native_tab_appearance(new, frame) {
            return None;
        }
        self.visible_native_tab_peer_for(new.pid, frame, &[new])
            .filter(|old| !self.has_other_visible_same_pid_frame(new.pid, frame, &[*old, new]))
    }

    pub(super) fn activate_native_tab_replacement(&mut self, old: WindowId, new: WindowId) -> bool {
        let Some(old_window) = self.window_manager.windows.get(&old) else {
            return false;
        };
        let frame = self.native_tab_group_frame_for(old).unwrap_or(old_window.frame_monotonic);
        let old_wsid = old_window.info.sys_id;
        let new_wsid = self.window_manager.windows.get(&new).and_then(|window| window.info.sys_id);
        self.log_native_tab_state("activate_before_old", old);
        self.log_native_tab_state("activate_before_new", new);

        if !self.window_is_native_tab_candidate(old) || !self.window_is_native_tab_candidate(new) {
            return false;
        }
        let _ = self.layout_manager.layout_engine.rekey_window(old, new);
        let group_id = self.native_tab_manager.replace_active_member(old, new, frame);
        self.set_native_tab_role(old, group_id, NativeTabRole::Suppressed);
        self.set_native_tab_role(new, group_id, NativeTabRole::Active);
        if let Some(window) = self.window_manager.windows.get_mut(&new) {
            window.frame_monotonic = frame;
            window.info.frame = frame;
        }
        self.main_window_tracker.rekey_window(old, new);
        if let (Some(old_wsid), Some(new_wsid)) = (old_wsid, new_wsid) {
            self.transaction_manager.rekey_window(old_wsid, new_wsid);
        }
        self.ensure_active_native_tab_frame(new, frame);

        if self.drag_manager.skip_layout_for_window == Some(old) {
            self.drag_manager.skip_layout_for_window = Some(new);
        }
        match &mut self.drag_manager.drag_state {
            crate::actor::reactor::DragState::Active { session }
            | crate::actor::reactor::DragState::PendingSwap { session, .. } => {
                if session.window == old {
                    session.window = new;
                }
            }
            crate::actor::reactor::DragState::Inactive => {}
        }
        if let crate::actor::reactor::DragState::PendingSwap { target, .. } =
            &mut self.drag_manager.drag_state
            && *target == old
        {
            *target = new;
        }
        if self.workspace_switch_manager.pending_workspace_mouse_warp == Some(old) {
            self.workspace_switch_manager.pending_workspace_mouse_warp = Some(new);
        }
        self.log_native_tab_state("activate_after_old", old);
        self.log_native_tab_state("activate_after_new", new);
        true
    }

    fn pending_native_tab_replacement_for(
        &self,
        pid: pid_t,
        new: WindowId,
        space: SpaceId,
        frame: CGRect,
    ) -> Option<WindowId> {
        self.native_tab_manager
            .pending_destroys_for_pid(pid)
            .into_iter()
            .find(|pending| {
                pending.window_id != new
                    && pending.space_id == space
                    && frames_match(pending.frame, frame)
                    && !self.has_other_visible_same_pid_frame(pid, pending.frame, &[
                        pending.window_id,
                        new,
                    ])
            })
            .map(|pending| pending.window_id)
    }

    fn native_tab_replacement_candidate_for(
        &self,
        new: WindowId,
        frame: CGRect,
    ) -> Option<WindowId> {
        let space = self.best_space_for_window_id(new)?;
        self.pending_native_tab_replacement_for(new.pid, new, space, frame)
            .or_else(|| self.visible_native_tab_replacement_peer_for(new, frame))
    }

    fn grouped_native_tab_replacement_for(
        &self,
        old: WindowId,
        known_visible: &std::collections::HashSet<WindowId>,
    ) -> Option<WindowId> {
        let group_id = self.native_tab_manager.group_for_window(old)?;
        let group = self.native_tab_manager.groups.get(&group_id)?;
        group.members.iter().copied().find(|candidate| {
            *candidate != old
                && known_visible.contains(candidate)
                && self.window_is_native_tab_candidate(*candidate)
                && self.window_manager.windows.contains_key(candidate)
        })
    }

    fn candidate_has_pending_native_tab_appearance(&self, wid: WindowId) -> bool {
        let Some(wsid) =
            self.window_manager.windows.get(&wid).and_then(|window| window.info.sys_id)
        else {
            return false;
        };
        self.native_tab_manager
            .pending_appearances_for_pid(wid.pid)
            .into_iter()
            .any(|pending| pending.wsid == wsid)
    }

    pub(super) fn stage_native_tab_destroy(&mut self, wsid: WindowServerId, sid: SpaceId) -> bool {
        let Some(&wid) = self.window_manager.window_ids.get(&wsid) else {
            return false;
        };
        if !self.window_is_native_tab_candidate(wid) {
            return false;
        }
        let Some(window) = self.window_manager.windows.get(&wid) else {
            return false;
        };
        self.native_tab_manager.stage_destroy(wid, wsid, sid, window.frame_monotonic);
        self.window_manager.visible_windows.remove(&wsid);
        self.window_server_info_manager.window_server_info.remove(&wsid);
        true
    }

    pub(super) fn note_native_tab_appearance(
        &mut self,
        wsid: WindowServerId,
        sid: SpaceId,
        info: WindowServerInfo,
    ) -> bool {
        self.window_manager.visible_windows.insert(wsid);
        self.window_server_info_manager.window_server_info.insert(wsid, info);

        let Some(&wid) = self.window_manager.window_ids.get(&wsid) else {
            self.native_tab_manager.stage_appearance(wsid, info.pid, sid, info.frame);
            return false;
        };
        self.native_tab_manager.clear_pending_appearance(info.pid, wsid);

        if let Some(pending_old) =
            self.pending_native_tab_replacement_for(info.pid, wid, sid, info.frame)
            && self.try_activate_native_tab_replacement(pending_old, wid)
        {
            return true;
        }

        let Some(group_id) = self.native_tab_manager.group_for_window(wid) else {
            self.native_tab_manager.stage_appearance(wsid, info.pid, sid, info.frame);
            return false;
        };
        if !self.is_native_tab_suppressed(wid) {
            return false;
        }
        let Some(group) = self.native_tab_manager.groups.get(&group_id) else {
            return false;
        };
        if !frames_match(group.canonical_frame, info.frame) {
            return false;
        }

        let old_active = group.active;
        if let Some(old_active) = old_active {
            if self.try_activate_native_tab_replacement(old_active, wid) {
                return true;
            }
            return false;
        }

        if let Some(group_id) = self.native_tab_manager.set_active_member(wid, info.frame) {
            self.set_native_tab_role(wid, group_id, NativeTabRole::Active);
        }
        true
    }

    pub(super) fn maybe_hold_native_tab_window_created(&self, wid: WindowId) -> bool {
        self.window_is_native_tab_candidate(wid)
            && self.window_manager.windows.get(&wid).is_some_and(|window| {
                self.native_tab_manager
                    .pending_destroys_for_pid(wid.pid)
                    .into_iter()
                    .any(|pending| frames_match(pending.frame, window.frame_monotonic))
                    || self.native_tab_manager.groups.values().any(|group| {
                        group.pid == wid.pid
                            && frames_match(group.canonical_frame, window.frame_monotonic)
                    })
                    || self
                        .visible_native_tab_replacement_peer_for(wid, window.frame_monotonic)
                        .is_some()
            })
    }

    pub(super) fn defer_native_tab_window_destroy(&mut self, wid: WindowId) -> bool {
        let Some(window) = self.window_manager.windows.get(&wid) else {
            return false;
        };
        if !self.window_is_native_tab_candidate(wid) {
            return false;
        }

        let has_pending_destroy = self
            .native_tab_manager
            .pending_destroys_for_pid(wid.pid)
            .into_iter()
            .any(|pending| pending.window_id == wid);
        if !has_pending_destroy {
            return false;
        }

        if let Some(wsid) = window.info.sys_id {
            self.transaction_manager.remove_for_window(wsid);
            self.window_manager.window_ids.remove(&wsid);
            self.window_server_info_manager.window_server_info.remove(&wsid);
            self.window_manager.visible_windows.remove(&wsid);
        }
        if let Some(window) = self.window_manager.windows.get_mut(&wid) {
            window.info.sys_id = None;
        }
        true
    }

    pub(super) fn finalize_native_tab_window_destroy(&mut self, wid: WindowId) {
        self.native_tab_manager.clear_pending_destroy(wid);
        self.native_tab_manager.remove_window(wid, &mut self.window_manager.windows);
    }

    pub(super) fn handle_native_tab_frame_changed(&mut self, wid: WindowId, sync_group: bool) {
        let Some(window) = self.window_manager.windows.get(&wid) else {
            return;
        };
        let frame = window.frame_monotonic;
        let membership = window.native_tab;
        self.native_tab_manager.update_frame(wid, frame, membership);
        if sync_group {
            self.sync_native_tab_group_frame(wid, frame, membership, true);
        }
    }

    pub(super) fn handle_native_tab_app_terminated(&mut self, pid: pid_t) {
        self.native_tab_manager.remove_app(pid, &mut self.window_manager.windows);
    }

    pub(super) fn pending_native_tab_destroys(
        &self,
    ) -> Vec<crate::actor::reactor::managers::PendingNativeTabDestroy> {
        self.native_tab_manager
            .pending_destroys
            .values()
            .flat_map(|pending| pending.iter().copied())
            .collect()
    }

    pub(super) fn handle_native_tab_main_window_changed(
        &mut self,
        pid: pid_t,
        wid: Option<WindowId>,
        quiet: Quiet,
    ) {
        if quiet == Quiet::Yes {
            return;
        }
        if wid.is_none() {
            if self.pid_has_native_tab_state(pid) {
                self.native_tab_manager.note_transient_empty_visibility(pid);
            }
            return;
        }
        self.native_tab_manager.clear_transient_empty_visibility(pid);
        let Some(wid) = wid else {
            return;
        };
        self.log_native_tab_state("main_window_changed_entry", wid);
        if wid.pid != pid || !self.window_is_native_tab_candidate(wid) {
            return;
        }
        if let Some(frame) =
            self.window_manager.windows.get(&wid).map(|window| window.frame_monotonic)
            && !self.is_native_tab_suppressed(wid)
            && let Some(old_active) = self.native_tab_replacement_candidate_for(wid, frame)
            && self.try_activate_native_tab_replacement(old_active, wid)
        {
            return;
        }

        if !self.is_native_tab_suppressed(wid) {
            return;
        }

        let Some(group_id) = self.native_tab_manager.group_for_window(wid) else {
            return;
        };
        let Some(group) = self.native_tab_manager.groups.get(&group_id) else {
            return;
        };
        if let Some(old_active) =
            group.active.filter(|old_active| *old_active != wid).or_else(|| {
                group.members.iter().copied().find(|member| {
                    *member != wid
                        && self.layout_manager.layout_engine.has_window_membership(*member)
                })
            })
            && self.try_activate_native_tab_replacement(old_active, wid)
        {
            return;
        }
    }

    pub(super) fn reconcile_native_tabs_for_pid(&mut self, pid: pid_t, known_visible: &[WindowId]) {
        let known_visible: std::collections::HashSet<WindowId> =
            known_visible.iter().copied().collect();

        for pending in self.native_tab_manager.pending_appearances_for_pid(pid) {
            let Some(&wid) = self.window_manager.window_ids.get(&pending.wsid) else {
                continue;
            };
            if !self.window_is_native_tab_candidate(wid) {
                continue;
            }
            let Some(old_active) =
                self.pending_native_tab_replacement_for(pid, wid, pending.space, pending.frame)
            else {
                continue;
            };
            if self.try_activate_native_tab_replacement(old_active, wid) {
                self.native_tab_manager.clear_pending_appearance(pid, pending.wsid);
            }
        }

        for pending in self.native_tab_manager.pending_destroys_for_pid(pid) {
            let replacement = known_visible.iter().copied().find(|&candidate| {
                if candidate == pending.window_id || candidate.pid != pending.window_id.pid {
                    return false;
                }
                if !self.window_is_native_tab_candidate(candidate) {
                    return false;
                }
                let Some(window) = self.window_manager.windows.get(&candidate) else {
                    return false;
                };
                let Some(sys_id) = window.info.sys_id else {
                    return false;
                };
                if !self
                    .has_matching_pending_native_tab_appearance(candidate, window.frame_monotonic)
                {
                    return false;
                }
                self.window_manager.visible_windows.contains(&sys_id)
                    && self.best_space_for_window_id(candidate) == Some(pending.space_id)
                    && frames_match(window.frame_monotonic, pending.frame)
                    && !self.has_other_visible_same_pid_frame(
                        pending.window_id.pid,
                        pending.frame,
                        &[pending.window_id, candidate],
                    )
            });

            if let Some(new_active) = replacement {
                if self.try_activate_native_tab_replacement(pending.window_id, new_active) {
                    if let Some(wsid) = self
                        .window_manager
                        .windows
                        .get(&new_active)
                        .and_then(|window| window.info.sys_id)
                    {
                        self.native_tab_manager.clear_pending_appearance(pid, wsid);
                    }
                    continue;
                }
            }

            if !known_visible.contains(&pending.window_id) {
                if let Some(new_active) = self
                    .grouped_native_tab_replacement_for(pending.window_id, &known_visible)
                    .filter(|candidate| {
                        self.candidate_has_pending_native_tab_appearance(*candidate)
                    })
                    && self.try_activate_native_tab_replacement(pending.window_id, new_active)
                {
                    if let Some(wsid) = self
                        .window_manager
                        .windows
                        .get(&new_active)
                        .and_then(|window| window.info.sys_id)
                    {
                        self.native_tab_manager.clear_pending_appearance(pid, wsid);
                    }
                    continue;
                }
                self.native_tab_manager.clear_pending_destroy(pending.window_id);
                let _ = crate::actor::reactor::events::window::WindowEventHandler::handle_window_destroyed(
                    self,
                    pending.window_id,
                );
                continue;
            }

            if self.window_manager.visible_windows.contains(&pending.window_server_id) {
                self.native_tab_manager.clear_pending_destroy(pending.window_id);
            }
        }

        let active_windows: Vec<WindowId> = self
            .window_manager
            .visible_windows
            .iter()
            .filter_map(|wsid| self.window_manager.window_ids.get(wsid))
            .copied()
            .filter(|wid| wid.pid == pid)
            .filter(|wid| self.window_is_native_tab_candidate(*wid))
            .filter(|wid| !self.is_native_tab_suppressed(*wid))
            .collect();

        for active_window in active_windows {
            let Some(active_state) = self.window_manager.windows.get(&active_window) else {
                continue;
            };
            let active_frame = active_state.frame_monotonic;
            for candidate in known_visible.iter().copied() {
                if candidate == active_window || candidate.pid != active_window.pid {
                    continue;
                }
                if !self.window_is_native_tab_candidate(candidate) {
                    continue;
                }
                let Some(candidate_state) = self.window_manager.windows.get(&candidate) else {
                    continue;
                };
                if candidate_state
                    .info
                    .sys_id
                    .is_some_and(|sys_id| self.window_manager.visible_windows.contains(&sys_id))
                {
                    continue;
                }
                if !frames_match(candidate_state.frame_monotonic, active_frame) {
                    continue;
                }
                let group_id = self.native_tab_manager.add_background_member(
                    active_window,
                    candidate,
                    active_frame,
                );
                self.set_native_tab_role(candidate, group_id, NativeTabRole::Suppressed);
                self.set_native_tab_role(active_window, group_id, NativeTabRole::Active);
            }
        }
    }
}
