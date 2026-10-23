//! METAPARAM (`SMD`, version 6): what a shader bundle's programs need to be bound, as the bundle's
//! `<shader>.metaparam` states it. Its texture slots (name, default texture, UV group, slot, type), the sampler masks
//! each render pass binds per stage, and the parameters the programs read through `SAT_CAParam` and `SAT_MtdParam`
//! (offsets in the constant buffer, defaults, name hashes). Every length, offset and zero field is checked; a file
//! that breaks one is refused with its offset. Names follow the third-party SoulsFormatsNEXT reader of the format;
//! a field it calls unknown is kept under its offset.
use crate::{Result, bytes::View, invalid};

const FORMAT: &str = "METAPARAM";

/// A texture slot the shader declares.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Texture {
    pub name: String,
    /// The texture used when the material binds none ("" for none).
    pub default_texture: String,
    /// The UV tiling parameter group the slot belongs to.
    pub uv_group: String,
    /// The slot's index (0-based).
    pub slot: u8,
    /// The slot's semantic category: 1 albedo, 3 reflectance, 4 normal, 6 emissive, 7 displacement, 10 flow, 14
    /// metallic, 15 one-channel mask, 16 three-channel mask, 17 vector map, 0 other.
    pub kind: u8,
    /// The byte at 8 of the record (the reader of the format skips it).
    pub unknown_08: u8,
    /// The i32 at 12 of the record (-1 on most).
    pub unknown_0c: i32,
}

/// A render pass's sampler bindings: bit n of a mask is sampler slot n.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RenderPass {
    pub pass_id: u32,
    /// Samplers the pixel stage binds.
    pub pixel_samplers: u32,
    /// Samplers the hull stage binds.
    pub hull_samplers: u32,
    /// Samplers the vertex stage binds.
    pub vertex_samplers: u32,
}

/// A parameter read through `SAT_CAParam`, one component per record.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CaParam {
    pub name: String,
    pub sequence_index: u32,
    pub param_id: u32,
    pub component: u32,
    pub default: f32,
    pub unknown_40: u32,
    /// A name hash.
    pub key: u32,
}

/// A parameter read through `SAT_MtdParam`, a whole value of up to five floats.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct MtdParam {
    pub name: String,
    /// Offset of its first component in the constant buffer, in 32-bit components.
    pub start: u32,
    pub sequence_index: i32,
    /// 0 bool, 1 float, 2 float2 (UV tiling), 3 float3, 4 float4, 12 colour in Sekiro's files (RGBA and an
    /// intensity; the third-party reader of the format calls it 13).
    pub value_type: u8,
    /// Set (stored 2) where the renderer reads the parameter and no shader does.
    pub not_shader_input: bool,
    pub srgb: bool,
    /// The byte at 0x2f of the record: 1 on the colours.
    pub colour: bool,
    /// CRC-32 of the UTF-16 bytes of the parameter's name in the material definition.
    pub key: u32,
    pub default: [f32; 5],
}

/// A shader's parameter definition.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Metaparam {
    /// Constant-buffer size in bytes.
    pub constant_buffer_size: u32,
    pub shader_id: u32,
    /// Feature flags: 0x10 declares CAParams, 0x100 the vertex stage reads the material's parameters, 0x10000 the
    /// shader is tessellated.
    pub flags: u32,
    /// The key the material's GX records are matched by.
    pub gxmd_key: u32,
    pub ca_param_block_size: u32,
    pub unknown_30: u32,
    pub unknown_40: u32,
    pub unknown_50: u32,
    pub textures: Vec<Texture>,
    pub passes: Vec<RenderPass>,
    pub ca_params: Vec<CaParam>,
    pub mtd_params: Vec<MtdParam>,
}

struct Reader<'a> {
    v: View<'a>,
}
impl Reader<'_> {
    fn zero(&self, at: usize, n: usize) -> Result<()> {
        if self.v.slice(at, n)?.iter().any(|&b| b != 0) {
            return Err(invalid(FORMAT, at, "nonzero padding"));
        }
        Ok(())
    }
    fn offset(&self, at: usize) -> Result<usize> {
        let o = self.v.size(at)?;
        if o > self.v.data.len() {
            return Err(invalid(FORMAT, at, "offset past the file"));
        }
        Ok(o)
    }
    /// The NUL-terminated UTF-16 string at the offset stored at `at`.
    fn string(&self, at: usize) -> Result<String> {
        let start = self.offset(at)?;
        let rest = self
            .v
            .data
            .get(start..)
            .ok_or_else(|| invalid(FORMAT, at, "string offset"))?;
        let mut units = Vec::new();
        for pair in rest.as_chunks::<2>().0 {
            let u = u16::from_le_bytes(*pair);
            if u == 0 {
                return String::from_utf16(&units).map_err(|_| invalid(FORMAT, start, "unpaired surrogate"));
            }
            units.push(u);
        }
        Err(invalid(FORMAT, start, "unterminated string"))
    }
    fn f32(&self, at: usize) -> Result<f32> {
        let x = f32::from_bits(self.v.u32(at)?);
        if !x.is_finite() {
            return Err(invalid(FORMAT, at, "non-finite value"));
        }
        Ok(x)
    }
    fn count(&self, at: usize, stride: usize) -> Result<usize> {
        let n = usize::try_from(self.v.u32(at)?).map_err(|_| invalid(FORMAT, at, "count"))?;
        if n > self.v.data.len() / stride {
            return Err(invalid(FORMAT, at, "count exceeds the file"));
        }
        Ok(n)
    }
    /// `n` records of `stride` bytes at `at`, inside the file.
    fn table(&self, at: usize, n: usize, stride: usize) -> Result<()> {
        let bytes = n
            .checked_mul(stride)
            .ok_or_else(|| invalid(FORMAT, at, "size overflow"))?;
        self.v.slice(at, bytes).map(|_| ())
    }
}

impl Metaparam {
    pub fn read(data: &[u8]) -> Result<Self> {
        let r = Reader {
            v: View {
                data,
                format: FORMAT,
                be: false,
            },
        };
        r.v.expect(0, b"SMD\0")?;
        r.v.expect(4, &[0; 4])?;
        if r.v.u32(8)? != 6 {
            return Err(invalid(FORMAT, 8, "version"));
        }
        let texture_count = r.count(0xc, 48)?;
        let (ca_at, mtd_at) = (r.offset(0x18)?, r.offset(0x20)?);
        let ca_count = r.count(0x28, 0x60)?;
        let mtd_count = r.count(0x2c, 0x50)?;
        r.zero(0x3c, 4)?;
        r.zero(0x44, 4)?;
        r.zero(0x5c, 0x3c)?;
        let mut textures = Vec::with_capacity(texture_count);
        r.table(0x98, texture_count, 48)?;
        for i in 0..texture_count {
            let at = 0x98 + i * 48;
            r.zero(at + 10, 1)?;
            r.zero(at + 0x20, 0x10)?;
            textures.push(Texture {
                name: r.string(at)?,
                default_texture: r.string(at + 0x10)?,
                uv_group: r.string(at + 0x18)?,
                slot: r.v.u8(at + 9)?,
                kind: r.v.u8(at + 11)?,
                unknown_08: r.v.u8(at + 8)?,
                unknown_0c: r.v.u32(at + 12)? as i32,
            });
        }
        let passes_at = 0x98 + texture_count * 48;
        let entries_at = r.offset(passes_at)?;
        let pass_count = r.count(passes_at + 8, 24)?;
        r.zero(passes_at + 12, 4)?;
        r.zero(passes_at + 16, 0x18)?;
        let mut passes = Vec::with_capacity(pass_count);
        if pass_count > 0 {
            r.table(entries_at, pass_count, 24)?;
        }
        for i in 0..pass_count {
            let at = entries_at + i * 24;
            r.zero(at + 4, 4)?;
            r.zero(at + 20, 4)?;
            passes.push(RenderPass {
                pass_id: r.v.u32(at)?,
                pixel_samplers: r.v.u32(at + 8)?,
                hull_samplers: r.v.u32(at + 12)?,
                vertex_samplers: r.v.u32(at + 16)?,
            });
        }
        let mut ca_params = Vec::with_capacity(ca_count);
        if ca_count > 0 {
            r.table(ca_at, ca_count, 0x60)?;
        }
        for i in 0..ca_count {
            let at = ca_at + i * 0x60;
            r.zero(at + 8, 0x1c + 4)?;
            r.zero(at + 0x40, 8)?;
            r.zero(at + 0x48, 0x18)?;
            ca_params.push(CaParam {
                name: r.string(at)?,
                sequence_index: r.v.u32(at + 0x28)?,
                param_id: r.v.u32(at + 0x2c)?,
                component: r.v.u32(at + 0x30)?,
                default: r.f32(at + 0x34)?,
                unknown_40: r.v.u32(at + 0x38)?,
                key: r.v.u32(at + 0x3c)?,
            });
        }
        let mut mtd_params = Vec::with_capacity(mtd_count);
        if mtd_count > 0 {
            r.table(mtd_at, mtd_count, 0x50)?;
        }
        for i in 0..mtd_count {
            let at = mtd_at + i * 0x50;
            r.zero(at + 8, 0x1c)?;
            r.zero(at + 0x48, 8)?;
            let value_type = r.v.u8(at + 0x2c)?;
            let not_shader_input = match r.v.u8(at + 0x2d)? {
                0 => false,
                2 => true,
                _ => return Err(invalid(FORMAT, at + 0x2d, "not-a-shader-input flag")),
            };
            let srgb = r.v.boolean(at + 0x2e)?;
            let colour = r.v.boolean(at + 0x2f)?;
            let mut default = [0.; 5];
            for (j, d) in default.iter_mut().enumerate() {
                *d = r.f32(at + 0x34 + j * 4)?;
            }
            mtd_params.push(MtdParam {
                name: r.string(at)?,
                start: r.v.u32(at + 0x24)?,
                sequence_index: r.v.u32(at + 0x28)? as i32,
                value_type,
                not_shader_input,
                srgb,
                colour,
                key: r.v.u32(at + 0x30)?,
                default,
            });
        }
        Ok(Metaparam {
            constant_buffer_size: r.v.u32(0x34)?,
            shader_id: r.v.u32(0x58)?,
            flags: r.v.u32(0x38)?,
            gxmd_key: r.v.u32(0x48)?,
            ca_param_block_size: r.v.u32(0x4c)?,
            unknown_30: r.v.u32(0x30)?,
            unknown_40: r.v.u32(0x40)?,
            unknown_50: r.v.u32(0x50)?,
            textures,
            passes,
            ca_params,
            mtd_params,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;

    fn utf16(b: &mut Vec<u8>, s: &str) -> u64 {
        let at = b.len() as u64;
        for u in s.encode_utf16().chain([0]) {
            b.extend(u.to_le_bytes());
        }
        at
    }

    /// A file worked by hand from the layout: one texture, two render passes, one CAParam, one MtdParam; strings
    /// after the tables.
    fn fixture() -> Vec<u8> {
        let (textures, passes_at) = (1usize, 0x98 + 48);
        let entries_at = passes_at + 0x28;
        let ca_at = entries_at + 2 * 24;
        let mtd_at = ca_at + 0x60;
        let strings_at = mtd_at + 0x50;
        let mut strings = Vec::new();
        let names: Vec<u64> = ["g_Diffuse", "", "group_0", "_color_0", "color_0"]
            .iter()
            .map(|s| strings_at as u64 + utf16(&mut strings, s))
            .collect();
        let mut b = Vec::new();
        b.extend(b"SMD\0");
        for v in [0u32, 6, textures as u32] {
            b.extend(v.to_le_bytes());
        }
        for v in [ca_at as u64, ca_at as u64, mtd_at as u64] {
            b.extend(v.to_le_bytes());
        }
        // CaParam count, MtdParam count, unk30, constant buffer size, flags, 0, unk40, 0, gxmd key, CaParam block
        // size, unk50, CaParam count, shader id
        for v in [1u32, 1, 0x401, 0x40, 0x10, 0, 2, 0, 0xabcd, 8, 7, 1, 0x1234] {
            b.extend(v.to_le_bytes());
        }
        b.resize(0x98, 0);
        // texture: name, a byte, slot 3, 0, type 4, -1, default texture, UV group, 16 zero bytes
        b.extend(names[0].to_le_bytes());
        b.extend([9, 3, 0, 4]);
        b.extend((-1i32).to_le_bytes());
        b.extend(names[1].to_le_bytes());
        b.extend(names[2].to_le_bytes());
        b.extend([0; 16]);
        // render passes: entries offset, count, zeros
        b.extend((entries_at as u64).to_le_bytes());
        b.extend(2u32.to_le_bytes());
        b.resize(passes_at + 0x28, 0);
        for (id, px, hull, vertex) in [(5u32, 0b101u32, 0u32, 0b1u32), (6, 0b10, 0b11, 0)] {
            for v in [id, 0, px, hull, vertex, 0] {
                b.extend(v.to_le_bytes());
            }
        }
        // CaParam: name, zeros to 0x28, sequence, id, component, default, unk40, key, zeros
        b.extend(names[3].to_le_bytes());
        b.resize(ca_at + 0x28, 0);
        for v in [2u32, 24, 1, 0.5f32.to_bits(), 9, 0xfeed] {
            b.extend(v.to_le_bytes());
        }
        b.resize(ca_at + 0x60, 0);
        // MtdParam: name, zeros to 0x24, start, sequence, type 12, flag 2, srgb 1, colour 1, key, 5 floats
        b.extend(names[4].to_le_bytes());
        b.resize(mtd_at + 0x24, 0);
        for v in [4u32, 3] {
            b.extend(v.to_le_bytes());
        }
        b.extend([12, 2, 1, 1]);
        b.extend(0xdead_beefu32.to_le_bytes());
        for f in [1.0f32, 0.5, 0.25, 1.0, 2.0] {
            b.extend(f.to_bits().to_le_bytes());
        }
        b.resize(mtd_at + 0x50, 0);
        b.extend(strings);
        b
    }

    #[test]
    fn a_file_worked_by_hand_reads_field_by_field() {
        let m = Metaparam::read(&fixture()).unwrap();
        assert_eq!(
            (
                m.constant_buffer_size,
                m.shader_id,
                m.flags,
                m.gxmd_key,
                m.ca_param_block_size
            ),
            (0x40, 0x1234, 0x10, 0xabcd, 8)
        );
        assert_eq!((m.unknown_30, m.unknown_40, m.unknown_50), (0x401, 2, 7));
        assert_eq!(
            m.textures,
            [Texture {
                name: "g_Diffuse".into(),
                default_texture: String::new(),
                uv_group: "group_0".into(),
                slot: 3,
                kind: 4,
                unknown_08: 9,
                unknown_0c: -1,
            }]
        );
        assert_eq!(
            m.passes,
            [
                RenderPass {
                    pass_id: 5,
                    pixel_samplers: 0b101,
                    hull_samplers: 0,
                    vertex_samplers: 1
                },
                RenderPass {
                    pass_id: 6,
                    pixel_samplers: 0b10,
                    hull_samplers: 0b11,
                    vertex_samplers: 0
                },
            ]
        );
        assert_eq!(
            m.ca_params,
            [CaParam {
                name: "_color_0".into(),
                sequence_index: 2,
                param_id: 24,
                component: 1,
                default: 0.5,
                unknown_40: 9,
                key: 0xfeed,
            }]
        );
        assert_eq!(
            m.mtd_params,
            [MtdParam {
                name: "color_0".into(),
                start: 4,
                sequence_index: 3,
                value_type: 12,
                not_shader_input: true,
                srgb: true,
                colour: true,
                key: 0xdead_beef,
                default: [1., 0.5, 0.25, 1., 2.],
            }]
        );
    }

    #[test]
    fn what_it_cannot_read_is_refused_with_its_offset() {
        let good = fixture();
        let mut bad = good.clone();
        bad[8] = 4; // another version
        assert!(Metaparam::read(&bad).unwrap_err().to_string().contains("version"));
        let mut bad = good.clone();
        bad[0x5c] = 1; // padding
        assert!(Metaparam::read(&bad).unwrap_err().to_string().contains("0x5c"));
        let mut bad = good.clone();
        bad[0xc] = 0xff; // a texture count the file cannot hold
        assert!(Metaparam::read(&bad).is_err());
        assert!(
            Metaparam::read(&good[..good.len() - 2]).is_err(),
            "unterminated string"
        );
        assert!(Metaparam::read(&good[..0x90]).is_err());
    }
}
