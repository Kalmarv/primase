//! A channel's value at a time, by core's evaluation rule (`core.clip`): [`Interpolation::Step`] holds a key until
//! the next; [`Interpolation::Linear`] interpolates linearly, rotations by slerp along the shorter arc;
//! [`Interpolation::Hermite`] is the cubic Hermite spline of glTF's `CUBICSPLINE` (per key in-tangent, value,
//! out-tangent; tangents per second, times the segment's length), rotations componentwise then normalised. Before the
//! first key and after the last the end value holds; mapping a clip's time into its keys (looping) is the caller's.

use primase_core::Interpolation;

use crate::quat;
use crate::{Error, Result};

/// The value of a channel of `width` values a key (4 for a rotation, `rotation` true) at time `t` seconds.
pub fn sample(
    interpolation: Interpolation,
    times: &[f32],
    values: &[f32],
    width: usize,
    rotation: bool,
    t: f64,
) -> Result<Vec<f64>> {
    let per_key = if interpolation == Interpolation::Hermite {
        3 * width
    } else {
        width
    };
    if times.is_empty() || values.len() != times.len() * per_key || width == 0 {
        return Err(Error(format!(
            "{} values for {} keys of {per_key}",
            values.len(),
            times.len()
        )));
    }
    // the key value of key k (the middle of a Hermite triple)
    let value = |k: usize| -> Vec<f64> {
        let at = k * per_key
            + if interpolation == Interpolation::Hermite {
                width
            } else {
                0
            };
        values[at..at + width].iter().map(|&x| f64::from(x)).collect()
    };
    let last = times.len() - 1;
    if t <= f64::from(times[0]) {
        return Ok(value(0));
    }
    if t >= f64::from(times[last]) {
        return Ok(value(last));
    }
    // the segment [k, k + 1] holding t: the last key at or before it
    let k = times.partition_point(|&x| f64::from(x) <= t) - 1;
    let (t0, t1) = (f64::from(times[k]), f64::from(times[k + 1]));
    let dt = t1 - t0;
    let s = (t - t0) / dt;
    let (a, b) = (value(k), value(k + 1));
    let out = match interpolation {
        Interpolation::Step => a,
        Interpolation::Linear if rotation => {
            let q = |v: &[f64]| [v[0], v[1], v[2], v[3]];
            quat::slerp(q(&a), q(&b), s).to_vec()
        }
        Interpolation::Linear => a.iter().zip(&b).map(|(x, y)| x + (y - x) * s).collect(),
        Interpolation::Hermite => {
            let tangent = |k: usize, which: usize| -> Vec<f64> {
                let at = k * per_key + which * width;
                values[at..at + width]
                    .iter()
                    .map(|&x| f64::from(x) * dt)
                    .collect()
            };
            let (out_a, in_b) = (tangent(k, 2), tangent(k + 1, 0));
            let (s2, s3) = (s * s, s * s * s);
            let (h00, h10, h01, h11) = (
                2.0 * s3 - 3.0 * s2 + 1.0,
                s3 - 2.0 * s2 + s,
                -2.0 * s3 + 3.0 * s2,
                s3 - s2,
            );
            let v: Vec<f64> = (0..width)
                .map(|i| h00 * a[i] + h10 * out_a[i] + h01 * b[i] + h11 * in_b[i])
                .collect();
            if rotation {
                quat::normalize([v[0], v[1], v[2], v[3]])
                    .ok_or_else(|| Error("a Hermite rotation passing through zero".into()))?
                    .to_vec()
            } else {
                v
            }
        }
    };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn near(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    /// Hand-worked: keys 0 at t=0 and 10 at t=2. Step holds 0 until 2; linear is 5 at 1; ends hold outside.
    #[test]
    fn step_and_linear_scalars() {
        let (t, v) = ([0.0, 2.0], [0.0, 10.0]);
        let at = |i, x| sample(i, &t, &v, 1, false, x).unwrap();
        assert!(near(&at(Interpolation::Step, 1.9), &[0.0]));
        assert!(near(&at(Interpolation::Step, 2.0), &[10.0]));
        assert!(near(&at(Interpolation::Linear, 1.0), &[5.0]));
        assert!(near(&at(Interpolation::Linear, -1.0), &[0.0]));
        assert!(near(&at(Interpolation::Linear, 3.0), &[10.0]));
    }

    /// Hand-worked: identity to a half turn about z, halfway is a quarter turn about z (slerp, not lerp).
    #[test]
    fn linear_rotation_is_slerp() {
        let v = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0];
        let q = sample(Interpolation::Linear, &[0.0, 1.0], &v, 4, true, 0.5).unwrap();
        assert!(near(&q, &[0.0, 0.0, f64::from(S), f64::from(S)]), "{q:?}");
    }

    /// Hand-worked Hermite: values 0 and 1 over [0, 2] with zero tangents is smoothstep: at t=1 (s=0.5) it is 0.5, at
    /// t=0.5 (s=0.25) 3s^2 - 2s^3 = 0.15625. With out-tangent 1 per second at the first key and in-tangent 0 at the
    /// second, h10 = s^3 - 2s^2 + s = 0.140625 at s=0.25, times dt=2: 0.15625 + 0.28125 = 0.4375.
    #[test]
    fn hermite_matches_the_basis_functions() {
        let times = [0.0, 2.0];
        let flat = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        assert!(near(
            &sample(Interpolation::Hermite, &times, &flat, 1, false, 1.0).unwrap(),
            &[0.5]
        ));
        assert!(near(
            &sample(Interpolation::Hermite, &times, &flat, 1, false, 0.5).unwrap(),
            &[0.15625]
        ));
        let sloped = [0.0, 0.0, 1.0, 0.0, 1.0, 0.0];
        assert!(near(
            &sample(Interpolation::Hermite, &times, &sloped, 1, false, 0.5).unwrap(),
            &[0.4375]
        ));
    }

    #[test]
    fn malformed_channels_are_errors() {
        assert!(sample(Interpolation::Linear, &[0.0, 1.0], &[0.0], 1, false, 0.5).is_err());
        assert!(sample(Interpolation::Hermite, &[0.0], &[0.0], 1, false, 0.5).is_err());
        assert!(sample(Interpolation::Linear, &[], &[], 1, false, 0.5).is_err());
    }
}
