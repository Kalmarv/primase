//! The world frame: metres, right-handed, +Y up, -Z north (glTF's frame, and skate-scene v1's); +X is east.
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

    /// East, north, up, metres (right-handed, z up): GTA's own frame, the skate writer's internal frame, and
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
}

/// `x` negated (sign bit flipped) when the axis is negative.
fn signed(x: f32, a: Axis) -> f32 {
    if a.is_negative() { -x } else { x }
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

    /// FromSoft: left-handed, Y up, metres; scene = (x, y, -z),
    /// transforms mirrored as `C M C` with C = diag(1, 1, -1).
    const FROMSOFT: SourceFrame = SourceFrame::new([PosX, PosY, NegZ], 1.0);
    /// Unity: left-handed, Y up -> (x, y, -z).
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
        // and GTA's (east, north, up) to the world
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
