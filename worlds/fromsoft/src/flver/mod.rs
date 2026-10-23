//! Sekiro FLVER2 geometry, retaining file coordinates and material bindings.
mod geometry;
mod vertex;
use crate::{Result, bytes::View, invalid};
pub use geometry::{FaceSetKind, Part, tangent_to_scene};
pub use vertex::Vertex;

#[derive(Clone, Debug)]
pub struct Member {
    pub unknown: u32,
    pub offset: usize,
    pub kind: u32,
    pub semantic: u32,
    pub index: u32,
}
#[derive(Clone, Debug)]
pub struct Texture {
    pub path: String,
    pub kind: String,
    pub scale: [f32; 2],
    /// How the texture repeats along u and v (`TilingTypeU`, `TilingTypeV`).
    pub tiling: [Tiling; 2],
    /// The three floats after the tiling types, whose meaning is not known.
    pub unknown: [f32; 3],
}
impl Default for Texture {
    fn default() -> Self {
        Texture {
            path: String::new(),
            kind: String::new(),
            scale: [1.; 2],
            tiling: [Tiling::None; 2],
            unknown: [0.; 3],
        }
    }
}
/// A texture's tiling type along one axis, as the FLVER texture record stores it (SoulsFormatsNEXT's `TilingType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Tiling {
    /// None stated.
    #[default]
    None,
    Repeat,
    MirrorRepeat,
    Clamp,
    Border,
    MirrorOnce,
}
impl Tiling {
    fn read(byte: u8) -> Option<Tiling> {
        Some(match byte {
            0 => Tiling::None,
            1 => Tiling::Repeat,
            2 => Tiling::MirrorRepeat,
            3 => Tiling::Clamp,
            4 => Tiling::Border,
            5 => Tiling::MirrorOnce,
            _ => return None,
        })
    }
}
/// Raw, uninterpreted material extension record; payload bytes retain source order.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct GxItem {
    pub id: [u8; 4],
    pub version: u32,
    pub data: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub mtd: String,
    pub textures: Vec<Texture>,
    /// Raw, uninterpreted GX records in source order, excluding the terminator.
    pub gx: Vec<GxItem>,
}
#[derive(Clone, Debug)]
pub struct FaceSet {
    pub flags: u32,
    pub strip: bool,
    pub cull: bool,
    pub indices: Vec<u32>,
}
impl FaceSet {
    /// File-specific ordering: odd strip faces reverse all three indices. Restarts
    /// reset parity; degenerate joints advance parity without emitting a face.
    pub fn triangulate(&self) -> Vec<u32> {
        if !self.strip {
            return self.indices.clone();
        }
        let mut out = Vec::new();
        let mut flip = false;
        for t in self.indices.windows(3) {
            if t.contains(&0xffff) {
                flip = false;
                continue;
            }
            if t[0] != t[1] && t[0] != t[2] && t[1] != t[2] {
                if flip {
                    out.extend([t[2], t[1], t[0]]);
                } else {
                    out.extend_from_slice(t);
                }
            }
            flip = !flip;
        }
        out
    }
    /// Not a motion-blur face set (bit 31) and of level of detail 0: bits 24 to 26 hold the level (Sekiro's face sets
    /// use 1 and 2; Elden Ring's also 3 and 4, the coarse stand-ins its assets carry for far views).
    pub fn is_lod0(&self) -> bool {
        self.flags & 0x8700_0000 == 0
    }
}
#[derive(Clone, Debug)]
pub struct Mesh {
    pub dynamic: bool,
    pub material: usize,
    /// The mesh's bone palette as stored (bone indices): empty when its vertices name bones directly.
    pub palette: Vec<u32>,
    pub layouts: Vec<usize>,
    pub vertices: Vec<Vertex>,
    pub faces: Vec<FaceSet>,
}
#[derive(Clone, Debug)]
pub struct Bone {
    /// The bone's name as stored.
    pub name: String,
    pub translation: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub parent: i16,
}
#[derive(Clone, Debug)]
pub struct Model {
    pub bones: Vec<Bone>,
    pub version: u32,
    pub layouts: Vec<Vec<Member>>,
    pub materials: Vec<Material>,
    pub meshes: Vec<Mesh>,
}
fn count(v: &View<'_>, o: usize) -> Result<usize> {
    let n = v.u32(o)?;
    if n > i32::MAX as u32 {
        return Err(invalid(v.format, o, "negative count or offset"));
    }
    Ok(n as usize)
}
fn table(v: &View<'_>, pos: &mut usize, n: usize, stride: usize) -> Result<usize> {
    let len = n
        .checked_mul(stride)
        .ok_or_else(|| invalid(v.format, *pos, "table overflow"))?;
    v.slice(*pos, len)?;
    let start = *pos;
    *pos += len;
    Ok(start)
}
fn string(v: &View<'_>, o: usize) -> Result<String> {
    let mut units = Vec::new();
    let mut p = o;
    loop {
        let c = v.u16(p)?;
        p += 2;
        if c == 0 {
            break;
        }
        units.push(c);
    }
    String::from_utf16(&units).map_err(|e| invalid(v.format, o, e))
}
fn float(v: &View<'_>, o: usize) -> Result<f32> {
    let n = f32::from_bits(v.u32(o)?);
    if !n.is_finite() {
        return Err(invalid(v.format, o, "non-finite value"));
    }
    Ok(n)
}
fn refs(v: &View<'_>, o: usize, n: usize, limit: usize) -> Result<Vec<usize>> {
    v.slice(
        o,
        n.checked_mul(4)
            .ok_or_else(|| invalid(v.format, o, "array overflow"))?,
    )?;
    (0..n)
        .map(|i| {
            let r = count(v, o + i * 4)?;
            if r >= limit {
                Err(invalid(v.format, o + i * 4, "index out of range"))
            } else {
                Ok(r)
            }
        })
        .collect()
}
impl Model {
    pub fn read(data: &[u8]) -> Result<Self> {
        let v = View {
            data,
            format: "FLVER",
            be: false,
        };
        v.expect(0, b"FLVER\0L\0")?;
        let version = v.u32(8)?;
        if !matches!(version, 0x20013 | 0x20014 | 0x20016 | 0x2001a) {
            return Err(invalid(v.format, 8, format!("unsupported version {version:#x}")));
        }
        let data_start = count(&v, 12)?;
        let data_len = count(&v, 16)?;
        v.slice(data_start, data_len)?;
        // Elden Ring's files may be zero-padded after the data block to the next 2048-byte boundary.
        let end = data_start + data_len;
        if !(end == data.len()
            || (data.len() == end.next_multiple_of(0x800) && data[end..].iter().all(|&b| b == 0)))
        {
            return Err(invalid(v.format, 16, "data length mismatch"));
        }
        let meta = View {
            data: v.slice(0, data_start)?,
            format: v.format,
            be: false,
        };
        for i in 0..6 {
            float(&v, 40 + i * 4)?;
        }
        if !v.boolean(73)? {
            return Err(invalid(v.format, 73, "legacy text encoding unsupported"));
        }
        v.boolean(74)?;
        v.expect(75, &[0])?;
        v.expect(94, &[0; 10])?;
        v.expect(108, &[0; 20])?;
        let mut pos = 128;
        let dummy_count = count(&v, 20)?;
        let mat_count = count(&v, 24)?;
        let bone_count = count(&v, 28)?;
        let mesh_count = count(&v, 32)?;
        let vb_count = count(&v, 36)?;
        let face_count = count(&v, 80)?;
        let layout_count = count(&v, 84)?;
        let texture_count = count(&v, 88)?;
        let dummy = table(&v, &mut pos, dummy_count, 64)?;
        let mat = table(&v, &mut pos, mat_count, 32)?;
        let bone = table(&v, &mut pos, bone_count, 128)?;
        let mesh = table(&v, &mut pos, mesh_count, 48)?;
        let face = table(&v, &mut pos, face_count, 32)?;
        let vb = table(&v, &mut pos, vb_count, 32)?;
        let layout = table(&v, &mut pos, layout_count, 16)?;
        let texture = table(&v, &mut pos, texture_count, 32)?;
        if version == 0x2001a {
            let s = table(&v, &mut pos, 1, 32)?;
            v.expect(s + 12, &[0; 20])?;
            for i in 0..2 {
                let n = v.u16(s + i * 2)? as usize;
                let mut p = count(&v, s + 4 + i * 4)?;
                let a = table(&meta, &mut p, n, 16)?;
                for j in 0..n {
                    meta.expect(a + j * 16 + 12, &[0; 4])?;
                }
            }
        }
        if pos > data_start {
            return Err(invalid(v.format, pos, "tables overlap data"));
        }
        for i in 0..dummy_count {
            let o = dummy + i * 64;
            v.boolean(o + 46)?;
            v.boolean(o + 47)?;
            v.expect(o + 56, &[0; 8])?;
        }
        let mut bones = Vec::new();
        for i in 0..bone_count {
            let o = bone + i * 128;
            let name = string(&meta, count(&v, o + 12)?)?;
            v.expect(o + 76, &[0; 52])?;
            bones.push(Bone {
                name,
                translation: [float(&v, o)?, float(&v, o + 4)?, float(&v, o + 8)?],
                rotation: [float(&v, o + 16)?, float(&v, o + 20)?, float(&v, o + 24)?],
                scale: [float(&v, o + 32)?, float(&v, o + 36)?, float(&v, o + 40)?],
                parent: v.u16(o + 28)? as i16,
            });
        }
        let mut layouts = Vec::new();
        for i in 0..layout_count {
            let o = layout + i * 16;
            v.expect(o + 4, &[0; 8])?;
            let n = count(&v, o)?;
            let mut p = count(&v, o + 12)?;
            let a = table(&meta, &mut p, n, 20)?;
            let mut members = Vec::new();
            let mut offset = 0;
            for j in 0..n {
                let r = a + j * 20;
                if count(&v, r + 4)? != offset {
                    return Err(invalid(v.format, r + 4, "noncontiguous vertex layout"));
                }
                let kind = v.u32(r + 8)?;
                members.push(Member {
                    unknown: v.u32(r)?,
                    offset,
                    kind,
                    semantic: v.u32(r + 12)?,
                    index: v.u32(r + 16)?,
                });
                offset += vertex::size(kind)?;
            }
            layouts.push(members);
        }
        let mut textures = Vec::new();
        for i in 0..texture_count {
            let o = texture + i * 32;
            v.expect(o + 18, &[0; 2])?;
            let tiling = |at: usize| -> Result<Tiling> {
                Tiling::read(v.u8(at)?).ok_or_else(|| invalid(v.format, at, "texture tiling type"))
            };
            textures.push(Some(Texture {
                path: string(&meta, count(&v, o)?)?,
                kind: string(&meta, count(&v, o + 4)?)?,
                scale: [float(&v, o + 8)?, float(&v, o + 12)?],
                tiling: [tiling(o + 16)?, tiling(o + 17)?],
                unknown: [float(&v, o + 20)?, float(&v, o + 24)?, float(&v, o + 28)?],
            }));
        }
        let mut materials = Vec::new();
        for i in 0..mat_count {
            let o = mat + i * 32;
            v.expect(o + 28, &[0; 4])?;
            let n = count(&v, o + 8)?;
            let first = count(&v, o + 12)?;
            let mut bound = Vec::new();
            for j in first
                ..first
                    .checked_add(n)
                    .ok_or_else(|| invalid(v.format, o, "texture range overflow"))?
            {
                bound.push(
                    textures
                        .get_mut(j)
                        .and_then(Option::take)
                        .ok_or_else(|| invalid(v.format, o, "missing or shared texture"))?,
                );
            }
            let mut gx = count(&v, o + 20)?;
            let mut gx_items = Vec::new();
            if gx != 0 {
                loop {
                    let id = meta.u32(gx)?;
                    let len = count(&meta, gx + 8)?;
                    if len < 12 {
                        return Err(invalid(v.format, gx, "short GX record"));
                    }
                    meta.slice(gx, len)?;
                    if matches!(id, 0x7fff_ffff | 0xffff_ffff) {
                        if v.u32(gx + 4)? != 100 || v.slice(gx + 12, len - 12)?.iter().any(|&b| b != 0) {
                            return Err(invalid(v.format, gx, "GX terminator"));
                        }
                        break;
                    }
                    gx_items.push(GxItem {
                        id: meta.slice(gx, 4)?.try_into().expect("four-byte checked slice"),
                        version: meta.u32(gx + 4)?,
                        data: meta.slice(gx + 12, len - 12)?.to_vec(),
                    });
                    gx += len;
                }
            }
            materials.push(Material {
                name: string(&meta, count(&v, o)?)?,
                mtd: string(&meta, count(&v, o + 4)?)?,
                textures: bound,
                gx: gx_items,
            });
        }
        if textures.iter().any(Option::is_some) {
            return Err(invalid(v.format, texture, "unclaimed textures"));
        }
        let mut faces = Vec::new();
        for i in 0..face_count {
            let o = face + i * 32;
            v.expect(o + 20, &[0; 4])?;
            v.expect(o + 28, &[0; 4])?;
            let flags = v.u32(o)?;
            let n = count(&v, o + 8)?;
            let width = match v.u32(o + 24)? {
                0 => v.u8(72)? as u32,
                w => w,
            };
            if !matches!(width, 16 | 32) || flags & 0x4000_0000 != 0 {
                return Err(invalid(v.format, o, "unsupported compressed indices"));
            }
            let start = data_start
                .checked_add(count(&v, o + 12)?)
                .ok_or_else(|| invalid(v.format, o, "index offset overflow"))?;
            if count(&v, o + 16)?
                != n.checked_mul(width as usize / 8)
                    .ok_or_else(|| invalid(v.format, o, "index length overflow"))?
            {
                return Err(invalid(v.format, o + 16, "index length mismatch"));
            }
            v.slice(
                start,
                n.checked_mul(width as usize / 8)
                    .ok_or_else(|| invalid(v.format, o, "index size overflow"))?,
            )?;
            let indices = (0..n)
                .map(|j| {
                    if width == 16 {
                        v.u16(start + j * 2).map(u32::from)
                    } else {
                        v.u32(start + j * 4)
                    }
                })
                .collect::<Result<_>>()?;
            faces.push(Some(FaceSet {
                flags,
                strip: v.boolean(o + 4)?,
                cull: v.boolean(o + 5)?,
                indices,
            }));
        }
        let mut used_vb = vec![false; vb_count];
        let mut meshes = Vec::new();
        for i in 0..mesh_count {
            let o = mesh + i * 48;
            let dynamic = v.boolean(o)?;
            v.expect(o + 1, &[0; 3])?;
            v.expect(o + 8, &[0; 8])?;
            let material = count(&v, o + 4)?;
            if material >= materials.len() {
                return Err(invalid(v.format, o + 4, "material index"));
            }
            let palette_len = count(&v, o + 20)?;
            let mut at = count(&v, o + 28)?;
            let start = table(&meta, &mut at, palette_len, 4)?;
            let palette = (0..palette_len)
                .map(|i| meta.u32(start + i * 4))
                .collect::<Result<Vec<u32>>>()?;
            let face_refs = refs(&meta, count(&v, o + 36)?, count(&v, o + 32)?, face_count)?;
            let buffers = refs(&meta, count(&v, o + 44)?, count(&v, o + 40)?, vb_count)?;
            // A mesh with neither face sets nor vertex buffers is empty: its bounds are the exporter's sentinel
            // (minimum -inf, maximum NaN in Elden Ring's files) and are read but not required to be finite.
            let empty = face_refs.is_empty() && buffers.is_empty();
            let bounds = count(&v, o + 24)?;
            if bounds != 0 {
                for j in 0..if version == 0x2001a { 9 } else { 6 } {
                    if empty {
                        meta.u32(bounds + j * 4)?;
                    } else {
                        float(&meta, bounds + j * 4)?;
                    }
                }
            }
            if empty {
                meshes.push(Mesh {
                    dynamic,
                    material,
                    palette,
                    layouts: Vec::new(),
                    vertices: Vec::new(),
                    faces: Vec::new(),
                });
                continue;
            }
            if buffers.is_empty() || buffers.len() > 3 {
                return Err(invalid(v.format, o + 40, "vertex buffer count"));
            }
            let n = count(&v, vb + buffers[0] * 32 + 12)?;
            // Validate every buffer extent before allocating decoded vertices.
            let mut streams = Vec::new();
            for (ordinal, &b) in buffers.iter().enumerate() {
                if used_vb[b] {
                    return Err(invalid(v.format, o, "shared vertex buffer"));
                }
                used_vb[b] = true;
                let r = vb + b * 32;
                v.expect(r + 16, &[0; 8])?;
                if v.u32(r)? != ordinal as u32 || count(&v, r + 12)? != n {
                    return Err(invalid(v.format, r, "vertex buffer identity/count"));
                }
                let l = count(&v, r + 4)?;
                let members = layouts
                    .get(l)
                    .ok_or_else(|| invalid(v.format, r + 4, "layout index"))?;
                let stride = count(&v, r + 8)?;
                let expected = members
                    .iter()
                    .try_fold(0usize, |a, m| vertex::size(m.kind).map(|s| a + s))?;
                if stride != expected || stride == 0 {
                    return Err(invalid(v.format, r + 8, "vertex stride"));
                }
                if count(&v, r + 24)?
                    != n.checked_mul(stride)
                        .ok_or_else(|| invalid(v.format, r, "vertex length overflow"))?
                {
                    return Err(invalid(v.format, r + 24, "vertex length mismatch"));
                }
                let start = data_start
                    .checked_add(count(&v, r + 28)?)
                    .ok_or_else(|| invalid(v.format, r, "buffer offset overflow"))?;
                v.slice(
                    start,
                    n.checked_mul(stride)
                        .ok_or_else(|| invalid(v.format, r, "buffer size overflow"))?,
                )?;
                streams.push((l, stride, start));
            }
            let mut vertices = vec![Vertex::default(); n];
            for &(l, stride, start) in &streams {
                for (j, vert) in vertices.iter_mut().enumerate() {
                    vert.read(v.slice(start + j * stride, stride)?, &layouts[l], 2048.)?;
                }
            }
            let mut mesh_faces = Vec::new();
            for f in face_refs {
                let fs = faces[f]
                    .take()
                    .ok_or_else(|| invalid(v.format, o, "shared face set"))?;
                if fs
                    .indices
                    .iter()
                    .any(|&r| r as usize >= n && !(fs.strip && r == 0xffff))
                {
                    return Err(invalid(v.format, o, "vertex index out of range"));
                }
                mesh_faces.push(fs);
            }
            meshes.push(Mesh {
                dynamic,
                material,
                palette,
                layouts: streams.iter().map(|s| s.0).collect(),
                vertices,
                faces: mesh_faces,
            });
        }
        if faces.iter().any(Option::is_some) || used_vb.contains(&false) {
            return Err(invalid(v.format, mesh, "unclaimed face set or vertex buffer"));
        }
        Ok(Self {
            bones,
            version,
            layouts,
            materials,
            meshes,
        })
    }
}

/// Whether a FLVER stores SpeedTree vegetation: the high half of its header word at 0x68 is 0x8000. Its vertex layouts
/// address several streams with per-stream offsets, which [`Model::read`] does not decode (nor does the reference
/// reader, SoulsFormatsNEXT).
pub fn is_speedtree(data: &[u8]) -> bool {
    data.starts_with(b"FLVER\0L\0") && data.get(0x6a..0x6c) == Some(&[0x00, 0x80][..])
}
