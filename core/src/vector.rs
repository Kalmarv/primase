//! Three-component vectors in f64, as plain arrays: the one set of the arithmetic every crate's geometry in f64 shares
//! (f32 meshes have `primase_meshops::Vec3`). Each operation is one fixed order of operations, so any two crates
//! computing the same thing get the same bits.

/// A vector or point, x, y, z.
pub type V3 = [f64; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a` times `s`.
pub fn scale(a: V3, s: f64) -> V3 {
    a.map(|x| x * s)
}

/// `a0 b0 + a1 b1 + a2 b2`, summed in that order.
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The Euclidean length, `sqrt(a . a)`.
pub fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` divided by its length; `None` where the length is 0 or not finite (a vector with no direction), for the caller
/// to say what that means where it can happen.
pub fn unit(a: V3) -> Option<V3> {
    let l = length(a);
    (l > 0.0 && l.is_finite()).then(|| a.map(|x| x / l))
}

/// `a` divided by its length where that is above `shortest` (the caller's: below it a direction is noise), else `None`.
pub fn unit_beyond(a: V3, shortest: f64) -> Option<V3> {
    let l = length(a);
    (l > shortest && l.is_finite()).then(|| a.map(|x| x / l))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Worked by hand: the operations, a unit vector of a 3-4-5 triangle, and no direction for zero or past a stated
    /// floor.
    #[test]
    fn arithmetic_and_directions() {
        let (a, b) = ([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]);
        assert_eq!(add(a, b), [5.0, 7.0, 9.0]);
        assert_eq!(sub(b, a), [3.0, 3.0, 3.0]);
        assert_eq!(scale(a, 2.0), [2.0, 4.0, 6.0]);
        assert_eq!(dot(a, b), 32.0);
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(length([3.0, 4.0, 0.0]), 5.0);
        assert_eq!(unit([3.0, 4.0, 0.0]), Some([0.6, 0.8, 0.0]));
        assert_eq!(unit([0.0; 3]), None);
        assert_eq!(unit([f64::INFINITY, 0.0, 0.0]), None);
        assert_eq!(unit_beyond([1e-10, 0.0, 0.0], 1e-9), None);
        assert_eq!(unit_beyond([2e-9, 0.0, 0.0], 1e-9), Some([1.0, 0.0, 0.0]));
    }
}
