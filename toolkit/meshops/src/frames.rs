//! Tangent frames from texture coordinates, and how a normal map's two tangent-plane channels, written against one
//! frame, read against another: the frame math of `uv-frame-normals` (toolkit/image).
use crate::vector::Vec3;

/// A map of two signed axes onto two: output channel `k` is input channel `from[k]`, negated when `negate[k]`.
/// The eight such maps are the rotations by quarter turns and the mirrors of the tangent plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AxisMap {
    pub from: [u8; 2],
    pub negate: [bool; 2],
}

impl AxisMap {
    pub const IDENTITY: AxisMap = AxisMap {
        from: [0, 1],
        negate: [false, false],
    };

    /// All eight.
    pub fn all() -> impl Iterator<Item = AxisMap> {
        [[0u8, 1u8], [1, 0]].into_iter().flat_map(|from| {
            [[false, false], [false, true], [true, false], [true, true]]
                .into_iter()
                .map(move |negate| AxisMap { from, negate })
        })
    }

    /// The map as a 2x2 matrix, rows the outputs.
    pub fn matrix(self) -> [[f32; 2]; 2] {
        std::array::from_fn(|k| {
            let mut row = [0.; 2];
            row[usize::from(self.from[k])] = if self.negate[k] { -1. } else { 1. };
            row
        })
    }

    /// `[x, y]` mapped.
    pub fn apply<T: Copy>(self, v: [T; 2], negate: impl Fn(T) -> T) -> [T; 2] {
        std::array::from_fn(|k| {
            let x = v[usize::from(self.from[k])];
            if self.negate[k] { negate(x) } else { x }
        })
    }

    /// `[r, g]` as UNORM8 bytes encoding `2v/255 - 1`: negation is `255 - v`, exactly.
    pub fn apply_bytes(self, v: [u8; 2]) -> [u8; 2] {
        self.apply(v, |x| 255 - x)
    }
}

/// A triangle's texture-coordinate axes on its surface: the directions of increasing u and v (`dP/du`, `dP/dv`),
/// each made perpendicular to `normal` and unit length. `None` when the coordinates or the triangle are degenerate.
pub fn uv_axes(p: [[f32; 3]; 3], uv: [[f32; 2]; 3], normal: [f32; 3]) -> Option<([f32; 3], [f32; 3])> {
    let e1 = Vec3::from(p[1]) - Vec3::from(p[0]);
    let e2 = Vec3::from(p[2]) - Vec3::from(p[0]);
    let (du1, dv1) = (uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]);
    let (du2, dv2) = (uv[2][0] - uv[0][0], uv[2][1] - uv[0][1]);
    let det = du1 * dv2 - du2 * dv1;
    if det == 0. || !det.is_finite() {
        return None;
    }
    let dpdu = (e1 * dv2 - e2 * dv1) * (1. / det);
    let dpdv = (e2 * du1 - e1 * du2) * (1. / det);
    let n = Vec3::from(normal).normalize_or(Vec3::ZERO);
    if n == Vec3::ZERO {
        return None;
    }
    let flat = |v: Vec3| {
        let v = v - n * v.dot(n);
        let l = v.length();
        (l > 0. && l.is_finite()).then(|| <[f32; 3]>::from(v * (1. / l)))
    };
    Some((flat(dpdu)?, flat(dpdv)?))
}

/// How channels written against axes `stored` (the directions of the map's first and second tangent-plane channel)
/// read against axes `target`: the nearest [`AxisMap`] and the largest difference between its matrix and the true
/// one (`out_k = sum_j (target_k . stored_j) in_j`): 0 when the frames are the same up to a map, sqrt(1/2) when they
/// are half a quarter turn apart.
pub fn axis_map(stored: [[f32; 3]; 2], target: [[f32; 3]; 2]) -> (AxisMap, f32) {
    let m: [[f32; 2]; 2] =
        std::array::from_fn(|k| std::array::from_fn(|j| Vec3::from(target[k]).dot(Vec3::from(stored[j]))));
    let mut best = (AxisMap::IDENTITY, f32::INFINITY);
    for map in AxisMap::all() {
        let p = map.matrix();
        let error = (0..2)
            .flat_map(|k| (0..2).map(move |j| (k, j)))
            .map(|(k, j)| (m[k][j] - p[k][j]).abs())
            .fold(0f32, f32::max);
        if error < best.1 {
            best = (map, error);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_follow_u_and_v_on_the_surface() {
        // u along +x, v along -z, on the ground; tilted normals are flattened out
        let (u, v) = uv_axes(
            [[0., 0., 0.], [2., 0., 0.], [0., 0., -4.]],
            [[0., 0.], [1., 0.], [0., 1.]],
            [0., 1., 0.],
        )
        .unwrap();
        assert_eq!((u, v), ([1., 0., 0.], [0., 0., -1.]));
        // mirrored coordinates mirror the axes
        let (u, _) = uv_axes(
            [[0., 0., 0.], [2., 0., 0.], [0., 0., -4.]],
            [[1., 0.], [0., 0.], [1., 1.]],
            [0., 1., 0.],
        )
        .unwrap();
        assert_eq!(u, [-1., 0., 0.]);
        assert!(uv_axes([[0.; 3]; 3], [[0., 0.], [1., 0.], [0., 1.]], [0., 1., 0.]).is_none());
        assert!(
            uv_axes(
                [[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]],
                [[0.; 2]; 3],
                [0., 1., 0.]
            )
            .is_none()
        );
    }

    #[test]
    fn maps_are_the_eight_signed_permutations() {
        let x = [1., 0., 0.];
        let z = [0., 0., 1.];
        assert_eq!(axis_map([x, z], [x, z]), (AxisMap::IDENTITY, 0.));
        // green against the other v direction: flip green
        let (map, error) = axis_map([x, z], [x, [0., 0., -1.]]);
        assert_eq!(
            (map, error),
            (
                AxisMap {
                    from: [0, 1],
                    negate: [false, true]
                },
                0.
            )
        );
        assert_eq!(map.apply_bytes([10, 20]), [10, 235]);
        // stored red along v and green along u: swap
        let (map, _) = axis_map([z, x], [x, z]);
        assert_eq!(map.from, [1, 0]);
        assert_eq!(map.apply_bytes([10, 20]), [20, 10]);
        // twenty degrees: the identity, 1 - cos away on the diagonal and sin off it
        let (sn, cs) = 20f32.to_radians().sin_cos();
        let (map, error) = axis_map([[cs, 0., sn], [-sn, 0., cs]], [x, z]);
        assert_eq!(map, AxisMap::IDENTITY);
        assert!((error - sn).abs() < 1e-6, "{error}");
        // an eighth of a turn: sqrt(1/2) from every map
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let (_, error) = axis_map([[s, 0., s], [-s, 0., s]], [x, z]);
        assert!((error - s).abs() < 1e-6, "{error}");
        assert_eq!(AxisMap::all().count(), 8);
        for map in AxisMap::all() {
            for v in [[0u8, 255], [17, 200], [128, 127]] {
                // a map is undone by its matrix's transpose
                let back = AxisMap::all()
                    .find(|m| {
                        let (a, b) = (map.matrix(), m.matrix());
                        (0..2).all(|i| (0..2).all(|j| a[i][j] == b[j][i]))
                    })
                    .unwrap();
                assert_eq!(back.apply_bytes(map.apply_bytes(v)), v);
            }
        }
    }
}
