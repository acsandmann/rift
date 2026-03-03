use nix::libc::pid_t;
use objc2_core_foundation::{CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::WindowId;
use crate::common::config::{StackDefaultOrientation, default_stack_orientation};
use crate::layout_engine::systems::LayoutSystem;
use crate::layout_engine::{Direction, LayoutId, LayoutKind, TraditionalLayoutSystem};

#[derive(Serialize, Deserialize, Debug)]
pub struct StackLayoutSystem {
    inner: TraditionalLayoutSystem,
    #[serde(default = "default_stack_orientation")]
    default_orientation: StackDefaultOrientation,
}

impl Default for StackLayoutSystem {
    fn default() -> Self { Self::new(default_stack_orientation()) }
}

impl StackLayoutSystem {
    pub fn new(default_orientation: StackDefaultOrientation) -> Self {
        Self {
            inner: TraditionalLayoutSystem::default(),
            default_orientation,
        }
    }

    pub fn update_settings(&mut self, default_orientation: StackDefaultOrientation) {
        self.default_orientation = default_orientation;
    }

    fn initial_stack_kind(&self) -> LayoutKind {
        match self.default_orientation {
            StackDefaultOrientation::Perpendicular | StackDefaultOrientation::Vertical => {
                LayoutKind::VerticalStack
            }
            StackDefaultOrientation::Same | StackDefaultOrientation::Horizontal => {
                LayoutKind::HorizontalStack
            }
        }
    }

    fn stack_kind_for(kind: LayoutKind) -> LayoutKind {
        match kind {
            LayoutKind::Horizontal | LayoutKind::HorizontalStack => LayoutKind::HorizontalStack,
            LayoutKind::Vertical | LayoutKind::VerticalStack => LayoutKind::VerticalStack,
        }
    }

    fn windows_in_layout_preorder(&self, layout: LayoutId) -> Vec<WindowId> {
        let root = self.inner.root(layout);
        root.traverse_preorder(self.inner.map())
            .filter_map(|node| self.inner.window_at(node))
            .collect()
    }

    fn normalize_layout(&mut self, layout: LayoutId) {
        let root = self.inner.root(layout);
        let stack_kind = Self::stack_kind_for(self.inner.layout(root));
        let selected = self.inner.selected_window(layout);
        let windows = self.windows_in_layout_preorder(layout);

        let children: Vec<_> = root.children(self.inner.map()).collect();
        for child in children {
            child.detach(&mut self.inner.tree).remove();
        }

        self.inner.set_layout(root, stack_kind);

        let mut selected_node = None;
        let mut first_node = None;
        for wid in windows {
            let node = self.inner.add_window_under(layout, root, wid);
            if first_node.is_none() {
                first_node = Some(node);
            }
            if Some(wid) == selected {
                selected_node = Some(node);
            }
        }

        if let Some(node) = selected_node.or(first_node) {
            self.inner.select(node);
        }
    }

    fn toggle_root_stack_orientation(&mut self, layout: LayoutId) {
        self.normalize_layout(layout);
        let root = self.inner.root(layout);
        let next = match self.inner.layout(root) {
            LayoutKind::Horizontal | LayoutKind::HorizontalStack => LayoutKind::VerticalStack,
            LayoutKind::Vertical | LayoutKind::VerticalStack => LayoutKind::HorizontalStack,
        };
        self.inner.set_layout(root, next);
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

impl LayoutSystem for StackLayoutSystem {
    fn create_layout(&mut self) -> LayoutId {
        let layout = self.inner.create_layout();
        self.inner.set_layout(self.inner.root(layout), self.initial_stack_kind());
        self.normalize_layout(layout);
        layout
    }

    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId {
        let cloned = self.inner.clone_layout(layout);
        self.normalize_layout(cloned);
        cloned
    }

    fn remove_layout(&mut self, layout: LayoutId) { self.inner.remove_layout(layout); }

    fn draw_tree(&self, layout: LayoutId) -> String { self.inner.draw_tree(layout) }

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
        self.normalize_layout(layout);
        let node = self.inner.add_window_under(layout, self.inner.root(layout), wid);
        self.inner.select(node);
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

    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        let before = self.windows_in_layout_preorder(layout);
        self.inner.set_windows_for_app(layout, pid, desired);
        let after = self.windows_in_layout_preorder(layout);
        if before != after {
            self.normalize_layout(layout);
        }
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

    fn apply_window_size_constraint(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        current_frame: CGRect,
        target_size: CGSize,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    ) {
        self.inner.apply_window_size_constraint(
            layout,
            wid,
            current_frame,
            target_size,
            screen,
            gaps,
        );
    }

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool {
        self.inner.swap_windows(layout, a, b)
    }

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool {
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
        self.normalize_layout(from_layout);
        self.normalize_layout(to_layout);
    }

    fn split_selection(&mut self, _layout: LayoutId, _kind: LayoutKind) {}

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.inner.toggle_fullscreen_of_selection(layout)
    }

    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId> {
        self.inner.toggle_fullscreen_within_gaps_of_selection(layout)
    }

    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool {
        self.inner.has_any_fullscreen_node(layout)
    }

    fn join_selection_with_direction(&mut self, _layout: LayoutId, _direction: Direction) {}

    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        self.toggle_root_stack_orientation(layout);
        self.inner.visible_windows_in_layout(layout)
    }

    fn unstack_parent_of_selection(
        &mut self,
        _layout: LayoutId,
        _default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId> {
        Vec::new()
    }

    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool {
        let root = self.inner.root(layout);
        self.inner.layout(root).is_stacked()
    }

    fn unjoin_selection(&mut self, _layout: LayoutId) {}

    fn resize_selection_by(&mut self, _layout: LayoutId, _amount: f64) {}

    fn rebalance(&mut self, _layout: LayoutId) {}

    fn toggle_tile_orientation(&mut self, layout: LayoutId) {
        self.toggle_root_stack_orientation(layout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(idx: u32) -> WindowId { WindowId::new(1, idx) }

    #[test]
    fn create_layout_starts_as_stack() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();
        let root = system.inner.root(layout);
        assert!(system.inner.layout(root).is_stacked());
        assert_eq!(system.inner.layout(root), LayoutKind::VerticalStack);
    }

    #[test]
    fn create_layout_honors_stack_default_orientation_setting() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Horizontal);
        let layout = system.create_layout();
        let root = system.inner.root(layout);
        assert_eq!(system.inner.layout(root), LayoutKind::HorizontalStack);
    }

    #[test]
    fn toggle_stack_command_path_keeps_layout_stacked() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));

        let _ = system.unstack_parent_of_selection(
            layout,
            crate::common::config::StackDefaultOrientation::Perpendicular,
        );
        let _ = system.apply_stacking_to_parent_of_selection(
            layout,
            crate::common::config::StackDefaultOrientation::Perpendicular,
        );

        let root = system.inner.root(layout);
        assert!(system.inner.layout(root).is_stacked());
    }

    #[test]
    fn set_windows_for_app_noop_keeps_fullscreen_state() {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();

        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        let _ = system.toggle_fullscreen_of_selection(layout);
        assert!(system.has_any_fullscreen_node(layout));

        system.set_windows_for_app(layout, 1, vec![w(1), w(2)]);

        assert!(system.has_any_fullscreen_node(layout));
    }

    fn setup_fullscreen_stack_system() -> (StackLayoutSystem, LayoutId) {
        let mut system = StackLayoutSystem::new(StackDefaultOrientation::Perpendicular);
        let layout = system.create_layout();
        system.add_window_after_selection(layout, w(1));
        system.add_window_after_selection(layout, w(2));
        let _ = system.toggle_fullscreen_of_selection(layout);
        assert!(system.has_any_fullscreen_node(layout));
        (system, layout)
    }

    #[test]
    fn resize_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.resize_selection_by(layout, 0.1);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn rebalance_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.rebalance(layout);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn split_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.split_selection(layout, LayoutKind::Horizontal);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn join_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.join_selection_with_direction(layout, Direction::Right);
        assert!(system.has_any_fullscreen_node(layout));
    }

    #[test]
    fn unjoin_selection_noop_keeps_fullscreen_state() {
        let (mut system, layout) = setup_fullscreen_stack_system();
        system.unjoin_selection(layout);
        assert!(system.has_any_fullscreen_node(layout));
    }
}
