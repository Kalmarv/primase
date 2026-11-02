//! The world frame: metres, right-handed, +Y up, -Z north (glTF's frame, and one consumer's); +X is east.
//!
//! The world is always in this frame. A reader converts from its game's frame on the way in and a writer to its
//! target's on the way out, with a [`SourceFrame`]: a signed axis permutation plus a unit scale. Handedness follows
//! from the permutation's determinant; a mirroring frame (determinant -1) reverses triangle winding, which the
//! converting mesh code must undo ([`SourceFrame::flips_winding`]).
//!
//! Exactness: the axis part only moves and negates values, so it is bit-exact for every float, signed zeros
//! included (negation flips the sign bit; nothing is computed as `0 - x`). A unit scale other than 1 multiplies
//! positions and translations once, in f64, rounded once to f32: exact both ways when the scale is a power of two.
//!
//! Game-specific rotation conventions (Euler orders, row-vector matrices, angle units) are not frames: they belong
//! to the game crates, which build a [`Transform`] in the game's own frame and convert it here.

use crate::components::Transform;

/// A signed source axis: which source coordinate, with which sign, becomes one world coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Axis {
    /// The source coordinate's index (0 = x, 1 = y, 2 = z).
    pub const fn index(self) -> usize {
        match self {
            Axis::PosX | Axis::NegX => 0,
            Axis::PosY | Axis::NegY => 1,
            Axis::PosZ | Axis::NegZ => 2,
        }
    }

    pub const fn is_negative(self) -> bool {
        matches!(self, Axis::NegX | Axis::NegY | Axis::NegZ)
    }

    const fn of(index: usize, negative: bool) -> Axis {
        match (index, negative) {
            (0, false) => Axis::PosX,
            (0, true) => Axis::NegX,
            (1, false) => Axis::PosY,
            (1, true) => Axis::NegY,
            (2, false) => Axis::PosZ,
            _ => Axis::NegZ,
        }
    }
}

/// Why a frame description is not a frame.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum FrameError {
    #[error("axes {0:?} are not a permutation of x, y, z")]
    NotAPermutation([Axis; 3]),
    #[error("metres per unit {0} is not a finite positive number")]
    BadScale(f64),
}

/// A source coordinate frame described in world terms: `world[i] = sign_i * source[axes[i]] * metres_per_unit`
/// for positions, without the scale for directions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceFrame {
    axes: [Axis; 3],
    metres_per_unit: f64,
}

impl SourceFrame {
    /// The world frame itself.
    pub const WORLD: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0);

    /// East, north, up, metres (right-handed, z up): a source's own frame, one writer's internal frame, and
    /// Blender's. `world = (east, up, -north)`; back out, `(east, north, up) = (x, -z, y)`.
    pub const ENU: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosZ, Axis::NegY], 1.0);

    /// A frame from constant parts. Panics (a compile error in a `const`) when the axes are not a permutation or the
    /// scale is not finite and positive: frames written in code are checked where they are written. For frames read
    /// from data use [`SourceFrame::try_new`].
    pub const fn new(axes: [Axis; 3], metres_per_unit: f64) -> SourceFrame {
        match SourceFrame::try_new(axes, metres_per_unit) {
            Ok(f) => f,
            Err(_) => {
                panic!("not a frame: axes must be a permutation of x, y, z and the scale finite and positive")
            }
        }
    }

    pub const fn try_new(axes: [Axis; 3], metres_per_unit: f64) -> Result<SourceFrame, FrameError> {
        let (a, b, c) = (axes[0].index(), axes[1].index(), axes[2].index());
        if a == b || a == c || b == c {
            return Err(FrameError::NotAPermutation(axes));
        }
        if !(metres_per_unit.is_finite() && metres_per_unit > 0.0) {
            return Err(FrameError::BadScale(metres_per_unit));
        }
        Ok(SourceFrame {
            axes,
            metres_per_unit,
        })
    }

    /// For world axis i, the source axis (with sign) it is taken from.
    pub const fn axes(&self) -> [Axis; 3] {
        self.axes
    }

    pub const fn metres_per_unit(&self) -> f64 {
        self.metres_per_unit
    }

    /// The determinant of the axis part: +1 when the source is right-handed like the world, -1 when it is
    /// left-handed (converting mirrors).
    pub const fn determinant(&self) -> i32 {
        let [a, b, c] = self.axes;
        // parity of the permutation (a, b, c) of (0, 1, 2): even when it is a rotation of the identity
        let even = (a.index() + 1) % 3 == b.index() && (b.index() + 1) % 3 == c.index();
        let negs = a.is_negative() as u32 + b.is_negative() as u32 + c.is_negative() as u32;
        if even == negs.is_multiple_of(2) { 1 } else { -1 }
    }

    /// Whether converting a mesh mirrors it, so its triangles' winding must be reversed to keep their fronts.
    pub const fn flips_winding(&self) -> bool {
        self.determinant() == -1
    }

    /// The world frame described in this frame's terms: converting with the inverse undoes a conversion.
    pub const fn inverse(&self) -> SourceFrame {
        let mut inv = [Axis::PosX; 3];
        let mut i = 0;
        while i < 3 {
            let a = self.axes[i];
            inv[a.index()] = Axis::of(i, a.is_negative());
            i += 1;
        }
        SourceFrame::new(inv, 1.0 / self.metres_per_unit)
    }

    /// A direction (normal, tangent, axis): signed permutation only, bit-exact.
    pub fn direction(&self, d: [f32; 3]) -> [f32; 3] {
        self.axes.map(|a| signed(d[a.index()], a))
    }

    /// A position: signed permutation, then the unit scale (f64, rounded once; exact for scale 1).
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let d = self.direction(p);
        if self.metres_per_unit == 1.0 {
            return d;
        }
        d.map(|x| (f64::from(x) * self.metres_per_unit) as f32)
    }

    /// A transform `world = M * local` whose local and world sides are both in this frame: `C M C^-1`. The linear
    /// part only moves and negates entries (bit-exact; the scale cancels), the translation converts as a point.
    pub fn transform(&self, m: &Transform) -> Transform {
        let mut out = Transform { cols: [[0.0; 3]; 4] };
        for (j, a) in self.axes.iter().enumerate() {
            out.cols[j] = self.direction(m.cols[a.index()]).map(|x| signed(x, *a));
        }
        out.cols[3] = self.point(m.cols[3]);
        out
    }

    /// World to this frame: the inverse conversions.
    pub fn direction_to_source(&self, d: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0; 3];
        for (i, a) in self.axes.iter().enumerate() {
            out[a.index()] = signed(d[i], *a);
        }
        out
    }

    pub fn point_to_source(&self, p: [f32; 3]) -> [f32; 3] {
        let d = self.direction_to_source(p);
        if self.metres_per_unit == 1.0 {
            return d;
        }
        d.map(|x| (f64::from(x) / self.metres_per_unit) as f32)
    }

    pub fn transform_to_source(&self, m: &Transform) -> Transform {
        let mut out = Transform { cols: [[0.0; 3]; 4] };
        for (i, a) in self.axes.iter().enumerate() {
            out.cols[a.index()] = self.direction_to_source(m.cols[i]).map(|x| signed(x, *a));
        }
        out.cols[3] = self.point_to_source(m.cols[3]);
        out
    }

    /// A length in metres, in this frame's units (exact for scale 1).
    pub fn length_to_source(&self, l: f32) -> f32 {
        if self.metres_per_unit == 1.0 {
            return l;
        }
        (f64::from(l) / self.metres_per_unit) as f32
    }

    /// One over a length in metres, in this frame's units (exact for scale 1).
    pub fn inverse_length_to_source(&self, l: f32) -> f32 {
        scale(l, self.metres_per_unit)
    }

    /// A tangent (xyz, w the bitangent sign) in this frame: xyz as a direction, w negated where the frame mirrors.
    pub fn tangent_to_source(&self, t: [f32; 4]) -> [f32; 4] {
        let d = self.direction_to_source([t[0], t[1], t[2]]);
        [d[0], d[1], d[2], if self.flips_winding() { -t[3] } else { t[3] }]
    }

    /// A plane `(n, d)` (`dot(n, p) + d = 0` for its world points `p`) in this frame: the normal as a direction, the
    /// offset as a length.
    pub fn plane_to_source(&self, plane: [f32; 4]) -> [f32; 4] {
        let n = self.direction_to_source([plane[0], plane[1], plane[2]]);
        [n[0], n[1], n[2], self.length_to_source(plane[3])]
    }

    /// A 4 x 4 matrix (by columns) that takes world positions to another space (the view, the clip), made to take this
    /// frame's positions to the same space: `M C`, `C` this frame's map to the world, so `(M C) (C^-1 p) = M p` and
    /// what it gives is unchanged. Column `j` of `M C` is the column of `M` for the world axis this frame's axis `j`
    /// is, signed and times the unit scale: for scale 1 the entries only move and change sign (bit-exact), and the
    /// product with a converted point sums the same terms in this frame's axis order (the same order, so the same
    /// bits, for a frame that only changes signs).
    pub fn from_world_matrix_to_source(&self, m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let mut out = m;
        for (i, a) in self.axes.iter().enumerate() {
            out[a.index()] = m[i].map(|x| scale(signed(x, *a), self.metres_per_unit));
        }
        out
    }

    /// A 4 x 4 matrix (by columns) that gives world positions (from the view, the clip), made to give this frame's:
    /// `C^-1 M`, each column's xyz converted as a point and its w kept.
    pub fn to_world_matrix_to_source(&self, m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        m.map(|c| {
            let p = self.point_to_source([c[0], c[1], c[2]]);
            [p[0], p[1], p[2], c[3]]
        })
    }

    /// A 4 x 4 matrix (by columns) from one world-frame space to another (an object's to the world, a joint's) in this
    /// frame's terms: `C^-1 M C`, as [`SourceFrame::transform_to_source`] converts an affine transform.
    pub fn world_matrix_to_source(&self, m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        self.to_world_matrix_to_source(self.from_world_matrix_to_source(m))
    }
}

/// A frame conversion moves a mesh's streams as it moves positions and directions: normals and tangents by the signed
/// permutation (a uniform scale turns no direction), a tangent's bitangent sign negated where the conversion mirrors.
impl crate::VertexMap for SourceFrame {
    fn point(&self, p: [f32; 3]) -> [f32; 3] {
        SourceFrame::point(self, p)
    }
    fn normal(&self, n: [f32; 3]) -> Option<[f32; 3]> {
        Some(self.direction(n))
    }
    fn tangent(&self, t: [f32; 4]) -> Option<[f32; 4]> {
        let d = self.direction([t[0], t[1], t[2]]);
        Some([d[0], d[1], d[2], if self.flips_winding() { -t[3] } else { t[3] }])
    }
}

/// `x` negated (sign bit flipped) when the axis is negative.
fn signed(x: f32, a: Axis) -> f32 {
    if a.is_negative() { -x } else { x }
}

/// `x` times the unit scale, in f64, rounded once (`x` itself for scale 1).
fn scale(x: f32, metres_per_unit: f64) -> f32 {
    if metres_per_unit == 1.0 {
        return x;
    }
    (f64::from(x) * metres_per_unit) as f32
}

/// The ground frame `(east, north)` of a world point: `(x, -z)`, bit-exact.
pub fn ground(p: [f32; 3]) -> [f32; 2] {
    [p[0], -p[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use Axis::*;

    fn bits(v: [f32; 3]) -> [u32; 3] {
        v.map(f32::to_bits)
    }

    fn mul(m: &Transform, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            m.cols[0][i] * p[0] + m.cols[1][i] * p[1] + m.cols[2][i] * p[2] + m.cols[3][i]
        })
    }

    /// A left-handed source, Y up, metres; scene = (x, y, -z),
    /// transforms mirrored as `C M C` with C = diag(1, 1, -1).
    const FROMSOFT: SourceFrame = SourceFrame::new([PosX, PosY, NegZ], 1.0);
    /// A left-handed source, Y up -> (x, y, -z).
    const UNITY: SourceFrame = SourceFrame::new([PosX, PosY, NegZ], 1.0);
    /// ForzaTech: left-handed, Y up, metres; Blender =
    /// (-x, -z, y). Blender is ENU, so world = (-x, y, z).
    const FORZA: SourceFrame = SourceFrame::new([NegX, PosY, PosZ], 1.0);

    #[test]
    fn handedness_follows_the_determinant() {
        assert_eq!(SourceFrame::WORLD.determinant(), 1);
        assert_eq!(SourceFrame::ENU.determinant(), 1);
        assert!(!SourceFrame::ENU.flips_winding());
        for f in [FROMSOFT, UNITY, FORZA] {
            assert_eq!(f.determinant(), -1, "{f:?}");
            assert!(f.flips_winding());
        }
        // every signed permutation against the determinant computed as a matrix
        let all = [PosX, NegX, PosY, NegY, PosZ, NegZ];
        let mut n = 0;
        for a in all {
            for b in all {
                for c in all {
                    let Ok(f) = SourceFrame::try_new([a, b, c], 1.0) else {
                        continue;
                    };
                    let m: [[i32; 3]; 3] = std::array::from_fn(|i| {
                        let ax = f.axes()[i];
                        std::array::from_fn(|j| {
                            if ax.index() == j {
                                if ax.is_negative() { -1 } else { 1 }
                            } else {
                                0
                            }
                        })
                    });
                    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
                    assert_eq!(f.determinant(), det, "{f:?}");
                    n += 1;
                }
            }
        }
        assert_eq!(n, 48, "6 permutations x 8 sign choices");
    }

    #[test]
    fn invalid_frames_are_errors() {
        assert_eq!(
            SourceFrame::try_new([PosX, NegX, PosZ], 1.0),
            Err(FrameError::NotAPermutation([PosX, NegX, PosZ]))
        );
        assert!(matches!(
            SourceFrame::try_new([PosX, PosY, PosZ], 0.0),
            Err(FrameError::BadScale(_))
        ));
        assert!(matches!(
            SourceFrame::try_new([PosX, PosY, PosZ], f64::NAN),
            Err(FrameError::BadScale(_))
        ));
    }

    const IDENTITY: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
        std::array::from_fn(|j| std::array::from_fn(|i| (0..4).map(|k| a[k][i] * b[j][k]).sum()))
    }

    fn mat_apply(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 4] {
        let p = [p[0], p[1], p[2], 1.0];
        std::array::from_fn(|i| (0..4).map(|k| m[k][i] * p[k]).sum())
    }

    fn xyz(v: [f32; 4]) -> [f32; 3] {
        [v[0], v[1], v[2]]
    }

    /// The matrix forms against the frame's own point map, over every signed permutation at scale 1 and a scaled frame:
    /// a world-to-view matrix gives what it gave for the converted point, an object-to-world one takes a converted
    /// point to the converted point, a view-to-world one gives the converted point; the entries only move and change
    /// sign at scale 1.
    #[test]
    fn matrices_take_and_give_the_frames_points() {
        let m: [[f32; 4]; 4] = [
            [0.6, -0.8, 0.1, 0.0],
            [0.8, 0.6, -0.3, 0.0],
            [0.25, 0.5, 0.9, 0.0],
            [3.0, -2.0, 7.5, 1.0],
        ];
        let p = [1.5, -2.25, 4.0];
        let world = mat_apply(&m, p);
        let all = [PosX, NegX, PosY, NegY, PosZ, NegZ];
        let mut n = 0;
        for a in all {
            for b in all {
                for c in all {
                    let Ok(f) = SourceFrame::try_new([a, b, c], 1.0) else {
                        continue;
                    };
                    let q = f.point_to_source(p);
                    let from = mat_apply(&f.from_world_matrix_to_source(m), q);
                    assert!(
                        from.iter().zip(world).all(|(x, y)| (x - y).abs() <= 1e-5),
                        "{f:?}"
                    );
                    let moved = f.point_to_source(xyz(world));
                    let within = xyz(mat_apply(&f.world_matrix_to_source(m), q));
                    assert!(
                        within.iter().zip(moved).all(|(x, y)| (x - y).abs() <= 1e-5),
                        "{f:?}"
                    );
                    let to = xyz(mat_apply(&f.to_world_matrix_to_source(m), p));
                    assert_eq!(bits(to), bits(moved), "{f:?}");
                    let entries = |m: [[f32; 4]; 4]| {
                        let mut e: Vec<u32> = m.iter().flatten().map(|x| x.abs().to_bits()).collect();
                        e.sort_unstable();
                        e
                    };
                    assert_eq!(entries(f.world_matrix_to_source(m)), entries(m), "{f:?}");
                    n += 1;
                }
            }
        }
        assert_eq!(n, 48);
        // a frame that only changes signs keeps the axis order: the products are bit-exact
        let from = mat_apply(
            &FROMSOFT.from_world_matrix_to_source(m),
            FROMSOFT.point_to_source(p),
        );
        assert_eq!(from.map(f32::to_bits), world.map(f32::to_bits));
        // (M C)(C^-1) = M: a world-to-clip matrix fed this way puts every vertex where it was, so rasterisation and
        // the winding a triangle shows on screen are unchanged
        let back = mat_mul(
            &FROMSOFT.from_world_matrix_to_source(m),
            &FROMSOFT.to_world_matrix_to_source(IDENTITY),
        );
        assert_eq!(back, m);
        // a scaled frame: positions, offsets and lengths in its units
        let cm = SourceFrame::new([PosX, PosZ, NegY], 0.01);
        let from = mat_apply(&cm.from_world_matrix_to_source(m), cm.point_to_source(p));
        assert!(
            from.iter().zip(world).all(|(x, y)| (x - y).abs() <= 1e-4),
            "{from:?}"
        );
        assert_eq!(cm.length_to_source(2.0), 200.0);
        assert_eq!(cm.inverse_length_to_source(0.5), 0.005);
        let plane = cm.plane_to_source([0.0, 1.0, 0.0, -3.0]);
        let on = cm.point_to_source([5.0, 3.0, -1.0]);
        let d = plane[0] * on[0] + plane[1] * on[1] + plane[2] * on[2] + plane[3];
        assert!(d.abs() < 1e-3, "{plane:?}");
        assert_eq!(
            FROMSOFT.tangent_to_source([1.0, 0.0, 0.5, 1.0]),
            [1.0, 0.0, -0.5, -1.0]
        );
        assert_eq!(SourceFrame::ENU.tangent_to_source([1.0, 0.0, 0.5, 1.0])[3], 1.0);
    }

    #[test]
    fn fromsoft_and_unity_mirror_z() {
        assert_eq!(FROMSOFT.point([1.5, 2.0, 3.0]), [1.5, 2.0, -3.0]);
        assert_eq!(UNITY.direction([0.0, 0.0, 1.0]), [0.0, 0.0, -1.0]);
        // C M C with C = diag(1, 1, -1): entries (i, j) with exactly one of i, j = z change sign
        let m = Transform {
            cols: [
                [1.0, 2.0, 3.0],
                [4.0, 5.0, 6.0],
                [7.0, 8.0, 9.0],
                [10.0, 11.0, 12.0],
            ],
        };
        let want = Transform {
            cols: [
                [1.0, 2.0, -3.0],
                [4.0, 5.0, -6.0],
                [-7.0, -8.0, 9.0],
                [10.0, 11.0, -12.0],
            ],
        };
        assert_eq!(FROMSOFT.transform(&m), want);
    }

    #[test]
    fn enu_is_the_writer_frame_bit_exact_with_signed_zeros() {
        // scene-interchange v1 "Frames": (east, north, up) = (x, -z, y), exact for every float including signed zeros
        let world = [0.0, -0.0, 0.0];
        let enu = SourceFrame::ENU.point_to_source(world);
        assert_eq!(bits(enu), bits([0.0, -0.0, -0.0]));
        assert_eq!(bits(SourceFrame::ENU.point(enu)), bits(world));
        let p = [1.25, -7.5, 3.0e-41]; // a subnormal survives too
        assert_eq!(SourceFrame::ENU.point_to_source(p), [1.25, -3.0e-41, -7.5]);
        assert_eq!(ground(p), [1.25, -3.0e-41]);
        assert_eq!(
            ground([0.0, 0.0, 0.0]).map(f32::to_bits),
            [0.0f32.to_bits(), (-0.0f32).to_bits()]
        );
        // and the ENU source's (east, north, up) to the world
        assert_eq!(SourceFrame::ENU.point([10.0, 20.0, 30.0]), [10.0, 30.0, -20.0]);
    }

    #[test]
    fn forza_through_blender_matches_forza_direct() {
        let to_blender = |p: [f32; 3]| [-p[0], -p[2], p[1]]; // the ForzaTech -> Blender mapping
        for p in [[1.0, 2.0, 3.0], [-0.0, 0.5, -4.0], [7.25, -1.0, 0.0]] {
            assert_eq!(bits(FORZA.point(p)), bits(SourceFrame::ENU.point(to_blender(p))));
        }
    }

    #[test]
    fn transforms_commute_with_points_and_invert() {
        let all = [PosX, NegX, PosY, NegY, PosZ, NegZ];
        // integer-valued so every product and sum below is exact in f32
        let m = Transform {
            cols: [
                [2.0, 0.0, 1.0],
                [-1.0, 3.0, 0.0],
                [0.0, 1.0, -2.0],
                [5.0, -6.0, 7.0],
            ],
        };
        let locals = [[1.0, 2.0, 3.0], [-4.0, 0.0, 1.0], [0.5, -0.25, 8.0]];
        for a in all {
            for b in all {
                for c in all {
                    for scale in [1.0, 0.5, 4.0] {
                        let Ok(f) = SourceFrame::try_new([a, b, c], scale) else {
                            continue;
                        };
                        let fm = f.transform(&m);
                        for l in locals {
                            assert_eq!(f.point(mul(&m, l)), mul(&fm, f.point(l)), "{f:?}");
                        }
                        assert_eq!(f.transform_to_source(&fm), m, "{f:?}");
                        assert_eq!(f.inverse().transform(&fm), m, "{f:?}");
                        assert_eq!(f.inverse().inverse(), f);
                        for l in locals {
                            assert_eq!(bits(f.point_to_source(f.point(l))), bits(l));
                            assert_eq!(bits(f.inverse().point(f.point(l))), bits(l));
                            assert_eq!(bits(f.direction_to_source(f.direction(l))), bits(l));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scale_applies_to_points_and_translations_only() {
        let cm = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 0.01);
        assert_eq!(cm.point([100.0, 250.0, -50.0]), [1.0, 2.5, -0.5]);
        assert_eq!(cm.direction([0.0, 1.0, 0.0]), [0.0, 1.0, 0.0]);
        let m = Transform {
            cols: [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [300.0, 0.0, 0.0],
            ],
        };
        assert_eq!(cm.transform(&m).cols[3], [3.0, 0.0, 0.0]);
        assert_eq!(
            cm.transform(&m).cols[0],
            [1.0, 0.0, 0.0],
            "the scale cancels in the linear part"
        );
    }
}
