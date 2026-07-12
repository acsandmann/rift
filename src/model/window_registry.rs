use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::model::VirtualWorkspaceId;
use crate::model::reactor::WindowState;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

#[derive(Debug, Default)]
struct WindowRecord {
    state: Option<WindowState>,
    workspace: Option<WindowWorkspaceInfo>,
    rule_floating: bool,
    last_rule_decision: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFullscreenTransition {
    EnterRequested,
    Suspended,
    ExitRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeFullscreenRecord {
    pub original_window_id: WindowId,
    pub current_window_id: WindowId,
    pub window_server_id: Option<WindowServerId>,
    pub workspace: Option<WindowWorkspaceInfo>,
    pub last_known_user_space: Option<SpaceId>,
    pub fullscreen_space: SpaceId,
    pub transition: NativeFullscreenTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingNativeFullscreenRecord {
    pub pid: i32,
    pub window_server_id: WindowServerId,
    pub last_known_user_space: Option<SpaceId>,
    pub fullscreen_space: SpaceId,
    pub transition: NativeFullscreenTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingNativeFullscreenState {
    pid: i32,
    last_known_user_space: Option<SpaceId>,
    fullscreen_space: SpaceId,
    transition: NativeFullscreenTransition,
}

#[derive(Debug, Default)]
struct WindowServerRecord {
    window_id: Option<WindowId>,
    visible: bool,
    observed: bool,
    space: Option<SpaceId>,
    info: Option<WindowServerInfo>,
    recent_at: Option<Instant>,
    pending_native_fullscreen: Option<PendingNativeFullscreenState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowWorkspaceInfo {
    pub space: SpaceId,
    pub workspace_id: VirtualWorkspaceId,
}

/// Authoritative per-window metadata tracked by Rift.
///
/// Workspace membership lives here, not inside `VirtualWorkspace`. Layout trees
/// are only a materialized projection for arranging visible windows. Keeping the
/// assignment index here avoids the old class of bugs where a window could be
/// present in multiple workspace-owned sets after sleep/wake or same-space
/// workspace moves, which then leaked into queries and layout recovery.
#[derive(Debug, Default)]
pub struct WindowRegistry {
    windows: HashMap<WindowId, WindowRecord>,
    window_servers: HashMap<WindowServerId, WindowServerRecord>,
    workspace_windows: HashMap<WindowWorkspaceInfo, HashSet<WindowId>>,
    native_fullscreen_records_by_original_window: HashMap<WindowId, NativeFullscreenRecord>,
    native_fullscreen_original_window_by_current_window: HashMap<WindowId, WindowId>,
    native_fullscreen_original_window_by_window_server: HashMap<WindowServerId, WindowId>,
}

impl WindowRegistry {
    fn native_fullscreen_original_window(&self, window_id: WindowId) -> Option<WindowId> {
        if self.native_fullscreen_records_by_original_window.contains_key(&window_id) {
            Some(window_id)
        } else {
            self.native_fullscreen_original_window_by_current_window
                .get(&window_id)
                .copied()
        }
    }

    fn upsert_native_fullscreen_record(
        &mut self,
        record: NativeFullscreenRecord,
    ) -> NativeFullscreenRecord {
        if let Some(previous) = self
            .native_fullscreen_records_by_original_window
            .insert(record.original_window_id, record)
        {
            self.native_fullscreen_original_window_by_current_window
                .remove(&previous.current_window_id);
            if let Some(previous_wsid) = previous.window_server_id {
                self.native_fullscreen_original_window_by_window_server.remove(&previous_wsid);
            }
        }

        self.native_fullscreen_original_window_by_current_window
            .insert(record.current_window_id, record.original_window_id);
        if let Some(wsid) = record.window_server_id {
            self.native_fullscreen_original_window_by_window_server
                .insert(wsid, record.original_window_id);
        }

        record
    }

    fn remove_native_fullscreen_record_by_original_window(
        &mut self,
        original_window_id: WindowId,
    ) -> Option<NativeFullscreenRecord> {
        let record =
            self.native_fullscreen_records_by_original_window.remove(&original_window_id)?;
        self.native_fullscreen_original_window_by_current_window
            .remove(&record.current_window_id);
        if let Some(wsid) = record.window_server_id {
            self.native_fullscreen_original_window_by_window_server.remove(&wsid);
        }
        Some(record)
    }

    fn remove_window_from_workspace_index(
        &mut self,
        window_id: WindowId,
        assignment: WindowWorkspaceInfo,
    ) {
        let should_prune = if let Some(windows) = self.workspace_windows.get_mut(&assignment) {
            windows.remove(&window_id);
            windows.is_empty()
        } else {
            false
        };
        if should_prune {
            self.workspace_windows.remove(&assignment);
        }
    }

    fn add_window_to_workspace_index(
        &mut self,
        window_id: WindowId,
        assignment: WindowWorkspaceInfo,
    ) {
        self.workspace_windows.entry(assignment).or_default().insert(window_id);
    }

    pub(crate) fn window(&self, window_id: WindowId) -> Option<&WindowState> {
        self.windows.get(&window_id).and_then(|record| record.state.as_ref())
    }

    pub(crate) fn window_mut(&mut self, window_id: WindowId) -> Option<&mut WindowState> {
        self.windows.get_mut(&window_id).and_then(|record| record.state.as_mut())
    }

    pub(crate) fn insert_window(&mut self, window_id: WindowId, window: WindowState) {
        self.windows.entry(window_id).or_default().state = Some(window);
    }

    pub fn contains_window(&self, window_id: WindowId) -> bool { self.window(window_id).is_some() }

    pub fn tracked_window_count(&self) -> usize {
        self.windows.values().filter(|record| record.state.is_some()).count()
    }

    pub(crate) fn iter_windows(&self) -> impl Iterator<Item = (WindowId, &WindowState)> + '_ {
        self.windows.iter().filter_map(|(&window_id, record)| {
            record.state.as_ref().map(|state| (window_id, state))
        })
    }

    pub fn window_ids_for_pid(&self, pid: i32) -> impl Iterator<Item = WindowId> + '_ {
        self.iter_windows()
            .filter(move |(window_id, _)| window_id.pid == pid)
            .map(|(window_id, _)| window_id)
    }

    pub fn iter_window_server_ids(&self) -> impl Iterator<Item = WindowServerId> + '_ {
        self.window_servers.keys().copied()
    }

    pub fn iter_tracked_window_server_ids(&self) -> impl Iterator<Item = WindowServerId> + '_ {
        self.window_servers
            .iter()
            .filter_map(|(&wsid, record)| record.window_id.map(|_| wsid))
    }

    pub fn iter_visible_window_server_ids(&self) -> impl Iterator<Item = WindowServerId> + '_ {
        self.window_servers
            .iter()
            .filter_map(|(&wsid, record)| record.visible.then_some(wsid))
    }

    pub fn window_server_info_count(&self) -> usize {
        self.window_servers.values().filter(|record| record.info.is_some()).count()
    }

    pub fn visible_window_server_count(&self) -> usize {
        self.window_servers.values().filter(|record| record.visible).count()
    }

    fn server_record_mut(&mut self, wsid: WindowServerId) -> &mut WindowServerRecord {
        self.window_servers.entry(wsid).or_default()
    }

    fn prune_window_record(&mut self, window_id: WindowId) {
        let should_remove = self.windows.get(&window_id).is_some_and(|record| {
            record.state.is_none()
                && record.workspace.is_none()
                && !record.rule_floating
                && !record.last_rule_decision
        });
        if should_remove {
            self.windows.remove(&window_id);
        }
    }

    fn prune_window_server_record(&mut self, wsid: WindowServerId) {
        let should_remove = self.window_servers.get(&wsid).is_some_and(|record| {
            record.window_id.is_none()
                && !record.visible
                && !record.observed
                && record.space.is_none()
                && record.info.is_none()
                && record.recent_at.is_none()
                && record.pending_native_fullscreen.is_none()
        });
        if should_remove {
            self.window_servers.remove(&wsid);
        }
    }

    pub fn tracked_window_id(&self, wsid: WindowServerId) -> Option<WindowId> {
        self.window_servers.get(&wsid).and_then(|record| record.window_id)
    }

    pub fn track_window_server_id(
        &mut self,
        wsid: WindowServerId,
        window_id: WindowId,
    ) -> Option<WindowId> {
        let (old, pending_record) = {
            let record = self.server_record_mut(wsid);
            let old = record.window_id;
            record.window_id = Some(window_id);
            let pending = record.pending_native_fullscreen.take();
            (old, pending)
        };
        if let Some(pending_record) = pending_record {
            if pending_record.pid != window_id.pid {
                self.prune_window_server_record(wsid);
                return old;
            }
            let _ = self.suspend_window_to_native_fullscreen(
                window_id,
                Some(wsid),
                pending_record.last_known_user_space,
                pending_record.fullscreen_space,
                pending_record.transition,
            );
        } else if let Some(original_window_id) = self.native_fullscreen_original_window(window_id)
            && let Some(mut native_record) =
                self.remove_native_fullscreen_record_by_original_window(original_window_id)
        {
            native_record.window_server_id = Some(wsid);
            self.upsert_native_fullscreen_record(native_record);
        }
        self.prune_window_server_record(wsid);
        old
    }

    pub fn track_window_server_info(&mut self, info: WindowServerInfo) -> Option<WindowServerInfo> {
        let record = self.server_record_mut(info.id);
        let old = record.info;
        record.info = Some(info);
        old
    }

    pub fn get_window_server_info(&self, wsid: WindowServerId) -> Option<WindowServerInfo> {
        self.window_servers.get(&wsid).and_then(|record| record.info)
    }

    pub fn knows_window_server_id(&self, wsid: WindowServerId) -> bool {
        self.window_servers.get(&wsid).is_some_and(|record| record.info.is_some())
    }

    pub fn mark_window_visible(&mut self, wsid: WindowServerId) -> bool {
        let record = self.server_record_mut(wsid);
        let changed = !record.visible;
        record.visible = true;
        changed
    }

    pub fn set_visible_windows<I>(&mut self, wsids: I)
    where I: IntoIterator<Item = WindowServerId> {
        for wsid in wsids {
            self.mark_window_visible(wsid);
        }
    }

    pub fn clear_visible_windows(&mut self) {
        let known_wsids: Vec<_> = self.window_servers.keys().copied().collect();
        for wsid in known_wsids {
            if let Some(record) = self.window_servers.get_mut(&wsid) {
                record.visible = false;
            }
            self.prune_window_server_record(wsid);
        }
    }

    pub fn mark_window_hidden(&mut self, wsid: WindowServerId) -> bool {
        let changed = self.window_servers.get(&wsid).is_some_and(|record| record.visible);
        if let Some(record) = self.window_servers.get_mut(&wsid) {
            record.visible = false;
        }
        self.prune_window_server_record(wsid);
        changed
    }

    pub fn is_window_visible(&self, wsid: WindowServerId) -> bool {
        self.window_servers.get(&wsid).is_some_and(|record| record.visible)
    }

    pub fn mark_window_server_observed(&mut self, wsid: WindowServerId) -> bool {
        let record = self.server_record_mut(wsid);
        let changed = !record.observed;
        record.observed = true;
        changed
    }

    pub fn clear_window_server_observed(&mut self, wsid: WindowServerId) -> bool {
        let changed = self.window_servers.get(&wsid).is_some_and(|record| record.observed);
        if let Some(record) = self.window_servers.get_mut(&wsid) {
            record.observed = false;
        }
        self.prune_window_server_record(wsid);
        changed
    }

    pub fn is_window_server_observed(&self, wsid: WindowServerId) -> bool {
        self.window_servers.get(&wsid).is_some_and(|record| record.observed)
    }

    pub fn set_window_server_space(&mut self, wsid: WindowServerId, space: Option<SpaceId>) {
        let record = self.server_record_mut(wsid);
        record.space = space;
        self.prune_window_server_record(wsid);
    }

    pub fn window_server_space(&self, wsid: WindowServerId) -> Option<SpaceId> {
        self.window_servers.get(&wsid).and_then(|record| record.space)
    }

    pub fn remove_window_server_state(&mut self, wsid: WindowServerId) -> Option<WindowId> {
        let wid = self.tracked_window_id(wsid);
        if let Some(record) = self.window_servers.get_mut(&wsid) {
            record.window_id = None;
            record.visible = false;
            record.observed = false;
            record.space = None;
            record.info = None;
            record.recent_at = None;
        }
        self.prune_window_server_record(wsid);
        wid
    }

    pub fn suspend_window_to_native_fullscreen(
        &mut self,
        window_id: WindowId,
        window_server_id: Option<WindowServerId>,
        fallback_last_known_user_space: Option<SpaceId>,
        fullscreen_space: SpaceId,
        transition: NativeFullscreenTransition,
    ) -> NativeFullscreenRecord {
        let original_window_id =
            self.native_fullscreen_original_window(window_id).unwrap_or(window_id);
        let existing = self
            .native_fullscreen_records_by_original_window
            .get(&original_window_id)
            .copied();
        let workspace = self.workspace_info_for_window(window_id);
        let record = NativeFullscreenRecord {
            original_window_id,
            current_window_id: window_id,
            window_server_id: window_server_id
                .or_else(|| existing.and_then(|record| record.window_server_id)),
            workspace: workspace.or_else(|| existing.and_then(|record| record.workspace)),
            last_known_user_space: workspace
                .map(|assignment| assignment.space)
                .or(fallback_last_known_user_space)
                .or_else(|| existing.and_then(|record| record.last_known_user_space)),
            fullscreen_space,
            transition,
        };
        self.upsert_native_fullscreen_record(record)
    }

    pub fn suspend_window_server_to_native_fullscreen(
        &mut self,
        pid: i32,
        window_server_id: WindowServerId,
        fallback_last_known_user_space: Option<SpaceId>,
        fullscreen_space: SpaceId,
        transition: NativeFullscreenTransition,
    ) -> PendingNativeFullscreenRecord {
        let state = {
            let existing = self
                .window_servers
                .get(&window_server_id)
                .and_then(|record| record.pending_native_fullscreen);
            PendingNativeFullscreenState {
                pid,
                last_known_user_space: fallback_last_known_user_space
                    .or_else(|| existing.and_then(|record| record.last_known_user_space)),
                fullscreen_space,
                transition,
            }
        };
        self.server_record_mut(window_server_id).pending_native_fullscreen = Some(state);
        PendingNativeFullscreenRecord {
            pid: state.pid,
            window_server_id,
            last_known_user_space: state.last_known_user_space,
            fullscreen_space: state.fullscreen_space,
            transition: state.transition,
        }
    }

    pub fn native_fullscreen_record_for_window(
        &self,
        window_id: WindowId,
    ) -> Option<NativeFullscreenRecord> {
        let original_window_id = self.native_fullscreen_original_window(window_id)?;
        self.native_fullscreen_records_by_original_window
            .get(&original_window_id)
            .copied()
    }

    pub fn native_fullscreen_record_for_window_server_id(
        &self,
        wsid: WindowServerId,
    ) -> Option<NativeFullscreenRecord> {
        let original_window_id =
            self.native_fullscreen_original_window_by_window_server.get(&wsid).copied()?;
        self.native_fullscreen_records_by_original_window
            .get(&original_window_id)
            .copied()
    }

    pub fn pending_native_fullscreen_record_for_window_server_id(
        &self,
        wsid: WindowServerId,
    ) -> Option<PendingNativeFullscreenRecord> {
        self.window_servers
            .get(&wsid)
            .and_then(|record| record.pending_native_fullscreen)
            .map(|record| PendingNativeFullscreenRecord {
                pid: record.pid,
                window_server_id: wsid,
                last_known_user_space: record.last_known_user_space,
                fullscreen_space: record.fullscreen_space,
                transition: record.transition,
            })
    }

    pub fn iter_native_fullscreen_records(
        &self,
    ) -> impl Iterator<Item = NativeFullscreenRecord> + '_ {
        self.native_fullscreen_records_by_original_window.values().copied()
    }

    pub fn restore_window_from_native_fullscreen(
        &mut self,
        window_id: WindowId,
    ) -> Option<NativeFullscreenRecord> {
        let original_window_id = self.native_fullscreen_original_window(window_id)?;
        self.remove_native_fullscreen_record_by_original_window(original_window_id)
    }

    pub fn restore_window_from_native_fullscreen_by_window_server_id(
        &mut self,
        wsid: WindowServerId,
    ) -> Option<NativeFullscreenRecord> {
        let original_window_id =
            self.native_fullscreen_original_window_by_window_server.get(&wsid).copied()?;
        self.remove_native_fullscreen_record_by_original_window(original_window_id)
    }

    pub fn is_window_native_fullscreen_suspended(&self, window_id: WindowId) -> bool {
        self.native_fullscreen_record_for_window(window_id)
            .is_some_and(|record| record.transition == NativeFullscreenTransition::Suspended)
    }

    pub fn is_window_server_id_native_fullscreen_suspended(&self, wsid: WindowServerId) -> bool {
        self.native_fullscreen_record_for_window_server_id(wsid)
            .is_some_and(|record| record.transition == NativeFullscreenTransition::Suspended)
    }

    pub fn pending_native_fullscreen_pid_for_window_server_id(
        &self,
        wsid: WindowServerId,
    ) -> Option<i32> {
        self.pending_native_fullscreen_record_for_window_server_id(wsid)
            .map(|record| record.pid)
    }

    pub fn assign_window_to_workspace(
        &mut self,
        window_id: WindowId,
        assignment: WindowWorkspaceInfo,
    ) -> Option<WindowWorkspaceInfo> {
        let old = self.windows.get(&window_id).and_then(|record| record.workspace);
        if let Some(old_assignment) = old {
            self.remove_window_from_workspace_index(window_id, old_assignment);
        }
        self.windows.entry(window_id).or_default().workspace = Some(assignment);
        self.add_window_to_workspace_index(window_id, assignment);
        if let Some(original_window_id) = self.native_fullscreen_original_window(window_id)
            && let Some(mut record) =
                self.remove_native_fullscreen_record_by_original_window(original_window_id)
        {
            record.workspace = Some(assignment);
            record.last_known_user_space = Some(assignment.space);
            self.upsert_native_fullscreen_record(record);
        }
        old
    }

    pub fn workspace_info_for_window(&self, window_id: WindowId) -> Option<WindowWorkspaceInfo> {
        self.windows.get(&window_id).and_then(|record| record.workspace)
    }

    pub fn workspace_for_window(
        &self,
        space: SpaceId,
        window_id: WindowId,
    ) -> Option<VirtualWorkspaceId> {
        self.workspace_info_for_window(window_id)
            .filter(|assignment| assignment.space == space)
            .map(|assignment| assignment.workspace_id)
    }

    pub fn workspaces_for_window(&self, window_id: WindowId) -> Vec<VirtualWorkspaceId> {
        self.workspace_info_for_window(window_id)
            .map(|assignment| vec![assignment.workspace_id])
            .unwrap_or_default()
    }

    pub fn workspace_windows(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> Vec<WindowId> {
        let assignment = WindowWorkspaceInfo { space, workspace_id };
        let mut windows: Vec<_> = self
            .workspace_windows
            .get(&assignment)
            .into_iter()
            .flat_map(|windows| windows.iter().copied())
            .collect();
        windows.sort_unstable_by_key(|wid| (wid.pid, wid.idx.get()));
        windows
    }

    pub fn workspace_window_count(
        &self,
        space: SpaceId,
        workspace_id: VirtualWorkspaceId,
    ) -> usize {
        let assignment = WindowWorkspaceInfo { space, workspace_id };
        self.workspace_windows.get(&assignment).map_or(0, HashSet::len)
    }

    pub fn has_workspace_assignments_in_space(&self, space: SpaceId) -> bool {
        self.workspace_windows.keys().any(|assignment| assignment.space == space)
    }

    pub fn remove_window_assignment(&mut self, window_id: WindowId) -> Option<WindowWorkspaceInfo> {
        let old = self.windows.get_mut(&window_id).and_then(|record| record.workspace.take());
        if let Some(old_assignment) = old {
            self.remove_window_from_workspace_index(window_id, old_assignment);
        }
        self.prune_window_record(window_id);
        old
    }

    /// Move workspace/rule metadata from an old AX window id to a new one when
    /// macOS rekeys the same WindowServer window across sleep/wake or similar
    /// churn. The caller remains responsible for replacing any layout/window
    /// state that still references `from`.
    pub fn transfer_persistent_window_metadata(&mut self, from: WindowId, to: WindowId) {
        if from == to {
            return;
        }

        let (workspace, rule_floating, last_rule_decision) = match self.windows.get(&from) {
            Some(record) => (record.workspace, record.rule_floating, record.last_rule_decision),
            None => return,
        };

        let target_workspace = self.windows.get(&to).and_then(|record| record.workspace);

        if let Some(assignment) = workspace {
            self.remove_window_from_workspace_index(from, assignment);
            if let Some(target_assignment) = target_workspace {
                self.remove_window_from_workspace_index(to, target_assignment);
            }
            self.add_window_to_workspace_index(to, assignment);
        }

        let target = self.windows.entry(to).or_default();
        if workspace.is_some() {
            target.workspace = workspace;
        }
        target.rule_floating |= rule_floating;
        target.last_rule_decision |= last_rule_decision;

        if let Some(source) = self.windows.get_mut(&from) {
            source.workspace = None;
            source.rule_floating = false;
            source.last_rule_decision = false;
        }

        if let Some(original_window_id) = self.native_fullscreen_original_window(from)
            && let Some(mut record) =
                self.remove_native_fullscreen_record_by_original_window(original_window_id)
        {
            if record.current_window_id == from {
                record.current_window_id = to;
            }
            if record.workspace.is_none() {
                record.workspace = workspace;
            }
            if record.last_known_user_space.is_none() {
                record.last_known_user_space = workspace.map(|assignment| assignment.space);
            }
            self.upsert_native_fullscreen_record(record);
        }

        self.prune_window_record(from);
    }

    pub fn set_rule_floating(&mut self, window_id: WindowId, value: bool) {
        self.windows.entry(window_id).or_default().rule_floating = value;
        self.prune_window_record(window_id);
    }

    pub fn clear_rule_floating(&mut self, window_id: WindowId) {
        if let Some(record) = self.windows.get_mut(&window_id) {
            record.rule_floating = false;
        }
        self.prune_window_record(window_id);
    }

    pub fn rule_floating(&self, window_id: WindowId) -> bool {
        self.windows.get(&window_id).is_some_and(|record| record.rule_floating)
    }

    pub fn set_last_rule_decision(&mut self, window_id: WindowId, value: bool) {
        self.windows.entry(window_id).or_default().last_rule_decision = value;
    }

    pub fn last_rule_decision(&self, window_id: WindowId) -> bool {
        self.windows.get(&window_id).is_some_and(|record| record.last_rule_decision)
    }

    pub fn clear_rule_metadata(&mut self, window_id: WindowId) {
        if let Some(record) = self.windows.get_mut(&window_id) {
            record.rule_floating = false;
            record.last_rule_decision = false;
        }
        self.prune_window_record(window_id);
    }

    pub fn remove_window(&mut self, window_id: WindowId) {
        if let Some(record) = self.windows.remove(&window_id)
            && let Some(assignment) = record.workspace
        {
            self.remove_window_from_workspace_index(window_id, assignment);
        }
        let server_ids: Vec<_> = self
            .window_servers
            .iter()
            .filter_map(|(&wsid, record)| (record.window_id == Some(window_id)).then_some(wsid))
            .collect();
        for wsid in server_ids {
            self.remove_window_server_state(wsid);
        }
    }

    pub fn remove_windows_for_app(&mut self, pid: i32) {
        let window_ids: Vec<_> =
            self.windows.keys().copied().filter(|window_id| window_id.pid == pid).collect();
        for window_id in window_ids {
            self.remove_window(window_id);
        }

        let fullscreen_keys: Vec<_> = self
            .native_fullscreen_records_by_original_window
            .keys()
            .copied()
            .filter(|window_id| window_id.pid == pid)
            .collect();
        for original_window_id in fullscreen_keys {
            let _ = self.remove_native_fullscreen_record_by_original_window(original_window_id);
        }

        let wsids: Vec<_> = self.window_servers.keys().copied().collect();
        for wsid in wsids {
            if let Some(record) = self.window_servers.get_mut(&wsid)
                && record.pending_native_fullscreen.is_some_and(|pending| pending.pid == pid)
            {
                record.pending_native_fullscreen = None;
            }
            self.prune_window_server_record(wsid);
        }
    }

    pub fn iter_workspace_assignments(
        &self,
    ) -> impl Iterator<Item = (WindowId, WindowWorkspaceInfo)> + '_ {
        self.windows.iter().filter_map(|(&window_id, record)| {
            record.workspace.map(|workspace| (window_id, workspace))
        })
    }

    pub fn workspace_assignment_count(&self) -> usize {
        self.windows.values().filter(|record| record.workspace.is_some()).count()
    }

    pub fn remap_space(&mut self, old_space: SpaceId, new_space: SpaceId) {
        if old_space == new_space {
            return;
        }

        let moved_assignments: Vec<_> = self
            .workspace_windows
            .keys()
            .copied()
            .filter(|assignment| assignment.space == old_space)
            .collect();
        for old_assignment in moved_assignments {
            if let Some(windows) = self.workspace_windows.remove(&old_assignment) {
                self.workspace_windows.insert(
                    WindowWorkspaceInfo {
                        space: new_space,
                        workspace_id: old_assignment.workspace_id,
                    },
                    windows,
                );
            }
        }

        for record in self.windows.values_mut() {
            if let Some(assignment) = record.workspace.as_mut()
                && assignment.space == old_space
            {
                assignment.space = new_space;
            }
        }
    }

    pub fn mark_wsids_recent<I>(&mut self, wsids: I)
    where I: IntoIterator<Item = WindowServerId> {
        let now = Instant::now();
        for wsid in wsids {
            self.server_record_mut(wsid).recent_at = Some(now);
        }
    }

    pub fn is_wsid_recent(&self, wsid: WindowServerId, ttl_ms: u64) -> bool {
        self.window_servers
            .get(&wsid)
            .and_then(|record| record.recent_at)
            .is_some_and(|ts| ts.elapsed().as_millis() < ttl_ms as u128)
    }

    pub fn purge_expired(&mut self, ttl_ms: u64) {
        let now = Instant::now();
        let wsids: Vec<_> = self.window_servers.keys().copied().collect();
        for wsid in wsids {
            if let Some(record) = self.window_servers.get_mut(&wsid)
                && record
                    .recent_at
                    .is_some_and(|ts| now.duration_since(ts).as_millis() >= ttl_ms as u128)
            {
                record.recent_at = None;
            }
            self.prune_window_server_record(wsid);
        }
    }

    pub fn current_window_server_space_for_window(&self, window_id: WindowId) -> Option<SpaceId> {
        let wsid = self
            .native_fullscreen_record_for_window(window_id)
            .and_then(|record| record.window_server_id)
            .or_else(|| {
                self.window_servers.iter().find_map(|(&wsid, record)| {
                    (record.window_id == Some(window_id)).then_some(wsid)
                })
            })?;
        self.window_server_space(wsid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::virtual_workspace::VirtualWorkspaceManager;

    #[test]
    fn authoritative_space_only_record_is_not_pruned() {
        let mut registry = WindowRegistry::default();
        let wsid = WindowServerId::new(77);
        let space = SpaceId::new(9);

        registry.set_window_server_space(wsid, Some(space));

        assert_eq!(registry.window_server_space(wsid), Some(space));
        assert_eq!(registry.iter_window_server_ids().collect::<Vec<_>>(), vec![wsid]);
    }

    #[test]
    fn authoritative_space_record_is_pruned_when_space_is_cleared() {
        let mut registry = WindowRegistry::default();
        let wsid = WindowServerId::new(78);

        registry.set_window_server_space(wsid, Some(SpaceId::new(10)));
        registry.set_window_server_space(wsid, None);

        assert_eq!(registry.window_server_space(wsid), None);
        assert!(registry.iter_window_server_ids().next().is_none());
    }

    #[test]
    fn transfer_persistent_metadata_replaces_existing_target_workspace_assignment() {
        let mut registry = WindowRegistry::default();
        let space = SpaceId::new(10);
        let mut workspaces = VirtualWorkspaceManager::new();
        let source_workspace = workspaces
            .create_workspace(space, Some("Source".to_string()))
            .expect("source workspace");
        let target_workspace = workspaces
            .create_workspace(space, Some("Target".to_string()))
            .expect("target workspace");
        let from = WindowId::new(1, 1);
        let to = WindowId::new(1, 2);

        registry.assign_window_to_workspace(from, WindowWorkspaceInfo {
            space,
            workspace_id: source_workspace,
        });
        registry.assign_window_to_workspace(to, WindowWorkspaceInfo {
            space,
            workspace_id: target_workspace,
        });

        registry.transfer_persistent_window_metadata(from, to);

        assert_eq!(
            registry.workspace_info_for_window(to),
            Some(WindowWorkspaceInfo {
                space,
                workspace_id: source_workspace,
            })
        );
        assert!(registry.workspace_windows(space, target_workspace).is_empty());
        assert_eq!(registry.workspace_windows(space, source_workspace), vec![to]);
    }

    #[test]
    fn transfer_persistent_metadata_rekeys_native_fullscreen_record() {
        let mut registry = WindowRegistry::default();
        let space = SpaceId::new(10);
        let fullscreen_space = SpaceId::new(0x400000000 + space.get());
        let mut workspaces = VirtualWorkspaceManager::new();
        let workspace_id =
            workspaces.create_workspace(space, Some("Main".to_string())).expect("workspace");
        let from = WindowId::new(1, 1);
        let to = WindowId::new(1, 2);
        let wsid = WindowServerId::new(77);

        registry.assign_window_to_workspace(from, WindowWorkspaceInfo { space, workspace_id });
        let _ = registry.suspend_window_to_native_fullscreen(
            from,
            Some(wsid),
            Some(space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );

        registry.transfer_persistent_window_metadata(from, to);

        let record = registry
            .native_fullscreen_record_for_window(to)
            .expect("fullscreen record should follow rekey");
        assert_eq!(record.current_window_id, to);
        assert_eq!(record.window_server_id, Some(wsid));
        assert_eq!(
            record.workspace,
            Some(WindowWorkspaceInfo { space, workspace_id })
        );
        assert_eq!(
            registry
                .native_fullscreen_record_for_window(from)
                .expect("original key should still resolve the lifecycle")
                .current_window_id,
            to
        );
    }

    #[test]
    fn native_fullscreen_record_preserves_explicit_fallback_user_space_without_assignment() {
        let mut registry = WindowRegistry::default();
        let wid = WindowId::new(1, 1);
        let wsid = WindowServerId::new(91);
        let user_space = SpaceId::new(11);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let record = registry.suspend_window_to_native_fullscreen(
            wid,
            Some(wsid),
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );

        assert_eq!(record.workspace, None);
        assert_eq!(record.last_known_user_space, Some(user_space));
        assert_eq!(
            registry
                .native_fullscreen_record_for_window_server_id(wsid)
                .expect("record should be discoverable by wsid")
                .last_known_user_space,
            Some(user_space)
        );
    }

    #[test]
    fn remove_window_preserves_native_fullscreen_record_until_app_cleanup() {
        let mut registry = WindowRegistry::default();
        let wid = WindowId::new(7, 1);
        let wsid = WindowServerId::new(92);
        let user_space = SpaceId::new(12);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let frame = objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(0.0, 0.0),
            objc2_core_foundation::CGSize::new(100.0, 100.0),
        );
        registry.insert_window(
            wid,
            WindowState::from(crate::sys::app::WindowInfo {
                is_standard: true,
                is_root: true,
                is_minimized: false,
                is_resizable: true,
                min_size: None,
                max_size: None,
                title: "Window".to_string(),
                frame,
                sys_id: Some(wsid),
                bundle_id: None,
                path: None,
                ax_role: None,
                ax_subrole: None,
            }),
        );
        let _ = registry.suspend_window_to_native_fullscreen(
            wid,
            Some(wsid),
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );

        registry.remove_window(wid);

        assert!(
            registry.native_fullscreen_record_for_window(wid).is_some(),
            "transient AX removal should not drop the fullscreen lifecycle record"
        );

        registry.remove_windows_for_app(wid.pid);

        assert!(
            registry.native_fullscreen_record_for_window(wid).is_none(),
            "app cleanup should retire the fullscreen lifecycle record"
        );
    }

    #[test]
    fn track_window_server_id_binds_pending_native_fullscreen_record() {
        let mut registry = WindowRegistry::default();
        let wid = WindowId::new(8, 1);
        let wsid = WindowServerId::new(93);
        let user_space = SpaceId::new(13);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let pending = registry.suspend_window_server_to_native_fullscreen(
            wid.pid,
            wsid,
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );
        assert_eq!(pending.pid, wid.pid);

        registry.track_window_server_id(wsid, wid);

        assert!(
            registry.pending_native_fullscreen_record_for_window_server_id(wsid).is_none(),
            "binding AX identity should consume the pending fullscreen record"
        );
        assert_eq!(
            registry
                .native_fullscreen_record_for_window_server_id(wsid)
                .expect("resolved record should be indexed by wsid")
                .current_window_id,
            wid
        );
    }

    #[test]
    fn track_window_server_id_discards_stale_pending_native_fullscreen_record_on_pid_mismatch() {
        let mut registry = WindowRegistry::default();
        let pending_wid = WindowId::new(8, 1);
        let rebound_wid = WindowId::new(9, 1);
        let wsid = WindowServerId::new(94);
        let user_space = SpaceId::new(14);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let pending = registry.suspend_window_server_to_native_fullscreen(
            pending_wid.pid,
            wsid,
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );
        assert_eq!(pending.pid, pending_wid.pid);

        registry.track_window_server_id(wsid, rebound_wid);

        assert!(
            registry.pending_native_fullscreen_record_for_window_server_id(wsid).is_none(),
            "binding a different app to the wsid should discard stale pending fullscreen state"
        );
        assert!(
            registry.native_fullscreen_record_for_window_server_id(wsid).is_none(),
            "stale pending fullscreen state must not be rebound onto a different app"
        );
    }
}
