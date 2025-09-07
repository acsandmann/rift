use enum_dispatch::enum_dispatch;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use super::{Direction, LayoutKind};
use crate::actor::app::{WindowId, pid_t};

slotmap::new_key_type! { pub struct LayoutId; }

#[enum_dispatch]
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
#[enum_dispatch(LayoutSystem)]
pub enum LayoutSystemKind {
    Traditional(TraditionalLayoutSystem),
    Bsp(BspLayoutSystem),
}
