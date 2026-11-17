use nix::libc::pid_t;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::{EditResult, Focus, FocusResult, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::common::config::{
    MasterStackNewWindowPlacement, MasterStackSettings, MasterStackSide, WindowInsertionPoint,
};
use crate::layout_engine::systems::{WindowLayoutConstraints, reconcile_app_membership};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{
    Direction, LayoutId, LayoutKind, LayoutSystem, Orientation, TraditionalLayoutSystem,
};
use crate::model::tree::NodeId;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(from = "StoredMasterStack")]
pub struct MasterStackLayoutSystem {
    inner: TraditionalLayoutSystem,
    regions: slotmap::SecondaryMap<LayoutId, Regions>,
    #[serde(skip, default = "default_master_stack_settings")]
    pub(super) settings: MasterStackSettings,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
struct Regions {
    master: NodeId,
    stack: NodeId,
}

#[derive(Deserialize)]
struct StoredMasterStack {
    inner: TraditionalLayoutSystem,
    #[serde(default)]
    regions: slotmap::SecondaryMap<LayoutId, Regions>,
}

impl From<StoredMasterStack> for MasterStackLayoutSystem {
    fn from(stored: StoredMasterStack) -> Self {
        let mut system = Self {
            inner: stored.inner,
            regions: stored.regions,
            settings: default_master_stack_settings(),
        };
        let layouts: Vec<_> = system.inner.layout_roots.keys().collect();
        for layout in layouts {
            let selected = system.inner.selected_window(layout);
            let mut reparented = false;
            if !system.regions.contains_key(layout) {
                // Historical layouts may have lost an empty region through tree cleanup.
                // Adopt their existing containers where possible; migrate only once.
                let root = system.inner.root(layout);
                let children: Vec<_> = root.children(system.inner.map()).collect();
                if children.len() == 2
                    && children.iter().all(|&n| system.inner.window_at(n).is_none())
                {
                    system.regions.insert(layout, Regions {
                        master: children[0],
                        stack: children[1],
                    });
                } else {
                    reparented = true;
                    let (master, stack) = system.create_containers(root);
                    system.regions.insert(layout, Regions { master, stack });
                    let mut count = 0;
                    for child in children {
                        let n = system.windows_in_container(child).count();
                        let target = if count < system.settings.master_count {
                            master
                        } else {
                            stack
                        };
                        child.detach(&mut system.inner.tree).push_back(target);
                        count += n;
                    }
                    system.apply_master_ratio(master, stack);
                }
            }
            let (_, master, stack) = system.region_roots(layout);
            system.inner.tree.data.layout.info[master].fixed_container = true;
            system.inner.tree.data.layout.info[stack].fixed_container = true;
            if reparented && let Some(window) = selected {
                system.inner.focus(layout, Focus::Select(window)).handled;
            }
        }
        system
    }
}

fn default_master_stack_settings() -> MasterStackSettings { MasterStackSettings::default() }

impl Default for MasterStackLayoutSystem {
    fn default() -> Self { Self::new(MasterStackSettings::default()) }
}

impl MasterStackLayoutSystem {
    pub fn new(settings: MasterStackSettings) -> Self {
        Self {
            inner: TraditionalLayoutSystem::default(),
            regions: Default::default(),
            settings,
        }
    }

    pub fn update_settings(&mut self, settings: MasterStackSettings) {
        if self.settings == settings {
            return;
        }
        let mut old_with_new_base = self.settings.clone();
        old_with_new_base.base = settings.base.clone();
        if old_with_new_base == settings {
            self.settings = settings;
            return;
        }
        let old_ratio = self.settings.master_ratio;
        self.settings = settings;
        let layouts: Vec<_> = self.inner.layout_roots.keys().collect();
        for layout in layouts {
            let (root, master, stack) = self.region_roots(layout);
            let first = if self.master_first() { master } else { stack };
            if root.first_child(self.inner.map()) != Some(first) {
                let other = if first == master { stack } else { master };
                first.reorder(other, true, &mut self.inner.tree.map);
            }
            self.inner.set_layout(root, LayoutKind::from(self.root_orientation()));
            self.inner.set_layout(master, LayoutKind::from(self.master_orientation()));
            self.inner.set_layout(stack, LayoutKind::from(self.stack_orientation()));
            if old_ratio != self.settings.master_ratio {
                self.apply_master_ratio(master, stack);
            }
            self.enforce_master_count(layout, master, stack, None);
        }
    }

    fn root_orientation(&self) -> Orientation {
        match self.settings.master_side {
            MasterStackSide::Left | MasterStackSide::Right => Orientation::Horizontal,
            MasterStackSide::Top | MasterStackSide::Bottom => Orientation::Vertical,
        }
    }

    fn container_orientation(&self) -> Orientation {
        match self.root_orientation() {
            Orientation::Horizontal => Orientation::Vertical,
            Orientation::Vertical => Orientation::Horizontal,
        }
    }

    fn master_orientation(&self) -> Orientation {
        self.settings.master_arrangement.unwrap_or_else(|| self.container_orientation())
    }

    fn stack_orientation(&self) -> Orientation {
        self.settings.stack_arrangement.unwrap_or_else(|| self.container_orientation())
    }

    fn master_first(&self) -> bool {
        matches!(
            self.settings.master_side,
            MasterStackSide::Left | MasterStackSide::Top
        )
    }

    fn windows_in_layout_by_container(&self, layout: LayoutId) -> Vec<WindowId> {
        let (_, master, stack) = self.region_roots(layout);
        self.windows_in_container(master)
            .chain(self.windows_in_container(stack))
            .collect()
    }

    fn windows_in_container(&self, container: NodeId) -> impl Iterator<Item = WindowId> + '_ {
        container
            .traverse_preorder(self.inner.map())
            .filter_map(|node| self.inner.window_at(node))
    }

    fn focused_container(&self, layout: LayoutId, master: NodeId, stack: NodeId) -> Option<NodeId> {
        let wid = self.inner.selected_window(layout)?;
        let node = self.inner.tree.data.window.node_for(layout, wid)?;
        let map = self.inner.map();
        if node.ancestors(map).any(|ancestor| ancestor == master) {
            Some(master)
        } else if node.ancestors(map).any(|ancestor| ancestor == stack) {
            Some(stack)
        } else {
            None
        }
    }

    fn target_container_for_new_window(
        &self,
        layout: LayoutId,
        master: NodeId,
        stack: NodeId,
    ) -> NodeId {
        if self.windows_in_container(master).count() < self.settings.master_count {
            return master;
        }

        match self.settings.new_window_placement {
            MasterStackNewWindowPlacement::Master => master,
            MasterStackNewWindowPlacement::Stack => stack,
            MasterStackNewWindowPlacement::Focused => {
                self.focused_container(layout, master, stack).unwrap_or(master)
            }
        }
    }

    fn focused_window_in_container(&self, container: NodeId) -> Option<WindowId> {
        let map = self.inner.map();
        let selection = self.inner.local_selection(container);
        let candidate = selection.or_else(|| container.first_child(map));
        let candidate = candidate?;
        candidate.traverse_preorder(map).find_map(|node| self.inner.window_at(node))
    }

    fn create_containers(&mut self, root: NodeId) -> (NodeId, NodeId) {
        self.inner.set_layout(root, LayoutKind::from(self.root_orientation()));
        let first = self.inner.tree.mk_node().push_back(root);
        let second = self.inner.tree.mk_node().push_back(root);
        self.inner.tree.data.layout.info[first].fixed_container = true;
        self.inner.tree.data.layout.info[second].fixed_container = true;
        let (master, stack) = if self.master_first() {
            (first, second)
        } else {
            (second, first)
        };
        self.inner.set_layout(master, LayoutKind::from(self.master_orientation()));
        self.inner.set_layout(stack, LayoutKind::from(self.stack_orientation()));
        (master, stack)
    }

    fn apply_master_ratio(&mut self, master: NodeId, stack: NodeId) {
        let ratio = self.settings.master_ratio.clamp(0.05, 0.95) as f32;
        let total = 2.0_f32;
        let master_size = (ratio * total).max(0.05);
        let stack_size = (total - master_size).max(0.05);
        self.inner.tree.data.layout.info[master].size = master_size;
        self.inner.tree.data.layout.info[stack].size = stack_size;
    }

    fn region_roots(&self, layout: LayoutId) -> (NodeId, NodeId, NodeId) {
        let Regions { master, stack } = self.regions[layout];
        (self.inner.root(layout), master, stack)
    }

    fn enforce_master_count(
        &mut self,
        layout: LayoutId,
        master: NodeId,
        stack: NodeId,
        keep_in_master: Option<WindowId>,
    ) {
        let current = self.windows_in_container(master).count();
        let desired = self.settings.master_count;
        if current == desired {
            return;
        }
        let overflow = current > desired;
        let count = current.abs_diff(desired);
        let source = if overflow { master } else { stack };
        let destination = if overflow { stack } else { master };
        let mut moving: Vec<_> = self.windows_in_container(source).collect();
        if overflow {
            if desired > 0 {
                moving.retain(|wid| Some(*wid) != keep_in_master);
            }
            moving.reverse();
        } else if !self.master_first() {
            moving.reverse();
        }
        moving.truncate(count);
        let selected = self.inner.selected_window(layout);
        let front = overflow == self.master_first();
        for wid in moving {
            if let Some(node) = self.move_window_to_container(layout, wid, destination, front)
                && Some(wid) == selected
            {
                self.inner.select(node);
            }
        }
    }

    fn move_window_to_container(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        container: NodeId,
        front: bool,
    ) -> Option<NodeId> {
        let node = self.inner.window_node(layout, wid)?;
        let anchor = front.then(|| container.first_child(self.inner.map())).flatten();
        if anchor == Some(node) || (!front && node.parent(self.inner.map()) == Some(container)) {
            return Some(node);
        }
        Some(if let Some(anchor) = anchor {
            node.detach(&mut self.inner.tree).insert_before(anchor).finish()
        } else {
            node.detach(&mut self.inner.tree).push_back(container).finish()
        })
    }

    fn move_window_to_end_of_current_container(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
    ) -> Option<NodeId> {
        let node = self.inner.tree.data.window.node_for(layout, wid)?;
        let parent = node.parent(self.inner.map())?;
        Some(node.detach(&mut self.inner.tree).push_back(parent).finish())
    }

    fn insert_window_in_container(
        &mut self,
        layout: LayoutId,
        container: NodeId,
        wid: WindowId,
    ) -> NodeId {
        debug_assert!(self.inner.map().contains(container));
        let first_child = container.children(self.inner.map()).next();
        let node = match self.settings.base.window_insertion_point {
            Some(WindowInsertionPoint::NextToSelection) => {
                let selection = self.inner.selection(layout);
                if selection.parent(self.inner.map()) == Some(container) {
                    self.inner.tree.mk_node().insert_after(selection)
                } else {
                    self.inner.tree.mk_node().push_back(container)
                }
            }
            Some(WindowInsertionPoint::EndOfTree) => self.inner.tree.mk_node().push_back(container),
            // Preserve the pre-base-settings behavior for old snapshots and direct
            // construction. Config-created systems always receive a resolved value.
            None => match first_child {
                Some(first_child) => self.inner.tree.mk_node().insert_before(first_child),
                None => self.inner.tree.mk_node().push_back(container),
            },
        };
        self.inner.tree.data.window.set_window(layout, node, wid);
        node
    }

    fn rebalance_after_new_window(
        &mut self,
        layout: LayoutId,
        master: NodeId,
        stack: NodeId,
        target: NodeId,
        wid: WindowId,
    ) {
        match self.settings.base.window_insertion_point {
            Some(WindowInsertionPoint::NextToSelection) => {
                let keep_in_master = (target == master).then_some(wid);
                self.enforce_master_count(layout, master, stack, keep_in_master);
            }
            Some(WindowInsertionPoint::EndOfTree) => {
                // EndOfTree is layout-wide, so normalize the areas first and then
                // put the new window at the end of whichever area it landed in.
                self.enforce_master_count(layout, master, stack, None);
                if let Some(node) = self.move_window_to_end_of_current_container(layout, wid) {
                    self.inner.select(node);
                }
            }
            None => self.enforce_master_count(layout, master, stack, None),
        }
    }

    fn distribute_regions(&mut self, layout: LayoutId) {
        let (_root, master, stack) = self.region_roots(layout);
        self.enforce_master_count(layout, master, stack, None);
    }

    pub fn adjust_master_ratio(&mut self, _layout: LayoutId, delta: f64) {
        let next = (self.settings.master_ratio + delta).clamp(0.05, 0.95);
        if (next - self.settings.master_ratio).abs() < f64::EPSILON {
            return;
        }
        self.settings.master_ratio = next;
        let layouts: Vec<_> = self.inner.layout_roots.keys().collect();
        for layout in layouts {
            self.distribute_regions(layout);
            let (_, master, stack) = self.region_roots(layout);
            self.apply_master_ratio(master, stack);
        }
    }

    pub fn adjust_master_count(&mut self, _layout: LayoutId, delta: i32) {
        let current = self.settings.master_count as i32;
        let next = (current + delta).max(1) as usize;
        if next == self.settings.master_count {
            return;
        }
        self.settings.master_count = next;
        let layouts: Vec<_> = self.inner.layout_roots.keys().collect();
        for layout in layouts {
            self.distribute_regions(layout);
        }
    }

    pub fn promote_to_master(&mut self, layout: LayoutId) {
        let (_root, master, stack) = self.region_roots(layout);
        let Some(focused_wid) = self.inner.selected_window(layout) else {
            return;
        };
        let Some(node) = self.inner.window_node(layout, focused_wid) else {
            return;
        };
        if node.parent(self.inner.map()) != Some(master)
            && let Some(overflow) = master.last_child(self.inner.map())
            && let Some(overflow_wid) = self.inner.window_at(overflow)
        {
            // Swap membership before reordering so moving the last stack window
            // cannot remove the stack container while it is still needed.
            self.inner.edit(layout, LayoutEdit::Swap(focused_wid, overflow_wid)).changed();
            self.move_window_to_container(layout, overflow_wid, stack, true);
        }
        // The focused window may already be first; inserting a node before
        // itself would unlink it before resolving the insertion anchor.
        if let Some(node) = self.move_window_to_container(layout, focused_wid, master, true) {
            self.inner.select(node);
        }
    }

    pub fn swap_master_stack(&mut self, layout: LayoutId) {
        let (_root, master, stack) = self.region_roots(layout);
        let (Some(master_wid), Some(stack_wid)) = (
            self.focused_window_in_container(master),
            self.focused_window_in_container(stack),
        ) else {
            return;
        };
        let _ = self.inner.edit(layout, LayoutEdit::Swap(master_wid, stack_wid)).changed();
    }

    pub(crate) fn collect_group_containers(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
        selection_path_only: bool,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        self.inner.collect_group_containers(
            layout,
            screen,
            stack_offset,
            gaps,
            stack_line,
            selection_path_only,
        )
    }
}

impl LayoutSystem for MasterStackLayoutSystem {
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.inner.has_windows_for_app(layout, pid)
    }

    fn window_participates(&self, window: WindowId) -> bool {
        self.inner.window_participates(window)
    }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        self.inner.set_window_participation(window, participates)
    }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.inner.contains_layout(layout) }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.inner.selected_window(layout)
    }

    fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        self.inner.selection_contains_window(layout, window)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.inner.contains_window(layout, wid)
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.inner.has_any_fullscreen_node(layout)
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        self.regions.remove(layout);
        self.inner.remove_layout(layout);
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.inner.window_in_direction(layout, direction)
    }

    fn create_layout(&mut self) -> LayoutId {
        let layout = self.inner.create_layout();
        let root = self.inner.root(layout);
        let (master, stack) = self.create_containers(root);
        self.apply_master_ratio(master, stack);
        self.regions.insert(layout, Regions { master, stack });
        layout
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let (_, source_master, _) = self.region_roots(layout);
        let master_first =
            self.inner.root(layout).first_child(self.inner.map()) == Some(source_master);
        let cloned = self.inner.clone_layout(layout);
        let root = self.inner.root(cloned);
        let first = root.first_child(self.inner.map()).unwrap();
        let second = root.last_child(self.inner.map()).unwrap();
        let (master, stack) = if master_first {
            (first, second)
        } else {
            (second, first)
        };
        self.regions.insert(cloned, Regions { master, stack });
        cloned
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        constraints: &crate::common::collections::HashMap<WindowId, WindowLayoutConstraints>,
        gaps: &crate::common::config::GapSettings,
        stack_line: crate::common::config::StackLineReserve,
    ) -> Vec<(WindowId, CGRect)> {
        let (_, master, stack) = self.region_roots(layout);
        if !stack
            .traverse_preorder(self.inner.map())
            .any(|node| self.inner.window_at(node).is_some_and(|w| self.window_participates(w)))
        {
            return self.inner.calculate_layout_for_node(
                master,
                screen,
                compute_tiling_area(screen, gaps),
                stack_offset,
                constraints,
                gaps,
                stack_line,
            );
        }
        self.inner
            .calculate_layout(layout, screen, stack_offset, constraints, gaps, stack_line)
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.inner.parent_of_selection_is_stacked(layout)
    }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        self.inner.windows(layout, query)
    }

    fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        self.inner.focus(layout, focus)
    }

    fn update(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Add(layout, window, _) => self.add_window_after_selection(layout, window),
            WindowChange::Remove(window) | WindowChange::RemoveAndRebalance(window) => {
                let wid = window;

                let layouts = self.inner.layouts_for_window(wid);
                self.inner.update(WindowChange::Remove(wid));
                for layout in layouts {
                    self.distribute_regions(layout);
                }
            }
            WindowChange::Replace(from, to) => {
                self.inner.update(WindowChange::Replace(from, to));
            }
            WindowChange::CloseApp(pid) => {
                let layouts: Vec<_> = self
                    .inner
                    .layout_roots
                    .keys()
                    .filter(|&layout| self.inner.has_windows_for_app(layout, pid))
                    .collect();
                self.inner.update(WindowChange::CloseApp(pid));
                for layout in layouts {
                    self.distribute_regions(layout);
                }
            }
            WindowChange::Reconcile(layout, pid, desired) => {
                let (_root, master, stack) = self.region_roots(layout);
                let root = self.inner.root(layout);
                let current = root
                    .traverse_postorder(self.inner.map())
                    .filter_map(|node| self.inner.window_at(node))
                    .filter(|wid| wid.pid == pid)
                    .collect::<Vec<_>>();
                let delta = reconcile_app_membership(pid, current, desired, |w| {
                    self.window_participates(w)
                });
                for wid in delta.removals {
                    if let Some(node) = self.inner.tree.data.window.node_for(layout, wid)
                        && !self.inner.tree.data.layout.info[node].is_fullscreen
                    {
                        node.detach(&mut self.inner.tree).remove();
                    }
                }
                for wid in delta.additions {
                    self.add_window_after_selection(layout, wid);
                }
                self.enforce_master_count(layout, master, stack, None);
            }
            WindowChange::Resize {
                layout,
                window,
                old,
                new,
                screen,
                gaps,
            } => {
                let wid = window;

                self.inner.update(WindowChange::Resize {
                    layout,
                    window: wid,
                    old,
                    new,
                    screen,
                    gaps,
                });
            }
        }
    }

    fn edit_layout(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult {
        match edit {
            LayoutEdit::Move {
                direction,
                has_display_neighbor: _,
            } => EditResult::from(self.move_selection(layout, direction)),
            LayoutEdit::Transfer(to) => {
                let from_layout = layout;
                let to_layout = to;

                if from_layout == to_layout {
                    return EditResult::Unchanged;
                }
                let (root, master, stack) = self.region_roots(from_layout);
                let selected = self.inner.selection(from_layout);
                if selected == root {
                    return EditResult::Unchanged;
                }
                let moving: Vec<_> = if selected == master || selected == stack {
                    selected.children(self.inner.map()).collect()
                } else {
                    vec![selected]
                };
                let (target_root, target_master, target_stack) = self.region_roots(to_layout);
                let target = self.inner.selection(to_layout);
                let container = if target == target_root {
                    self.target_container_for_new_window(to_layout, target_master, target_stack)
                } else if target == target_master || target == target_stack {
                    target
                } else {
                    target.parent(self.inner.map()).unwrap()
                };
                let mut anchor =
                    (target != target_root && target != target_master && target != target_stack)
                        .then_some(target);
                for node in moving {
                    if let Some(anchor) = anchor {
                        node.detach(&mut self.inner.tree).insert_after(anchor);
                    } else {
                        node.detach(&mut self.inner.tree).push_back(container);
                    }
                    self.inner.tree.data.window.reassign_subtree(
                        &self.inner.tree.map,
                        node,
                        to_layout,
                    );
                    anchor = Some(node);
                    self.inner.select(node);
                }
                self.distribute_regions(from_layout);
                self.distribute_regions(to_layout);
                EditResult::Changed
            }

            LayoutEdit::Fullscreen { .. } => self.inner.edit(layout, edit),
            LayoutEdit::Swap(a, b) => self.inner.edit(layout, LayoutEdit::Swap(a, b)),
            LayoutEdit::ReorderTab { window, target } => {
                self.inner.edit(layout, LayoutEdit::ReorderTab { window, target })
            }
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(self.apply_target_drop(layout, source, target, action))
            }

            LayoutEdit::Resize { amount, orientation } => {
                if self.windows(layout, WindowQuery::Visible).is_empty() {
                    return EditResult::Unchanged;
                }
                self.inner.edit(layout, LayoutEdit::Resize { amount, orientation }).discard();
                EditResult::Changed
            }

            LayoutEdit::Split(_)
            | LayoutEdit::Group(_)
            | LayoutEdit::Rebalance
            | LayoutEdit::ToggleOrientation => {
                self.distribute_regions(layout);
                EditResult::Changed
            }
        }
    }
}

impl MasterStackLayoutSystem {
    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let (_root, master, stack) = self.region_roots(layout);
        let target = self.target_container_for_new_window(layout, master, stack);
        let node = self.insert_window_in_container(layout, target, wid);
        self.inner.select(node);
        self.rebalance_after_new_window(layout, master, stack, target, wid);
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        if self.windows(layout, WindowQuery::Visible).is_empty() {
            return false;
        }
        let (_root, _master, _stack) = self.region_roots(layout);
        let Some(focused_wid) = self.inner.selected_window(layout) else {
            return false;
        };
        let windows = self.windows_in_layout_by_container(layout);
        let Some(focused_idx) = windows.iter().position(|&window| window == focused_wid) else {
            return false;
        };
        let master_count = self.settings.master_count;
        let in_master = focused_idx < master_count;
        let axis = if in_master {
            self.master_orientation()
        } else {
            self.stack_orientation()
        };
        let (towards_master, towards_stack) = match self.settings.master_side {
            MasterStackSide::Left => (direction == Direction::Left, direction == Direction::Right),
            MasterStackSide::Right => (direction == Direction::Right, direction == Direction::Left),
            MasterStackSide::Top => (direction == Direction::Up, direction == Direction::Down),
            MasterStackSide::Bottom => (direction == Direction::Down, direction == Direction::Up),
        };
        let master_first = self.master_first();
        let parallel = direction.orientation() == axis;
        let crossing = if towards_master && !in_master {
            let border = if master_first {
                master_count
            } else {
                windows.len() - 1
            };
            (!parallel || focused_idx == border)
                .then(|| if master_first { master_count - 1 } else { 0 })
        } else if towards_stack && in_master {
            let border = if master_first { master_count - 1 } else { 0 };
            (!parallel || focused_idx == border).then(|| {
                if windows.len() > master_count {
                    if master_first {
                        master_count
                    } else {
                        windows.len() - 1
                    }
                } else {
                    // The old reorder's first mismatch is the next master member.
                    (focused_idx + 1).min(windows.len() - 1)
                }
            })
        } else {
            None
        };
        let target_idx = if let Some(crossing) = crossing {
            crossing
        } else if !parallel {
            return false;
        } else {
            let neighbor = match direction {
                Direction::Left | Direction::Up => (focused_idx
                    > if in_master { 0 } else { master_count })
                .then_some(focused_idx.saturating_sub(1)),
                Direction::Right | Direction::Down => (focused_idx + 1
                    < if in_master {
                        master_count
                    } else {
                        windows.len()
                    })
                .then_some(focused_idx + 1),
            };
            let Some(neighbor) = neighbor else { return false };
            neighbor
        };
        let target_in_master = target_idx < master_count;
        let participates = |index: &usize| {
            (*index < master_count) == target_in_master
                && self.inner.window_participates(windows[*index])
        };
        let target_idx = if target_idx < focused_idx {
            (0..=target_idx).rev().find(participates)
        } else {
            (target_idx..windows.len()).find(participates)
        };
        let Some(target_idx) = target_idx else { return false };
        if target_idx != focused_idx
            && !self
                .inner
                .edit(layout, LayoutEdit::Swap(focused_wid, windows[target_idx]))
                .changed()
        {
            return false;
        }
        let _ = self.inner.focus(layout, Focus::Select(focused_wid)).handled;
        true
    }

    fn apply_target_drop(
        &mut self,
        layout: LayoutId,
        source: WindowId,
        target: WindowId,
        action: crate::layout_engine::WindowDropAction,
    ) -> bool {
        if !self.window_participates(source) || !self.window_participates(target) {
            return false;
        }
        self.inner.apply_explicit_window_drop(layout, source, target, action)
    }
}

impl MasterStackLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        let (_, master, stack) = self.region_roots(layout);
        self.inner.draw_tree_with_labels(
            layout,
            &HashMap::from_iter([(master, "master"), (stack, "stack")]),
        )
    }

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        let (_, master, stack) = self.region_roots(layout);
        self.inner.container_tree_with_roles(
            layout,
            &HashMap::from_iter([(master, "master"), (stack, "stack")]),
        )
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        self.inner.window_slot(layout, window)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::config::{LayoutMode, LayoutSettings};
    use crate::layout_engine::ResizeOrientation;

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn region_identity_and_grouping_survive_removal_and_settings_reload() {
        use crate::layout_engine::WindowDropAction;
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3)] {
            system.add_window_after_selection(layout, window);
        }
        assert!(
            system
                .edit(layout, LayoutEdit::Drop {
                    source: w(3),
                    target: w(2),
                    action: WindowDropAction::Stack
                })
                .changed()
        );
        system.focus(layout, Focus::Select(w(1))).handled;
        system
            .edit(layout, LayoutEdit::Resize {
                amount: 0.1,
                orientation: ResizeOrientation::Horizontal,
            })
            .discard();
        let before = system.container_tree(layout);
        let master_weight = before.children[0].weight;
        let master = before
            .children
            .iter()
            .find(|node| node.role.as_deref() == Some("master"))
            .unwrap()
            .node_id;
        let stack = before
            .children
            .iter()
            .find(|node| node.role.as_deref() == Some("stack"))
            .unwrap()
            .node_id;
        let group =
            before.children.iter().find(|node| node.node_id == stack).unwrap().children[0].node_id;
        let mut settings = system.settings.clone();
        settings.master_side = MasterStackSide::Right;
        system.update_settings(settings);
        let after = system.container_tree(layout);
        assert_eq!(after.children[0].node_id, stack);
        assert_eq!(after.children[1].node_id, master);
        assert_eq!(after.children[1].weight, master_weight);
        assert!(after.children[1].children[0].is_selected);
        assert_eq!(after.children[0].children[0].node_id, group);
        system.update(WindowChange::Remove(w(3)));
        system.update(WindowChange::Remove(w(2)));
        let after = system.container_tree(layout);
        assert_eq!(after.children[0].node_id, stack);
        assert_eq!(after.children[1].node_id, master);
        let serialized = ron::to_string(&system).unwrap();
        let restored: MasterStackLayoutSystem = ron::from_str(&serialized).unwrap();
        assert_eq!(restored.container_tree(layout), after);
    }

    #[test]
    fn transferring_a_region_preserves_regions_and_updates_membership_indexes() {
        let mut system = MasterStackLayoutSystem::default();
        let from = system.create_layout();
        let to = system.create_layout();
        system.add_window_after_selection(from, w(1));
        system.add_window_after_selection(from, w(2));
        let before = system.container_tree(from);
        assert!(system.focus(from, Focus::Ascend).handled);
        system.edit(from, LayoutEdit::Transfer(to)).discard();
        assert!(system.contains_window(to, w(2)));
        assert!(!system.contains_window(from, w(2)));
        assert_eq!(
            system.container_tree(from).children[1].node_id,
            before.children[1].node_id
        );
        system.update(WindowChange::Remove(w(2)));
        assert!(!system.contains_window(to, w(2)));
        assert_eq!(system.windows(from, WindowQuery::All), vec![w(1)]);
    }

    #[test]
    fn test_create_layout() {
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        let windows = system.windows_in_layout_by_container(layout);
        assert!(windows.is_empty());
    }

    #[test]
    fn test_add_windows() {
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));

        let windows = system.windows_in_layout_by_container(layout);
        // By default master_count = 1, new_window_placement = Master
        // When w1 is added: master=[w1]
        // When w2 is added: master=[w2], stack=[w1]
        // When w3 is added: master=[w3], stack=[w2, w1] (since w2 was at index 0 and got pushed to stack, w1 was pushed next)
        assert_eq!(windows, vec![w(3), w(2), w(1)]);
    }

    #[test]
    fn promotion_preserves_focus_and_is_repeatable() {
        for master_count in [1, 2] {
            let mut settings = MasterStackSettings::default();
            settings.master_count = master_count;
            let mut system = MasterStackLayoutSystem::new(settings);
            let layout = system.create_layout();
            system.add_window_after_selection(layout, w(1));
            system.add_window_after_selection(layout, w(2));
            system.add_window_after_selection(layout, w(3));

            for (focused, expected) in [
                (w(3), vec![w(3), w(2), w(1)]),
                (w(2), vec![w(2), w(3), w(1)]),
                (w(1), vec![w(1), w(2), w(3)]),
            ] {
                assert!(system.focus(layout, Focus::Select(focused)).handled);
                for _ in 0..2 {
                    system.promote_to_master(layout);
                    assert_eq!(system.windows_in_layout_by_container(layout), expected);
                    assert_eq!(system.selected_window(layout), Some(focused));
                }
            }
        }
    }

    #[test]
    fn next_to_selection_keeps_new_window_in_full_master_area() {
        let layout_settings: LayoutSettings = toml::from_str(
            r#"
                window_insertion_point = "next_to_selection"

                [master_stack]
                master_count = 1
                new_window_placement = "master"
            "#,
        )
        .unwrap();
        let mut settings = layout_settings.master_stack.clone();
        settings.base = layout_settings.resolved_base_for(LayoutMode::MasterStack);
        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));

        let (_root, master, stack) = system.region_roots(layout);
        assert_eq!(system.windows_in_container(master).collect::<Vec<_>>(), vec![w(
            2
        )]);
        assert_eq!(system.windows_in_container(stack).collect::<Vec<_>>(), vec![w(
            1
        )]);
    }

    #[test]
    fn next_to_selection_orders_within_the_selected_target_area() {
        let mut settings = MasterStackSettings::default();
        settings.base.window_insertion_point = Some(WindowInsertionPoint::NextToSelection);
        settings.master_count = 2;
        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        system.add_window_after_selection(layout, w(3));

        let (_root, master, stack) = system.region_roots(layout);
        assert_eq!(system.windows_in_container(master).collect::<Vec<_>>(), vec![
            w(1),
            w(3)
        ]);
        assert_eq!(system.windows_in_container(stack).collect::<Vec<_>>(), vec![w(
            2
        )]);
    }

    #[test]
    fn next_to_selection_respects_stack_placement() {
        let mut settings = MasterStackSettings::default();
        settings.base.window_insertion_point = Some(WindowInsertionPoint::NextToSelection);
        settings.new_window_placement = MasterStackNewWindowPlacement::Stack;
        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));

        let (_root, master, stack) = system.region_roots(layout);
        assert_eq!(system.windows_in_container(master).collect::<Vec<_>>(), vec![w(
            1
        )]);
        assert_eq!(system.windows_in_container(stack).collect::<Vec<_>>(), vec![w(
            2
        )]);
    }

    #[test]
    fn next_to_selection_respects_focused_placement() {
        let mut settings = MasterStackSettings::default();
        settings.base.window_insertion_point = Some(WindowInsertionPoint::NextToSelection);
        settings.new_window_placement = MasterStackNewWindowPlacement::Focused;
        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        system.add_window_after_selection(layout, w(3));

        let (_root, master, stack) = system.region_roots(layout);
        assert_eq!(system.windows_in_container(master).collect::<Vec<_>>(), vec![w(
            2
        )]);
        assert_eq!(system.windows_in_container(stack).collect::<Vec<_>>(), vec![
            w(1),
            w(3)
        ]);
    }

    #[test]
    fn end_of_tree_keeps_new_windows_after_master_overflow() {
        let mut settings = MasterStackSettings::default();
        settings.base.window_insertion_point = Some(WindowInsertionPoint::EndOfTree);
        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));

        assert_eq!(system.windows_in_layout_by_container(layout), vec![
            w(1),
            w(2),
            w(3)
        ]);
    }

    #[test]
    fn resize_selection_respects_the_requested_axis() {
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        let (_root, master, stack) = system.region_roots(layout);
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        let before = system.inner.tree.data.layout.info[stack].size;

        system
            .edit(layout, LayoutEdit::Resize {
                amount: 0.1,
                orientation: ResizeOrientation::Horizontal,
            })
            .discard();

        let after = system.inner.tree.data.layout.info[stack].size;
        assert_ne!(after, before);
        assert_ne!(master, stack);
    }

    #[test]
    fn master_keyboard_resize_survives_structure_checks() {
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        assert!(system.focus(layout, Focus::Select(w(1))).handled);

        let (_, master, _) = system.region_roots(layout);
        let original = system.inner.tree.data.layout.info[master].size;
        system
            .edit(layout, LayoutEdit::Resize {
                amount: 0.05,
                orientation: ResizeOrientation::Horizontal,
            })
            .discard();
        let resized = system.inner.tree.data.layout.info[master].size;
        assert_ne!(resized, original);

        system.region_roots(layout);
        assert_eq!(system.inner.tree.data.layout.info[master].size, resized);
        system
            .edit(layout, LayoutEdit::Resize {
                amount: 0.05,
                orientation: ResizeOrientation::Horizontal,
            })
            .discard();
        assert_ne!(system.inner.tree.data.layout.info[master].size, resized);
    }

    #[test]
    fn test_move_selection_towards_master_and_stack() {
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));
        // layout state: master=[w3], stack=[w2, w1]

        // Select w2 (which is in stack)
        assert!(system.focus(layout, Focus::Select(w(2))).handled);

        // Move towards master (Left) -> promotes w2 to master, pushes w3 to stack
        assert!(system.move_selection(layout, Direction::Left));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(2), w(3), w(1)]);

        // Select w2 (which is in master)
        assert!(system.focus(layout, Focus::Select(w(2))).handled);

        // Move towards stack (Right) -> demotes w2 to stack, top stack window (w3) becomes master
        assert!(system.move_selection(layout, Direction::Right));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(2), w(1)]);
    }

    #[test]
    fn moving_towards_empty_stack_swaps_with_next_master_member() {
        let mut settings = MasterStackSettings::default();
        settings.master_count = 3;
        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3)] {
            system.add_window_after_selection(layout, window);
        }
        let before = system.windows_in_layout_by_container(layout);
        assert!(system.focus(layout, Focus::Select(before[0])).handled);
        assert!(system.move_selection(layout, Direction::Right));
        assert_eq!(system.windows_in_layout_by_container(layout), vec![
            before[1], before[0], before[2]
        ]);
    }

    #[test]
    fn test_move_selection_within_container() {
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));
        // layout state: master=[w3], stack=[w2, w1]

        // Select w2 (which is at index 1, i.e., index 0 in stack)
        assert!(system.focus(layout, Focus::Select(w(2))).handled);

        // Move down (within stack) -> swaps w2 and w1
        assert!(system.move_selection(layout, Direction::Down));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(1), w(2)]);

        // Move up (within stack) -> swaps w1 and w2 back
        assert!(system.focus(layout, Focus::Select(w(2))).handled);
        assert!(system.move_selection(layout, Direction::Up));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(2), w(1)]);
        system.add_window_after_selection(layout, w(4));
        system.set_window_participation(w(2), false);
        assert!(system.focus(layout, Focus::Select(w(3))).handled);
        assert!(system.move_selection(layout, Direction::Down));
        assert_eq!(system.windows_in_layout_by_container(layout), vec![
            w(4),
            w(1),
            w(2),
            w(3)
        ]);
        system.set_window_participation(w(2), true);
        assert_eq!(system.windows_in_layout_by_container(layout), vec![
            w(4),
            w(1),
            w(2),
            w(3)
        ]);
    }

    #[test]
    fn test_parallel_horizontal_layout_reordering() {
        let mut settings = MasterStackSettings::default();
        settings.master_count = 2;
        settings.master_arrangement = Some(Orientation::Horizontal);
        settings.stack_arrangement = Some(Orientation::Horizontal);

        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));
        system.add_window_after_selection(layout, w(4));
        // State: master=[w4, w3], stack=[w2, w1] (all horizontal)
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(4), w(3), w(2), w(1)]);

        // 1. Focus w4 (index 0 in master) and move Right.
        // It is NOT at the border (which is w3 at index 1), so it should swap w4 and w3 within master.
        assert!(system.focus(layout, Focus::Select(w(4))).handled);
        assert!(system.move_selection(layout, Direction::Right));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(4), w(2), w(1)]);

        // 2. Focus w4 (now at index 1 in master, which is the border) and move Right.
        // It IS at the border, so it should cross to stack, demoting w4 and swapping with w2 (index 2).
        assert!(system.move_selection(layout, Direction::Right));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(2), w(4), w(1)]);

        // 3. Focus w4 (now at index 2 in stack, which is the border of stack facing master) and move Left.
        // It IS at the border of stack (index == master_count), so it should promote back to master, swapping with w2 (index 1).
        assert!(system.focus(layout, Focus::Select(w(4))).handled);
        assert!(system.move_selection(layout, Direction::Left));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(4), w(2), w(1)]);

        // 4. Focus w1 (at index 3, the rightmost stack window) and move Left.
        // It is NOT at the border of stack facing master (border is w2 at index 2), so it should swap w1 and w2 within stack.
        assert!(system.focus(layout, Focus::Select(w(1))).handled);
        assert!(system.move_selection(layout, Direction::Left));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(4), w(1), w(2)]);
    }

    #[test]
    fn test_parallel_horizontal_layout_reordering_stack_left() {
        let mut settings = MasterStackSettings::default();
        settings.master_side = MasterStackSide::Right; // Stack is on Left, Master is on Right
        settings.master_count = 2;
        settings.master_arrangement = Some(Orientation::Horizontal);
        settings.stack_arrangement = Some(Orientation::Horizontal);

        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        system.add_window_after_selection(layout, w(3));
        system.add_window_after_selection(layout, w(4));

        // Logical flat list: [M0, M1, S0, S1] -> [w4, w3, w1, w2] (since overflows append to stack)
        // Physical layout: w1 (leftmost stack) - w2 (rightmost stack) | w4 (leftmost master) - w3 (rightmost master)
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(4), w(3), w(1), w(2)]);

        // 1. Focus w3 (at index 1, the right-most Master window) and move Left.
        // It is NOT at the border facing stack (which is w4 at index 0), so it should swap w3 and w4 within Master.
        assert!(system.focus(layout, Focus::Select(w(3))).handled);
        assert!(system.move_selection(layout, Direction::Left));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(4), w(1), w(2)]);

        // 2. Focus w3 (now at index 0, which is the border facing stack) and move Left.
        // It IS at the border, so it should cross to stack, swapping with the right-most stack window w2 (index 3).
        assert!(system.move_selection(layout, Direction::Left));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(2), w(4), w(1), w(3)]);

        // 3. Focus w3 (now at index 3, the border of Stack facing Master) and move Right.
        // It IS at the border of Stack facing Master, so it should cross to Master, swapping with w2 (index 0).
        assert!(system.focus(layout, Focus::Select(w(3))).handled);
        assert!(system.move_selection(layout, Direction::Right));
        let windows = system.windows_in_layout_by_container(layout);
        assert_eq!(windows, vec![w(3), w(4), w(1), w(2)]);
    }

    #[test]
    fn removing_stack_window_preserves_master_membership_when_master_is_last() {
        for master_side in [MasterStackSide::Right, MasterStackSide::Bottom] {
            let mut settings = MasterStackSettings::default();
            settings.master_count = 2;
            settings.master_side = master_side;
            settings.new_window_placement = MasterStackNewWindowPlacement::Stack;
            let mut system = MasterStackLayoutSystem::new(settings);
            let layout = system.create_layout();

            system.add_window_after_selection(layout, w(1));
            system.add_window_after_selection(layout, w(2));
            system.add_window_after_selection(layout, w(3));
            system.add_window_after_selection(layout, w(4));

            let (_root, master, stack) = system.region_roots(layout);
            let master_before = system.windows_in_container(master).collect::<Vec<_>>();
            let stack_before = system.windows_in_container(stack).collect::<Vec<_>>();
            assert_eq!(master_before.len(), 2);
            assert_eq!(stack_before.len(), 2);
            assert!(stack_before.contains(&w(3)));

            system.update(WindowChange::Remove(w(3)));

            let (_root, master, stack) = system.region_roots(layout);
            assert_eq!(
                system.windows_in_container(master).collect::<Vec<_>>(),
                master_before
            );
            assert_eq!(
                system.windows_in_container(stack).collect::<Vec<_>>(),
                stack_before.into_iter().filter(|&window| window != w(3)).collect::<Vec<_>>()
            );
        }
    }
}
