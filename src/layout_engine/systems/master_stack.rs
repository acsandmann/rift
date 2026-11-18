use nix::libc::pid_t;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::tiles::TileLayouts;
use super::{EditResult, Focus, FocusResult, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::WindowId;
use crate::common::collections::HashMap;
use crate::common::config::{
    MasterStackNewWindowPlacement, MasterStackSettings, MasterStackSide, WindowInsertionPoint,
};
use crate::layout_engine::systems::WindowLayoutConstraints;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, LayoutSystem, Orientation};
use crate::model::tree::{NodeId, Placement};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(from = "StoredMasterStack")]
pub struct MasterStackLayoutSystem {
    #[serde(rename = "inner")]
    tiles: TileLayouts,
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
    #[serde(rename = "inner")]
    tiles: TileLayouts,
    #[serde(default)]
    regions: slotmap::SecondaryMap<LayoutId, Regions>,
}

impl From<StoredMasterStack> for MasterStackLayoutSystem {
    fn from(stored: StoredMasterStack) -> Self {
        let mut system = Self {
            tiles: stored.tiles,
            regions: stored.regions,
            settings: default_master_stack_settings(),
        };
        let layouts: Vec<_> = system.tiles.layout_roots.keys().collect();
        for layout in layouts {
            let selected = system.tiles.selected_window(layout);
            let mut reparented = false;
            if !system.regions.contains_key(layout) {
                // Historical layouts may have lost an empty region through tree cleanup.
                // Adopt their existing containers where possible; migrate only once.
                let root = system.tiles.root(layout);
                let children: Vec<_> = root.children(system.tiles.map()).collect();
                if children.len() == 2
                    && children.iter().all(|&n| system.tiles.window_at(n).is_none())
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
                        system.tiles.tree.move_node(child, Placement::Back(target));
                        count += n;
                    }
                    system.apply_master_ratio(master, stack);
                }
            }
            let (_, master, stack) = system.region_roots(layout);
            system.tiles.tree.map[master].layout.fixed_container = true;
            system.tiles.tree.map[stack].layout.fixed_container = true;
            if reparented && let Some(window) = selected {
                system.tiles.focus(layout, Focus::Select(window)).handled;
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
            tiles: TileLayouts::default(),
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
        let layouts: Vec<_> = self.tiles.layout_roots.keys().collect();
        for layout in layouts {
            let (root, master, stack) = self.region_roots(layout);
            let first = if self.master_first() { master } else { stack };
            if root.first_child(self.tiles.map()) != Some(first) {
                let other = if first == master { stack } else { master };
                first.reorder(other, true, &mut self.tiles.tree.map);
            }
            self.tiles.set_layout(root, LayoutKind::from(self.root_orientation()));
            self.tiles.set_layout(master, LayoutKind::from(self.master_orientation()));
            self.tiles.set_layout(stack, LayoutKind::from(self.stack_orientation()));
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

    fn windows_in_container(&self, container: NodeId) -> impl Iterator<Item = WindowId> + '_ {
        container
            .traverse_preorder(self.tiles.map())
            .filter_map(|node| self.tiles.window_at(node))
    }

    fn focused_container(&self, layout: LayoutId, master: NodeId, stack: NodeId) -> Option<NodeId> {
        let wid = self.tiles.selected_window(layout)?;
        let node = self.tiles.tree.window.node_for(layout, wid)?;
        let map = self.tiles.map();
        node.ancestors(map).find(|&ancestor| ancestor == master || ancestor == stack)
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
        let map = self.tiles.map();
        let selection = self.tiles.local_selection(container);
        let candidate = selection.or_else(|| container.first_child(map));
        let candidate = candidate?;
        candidate.traverse_preorder(map).find_map(|node| self.tiles.window_at(node))
    }

    fn create_containers(&mut self, root: NodeId) -> (NodeId, NodeId) {
        self.tiles.set_layout(root, LayoutKind::from(self.root_orientation()));
        let first = self.tiles.tree.insert(Placement::Back(root));
        let second = self.tiles.tree.insert(Placement::Back(root));
        self.tiles.tree.map[first].layout.fixed_container = true;
        self.tiles.tree.map[second].layout.fixed_container = true;
        let (master, stack) = if self.master_first() {
            (first, second)
        } else {
            (second, first)
        };
        self.tiles.set_layout(master, LayoutKind::from(self.master_orientation()));
        self.tiles.set_layout(stack, LayoutKind::from(self.stack_orientation()));
        (master, stack)
    }

    fn apply_master_ratio(&mut self, master: NodeId, stack: NodeId) {
        let ratio = self.settings.master_ratio.clamp(0.05, 0.95) as f32;
        let total = 2.0_f32;
        let master_size = (ratio * total).max(0.05);
        let stack_size = (total - master_size).max(0.05);
        self.tiles.tree.map[master].layout.size = master_size;
        self.tiles.tree.map[stack].layout.size = stack_size;
    }

    fn region_roots(&self, layout: LayoutId) -> (NodeId, NodeId, NodeId) {
        let Regions { master, stack } = self.regions[layout];
        (self.tiles.root(layout), master, stack)
    }

    fn enforce_master_count(
        &mut self,
        layout: LayoutId,
        master: NodeId,
        stack: NodeId,
        keep_in_master: Option<WindowId>,
    ) -> bool {
        let current = self.windows_in_container(master).count();
        let desired = self.settings.master_count;
        if current == desired {
            return false;
        }
        let overflow = current > desired;
        let count = current.abs_diff(desired);
        let source = if overflow { master } else { stack };
        let destination = if overflow { stack } else { master };
        let forward = !overflow && self.master_first();
        let start = if forward {
            source
        } else {
            source.last_descendant(self.tiles.map())
        };
        let moving: Vec<_> = self
            .tiles
            .windows_from(source, Some(start), forward)
            .filter(|&window| !overflow || desired == 0 || Some(window) != keep_in_master)
            .take(count)
            .collect();
        let changed = !moving.is_empty();
        let selected = self.tiles.selected_window(layout);
        let front = overflow == self.master_first();
        for wid in moving {
            if let Some(node) = self.move_window_to_container(layout, wid, destination, front)
                && Some(wid) == selected
            {
                self.tiles.select(node);
            }
        }
        changed
    }

    fn move_window_to_container(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        container: NodeId,
        front: bool,
    ) -> Option<NodeId> {
        let node = self.tiles.window_node(layout, wid)?;
        let anchor = front.then(|| container.first_child(self.tiles.map())).flatten();
        if anchor == Some(node) || (!front && node.parent(self.tiles.map()) == Some(container)) {
            return Some(node);
        }
        Some(if let Some(anchor) = anchor {
            self.tiles.tree.move_node(node, Placement::Before(anchor))
        } else {
            self.tiles.tree.move_node(node, Placement::Back(container))
        })
    }

    fn insert_window_in_container(
        &mut self,
        layout: LayoutId,
        container: NodeId,
        wid: WindowId,
    ) -> NodeId {
        debug_assert!(self.tiles.map().contains(container));
        let first_child = container.children(self.tiles.map()).next();
        let node = match self.settings.base.window_insertion_point {
            Some(WindowInsertionPoint::NextToSelection) => {
                let selection = self.tiles.selection(layout);
                if selection.parent(self.tiles.map()) == Some(container) {
                    self.tiles.tree.insert(Placement::After(selection))
                } else {
                    self.tiles.tree.insert(Placement::Back(container))
                }
            }
            Some(WindowInsertionPoint::EndOfTree) => {
                self.tiles.tree.insert(Placement::Back(container))
            }
            // Preserve the pre-base-settings behavior for old snapshots and direct
            // construction. Config-created systems always receive a resolved value.
            None => match first_child {
                Some(first_child) => self.tiles.tree.insert(Placement::Before(first_child)),
                None => self.tiles.tree.insert(Placement::Back(container)),
            },
        };
        self.tiles.tree.window.set_window(&mut self.tiles.tree.map, layout, node, wid);
        node
    }

    fn distribute_regions(&mut self, layout: LayoutId) -> bool {
        let Regions { master, stack } = self.regions[layout];
        self.enforce_master_count(layout, master, stack, None)
    }

    pub fn adjust_master_ratio(&mut self, _layout: LayoutId, delta: f64) {
        let next = (self.settings.master_ratio + delta).clamp(0.05, 0.95);
        if (next - self.settings.master_ratio).abs() < f64::EPSILON {
            return;
        }
        self.settings.master_ratio = next;
        let layouts: Vec<_> = self.tiles.layout_roots.keys().collect();
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
        let layouts: Vec<_> = self.tiles.layout_roots.keys().collect();
        for layout in layouts {
            self.distribute_regions(layout);
        }
    }

    pub fn promote_to_master(&mut self, layout: LayoutId) {
        let (_root, master, stack) = self.region_roots(layout);
        let Some(focused_wid) = self.tiles.selected_window(layout) else {
            return;
        };
        let Some(node) = self.tiles.window_node(layout, focused_wid) else {
            return;
        };
        if node.parent(self.tiles.map()) != Some(master)
            && let Some(overflow) = master.last_child(self.tiles.map())
            && let Some(overflow_wid) = self.tiles.window_at(overflow)
        {
            // Swap membership before reordering so moving the last stack window
            // cannot remove the stack container while it is still needed.
            self.tiles.swap_windows(layout, focused_wid, overflow_wid);
            self.move_window_to_container(layout, overflow_wid, stack, true);
        }
        // The focused window may already be first; inserting a node before
        // itself would unlink it before resolving the insertion anchor.
        if let Some(node) = self.move_window_to_container(layout, focused_wid, master, true) {
            self.tiles.select(node);
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
        let _ = self.tiles.swap_windows(layout, master_wid, stack_wid);
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
        self.tiles.collect_group_containers(
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
        self.tiles.has_windows_for_app(layout, pid)
    }

    fn window_participates(&self, window: WindowId) -> bool {
        self.tiles.window_participates(window)
    }

    fn set_window_participation(&mut self, window: WindowId, participates: bool) -> bool {
        self.tiles.set_window_participation(window, participates)
    }

    fn contains_layout(&self, layout: LayoutId) -> bool { self.tiles.contains_layout(layout) }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.tiles.selected_window(layout)
    }

    fn selection_contains_window(&self, layout: LayoutId, window: WindowId) -> bool {
        self.tiles.selection_contains_window(layout, window)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.tiles.contains_window(layout, wid)
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.tiles.has_any_fullscreen_node(layout)
    }

    fn remove_layout(&mut self, layout: LayoutId) {
        self.regions.remove(layout);
        self.tiles.remove_layout(layout);
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.tiles.window_in_direction(layout, direction)
    }

    fn create_layout(&mut self) -> LayoutId {
        let layout = self.tiles.create_layout();
        let root = self.tiles.root(layout);
        let (master, stack) = self.create_containers(root);
        self.apply_master_ratio(master, stack);
        self.regions.insert(layout, Regions { master, stack });
        layout
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let (_, source_master, _) = self.region_roots(layout);
        let master_first =
            self.tiles.root(layout).first_child(self.tiles.map()) == Some(source_master);
        let cloned = self.tiles.clone_layout(layout);
        let root = self.tiles.root(cloned);
        let first = root.first_child(self.tiles.map()).unwrap();
        let second = root.last_child(self.tiles.map()).unwrap();
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
        let (root, master, stack) = self.region_roots(layout);
        self.tiles.project(
            root,
            Some((master, stack)),
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
        )
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.tiles.parent_of_selection_is_stacked(layout)
    }

    fn windows(&self, layout: LayoutId, query: WindowQuery) -> Vec<WindowId> {
        self.tiles.windows(layout, query)
    }

    fn focus(&mut self, layout: LayoutId, focus: Focus) -> FocusResult {
        self.tiles.focus(layout, focus)
    }

    fn update(&mut self, change: WindowChange<'_>) {
        match change {
            WindowChange::Add(layout, window, _) => self.add_window_after_selection(layout, window),
            WindowChange::Remove(window) | WindowChange::RemoveAndRebalance(window) => {
                let layouts = self.tiles.layouts_for_window(window);
                self.tiles.update_existing(WindowChange::Remove(window));
                for layout in layouts {
                    self.distribute_regions(layout);
                }
            }
            WindowChange::CloseApp(pid) => {
                let layouts: Vec<_> = self
                    .tiles
                    .layout_roots
                    .keys()
                    .filter(|&layout| self.tiles.has_windows_for_app(layout, pid))
                    .collect();
                self.tiles.update_existing(WindowChange::CloseApp(pid));
                for layout in layouts {
                    self.distribute_regions(layout);
                }
            }
            WindowChange::Reconcile(layout, pid, desired) => {
                for wid in self.tiles.reconcile_windows(layout, pid, desired) {
                    self.add_window_after_selection(layout, wid);
                }
                self.distribute_regions(layout);
            }
            WindowChange::Replace(..) | WindowChange::Resize { .. } => {
                self.tiles.update_existing(change)
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
                if layout == to {
                    return EditResult::Unchanged;
                }
                let (root, master, stack) = self.region_roots(layout);
                let selected = self.tiles.selection(layout);
                if selected == root {
                    return EditResult::Unchanged;
                }
                if self
                    .tiles
                    .windows_from(selected, Some(selected), true)
                    .any(|window| self.tiles.contains_window(to, window))
                {
                    return EditResult::Unchanged;
                }
                let (target_root, target_master, target_stack) = self.region_roots(to);
                let target = self.tiles.selection(to);
                let (container, mut anchor) = if target == target_root {
                    (
                        self.target_container_for_new_window(to, target_master, target_stack),
                        None,
                    )
                } else if target == target_master || target == target_stack {
                    (target, None)
                } else {
                    (target.parent(self.tiles.map()).unwrap(), Some(target))
                };
                let source_is_region = selected == master || selected == stack;
                let mut moving = if source_is_region {
                    selected.first_child(self.tiles.map())
                } else {
                    Some(selected)
                };
                let moved = moving.is_some();
                while let Some(node) = moving {
                    if let Some(anchor) = anchor {
                        self.tiles.tree.move_node(node, Placement::After(anchor));
                    } else {
                        self.tiles.tree.move_node(node, Placement::Back(container));
                    }
                    self.tiles.tree.window.reassign_subtree(&self.tiles.tree.map, node, to);
                    anchor = Some(node);
                    self.tiles.select(node);
                    moving = if source_is_region {
                        selected.first_child(self.tiles.map())
                    } else {
                        None
                    };
                }
                let source_changed = self.distribute_regions(layout);
                let target_changed = self.distribute_regions(to);
                EditResult::from(moved || source_changed || target_changed)
            }

            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.tiles.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::Swap(a, b) => EditResult::from(self.tiles.swap_windows(layout, a, b)),
            LayoutEdit::ReorderTab { window, target } => {
                EditResult::from(self.tiles.move_stack_tab(layout, window, target))
            }
            LayoutEdit::Resize { amount, orientation } => {
                self.tiles.resize_selection(layout, amount, orientation)
            }
            LayoutEdit::Drop { source, target, action } => EditResult::from(
                self.window_participates(source)
                    && self.window_participates(target)
                    && self.tiles.apply_explicit_window_drop(layout, source, target, action),
            ),

            LayoutEdit::Split(_)
            | LayoutEdit::Group(_)
            | LayoutEdit::Rebalance
            | LayoutEdit::ToggleOrientation => EditResult::from(self.distribute_regions(layout)),
        }
    }
}

impl MasterStackLayoutSystem {
    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        if self.tiles.contains_window(layout, wid) {
            return;
        }
        let (_root, master, stack) = self.region_roots(layout);
        let target = self.target_container_for_new_window(layout, master, stack);
        let node = self.insert_window_in_container(layout, target, wid);
        self.tiles.select(node);

        let insertion = self.settings.base.window_insertion_point;
        let keep = (insertion == Some(WindowInsertionPoint::NextToSelection) && target == master)
            .then_some(wid);
        self.enforce_master_count(layout, master, stack, keep);
        if insertion == Some(WindowInsertionPoint::EndOfTree)
            && let Some(parent) = node.parent(self.tiles.map())
        {
            // This policy appends in the final region after master overflow is resolved.
            self.tiles.tree.move_node(node, Placement::Back(parent));
            self.tiles.select(node);
        }
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        if self.windows(layout, WindowQuery::Visible).is_empty() {
            return false;
        }
        let Some(focused) = self.tiles.selected_window(layout) else {
            return false;
        };
        let Some(node) = self.tiles.window_node(layout, focused) else {
            return false;
        };
        let (_, master, stack) = self.region_roots(layout);
        let map = self.tiles.map();
        let in_master = node.ancestors(map).any(|ancestor| ancestor == master);
        let (region, other, axis) = if in_master {
            (master, stack, self.master_orientation())
        } else {
            (stack, master, self.stack_orientation())
        };
        let forward = matches!(direction, Direction::Right | Direction::Down);
        let master_first = self.master_first();
        let parallel = direction.orientation() == axis;
        let toward_other = direction.orientation() == self.root_orientation()
            && (forward == master_first) == in_master;
        let start = if forward {
            node.next_in_subtree(region, map)
        } else {
            node.prev_in_subtree(region, map)
        };
        let mut neighbors = self.tiles.windows_from(region, start, forward).peekable();
        let participates = |window| self.tiles.window_participates(window);
        let target = if toward_other && (!parallel || neighbors.peek().is_none()) {
            // Match the region boundary even when its edge tab is suspended.
            let edge = if forward {
                other
            } else {
                other.last_descendant(map)
            };
            let mut destination = self.tiles.windows_from(other, Some(edge), forward).peekable();
            if destination.peek().is_some() {
                if master_first {
                    destination.find(|&window| participates(window))
                } else {
                    destination.next().filter(|&window| participates(window))
                }
            } else if in_master {
                // With no stack region, use the next master window if one exists.
                let mut next = self
                    .tiles
                    .windows_from(region, node.next_in_subtree(region, map), true)
                    .peekable();
                if next.peek().is_some() {
                    next.find(|&window| participates(window))
                } else {
                    Some(focused)
                }
            } else if parallel {
                neighbors.find(|&window| participates(window))
            } else {
                None
            }
        } else if parallel {
            neighbors.find(|&window| participates(window))
        } else {
            None
        };
        drop(neighbors);
        let Some(target) = target else { return false };
        if target != focused && !self.tiles.swap_windows(layout, focused, target) {
            return false;
        }
        self.tiles.focus(layout, Focus::Select(focused)).handled;
        true
    }
}

impl MasterStackLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        let (_, master, stack) = self.region_roots(layout);
        self.tiles.draw_tree_with_labels(
            layout,
            &HashMap::from_iter([(master, "master"), (stack, "stack")]),
        )
    }

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        let (_, master, stack) = self.region_roots(layout);
        self.tiles.container_tree_with_roles(
            layout,
            &HashMap::from_iter([(master, "master"), (stack, "stack")]),
        )
    }

    pub(crate) fn window_slot(&self, layout: LayoutId, window: WindowId) -> Option<Vec<usize>> {
        self.tiles.window_slot(layout, window)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
    fn keyboard_move_uses_the_region_created_by_a_stack_drop() {
        use crate::layout_engine::WindowDropAction;
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3)] {
            system.update(WindowChange::Add(layout, window, None));
        }
        assert!(
            system
                .edit(layout, LayoutEdit::Drop {
                    source: w(2),
                    target: w(3),
                    action: WindowDropAction::Stack,
                })
                .changed()
        );
        assert!(
            system
                .edit(layout, LayoutEdit::Move {
                    direction: Direction::Up,
                    has_display_neighbor: false,
                })
                .changed()
        );
        assert_eq!(system.windows(layout, WindowQuery::Stack(w(2))), vec![
            w(2),
            w(3)
        ]);
        assert_eq!(system.selected_window(layout), Some(w(2)));
    }
}
