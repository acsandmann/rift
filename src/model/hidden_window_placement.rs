use objc2_core_foundation::{CGPoint, CGRect, CGSize};

/// Where an inactive-workspace window is parked relative to its display.
///
/// The bottom corners leave a one-pixel sliver of the window's top edge on
/// screen and push the rest below the display. That only works when nothing
/// sits below the display: with a vertically stacked arrangement the parked
/// window would land on the lower display, and both macOS and Rift would then
/// treat it as belonging there. The edge placements keep the window inside the
/// display's vertical span and push it sideways instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HideCorner {
    BottomLeft,
    #[default]
    BottomRight,
    /// Just past the right edge, top-aligned with the display.
    RightEdge,
    /// Just past the left edge, top-aligned with the display.
    LeftEdge,
}

impl HideCorner {
    const ALL: [HideCorner; 4] = [
        HideCorner::BottomRight,
        HideCorner::BottomLeft,
        HideCorner::RightEdge,
        HideCorner::LeftEdge,
    ];

    pub fn opposite(self) -> Self {
        match self {
            Self::BottomLeft => Self::BottomRight,
            Self::BottomRight => Self::BottomLeft,
            Self::RightEdge => Self::LeftEdge,
            Self::LeftEdge => Self::RightEdge,
        }
    }

    /// Candidate order: the preferred corner, its mirror, then the remaining
    /// placements. Earlier candidates win ties.
    fn candidates(preferred: HideCorner) -> [HideCorner; 4] {
        let mut ordered = [preferred, preferred.opposite(), preferred, preferred];
        let mut next = 2;
        for corner in Self::ALL {
            if corner != preferred && corner != preferred.opposite() {
                ordered[next] = corner;
                next += 1;
            }
        }
        ordered
    }
}

/// Pure geometry used to place inactive-workspace windows just offscreen.
pub struct HiddenWindowPlacement;

impl HiddenWindowPlacement {
    const REVEAL_PX: f64 = 1.0;
    const VISIBLE_THRESHOLD_PX: f64 = 3.0;

    fn rect_for_corner(screen: CGRect, window: CGRect, corner: HideCorner) -> CGRect {
        match corner {
            HideCorner::BottomLeft => CGRect::new(
                CGPoint::new(
                    screen.origin.x - window.size.width + Self::REVEAL_PX,
                    screen.max().y - Self::REVEAL_PX,
                ),
                window.size,
            ),
            HideCorner::BottomRight => CGRect::new(
                CGPoint::new(
                    screen.max().x - Self::REVEAL_PX,
                    screen.max().y - Self::REVEAL_PX,
                ),
                window.size,
            ),
            // Edge placements must not poke out below the display, or they would
            // reintroduce the overlap they exist to avoid; clamp the height.
            HideCorner::RightEdge => CGRect::new(
                CGPoint::new(screen.max().x - Self::REVEAL_PX, screen.origin.y),
                CGSize::new(window.size.width, window.size.height.min(screen.size.height)),
            ),
            HideCorner::LeftEdge => CGRect::new(
                CGPoint::new(
                    screen.origin.x - window.size.width + Self::REVEAL_PX,
                    screen.origin.y,
                ),
                CGSize::new(window.size.width, window.size.height.min(screen.size.height)),
            ),
        }
    }

    fn intersection_area(a: CGRect, b: CGRect) -> f64 {
        let width = (a.max().x.min(b.max().x) - a.origin.x.max(b.origin.x)).max(0.0);
        let height = (a.max().y.min(b.max().y) - a.origin.y.max(b.origin.y)).max(0.0);
        width * height
    }

    /// Pick the parked frame that overlaps other displays the least, preferring
    /// `preferred_corner` and then its mirror on ties.
    pub fn calculate(
        screen: CGRect,
        window: CGRect,
        preferred_corner: HideCorner,
        other_screens: &[CGRect],
    ) -> CGRect {
        let overlap = |candidate: CGRect| {
            other_screens
                .iter()
                .filter(|other| **other != screen)
                .map(|other| Self::intersection_area(candidate, *other))
                .sum::<f64>()
        };
        let mut best: Option<(CGRect, f64)> = None;
        for corner in HideCorner::candidates(preferred_corner) {
            let candidate = Self::rect_for_corner(screen, window, corner);
            let area = overlap(candidate);
            if best.is_none_or(|(_, best_area)| area < best_area) {
                best = Some((candidate, area));
            }
            if area == 0.0 {
                break;
            }
        }
        best.map(|(rect, _)| rect).unwrap_or(window)
    }

    /// Whether `window` is parked offscreen relative to `screen`: it matches one
    /// of the parked placements, or at most a sliver of it is visible in either
    /// dimension.
    pub fn is_hidden(screen: CGRect, window: CGRect, other_screens: &[CGRect]) -> bool {
        HideCorner::ALL
            .into_iter()
            .any(|corner| Self::rect_for_corner(screen, window, corner) == window)
            || Self::calculate(screen, window, HideCorner::BottomRight, other_screens) == window
            || {
                let visible_width = (window.max().x.min(screen.max().x)
                    - window.origin.x.max(screen.origin.x))
                .max(0.0);
                let visible_height = (window.max().y.min(screen.max().y)
                    - window.origin.y.max(screen.origin.y))
                .max(0.0);
                visible_width <= Self::VISIBLE_THRESHOLD_PX
                    || visible_height <= Self::VISIBLE_THRESHOLD_PX
            }
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
        // External display stacked directly above a laptop panel, as macOS
        // reports it: the upper display has negative y and its bottom edge
        // touches the lower display's top.
        let upper = rect(81.0, -1049.0, 1920.0, 1049.0);
        let lower = rect(0.0, 40.0, 2056.0, 1289.0);
        let window = rect(91.0, -1049.0, 1900.0, 1050.0);

        let hidden =
            HiddenWindowPlacement::calculate(upper, window, HideCorner::BottomRight, &[lower]);

        assert_eq!(hidden.origin, CGPoint::new(upper.max().x - 1.0, upper.origin.y));
        assert_eq!(
            hidden.size,
            CGSize::new(1900.0, 1049.0),
            "edge placement clamps the height to the display"
        );
        assert_eq!(HiddenWindowPlacement::intersection_area(hidden, lower), 0.0);
        assert!(HiddenWindowPlacement::is_hidden(upper, hidden, &[lower]));

        // Bottom placements stay the default when nothing is below.
        let alone = HiddenWindowPlacement::calculate(upper, window, HideCorner::BottomRight, &[]);
        assert_eq!(alone.origin.y, upper.max().y - 1.0);
    }

    #[test]
    fn tall_windows_do_not_spill_onto_the_display_below() {
        let upper = rect(81.0, -1049.0, 1920.0, 1049.0);
        let lower = rect(0.0, 40.0, 2056.0, 1289.0);
        let tall = rect(16.0, 100.0, 1007.0, 1219.0);

        let hidden =
            HiddenWindowPlacement::calculate(upper, tall, HideCorner::BottomRight, &[lower]);

        assert_eq!(HiddenWindowPlacement::intersection_area(hidden, lower), 0.0);
        assert!(hidden.max().y <= upper.max().y);
    }

    #[test]
    fn sliver_visible_in_either_dimension_counts_as_hidden() {
        let screen = rect(0.0, 0.0, 1000.0, 800.0);
        assert!(HiddenWindowPlacement::is_hidden(
            screen,
            rect(998.0, 100.0, 300.0, 300.0),
            &[]
        ));
        assert!(HiddenWindowPlacement::is_hidden(
            screen,
            rect(100.0, 798.0, 300.0, 300.0),
            &[]
        ));
        assert!(!HiddenWindowPlacement::is_hidden(
            screen,
            rect(900.0, 700.0, 300.0, 300.0),
            &[]
        ));
    }
}
