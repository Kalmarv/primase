use super::Member;
use crate::{Result, bytes::View, invalid};
use primase_vertex::Format;

/// Decoded attributes in file coordinates. Repeated attributes retain stream order.
#[derive(Clone, Debug, Default)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub normal_w: i32,
    pub bone_weights: [f32; 4],
    pub bone_indices: [u16; 4],
    pub uvs: Vec<[f32; 3]>,
    /// Byte4C UV payloads before the signed-short interpretation, for layer weights.
    pub layer_bytes: Vec<(usize, [u8; 4])>,
    pub tangents: Vec<[f32; 4]>,
    pub bitangent: [f32; 4],
    pub colors: Vec<[f32; 4]>,
    /// A SpeedTree vertex's stored members in layout order, each as its format gives it to a vertex program (four
    /// lanes, unused ones zero): half floats as floats, normalized unsigned bytes as `x / 255`, floats as stored.
    /// What each member stands for is the game's shader's, not the layout's semantic: the position, normal, one
    /// tangent and one UV set are set from them ([`super::speedtree_layout`]), a billboard card's are not.
    pub speedtree: Vec<[f32; 4]>,
}

pub(super) fn size(kind: u32) -> Result<usize> {
    Ok(match kind {
        0x10..=0x13 | 0x15 | 0x2d | 0x2f => 4,
        1 | 0x16 | 0x18 | 0x1a | 0x2e => 8,
        2 => 12,
        3 => 16,
        _ => return Err(invalid("FLVER", 0, format!("unsupported element {kind:#x}"))),
    })
}

impl Vertex {
    /// A SpeedTree vertex's stored members ([`Vertex::speedtree`]): `Float4` (3), `Float2` (1), `Half4` (0x2e),
    /// `Half2` (0x2d) and `UByte4Norm` (0x13), the formats Elden Ring's SpeedTree layouts store; others are refused.
    pub(super) fn read_speedtree(&mut self, data: &[u8], members: &[Member]) -> Result<()> {
        let v = View {
            data,
            format: "FLVER SpeedTree vertex",
            be: false,
        };
        let mut at = 0;
        for m in members.iter().filter(|m| m.stored) {
            let n = size(m.kind)?;
            let raw = v.slice(at, n)?;
            let mut lanes = [0f32; 4];
            match m.kind {
                1 | 3 => {
                    for (k, lane) in lanes.iter_mut().take(n / 4).enumerate() {
                        *lane = f32::from_bits(v.u32(at + k * 4)?);
                    }
                }
                0x2d | 0x2e => {
                    for (k, lane) in lanes.iter_mut().take(n / 2).enumerate() {
                        *lane = half(v.u16(at + k * 2)?);
                    }
                }
                0x13 => {
                    for (lane, &b) in lanes.iter_mut().zip(raw) {
                        *lane = f32::from(b) / 255.;
                    }
                }
                other => {
                    return Err(invalid(v.format, at, format!("SpeedTree member type {other:#x}")));
                }
            }
            if lanes.iter().any(|x| !x.is_finite()) {
                return Err(invalid(v.format, at, "non-finite SpeedTree member"));
            }
            self.speedtree.push(lanes);
            at += n;
        }
        Ok(())
    }

    /// A SpeedTree vertex's attributes from its stored members (indices into [`Vertex::speedtree`], the roles
    /// [`super::SpeedTreeLayout::Geometry`] names): the position and UV0 as stored, the normal unpacked, and the
    /// tangent of the ordinary programs' convention. A SpeedTree pixel program bends the normal by its normal map's
    /// `x` along the stored tangent `T` and its `y` along `-w (N × T)`, `w` the 1 its vertex program passes; an
    /// ordinary one bends it by `x` along `w (N × T')` and `y` along `T'`. `T' = T × N` with `w` 1 is the same frame
    /// where `T` is perpendicular to `N` (the stored pairs are within a byte's precision of it).
    pub(super) fn speedtree_attributes(&mut self, position: usize, uv: usize, normal: usize, tangent: usize) {
        let unpack = |m: [f32; 4]| [m[0] * 2. - 1., m[1] * 2. - 1., m[2] * 2. - 1.];
        let p = self.speedtree[position];
        let t = self.speedtree[uv];
        let n = unpack(self.speedtree[normal]);
        let s = unpack(self.speedtree[tangent]);
        self.position = [p[0], p[1], p[2]];
        self.uvs = vec![[t[0], t[1], 0.]];
        self.normal = n;
        self.tangents = vec![[
            s[1] * n[2] - s[2] * n[1],
            s[2] * n[0] - s[0] * n[2],
            s[0] * n[1] - s[1] * n[0],
            1.,
        ]];
    }

    pub(super) fn read(&mut self, data: &[u8], members: &[Member], uv_factor: f32) -> Result<()> {
        let v = View {
            data,
            format: "FLVER vertex",
            be: false,
        };
        for m in members {
            let o = m.offset;
            let raw = v.slice(o, size(m.kind)?)?;
            let decode = |f: Format| f.decode(raw).map_err(|e| invalid(v.format, o, e));
            let f = |i: usize| -> Result<f32> {
                let x = f32::from_bits(v.u32(o + i * 4)?);
                if !x.is_finite() {
                    return Err(invalid(v.format, o, "non-finite attribute"));
                }
                Ok(x)
            };
            let short = |i: usize| -> Result<f32> { Ok(v.u16(o + i * 2)? as i16 as f32) };
            let byte_norm = || -> [f32; 4] {
                std::array::from_fn(|i| (raw[i] as f32 - 127.) * f32::from_bits(0x3c01_0204))
            };
            let short_norm = || -> Result<[f32; 4]> {
                Ok([
                    short(0)? / 32767.,
                    short(1)? / 32767.,
                    short(2)? / 32767.,
                    short(3)? / 32767.,
                ])
            };
            match (m.semantic, m.kind) {
                (0, 2 | 3) => {
                    self.position = [f(0)?, f(1)?, f(2)?];
                    if m.kind == 3 && f(3)? != 0. {
                        return Err(invalid(v.format, o, "position W is not zero"));
                    }
                }
                (1, 0x10) => self.bone_weights = std::array::from_fn(|i| raw[i] as i8 as f32 / 127.),
                (1, 0x13) => self.bone_weights = decode(Format::Unorm8x4)?,
                (1, 0x16 | 0x1a) => self.bone_weights = short_norm()?,
                (2, 0x11 | 0x2f) => self.bone_indices = std::array::from_fn(|i| raw[i] as u16),
                (2, 0x18) => self.bone_indices = [v.u16(o)?, v.u16(o + 2)?, v.u16(o + 4)?, v.u16(o + 6)?],
                (3, 2 | 3) => {
                    self.normal = [f(0)?, f(1)?, f(2)?];
                    if m.kind == 3 {
                        let w = f(3)?;
                        self.normal_w = w as i32;
                        if w != self.normal_w as f32 {
                            return Err(invalid(v.format, o, "fractional normal W"));
                        }
                    }
                }
                (3, 0x10 | 0x11 | 0x13 | 0x2f) => {
                    let n = byte_norm();
                    self.normal = [n[0], n[1], n[2]];
                    self.normal_w = raw[3] as i32;
                }
                (3, 0x12) => {
                    self.normal = [
                        raw[3] as i8 as f32 / 127.,
                        raw[2] as i8 as f32 / 127.,
                        raw[1] as i8 as f32 / 127.,
                    ];
                    self.normal_w = raw[0] as i32;
                }
                (3, 0x1a | 0x2e) => {
                    for i in 0..3 {
                        self.normal[i] = if m.kind == 0x1a {
                            short(i)? / 32767.
                        } else {
                            (v.u16(o + i * 2)? as f32 - 32767.) / 32767.
                        };
                    }
                    self.normal_w = v.u16(o + 6)? as i16 as i32;
                }
                (5, 1) => self.uvs.push([f(0)?, f(1)?, 0.]),
                (5, 2) => self.uvs.push([f(0)?, f(1)?, f(2)?]),
                (5, 3) => {
                    self.uvs.push([f(0)?, f(1)?, 0.]);
                    self.uvs.push([f(2)?, f(3)?, 0.]);
                }
                (5, 0x10..=0x13 | 0x15 | 0x16) => {
                    if m.kind == 0x13 {
                        self.layer_bytes
                            .push((self.uvs.len(), [raw[0], raw[1], raw[2], raw[3]]));
                    }
                    self.uvs.push([short(0)? / uv_factor, short(1)? / uv_factor, 0.]);
                    if m.kind == 0x16 {
                        self.uvs.push([short(2)? / uv_factor, short(3)? / uv_factor, 0.]);
                    }
                }
                (5, 0x2e) => {
                    self.uvs.push([
                        short(0)? / uv_factor,
                        short(1)? / uv_factor,
                        short(2)? / uv_factor,
                    ]);
                    v.expect(o + 6, &[0, 0])?;
                }
                (6, 3) => self.tangents.push(decode(Format::Float4)?),
                (6, 0x10 | 0x11 | 0x13 | 0x2f) => self.tangents.push(byte_norm()),
                (6, 0x1a) => self.tangents.push(short_norm()?),
                (7, 0x10 | 0x11 | 0x13 | 0x2f) => self.bitangent = byte_norm(),
                (10, 3) => self.colors.push(decode(Format::Float4)?),
                (10, 0x10 | 0x13) => self.colors.push(decode(Format::Unorm8x4)?),
                _ => {
                    return Err(invalid(
                        v.format,
                        o,
                        format!("unsupported semantic {} / type {:#x}", m.semantic, m.kind),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// An IEEE 754 half-precision value (subnormals, infinities and NaN as such).
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1f32 } else { 1. };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 if mantissa == 0. => f32::INFINITY,
        31 => f32::NAN,
        e => (1. + mantissa / 1024.) * 2f32.powi(e - 15),
    }
}
