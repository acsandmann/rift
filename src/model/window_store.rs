use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::model::VirtualWorkspaceId;
use crate::model::reactor::WindowState;
use crate::sys::screen::SpaceId;
use crate::sys::window_server::{WindowServerId, WindowServerInfo};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowVisibility {
    #[default]
    Unknown,
    Visible,
    Hidden,
    Minimized,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowPlacement {
    #[default]
    Tiled,
    Floating,
    NativeFullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingWindowOperation {
    pub generation: u64,
    pub requested_frame: Option<objc2_core_foundation::CGRect>,
    pub requested_space: Option<SpaceId>,
}

/// The complete reactor-owned state for one AX window identity.
#[derive(Debug, Default)]
pub struct WindowRecord {
    state: Option<WindowState>,
    window_server_id: Option<WindowServerId>,
    native_space: Option<SpaceId>,
    workspace: Option<WindowWorkspaceInfo>,
    visibility: WindowVisibility,
    placement: WindowPlacement,
    pending_operation: Option<PendingWindowOperation>,
    operation_generation: u64,
    rule_floating: bool,
}

impl WindowRecord {
    /// Last frame observed from Accessibility/WindowServer.
    pub fn observed_frame(&self) -> Option<objc2_core_foundation::CGRect> {
        self.state.as_ref().map(|state| state.info.frame)
    }

    /// Latest frame accepted by the reactor, including its own completed writes.
    pub fn frame(&self) -> Option<objc2_core_foundation::CGRect> {
        self.state.as_ref().map(|state| state.frame_monotonic)
    }

    pub fn requested_frame(&self) -> Option<objc2_core_foundation::CGRect> {
        self.pending_operation.and_then(|operation| operation.requested_frame)
    }

    pub(crate) fn is_admitted_with_rule_override(
        &self,
        rule_override: Option<bool>,
    ) -> Option<bool> {
        self.state.as_ref().map(|state| state.is_admitted_with_override(rule_override))
    }

    pub fn window_server_id(&self) -> Option<WindowServerId> { self.window_server_id }

    pub fn native_space(&self) -> Option<SpaceId> { self.native_space }

    pub fn workspace(&self) -> Option<WindowWorkspaceInfo> { self.workspace }

    pub fn visibility(&self) -> WindowVisibility { self.visibility }

    pub fn placement(&self) -> WindowPlacement { self.placement }

    pub fn pending_operation(&self) -> Option<PendingWindowOperation> { self.pending_operation }
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
    pending_native_fullscreen: Option<PendingNativeFullscreenState>,
}

impl WindowServerRecord {
    fn is_empty(&self) -> bool {
        self.window_id.is_none()
            && !self.visible
            && !self.observed
            && self.space.is_none()
            && self.info.is_none()
            && self.pending_native_fullscreen.is_none()
    }
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
pub struct WindowStore {
    windows: HashMap<WindowId, WindowRecord>,
    app_windows: HashMap<i32, HashSet<WindowId>>,
    window_servers: HashMap<WindowServerId, WindowServerRecord>,
    workspace_windows: HashMap<WindowWorkspaceInfo, HashSet<WindowId>>,
    native_fullscreen_records_by_original_window: HashMap<WindowId, NativeFullscreenRecord>,
    native_fullscreen_original_window_by_current_window: HashMap<WindowId, WindowId>,
    native_fullscreen_original_window_by_window_server: HashMap<WindowServerId, WindowId>,
}

/// Native facts supplementing a successful AX inventory omission. Unknown
/// fields carry no negative authority; an omission alone never retires identity.
#[derive(Debug)]
pub(crate) struct InventoryWindowObservation {
    pub(crate) info: Option<WindowServerInfo>,
    pub(crate) suitable: Option<bool>,
    pub(crate) ordered_in: Option<bool>,
}

/// A user-Space departure can be a move, a close, or temporary ordering out.
#[derive(Debug)]
pub(crate) enum NativeDeparture {
    Moved(Option<WindowId>, SpaceId),
    Closed(WindowId),
    Hidden {
        window: Option<WindowId>,
        remove_projection: bool,
    },
}

impl WindowStore {
    /// Direct topology, live membership, and pending moves have different authority.
    /// A live conflict can supersede a pending move; otherwise its target wins
    /// over a stale direct observation. Unknown live membership retains prior facts.
    pub(crate) fn resolve_native_space(
        &self,
        wsid: WindowServerId,
        observation: Option<SpaceId>,
        pending: Option<SpaceId>,
        live: Option<SpaceId>,
    ) -> Option<SpaceId> {
        match (observation, pending) {
            (Some(observed), Some(target)) if observed != target => {
                Some(if live == Some(observed) {
                    observed
                } else {
                    target
                })
            }
            (Some(observed), _) => Some(observed),
            (None, _) => live.or(pending).or(self.window_server_space(wsid)),
        }
    }

    /// Admission belongs to the catalog; rejecting it also clears stale ownership.
    pub(crate) fn reconcile_admission(&mut self, wid: WindowId) -> bool {
        let admitted = self.window(wid).is_some_and(WindowState::is_admitted);
        if !admitted {
            self.remove_window_assignment(wid);
        }
        admitted
    }

    pub(crate) fn is_visible_admitted(&self, wid: WindowId) -> bool {
        self.record(wid).is_some_and(|record| {
            record.state.as_ref().is_some_and(WindowState::is_admitted)
                && record.window_server_id.is_some_and(|wsid| self.is_window_visible(wsid))
        })
    }

    /// Restore the catalog's current logical owner, including AX replacement.
    /// The boolean requests repair of the superseded identity's projection.
    pub(crate) fn restore_native_identity(
        &mut self,
        wsid: WindowServerId,
        original: WindowId,
    ) -> Option<(WindowId, bool)> {
        let restored = self
            .restore_window_from_native_fullscreen_by_window_server_id(wsid)
            .or_else(|| self.restore_window_from_native_fullscreen(original))?;
        let owner = [
            Some(restored.current_window_id),
            restored.window_server_id.and_then(|id| self.tracked_window_id(id)),
            self.tracked_window_id(wsid),
            Some(original),
        ]
        .into_iter()
        .flatten()
        .find(|wid| self.contains_window(*wid))?;
        let remove_original =
            owner != original && self.remove_window_assignment(original).is_some();
        Some((owner, remove_original))
    }

    /// Native fullscreen suspends admission while retaining identity and ownership.
    /// Both Space appeared/disappeared observations enforce the same invariant.
    pub(crate) fn observe_native_fullscreen(
        &mut self,
        wsid: WindowServerId,
        space: SpaceId,
        fallback_user_space: Option<SpaceId>,
        pid: Option<i32>,
    ) -> Option<WindowId> {
        if let Some(wid) = self.tracked_window_id(wsid) {
            let assignment = self.workspace_info_for_window(wid);
            self.suspend_window_to_native_fullscreen(
                wid,
                Some(wsid),
                fallback_user_space,
                space,
                NativeFullscreenTransition::Suspended,
            );
            assignment
                .filter(|assignment| Some(assignment.space) == fallback_user_space)
                .map(|_| wid)
        } else {
            if let Some(pid) =
                pid.or_else(|| self.get_window_server_info(wsid).map(|info| info.pid))
            {
                self.suspend_window_server_to_native_fullscreen(
                    pid,
                    wsid,
                    fallback_user_space,
                    space,
                    NativeFullscreenTransition::Suspended,
                );
            }
            None
        }
    }

    pub(crate) fn observe_native_departure(
        &mut self,
        wsid: WindowServerId,
        reported_space: SpaceId,
        resolved_space: Option<SpaceId>,
        active_spaces: &HashSet<SpaceId>,
        ordered_in: Option<bool>,
    ) -> NativeDeparture {
        if let Some(space) = resolved_space.filter(|space| *space != reported_space) {
            return NativeDeparture::Moved(
                self.observe_native_space(wsid, space, active_spaces.contains(&space)),
                space,
            );
        }
        let window = self.tracked_window_id(wsid);
        if let Some(wid) = window
            && ordered_in == Some(false)
        {
            return NativeDeparture::Closed(wid);
        }
        let remove_projection = window
            .and_then(|wid| self.workspace_info_for_window(wid))
            .is_some_and(|assignment| assignment.space == reported_space);
        self.observe_native_space(wsid, reported_space, false);
        NativeDeparture::Hidden { window, remove_projection }
    }

    /// Bind a positive AX observation to its stable native identity. Persistent
    /// membership survives AX replacement; only projection rekeying is external.
    pub(crate) fn reconcile_ax_identity(
        &mut self,
        wid: WindowId,
        wsid: Option<WindowServerId>,
        native_space: Option<SpaceId>,
    ) -> Option<WindowId> {
        if let Some(old) = self.record(wid).and_then(WindowRecord::window_server_id)
            && Some(old) != wsid
        {
            self.remove_window_server_state(old);
        }
        let wsid = wsid?;
        let previous = self.track_window_server_id(wsid, wid).filter(|old| *old != wid);
        if let Some(old) = previous {
            self.transfer_persistent_window_metadata(old, wid);
            if !self.contains_window(wid) {
                let state = self.windows.get_mut(&old).and_then(|record| record.state.take());
                if let Some(state) = state {
                    self.insert_window(wid, state);
                }
            }
            self.remove_window(old);
        }
        self.clear_window_server_observed(wsid);
        if let (Some(record), Some(space)) =
            (self.native_fullscreen_record_for_window(wid), native_space)
            && space != record.fullscreen_space
            && Some(space)
                == record
                    .workspace
                    .map(|workspace| workspace.space)
                    .or(record.last_known_user_space)
        {
            self.restore_window_from_native_fullscreen(wid);
        }
        previous
    }

    pub(crate) fn observe_native_space(
        &mut self,
        wsid: WindowServerId,
        space: SpaceId,
        visible: bool,
    ) -> Option<WindowId> {
        self.set_window_server_space(wsid, Some(space));
        self.set_native_visibility(wsid, visible);
        let wid = self.tracked_window_id(wsid);
        if wid.is_some() {
            self.clear_window_server_observed(wsid);
        }
        wid
    }

    /// Enforce the minimized invariant even if the AX flag was already set.
    /// Every known identity requests projection repair; the projection reports
    /// whether removal actually changed geometry, so correct duplicates are cheap.
    pub(crate) fn observe_minimized(&mut self, wid: WindowId) -> bool {
        let Some(window) = self.window_mut(wid) else {
            return false;
        };
        window.info.is_minimized = true;
        window.is_manageable = false;
        if let Some(wsid) = self.record(wid).and_then(WindowRecord::window_server_id) {
            self.mark_window_hidden(wsid);
        }
        self.set_visibility(wid, WindowVisibility::Minimized);
        self.remove_window_assignment(wid);
        true
    }

    /// Hidden is visibility evidence, never destruction. Return the owning PID
    /// for a targeted, coalesced AX refresh (including pending native identity).
    pub(crate) fn observe_native_visibility(
        &mut self,
        wsid: WindowServerId,
        visible: bool,
    ) -> Option<i32> {
        let pid = self
            .tracked_window_id(wsid)
            .map(|wid| wid.pid)
            .or_else(|| self.get_window_server_info(wsid).map(|info| info.pid));
        self.set_native_visibility(wsid, visible);
        pid
    }

    /// A current successful AX omission is destructive only with native negative evidence.
    pub(crate) fn reconcile_app_inventory(
        &mut self,
        pid: i32,
        observed: &[WindowId],
        inactive: &HashSet<WindowId>,
        mut observe: impl FnMut(WindowServerId, Option<WindowServerInfo>) -> InventoryWindowObservation,
    ) -> Vec<(WindowId, Option<WindowServerId>)> {
        let observed: HashSet<_> = observed.iter().copied().collect();
        let retired: Vec<_> = self
            .window_ids_for_pid(pid)
            .filter_map(|wid| {
                let record = self.record(wid)?;
                if observed.contains(&wid)
                    || inactive.contains(&wid)
                    || record.state.as_ref()?.info.is_minimized
                    || record.placement == WindowPlacement::NativeFullscreen
                {
                    return None;
                }
                let wsid = record.window_server_id?;
                let observation = observe(wsid, self.get_window_server_info(wsid));
                (observation.suitable == Some(false)
                    || observation.ordered_in == Some(false)
                    || observation.info.is_some_and(|info| {
                        info.layer != 0
                            || info.frame.size.width.abs() < 2.0
                            || info.frame.size.height.abs() < 2.0
                    }))
                .then_some((wid, Some(wsid)))
            })
            .collect();
        for &(wid, _) in &retired {
            self.remove_window(wid);
        }
        retired
    }

    /// Apply native presence and visibility, returning only affected logical
    /// ownership requiring location confirmation. Unmapped/inactive windows
    /// can become hidden without an unnecessary native membership query.
    pub(crate) fn reconcile_native_snapshot(
        &mut self,
        observed: &[(WindowServerId, Option<SpaceId>)],
        active_spaces: &HashSet<SpaceId>,
    ) -> HashSet<WindowServerId> {
        let observed_ids: HashSet<_> = observed.iter().map(|(wsid, _)| *wsid).collect();
        let hidden: Vec<_> = self
            .iter_visible_window_server_ids()
            .filter(|wsid| !observed_ids.contains(wsid))
            .collect();
        for wsid in hidden {
            self.mark_window_hidden(wsid);
        }
        for &(wsid, space) in observed {
            if let Some(space) = space {
                self.observe_native_space(wsid, space, true);
            } else {
                self.mark_window_visible(wsid);
            }
        }
        self.workspace_windows
            .iter()
            .filter(|(assignment, _)| active_spaces.contains(&assignment.space))
            .flat_map(|(_, windows)| windows.iter())
            .filter_map(|wid| {
                let record = self.record(*wid)?;
                let wsid = record.window_server_id?;
                (record.placement != WindowPlacement::NativeFullscreen
                    && !observed_ids.contains(&wsid))
                .then_some(wsid)
            })
            .collect()
    }

    /// Authoritative absence removes admission, not identity. A confirmed move
    /// retains assignment until the coordinator resolves its target workspace.
    pub(crate) fn reconcile_native_absence(
        &mut self,
        wsid: WindowServerId,
        active_spaces: &HashSet<SpaceId>,
        inactive_target: Option<SpaceId>,
        preserve_assignment: bool,
    ) -> Option<(WindowId, Option<SpaceId>)> {
        self.mark_window_hidden(wsid);
        let wid = self.tracked_window_id(wsid)?;
        let record = self.record(wid)?;
        if record.placement == WindowPlacement::NativeFullscreen
            || !record
                .workspace
                .is_some_and(|assignment| active_spaces.contains(&assignment.space))
        {
            return None;
        }
        if let Some(space) = inactive_target {
            self.set_window_server_space(wsid, Some(space));
            return Some((wid, Some(space)));
        }
        if preserve_assignment {
            return None;
        }
        self.remove_window_assignment(wid);
        Some((wid, None))
    }

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
        let wsid = window.info.sys_id;
        let retained_workspace = {
            let record = self.windows.entry(window_id).or_default();
            record.state = Some(window);
            record.workspace
        };
        self.app_windows.entry(window_id.pid).or_default().insert(window_id);
        if let Some(workspace) = retained_workspace {
            self.add_window_to_workspace_index(window_id, workspace);
        }
        if let Some(wsid) = wsid {
            self.track_window_server_id(wsid, window_id);
        }
    }

    pub fn record(&self, window_id: WindowId) -> Option<&WindowRecord> {
        self.windows.get(&window_id)
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
        self.app_windows
            .get(&pid)
            .into_iter()
            .flat_map(|windows| windows.iter().copied())
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
                && record.window_server_id.is_none()
                && record.workspace.is_none()
                && !record.rule_floating
        });
        if should_remove {
            self.windows.remove(&window_id);
            if let Some(windows) = self.app_windows.get_mut(&window_id.pid) {
                windows.remove(&window_id);
                if windows.is_empty() {
                    self.app_windows.remove(&window_id.pid);
                }
            }
        }
    }

    fn prune_window_server_record(&mut self, wsid: WindowServerId) {
        let should_remove =
            self.window_servers.get(&wsid).is_some_and(WindowServerRecord::is_empty);
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
        let (old, pending_record, space) = {
            let record = self.server_record_mut(wsid);
            let old = record.window_id;
            record.window_id = Some(window_id);
            let pending = record.pending_native_fullscreen.take();
            (old, pending, record.space)
        };
        if let Some(old) = old.filter(|old| *old != window_id)
            && let Some(record) = self.windows.get_mut(&old)
        {
            record.window_server_id = None;
        }
        if let Some(previous_wsid) = self
            .windows
            .get(&window_id)
            .and_then(|record| record.window_server_id)
            .filter(|previous| *previous != wsid)
            && let Some(record) = self.window_servers.get_mut(&previous_wsid)
        {
            record.window_id = None;
            self.prune_window_server_record(previous_wsid);
        }
        let window = self.windows.entry(window_id).or_default();
        window.window_server_id = Some(wsid);
        window.native_space = space;
        self.app_windows.entry(window_id.pid).or_default().insert(window_id);
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

    fn set_native_visibility(&mut self, wsid: WindowServerId, visible: bool) -> bool {
        let record = if visible {
            self.server_record_mut(wsid)
        } else if let Some(record) = self.window_servers.get_mut(&wsid) {
            record
        } else {
            return false;
        };
        let changed = record.visible != visible;
        record.visible = visible;
        if let Some(wid) = record.window_id {
            self.windows.entry(wid).or_default().visibility = if visible {
                WindowVisibility::Visible
            } else {
                WindowVisibility::Hidden
            };
        } else if !visible {
            record.observed = false;
        }
        self.prune_window_server_record(wsid);
        changed
    }

    pub fn mark_window_visible(&mut self, wsid: WindowServerId) -> bool {
        self.set_native_visibility(wsid, true)
    }

    pub fn mark_window_hidden(&mut self, wsid: WindowServerId) -> bool {
        self.set_native_visibility(wsid, false)
    }

    pub fn clear_visible_windows(&mut self) {
        for record in self.window_servers.values_mut() {
            record.visible = false;
            if let Some(wid) = record.window_id {
                self.windows.entry(wid).or_default().visibility = WindowVisibility::Hidden;
            }
        }
        self.window_servers.retain(|_, record| !record.is_empty());
    }

    pub(crate) fn observe_native_window(&mut self, info: WindowServerInfo) -> Option<WindowId> {
        self.track_window_server_info(info);
        self.mark_window_visible(info.id);
        let wid = self.tracked_window_id(info.id)?;
        self.clear_window_server_observed(info.id);
        let window = self.window_mut(wid)?;
        if info.layer == 0 {
            window.frame_monotonic = info.frame;
        }
        Some(wid)
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

    pub fn has_pending_window_for_pid(&self, pid: i32) -> bool {
        self.window_servers.values().any(|record| {
            record.observed
                && record.visible
                && record.window_id.is_none()
                && record.info.is_some_and(|info| info.pid == pid)
        })
    }

    pub fn set_window_server_space(&mut self, wsid: WindowServerId, space: Option<SpaceId>) {
        let record = self.server_record_mut(wsid);
        record.space = space;
        if let Some(window_id) = record.window_id {
            self.windows.entry(window_id).or_default().native_space = space;
        }
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
        }
        if let Some(wid) = wid
            && let Some(record) = self.windows.get_mut(&wid)
        {
            record.window_server_id = None;
            record.native_space = None;
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
        self.windows.entry(window_id).or_default().placement = WindowPlacement::NativeFullscreen;
        self.app_windows.entry(window_id.pid).or_default().insert(window_id);
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
        let record = self.remove_native_fullscreen_record_by_original_window(original_window_id)?;
        if let Some(window) = self.windows.get_mut(&record.current_window_id) {
            window.placement = if window.rule_floating {
                WindowPlacement::Floating
            } else {
                WindowPlacement::Tiled
            };
        }
        Some(record)
    }

    pub fn restore_window_from_native_fullscreen_by_window_server_id(
        &mut self,
        wsid: WindowServerId,
    ) -> Option<NativeFullscreenRecord> {
        let original_window_id =
            self.native_fullscreen_original_window_by_window_server.get(&wsid).copied()?;
        let current_window_id = self
            .native_fullscreen_records_by_original_window
            .get(&original_window_id)?
            .current_window_id;
        self.restore_window_from_native_fullscreen(current_window_id)
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
        self.app_windows.entry(window_id.pid).or_default().insert(window_id);
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

        let (workspace, rule_floating, placement, visibility, pending, generation) =
            match self.windows.get(&from) {
                Some(record) => (
                    record.workspace,
                    record.rule_floating,
                    record.placement,
                    record.visibility,
                    record.pending_operation,
                    record.operation_generation,
                ),
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
        target.placement = placement;
        target.visibility = visibility;
        target.pending_operation = pending;
        target.operation_generation = target.operation_generation.max(generation);
        self.app_windows.entry(to.pid).or_default().insert(to);

        if let Some(source) = self.windows.get_mut(&from) {
            source.workspace = None;
            source.rule_floating = false;
            source.pending_operation = None;
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

    pub fn replace_rule_floating(&mut self, window_id: WindowId, value: bool) -> bool {
        let record = self.windows.entry(window_id).or_default();
        let previous = std::mem::replace(&mut record.rule_floating, value);
        record.placement = if value {
            WindowPlacement::Floating
        } else {
            WindowPlacement::Tiled
        };
        self.prune_window_record(window_id);
        previous
    }

    pub fn clear_rule_floating(&mut self, window_id: WindowId) {
        if let Some(record) = self.windows.get_mut(&window_id) {
            record.rule_floating = false;
            if record.placement == WindowPlacement::Floating {
                record.placement = WindowPlacement::Tiled;
            }
        }
        self.prune_window_record(window_id);
    }

    pub fn rule_floating(&self, window_id: WindowId) -> bool {
        self.windows.get(&window_id).is_some_and(|record| record.rule_floating)
    }

    pub fn clear_rule_metadata(&mut self, window_id: WindowId) {
        if let Some(record) = self.windows.get_mut(&window_id) {
            record.rule_floating = false;
        }
        self.prune_window_record(window_id);
    }

    pub fn remove_window(&mut self, window_id: WindowId) -> Option<WindowRecord> {
        let Some(record) = self.windows.remove(&window_id) else {
            return None;
        };
        if let Some(assignment) = record.workspace {
            self.remove_window_from_workspace_index(window_id, assignment);
        }
        if let Some(wsid) = record.window_server_id {
            self.remove_window_server_state(wsid);
        }
        if let Some(windows) = self.app_windows.get_mut(&window_id.pid) {
            windows.remove(&window_id);
            if windows.is_empty() {
                self.app_windows.remove(&window_id.pid);
            }
        }
        Some(record)
    }

    pub fn remove_windows_for_app(&mut self, pid: i32) {
        let window_ids: Vec<_> =
            self.app_windows.get(&pid).into_iter().flatten().copied().collect();
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

        self.window_servers.retain(|_, record| {
            if record.info.is_some_and(|info| info.pid == pid) {
                return false;
            }
            if record.pending_native_fullscreen.is_some_and(|pending| pending.pid == pid) {
                record.pending_native_fullscreen = None;
            }
            !record.is_empty()
        });
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
            if record.native_space == Some(old_space) {
                record.native_space = Some(new_space);
            }
        }
        for record in self.window_servers.values_mut() {
            if record.space == Some(old_space) {
                record.space = Some(new_space);
            }
        }
    }

    pub fn current_window_server_space_for_window(&self, window_id: WindowId) -> Option<SpaceId> {
        let wsid = self
            .native_fullscreen_record_for_window(window_id)
            .and_then(|record| record.window_server_id)
            .or_else(|| self.record(window_id).and_then(WindowRecord::window_server_id))?;
        self.window_server_space(wsid)
    }

    pub fn set_visibility(&mut self, window_id: WindowId, visibility: WindowVisibility) {
        self.windows.entry(window_id).or_default().visibility = visibility;
        self.app_windows.entry(window_id.pid).or_default().insert(window_id);
    }

    pub fn set_placement(&mut self, window_id: WindowId, placement: WindowPlacement) {
        self.windows.entry(window_id).or_default().placement = placement;
        self.app_windows.entry(window_id.pid).or_default().insert(window_id);
    }

    pub fn begin_operation(
        &mut self,
        window_id: WindowId,
        requested_frame: Option<objc2_core_foundation::CGRect>,
        requested_space: Option<SpaceId>,
    ) -> PendingWindowOperation {
        let record = self.windows.entry(window_id).or_default();
        record.operation_generation = record.operation_generation.wrapping_add(1);
        let operation = PendingWindowOperation {
            generation: record.operation_generation,
            requested_frame,
            requested_space,
        };
        record.pending_operation = Some(operation);
        self.app_windows.entry(window_id.pid).or_default().insert(window_id);
        operation
    }

    pub fn confirm_operation(&mut self, window_id: WindowId, generation: u64) -> bool {
        let Some(record) = self.windows.get_mut(&window_id) else {
            return false;
        };
        if record
            .pending_operation
            .is_some_and(|operation| operation.generation == generation)
        {
            record.pending_operation = None;
            true
        } else {
            false
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub fn debug_assert_invariants(&self) {
        for (&wid, record) in &self.windows {
            debug_assert!(self.app_windows.get(&wid.pid).is_some_and(|ids| ids.contains(&wid)));
            if let Some(wsid) = record.window_server_id {
                debug_assert_eq!(self.tracked_window_id(wsid), Some(wid));
                debug_assert_eq!(record.native_space, self.window_server_space(wsid));
            }
            if let Some(workspace) = record.workspace {
                let indexed =
                    self.workspace_windows.get(&workspace).is_some_and(|ids| ids.contains(&wid));
                debug_assert_eq!(indexed, record.state.is_some());
            }
        }
        for (&pid, ids) in &self.app_windows {
            debug_assert!(ids.iter().all(|wid| wid.pid == pid && self.windows.contains_key(wid)));
        }
        for (&wsid, server) in &self.window_servers {
            if let Some(wid) = server.window_id {
                debug_assert_eq!(
                    self.windows.get(&wid).and_then(|record| record.window_server_id),
                    Some(wsid)
                );
            }
        }
        for (workspace, ids) in &self.workspace_windows {
            debug_assert!(ids.iter().all(|wid| self.windows.get(wid).is_some_and(|record| {
                record.state.is_some() && record.workspace == Some(*workspace)
            })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::virtual_workspace::WorkspaceStore;

    #[test]
    fn app_termination_clears_stale_pending_fullscreen_without_erasing_another_pid() {
        let mut store = WindowStore::default();
        let wsid = WindowServerId::new(42);
        store.track_window_server_info(WindowServerInfo {
            id: wsid,
            pid: 8,
            layer: 0,
            frame: Default::default(),
            min_frame: Default::default(),
            max_frame: Default::default(),
        });
        store.suspend_window_server_to_native_fullscreen(
            7,
            wsid,
            Some(SpaceId::new(1)),
            SpaceId::new(99),
            NativeFullscreenTransition::Suspended,
        );
        store.remove_windows_for_app(7);
        assert_eq!(store.get_window_server_info(wsid).unwrap().pid, 8);
        assert!(store.pending_native_fullscreen_record_for_window_server_id(wsid).is_none());
        store.debug_assert_invariants();
    }

    #[test]
    fn pending_registration_requires_visible_unmapped_window_for_pid() {
        let mut store = WindowStore::default();
        let wsid = WindowServerId::new(77);
        store.track_window_server_info(WindowServerInfo {
            id: wsid,
            pid: 2,
            layer: 0,
            frame: Default::default(),
            min_frame: Default::default(),
            max_frame: Default::default(),
        });
        store.mark_window_server_observed(wsid);
        assert!(!store.has_pending_window_for_pid(2));
        store.mark_window_visible(wsid);
        assert!(store.has_pending_window_for_pid(2));
        assert!(!store.has_pending_window_for_pid(3));
        store.track_window_server_id(wsid, WindowId::new(2, 1));
        assert!(!store.has_pending_window_for_pid(2));
    }

    #[test]
    fn authoritative_space_only_record_is_not_pruned() {
        let mut window_store = WindowStore::default();
        let wsid = WindowServerId::new(77);
        let space = SpaceId::new(9);

        window_store.set_window_server_space(wsid, Some(space));

        assert_eq!(window_store.window_server_space(wsid), Some(space));
        assert_eq!(window_store.iter_window_server_ids().collect::<Vec<_>>(), vec![
            wsid
        ]);
    }

    #[test]
    fn authoritative_space_record_is_pruned_when_space_is_cleared() {
        let mut window_store = WindowStore::default();
        let wsid = WindowServerId::new(78);

        window_store.set_window_server_space(wsid, Some(SpaceId::new(10)));
        window_store.set_window_server_space(wsid, None);

        assert_eq!(window_store.window_server_space(wsid), None);
        assert!(window_store.iter_window_server_ids().next().is_none());
    }

    #[test]
    fn transfer_persistent_metadata_replaces_existing_target_workspace_assignment() {
        let mut window_store = WindowStore::default();
        let space = SpaceId::new(10);
        let mut workspaces = WorkspaceStore::new();
        let source_workspace = workspaces
            .create_workspace(space, Some("Source".to_string()))
            .expect("source workspace");
        let target_workspace = workspaces
            .create_workspace(space, Some("Target".to_string()))
            .expect("target workspace");
        let from = WindowId::new(1, 1);
        let to = WindowId::new(1, 2);

        window_store.assign_window_to_workspace(from, WindowWorkspaceInfo {
            space,
            workspace_id: source_workspace,
        });
        window_store.assign_window_to_workspace(to, WindowWorkspaceInfo {
            space,
            workspace_id: target_workspace,
        });

        window_store.transfer_persistent_window_metadata(from, to);

        assert_eq!(
            window_store.workspace_info_for_window(to),
            Some(WindowWorkspaceInfo {
                space,
                workspace_id: source_workspace,
            })
        );
        assert!(window_store.workspace_windows(space, target_workspace).is_empty());
        assert_eq!(window_store.workspace_windows(space, source_workspace), vec![to]);
    }

    #[test]
    fn transfer_persistent_metadata_rekeys_native_fullscreen_record() {
        let mut window_store = WindowStore::default();
        let space = SpaceId::new(10);
        let fullscreen_space = SpaceId::new(0x400000000 + space.get());
        let mut workspaces = WorkspaceStore::new();
        let workspace_id =
            workspaces.create_workspace(space, Some("Main".to_string())).expect("workspace");
        let from = WindowId::new(1, 1);
        let to = WindowId::new(1, 2);
        let wsid = WindowServerId::new(77);

        window_store.assign_window_to_workspace(from, WindowWorkspaceInfo { space, workspace_id });
        let _ = window_store.suspend_window_to_native_fullscreen(
            from,
            Some(wsid),
            Some(space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );

        window_store.transfer_persistent_window_metadata(from, to);

        let record = window_store
            .native_fullscreen_record_for_window(to)
            .expect("fullscreen record should follow rekey");
        assert_eq!(record.current_window_id, to);
        assert_eq!(record.window_server_id, Some(wsid));
        assert_eq!(
            record.workspace,
            Some(WindowWorkspaceInfo { space, workspace_id })
        );
        assert_eq!(
            window_store
                .native_fullscreen_record_for_window(from)
                .expect("original key should still resolve the lifecycle")
                .current_window_id,
            to
        );
    }

    #[test]
    fn native_fullscreen_record_preserves_explicit_fallback_user_space_without_assignment() {
        let mut window_store = WindowStore::default();
        let wid = WindowId::new(1, 1);
        let wsid = WindowServerId::new(91);
        let user_space = SpaceId::new(11);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let record = window_store.suspend_window_to_native_fullscreen(
            wid,
            Some(wsid),
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );

        assert_eq!(record.workspace, None);
        assert_eq!(record.last_known_user_space, Some(user_space));
        assert_eq!(
            window_store
                .native_fullscreen_record_for_window_server_id(wsid)
                .expect("record should be discoverable by wsid")
                .last_known_user_space,
            Some(user_space)
        );
    }

    #[test]
    fn remove_window_preserves_native_fullscreen_record_until_app_cleanup() {
        let mut window_store = WindowStore::default();
        let wid = WindowId::new(7, 1);
        let wsid = WindowServerId::new(92);
        let user_space = SpaceId::new(12);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let frame = objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(0.0, 0.0),
            objc2_core_foundation::CGSize::new(100.0, 100.0),
        );
        window_store.insert_window(
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
        let _ = window_store.suspend_window_to_native_fullscreen(
            wid,
            Some(wsid),
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );

        window_store.remove_window(wid);

        assert!(
            window_store.native_fullscreen_record_for_window(wid).is_some(),
            "transient AX removal should not drop the fullscreen lifecycle record"
        );

        window_store.remove_windows_for_app(wid.pid);

        assert!(
            window_store.native_fullscreen_record_for_window(wid).is_none(),
            "app cleanup should retire the fullscreen lifecycle record"
        );
    }

    #[test]
    fn track_window_server_id_binds_pending_native_fullscreen_record() {
        let mut window_store = WindowStore::default();
        let wid = WindowId::new(8, 1);
        let wsid = WindowServerId::new(93);
        let user_space = SpaceId::new(13);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let pending = window_store.suspend_window_server_to_native_fullscreen(
            wid.pid,
            wsid,
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );
        assert_eq!(pending.pid, wid.pid);

        window_store.track_window_server_id(wsid, wid);

        assert!(
            window_store
                .pending_native_fullscreen_record_for_window_server_id(wsid)
                .is_none(),
            "binding AX identity should consume the pending fullscreen record"
        );
        assert_eq!(
            window_store
                .native_fullscreen_record_for_window_server_id(wsid)
                .expect("resolved record should be indexed by wsid")
                .current_window_id,
            wid
        );
    }

    #[test]
    fn track_window_server_id_discards_stale_pending_native_fullscreen_record_on_pid_mismatch() {
        let mut window_store = WindowStore::default();
        let pending_wid = WindowId::new(8, 1);
        let rebound_wid = WindowId::new(9, 1);
        let wsid = WindowServerId::new(94);
        let user_space = SpaceId::new(14);
        let fullscreen_space = SpaceId::new(0x400000000 + user_space.get());

        let pending = window_store.suspend_window_server_to_native_fullscreen(
            pending_wid.pid,
            wsid,
            Some(user_space),
            fullscreen_space,
            NativeFullscreenTransition::Suspended,
        );
        assert_eq!(pending.pid, pending_wid.pid);

        window_store.track_window_server_id(wsid, rebound_wid);

        assert!(
            window_store
                .pending_native_fullscreen_record_for_window_server_id(wsid)
                .is_none(),
            "binding a different app to the wsid should discard stale pending fullscreen state"
        );
        assert!(
            window_store.native_fullscreen_record_for_window_server_id(wsid).is_none(),
            "stale pending fullscreen state must not be rebound onto a different app"
        );
    }

    #[test]
    fn rebinding_server_identity_keeps_both_indexes_unique() {
        let mut store = WindowStore::default();
        let first = WindowId::new(1, 1);
        let second = WindowId::new(1, 2);
        let wsid = WindowServerId::new(44);

        store.track_window_server_id(wsid, first);
        assert_eq!(store.track_window_server_id(wsid, second), Some(first));

        assert_eq!(
            store.record(first).and_then(WindowRecord::window_server_id),
            None
        );
        assert_eq!(
            store.record(second).and_then(WindowRecord::window_server_id),
            Some(wsid)
        );
        assert_eq!(store.tracked_window_id(wsid), Some(second));
        store.debug_assert_invariants();
    }

    #[test]
    fn stale_operation_confirmation_cannot_clear_a_newer_request() {
        let mut store = WindowStore::default();
        let wid = WindowId::new(2, 1);
        let first = store.begin_operation(wid, None, Some(SpaceId::new(1)));
        let second = store.begin_operation(wid, None, Some(SpaceId::new(2)));

        assert!(!store.confirm_operation(wid, first.generation));
        assert_eq!(
            store.record(wid).and_then(WindowRecord::pending_operation),
            Some(second)
        );
        assert!(store.confirm_operation(wid, second.generation));
        assert_eq!(store.record(wid).and_then(WindowRecord::pending_operation), None);
    }

    #[test]
    fn rekey_transfers_placement_and_pending_operation() {
        let mut store = WindowStore::default();
        let from = WindowId::new(3, 1);
        let to = WindowId::new(3, 2);
        store.set_placement(from, WindowPlacement::Floating);
        let pending = store.begin_operation(from, None, Some(SpaceId::new(8)));

        store.transfer_persistent_window_metadata(from, to);

        let target = store.record(to).expect("target record");
        assert_eq!(target.placement(), WindowPlacement::Floating);
        assert_eq!(target.pending_operation(), Some(pending));
        assert_eq!(
            store.record(from).and_then(WindowRecord::pending_operation),
            None
        );
        store.debug_assert_invariants();
    }

    #[test]
    fn app_cleanup_removes_all_indexes_and_pending_operations() {
        let mut store = WindowStore::default();
        let wid = WindowId::new(4, 1);
        let wsid = WindowServerId::new(55);
        let mut workspaces = WorkspaceStore::new();
        let workspace_id = workspaces
            .create_workspace(SpaceId::new(9), Some("Cleanup".to_string()))
            .expect("workspace");
        store.track_window_server_id(wsid, wid);
        store.assign_window_to_workspace(wid, WindowWorkspaceInfo {
            space: SpaceId::new(9),
            workspace_id,
        });
        store.begin_operation(wid, None, Some(SpaceId::new(9)));

        store.remove_windows_for_app(wid.pid);

        assert!(store.record(wid).is_none());
        assert_eq!(store.tracked_window_id(wsid), None);
        assert!(store.window_ids_for_pid(wid.pid).next().is_none());
        store.debug_assert_invariants();
    }
}
