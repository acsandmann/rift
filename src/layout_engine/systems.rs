use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::{Direction, LayoutKind};
use crate::actor::app::{WindowId, pid_t};

slotmap::new_key_type! { pub struct LayoutId; }

pub trait LayoutSystem: Serialize + for<'de> Deserialize<'de> {
    type LayoutId: Copy
        + Eq
        + std::hash::Hash
        + Serialize
        + for<'de> Deserialize<'de>
        + core::fmt::Debug;

    fn create_layout(&mut self) -> Self::LayoutId;
    fn clone_layout(&mut self, layout: Self::LayoutId) -> Self::LayoutId;
    fn remove_layout(&mut self, layout: Self::LayoutId);

    fn draw_tree(&self, layout: Self::LayoutId) -> String;

    fn calculate_layout(
        &self,
        layout: Self::LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)>;

    fn selected_window(&self, layout: Self::LayoutId) -> Option<WindowId>;
    fn visible_windows_in_layout(&self, layout: Self::LayoutId) -> Vec<WindowId>;
    fn visible_windows_under_selection(&self, layout: Self::LayoutId) -> Vec<WindowId>;
    fn ascend_selection(&mut self, layout: Self::LayoutId) -> bool;
    fn descend_selection(&mut self, layout: Self::LayoutId) -> bool;
    fn move_focus(
        &mut self,
        layout: Self::LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>);

    fn add_window_after_selection(&mut self, layout: Self::LayoutId, wid: WindowId);
    fn remove_window(&mut self, wid: WindowId);
    fn remove_windows_for_app(&mut self, pid: pid_t);
    fn set_windows_for_app(&mut self, layout: Self::LayoutId, pid: pid_t, desired: Vec<WindowId>);
    fn has_windows_for_app(&self, layout: Self::LayoutId, pid: pid_t) -> bool;
    fn contains_window(&self, layout: Self::LayoutId, wid: WindowId) -> bool;
    fn select_window(&mut self, layout: Self::LayoutId, wid: WindowId) -> bool;
    fn on_window_resized(
        &mut self,
        layout: Self::LayoutId,
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
    );

    fn move_selection(&mut self, layout: Self::LayoutId, direction: Direction) -> bool;
    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: Self::LayoutId,
        to_layout: Self::LayoutId,
    );
    fn split_selection(&mut self, layout: Self::LayoutId, kind: LayoutKind);
    fn toggle_fullscreen_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId>;
    fn join_selection_with_direction(&mut self, layout: Self::LayoutId, direction: Direction);
    fn apply_stacking_to_parent_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId>;
    fn unstack_parent_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId>;
    fn unjoin_selection(&mut self, layout: Self::LayoutId);
    fn resize_selection_by(&mut self, layout: Self::LayoutId, amount: f64);

    fn rebalance(&mut self, layout: Self::LayoutId);
}

mod traditional;
pub use traditional::TraditionalLayoutSystem;
mod bsp;
pub use bsp::BspLayoutSystem;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutSystemKind {
    Traditional(TraditionalLayoutSystem),
    Bsp(BspLayoutSystem),
}

impl LayoutSystem for LayoutSystemKind {
    type LayoutId = crate::layout_engine::LayoutId;

    fn create_layout(&mut self) -> Self::LayoutId {
        match self {
            LayoutSystemKind::Traditional(s) => s.create_layout(),
            LayoutSystemKind::Bsp(s) => s.create_layout(),
        }
    }

    fn clone_layout(&mut self, layout: Self::LayoutId) -> Self::LayoutId {
        match self {
            LayoutSystemKind::Traditional(s) => s.clone_layout(layout),
            LayoutSystemKind::Bsp(s) => s.clone_layout(layout),
        }
    }

    fn remove_layout(&mut self, layout: Self::LayoutId) {
        match self {
            LayoutSystemKind::Traditional(s) => s.remove_layout(layout),
            LayoutSystemKind::Bsp(s) => s.remove_layout(layout),
        }
    }

    fn draw_tree(&self, layout: Self::LayoutId) -> String {
        match self {
            LayoutSystemKind::Traditional(s) => s.draw_tree(layout),
            LayoutSystemKind::Bsp(s) => s.draw_tree(layout),
        }
    }

    fn calculate_layout(
        &self,
        layout: Self::LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)> {
        match self {
            LayoutSystemKind::Traditional(s) => s.calculate_layout(
                layout,
                screen,
                stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
            LayoutSystemKind::Bsp(s) => s.calculate_layout(
                layout,
                screen,
                stack_offset,
                gaps,
                stack_line_thickness,
                stack_line_horiz,
                stack_line_vert,
            ),
        }
    }

    fn selected_window(&self, layout: Self::LayoutId) -> Option<WindowId> {
        match self {
            LayoutSystemKind::Traditional(s) => s.selected_window(layout),
            LayoutSystemKind::Bsp(s) => s.selected_window(layout),
        }
    }

    fn visible_windows_in_layout(&self, layout: Self::LayoutId) -> Vec<WindowId> {
        match self {
            LayoutSystemKind::Traditional(s) => s.visible_windows_in_layout(layout),
            LayoutSystemKind::Bsp(s) => s.visible_windows_in_layout(layout),
        }
    }

    fn visible_windows_under_selection(&self, layout: Self::LayoutId) -> Vec<WindowId> {
        match self {
            LayoutSystemKind::Traditional(s) => s.visible_windows_under_selection(layout),
            LayoutSystemKind::Bsp(s) => s.visible_windows_under_selection(layout),
        }
    }

    fn ascend_selection(&mut self, layout: Self::LayoutId) -> bool {
        match self {
            LayoutSystemKind::Traditional(s) => s.ascend_selection(layout),
            LayoutSystemKind::Bsp(s) => s.ascend_selection(layout),
        }
    }

    fn descend_selection(&mut self, layout: Self::LayoutId) -> bool {
        match self {
            LayoutSystemKind::Traditional(s) => s.descend_selection(layout),
            LayoutSystemKind::Bsp(s) => s.descend_selection(layout),
        }
    }

    fn move_focus(
        &mut self,
        layout: Self::LayoutId,
        direction: super::Direction,
    ) -> (Option<WindowId>, Vec<WindowId>) {
        match self {
            LayoutSystemKind::Traditional(s) => s.move_focus(layout, direction),
            LayoutSystemKind::Bsp(s) => s.move_focus(layout, direction),
        }
    }

    fn add_window_after_selection(&mut self, layout: Self::LayoutId, wid: WindowId) {
        match self {
            LayoutSystemKind::Traditional(s) => s.add_window_after_selection(layout, wid),
            LayoutSystemKind::Bsp(s) => s.add_window_after_selection(layout, wid),
        }
    }

    fn remove_window(&mut self, wid: WindowId) {
        match self {
            LayoutSystemKind::Traditional(s) => s.remove_window(wid),
            LayoutSystemKind::Bsp(s) => s.remove_window(wid),
        }
    }

    fn remove_windows_for_app(&mut self, pid: pid_t) {
        match self {
            LayoutSystemKind::Traditional(s) => s.remove_windows_for_app(pid),
            LayoutSystemKind::Bsp(s) => s.remove_windows_for_app(pid),
        }
    }

    fn set_windows_for_app(&mut self, layout: Self::LayoutId, pid: pid_t, desired: Vec<WindowId>) {
        match self {
            LayoutSystemKind::Traditional(s) => s.set_windows_for_app(layout, pid, desired),
            LayoutSystemKind::Bsp(s) => s.set_windows_for_app(layout, pid, desired),
        }
    }

    fn has_windows_for_app(&self, layout: Self::LayoutId, pid: pid_t) -> bool {
        match self {
            LayoutSystemKind::Traditional(s) => s.has_windows_for_app(layout, pid),
            LayoutSystemKind::Bsp(s) => s.has_windows_for_app(layout, pid),
        }
    }

    fn contains_window(&self, layout: Self::LayoutId, wid: WindowId) -> bool {
        match self {
            LayoutSystemKind::Traditional(s) => s.contains_window(layout, wid),
            LayoutSystemKind::Bsp(s) => s.contains_window(layout, wid),
        }
    }

    fn select_window(&mut self, layout: Self::LayoutId, wid: WindowId) -> bool {
        match self {
            LayoutSystemKind::Traditional(s) => s.select_window(layout, wid),
            LayoutSystemKind::Bsp(s) => s.select_window(layout, wid),
        }
    }

    fn on_window_resized(
        &mut self,
        layout: Self::LayoutId,
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
    ) {
        match self {
            LayoutSystemKind::Traditional(s) => {
                s.on_window_resized(layout, wid, old_frame, new_frame, screen)
            }
            LayoutSystemKind::Bsp(s) => {
                s.on_window_resized(layout, wid, old_frame, new_frame, screen)
            }
        }
    }

    fn move_selection(&mut self, layout: Self::LayoutId, direction: super::Direction) -> bool {
        match self {
            LayoutSystemKind::Traditional(s) => s.move_selection(layout, direction),
            LayoutSystemKind::Bsp(s) => s.move_selection(layout, direction),
        }
    }

    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: Self::LayoutId,
        to_layout: Self::LayoutId,
    ) {
        match self {
            LayoutSystemKind::Traditional(s) => {
                s.move_selection_to_layout_after_selection(from_layout, to_layout)
            }
            LayoutSystemKind::Bsp(s) => {
                s.move_selection_to_layout_after_selection(from_layout, to_layout)
            }
        }
    }

    fn split_selection(&mut self, layout: Self::LayoutId, kind: LayoutKind) {
        match self {
            LayoutSystemKind::Traditional(s) => s.split_selection(layout, kind),
            LayoutSystemKind::Bsp(s) => s.split_selection(layout, kind),
        }
    }

    fn toggle_fullscreen_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId> {
        match self {
            LayoutSystemKind::Traditional(s) => s.toggle_fullscreen_of_selection(layout),
            LayoutSystemKind::Bsp(s) => s.toggle_fullscreen_of_selection(layout),
        }
    }

    fn join_selection_with_direction(
        &mut self,
        layout: Self::LayoutId,
        direction: super::Direction,
    ) {
        match self {
            LayoutSystemKind::Traditional(s) => s.join_selection_with_direction(layout, direction),
            LayoutSystemKind::Bsp(s) => s.join_selection_with_direction(layout, direction),
        }
    }

    fn apply_stacking_to_parent_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId> {
        match self {
            LayoutSystemKind::Traditional(s) => s.apply_stacking_to_parent_of_selection(layout),
            LayoutSystemKind::Bsp(s) => s.apply_stacking_to_parent_of_selection(layout),
        }
    }

    fn unstack_parent_of_selection(&mut self, layout: Self::LayoutId) -> Vec<WindowId> {
        match self {
            LayoutSystemKind::Traditional(s) => s.unstack_parent_of_selection(layout),
            LayoutSystemKind::Bsp(s) => s.unstack_parent_of_selection(layout),
        }
    }

    fn unjoin_selection(&mut self, layout: Self::LayoutId) {
        match self {
            LayoutSystemKind::Traditional(s) => s.unjoin_selection(layout),
            LayoutSystemKind::Bsp(s) => s.unjoin_selection(layout),
        }
    }

    fn resize_selection_by(&mut self, layout: Self::LayoutId, amount: f64) {
        match self {
            LayoutSystemKind::Traditional(s) => s.resize_selection_by(layout, amount),
            LayoutSystemKind::Bsp(s) => s.resize_selection_by(layout, amount),
        }
    }

    fn rebalance(&mut self, layout: Self::LayoutId) {
        match self {
            LayoutSystemKind::Traditional(s) => s.rebalance(layout),
            LayoutSystemKind::Bsp(s) => s.rebalance(layout),
        }
    }
}
