use nix::libc::pid_t;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::groups::GroupLayouts;
use super::{EditResult, Focus, FocusResult, LayoutEdit, WindowChange, WindowQuery};
use crate::actor::app::WindowId;
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
    pub(super) tiles: GroupLayouts,
    #[serde(skip)]
    pub(super) settings: MasterStackSettings,
}

#[derive(Deserialize)]
struct Regions {
    master: NodeId,
    stack: NodeId,
}

#[derive(Deserialize)]
struct StoredMasterStack {
    #[serde(rename = "inner")]
    tiles: GroupLayouts,
    #[serde(default)]
    regions: slotmap::SecondaryMap<LayoutId, Regions>,
}

impl From<StoredMasterStack> for MasterStackLayoutSystem {
    fn from(stored: StoredMasterStack) -> Self {
        let mut system = Self {
            tiles: stored.tiles,
            settings: MasterStackSettings::default(),
        };
        let layouts: Vec<_> = system.tiles.layout_roots.keys().collect();
        for layout in layouts {
            let root = system.tiles.root(layout);
            if system.tiles.map().region_pair(root).is_some() {
                continue;
            }
            let pair = if let Some(&Regions { master, stack }) = stored.regions.get(layout) {
                (master, stack)
            } else {
                let children: Vec<_> = root.children(system.tiles.map()).collect();
                if let [master, stack] = children[..]
                    && children.iter().all(|&node| system.tiles.window_at(node).is_none())
                {
                    (master, stack)
                } else {
                    let selected = system.tiles.selected_window(layout);
                    let (master, stack) = system.create_containers(root);
                    let mut count = 0;
                    for child in children {
                        let n = system.count_windows(child);
                        let target = if count < system.settings.master_count {
                            master
                        } else {
                            stack
                        };
                        system.tiles.tree.move_node(child, Placement::Back(target));
                        count += n;
                    }
                    system.apply_master_ratio(master, stack);
                    if let Some(window) = selected {
                        system.tiles.focus(layout, Focus::Select(window)).handled;
                    }
                    (master, stack)
                }
            };
            system.tiles.tree.map.set_region_pair(root, pair.0, pair.1);
        }
        system
    }
}

impl Default for MasterStackLayoutSystem {
    fn default() -> Self { Self::new(MasterStackSettings::default()) }
}

impl MasterStackLayoutSystem {
    pub fn new(settings: MasterStackSettings) -> Self {
        Self {
            tiles: GroupLayouts::default(),
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
            let master_first = self.master_first();
            self.tiles.tree.map.set_primary_region_first(root, master_first);
            self.tiles.set_layout(root, LayoutKind::from(self.root_orientation()));
            self.tiles.set_layout(master, LayoutKind::from(self.master_orientation()));
            self.tiles.set_layout(stack, LayoutKind::from(self.stack_orientation()));
            if old_ratio != self.settings.master_ratio {
                self.apply_master_ratio(master, stack);
            }
            self.distribute_regions(layout);
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

    fn count_windows(&self, container: NodeId) -> usize {
        if self.tiles.window_at(container).is_some() {
            return 1;
        }
        container
            .children(self.tiles.map())
            .map(|child| self.count_windows(child))
            .sum()
    }

    fn focused_container(&self, layout: LayoutId, master: NodeId, stack: NodeId) -> Option<NodeId> {
        let (node, _) = self.tiles.focused_leaf(layout)?;
        let map = self.tiles.map();
        node.ancestors(map).find(|&ancestor| ancestor == master || ancestor == stack)
    }

    fn target_container_for_new_window(
        &self,
        layout: LayoutId,
        master: NodeId,
        stack: NodeId,
        master_count: usize,
    ) -> NodeId {
        if master_count < self.settings.master_count {
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

    pub(super) fn region_roots(&self, layout: LayoutId) -> (NodeId, NodeId, NodeId) {
        let root = self.tiles.root(layout);
        let (master, stack) = self.tiles.map().region_pair(root).expect("master stack region pair");
        (root, master, stack)
    }

    fn enforce_master_count(
        &mut self,
        layout: LayoutId,
        master: NodeId,
        stack: NodeId,
        current: usize,
        keep_in_master: Option<WindowId>,
    ) -> bool {
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
        let protected = keep_in_master
            .filter(|_| overflow && desired != 0)
            .and_then(|window| self.tiles.window_node(layout, window));
        let next_window = |system: &Self, start: Option<NodeId>| {
            let map = system.tiles.map();
            std::iter::successors(start, |node| node.step(source, map, forward))
                .find(|&node| Some(node) != protected && map.window(node).is_some())
        };
        let mut next = next_window(self, Some(start));
        if next.is_none() {
            return false;
        }
        let selected = self.tiles.focused_leaf(layout).map(|(node, _)| node);
        let front = overflow == self.master_first();
        let mut remaining = count;
        while let Some(leaf) = next {
            let map = self.tiles.map();
            let (mut node, mut moved) = (leaf, 1);
            if remaining > 1 {
                // Count each sibling subtree once as we walk outward from this leaf.
                // A complete group can move atomically while a partial quota still moves leaves.
                let mut child = leaf;
                let mut size = 1;
                while let Some(parent) = child.parent(map).filter(|&node| node != source) {
                    size += parent
                        .children(map)
                        .filter(|&node| node != child)
                        .map(|node| self.count_windows(node))
                        .sum::<usize>();
                    if size > remaining
                        || protected.is_some_and(|keep| {
                            keep.ancestors(map).any(|ancestor| ancestor == parent)
                        })
                    {
                        break;
                    }
                    if size > 1 {
                        (node, moved) = (parent, size);
                    }
                    child = parent;
                }
            }
            let restore_focus =
                selected.filter(|&focus| focus.ancestors(map).any(|ancestor| ancestor == node));
            remaining -= moved;
            next = if remaining == 0 {
                None
            } else {
                // The next leaf retains its identity even if removing this branch
                // collapses an ancestor.
                let edge = if forward {
                    node.last_descendant(map)
                } else {
                    node
                };
                next_window(self, edge.step(source, map, forward))
            };
            self.move_to_region(node, destination, front);
            if let Some(focus) = restore_focus {
                self.tiles.select(focus);
            }
        }
        true
    }

    fn move_to_region(&mut self, node: NodeId, region: NodeId, front: bool) {
        let placement = front
            .then(|| region.first_child(self.tiles.map()))
            .flatten()
            .map_or(Placement::Back(region), Placement::Before);
        self.tiles.tree.move_node(node, placement);
    }

    fn insert_window_in_container(
        &mut self,
        layout: LayoutId,
        container: NodeId,
        wid: WindowId,
    ) -> NodeId {
        let map = self.tiles.map();
        let relative = match self.settings.base.window_insertion_point {
            Some(WindowInsertionPoint::NextToSelection) => {
                let selection = self.tiles.selection(layout);
                (selection.parent(map) == Some(container)).then_some(Placement::After(selection))
            }
            Some(WindowInsertionPoint::EndOfTree) => None,
            None => container.first_child(map).map(Placement::Before),
        };
        let node = self.tiles.tree.insert(relative.unwrap_or(Placement::Back(container)));
        self.tiles.tree.window.set_window(&mut self.tiles.tree.map, layout, node, wid);
        node
    }

    fn distribute_regions(&mut self, layout: LayoutId) -> bool {
        let (_, master, stack) = self.region_roots(layout);
        self.enforce_master_count(layout, master, stack, self.count_windows(master), None)
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
        let Some((node, focused_wid)) = self.tiles.focused_leaf(layout) else {
            return;
        };
        let map = self.tiles.map();
        let overflow = master.last_descendant(map);
        let promoted = if !node.ancestors(map).any(|ancestor| ancestor == master)
            && self.count_windows(master) >= self.settings.master_count
            && let Some(overflow_wid) = self.tiles.window_at(overflow)
        {
            let swapped = self.tiles.swap_windows(layout, focused_wid, overflow_wid);
            let (demoted, promoted) = if swapped {
                (node, overflow)
            } else {
                (overflow, node)
            };
            self.move_to_region(demoted, stack, true);
            promoted
        } else {
            node
        };
        self.move_to_region(promoted, master, true);
        let current = self.count_windows(master);
        if current > self.settings.master_count {
            self.enforce_master_count(layout, master, stack, current, Some(focused_wid));
        }
        self.tiles.select(promoted);
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

    pub fn rotate_master_stack(&mut self, layout: LayoutId, backwards: bool) -> bool {
        let (_, master, stack) = self.region_roots(layout);
        let mut windows: Vec<_> = self
            .tiles
            .windows_from(master, Some(master), true)
            .chain(self.tiles.windows_from(stack, Some(stack), true))
            .filter(|&window| self.tiles.window_participates(window))
            .collect();
        if windows.len() < 2 {
            return false;
        }
        if backwards {
            windows.reverse();
        }
        let (wrap, rest) = windows.split_first().unwrap();
        // Carry the wrapping window across the slots without moving their containers.
        for &window in rest {
            self.tiles.swap_windows(layout, *wrap, window);
        }
        true
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

    fn remove_layout(&mut self, layout: LayoutId) { self.tiles.remove_layout(layout); }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.tiles.window_in_direction(layout, direction)
    }

    fn create_layout(&mut self) -> LayoutId {
        let layout = self.tiles.create_layout();
        let root = self.tiles.root(layout);
        let (master, stack) = self.create_containers(root);
        self.apply_master_ratio(master, stack);
        self.tiles.tree.map.set_region_pair(root, master, stack);
        layout
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId { self.tiles.clone_layout(layout) }

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
        super::tiles::project(
            &self.tiles,
            root,
            |active| super::tiles::project_root(root, Some((master, stack)), active),
            screen,
            stack_offset,
            constraints,
            gaps,
            stack_line,
            true,
            None,
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
                for info in self.tiles.remove_window(window) {
                    self.distribute_regions(info.layout);
                }
            }
            WindowChange::CloseApp(pid) => {
                let layouts: Vec<_> = self
                    .tiles
                    .layout_roots
                    .keys()
                    .filter(|&layout| self.tiles.has_windows_for_app(layout, pid))
                    .collect();
                self.tiles.close_app(pid);
                for layout in layouts {
                    self.distribute_regions(layout);
                }
            }
            WindowChange::Replace(from, to) => self.tiles.replace_window(from, to),
            WindowChange::Resize {
                layout,
                window,
                old,
                new,
                screen,
                gaps,
            } => {
                super::tiles::resize_window(
                    &mut self.tiles,
                    layout,
                    window,
                    old,
                    new,
                    screen,
                    gaps,
                );
            }
        }
    }

    fn edit_layout(&mut self, layout: LayoutId, edit: LayoutEdit) -> EditResult {
        match edit {
            LayoutEdit::Move {
                direction,
                has_display_neighbor: _,
            } => EditResult::from(self.move_selection(layout, direction)),
            LayoutEdit::Fullscreen { within_gaps } => {
                EditResult::Raise(self.tiles.toggle_fullscreen(layout, within_gaps))
            }
            LayoutEdit::Swap(a, b) => EditResult::from(self.tiles.swap_windows(layout, a, b)),
            LayoutEdit::ReorderTab { window, target } => {
                EditResult::from(self.tiles.move_stack_tab(layout, window, target))
            }
            LayoutEdit::Resize { amount, orientation } => {
                super::tiles::resize_selection(&mut self.tiles, layout, amount, orientation)
            }
            LayoutEdit::Drop { source, target, action } => {
                EditResult::from(super::tiles::apply_explicit_window_drop(
                    &mut self.tiles,
                    layout,
                    source,
                    target,
                    action,
                ))
            }

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
        let count = self.count_windows(master);
        let target = self.target_container_for_new_window(layout, master, stack, count);
        let node = self.insert_window_in_container(layout, target, wid);
        self.tiles.select(node);

        let insertion = self.settings.base.window_insertion_point;
        let keep = (insertion == Some(WindowInsertionPoint::NextToSelection) && target == master)
            .then_some(wid);
        self.enforce_master_count(
            layout,
            master,
            stack,
            count + usize::from(target == master),
            keep,
        );
        if insertion == Some(WindowInsertionPoint::EndOfTree)
            && let Some(parent) = node.parent(self.tiles.map())
        {
            // This policy appends in the final region after master overflow is resolved.
            self.tiles.tree.move_node(node, Placement::Back(parent));
            self.tiles.select(node);
        }
    }

    pub(in crate::layout_engine) fn can_move_window(
        &self,
        layout: LayoutId,
        window: WindowId,
        direction: Direction,
    ) -> bool {
        self.window_participates(window)
            && self
                .tiles
                .window_node(layout, window)
                .is_some_and(|node| self.move_target(layout, node, direction).is_some())
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let Some((node, focused)) = self.tiles.focused_leaf(layout) else {
            return false;
        };
        let Some(target) = self.move_target(layout, node, direction) else {
            return false;
        };
        if !self.tiles.swap_windows(layout, focused, target) {
            return false;
        }
        self.tiles.focus(layout, Focus::Select(focused)).handled;
        true
    }

    fn move_target(
        &self,
        layout: LayoutId,
        node: NodeId,
        direction: Direction,
    ) -> Option<WindowId> {
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
        let start = node.step(region, map, forward);
        let mut neighbors = self.tiles.windows_from(region, start, forward).peekable();
        let participates = |window| self.tiles.window_participates(window);
        if toward_other && (!parallel || neighbors.peek().is_none()) {
            // Enter the destination from the near edge, skipping suspended windows.
            let edge = if forward {
                other
            } else {
                other.last_descendant(map)
            };
            let mut destination = self.tiles.windows_from(other, Some(edge), forward).peekable();
            if destination.peek().is_some() {
                destination.find(|&window| participates(window))
            } else if in_master {
                // With no stack region, use the next master window if one exists.
                self.tiles
                    .windows_from(region, node.next_in_subtree(region, map), true)
                    .find(|&window| participates(window))
            } else if parallel {
                neighbors.find(|&window| participates(window))
            } else {
                None
            }
        } else if parallel {
            neighbors.find(|&window| participates(window))
        } else {
            None
        }
    }
}

impl MasterStackLayoutSystem {
    pub(crate) fn draw_tree(&self, layout: LayoutId) -> String {
        let (_, master, stack) = self.region_roots(layout);
        self.tiles.draw_tree(layout, &[(master, "master"), (stack, "stack")])
    }

    pub(crate) fn container_tree(&self, layout: LayoutId) -> rift_protocol::ContainerTreeNode {
        let (_, master, stack) = self.region_roots(layout);
        self.tiles
            .container_tree_with_roles(layout, &[(master, "master"), (stack, "stack")])
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

        // The previous format stored semantic region IDs beside the forest.
        #[derive(Serialize)]
        struct LegacyRegions {
            master: NodeId,
            stack: NodeId,
        }
        #[derive(Serialize)]
        struct Legacy<'a> {
            inner: &'a GroupLayouts,
            regions: slotmap::SecondaryMap<LayoutId, LegacyRegions>,
        }
        let (_, master, stack) = system.region_roots(layout);
        let mut regions = slotmap::SecondaryMap::new();
        regions.insert(layout, LegacyRegions { master, stack });
        let legacy = ron::to_string(&Legacy { inner: &system.tiles, regions }).unwrap();
        assert!(legacy.contains(",master_first:Some(false)"));
        let legacy = legacy.replace(",master_first:Some(false)", "");
        let restored: MasterStackLayoutSystem = ron::from_str(&legacy).unwrap();
        assert_eq!(restored.container_tree(layout), after);
    }

    #[test]
    fn master_count_moves_a_complete_nested_group_without_flattening_it() {
        use crate::layout_engine::WindowDropAction;
        let mut system = MasterStackLayoutSystem::default();
        let layout = system.create_layout();
        for window in [w(1), w(2), w(3), w(4)] {
            system.update(WindowChange::Add(layout, window, None));
        }
        for (source, action) in [
            (w(3), WindowDropAction::Stack),
            (w(4), WindowDropAction::Insert(Direction::Right)),
        ] {
            assert!(
                system.edit(layout, LayoutEdit::Drop { source, target: w(2), action }).changed()
            );
        }
        system.focus(layout, Focus::Select(w(2))).handled;
        let before = system.container_tree(layout);
        let group = &before.children[1].children[0];
        assert!(group.children.iter().any(|child| child.children.len() == 2));

        system.adjust_master_count(layout, 3);

        let after = system.container_tree(layout);
        assert_eq!(after.children[0].node_id, before.children[0].node_id);
        assert_eq!(after.children[1].node_id, before.children[1].node_id);
        assert!(after.children[1].children.is_empty());
        let moved = after.children[0]
            .children
            .iter()
            .find(|child| child.node_id == group.node_id)
            .expect("the complete group moved into the master region");
        assert_eq!(moved.children, group.children);
        assert_eq!(system.selected_window(layout), Some(w(2)));
    }

    #[test]
    fn promotion_demotes_a_window_from_the_last_nested_master_group() {
        use crate::layout_engine::WindowDropAction;
        let settings = MasterStackSettings {
            master_count: 3,
            new_window_placement: MasterStackNewWindowPlacement::Stack,
            ..Default::default()
        };
        let mut system = MasterStackLayoutSystem::new(settings);
        let layout = system.create_layout();
        for index in 1..=5 {
            system.update(WindowChange::Add(layout, w(index), None));
        }
        for source in [w(2), w(1)] {
            assert!(
                system
                    .edit(layout, LayoutEdit::Drop {
                        source,
                        target: w(3),
                        action: WindowDropAction::Stack,
                    })
                    .changed()
            );
        }
        let before = system.container_tree(layout);
        let [group] = &before.children[0].children[..] else {
            panic!("expected one grouped master child");
        };
        assert_eq!(group.children.len(), 3);

        assert!(system.focus(layout, Focus::Select(w(4))).handled);
        system.promote_to_master(layout);

        let in_master =
            |window| system.tiles.window_slot(layout, window).unwrap().first() == Some(&0);
        assert_eq!((1..=5).map(w).filter(|&window| in_master(window)).count(), 3);
        assert_eq!(system.tiles.window_slot(layout, w(4)), Some(vec![0, 0]));
        assert_eq!(system.selected_window(layout), Some(w(4)));
        let after = system.container_tree(layout);
        assert_eq!(after.children[0].children[1].node_id, group.node_id);
        assert_eq!(after.children[0].children[1].children.len(), 2);
    }

    #[test]
    fn keyboard_move_across_regions_skips_suspended_edge_windows() {
        for (master_side, direction, edge, target) in [
            (MasterStackSide::Left, Direction::Right, w(2), w(3)),
            (MasterStackSide::Right, Direction::Left, w(3), w(2)),
        ] {
            let mut settings = MasterStackSettings {
                master_side,
                new_window_placement: MasterStackNewWindowPlacement::Stack,
                ..Default::default()
            };
            settings.base.window_insertion_point = Some(WindowInsertionPoint::EndOfTree);
            let mut system = MasterStackLayoutSystem::new(settings);
            let layout = system.create_layout();
            for window in [w(1), w(2), w(3)] {
                system.update(WindowChange::Add(layout, window, None));
            }
            assert!(system.focus(layout, Focus::Select(w(1))).handled);
            let target_slot = system.tiles.window_slot(layout, target);
            let edge_slot = system.tiles.window_slot(layout, edge);
            system.set_window_participation(edge, false);
            assert!(
                system
                    .edit(layout, LayoutEdit::Move {
                        direction,
                        has_display_neighbor: false
                    })
                    .changed(),
                "{master_side:?}"
            );
            assert_eq!(system.tiles.window_slot(layout, w(1)), target_slot);
            assert_eq!(system.tiles.window_slot(layout, edge), edge_slot);
        }
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
