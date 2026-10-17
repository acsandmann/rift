//! Restores the layout position of windows that leave temporarily.
//!
//! A window that is minimized, hidden with its app, or moved to a native
//! fullscreen space is removed from its layout. Without this, it would come
//! back through the new-window path: inserted next to the current selection,
//! with default sizes, and in the active workspace when minimizing has already
//! cleared its assignment.
//!
//! When a window leaves, the engine keeps a copy of its workspace's layout
//! system as it was before the first of the currently departed windows left.
//! When a window returns and the workspace has otherwise only lost windows,
//! that copy, minus the windows that are still away, replaces the current
//! layouts. If the workspace changed in another way, the window is inserted
//! after the window that preceded it instead.

use rift_protocol::ContainerTreeNode;

use super::LayoutEngine;
use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::{HashMap, HashSet};
use crate::layout_engine::{LayoutId, LayoutSystem, LayoutSystemKind};
use crate::model::WindowStore;
use crate::model::virtual_workspace::{VirtualWorkspace, VirtualWorkspaceId};
use crate::sys::screen::SpaceId;

#[derive(Debug, Default)]
pub(super) struct Departures {
    windows: HashMap<WindowId, Departure>,
    /// Per workspace, its layout system from before its first departed window
    /// left.
    snapshots: HashMap<VirtualWorkspaceId, LayoutSystemKind>,
    sequence: u64,
}

#[derive(Debug)]
struct Departure {
    space: SpaceId,
    workspace: VirtualWorkspaceId,
    /// The windows of the active layout when it left, used to find a neighbor
    /// when the snapshot no longer matches.
    order: Vec<WindowId>,
    /// Removal order and method, so that the snapshot can drop the window the
    /// same way the live layout did.
    sequence: u64,
    rebalanced: bool,
}

impl LayoutEngine {
    /// Record where `wid` is and remove it from its workspace's layouts.
    /// Called for every removal, since a window can leave through several
    /// paths (native fullscreen alone removes it more than once).
    pub(super) fn note_departure(&mut self, window_store: &WindowStore, wid: WindowId) {
        let tiled_in = self.tiling_workspace(wid);
        // A later removal of a window that is already away (e.g. once it is
        // on the fullscreen space) must not replace the original record.
        if tiled_in.is_none() && self.departures.windows.contains_key(&wid) {
            return;
        }
        let Some((space, workspace_id)) = tiled_in.or_else(|| {
            // Floating windows are only known through their assignment.
            let assigned = window_store.workspace_info_for_window(wid)?;
            Some((assigned.space, assigned.workspace_id))
        }) else {
            return;
        };

        // The engine rebalances the parent split when the store no longer
        // assigns the window, as after minimizing.
        let rebalanced = window_store.workspaces_for_window(wid).is_empty();
        let workspace = &mut self.workspaces[workspace_id];
        let order = workspace
            .active_layout()
            .filter(|&layout| workspace.layout_system.contains_window(layout, wid))
            .map(|layout| workspace.layout_system.all_windows_in_layout(layout))
            .unwrap_or_default();
        if tiled_in.is_some() {
            let current = windows_in(workspace, &workspace.layout_system);
            let snapshot_still_valid =
                self.departures.snapshots.get(&workspace_id).is_some_and(|snapshot| {
                    reduced_to(workspace, snapshot, &current, &self.departures.windows)
                        .is_some_and(|reduced| same_layouts(workspace, &reduced))
                });
            if !snapshot_still_valid && let Some(snapshot) = workspace.layout_system.preview_clone()
            {
                self.departures.snapshots.insert(workspace_id, snapshot);
            }
            remove(&mut workspace.layout_system, wid, rebalanced);
        }

        self.departures.sequence += 1;
        self.departures.windows.insert(wid, Departure {
            space,
            workspace: workspace_id,
            order,
            sequence: self.departures.sequence,
            rebalanced,
        });
    }

    /// The space and workspace whose layouts contain `wid`.
    fn tiling_workspace(&self, wid: WindowId) -> Option<(SpaceId, VirtualWorkspaceId)> {
        self.workspaces.workspaces.iter().find_map(|(id, workspace)| {
            let contains = |layout| workspace.layout_system.contains_window(layout, wid);
            workspace
                .layout_state
                .all_layouts()
                .any(contains)
                .then_some((workspace.space, id))
        })
    }

    /// The workspace `wid` left, if it left from `space`.
    pub(super) fn departed_workspace(
        &self,
        wid: WindowId,
        space: SpaceId,
    ) -> Option<VirtualWorkspaceId> {
        let departure = self.departures.windows.get(&wid)?;
        (departure.space == space).then_some(departure.workspace)
    }

    /// Forget `wid` once it is placed on the space it left. While in native
    /// fullscreen it is placed on the fullscreen space, which does not count.
    pub(super) fn forget_departure_on(&mut self, wid: WindowId, space: SpaceId) {
        if self.departed_workspace(wid, space).is_some() {
            self.forget_departure(wid);
        }
    }

    fn forget_departure(&mut self, wid: WindowId) {
        if let Some(departure) = self.departures.windows.remove(&wid) {
            self.drop_unused_snapshot(departure.workspace);
        }
    }

    pub(super) fn forget_departures_of_app(&mut self, pid: pid_t) {
        let windows: Vec<_> =
            self.departures.windows.keys().filter(|wid| wid.pid == pid).copied().collect();
        for wid in windows {
            self.forget_departure(wid);
        }
    }

    /// Snapshots hold the layout settings they were taken with, so they are
    /// dropped when the settings change. Departed windows then return next to
    /// their neighbors.
    pub(super) fn drop_departure_snapshots(&mut self) { self.departures.snapshots.clear(); }

    fn drop_unused_snapshot(&mut self, workspace: VirtualWorkspaceId) {
        if !self
            .departures
            .windows
            .values()
            .any(|departure| departure.workspace == workspace)
        {
            self.departures.snapshots.remove(&workspace);
        }
    }

    /// Put a returning `wid` back into `layout` of `workspace_id`, the
    /// workspace it left. Returns false if it did not leave from there, or no
    /// position could be restored; the caller then inserts it as usual.
    pub(super) fn restore_departed_window(
        &mut self,
        workspace_id: VirtualWorkspaceId,
        layout: LayoutId,
        wid: WindowId,
    ) -> bool {
        if self.departures.windows.get(&wid).map(|d| d.workspace) != Some(workspace_id) {
            return false;
        }
        let Some(departure) = self.departures.windows.remove(&wid) else {
            return false;
        };
        let restored = self.restore_snapshot(workspace_id, wid)
            || self.insert_after_neighbor(workspace_id, layout, wid, &departure.order);
        self.drop_unused_snapshot(workspace_id);
        restored
    }

    /// Replace the workspace's layouts with its snapshot, reduced to the
    /// current windows plus `wid`, if the snapshot without `wid` matches the
    /// current layouts.
    fn restore_snapshot(&mut self, workspace_id: VirtualWorkspaceId, wid: WindowId) -> bool {
        let Some(snapshot) = self.departures.snapshots.get(&workspace_id) else {
            return false;
        };
        let workspace = &mut self.workspaces[workspace_id];
        if !windows_in(workspace, snapshot).contains(&wid) {
            return false;
        }
        let mut current = windows_in(workspace, &workspace.layout_system);
        let departed = &self.departures.windows;
        let matches = reduced_to(workspace, snapshot, &current, departed)
            .is_some_and(|reduced| same_layouts(workspace, &reduced));
        if !matches {
            return false;
        }
        current.insert(wid);
        let Some(restored) = reduced_to(workspace, snapshot, &current, departed) else {
            return false;
        };
        workspace.layout_system = restored;
        true
    }

    /// Insert `wid` after the nearest window that preceded it in `order` and
    /// is still in `layout`, or else after the nearest that followed it.
    fn insert_after_neighbor(
        &mut self,
        workspace_id: VirtualWorkspaceId,
        layout: LayoutId,
        wid: WindowId,
        order: &[WindowId],
    ) -> bool {
        let system = &mut self.workspaces[workspace_id].layout_system;
        let Some(index) = order.iter().position(|&window| window == wid) else {
            return false;
        };
        let Some(&neighbor) = order[..index]
            .iter()
            .rev()
            .chain(&order[index + 1..])
            .find(|&&window| system.contains_window(layout, window))
        else {
            return false;
        };
        system.select_window(layout, neighbor);
        system.add_window_after_selection(layout, wid);
        true
    }
}

/// The windows in `system`'s layouts for `workspace`.
fn windows_in(workspace: &VirtualWorkspace, system: &LayoutSystemKind) -> HashSet<WindowId> {
    workspace
        .layout_state
        .all_layouts()
        .filter(|&layout| system.contains_layout(layout))
        .flat_map(|layout| system.all_windows_in_layout(layout))
        .collect()
}

/// A copy of `snapshot` with only the windows in `keep`. The others are
/// removed in the order and the way they left, so the result matches the
/// live layouts if nothing else changed.
fn reduced_to(
    workspace: &VirtualWorkspace,
    snapshot: &LayoutSystemKind,
    keep: &HashSet<WindowId>,
    departed: &HashMap<WindowId, Departure>,
) -> Option<LayoutSystemKind> {
    let mut removed: Vec<_> = windows_in(workspace, snapshot).difference(keep).copied().collect();
    removed.sort_by_key(|wid| departed.get(wid).map_or(u64::MAX, |d| d.sequence));
    let mut reduced = snapshot.preview_clone()?;
    for wid in removed {
        let rebalanced = departed.get(&wid).is_some_and(|d| d.rebalanced);
        remove(&mut reduced, wid, rebalanced);
    }
    Some(reduced)
}

fn remove(system: &mut LayoutSystemKind, wid: WindowId, rebalance: bool) {
    if rebalance {
        system.remove_window_and_rebalance_parent(wid);
    } else {
        system.remove_window(wid);
    }
}

/// Whether `other` arranges every layout of `workspace` as the workspace's own
/// layout system does.
fn same_layouts(workspace: &VirtualWorkspace, other: &LayoutSystemKind) -> bool {
    workspace.layout_state.all_layouts().all(|layout| {
        other.contains_layout(layout)
            && same_arrangement(
                &other.container_tree(layout),
                &workspace.layout_system.container_tree(layout),
            )
    })
}

/// Compares two container trees by windows, structure and weights, ignoring
/// node ids and selection, which differ between copies.
fn same_arrangement(a: &ContainerTreeNode, b: &ContainerTreeNode) -> bool {
    let same_weight = match (a.weight, b.weight) {
        (Some(a), Some(b)) => (a - b).abs() < 1e-6,
        (a, b) => a.is_none() && b.is_none(),
    };
    same_weight
        && a.node_type == b.node_type
        && a.layout_kind == b.layout_kind
        && a.window_id == b.window_id
        && a.is_fullscreen == b.is_fullscreen
        && a.is_fullscreen_within_gaps == b.is_fullscreen_within_gaps
        && a.role == b.role
        && a.children.len() == b.children.len()
        && a.children.iter().zip(&b.children).all(|(a, b)| same_arrangement(a, b))
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::common::config::{LayoutMode, LayoutSettings, VirtualWorkspaceSettings};
    use crate::layout_engine::LayoutEvent;
    use crate::layout_engine::engine::WindowLayoutInfo;

    const SCREEN: CGRect = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 1200.0, height: 800.0 },
    };

    struct Setup {
        engine: LayoutEngine,
        store: WindowStore,
        space: SpaceId,
    }

    impl Setup {
        fn new(mode: LayoutMode, windows: &[WindowId]) -> Self {
            let settings = LayoutSettings {
                mode,
                ..LayoutSettings::default()
            };
            let mut setup = Self {
                engine: LayoutEngine::new(&VirtualWorkspaceSettings::default(), &settings, None),
                store: WindowStore::default(),
                space: SpaceId::new(7),
            };
            setup.event(LayoutEvent::SpaceExposed(setup.space, SCREEN.size));
            setup.observe(windows);
            setup
        }

        fn event(&mut self, event: LayoutEvent) {
            let _ = self.engine.handle_event(&mut self.store, event);
        }

        fn observe(&mut self, windows: &[WindowId]) {
            let info = windows
                .iter()
                .map(|&window_id| WindowLayoutInfo {
                    window_id,
                    title: None,
                    ax_role: None,
                    ax_subrole: None,
                    is_resizable: true,
                    current_size: CGSize::new(0.0, 0.0),
                    min_size: None,
                    max_size: None,
                })
                .collect();
            self.event(LayoutEvent::windows_observed(self.space, 1, info, None));
        }

        fn focus(&mut self, window: WindowId) {
            self.event(LayoutEvent::WindowFocused(self.space, window));
        }

        fn frames(&mut self) -> Vec<(WindowId, CGRect)> {
            let mut frames = self.engine.calculate_layout(
                self.space,
                SCREEN,
                &LayoutSettings::default().gaps,
                Default::default(),
            );
            frames.sort_by_key(|(window, _)| window.idx);
            frames
        }

        fn order(&self) -> Vec<WindowId> {
            let (workspace, layout) =
                self.engine.workspaces.active_layout_for_space(self.space).unwrap();
            self.engine.workspaces[workspace].layout_system.all_windows_in_layout(layout)
        }
    }

    fn windows(count: u32) -> Vec<WindowId> {
        (1..=count).map(|idx| WindowId::new(1, idx)).collect()
    }

    #[test]
    fn a_window_comes_back_to_its_place_in_every_layout() {
        for mode in [
            LayoutMode::Bsp,
            LayoutMode::Traditional,
            LayoutMode::MasterStack,
            LayoutMode::Scrolling,
            LayoutMode::Stack,
        ] {
            let w = windows(3);
            let mut setup = Setup::new(mode, &w);
            // Select another window, where a new window would be inserted.
            setup.focus(w[2]);
            let before = setup.frames();
            setup.event(LayoutEvent::WindowRemoved(w[0]));
            setup.event(LayoutEvent::WindowAdded(setup.space, w[0]));
            assert_eq!(setup.frames(), before, "{mode:?}");
        }
    }

    #[test]
    fn a_minimized_window_comes_back_at_its_size() {
        let w = windows(3);
        let mut setup = Setup::new(LayoutMode::Bsp, &w);
        setup.focus(w[0]);
        let space = setup.space;
        let _ = setup.engine.handle_command(
            &mut setup.store,
            Some(space),
            &[space],
            &HashMap::default(),
            crate::layout_engine::LayoutCommand::ResizeWindowBy { amount: 0.2 },
        );
        let before = setup.frames();
        setup.focus(w[1]);
        // Minimizing clears the store's assignment before the removal, so the
        // split is rebalanced.
        let _ = setup.store.remove_window_assignment(w[2]);
        setup.event(LayoutEvent::WindowRemoved(w[2]));
        assert_ne!(setup.frames().len(), before.len());
        setup.event(LayoutEvent::WindowAdded(space, w[2]));
        assert_eq!(setup.frames(), before);
    }

    #[test]
    fn windows_away_together_come_back_in_any_order() {
        let w = windows(4);
        let mut setup = Setup::new(LayoutMode::Bsp, &w);
        let before = setup.frames();
        setup.focus(w[3]);
        for &window in &w[..3] {
            setup.event(LayoutEvent::WindowRemoved(window));
        }
        for &window in [w[1], w[2], w[0]].iter() {
            setup.event(LayoutEvent::WindowAdded(setup.space, window));
        }
        assert_eq!(setup.frames(), before);
    }

    #[test]
    fn a_window_comes_back_after_its_neighbor_when_the_layout_changed() {
        let w = windows(4);
        let mut setup = Setup::new(LayoutMode::Traditional, &w[..3]);
        let before = setup.order();
        let after = before[before.iter().position(|&window| window == w[1]).unwrap() - 1];
        setup.event(LayoutEvent::WindowRemoved(w[1]));
        // Open and select another window while it is away.
        setup.observe(&[w[0], w[2], w[3]]);
        setup.focus(w[3]);
        setup.event(LayoutEvent::WindowAdded(setup.space, w[1]));
        let order = setup.order();
        let at = |window| order.iter().position(|&w| w == window).unwrap();
        assert_eq!(at(w[1]), at(after) + 1, "{order:?}");
    }

    #[test]
    fn a_returning_window_asks_for_a_layout_so_the_others_make_room() {
        let w = windows(3);
        let mut setup = Setup::new(LayoutMode::Bsp, &w);
        let space = setup.space;
        let _ = setup.store.remove_window_assignment(w[0]);
        setup.event(LayoutEvent::WindowRemoved(w[0]));
        let outcome = setup
            .engine
            .handle_event(&mut setup.store, LayoutEvent::WindowAdded(space, w[0]));
        assert!(outcome.response.changed);
    }

    #[test]
    fn a_window_returning_to_a_workspace_not_shown_asks_for_a_layout() {
        let w = windows(2);
        let mut setup = Setup::new(LayoutMode::Bsp, &w);
        let space = setup.space;
        let left = setup.store.workspace_for_window(space, w[0]).unwrap();
        let _ = setup.store.remove_window_assignment(w[0]);
        setup.event(LayoutEvent::WindowRemoved(w[0]));
        let ids = setup.engine.workspaces.workspace_ids(space);
        let other = *ids.iter().find(|&&id| id != left).unwrap();
        setup.engine.workspaces.set_active_workspace(space, other);
        let outcome = setup
            .engine
            .handle_event(&mut setup.store, LayoutEvent::WindowAdded(space, w[0]));
        // The layout pass that follows moves the window off screen.
        assert!(outcome.response.changed);
    }

    #[test]
    fn a_window_without_its_workspace_comes_back_to_the_one_it_left() {
        let w = windows(2);
        let mut setup = Setup::new(LayoutMode::Bsp, &w);
        let space = setup.space;
        let left = setup.store.workspace_for_window(space, w[0]).unwrap();
        // Minimizing clears the store's assignment before the removal.
        let _ = setup.store.remove_window_assignment(w[0]);
        setup.event(LayoutEvent::WindowRemoved(w[0]));
        let ids = setup.engine.workspaces.workspace_ids(space);
        let other = *ids.iter().find(|&&id| id != left).unwrap();
        setup.engine.workspaces.set_active_workspace(space, other);
        setup.event(LayoutEvent::WindowAdded(space, w[0]));
        assert_eq!(setup.store.workspace_for_window(space, w[0]), Some(left));
    }
}
