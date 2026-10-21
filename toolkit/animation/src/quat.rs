//! Unit quaternions `[x, y, z, w]` in f64: products, spherical interpolation, and the rotation of a matrix.

/// `a * b` (apply `b`, then `a`).
pub fn mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The inverse of a unit quaternion (its conjugate).
pub fn inverse(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// `q` of unit length; `None` for a zero or non-finite one.
pub fn normalize(q: [f64; 4]) -> Option<[f64; 4]> {
    let len = q.iter().map(|x| x * x).sum::<f64>().sqrt();
    (len > 0.0 && len.is_finite()).then(|| q.map(|x| x / len))
}

/// Spherical linear interpolation from `a` to `b` at `s` in [0, 1], along the shorter arc (`b` negated when the two
/// are more than a half turn apart). Nearly equal rotations interpolate linearly, then normalise.
pub fn slerp(a: [f64; 4], b: [f64; 4], s: f64) -> [f64; 4] {
    let mut dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let b = if dot < 0.0 {
        dot = -dot;
        b.map(|x| -x)
    } else {
        b
    };
    let (wa, wb) = if dot > 1.0 - 1e-9 {
        (1.0 - s, s)
    } else {
        let theta = dot.min(1.0).acos();
        let sin = theta.sin();
        (((1.0 - s) * theta).sin() / sin, (s * theta).sin() / sin)
    };
    let q: [f64; 4] = std::array::from_fn(|i| wa * a[i] + wb * b[i]);
    normalize(q).unwrap_or(a)
}

/// The rotation whose matrix has these columns (orthonormal, determinant +1), by the largest of the diagonal's
/// combinations (Shepperd): no division by a small number.
pub fn from_columns(c: [[f64; 3]; 3]) -> [f64; 4] {
    // m(r, k) is row r of column k
    let m = |r: usize, k: usize| c[k][r];
    let trace = m(0, 0) + m(1, 1) + m(2, 2);
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (m(2, 1) - m(1, 2)) / s,
            (m(0, 2) - m(2, 0)) / s,
            (m(1, 0) - m(0, 1)) / s,
            0.25 * s,
        ]
    } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
        let s = (1.0 + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.0;
        [
            0.25 * s,
            (m(0, 1) + m(1, 0)) / s,
            (m(0, 2) + m(2, 0)) / s,
            (m(2, 1) - m(1, 2)) / s,
        ]
    } else if m(1, 1) > m(2, 2) {
        let s = (1.0 + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.0;
        [
            (m(0, 1) + m(1, 0)) / s,
            0.25 * s,
            (m(1, 2) + m(2, 1)) / s,
            (m(0, 2) - m(2, 0)) / s,
        ]
    } else {
        let s = (1.0 + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.0;
        [
            (m(0, 2) + m(2, 0)) / s,
            (m(1, 2) + m(2, 1)) / s,
            0.25 * s,
            (m(1, 0) - m(0, 1)) / s,
        ]
    };
    normalize(q).unwrap_or([0.0, 0.0, 0.0, 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: f64 = std::f64::consts::FRAC_1_SQRT_2;

    fn near(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-12)
    }

    /// Hand-worked: a quarter turn about z twice is a half turn; slerp halfway from identity to a half turn about z
    /// is the quarter turn; the shorter arc is taken from q to -q's neighbour; the columns of a quarter turn about z
    /// (x to y, y to -x) give its quaternion back.
    #[test]
    fn products_slerp_and_columns() {
        let quarter = [0.0, 0.0, S, S];
        assert!(near(mul(quarter, quarter), [0.0, 0.0, 1.0, 0.0]));
        assert!(near(mul(quarter, inverse(quarter)), [0.0, 0.0, 0.0, 1.0]));
        assert!(near(
            slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 0.0], 0.5),
            quarter
        ));
        // -identity is identity: the path from identity to it is no path
        assert!(near(
            slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, -1.0], 0.5),
            [0.0, 0.0, 0.0, 1.0]
        ));
        assert!(near(
            from_columns([[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
            quarter
        ));
        // a half turn about x (trace -1: the second branch)
        assert!(near(
            from_columns([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]]),
            [1.0, 0.0, 0.0, 0.0]
        ));
    }
}
