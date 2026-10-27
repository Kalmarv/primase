//! Affine arithmetic on the places files store: column matrices in f64 ([`Matrix`]) and in f32 as a pose's columns
//! ([`pose_columns`]: a Havok body's position and unit quaternion).

/// An affine map in f64: the images of the x, y and z axes, then the translation.
pub type Matrix = [[f64; 3]; 4];
/// The direction `p` through `m` (no translation).
pub fn affine_vector(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|a| (0..3).map(|i| m[i][a] * p[i]).sum())
}
/// The point `p` through `m`.
pub fn affine_point(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    let v = affine_vector(m, p);
    std::array::from_fn(|i| v[i] + m[3][i])
}
/// `local`, then `parent`: the map of a child placed in its parent's space.
pub fn compose(parent: &Matrix, local: &Matrix) -> Matrix {
    [
        affine_vector(parent, local[0]),
        affine_vector(parent, local[1]),
        affine_vector(parent, local[2]),
        affine_point(parent, local[3]),
    ]
}

/// A body pose as the columns of its affine map (rotation by the unit quaternion `q`, then translation by `p`).
pub fn pose_columns(p: [f32; 3], q: [f32; 4]) -> [[f32; 3]; 4] {
    let [x, y, z, w] = q;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let xw = x * w;
    let yw = y * w;
    let zw = z * w;
    [
        [1. - 2. * (yy + zz), 2. * (xy + zw), 2. * (xz - yw)],
        [2. * (xy - zw), 1. - 2. * (zz + xx), 2. * (yz + xw)],
        [2. * (xz + yw), 2. * (yz - xw), 1. - 2. * (yy + xx)],
        p,
    ]
}

/// `v` through the affine map with columns `c`.
pub fn posed(c: &[[f32; 3]; 4], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| v[0] * c[0][i] + v[1] * c[1][i] + v[2] * c[2][i] + c[3][i])
}

/// `v` through the pose of position `p` (its `xyz`) and orientation `q`.
pub fn pose(v: [f32; 3], p: [f32; 4], q: [f32; 4]) -> [f32; 3] {
    posed(&pose_columns([p[0], p[1], p[2]], q), v)
}
