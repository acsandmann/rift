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
/// - remainder is given to growable nodes by weight, then equally
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

    for idx in 0..n {
        if fixed[idx].is_none() {
            let need = mins[idx].min(remaining);
            lengths[idx] += need;
            remaining = (remaining - need).max(0.0);
        }
    }

    while remaining > f64::EPSILON {
        let growable: Vec<usize> = (0..n)
            .filter(|&idx| fixed[idx].is_none() && can_grow[idx])
            .filter(|&idx| match maxs[idx] {
                Some(max) => lengths[idx] + f64::EPSILON < max,
                None => true,
            })
            .collect();
        if growable.is_empty() {
            break;
        }

        let total_weight: f64 = growable.iter().map(|&idx| weights[idx]).sum();
        let mut consumed = 0.0;
        if total_weight > 0.0 {
            for &idx in &growable {
                let share = remaining * (weights[idx] / total_weight);
                let cap = maxs[idx].map(|max| (max - lengths[idx]).max(0.0)).unwrap_or(share);
                let delta = share.min(cap);
                lengths[idx] += delta;
                consumed += delta;
            }
        } else {
            let each = remaining / growable.len() as f64;
            for &idx in &growable {
                let cap = maxs[idx].map(|max| (max - lengths[idx]).max(0.0)).unwrap_or(each);
                let delta = each.min(cap);
                lengths[idx] += delta;
                consumed += delta;
            }
        }
        if consumed <= f64::EPSILON {
            break;
        }
        remaining = (remaining - consumed).max(0.0);
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
}
