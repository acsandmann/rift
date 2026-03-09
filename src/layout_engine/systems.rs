use enum_dispatch::enum_dispatch;
use objc2_core_foundation::CGRect;
use serde::{Deserialize, Serialize};

use crate::actor::app::{WindowId, pid_t};
use crate::common::collections::HashMap;
use crate::layout_engine::{Direction, LayoutKind};

slotmap::new_key_type! { pub struct LayoutId; }

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowLayoutConstraints {
    pub is_resizable: bool,
    pub locked_width: f64,
    pub locked_height: f64,
    pub min_width: f64,
    pub min_height: f64,
    pub max_width: f64,
    pub max_height: f64,
}

impl WindowLayoutConstraints {
    pub fn normalized(self) -> Self {
        let clean = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
        let min_width = clean(self.min_width);
        let min_height = clean(self.min_height);
        let mut max_width = clean(self.max_width);
        let mut max_height = clean(self.max_height);
        if max_width > 0.0 && max_width < min_width {
            max_width = min_width;
        }
        if max_height > 0.0 && max_height < min_height {
            max_height = min_height;
        }
        Self {
            is_resizable: self.is_resizable,
            locked_width: clean(self.locked_width),
            locked_height: clean(self.locked_height),
            min_width,
            min_height,
            max_width,
            max_height,
        }
    }

    pub fn min_for_axis(self, horizontal: bool) -> f64 {
        if horizontal {
            self.min_width
        } else {
            self.min_height
        }
    }

    pub fn max_for_axis(self, horizontal: bool) -> f64 {
        if horizontal {
            self.max_width
        } else {
            self.max_height
        }
    }

    pub fn fixed_for_axis(self, horizontal: bool) -> Option<f64> {
        let locked = if horizontal {
            self.locked_width
        } else {
            self.locked_height
        };
        let min = self.min_for_axis(horizontal);
        let max = self.max_for_axis(horizontal);
        // Axis-specific lock: when min/max collapse to the same positive value,
        // treat that axis as fixed even if the window is generally resizable.
        if min > 0.0 && max > 0.0 && (min - max).abs() <= f64::EPSILON {
            return Some(max);
        }
        if !self.is_resizable {
            return (locked > 0.0).then_some(locked);
        }
        None
    }

    pub fn resizable_for_axis(self, horizontal: bool) -> bool {
        self.fixed_for_axis(horizontal).is_none()
    }

    pub fn resizable_any_axis(self) -> bool {
        self.resizable_for_axis(true) || self.resizable_for_axis(false)
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
        constraints: &HashMap<WindowId, WindowLayoutConstraints>,
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
pub(crate) mod constraints;
pub use bsp::BspLayoutSystem;
mod master_stack;
pub use master_stack::MasterStackLayoutSystem;
mod scrolling;
pub use scrolling::ScrollingLayoutSystem;

#[cfg(test)]
mod tests {
    use super::WindowLayoutConstraints;

    #[test]
    fn axis_specific_fixed_detection_supports_one_axis_locked_other_resizable() {
        let c = WindowLayoutConstraints {
            is_resizable: true,
            locked_width: 700.0,
            locked_height: 400.0,
            min_width: 723.0,
            min_height: 470.0,
            max_width: 723.0,
            max_height: 0.0,
        }
        .normalized();

        assert_eq!(c.fixed_for_axis(true), Some(723.0));
        assert_eq!(c.fixed_for_axis(false), None);
        assert!(!c.resizable_for_axis(true));
        assert!(c.resizable_for_axis(false));
        assert!(c.resizable_any_axis());
    }

    #[test]
    fn non_resizable_zero_locked_size_is_not_treated_as_fixed() {
        let c = WindowLayoutConstraints {
            is_resizable: false,
            locked_width: 0.0,
            locked_height: 0.0,
            min_width: 0.0,
            min_height: 0.0,
            max_width: 0.0,
            max_height: 0.0,
        }
        .normalized();

        assert_eq!(c.fixed_for_axis(true), None);
        assert_eq!(c.fixed_for_axis(false), None);
        assert!(c.resizable_for_axis(true));
        assert!(c.resizable_for_axis(false));
    }

    #[test]
    fn non_resizable_positive_locked_size_remains_fixed() {
        let c = WindowLayoutConstraints {
            is_resizable: false,
            locked_width: 640.0,
            locked_height: 360.0,
            min_width: 0.0,
            min_height: 0.0,
            max_width: 0.0,
            max_height: 0.0,
        }
        .normalized();

        assert_eq!(c.fixed_for_axis(true), Some(640.0));
        assert_eq!(c.fixed_for_axis(false), Some(360.0));
        assert!(!c.resizable_for_axis(true));
        assert!(!c.resizable_for_axis(false));
        assert!(!c.resizable_any_axis());
    }

    #[test]
    fn positive_max_only_constraint_is_not_treated_as_fixed() {
        let c = WindowLayoutConstraints {
            is_resizable: true,
            locked_width: 0.0,
            locked_height: 0.0,
            min_width: 0.0,
            min_height: 0.0,
            max_width: 600.0,
            max_height: 480.0,
        }
        .normalized();

        assert_eq!(c.fixed_for_axis(true), None);
        assert_eq!(c.fixed_for_axis(false), None);
        assert_eq!(c.max_for_axis(true), 600.0);
        assert_eq!(c.max_for_axis(false), 480.0);
        assert!(c.resizable_for_axis(true));
        assert!(c.resizable_for_axis(false));
    }
}
mod stack;
pub use stack::StackLayoutSystem;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[derive(Debug)]
#[enum_dispatch(LayoutSystem)]
pub enum LayoutSystemKind {
    Traditional(TraditionalLayoutSystem),
    Bsp(BspLayoutSystem),
    MasterStack(MasterStackLayoutSystem),
    Scrolling(ScrollingLayoutSystem),
    Stack(StackLayoutSystem),
}
