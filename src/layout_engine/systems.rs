use std::sync::OnceLock;

use enum_dispatch::enum_dispatch;
use objc2_core_foundation::{CGRect, CGSize};
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
use crate::layout_engine::{Direction, LayoutKind};

slotmap::new_key_type! { pub struct LayoutId; }

#[derive(Clone, Copy)]
pub(crate) struct LayoutCalcInputs<'a> {
    pub(crate) screen: CGRect,
    pub(crate) stack_offset: f64,
    pub(crate) gaps: &'a crate::common::config::GapSettings,
    pub(crate) stack_line_thickness: f64,
    pub(crate) stack_line_horiz: crate::common::config::HorizontalPlacement,
    pub(crate) stack_line_vert: crate::common::config::VerticalPlacement,
}

impl<'a> LayoutCalcInputs<'a> {
    pub(crate) fn new(
        screen: CGRect,
        stack_offset: f64,
        gaps: &'a crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Self {
        Self {
            screen,
            stack_offset,
            gaps,
            stack_line_thickness,
            stack_line_horiz,
            stack_line_vert,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LayoutConstraints<'a> {
    pub(crate) fixed_sizes: &'a HashMap<WindowId, CGSize>,
}

impl<'a> LayoutConstraints<'a> {
    pub(crate) fn with_fixed_sizes(fixed_sizes: &'a HashMap<WindowId, CGSize>) -> Self {
        Self { fixed_sizes }
    }
}

impl LayoutConstraints<'static> {
    pub(crate) fn unconstrained() -> Self {
        static EMPTY_FIXED_SIZES: OnceLock<HashMap<WindowId, CGSize>> = OnceLock::new();
        Self::with_fixed_sizes(EMPTY_FIXED_SIZES.get_or_init(HashMap::default))
    }
}

#[enum_dispatch]
pub trait LayoutSystem: Serialize + for<'de> Deserialize<'de> {
    fn create_layout(&mut self) -> LayoutId;
    fn clone_layout(&mut self, layout: LayoutId) -> LayoutId;
    fn remove_layout(&mut self, layout: LayoutId);

    fn draw_tree(&self, layout: LayoutId) -> String;

    fn calculate_layout(
        &self,
        layout: LayoutId,
        screen: CGRect,
        stack_offset: f64,
        gaps: &crate::common::config::GapSettings,
        stack_line_thickness: f64,
        stack_line_horiz: crate::common::config::HorizontalPlacement,
        stack_line_vert: crate::common::config::VerticalPlacement,
    ) -> Vec<(WindowId, CGRect)>;

    fn selected_window(&self, layout: LayoutId) -> Option<WindowId>;
    fn visible_windows_in_layout(&self, layout: LayoutId) -> Vec<WindowId>;
    fn visible_windows_under_selection(&self, layout: LayoutId) -> Vec<WindowId>;
    fn ascend_selection(&mut self, layout: LayoutId) -> bool;
    fn descend_selection(&mut self, layout: LayoutId) -> bool;
    fn move_focus(
        &mut self,
        layout: LayoutId,
        direction: Direction,
    ) -> (Option<WindowId>, Vec<WindowId>);
    fn window_in_direction(&self, layout: LayoutId, direction: Direction) -> Option<WindowId>;
    fn add_window_after_selection(&mut self, layout: LayoutId, wid: WindowId);
    fn remove_window(&mut self, wid: WindowId);
    fn remove_windows_for_app(&mut self, pid: pid_t);
    fn set_windows_for_app(&mut self, layout: LayoutId, pid: pid_t, desired: Vec<WindowId>);
    fn has_windows_for_app(&self, layout: LayoutId, pid: pid_t) -> bool;
    fn contains_window(&self, layout: LayoutId, wid: WindowId) -> bool;
    fn select_window(&mut self, layout: LayoutId, wid: WindowId) -> bool;
    fn on_window_resized(
        &mut self,
        layout: LayoutId,
        wid: WindowId,
        old_frame: CGRect,
        new_frame: CGRect,
        screen: CGRect,
        gaps: &crate::common::config::GapSettings,
    );

    fn swap_windows(&mut self, layout: LayoutId, a: WindowId, b: WindowId) -> bool;

    fn move_selection(&mut self, layout: LayoutId, direction: Direction) -> bool;
    fn move_selection_to_layout_after_selection(
        &mut self,
        from_layout: LayoutId,
        to_layout: LayoutId,
    );
    fn split_selection(&mut self, layout: LayoutId, kind: LayoutKind);

    fn toggle_fullscreen_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId>;
    fn toggle_fullscreen_within_gaps_of_selection(&mut self, layout: LayoutId) -> Vec<WindowId>;
    fn has_any_fullscreen_node(&self, layout: LayoutId) -> bool;

    fn join_selection_with_direction(&mut self, layout: LayoutId, direction: Direction);
    fn apply_stacking_to_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId>;
    fn unstack_parent_of_selection(
        &mut self,
        layout: LayoutId,
        default_orientation: crate::common::config::StackDefaultOrientation,
    ) -> Vec<WindowId>;
    fn parent_of_selection_is_stacked(&self, layout: LayoutId) -> bool;
    fn unjoin_selection(&mut self, _layout: LayoutId);
    fn resize_selection_by(&mut self, layout: LayoutId, amount: f64);
    fn rebalance(&mut self, layout: LayoutId);
    fn toggle_tile_orientation(&mut self, layout: LayoutId);
}

mod traditional;
pub use traditional::TraditionalLayoutSystem;
mod bsp;
pub use bsp::BspLayoutSystem;
mod master_stack;
pub use master_stack::MasterStackLayoutSystem;
mod scrolling;
pub use scrolling::ScrollingLayoutSystem;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[derive(Debug)]
#[enum_dispatch(LayoutSystem)]
pub enum LayoutSystemKind {
    Traditional(TraditionalLayoutSystem),
    Bsp(BspLayoutSystem),
    MasterStack(MasterStackLayoutSystem),
    Scrolling(ScrollingLayoutSystem),
}

impl LayoutSystemKind {
    pub(crate) fn calculate_layout_constrained(
        &self,
        layout: LayoutId,
        inputs: LayoutCalcInputs<'_>,
        constraints: LayoutConstraints<'_>,
    ) -> Vec<(WindowId, CGRect)> {
        match self {
            LayoutSystemKind::Traditional(system) => {
                system.calculate_layout_constrained(layout, inputs, constraints)
            }
            LayoutSystemKind::Bsp(system) => {
                system.calculate_layout_constrained(layout, inputs, constraints)
            }
            LayoutSystemKind::MasterStack(system) => {
                system.calculate_layout_constrained(layout, inputs, constraints)
            }
            LayoutSystemKind::Scrolling(system) => {
                system.calculate_layout_constrained(layout, inputs, constraints)
            }
        }
    }
}
