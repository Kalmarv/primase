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
    /// The UV members in stream order, each as one four-lane attribute the way the game's vertex program receives it
    /// (`TEXCOORDn.xyzw`; a lane its format has no value for is zero). A `Short2`, `Byte4A`, `Byte4B` or `Short2toFloat2`
    /// member is two int16 lanes over the UV factor (the program's integer input, scaled by 1/2048 in the program) and a
    /// `Float2` or `Float3` member its floats; a `Float4` or `UVPair` member gives two such attributes. A `UByte4Norm`
    /// (`0x13`) member is four normalised unsigned bytes (`b / 255`), which the program reads as a float `TEXCOORDn` and
    /// passes on unscaled: Elden Ring's `M[AMSN_V][Mb3][Ov_N]_Gbuf.vpo` (`TEXCOORD1 xyzw float`, loaded by
    /// `dx.op.loadInput.f32` at lines 288-290 of its disassembly and stored to the output unchanged at lines 501-503) and
    /// Sekiro's `M_MultipleGround__Gbuf.vpo` (`TEXCOORD1..6` float, `mov o5.x, v5.x` and the like, lines 222-228) alike. A
    /// `Half4` (`0x2e`) member is four IEEE half floats, which is also a float `TEXCOORDn xyzw` in the program with no
    /// scale: Elden Ring's `M[AMSN_V]_Vesitation_Emissive_Glow_Gbuf.vpo` declares `TEXCOORD3 xyzw float`, loads it with
    /// `dx.op.loadInput.f32` (lines 400-402), multiplies and adds it into a vector (lines 575-585) and stores it
    /// unchanged (lines 962-964); no 1/2048 or 1/1024 follows the load, as one does for the `Short2` member
    /// (`TEXCOORD0 int`, `sitofp` then a multiply by 2^-11). Read as int16 over the UV factor (SoulsFormats'
    /// `Short4toFloat4B`, SoulsFormatsNEXT's mislabelled `Half4` UV branch) the first mesh's vector would be
    /// (5.5, -11.9, -8.8) for an asset whose vertices span 0.1; as halves it is (0.065, -0.010, -0.721).
    pub uvs: Vec<[f32; 4]>,
    /// The four stored bytes of each `UByte4Norm` UV member and the index in [`Vertex::uvs`] of its attribute, for layer
    /// weights (the same values as that attribute's lanes before the `/ 255`).
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
    /// [`super::SpeedTreeLayout::Geometry`] names): the position and UV0 as stored, and the normal and tangent
    /// ([`Vertex::speedtree_frame`]).
    pub(super) fn speedtree_attributes(&mut self, position: usize, uv: usize, normal: usize, tangent: usize) {
        let p = self.speedtree[position];
        let t = self.speedtree[uv];
        self.position = [p[0], p[1], p[2]];
        self.uvs = vec![[t[0], t[1], 0., 0.]];
        self.speedtree_frame(normal, tangent);
    }

    /// A SpeedTree vertex's normal and tangent from the stored members of indices `normal` and `tangent`, unpacked
    /// as its programs unpack them (`x * 2 - 1`), the tangent in the ordinary programs' convention. A SpeedTree pixel
    /// program bends the normal by its normal map's `x` along the stored tangent `T` and its `y` along `-w (N × T)`,
    /// `w` the 1 its vertex program passes; an ordinary one bends it by `x` along `w (N × T')` and `y` along `T'`.
    /// `T' = T × N` with `w` 1 is the same frame where `T` is perpendicular to `N` (the stored pairs are within a
    /// byte's precision of it).
    pub(super) fn speedtree_frame(&mut self, normal: usize, tangent: usize) {
        let unpack = |m: [f32; 4]| [m[0] * 2. - 1., m[1] * 2. - 1., m[2] * 2. - 1.];
        let n = unpack(self.speedtree[normal]);
        let s = unpack(self.speedtree[tangent]);
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
                (5, 1) => self.uvs.push([f(0)?, f(1)?, 0., 0.]),
                (5, 2) => self.uvs.push([f(0)?, f(1)?, f(2)?, 0.]),
                (5, 3) => {
                    self.uvs.push([f(0)?, f(1)?, 0., 0.]);
                    self.uvs.push([f(2)?, f(3)?, 0., 0.]);
                }
                (5, 0x10..=0x12 | 0x15 | 0x16) => {
                    self.uvs
                        .push([short(0)? / uv_factor, short(1)? / uv_factor, 0., 0.]);
                    if m.kind == 0x16 {
                        self.uvs
                            .push([short(2)? / uv_factor, short(3)? / uv_factor, 0., 0.]);
                    }
                }
                (5, 0x13) => {
                    self.layer_bytes
                        .push((self.uvs.len(), [raw[0], raw[1], raw[2], raw[3]]));
                    self.uvs.push(std::array::from_fn(|i| f32::from(raw[i]) / 255.));
                }
                (5, 0x2e) => {
                    let lanes = [
                        half(v.u16(o)?),
                        half(v.u16(o + 2)?),
                        half(v.u16(o + 4)?),
                        half(v.u16(o + 6)?),
                    ];
                    if lanes.iter().any(|x: &f32| !x.is_finite()) {
                        return Err(invalid(v.format, o, "non-finite half-float UV member"));
                    }
                    self.uvs.push(lanes);
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
