use std::cmp::Ordering;

use objc2_core_foundation::{CGPoint, CGRect};

use crate::actor::app::WindowId;
use crate::common::config::WindowSnappingSettings;
use crate::sys::geometry::CGRectExt;

// less overlap once activated for a sticky
const STICK_RATIO: f64 = 0.6;
// blend overlap and proximity into a single score (overlap still dominates).
const OVERLAP_WEIGHT: f64 = 0.7;
const CENTER_WEIGHT: f64 = 1.0 - OVERLAP_WEIGHT;
// require only a modest improvement before switching to a new candidate.
const SWITCH_DELTA: f64 = 0.04;

#[derive(Debug, Clone, Copy)]
struct CandidateMetrics {
    window: WindowId,
    overlap: f64,
    score: f64,
}

#[derive(Debug, Clone, Copy)]
struct ActiveCandidate {
    window: WindowId,
}

#[derive(Debug, Clone)]
pub struct DragManager {
    dragged_window: Option<WindowId>,
    drag_origin_frame: Option<CGRect>,
    active_candidate: Option<ActiveCandidate>,
    config: WindowSnappingSettings,
}

impl Default for DragManager {
    fn default() -> Self {
        Self::new(WindowSnappingSettings::default())
    }
}

impl DragManager {
    pub fn new(config: WindowSnappingSettings) -> Self {
        Self {
            dragged_window: None,
            drag_origin_frame: None,
            active_candidate: None,
            config,
        }
    }

    pub fn on_frame_change(
        &mut self,
        wid: WindowId,
        new_frame: CGRect,
        candidates: &[(WindowId, CGRect)],
    ) -> Option<WindowId> {
        if self.dragged_window.is_none() {
            self.dragged_window = Some(wid);
            self.drag_origin_frame = Some(new_frame);
            self.active_candidate = None;
        } else if self.dragged_window != Some(wid) {
            self.dragged_window = Some(wid);
            self.drag_origin_frame = Some(new_frame);
            self.active_candidate = None;
        }

        let dragged_area = new_frame.size.width * new_frame.size.height;
        if dragged_area <= 0.0 {
            return None;
        }

        let stick_fraction = (self.config.drag_swap_fraction * STICK_RATIO)
            .clamp(0.0, self.config.drag_swap_fraction);
        let dragged_center = Self::rect_center(new_frame);
        let dragged_diag =
            f64::hypot(new_frame.size.width, new_frame.size.height).max(f64::EPSILON);

        let mut scored: Vec<CandidateMetrics> = Vec::new();
        for (other_wid, other_frame) in candidates {
            if *other_wid == wid {
                continue;
            }

            let inter = new_frame.intersection(other_frame);
            if inter.size.width <= 0.0 || inter.size.height <= 0.0 {
                continue;
            }

            let inter_area = inter.size.width * inter.size.height;

            let other_area = other_frame.size.width * other_frame.size.height;
            let union_area = dragged_area + other_area - inter_area;
            if union_area <= 0.0 {
                continue;
            }
            let iou = inter_area / union_area;
            if iou < stick_fraction {
                continue;
            }

            let other_center = Self::rect_center(*other_frame);
            let distance = f64::hypot(
                dragged_center.x - other_center.x,
                dragged_center.y - other_center.y,
            );

            let other_diag =
                f64::hypot(other_frame.size.width, other_frame.size.height).max(f64::EPSILON);
            let proximity = 1.0 - (distance / (dragged_diag + other_diag)).clamp(0.0, 1.0);
            let score = iou * OVERLAP_WEIGHT + proximity * CENTER_WEIGHT;

            scored.push(CandidateMetrics {
                window: *other_wid,
                overlap: iou,
                score,
            });
        }

        if scored.is_empty() {
            self.active_candidate = None;
            return None;
        }

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        let best = scored[0];

        let active_metrics = self
            .active_candidate
            .and_then(|active| scored.iter().copied().find(|c| c.window == active.window));

        if let Some(active) = active_metrics {
            self.active_candidate = Some(ActiveCandidate { window: active.window });

            if active.window == best.window {
                return None;
            }

            if best.overlap >= self.config.drag_swap_fraction
                && best.score >= active.score + SWITCH_DELTA
            {
                self.active_candidate = Some(ActiveCandidate { window: best.window });
                return Some(best.window);
            }

            return None;
        }

        if best.overlap >= self.config.drag_swap_fraction {
            self.active_candidate = Some(ActiveCandidate { window: best.window });
            return Some(best.window);
        }

        self.active_candidate = None;
        None
    }

    pub fn reset(&mut self) {
        self.dragged_window = None;
        self.drag_origin_frame = None;
        self.active_candidate = None;
    }

    pub fn last_target(&self) -> Option<WindowId> {
        self.active_candidate.map(|candidate| candidate.window)
    }

    pub fn dragged(&self) -> Option<WindowId> {
        self.dragged_window
    }

    pub fn origin_frame(&self) -> Option<CGRect> {
        self.drag_origin_frame
    }

    pub fn update_config(&mut self, config: WindowSnappingSettings) {
        self.config.drag_swap_fraction = if config.drag_swap_fraction <= 0.0 {
            0.5
        } else {
            config.drag_swap_fraction
        };
    }

    fn rect_center(rect: CGRect) -> CGPoint {
        CGPoint::new(
            rect.origin.x + rect.size.width * 0.5,
            rect.origin.y + rect.size.height * 0.5,
        )
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    use super::*;
    use crate::actor::app::WindowId;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width: w, height: h },
        }
    }

    #[test]
    fn selects_candidate_based_on_scored_overlap() {
        let mut dm = DragManager::new(WindowSnappingSettings { drag_swap_fraction: 0.3 });

        let dragged = rect(0.0, 0.0, 100.0, 100.0);
        let wid = WindowId::new(1, 1);

        let cand_a = (WindowId::new(1, 2), rect(0.0, 0.0, 40.0, 100.0)); // 40%
        let cand_b = (WindowId::new(1, 3), rect(0.0, 0.0, 60.0, 100.0)); // 60%

        let chosen = dm.on_frame_change(wid, dragged, &[cand_a, cand_b]);
        assert_eq!(chosen, Some(WindowId::new(1, 3)));
    }

    #[test]
    fn respects_last_target_to_avoid_repeats() {
        let mut dm = DragManager::new(WindowSnappingSettings { drag_swap_fraction: 0.25 });
        let wid = WindowId::new(1, 10);
        let dragged = rect(0.0, 0.0, 200.0, 100.0);

        let cand = (WindowId::new(1, 20), rect(0.0, 0.0, 100.0, 100.0)); // 50% overlap

        let chosen1 = dm.on_frame_change(wid, dragged, &[cand]);
        assert_eq!(chosen1, Some(WindowId::new(1, 20)));

        let chosen2 = dm.on_frame_change(wid, dragged, &[cand]);
        assert_eq!(chosen2, None);
    }

    #[test]
    fn clears_active_target_when_overlap_is_lost() {
        let mut dm = DragManager::new(WindowSnappingSettings { drag_swap_fraction: 0.2 });
        let wid = WindowId::new(1, 42);
        let dragged = rect(0.0, 0.0, 100.0, 100.0);
        let cand = (WindowId::new(1, 99), rect(0.0, 0.0, 60.0, 100.0));

        let chosen = dm.on_frame_change(wid, dragged, &[cand]);
        assert_eq!(chosen, Some(WindowId::new(1, 99)));
        assert_eq!(dm.last_target(), Some(WindowId::new(1, 99)));

        let moved = rect(200.0, 0.0, 100.0, 100.0);
        let cleared = dm.on_frame_change(wid, moved, &[cand]);
        assert!(cleared.is_none());
        assert!(dm.last_target().is_none());
    }

    #[test]
    fn hysteresis_keeps_candidate_when_overlap_drops_slightly() {
        let mut dm = DragManager::new(WindowSnappingSettings { drag_swap_fraction: 0.4 });
        let wid = WindowId::new(5, 1);
        let dragged = rect(0.0, 0.0, 100.0, 100.0);
        let cand = (WindowId::new(5, 2), rect(0.0, 0.0, 50.0, 100.0)); // 50%

        let chosen = dm.on_frame_change(wid, dragged, &[cand]);
        assert_eq!(chosen, Some(WindowId::new(5, 2)));

        let shifted = rect(20.0, 0.0, 100.0, 100.0); // 30% overlap
        let result = dm.on_frame_change(wid, shifted, &[cand]);
        assert!(result.is_none());
        assert_eq!(dm.last_target(), Some(WindowId::new(5, 2)));
    }

    #[test]
    fn switches_only_when_new_candidate_is_meaningfully_better() {
        let mut dm = DragManager::new(WindowSnappingSettings { drag_swap_fraction: 0.3 });
        let wid = WindowId::new(7, 1);
        let dragged = rect(0.0, 0.0, 120.0, 100.0);

        let cand_a = (WindowId::new(7, 2), rect(0.0, 0.0, 60.0, 100.0)); // 50%
        let cand_b = (WindowId::new(7, 3), rect(0.0, 0.0, 68.0, 100.0)); // 56.6%

        assert_eq!(
            dm.on_frame_change(wid, dragged, &[cand_a, cand_b]),
            Some(WindowId::new(7, 3))
        );

        let cand_a_shifted = (WindowId::new(7, 2), rect(0.0, 0.0, 66.0, 100.0)); // 55%
        let result = dm.on_frame_change(wid, dragged, &[cand_a_shifted, cand_b]);
        assert!(result.is_none());
        assert_eq!(dm.last_target(), Some(WindowId::new(7, 3)));

        let cand_a_dominant = (WindowId::new(7, 2), rect(-10.0, 0.0, 120.0, 100.0)); // 100% overlap
        let switched = dm.on_frame_change(wid, dragged, &[cand_a_dominant, cand_b]);
        assert_eq!(switched, Some(WindowId::new(7, 2)));
        assert_eq!(dm.last_target(), Some(WindowId::new(7, 2)));
    }
}
