use std::ptr::NonNull;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
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

#[derive(Debug, Default)]
struct WindowServerRecord {
    window_id: Option<WindowId>,
    visible: bool,
    observed: bool,
    info: Option<WindowServerInfo>,
    recent_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowWorkspaceInfo {
    pub space: SpaceId,
    pub workspace_id: VirtualWorkspaceId,
}

#[derive(Debug, Default)]
pub struct WindowRegistry {
    windows: HashMap<WindowId, WindowRecord>,
    window_servers: HashMap<WindowServerId, WindowServerRecord>,
}

impl WindowRegistry {
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
                && record.info.is_none()
                && record.recent_at.is_none()
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
        let record = self.server_record_mut(wsid);
        let old = record.window_id;
        record.window_id = Some(window_id);
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

    pub fn remove_window_server_state(&mut self, wsid: WindowServerId) -> Option<WindowId> {
        let wid = self.tracked_window_id(wsid);
        if let Some(record) = self.window_servers.get_mut(&wsid) {
            record.window_id = None;
            record.visible = false;
            record.observed = false;
            record.info = None;
            record.recent_at = None;
        }
        self.prune_window_server_record(wsid);
        wid
    }

    pub fn assign_window_to_workspace(
        &mut self,
        window_id: WindowId,
        assignment: WindowWorkspaceInfo,
    ) -> Option<WindowWorkspaceInfo> {
        let record = self.windows.entry(window_id).or_default();
        let old = record.workspace;
        record.workspace = Some(assignment);
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

    pub fn remove_window_assignment(&mut self, window_id: WindowId) -> Option<WindowWorkspaceInfo> {
        let old = self.windows.get_mut(&window_id).and_then(|record| record.workspace.take());
        self.prune_window_record(window_id);
        old
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
        self.windows.remove(&window_id);
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
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowRegistryHandle(Option<NonNull<WindowRegistry>>);

// SAFETY: The handle is only attached to the boxed registry owned by Reactor and
// is used from the reactor thread after construction. It does not provide
// independent ownership or synchronization.
unsafe impl Send for WindowRegistryHandle {}
unsafe impl Sync for WindowRegistryHandle {}

impl WindowRegistryHandle {
    pub fn new() -> Self { Self::default() }

    pub fn attach(&mut self, registry: &mut WindowRegistry) {
        self.0 = Some(NonNull::from(registry));
    }

    pub fn get(&self) -> &WindowRegistry {
        unsafe { self.0.expect("window registry was not attached").as_ref() }
    }

    pub fn get_mut(&mut self) -> &mut WindowRegistry {
        unsafe { self.0.expect("window registry was not attached").as_mut() }
    }
}
