use objc2_core_foundation::{CGPoint, CGRect, CGSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HideCorner {
    BottomLeft,
    #[default]
    BottomRight,
}

impl HideCorner {
    pub fn opposite(self) -> Self {
        match self {
            Self::BottomLeft => Self::BottomRight,
            Self::BottomRight => Self::BottomLeft,
        }
    }

    fn side(self) -> HideSide {
        match self {
            Self::BottomLeft => HideSide::Left,
            Self::BottomRight => HideSide::Right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideSide {
    Left,
    Right,
}

/// Pure geometry used to place windows just offscreen: windows of inactive
/// workspaces, and scrolling columns outside the viewport.
pub struct HiddenWindowPlacement;

impl HiddenWindowPlacement {
    const REVEAL_PX: f64 = 1.0;
    const VISIBLE_THRESHOLD_PX: f64 = 3.0;

    /// The x of a window `width` wide parked just past `side` of `screen`. It
    /// keeps a sliver on the screen: macOS moves a window that touches no
    /// display into another display's Space and pulls it back 40pt onto a
    /// screen.
    pub fn parked_x(screen: CGRect, width: f64, side: HideSide) -> f64 {
        match side {
            HideSide::Left => screen.origin.x - width + Self::REVEAL_PX,
            HideSide::Right => screen.max().x - Self::REVEAL_PX,
        }
    }

    fn rect_for_corner(screen: CGRect, window: CGRect, corner: HideCorner) -> CGRect {
        let x = Self::parked_x(screen, window.size.width, corner.side());
        CGRect::new(CGPoint::new(x, screen.max().y - Self::REVEAL_PX), window.size)
    }

    fn intersection_area(a: CGRect, b: CGRect) -> f64 {
        let width = (a.max().x.min(b.max().x) - a.origin.x.max(b.origin.x)).max(0.0);
        let height = (a.max().y.min(b.max().y) - a.origin.y.max(b.origin.y)).max(0.0);
        width * height
    }

    pub fn calculate(
        screen: CGRect,
        window: CGRect,
        preferred_corner: HideCorner,
        other_screens: &[CGRect],
    ) -> CGRect {
        let overlap = |candidate: &CGRect| {
            other_screens
                .iter()
                .filter(|other| **other != screen)
                .map(|other| Self::intersection_area(*candidate, *other))
                .sum::<f64>()
        };
        // A bottom corner lands on any display stacked below; parking just past the
        // side edge, top-aligned and no taller than the display, avoids that.
        let corners = [preferred_corner, preferred_corner.opposite()];
        let bottom = corners.map(|corner| Self::rect_for_corner(screen, window, corner));
        let side = bottom.map(|rect| {
            CGRect::new(
                CGPoint::new(rect.origin.x, screen.origin.y),
                CGSize::new(window.size.width, window.size.height.min(screen.size.height)),
            )
        });
        bottom
            .into_iter()
            .chain(side)
            // Score each candidate once; a comparator otherwise rescans all
            // displays for the current best candidate on every comparison.
            .map(|candidate| (candidate, overlap(&candidate)))
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(candidate, _)| candidate)
            .unwrap_or(window)
    }

    pub fn is_hidden(screen: CGRect, window: CGRect, other_screens: &[CGRect]) -> bool {
        // Most parked windows expose at most a pixel. Check that inexpensive
        // case before evaluating parking candidates against every other display.
        let visible_width =
            (window.max().x.min(screen.max().x) - window.origin.x.max(screen.origin.x)).max(0.0);
        let visible_height =
            (window.max().y.min(screen.max().y) - window.origin.y.max(screen.origin.y)).max(0.0);
        visible_width <= Self::VISIBLE_THRESHOLD_PX
            || visible_height <= Self::VISIBLE_THRESHOLD_PX
            || [HideCorner::BottomLeft, HideCorner::BottomRight]
                .into_iter()
                .any(|corner| Self::calculate(screen, window, corner, other_screens) == window)
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};

    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
    }

    #[test]
    fn anchors_to_requested_corner() {
        let hidden = HiddenWindowPlacement::calculate(
            rect(0.0, 0.0, 1000.0, 800.0),
            rect(10.0, 20.0, 200.0, 100.0),
            HideCorner::BottomRight,
            &[],
        );
        assert_eq!(hidden, rect(999.0, 799.0, 200.0, 100.0));
    }

    #[test]
    fn avoids_an_adjacent_monitor() {
        let screen = rect(0.0, 0.0, 1000.0, 800.0);
        let hidden = HiddenWindowPlacement::calculate(
            screen,
            rect(0.0, 0.0, 200.0, 100.0),
            HideCorner::BottomRight,
            &[rect(1000.0, 0.0, 1000.0, 800.0)],
        );
        assert_eq!(hidden.origin.x, -199.0);
    }

    #[test]
    fn parks_beside_the_display_when_another_display_sits_below() {
        let upper = rect(81.0, -1049.0, 1920.0, 1049.0);
        let lower = rect(0.0, 40.0, 2056.0, 1289.0);
        let window = rect(16.0, 100.0, 1007.0, 1219.0);
        let hidden =
            HiddenWindowPlacement::calculate(upper, window, HideCorner::BottomRight, &[lower]);
        assert_eq!(hidden, rect(2000.0, -1049.0, 1007.0, 1049.0));
        assert!(HiddenWindowPlacement::is_hidden(upper, hidden, &[lower]));
    }
}
