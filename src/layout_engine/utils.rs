use objc2_core_foundation::CGRect;

pub fn compute_tiling_area(screen: CGRect, gaps: &crate::common::config::GapSettings) -> CGRect {
    use objc2_core_foundation::{CGPoint, CGSize};

    use crate::sys::geometry::Round;
    if gaps.outer.top == 0.0
        && gaps.outer.left == 0.0
        && gaps.outer.bottom == 0.0
        && gaps.outer.right == 0.0
    {
        screen
    } else {
        CGRect {
            origin: CGPoint {
                x: screen.origin.x + gaps.outer.left,
                y: screen.origin.y + gaps.outer.top,
            },
            size: CGSize {
                width: (screen.size.width - gaps.outer.left - gaps.outer.right).max(0.0),
                height: (screen.size.height - gaps.outer.top - gaps.outer.bottom).max(0.0),
            },
        }
        .round()
    }
}
