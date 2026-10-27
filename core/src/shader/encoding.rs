//! How a value is stored in the lanes of an output: the encodings of a normal ([`NormalEncoding`], in a
//! [`NormalSpace`]) and of a scalar ([`ScalarEncoding`]), each with its decoding (and, for the packed normals, its
//! encoding), as an output role states them (`OutputRole`).

use serde::Serialize;

/// Where a normal vector is expressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalSpace {
    /// World space.
    World,
    /// View (camera) space.
    View,
    /// Tangent space of the surface.
    Tangent,
}
/// How a normal is stored in the lanes of an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalEncoding {
    /// x, y, z as signed values.
    Xyz,
    /// x, y, z mapped to 0-1 (`n * 0.5 + 0.5`).
    XyzUnorm,
    /// x and y signed, z from the unit length.
    Xy,
    /// x and y mapped to 0-1, z from the unit length.
    XyUnorm,
    /// x, y, z stored as `n * 0.5 + bias` (the bias given by its f32 bits): decoded `(v - bias) * 2`.
    XyzBiased {
        /// The bias, as the bits of an `f32`.
        bias_bits: u32,
    },
    /// Octahedral mapping in lanes 0 and 1: each stored as `e * stored.scale + stored.bias` (the producer's scale and
    /// bias, as measured) and read back by the engine as `v * read.scale + read.bias` before the unfold (`z = 1 - |x| -
    /// |y|`, x and y folded where z < 0, normalised). The two halves are stated as each side measures them and need not
    /// be exact inverses: an engine may store `0.4995 e + 0.4995` and read `2 v - 1`. The common 0-1 layout is `stored`
    /// [`LaneAffine::UNORM`] and `read` [`LaneAffine::SIGNED`].
    Octahedral {
        /// The map from the unfolded coordinate to the stored value.
        stored: LaneAffine,
        /// The map from the stored value to the coordinate the unfold takes.
        read: LaneAffine,
    },
    /// Octahedral in the diamond layout over three 8-bit UNORM lanes: lanes 0 and 1 are the low eight bits of two 9-bit
    /// coordinates `u` and `v` (`round(255 (c + 1))`, 0 to 510 over -1 to 1), lane 2's bit 1 and bit 2 their ninth bits and
    /// its bit 0 the sign of z; the lane's other five bits are not part of the normal (a payload byte, which an engine
    /// may keep a class in). `x = (u + v) / 2`, `y = (u - v) / 2`, `|z| = 1 - |x| - |y|`, normalised
    /// ([`NormalEncoding::decode_octahedral_diamond`]).
    OctahedralDiamond,
    /// The normal by its major axis, over four lanes. The face is D3D's cube-map face of the axis whose component is
    /// largest in magnitude (`2 * axis + 1` where that component is negative: +x 0, -x 1, +y 2, -y 3, +z 4, -z 5), and
    /// lanes 0 and 1 are that face's `sc` and `tc` components of the normal, not divided by the major component (+x
    /// `(-z, -y)`, -x `(z, -y)`, +y `(x, z)`, -y `(x, -z)`, +z `(x, -y)`, -z `(-x, -y)`), each stored as `e *
    /// minor.scale + minor.bias`. Lane 2 is the major component's sign by its side of one half (below for a positive
    /// face, above for a negative one) and a payload `p` in 0 to 1 by its distance from one half, `p * payload.scale +
    /// payload.bias`; the payload is not part of the normal (a value an engine keeps beside it). Lane 3 is the axis
    /// over three (x 0, y 1/3, z 2/3). The major component's magnitude is the unit length's remainder
    /// ([`NormalEncoding::decode_cube_face`]).
    CubeFace {
        /// The map applied to the `sc` and `tc` components for lanes 0 and 1.
        minor: LaneAffine,
        /// The map applied to the payload value in lane 2.
        payload: LaneAffine,
    },
}
/// One lane's affine map, `v * scale + bias`, by the f32 bits of each so that it compares and hashes exactly; it is written
/// as the numbers `scale` and `bias`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LaneAffine {
    /// The scale, as the bits of an `f32`.
    pub scale_bits: u32,
    /// The bias, as the bits of an `f32`.
    pub bias_bits: u32,
}
impl Serialize for LaneAffine {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("LaneAffine", 2)?;
        out.serialize_field("scale", &self.scale())?;
        out.serialize_field("bias", &self.bias())?;
        out.end()
    }
}
impl LaneAffine {
    /// A signed value stored in 0 to 1: `v * 0.5 + 0.5`.
    pub const UNORM: Self = Self {
        scale_bits: 0x3F00_0000,
        bias_bits: 0x3F00_0000,
    };

    /// A value in 0 to 1 read back as signed: `v * 2 - 1`.
    pub const SIGNED: Self = Self {
        scale_bits: 0x4000_0000,
        bias_bits: 0xBF80_0000,
    };

    /// A map with this scale and bias.
    pub fn new(scale: f32, bias: f32) -> Self {
        Self {
            scale_bits: scale.to_bits(),
            bias_bits: bias.to_bits(),
        }
    }

    /// The scale.
    pub fn scale(&self) -> f32 {
        f32::from_bits(self.scale_bits)
    }

    /// The bias.
    pub fn bias(&self) -> f32 {
        f32::from_bits(self.bias_bits)
    }

    /// `v * scale + bias`.
    pub fn apply(&self, v: f32) -> f32 {
        v * self.scale() + self.bias()
    }
}
/// An axis of a normal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Axis {
    X,
    Y,
    Z,
}
impl Axis {
    pub(super) const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    /// This axis's component of a vector.
    pub(super) fn of<T: Copy>(self, [x, y, z]: [T; 3]) -> T {
        match self {
            Axis::X => x,
            Axis::Y => y,
            Axis::Z => z,
        }
    }
}

/// A face of the cube map of [`NormalEncoding::CubeFace`]: the axis of the normal's major component and, for lanes 0
/// and 1, the axis and sign of the normal's component each holds.
#[derive(Clone, Copy, Debug)]
pub(super) struct CubeFace {
    pub(super) major: Axis,
    pub(super) minor: [(Axis, f64); 2],
}
impl CubeFace {
    /// The face whose major component is on `axis`, facing the negative side or not.
    pub(super) fn new(axis: Axis, negative: bool) -> CubeFace {
        use Axis::{X, Y, Z};
        let (major, minor) = match (axis, negative) {
            (X, false) => (X, [(Z, -1.0), (Y, -1.0)]),
            (X, true) => (X, [(Z, 1.0), (Y, -1.0)]),
            (Y, false) => (Y, [(X, 1.0), (Z, 1.0)]),
            (Y, true) => (Y, [(X, 1.0), (Z, -1.0)]),
            (Z, false) => (Z, [(X, 1.0), (Y, -1.0)]),
            (Z, true) => (Z, [(X, -1.0), (Y, -1.0)]),
        };
        CubeFace { major, minor }
    }

    /// A vector with `major` on the major axis, `first` and `second` on the axes lanes 0 and 1 hold, `none` on the other.
    pub(super) fn place<T: Copy>(&self, major: T, first: T, second: T, none: T) -> [T; 3] {
        let [(a, _), (b, _)] = self.minor;
        Axis::ALL.map(|axis| {
            if axis == self.major {
                major
            } else if axis == a {
                first
            } else if axis == b {
                second
            } else {
                none
            }
        })
    }
}
/// The 8-bit value an UNORM lane rounds to.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=255 first
fn unorm8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}

/// `v` rounded to a whole number of 0 to 510 (nine bits less one).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=510 first
fn nine_bits(v: f64) -> u16 {
    v.round().clamp(0.0, 510.0) as u16
}

/// `v` as an f32, rounded to the nearest.
#[allow(clippy::cast_possible_truncation)] // the rounding is the point; the caller's values are lane values
pub(super) fn narrow(v: f64) -> f32 {
    v as f32
}
impl NormalEncoding {
    /// The unit normal and the payload of four lanes stored as [`NormalEncoding::CubeFace`] with `minor` and
    /// `payload`; `steps` is each lane's quantisation step in the target's format (1/1023 for a 10-bit UNORM lane,
    /// 1/3 for a 2-bit one). Lane 3 is read as the nearest of 0, 1/3 and 2/3, and lane 2's side of one half as the
    /// major component's sign; a lane 3 not within half its step of one of them, or a lane 2 within one of its steps of
    /// one half, cannot be placed and is refused.
    pub fn decode_cube_face(
        minor: LaneAffine,
        payload: LaneAffine,
        lanes: [f32; 4],
        steps: [f32; 4],
    ) -> Result<([f64; 3], f32), String> {
        let [a, b, side, lane_3] = lanes;
        let thirds = f64::from(lane_3) * 3.0;
        let nearest = thirds.round();
        let [_, _, step_side, step_axis] = steps;
        // by an exact comparison: a NaN, or an axis past z, is no axis
        let axis_index = [0.0, 1.0, 2.0].iter().position(|&candidate| candidate == nearest);
        let axis = axis_index
            .and_then(|i| Axis::ALL.get(i))
            .filter(|_| (thirds - nearest).abs() / 3.0 < f64::from(step_axis) / 2.0);
        let Some(&axis) = axis else {
            return Err(format!(
                "a cube-face lane 3 of {lane_3} is not an axis (0, 1/3 or 2/3 within half a step of {step_axis})"
            ));
        };
        let from_half = f64::from(side) - 0.5;
        if from_half.abs() < f64::from(step_side) {
            return Err(format!(
                "a cube-face lane 2 of {side} is within a step ({step_side}) of one half: its sign cannot be read"
            ));
        }
        let negative = from_half > 0.0;
        let face = CubeFace::new(axis, negative);
        let unmap = |v: f32| (f64::from(v) - f64::from(minor.bias())) / f64::from(minor.scale());
        let (sc, tc) = (unmap(a), unmap(b));
        let [(_, first_sign), (_, second_sign)] = face.minor;
        let depth = (1.0 - sc * sc - tc * tc).max(0.0).sqrt() * if negative { -1.0 } else { 1.0 };
        let n = face.place(depth, sc * first_sign, tc * second_sign, 0.0);
        let length = n.iter().map(|c| c * c).sum::<f64>().sqrt();
        let p = ((narrow(from_half.abs()) - payload.bias()) / payload.scale()).clamp(0.0, 1.0);
        Ok((n.map(|c| c / length), p))
    }

    /// The four lanes [`NormalEncoding::CubeFace`] with `minor` and `payload` stores the unit normal `normal` and the
    /// payload `p` (0 to 1) as, unquantised. The major axis is z where its magnitude is at least x's and y's, else y
    /// where it is at least x's, else x (on an edge between faces either face decodes to the normal).
    pub fn encode_cube_face(minor: LaneAffine, payload: LaneAffine, normal: [f32; 3], p: f32) -> [f32; 4] {
        let [x, y, z] = normal;
        let (axis, thirds) = if z.abs() >= x.abs() && z.abs() >= y.abs() {
            (Axis::Z, 2.0)
        } else if y.abs() >= x.abs() {
            (Axis::Y, 1.0)
        } else {
            (Axis::X, 0.0)
        };
        let negative = axis.of(normal) < 0.0;
        let [first, second] = CubeFace::new(axis, negative).minor;
        let lane =
            |(of, sign): (Axis, f64)| minor.apply(if sign < 0.0 { -of.of(normal) } else { of.of(normal) });
        let offset = p * payload.scale() + payload.bias();
        [
            lane(first),
            lane(second),
            if negative { 0.5 + offset } else { 0.5 - offset },
            thirds / 3.0,
        ]
    }

    /// The normal and the payload byte of three UNORM lanes stored as [`NormalEncoding::OctahedralDiamond`]: each lane
    /// is taken as the 8-bit value it rounds to.
    pub fn decode_octahedral_diamond(lanes: [f32; 3]) -> ([f64; 3], u8) {
        let [r, g, b] = lanes.map(unorm8);
        let u9 = f64::from(b >> 1 & 1) * 256.0 + f64::from(r);
        let v9 = f64::from(b >> 2 & 1) * 256.0 + f64::from(g);
        let pu = (u9 / 255.0 - 1.0).clamp(-1.0, 1.0);
        let pv = (v9 / 255.0 - 1.0).clamp(-1.0, 1.0);
        let (x, y) = ((pu + pv) / 2.0, (pu - pv) / 2.0);
        let z = (1.0 - x.abs() - y.abs()) * if b & 1 == 1 { 1.0 } else { -1.0 };
        let n = (x * x + y * y + z * z).sqrt();
        ([x / n, y / n, z / n], b >> 3)
    }

    /// The three 8-bit lanes [`NormalEncoding::OctahedralDiamond`] stores a normal and a payload byte (below 32) as;
    /// a z of 0 stores as positive. A vector that is not zero is normalised by the encoding itself.
    pub fn encode_octahedral_diamond(normal: [f64; 3], payload: u8) -> [u8; 3] {
        let [x, y, z] = normal;
        let sum = x.abs() + y.abs() + z.abs();
        let (x, y) = (x / sum, y / sum);
        let nine = |c: f64| nine_bits(c * 255.0 + 255.0);
        let (u9, v9) = (nine(x + y), nine(x - y));
        let sign = u8::from(z >= 0.0);
        [
            (u9 & 0xff) as u8,
            (v9 & 0xff) as u8,
            sign | ((u9 >> 8) as u8) << 1 | ((v9 >> 8) as u8) << 2 | (payload & 0x1f) << 3,
        ]
    }
}
/// How a scalar is stored in the lanes of an output: `stored = (value / scale)^(root / power)`, so the value is
/// `stored^(power / root) * scale` (the scale given by its f32 bits). Power and root 1 are linear; power 2 a square root
/// stored (a value kept in a perceptual range), 4 a fourth root; root 2 a square stored (a value the lighting takes the
/// square root of).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScalarEncoding {
    /// The power the stored value is raised to, to give the value over the scale: 1 for none, 2 for the square root
    /// stored, 4 for the fourth root. Never 0.
    pub power: u8,
    /// The root taken of the stored value raised to `power`: 1 for none, 2 for the square stored. Never 0.
    pub root: u8,
    /// The scale's f32 bits (1.0 for none): bits so the encoding compares and hashes exactly; it is written as the
    /// number `scale`.
    pub scale_bits: u32,
}
impl Serialize for ScalarEncoding {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        // `root` is written only where it is not 1, so an encoding without one reads as it always has
        let mut out = serializer.serialize_struct(
            "ScalarEncoding",
            2usize.saturating_add(usize::from(self.root != 1)),
        )?;
        out.serialize_field("power", &self.power)?;
        if self.root != 1 {
            out.serialize_field("root", &self.root)?;
        } else {
            out.skip_field("root")?;
        }
        out.serialize_field("scale", &f32::from_bits(self.scale_bits))?;
        out.end()
    }
}
impl ScalarEncoding {
    /// The value as it is (scale 1).
    pub const LINEAR: Self = Self {
        power: 1,
        root: 1,
        scale_bits: 0x3F80_0000,
    };

    /// Stored as the value over `scale`, no power.
    pub fn linear_over(scale: f32) -> Self {
        Self::power_of(1, scale)
    }

    /// Stored as the square root of the value over `scale`.
    pub fn square_root_of(scale: f32) -> Self {
        Self::power_of(2, scale)
    }

    /// Stored as the `power`th root of the value over `scale` (`power` 0 is taken as 1: no power).
    pub fn power_of(power: u8, scale: f32) -> Self {
        Self {
            power: power.max(1),
            root: 1,
            scale_bits: scale.to_bits(),
        }
    }

    /// Stored as the square of the value over `scale`: the value is the stored one's square root times `scale`.
    pub fn square_of(scale: f32) -> Self {
        Self {
            power: 1,
            root: 2,
            scale_bits: scale.to_bits(),
        }
    }

    /// The value for a stored one (a root of a negative stored value is taken of 0).
    pub fn decode(&self, stored: f32) -> f32 {
        let raised = stored.powi(i32::from(self.power));
        let value = match self.root {
            0 | 1 => raised,
            2 => raised.max(0.0).sqrt(),
            root => raised.max(0.0).powf(1.0 / f32::from(root)),
        };
        value * f32::from_bits(self.scale_bits)
    }
}
impl ScalarEncoding {
    /// Whether the encoding is one: a power and a root other than 0, and a finite scale.
    pub fn check(&self) -> Result<(), String> {
        let scale = f32::from_bits(self.scale_bits);
        if self.power == 0 || self.root == 0 || !scale.is_finite() {
            return Err(format!(
                "an encoding of power {}, root {} and scale {scale}: power and root are never 0, the scale is finite",
                self.power, self.root
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "hand-worked values")]
mod tests {
    use super::*;

    /// An octahedral encoding states both of its maps, by bits, and writes them as numbers; the 0-1 layout's maps are
    /// the constants, and they invert each other.
    #[test]
    fn an_octahedral_encoding_states_its_stored_and_read_maps() {
        assert_eq!(LaneAffine::UNORM, LaneAffine::new(0.5, 0.5));
        assert_eq!(LaneAffine::SIGNED, LaneAffine::new(2.0, -1.0));
        for e in [-1.0f32, -0.25, 0.0, 0.5, 1.0] {
            assert_eq!(LaneAffine::SIGNED.apply(LaneAffine::UNORM.apply(e)), e);
        }
        let measured = NormalEncoding::Octahedral {
            stored: LaneAffine::new(0.4995, 0.4995),
            read: LaneAffine::SIGNED,
        };
        assert_ne!(
            measured,
            NormalEncoding::Octahedral {
                stored: LaneAffine::UNORM,
                read: LaneAffine::SIGNED,
            }
        );
        assert_eq!(
            serde_json::to_value(measured).unwrap(),
            serde_json::json!({"octahedral": {
                "stored": {"scale": 0.4995f32, "bias": 0.4995f32},
                "read": {"scale": 2.0, "bias": -1.0},
            }})
        );
    }
    /// The cube-face store as the source's G-buffer program writes it (`minor` 511/1023 both, `payload` 0.49804688 and
    /// 0.0014662757, the program's literals) gives the program's own output for the ten normals it was evaluated at
    /// with a payload of 0, and decodes back to them; every face, the edges and corners between faces and a grid of
    /// normals round-trip through 10-bit lanes and a 2-bit lane 3; a lane 3 that is not an axis, and a lane 2 within a
    /// step of one half, are refused.
    #[test]
    fn the_cube_face_decode_inverts_its_encoding_and_the_measured_lanes() {
        let k = 511.0f32 / 1023.0;
        let minor = LaneAffine::new(k, k);
        assert_eq!(minor.scale_bits, 0x3EFF_BFF0);
        let payload = LaneAffine::new(0.498_046_88, 0.001_466_275_7);
        let steps = [1.0 / 1023.0, 1.0 / 1023.0, 1.0 / 1023.0, 1.0 / 3.0];
        let encoding = NormalEncoding::CubeFace { minor, payload };
        assert_eq!(
            serde_json::to_value(encoding).unwrap()["cube_face"]["minor"]["scale"],
            serde_json::json!(k)
        );
        // the program's T0 for each normal (its TEXCOORD2), as the evaluation printed it
        let measured: [([f32; 3], [f32; 4]); 10] = [
            ([0.36, 0.48, 0.8], [0.679335, 0.259746, 0.498534, 0.666667]),
            ([0.36, 0.48, -0.8], [0.319687, 0.259746, 0.501466, 0.666667]),
            ([-0.48, 0.36, -0.8], [0.739277, 0.319687, 0.501466, 0.666667]),
            ([0.8, 0.36, 0.48], [0.259746, 0.319687, 0.498534, 0.0]),
            ([-0.8, 0.36, -0.48], [0.259746, 0.319687, 0.501466, 0.0]),
            ([0.36, 0.8, -0.48], [0.679335, 0.259746, 0.498534, 0.333333]),
            ([-0.48, -0.8, 0.36], [0.259746, 0.319687, 0.501466, 0.333333]),
            ([0.0, 0.0, -1.0], [0.499511, 0.499511, 0.501466, 0.666667]),
            ([0.0, 0.0, 1.0], [0.499511, 0.499511, 0.498534, 0.666667]),
            ([-0.6, 0.0, -0.8], [0.799218, 0.499511, 0.501466, 0.666667]),
        ];
        for (n, t0) in measured {
            let lanes = NormalEncoding::encode_cube_face(minor, payload, n, 0.0);
            assert!(
                lanes.iter().zip(&t0).all(|(a, b)| (a - b).abs() < 1e-6),
                "{n:?}: {lanes:?} against {t0:?}"
            );
            let (back, p) = NormalEncoding::decode_cube_face(minor, payload, t0, steps).unwrap();
            let dot: f64 = back.iter().zip(&n).map(|(a, b)| a * f64::from(*b)).sum();
            assert!(dot > 1.0 - 1e-6 && p < 1e-3, "{n:?}: {back:?} payload {p}");
        }
        let quantise = |lanes: [f32; 4]| {
            let q = |v: f32, levels: f32| (v * levels).round() / levels;
            [
                q(lanes[0], 1023.0),
                q(lanes[1], 1023.0),
                q(lanes[2], 1023.0),
                q(lanes[3], 3.0),
            ]
        };
        let round_trip = |n: [f64; 3], p: f32| {
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = n.map(|c| (c / length) as f32);
            let lanes = quantise(NormalEncoding::encode_cube_face(minor, payload, n, p));
            let (back, read) = NormalEncoding::decode_cube_face(minor, payload, lanes, steps).unwrap();
            let dot: f64 = back.iter().zip(&n).map(|(a, b)| a * f64::from(*b)).sum();
            (dot, (read - p).abs())
        };
        // the six faces, the twelve edges and the eight corners between them, with payloads 0, 1/2 and 1
        let mut worst = (1.0f64, 0.0f32);
        for x in [-1.0, 0.0, 1.0] {
            for y in [-1.0, 0.0, 1.0] {
                for z in [-1.0, 0.0, 1.0] {
                    if [x, y, z] == [0.0; 3] {
                        continue;
                    }
                    for p in [0.0, 0.5, 1.0] {
                        let (dot, off) = round_trip([x, y, z], p);
                        worst = (worst.0.min(dot), worst.1.max(off));
                    }
                }
            }
        }
        // and a grid over the sphere
        for i in 0..40 {
            for j in 0..80 {
                let (theta, phi) = (
                    std::f64::consts::PI * (f64::from(i) + 0.5) / 40.0,
                    std::f64::consts::TAU * f64::from(j) / 80.0,
                );
                let (dot, off) = round_trip(
                    [theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()],
                    0.25,
                );
                worst = (worst.0.min(dot), worst.1.max(off));
            }
        }
        // a 10-bit step of a minor component is about 2/1023: the direction is within a few of them
        assert!(
            worst.0 > 1.0 - 2e-5 && worst.1 < 2.0 / 1023.0,
            "worst dot {}, payload off {}",
            worst.0,
            worst.1
        );
        let refused =
            |lanes: [f32; 4]| NormalEncoding::decode_cube_face(minor, payload, lanes, steps).unwrap_err();
        assert!(refused([0.5, 0.5, 0.49, 1.0]).contains("is not an axis"));
        // between two thirds in a lane finer than 2 bits (a 10-bit lane 3)
        let fine = [steps[0], steps[1], steps[2], 1.0 / 1023.0];
        assert!(
            NormalEncoding::decode_cube_face(minor, payload, [0.5, 0.5, 0.49, 0.5], fine)
                .unwrap_err()
                .contains("is not an axis")
        );
        assert!(
            NormalEncoding::decode_cube_face(minor, payload, [0.5, 0.5, 0.49, 341.0 / 1023.0], fine).is_ok()
        );
        assert!(refused([0.5, 0.5, 0.5 + 0.5 / 1023.0, 0.0]).contains("its sign cannot be read"));
    }
    /// The diamond octahedral store round-trips: the hand-worked axes (+z stores x = y = 255 and the sign bit; +x
    /// stores u = v = 510, both ninth bits; -y stores u = 0, v = 510), the payload kept, and a grid of normals to
    /// the 9-bit step.
    #[test]
    fn the_diamond_octahedral_decode_inverts_its_encoding() {
        let unorm = |lanes: [u8; 3]| lanes.map(|v| f32::from(v) / 255.0);
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9);
        let (n, class) = NormalEncoding::decode_octahedral_diamond(unorm([255, 255, 5 << 3 | 1]));
        assert!(near(n, [0.0, 0.0, 1.0]) && class == 5, "{n:?}");
        let (n, _) = NormalEncoding::decode_octahedral_diamond(unorm([254, 254, 5 << 3 | 0b111]));
        assert!(near(n, [1.0, 0.0, 0.0]), "{n:?}");
        let (n, _) = NormalEncoding::decode_octahedral_diamond(unorm([0, 254, 5 << 3 | 0b101]));
        assert!(near(n, [0.0, -1.0, 0.0]), "{n:?}");
        assert_eq!(
            NormalEncoding::encode_octahedral_diamond([0.0, 0.0, 1.0], 5),
            [255, 255, 5 << 3 | 1]
        );
        assert_eq!(
            NormalEncoding::encode_octahedral_diamond([1.0, 0.0, 0.0], 5),
            [254, 254, 5 << 3 | 0b111]
        );
        assert_eq!(
            NormalEncoding::encode_octahedral_diamond([0.0, -1.0, 0.0], 5),
            [0, 254, 5 << 3 | 0b101]
        );
        // a grid over the sphere, both hemispheres, every payload class that matters
        let mut worst = 1.0f64;
        for i in 0..40 {
            for j in 0..80 {
                let (theta, phi) = (
                    std::f64::consts::PI * (f64::from(i) + 0.5) / 40.0,
                    std::f64::consts::TAU * f64::from(j) / 80.0,
                );
                let n = [theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()];
                let class = (i % 6) as u8;
                let lanes = NormalEncoding::encode_octahedral_diamond(n, class);
                let (back, payload) = NormalEncoding::decode_octahedral_diamond(unorm(lanes));
                assert_eq!(payload, class);
                worst = worst.min(n[0] * back[0] + n[1] * back[1] + n[2] * back[2]);
            }
        }
        // a 9-bit coordinate step is 2/510: the direction is within a few of them
        assert!(worst > 1.0 - 2e-4, "worst dot {worst}");
    }
    #[test]
    fn a_scalar_encoding_decodes_what_it_stores() {
        // stored = sqrt(value / 512): the value is the stored one squared times 512
        let e = ScalarEncoding::square_root_of(512.0);
        assert_eq!(e.decode(0.5), 128.0);
        assert_eq!(ScalarEncoding::LINEAR.decode(0.25), 0.25);
        assert_eq!(ScalarEncoding::square_root_of(1.0).decode(0.5), 0.25);
        // a fourth root over 4: the value is the stored one to the fourth, times 4
        let fourth = ScalarEncoding::power_of(4, 4.0);
        assert_eq!(fourth.decode(0.5), 0.25);
        assert_eq!(ScalarEncoding::linear_over(16.0).decode(0.25), 4.0);
        // written as the number it is, not its bit pattern
        assert_eq!(
            serde_json::to_value(fourth).unwrap(),
            serde_json::json!({"power": 4, "scale": 4.0})
        );
        assert_eq!(
            serde_json::to_value(ScalarEncoding::linear_over(0.0625)).unwrap(),
            serde_json::json!({"power": 1, "scale": 0.0625})
        );
        // a power of 0 is no power
        assert_eq!(ScalarEncoding::power_of(0, 2.0), ScalarEncoding::linear_over(2.0));
        // a square stored over 0.95: the value is the square root of the stored one, times 0.95; written with its root
        let square = ScalarEncoding::square_of(0.95);
        assert_eq!(square.decode(0.25), 0.475);
        assert_eq!(square.decode(-0.25), 0.0);
        assert_eq!(
            serde_json::to_value(square).unwrap(),
            serde_json::json!({"power": 1, "root": 2, "scale": 0.95f32})
        );
    }
}
