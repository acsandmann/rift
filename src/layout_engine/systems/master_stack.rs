use std::collections::HashMap;

use nix::libc::pid_t;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use crate::common::config::{MasterStackNewWindowPlacement, MasterStackSettings, MasterStackSide};
use crate::layout_engine::utils::compute_tiling_area;
use crate::layout_engine::{
    Direction, LayoutId, LayoutKind, LayoutSystem, Orientation, TraditionalLayoutSystem,
    WindowConstraint,
};
use crate::model::tree::NodeId;

#[derive(Serialize, Deserialize, Debug)]
pub struct MasterStackLayoutSystem {
    inner: TraditionalLayoutSystem,
    settings: MasterStackSettings,
}

impl Default for MasterStackLayoutSystem {
    fn default() -> Self { Self::new(MasterStackSettings::default()) }
}

impl MasterStackLayoutSystem {
    pub fn new(settings: MasterStackSettings) -> Self {
        Self {
            inner: TraditionalLayoutSystem::default(),
            settings,
        }
    }

    pub fn update_settings(&mut self, settings: MasterStackSettings) {
        if self.settings == settings {
            return;
        }
        let old_master_first = self.master_first();
        self.settings = settings;
        let layouts: Vec<_> = self.inner.layout_roots.keys().collect();
        for layout in layouts {
            if let Some(windows) =
                self.windows_in_layout_by_container_with_order(layout, old_master_first)
            {
                self.rebuild_layout_with_windows(layout, &windows);
                continue;
            }
            self.rebuild_layout(layout);
        }
    }

    pub fn set_window_constraint(&mut self, wid: WindowId, constraint: WindowConstraint) {
        self.inner.set_window_constraint(wid, constraint);
    }

    pub fn clear_window_constraint(&mut self, wid: WindowId) {
        self.inner.clear_window_constraint(wid);
    }

    pub fn window_constraint(&self, wid: WindowId) -> Option<WindowConstraint> {
        self.inner.window_constraint(wid)
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

    fn master_first(&self) -> bool {
        matches!(
            self.settings.master_side,
            MasterStackSide::Left | MasterStackSide::Top
        )
    }

    fn all_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        let root = self.inner.root(layout);
        root.traverse_preorder(self.inner.map())
            .filter_map(|node| self.inner.window_at(node))
            .collect()
    }

    fn windows_in_layout_by_container(&self, layout: LayoutId) -> Vec<WindowId> {
        self.windows_in_layout_by_container_with_order(layout, self.master_first())
            .unwrap_or_else(|| self.all_windows_in_layout(layout))
    }

    fn windows_in_layout_by_container_with_order(
        &self,
        layout: LayoutId,
        master_first: bool,
    ) -> Option<Vec<WindowId>> {
        let root = self.inner.root(layout);
        let children: Vec<_> = root.children(self.inner.map()).collect();
        if children.len() != 2
            || children.iter().any(|&child| self.inner.window_at(child).is_some())
        {
            return None;
        }
        let (master, stack) = if master_first {
            (children[0], children[1])
        } else {
            (children[1], children[0])
        };
        let mut ordered = self.windows_in_container(master);
        ordered.extend(self.windows_in_container(stack));
        Some(ordered)
    }

    fn windows_in_container(&self, container: NodeId) -> Vec<WindowId> {
        container
            .traverse_preorder(self.inner.map())
            .filter_map(|node| self.inner.window_at(node))
            .collect()
    }

    fn container_is_flat(&self, container: NodeId) -> bool {
        container
            .children(self.inner.map())
            .all(|child| self.inner.window_at(child).is_some())
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

    fn focused_window_in_container(&self, container: NodeId) -> Option<WindowId> {
        let map = self.inner.map();
        let selection = self.inner.local_selection(container);
        let candidate = selection.or_else(|| container.first_child(map));
        let candidate = candidate?;
        candidate.traverse_preorder(map).find_map(|node| self.inner.window_at(node))
    }

    fn create_containers(&mut self, root: NodeId) -> (NodeId, NodeId) {
        self.inner.set_layout(root, LayoutKind::from(self.root_orientation()));
        let container_kind = LayoutKind::from(self.container_orientation());
        let first = self.inner.tree.mk_node().push_back(root);
        self.inner.set_layout(first, container_kind);
        let second = self.inner.tree.mk_node().push_back(root);
        self.inner.set_layout(second, container_kind);
        if self.master_first() {
            (first, second)
        } else {
            (second, first)
        }
    }

    fn apply_master_ratio(&mut self, root: NodeId, master: NodeId, stack: NodeId) {
        let ratio = self.settings.master_ratio.clamp(0.05, 0.95) as f32;
        let total = 2.0_f32;
        let master_size = (ratio * total).max(0.05);
        let stack_size = (total - master_size).max(0.05);
        self.inner.tree.data.layout.info[master].size = master_size;
        self.inner.tree.data.layout.info[stack].size = stack_size;
        self.inner.tree.data.layout.info[root].total = master_size + stack_size;
    }

    fn ensure_structure(&mut self, layout: LayoutId) -> (NodeId, NodeId, NodeId) {
        let root = self.inner.root(layout);
        let children: Vec<_> = root.children(self.inner.map()).collect();
        let valid = children.len() == 2
            && children.iter().all(|&c| self.inner.window_at(c).is_none())
            && children.iter().all(|&c| self.container_is_flat(c));
        if !valid {
            self.rebuild_layout(layout);
        }
        let children: Vec<_> = root.children(self.inner.map()).collect();
        if children.len() != 2 {
            let (master, stack) = self.create_containers(root);
            self.apply_master_ratio(root, master, stack);
            return (root, master, stack);
        }
        let first = children[0];
        let second = children[1];
        self.inner.set_layout(root, LayoutKind::from(self.root_orientation()));
        let container_kind = LayoutKind::from(self.container_orientation());
        self.inner.set_layout(first, container_kind);
        self.inner.set_layout(second, container_kind);
        let (master, stack) = if self.master_first() {
            (first, second)
        } else {
            (second, first)
        };
        self.apply_master_ratio(root, master, stack);
        (root, master, stack)
    }

    fn rebuild_layout(&mut self, layout: LayoutId) {
        let windows = self.windows_in_layout_by_container(layout);
        self.rebuild_layout_with_windows(layout, &windows);
    }

    fn rebuild_layout_with_windows(&mut self, layout: LayoutId, windows: &[WindowId]) {
        let selected = self.inner.selected_window(layout);
        let root = self.inner.root(layout);
        let children: Vec<_> = root.children(self.inner.map()).collect();
        for child in children {
            child.detach(&mut self.inner.tree).remove();
        }
        let (master, stack) = self.create_containers(root);
        for (idx, wid) in windows.iter().enumerate() {
            let target = if idx < self.settings.master_count {
                master
            } else {
                stack
            };
            let node = self.inner.add_window_under(layout, target, *wid);
            if Some(*wid) == selected {
                self.inner.select(node);
            }
        }
        self.apply_master_ratio(root, master, stack);
        if let Some(wid) = selected {
            let _ = self.inner.select_window(layout, wid);
        }
        self.enforce_master_count(layout, master, stack);
    }

    fn enforce_master_count(&mut self, layout: LayoutId, master: NodeId, stack: NodeId) {
        let mut master_windows = self.windows_in_container(master);
        let mut stack_windows = self.windows_in_container(stack);
        let selected = self.inner.selected_window(layout);
        let desired = self.settings.master_count;

        if master_windows.is_empty() && !stack_windows.is_empty() {
            if let Some(wid) = stack_windows.get(0).copied() {
                if let Some(node) = self.move_window_to_container(layout, wid, master) {
                    if Some(wid) == selected {
                        self.inner.select(node);
                    }
                }
                master_windows.push(wid);
                stack_windows.remove(0);
            }
        }

        if master_windows.len() > desired {
            let overflow = master_windows.split_off(desired);
            for wid in overflow.into_iter().rev() {
                if let Some(node) = self.move_window_to_container_front(layout, wid, stack) {
                    if Some(wid) == selected {
                        self.inner.select(node);
                    }
                }
            }
        } else if master_windows.len() < desired {
            let needed = desired - master_windows.len();
            let to_move: Vec<_> = stack_windows.drain(..needed.min(stack_windows.len())).collect();
            for wid in to_move {
                if let Some(node) = self.move_window_to_container(layout, wid, master) {
                    if Some(wid) == selected {
                        self.inner.select(node);
                    }
                }
            }
        }
    }

    fn move_window_to_container(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        container: NodeId,
    ) -> Option<NodeId> {
        if !self.inner.map().contains(container) {
            return None;
        }
        let node = self.inner.tree.data.window.node_for(layout, wid)?;
        if !self.inner.map().contains(node) {
            return None;
        }
        if node.parent(self.inner.map()) == Some(container) {
            return Some(node);
        }
        Some(node.detach(&mut self.inner.tree).push_back(container).finish())
    }

    fn add_window_to_container_front(
        &mut self,
        layout: LayoutId,
        container: NodeId,
        wid: WindowId,
    ) -> Option<NodeId> {
        if !self.inner.map().contains(container) {
            return None;
        }
        let first_child = container.children(self.inner.map()).next();
        let node = match first_child {
            Some(first_child) => self.inner.tree.mk_node().insert_before(first_child),
            None => self.inner.tree.mk_node().push_back(container),
        };
        self.inner.tree.data.window.set_window(layout, node, wid);
        Some(node)
    }

    fn move_window_to_container_front(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        container: NodeId,
    ) -> Option<NodeId> {
        if !self.inner.map().contains(container) {
            return None;
        }
        let node = self.inner.tree.data.window.node_for(layout, wid)?;
        if !self.inner.map().contains(node) {
            return None;
        }
        if node.parent(self.inner.map()) == Some(container) {
            return Some(node);
        }
        let first_child = {
            let mut children_iter = container.children(self.inner.map());
            children_iter.next()
        };
        if let Some(first_child) = first_child {
            Some(node.detach(&mut self.inner.tree).insert_before(first_child).finish())
        } else {
            Some(node.detach(&mut self.inner.tree).push_back(container).finish())
        }
    }

    fn normalize_layout(&mut self, layout: LayoutId) {
        let (_root, master, stack) = self.ensure_structure(layout);
        self.enforce_master_count(layout, master, stack);
    }

    pub fn adjust_master_ratio(&mut self, _layout: LayoutId, delta: f64) {
        let next = (self.settings.master_ratio + delta).clamp(0.05, 0.95);
        if (next - self.settings.master_ratio).abs() < f64::EPSILON {
            return;
        }
        self.settings.master_ratio = next;
        let layouts: Vec<_> = self.inner.layout_roots.keys().collect();
        for layout in layouts {
            self.normalize_layout(layout);
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
            self.normalize_layout(layout);
        }
    }

    pub fn promote_to_master(&mut self, layout: LayoutId) {
        let (_root, master, stack) = self.ensure_structure(layout);
        let Some(wid) = self.inner.selected_window(layout) else {
            return;
        };
        let master_windows = self.windows_in_container(master);
        if master_windows.first().copied() == Some(wid) {
            return;
        }
        if let Some(node) = self.move_window_to_container_front(layout, wid, master) {
            self.inner.select(node);
        }
        self.enforce_master_count(layout, master, stack);
    }

    pub fn swap_master_stack(&mut self, layout: LayoutId) {
        let (_root, master, stack) = self.ensure_structure(layout);
        let (Some(master_wid), Some(stack_wid)) = (
            self.focused_window_in_container(master),
            self.focused_window_in_container(stack),
        ) else {
            return;
        };
        let selected = self.inner.selected_window(layout);
        let _ = self.inner.swap_windows(layout, master_wid, stack_wid);
        if let Some(wid) = selected {
            let _ = self.inner.select_window(layout, wid);
        }
    }

    pub(crate) fn collect_group_containers_in_selection_path(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        self.inner.collect_group_containers_in_selection_path(
            layout,
            screen,
            stack_offset,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        )
    }

    pub(crate) fn collect_group_containers(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<crate::layout_engine::engine::GroupContainerInfo> {
        self.inner.collect_group_containers(
            layout,
            screen,
            stack_offset,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        )
    }
}

impl LayoutSystem for MasterStackLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let layout = self.inner.create_layout();
        let root = self.inner.root(layout);
        let (master, stack) = self.create_containers(root);
        self.apply_master_ratio(root, master, stack);
        layout
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let cloned = self.inner.clone_layout(layout);
        let (_root, master, stack) = self.ensure_structure(cloned);
        self.enforce_master_count(cloned, master, stack);
        cloned
    }

    fn remove_layout(&mut self, layout: LayoutId) { self.inner.remove_layout(layout); }

    fn draw_tree(&self, layout: LayoutId) -> String {
        let root = self.inner.root(layout);
        let children: Vec<_> = root.children(self.inner.map()).collect();
        if children.len() != 2 {
            return self.inner.draw_tree(layout);
        }
        if children.iter().any(|&child| self.inner.tree.data.window.at(child).is_some()) {
            return self.inner.draw_tree(layout);
        }
        let (master, stack) = if self.master_first() {
            (children[0], children[1])
        } else {
            (children[1], children[0])
        };
        let mut labels = HashMap::new();
        labels.insert(master, "master");
        labels.insert(stack, "stack");
        self.inner.draw_tree_with_labels(layout, &labels)
    }

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        let root = self.inner.root(layout);
        let children: Vec<_> = root.children(self.inner.map()).collect();
        if children.len() == 2 && children.iter().all(|&c| self.inner.window_at(c).is_none()) {
            let (master, stack) = if self.master_first() {
                (children[0], children[1])
            } else {
                (children[1], children[0])
            };
            if self.inner.visible_windows_in_subtree(stack).is_empty() {
                let rect = compute_tiling_area(screen, gaps);
                return self.inner.calculate_layout_for_node(
                    master,
                    screen,
                    rect,
                    stack_offset,
                    gaps,
                    stack_line_thickness,
                    stack_line_horiz,
                    stack_line_vert,
                );
            }
        }
        self.inner.calculate_layout(
            layout,
            screen,
            stack_offset,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        )
    }

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId> {
        self.inner.selected_window(layout)
    }

    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId> {
        self.inner.visible_windows_in_layout(layout)
    }

    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId> {
        self.inner.visible_windows_under_selection(layout)
    }

    fn ascend_selection(&mut self, layout: LayoutId) -> bool { self.inner.ascend_selection(layout) }

    fn descend_selection(&mut self, layout: LayoutId) -> bool {
        self.inner.descend_selection(layout)
    }

    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        self.inner.move_focus(layout, direction)
    }

    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId> {
        self.inner.window_in_direction(layout, direction)
    }

    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId) {
        let (_root, master, stack) = self.ensure_structure(layout);
        let master_windows = self.windows_in_container(master);
        let master_has_capacity = master_windows.len() < self.settings.master_count;
        let target = if master_has_capacity {
            master
        } else {
            match self.settings.new_window_placement {
                MasterStackNewWindowPlacement::Master => master,
                MasterStackNewWindowPlacement::Stack => stack,
                MasterStackNewWindowPlacement::Focused => {
                    self.focused_container(layout, master, stack).unwrap_or(master)
                }
            }
        };
        let node = self
            .add_window_to_container_front(layout, target, wid)
            .unwrap_or_else(|| self.inner.add_window_under(layout, target, wid));
        self.inner.select(node);
        self.enforce_master_count(layout, master, stack);
    }

    fn remove_window(&mut self, wid: WindowId) {
        let layouts = self.inner.layouts_for_window(wid);
        self.inner.remove_window(wid);
        for layout in layouts {
            self.normalize_layout(layout);
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        let layouts: Vec<_> = self
            .inner
            .layout_roots
            .keys()
            .filter(|&layout| self.inner.has_windows_for_app(layout, pid))
            .collect();
        self.inner.remove_windows_for_app(pid);
        for layout in layouts {
            self.normalize_layout(layout);
        }
    }

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, mut desired: Vec<WindowId>) {
        let (_root, master, stack) = self.ensure_structure(layout);
        let root = self.inner.root(layout);
        let mut current = root
            .traverse_postorder(self.inner.map())
            .filter_map(|node| self.inner.window_at(node).map(|wid| (wid, node)))
            .filter(|(wid, _)| wid.pid == pid)
            .collect::<Vec<_>>();
        desired.sort_unstable();
        current.sort_unstable();
        debug_assert!(desired.iter().all(|wid| wid.pid == pid));
        let mut desired = desired.into_iter().peekable();
        let mut current = current.into_iter().peekable();
        loop {
            match (desired.peek(), current.peek()) {
                (Some(des), Some((cur, _))) if des == cur => {
                    desired.next();
                    current.next();
                }
                (Some(des), None) => {
                    self.add_window_after_selection(layout, *des);
                    desired.next();
                }
                (Some(des), Some((cur, _))) if des < cur => {
                    self.add_window_after_selection(layout, *des);
                    desired.next();
                }
                (_, Some((_, node))) => {
                    if self.inner.tree.data.layout.info[*node].is_fullscreen {
                        current.next();
                    } else {
                        node.detach(&mut self.inner.tree).remove();
                        current.next();
                    }
                }
                (None, None) => break,
            }
        }
        self.enforce_master_count(layout, master, stack);
    }

    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool {
        self.inner.has_windows_for_app(layout, pid)
    }

    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool {
        self.inner.contains_window(layout, wid)
    }

    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool {
        self.inner.select_window(layout, wid)
    }

    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) {
        self.inner.on_window_resized(layout, wid, old_frame, new_frame, screen, gaps);
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        self.inner.swap_windows(layout, a, b)
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
        let (_root, master, stack) = self.ensure_structure(layout);
        let Some(container) = self.focused_container(layout, master, stack) else {
            return false;
        };
        let container_axis = self.container_orientation();
        let (towards_master, towards_stack) = match self.settings.master_side {
            MasterStackSide::Left => (direction == Direction::Left, direction == Direction::Right),
            MasterStackSide::Right => (direction == Direction::Right, direction == Direction::Left),
            MasterStackSide::Top => (direction == Direction::Up, direction == Direction::Down),
            MasterStackSide::Bottom => (direction == Direction::Down, direction == Direction::Up),
        };

        if towards_master && container == stack {
            self.promote_to_master(layout);
            self.normalize_layout(layout);
            return true;
        }
        if towards_stack && container == master {
            if self.focused_window_in_container(master).is_some()
                && self.focused_window_in_container(stack).is_some()
            {
                self.swap_master_stack(layout);
                self.normalize_layout(layout);
                return true;
            }
            return false;
        }
        if direction.orientation() != container_axis {
            return false;
        }

        let moved = self.inner.move_selection(layout, direction);
        if moved {
            self.normalize_layout(layout);
        }
        moved
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    ) {
        self.inner.move_selection_to_layout_after_selection(from_layout, to_layout);
        let _ = self.ensure_structure(from_layout);
        let _ = self.ensure_structure(to_layout);
    }

    fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind) {
        let _ = kind;
        self.normalize_layout(layout);
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.inner.toggle_fullscreen_of_selection(layout)
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.inner.toggle_fullscreen_within_gaps_of_selection(layout)
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.inner.has_any_fullscreen_node(layout)
    }

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction) {
        let _ = direction;
        self.normalize_layout(layout);
    }

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let _ = default_orientation;
        self.normalize_layout(layout);
        vec![]
    }

    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        let _ = default_orientation;
        self.normalize_layout(layout);
        vec![]
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        self.inner.parent_of_selection_is_stacked(layout)
    }

    fn unjoin_selection(&mut self, layout: LayoutId) { self.normalize_layout(layout); }

    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64) {
        let _ = amount;
        self.normalize_layout(layout);
    }

    fn rebalance(&mut self, layout: LayoutId) { self.normalize_layout(layout); }

    fn toggle_tile_orientation(&mut self, layout: LayoutId) { self.normalize_layout(layout); }
}
