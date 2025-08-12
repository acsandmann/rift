use objc2_core_foundation::CGRect;

use crate::sys::geometry::Round;

pub struct StackLayoutResult {
    pub(crate) container_rect: CGRect,
    pub(crate) _window_count: usize,
    pub(crate) stack_offset: f64,
    pub(crate) is_horizontal: bool,
    pub(crate) window_width: f64,
    pub(crate) window_height: f64,
}

impl StackLayoutResult {
    pub fn new(
        container_rect: CGRect,
        window_count: usize,
        stack_offset: f64,
        is_horizontal: bool,
    ) -> Self {
        let total_offset_space = if window_count > 0 {
            (window_count - 1) as f64 * stack_offset
        } else {
            0.0
        };
        let (window_width, window_height) = if is_horizontal {
            (
                (container_rect.size.width - total_offset_space).max(100.0),
                container_rect.size.height.max(100.0),
            )
        } else {
            (
                container_rect.size.width.max(100.0),
                (container_rect.size.height - total_offset_space).max(100.0),
            )
        };
        Self {
            container_rect,
            _window_count: window_count,
            stack_offset,
            is_horizontal,
            window_width,
            window_height,
        }
    }

    pub fn get_frame_for_index(&self, index: usize) -> CGRect {
        use objc2_core_foundation::{CGPoint, CGSize};
        let offset_amount = index as f64 * self.stack_offset;
        let (x_offset, y_offset) = if self.is_horizontal {
            (offset_amount, 0.0)
        } else {
            (0.0, offset_amount)
        };
        CGRect {
            origin: CGPoint {
                x: self.container_rect.origin.x + x_offset,
                y: self.container_rect.origin.y + y_offset,
            },
            size: CGSize {
                width: self.window_width,
                height: self.window_height,
            },
        }
        .round()
    }

    pub fn get_focused_frame_for_index(&self, index: usize, _focused_idx: usize) -> CGRect {
        use objc2_core_foundation::{CGPoint, CGSize};
        const FOCUS_SIZE_INCREASE: f64 = 10.0;
        const FOCUS_OFFSET_DECREASE: f64 = 5.0;
        let offset_amount = index as f64 * self.stack_offset;
        let (mut origin_x, mut origin_y) = if self.is_horizontal {
            (
                self.container_rect.origin.x + offset_amount - FOCUS_OFFSET_DECREASE,
                self.container_rect.origin.y - FOCUS_OFFSET_DECREASE,
            )
        } else {
            (
                self.container_rect.origin.x - FOCUS_OFFSET_DECREASE,
                self.container_rect.origin.y + offset_amount - FOCUS_OFFSET_DECREASE,
            )
        };
        if self.is_horizontal {
            if index == 0 {
                origin_x = self.container_rect.origin.x;
            }
            let max_x = self.container_rect.origin.x + self.container_rect.size.width
                - (self.window_width + FOCUS_SIZE_INCREASE);
            origin_x = origin_x.min(max_x);
        }
        if !self.is_horizontal {
            if index == 0 {
                origin_y = self.container_rect.origin.y;
            }
            let max_y = self.container_rect.origin.y + self.container_rect.size.height
                - (self.window_height + FOCUS_SIZE_INCREASE);
            origin_y = origin_y.min(max_y);
        }
        let screen_x = self.container_rect.origin.x;
        let screen_y = self.container_rect.origin.y;
        let screen_width = self.container_rect.size.width;
        let screen_height = self.container_rect.size.height;
        let width = (self.window_width + FOCUS_SIZE_INCREASE).min(screen_width);
        let height = (self.window_height + FOCUS_SIZE_INCREASE).min(screen_height);
        let x = origin_x.clamp(screen_x, screen_x + screen_width - width);
        let y = origin_y.clamp(screen_y, screen_y + screen_height - height);
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
        .round()
    }
}
