use super::WindowLayoutConstraints;
use crate::layout_engine::LayoutKind;

type AxisSummary = (f64, Option<f64>, Option<f64>, bool);
// Both axes are prepared bottom-up; absent nodes do not participate.
pub(super) type ConstraintCache =
    crate::common::collections::HashMap<crate::model::tree::NodeId, [AxisSummary; 2]>;

// Shared by Traditional containers and BSP splits. A sole child contributes
// exactly its own constraints; gaps and stack decorations belong to projections.
pub(super) fn container_axis_constraints(
    children: impl Iterator<Item = AxisSummary>,
    kind: LayoutKind,
    horizontal: bool,
    gap: f64,
    stack_offset: f64,
    reserve: f64,
) -> AxisSummary {
    let aligned =
        (kind.orientation() == crate::layout_engine::Orientation::Horizontal) == horizontal;
    let stacked = kind.is_stacked();
    let mut count = 0;
    let mut axis = AxisConstraints {
        fixed: Some(0.0),
        max: Some(0.0),
        ..Default::default()
    };
    let mut focused: f64 = 0.0;
    let combine = |a: f64, b: f64| {
        if aligned && !stacked {
            a + b.max(0.0)
        } else {
            a.max(b.max(0.0))
        }
    };
    for (min, fixed, max, grow) in children {
        count += 1;
        axis.min = combine(axis.min, min);
        axis.fixed = axis.fixed.zip(fixed).map(|(a, b)| combine(a, b));
        axis.max = axis.max.zip(max).map(|(a, b)| combine(a, b));
        axis.can_grow |= grow;
        focused = focused.max(fixed.unwrap_or(min));
    }
    if count == 0 {
        return (0.0, None, None, true);
    }
    let extra = if count == 1 {
        0.0
    } else if !aligned {
        reserve
    } else if stacked {
        stack_offset.max(0.0) * (count - 1) as f64 + reserve
    } else {
        gap * (count - 1) as f64
    };
    if count > 1 && stacked && aligned {
        axis.min = focused;
        axis.fixed = None;
    }
    (
        axis.min + extra,
        axis.fixed.map(|v| v + extra),
        axis.max.map(|v| v + extra),
        axis.can_grow,
    )
}

/// Fit a leaf into its assigned slot without changing the layout's sizing policy.
pub(super) fn constrain_frame(
    mut frame: objc2_core_foundation::CGRect,
    axes: impl Fn(bool) -> AxisSummary,
) -> objc2_core_foundation::CGRect {
    for horizontal in [true, false] {
        let (min, fixed, max, _) = axes(horizontal);
        let size = if horizontal {
            &mut frame.size.width
        } else {
            &mut frame.size.height
        };
        let desired = fixed.unwrap_or(*size).max(min);
        *size = max.map_or(desired, |bound| desired.min(bound)).min(*size).max(0.0);
    }
    frame
}

pub(super) fn traditional_leaf_axis_constraints(
    c: WindowLayoutConstraints,
    horizontal: bool,
) -> (f64, Option<f64>, Option<f64>, bool) {
    let c = c.normalized();
    let min = c.min_for_axis(horizontal);
    let max = (c.max_for_axis(horizontal) > 0.0).then(|| c.max_for_axis(horizontal));
    if c.is_resizable {
        if let Some(fixed) = c.fixed_for_axis(horizontal) {
            return (min, Some(fixed), Some(fixed), false);
        }
        // Some apps report transient or overly conservative min/max bounds while still
        // being user-resizable. In traditional tiling, honoring those bounds at split
        // time causes visibly uneven insertion (e.g. 2:1 right after a 50/50 split).
        return (0.0, None, max, true);
    }
    let fixed = c.fixed_for_axis(horizontal).filter(|v| *v > 0.0);
    let can_grow = fixed.is_none();
    (min, fixed, max.or(fixed), can_grow)
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AxisConstraints {
    pub min: f64,
    pub fixed: Option<f64>,
    pub max: Option<f64>,
    pub weight: f64,
    pub can_grow: bool,
}

fn sanitize(v: f64) -> f64 { if v.is_finite() { v.max(0.0) } else { 0.0 } }

/// Solve 1D segment lengths for a container axis.
///
/// Rules:
/// - never negative
/// - `min` is a lower bound unless physically infeasible (then minima are scaled down proportionally)
/// - `fixed` values are enforced before distributing remainder
/// - `max` caps growth when positive
/// - growable nodes preserve their final weighted proportions whenever their bounds allow it
/// - zero-weight growable nodes share equally when no positive weights are present
/// - if nothing can grow, remainder becomes blank space
pub(crate) fn solve_axis_lengths(items: &[AxisConstraints], usable: f64) -> Vec<f64> {
    if items.is_empty() {
        return Vec::new();
    }
    let usable = sanitize(usable);
    let n = items.len();
    let mut mins: Vec<f64> = items.iter().map(|i| sanitize(i.min)).collect();
    let mut fixed: Vec<Option<f64>> =
        items.iter().map(|i| i.fixed.map(sanitize).filter(|v| v.is_finite())).collect();
    let maxs: Vec<Option<f64>> = items
        .iter()
        .map(|i| i.max.map(sanitize).filter(|v| *v > 0.0 && v.is_finite()))
        .collect();
    let weights: Vec<f64> = items.iter().map(|i| sanitize(i.weight)).collect();
    let can_grow: Vec<bool> = items.iter().map(|i| i.can_grow).collect();

    for (idx, f) in fixed.iter_mut().enumerate() {
        if let Some(v) = f {
            if *v < mins[idx] {
                *v = mins[idx];
            }
            if let Some(max) = maxs[idx] {
                if *v > max {
                    *v = max;
                }
            }
        }
    }

    let fixed_sum: f64 = fixed.iter().flatten().copied().sum();
    if fixed_sum > usable && fixed_sum > 0.0 {
        let scale = usable / fixed_sum;
        let mut lengths = vec![0.0; n];
        for idx in 0..n {
            if let Some(v) = fixed[idx] {
                lengths[idx] = v * scale;
            }
        }
        return lengths;
    }

    let min_indices: Vec<usize> = (0..n).filter(|&idx| fixed[idx].is_none()).collect();
    let min_sum: f64 = min_indices.iter().map(|&idx| mins[idx]).sum();
    let remaining_for_mins = (usable - fixed_sum).max(0.0);
    if min_sum > remaining_for_mins && min_sum > 0.0 {
        let scale = remaining_for_mins / min_sum;
        for &idx in &min_indices {
            mins[idx] *= scale;
        }
    }

    let mut lengths = vec![0.0; n];
    let mut remaining = usable;
    for idx in 0..n {
        if let Some(v) = fixed[idx] {
            let assigned = v.min(remaining);
            lengths[idx] = assigned;
            remaining = (remaining - assigned).max(0.0);
        }
    }

    // Non-growable segments stay at their minimum. Growable segments are solved from their
    // *final* weighted sizes, clamped to their bounds. Seeding every segment with its minimum and
    // distributing only the remainder would skew equal-weight splits whenever the minima differ.
    let mut growable: Vec<usize> = Vec::new();
    for idx in 0..n {
        if fixed[idx].is_none() {
            if can_grow[idx] {
                growable.push(idx);
            } else {
                let assigned = mins[idx].min(remaining);
                lengths[idx] = assigned;
                remaining = (remaining - assigned).max(0.0);
            }
        }
    }

    while !growable.is_empty() && remaining > f64::EPSILON {
        let total_weight: f64 = growable.iter().map(|&idx| weights[idx]).sum();
        let use_equal_weights = total_weight <= f64::EPSILON;
        let divisor = if use_equal_weights {
            growable.len() as f64
        } else {
            total_weight
        };

        let mut clamped = Vec::new();
        for &idx in &growable {
            let weight = if use_equal_weights { 1.0 } else { weights[idx] };
            let proposed = remaining * weight / divisor;
            let max = maxs[idx].map(|value| value.max(mins[idx]));
            if proposed + f64::EPSILON < mins[idx] {
                lengths[idx] = mins[idx].min(remaining);
                clamped.push(idx);
            } else if let Some(max) = max
                && proposed > max + f64::EPSILON
            {
                lengths[idx] = max.min(remaining);
                clamped.push(idx);
            }
        }

        if clamped.is_empty() {
            for &idx in &growable {
                let weight = if use_equal_weights { 1.0 } else { weights[idx] };
                lengths[idx] = remaining * weight / divisor;
            }
            remaining = 0.0;
            break;
        }

        let clamped_sum: f64 = clamped.iter().map(|&idx| lengths[idx]).sum();
        remaining = (remaining - clamped_sum).max(0.0);
        growable.retain(|idx| !clamped.contains(idx));
    }

    if remaining <= f64::EPSILON {
        let used: f64 = lengths.iter().sum();
        let drift = usable - used;
        if drift.abs() > f64::EPSILON {
            if let Some(idx) = (0..n).rfind(|&idx| lengths[idx] > 0.0) {
                lengths[idx] = (lengths[idx] + drift).max(0.0);
            }
        }
    }

    lengths
}

#[cfg(test)]
mod tests {
    use super::{AxisConstraints, solve_axis_lengths};

    #[test]
    fn scales_non_fixed_minima_after_reserving_fixed_segments() {
        let solved = solve_axis_lengths(
            &[
                AxisConstraints {
                    min: 0.0,
                    fixed: Some(600.0),
                    max: None,
                    weight: 1.0,
                    can_grow: false,
                },
                AxisConstraints {
                    min: 300.0,
                    fixed: None,
                    max: None,
                    weight: 1.0,
                    can_grow: true,
                },
                AxisConstraints {
                    min: 300.0,
                    fixed: None,
                    max: None,
                    weight: 1.0,
                    can_grow: true,
                },
            ],
            1000.0,
        );

        assert_eq!(solved.len(), 3);
        assert!((solved[0] - 600.0).abs() < 0.001);
        assert!((solved[1] - 200.0).abs() < 0.001);
        assert!((solved[2] - 200.0).abs() < 0.001);
    }

    #[test]
    fn scales_overcommitted_fixed_segments_symmetrically() {
        let solved = solve_axis_lengths(
            &[
                AxisConstraints {
                    min: 0.0,
                    fixed: Some(900.0),
                    max: None,
                    weight: 1.0,
                    can_grow: false,
                },
                AxisConstraints {
                    min: 0.0,
                    fixed: Some(900.0),
                    max: None,
                    weight: 1.0,
                    can_grow: false,
                },
            ],
            1400.0,
        );

        assert_eq!(solved.len(), 2);
        assert!((solved[0] - 700.0).abs() < 0.001);
        assert!((solved[1] - 700.0).abs() < 0.001);
    }

    #[test]
    fn max_caps_participate_in_growth_distribution() {
        let solved = solve_axis_lengths(
            &[
                AxisConstraints {
                    min: 0.0,
                    fixed: None,
                    max: Some(600.0),
                    weight: 1.0,
                    can_grow: true,
                },
                AxisConstraints {
                    min: 0.0,
                    fixed: None,
                    max: None,
                    weight: 1.0,
                    can_grow: true,
                },
            ],
            1600.0,
        );

        assert_eq!(solved.len(), 2);
        assert!((solved[0] - 600.0).abs() < 0.001);
        assert!((solved[1] - 1000.0).abs() < 0.001);
    }

    #[test]
    fn non_binding_minima_do_not_skew_equal_weight_segments() {
        let solved = solve_axis_lengths(
            &[
                AxisConstraints {
                    min: 0.0,
                    fixed: None,
                    max: None,
                    weight: 1.0,
                    can_grow: true,
                },
                AxisConstraints {
                    min: 400.0,
                    fixed: None,
                    max: None,
                    weight: 1.0,
                    can_grow: true,
                },
            ],
            1200.0,
        );

        assert_eq!(solved.len(), 2);
        assert!((solved[0] - 600.0).abs() < 0.001);
        assert!((solved[1] - 600.0).abs() < 0.001);
    }
}
