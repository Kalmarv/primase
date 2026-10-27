//! A Mesh's vertex attributes decoded as the engine's vertex declarations state them (Riders Republic docs/vertex.md,
//! "The engine's vertex declarations": the element lists, one per `VertexFormat`, which is the Mesh's kind byte, read
//! from the runtime image with their DXGI formats; each layout's vertex programs' arithmetic from their disassembly;
//! every layout's input signature the same semantics in all 432 templates' vertex permutations).
//!
//! - **static24** (kind 2, 24 bytes) and **static28** (kind 3, 28 bytes): `NORMAL` bytes 8..11 and `TANGENT` 12..15,
//!   `R8G8B8A8_UINT`, each component `(b - 127) / 127` (the programs' 1/127; their fourth byte isn't geometry: an unorm
//!   pair the fragment-recording pass reads); the bitangent's sign the sign of the position's scale word (the i16 at
//!   6); `COLOR0` bytes 16..19, `R8G8B8A8_UNORM` (byte 16 red); `TEXCOORD0` bytes 20..23, `R16G16_SNORM` times 16 (the
//!   programs' `uv = v * 16`), no flip; static28's `TEXCOORD1` bytes 24..27 likewise.
//! - **static16** (kind 11, 16 bytes, the far terrain's cells): its 4-byte `NORMAL` two unit vectors in spherical form,
//!   `b' = b / 127 - 1`: `P = (sqrt(1 - z^2) cos(pi a), sqrt(1 - z^2) sin(pi a), z)` with `a`, `z` bytes 10 and 11, `Q`
//!   likewise from 8 and 9 (which the pixel stage takes as the normal isn't read: both are kept); `TEXCOORD0` bytes
//!   12..15 as static24's. No tangent, no colour.
//! - **split12** (kind 0) and **split24** (kind 1), the primary stream: `COLOR0` bytes 0..3, the texture coordinates
//!   bytes 4..7 (`TEXCOORD1`, the UV the pixel stage gets), split24's second set bytes 8..11. Their position stream
//!   (12 bytes a vertex) holds the frame, as their layout-0 vertex program decodes it: the flag word the position's
//!   fourth word (`R16G16B16A16_SNORM` times 32767 to an unsigned integer: the stored i16 where it isn't negative,
//!   else 0); `NORMAL` bytes 8..11 `R8G8B8A8_UINT`, each `p = min(b / 127 - 1, 1)`; the normal from bytes 8, 9 and the
//!   tangent from 10, 11 as diamond vectors, `X = (p0 + p1) / 2`, `Y = (p0 - p1) / 2`, `Z = 1 - |X| - |Y|` negated
//!   where flag bit 0 (the normal's) or bit 1 (the tangent's) is set, normalised; the bitangent's sign -1 where bit 2 is
//!   set.
//!
//! An `R16G16_SNORM` word is `max(w / 32767, -1)`.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// A Mesh's vertex attributes as the engine's vertex declarations decode them ([`crate::vertex`]; each empty where
/// its layout declares none), in the source's frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VertexAttributes {
    /// Unnormalised, as the programs read them.
    pub normals: Vec<[f32; 3]>,
    /// xyz unnormalised, w the bitangent's sign (+1 or -1).
    pub tangents: Vec<[f32; 4]>,
    /// The texture coordinates of `TEXCOORD0` (or the split layouts' first set), then the second set.
    pub uvs: [Vec<[f32; 2]>; 2],
    /// RGBA, UNORM8.
    pub colour: Vec<[u8; 4]>,
    /// static16's two spherical vectors: `P` (bytes 10, 11) and `Q` (bytes 8, 9).
    pub spherical: Vec<[[f32; 3]; 2]>,
}

/// An `R16G16_SNORM` word's value.
fn snorm16(w: [u8; 2]) -> f32 {
    (f32::from(i16::from_le_bytes(w)) / 32767.0).max(-1.0)
}

/// A texture coordinate pair at `at` of `r`: two `R16G16_SNORM` words times 16.
fn uv(r: &[u8], at: usize) -> [f32; 2] {
    let word = |k: usize| -> [u8; 2] {
        r.get(at.saturating_add(k)..at.saturating_add(k).saturating_add(2))
            .and_then(|w| w.try_into().ok())
            .unwrap_or([0; 2])
    };
    [snorm16(word(0)) * 16.0, snorm16(word(2)) * 16.0]
}

/// Bytes `at..at + 4` of `r`.
fn four(r: &[u8], at: usize) -> [u8; 4] {
    r.get(at..at.saturating_add(4))
        .and_then(|w| w.try_into().ok())
        .unwrap_or([0; 4])
}

/// An `R8G8B8A8_UINT` vector's first three components as the programs decode them, `(b - 127) / 127`.
fn centred(b: [u8; 4]) -> [f32; 3] {
    let [x, y, z, _] = b;
    [x, y, z].map(|c| (f32::from(c) - 127.0) / 127.0)
}

/// A spherical unit vector from its azimuth byte `a` and height byte `z` (`b' = b / 127 - 1`), in f32 as the program.
fn spherical(a: u8, z: u8) -> [f32; 3] {
    let decode = |b: u8| f32::from(b) * (1.0f32 / 127.0) - 1.0;
    let (a, z) = (decode(a), decode(z));
    let r = (1.0 - z * z).max(0.0).sqrt();
    let (sin, cos) = (std::f32::consts::PI * a).sin_cos();
    [r * cos, r * sin, z]
}

/// A diamond vector from its two components and the sign of its Z, normalised.
fn diamond(p: f32, q: f32, z_sign: f32) -> [f32; 3] {
    let (x, y) = (0.5 * (p + q), 0.5 * (p - q));
    let v = [x, y, z_sign * (1.0 - x.abs() - y.abs())];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 { v.map(|c| c / len) } else { v }
}

/// A split layout's normal and tangent (with its bitangent sign) from its position-stream record `r` (see the module
/// documentation).
pub fn split_frame(r: &[u8; 12]) -> ([f32; 3], [f32; 4]) {
    let [.., w0, w1, b0, b1, b2, b3] = *r;
    let raw = i16::from_le_bytes([w0, w1]);
    let flags = u16::try_from(raw).unwrap_or(0);
    let sign = |bit: u16| if flags & bit != 0 { -1.0 } else { 1.0 };
    let p = [b0, b1, b2, b3].map(|b| (f32::from(b) * (1.0f32 / 127.0) - 1.0).min(1.0));
    let [p0, p1, p2, p3] = p;
    let normal = diamond(p0, p1, sign(1));
    let [x, y, z] = diamond(p2, p3, sign(2));
    (normal, [x, y, z, sign(4)])
}

/// The attributes of `records` (the primary stream, `stride` bytes each) of a layout of kind `kind`, the split layouts'
/// frame from their `position_stream` (12 bytes a vertex); none for a layout not listed above.
pub fn attributes(kind: u8, stride: usize, records: &[u8], position_stream: &[u8]) -> VertexAttributes {
    let each = || records.chunks_exact(stride.max(1));
    let mut a = VertexAttributes::default();
    match (kind, stride) {
        (2, 24) | (3, 28) => {
            for r in each() {
                let w = i16::from_le_bytes([r.get(6).copied().unwrap_or(0), r.get(7).copied().unwrap_or(0)]);
                let sign = if w < 0 { -1.0 } else { 1.0 };
                a.normals.push(centred(four(r, 8)));
                let [x, y, z] = centred(four(r, 12));
                a.tangents.push([x, y, z, sign]);
                a.colour.push(four(r, 16));
                a.uvs[0].push(uv(r, 20));
                if kind == 3 {
                    a.uvs[1].push(uv(r, 24));
                }
            }
        }
        (11, 16) => {
            for r in each() {
                let [b8, b9, b10, b11] = four(r, 8);
                a.spherical.push([spherical(b10, b11), spherical(b8, b9)]);
                a.uvs[0].push(uv(r, 12));
            }
        }
        (0, 12) | (1, 24) => {
            for r in each() {
                a.colour.push(four(r, 0));
                a.uvs[0].push(uv(r, 4));
                if kind == 1 {
                    a.uvs[1].push(uv(r, 8));
                }
            }
            for r in position_stream.as_chunks::<12>().0 {
                let (n, t) = split_frame(r);
                a.normals.push(n);
                a.tangents.push(t);
            }
        }
        _ => {}
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The split frame worked by hand from the layout-0 program's arithmetic: bytes 8..11 = (254, 127, 127, 254) give
    /// p = (1, 0, 0, 1); the normal X = Y = 0.5, Z = 0, so (1, 1, 0) / sqrt 2; the tangent X = 0.5, Y = -0.5, Z = 0; flags 0b101
    /// negate the normal's Z (0 here) and set the bitangent's sign -1; byte 255 caps at 1; a negative flag word is 0.
    #[test]
    fn split_frame_as_the_program_decodes_it() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let record = |flags: i16, b: [u8; 4]| {
            let [w0, w1] = flags.to_le_bytes();
            [0, 0, 0, 0, 0, 0, w0, w1, b[0], b[1], b[2], b[3]]
        };
        let (n, t) = split_frame(&record(0b101, [254, 127, 127, 254]));
        assert_eq!((n, t), ([h, h, 0.0], [h, -h, 0.0, -1.0]));
        // straight up and down: p0 = p1 = 0 gives Z = 1, negated by flag bit 0
        let (n, _) = split_frame(&record(0b001, [127, 127, 127, 127]));
        assert_eq!(n, [0.0, 0.0, -1.0]);
        let (n, t) = split_frame(&record(-3, [127, 127, 127, 127]));
        assert_eq!((n, t[3]), ([0.0, 0.0, 1.0], 1.0));
        // 255 reads 1.0079, capped at 1: the same as 254
        assert_eq!(
            split_frame(&record(0, [255, 127, 0, 0])).0,
            split_frame(&record(0, [254, 127, 0, 0])).0
        );
    }
}
