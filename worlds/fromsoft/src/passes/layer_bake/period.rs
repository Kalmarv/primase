//! Bounded common-repeat policy for static bakes, not runtime sampler state.
use crate::{Result, invalid};

/// Find a common repeat within sixteen UV units. A repeat is accepted when
/// each sampled scale is within one percent of an integer repeat count.
/// Otherwise use the main layer's repeat and explicitly report approximation.
pub fn common_period(scales: &[[f32; 2]], main: [f32; 2]) -> Result<([f32; 2], bool)> {
    if !scales.iter().flatten().chain(&main).all(|v| v.is_finite()) {
        return Err(invalid("layer period", 0, "nonfinite scale"));
    }
    let mut period = [1.; 2];
    let mut exact = true;
    for axis in 0..2 {
        let mut scales: Vec<f32> = scales.iter().map(|s| s[axis]).filter(|&s| s > 0.).collect();
        scales.sort_by(f32::total_cmp);
        scales.dedup();
        if scales.is_empty() {
            continue;
        }
        let mut candidates: Vec<f32> = scales
            .iter()
            .flat_map(|s| (1..=16).map(move |n| n as f32 / s))
            .chain([1.])
            .filter(|&p| p <= 16.001)
            .collect();
        candidates.sort_by(f32::total_cmp);
        if let Some(p) = candidates.into_iter().find(|&p| {
            scales.iter().all(|s| {
                let k = s * p;
                k.round_ties_even() >= 1. && (k - k.round_ties_even()).abs() < 0.01 * k.max(1.)
            })
        }) {
            period[axis] = p;
        } else {
            exact = false;
            if main[axis] <= 0. {
                return Err(invalid("layer period", axis, "nonpositive fallback scale"));
            }
            period[axis] = 1. / main[axis];
            if !period[axis].is_finite() {
                return Err(invalid("layer period", axis, "fallback period overflow"));
            }
        }
    }
    Ok((period, exact))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_repeats_and_named_fallback() {
        assert_eq!(common_period(&[], [1.; 2]).unwrap(), ([1.; 2], true));
        assert_eq!(
            common_period(&[[1., 0.2], [2., 0.5]], [1., 0.2]).unwrap(),
            ([1., 10.], true)
        );
        assert_eq!(
            common_period(&[[0.25, 0.2]], [0.25, 0.2]).unwrap(),
            ([4., 5.], true)
        );
        assert_eq!(
            common_period(&[[0.001, 1.]], [0.5, 1.]).unwrap(),
            ([2., 1.], false)
        );
        assert!(common_period(&[[f32::NAN, 1.]], [1.; 2]).is_err());
        assert!(common_period(&[[0.001, 1.]], [0., 1.]).is_err());
    }
}
