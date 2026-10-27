//! The packed tangent frame of skate.'s vertices: element usage 52 ([`meshset::usage::TANGENT_SPACE`]), format
//! UByte4N, four bytes a vertex. Read with [`decode`], written with [`encode`].
//!
//! ```text
//! byte 3: bits 0-2 the normal's largest axis and its sign (code 0..5: +x -x +y -y +z -z), bit 3 the bitangent sign
//!         (set: -1), bits 4 and 5 the ninth bits of the two other normal components, bits 6-7 the angle's quadrant
//! bytes 0, 1: the two other normal components (axes per code: z,y  z,y  x,z  x,z  x,y  x,y), each a 9-bit value
//!         q: (q - 256) / (255 * sqrt 2)
//! byte 2: the angle within its quadrant, 0..255 for 0..90 degrees
//! ```
//!
//! The normal's largest component is the rest of its unit length. The tangent is a reference axis per code (+z -z +x
//! -x -x +x) made perpendicular to the normal and turned about it by the angle (`t0 cos a + (n x t0) sin a`); the
//! bitangent is `sign * (n x t)`.
//!
//! Read from ReSkate Studio 20261012's decoder (`reskate_cli`, unpacked, FUN_140275160, constants at 0x1404fded0,
//! 0x1404fdf30, 0x1404fdf98..0x1404fdfb8), and checked against the game's own meshes: on the LOD-0 sections of 120
//! Quick Drop meshes the decoded normal lies along the face normal (median dot 0.998), the tangent along the
//! direction U grows (median 1.000), the bitangent along V's (median 0.998). Re-encoded, the decoded frames of 60
//! one-LOD Quick Drop meshes give back 22,085 of their 23,704 stored frames byte for byte and the rest as the same
//! frame: an angle on the edge of a quadrant (step 255, or the next quadrant's 0) or a normal between two axes has
//! two encodings (`tests/rigid.rs`).

use crate::meshset;

/// A vertex's tangent frame: unit normal, unit tangent, and the bitangent's sign (+1 or -1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
    pub sign: f32,
}

/// The two smaller normal components' axes, by code.
const AXES: [(usize, usize); 6] = [(2, 1), (2, 1), (0, 2), (0, 2), (0, 1), (0, 1)];
/// The reference tangent, by code: its axis and direction.
const REFERENCE: [(usize, f64); 6] = [(2, 1.0), (2, -1.0), (0, 1.0), (0, -1.0), (0, -1.0), (0, 1.0)];
/// A smaller component's scale: 255 steps a 1/sqrt 2.
const SCALE: f64 = 255.0 * std::f64::consts::SQRT_2;
/// Angle steps a quadrant.
const STEPS: u32 = 255;

const _: () = assert!(meshset::usage::TANGENT_SPACE == 52);

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-12 && l.is_finite()).then(|| a.map(|c| c / l))
}

/// The normal that `code` and the two 9-bit values name.
fn normal(code: usize, qi: u32, qj: u32) -> [f64; 3] {
    let (i, j) = AXES[code];
    let mut n = [0.0; 3];
    n[i] = (f64::from(qi) - 256.0) / SCALE;
    n[j] = (f64::from(qj) - 256.0) / SCALE;
    let rest = (1.0 - n[i] * n[i] - n[j] * n[j]).max(0.0).sqrt();
    n[code >> 1] = if code & 1 == 1 { -rest } else { rest };
    n
}

/// The tangent at angle 0 for normal `n` of `code`: the code's reference axis made perpendicular to `n`.
fn base(code: usize, n: [f64; 3]) -> [f64; 3] {
    let (axis, dir) = REFERENCE[code];
    let mut r = [0.0; 3];
    r[axis] = dir;
    let d = dot(n, r);
    let t = [r[0] - n[0] * d, r[1] - n[1] * d, r[2] - n[2] * d];
    unit(t).unwrap_or(t)
}

/// The frame stored in `b`.
pub fn decode(b: [u8; 4]) -> Frame {
    let code = usize::from(b[3] & 7).min(5);
    let qi = u32::from(b[3] >> 4 & 1) << 8 | u32::from(b[0]);
    let qj = u32::from(b[3] >> 5 & 1) << 8 | u32::from(b[1]);
    let n = normal(code, qi, qj);
    let t0 = base(code, n);
    let degrees = f64::from(b[2]) * 90.0 / f64::from(STEPS) + f64::from(b[3] >> 6) * 90.0;
    let (s, c) = degrees.to_radians().sin_cos();
    let nt = cross(n, t0);
    let t = [0, 1, 2].map(|k| nt[k] * s + t0[k] * c);
    Frame {
        normal: n.map(|v| v as f32),
        tangent: t.map(|v| v as f32),
        sign: if b[3] & 8 != 0 { -1.0 } else { 1.0 },
    }
}

/// The bytes of a frame: `normal` (made unit; refused when it has no length), `tangent` (its part perpendicular to the
/// normal; when that has no length, the angle is 0) and the bitangent's sign (negative: -1, else +1). The normal's
/// code is its largest axis (the first of equals), the tangent's angle the nearest step.
pub fn encode(normal_: [f32; 3], tangent: [f32; 3], sign: f32) -> Option<[u8; 4]> {
    let n = unit(normal_.map(f64::from))?;
    let axis = (0..3).fold(0, |best, k| if n[k].abs() > n[best].abs() { k } else { best });
    let code = 2 * axis + usize::from(n[axis] < 0.0);
    let (i, j) = AXES[code];
    let q = |v: f64| (v * SCALE + 256.0).round().clamp(0.0, 511.0) as u32;
    let (qi, qj) = (q(n[i]), q(n[j]));
    // the angle against the frame the decoder rebuilds from the stored normal
    let nq = normal(code, qi, qj);
    let t0 = base(code, nq);
    let t = tangent.map(f64::from);
    let along = dot(t, nq);
    let tp = [t[0] - nq[0] * along, t[1] - nq[1] * along, t[2] - nq[2] * along];
    let degrees = match unit(tp) {
        Some(tp) => dot(tp, cross(nq, t0))
            .atan2(dot(tp, t0))
            .to_degrees()
            .rem_euclid(360.0),
        None => 0.0,
    };
    let steps = (degrees * f64::from(STEPS) / 90.0).round() as u32 % (4 * STEPS);
    let (quadrant, step) = (steps / STEPS, steps % STEPS);
    Some([
        (qi & 0xff) as u8,
        (qj & 0xff) as u8,
        step as u8,
        code as u8
            | u8::from(sign < 0.0) << 3
            | ((qi >> 8) as u8) << 4
            | ((qj >> 8) as u8) << 5
            | (quadrant as u8) << 6,
    ])
}
