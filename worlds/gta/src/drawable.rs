//! Drawables (ydr) in Enhanced's layout: bounds, the shader group (shaders with their parameters, an embedded texture
//! dictionary), and up to four LOD model lists whose geometries carry a vertex buffer and an index buffer.
//!
//! Layouts (offsets in bytes):
//! - drawable: resource file header (16), shader group pointer 0x10, skeleton 0x18, bounding centre and radius 0x20,
//!   box min 0x30, box max 0x40, model list pointers high/med/low/vlow 0x50..0x70, LOD distances 0x70, render masks
//!   0x80, joints 0x90, models block 0xA0;
//! - model list: pointer, count, capacity, then that many model pointers;
//! - model (48): geometry pointer array 0x08 with counts 0x10/0x12, bounds 0x18 (one box, or count + 1 boxes with the
//!   whole first), shader mapping 0x20 (one shader index per geometry), skeleton binding 0x28, render mask 0x2C;
//! - geometry (152): vertex buffer 0x18, index buffer 0x38, index and triangle counts 0x58, vertex count 0x60, vertex
//!   stride 0x70;
//! - vertex buffer (64): vertex count 0x08, stride 0x0C, data 0x18, declaration 0x38; the declaration (320) gives per
//!   semantic slot (52: POSITION0-3, NORMAL0-3, TANGENT0-3, BINORMAL0-3, BLENDWEIGHT0-3, BLENDINDICES0-3, COLOR0-3,
//!   TEXCOORD0-23) an offset, the stride, and a buffer format (0: absent); vertices interleave, in offset order;
//! - index buffer (64): index count 0x08, index size 0x0C (2), indices 0x18;
//! - shader (64): name 0x00, preset 0x04, parameter buffers 0x08, texture references 0x10, parameter infos 0x20, render
//!   bucket 0x39, bucket mask 0x3C; parameter infos: buffer, texture, other, sampler and parameter counts and a copy
//!   count (8 bytes), then per parameter a name and a packed word (kind 2 bits; texture index or cbuffer index; offset
//!   and length in the cbuffer, 12 bits each). The buffer pointers come `copy count` times over; the first copy is read.
use crate::le;
use crate::resource::Resource;
use crate::sga::BufferFormat;
use crate::texture::{Texture, TextureDictionary};
use crate::{Result, invalid};

fn err(at: u64, detail: impl ToString) -> crate::Error {
    invalid("drawable", at, detail)
}

/// Semantic slot names, by slot.
pub const SEMANTICS: [&str; 52] = [
    "POSITION0",
    "POSITION1",
    "POSITION2",
    "POSITION3",
    "NORMAL0",
    "NORMAL1",
    "NORMAL2",
    "NORMAL3",
    "TANGENT0",
    "TANGENT1",
    "TANGENT2",
    "TANGENT3",
    "BINORMAL0",
    "BINORMAL1",
    "BINORMAL2",
    "BINORMAL3",
    "BLENDWEIGHT0",
    "BLENDWEIGHT1",
    "BLENDWEIGHT2",
    "BLENDWEIGHT3",
    "BLENDINDICES0",
    "BLENDINDICES1",
    "BLENDINDICES2",
    "BLENDINDICES3",
    "COLOR0",
    "COLOR1",
    "COLOR2",
    "COLOR3",
    "TEXCOORD0",
    "TEXCOORD1",
    "TEXCOORD2",
    "TEXCOORD3",
    "TEXCOORD4",
    "TEXCOORD5",
    "TEXCOORD6",
    "TEXCOORD7",
    "TEXCOORD8",
    "TEXCOORD9",
    "TEXCOORD10",
    "TEXCOORD11",
    "TEXCOORD12",
    "TEXCOORD13",
    "TEXCOORD14",
    "TEXCOORD15",
    "TEXCOORD16",
    "TEXCOORD17",
    "TEXCOORD18",
    "TEXCOORD19",
    "TEXCOORD20",
    "TEXCOORD21",
    "TEXCOORD22",
    "TEXCOORD23",
];

/// One vertex element: semantic slot, buffer format, byte offset in the vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Element {
    pub slot: u8,
    pub format: BufferFormat,
    pub offset: u32,
}

impl Element {
    pub fn semantic(self) -> &'static str {
        SEMANTICS[usize::from(self.slot)]
    }
}

/// A geometry: interleaved vertices, 16-bit triangle-list indices, its shader.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// Index into the drawable's shaders.
    pub shader: u16,
    /// Bounds, as stored (`min`, `max`, each with a fourth component).
    pub bounds: [[f32; 4]; 2],
    pub vertex_count: u32,
    pub stride: u16,
    /// Elements in slot order.
    pub elements: Vec<Element>,
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
}

impl Geometry {
    /// The bytes of one element across all vertices, vertex after vertex.
    pub fn element_bytes(&self, e: Element) -> Vec<u8> {
        let size = e.format.element_size().expect("checked on reading");
        let (stride, off) = (usize::from(self.stride), e.offset as usize);
        (0..self.vertex_count as usize)
            .flat_map(|v| &self.vertices[v * stride + off..v * stride + off + size])
            .copied()
            .collect()
    }
}

/// A model: geometries drawn with one bone binding and render mask.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub skeleton_binding: u32,
    pub render_mask: u16,
    pub geometries: Vec<Geometry>,
}

/// A shader parameter's value.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// A texture by name (`None`: no texture bound).
    Texture(Option<String>),
    /// Floats from a constant buffer (the parameter's length / 4).
    Floats(Vec<f32>),
    /// A sampler index.
    Sampler(u8),
    /// Kind 1, index as stored.
    Other(u8),
}

/// A shader parameter: its stored name hash, its packed info word as stored (kind in bits 0-1; texture, sampler or
/// other index in bits 2-9, or constant buffer index in bits 2-7 with byte offset in bits 8-19 and length in bits
/// 20-31), and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: u32,
    pub info: u32,
    pub value: ParamValue,
}

/// A shader: effect name, preset, render bucket and parameters in stored order.
#[derive(Clone, Debug, PartialEq)]
pub struct Shader {
    pub name: u32,
    pub preset: u32,
    pub render_bucket: u8,
    pub bucket_mask: u32,
    pub params: Vec<Param>,
}

/// A drawable.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawable {
    pub bounding_centre: [f32; 3],
    pub bounding_radius: f32,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// High, medium, low and very low LOD distances.
    pub lod_dist: [f32; 4],
    pub render_masks: [u32; 4],
    /// Models per LOD: high, medium, low, very low.
    pub lods: [Vec<Model>; 4],
    pub shaders: Vec<Shader>,
    /// The shader group's embedded texture dictionary.
    pub textures: Option<TextureDictionary>,
    /// Whether it has a skeleton (skeletons are not read yet).
    pub skeleton: bool,
}

/// The resource version of Enhanced's drawables (ydr).
pub const VERSION: u32 = 159;

impl Drawable {
    /// A ydr's drawable: its root, after checking the resource is in Enhanced's layout.
    pub fn read_resource(r: &Resource) -> Result<Drawable> {
        if r.version != VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's ydr layout is {VERSION}",
                    r.version
                ),
            ));
        }
        Self::read(r, 0x5000_0000)
    }

    /// The drawable at `pointer` (`0x5000_0000` for a ydr's root).
    pub fn read(r: &Resource, pointer: u64) -> Result<Drawable> {
        let d = r.at(pointer, 0xA8)?;
        let mut lods: [Vec<Model>; 4] = Default::default();
        for (i, lod) in lods.iter_mut().enumerate() {
            let p = le::u64(d, 0x50 + 8 * i);
            if p != 0 {
                *lod = read_models(r, p)?;
            }
        }
        let group = le::u64(d, 0x10);
        let (shaders, textures) = read_shader_group(r, group)?;
        // a drawable without a shader group (a fragment's extra drawables) uses its owner's shaders
        if group != 0 {
            check_shaders(pointer, &lods, shaders.len())?;
        }
        Ok(Drawable {
            bounding_centre: le::vec3(d, 0x20),
            bounding_radius: le::f32(d, 0x2C),
            bbox_min: le::vec3(d, 0x30),
            bbox_max: le::vec3(d, 0x40),
            lod_dist: [
                le::f32(d, 0x70),
                le::f32(d, 0x74),
                le::f32(d, 0x78),
                le::f32(d, 0x7C),
            ],
            render_masks: [
                le::u32(d, 0x80),
                le::u32(d, 0x84),
                le::u32(d, 0x88),
                le::u32(d, 0x8C),
            ],
            lods,
            shaders,
            textures,
            skeleton: le::u64(d, 0x18) != 0,
        })
    }
}

/// Every geometry's shader index is below `shaders`.
fn check_shaders(at: u64, lods: &[Vec<Model>; 4], shaders: usize) -> Result<()> {
    match lods
        .iter()
        .flatten()
        .flat_map(|m| &m.geometries)
        .find(|g| usize::from(g.shader) >= shaders)
    {
        Some(g) => Err(err(at, format!("geometry shader {} of {shaders}", g.shader))),
        None => Ok(()),
    }
}

/// A pointer list: pointer, count, capacity; the `count` pointers it holds.
fn pointer_list(r: &Resource, at: u64) -> Result<Vec<u64>> {
    let h = r.at(at, 16)?;
    let (count, cap) = (le::u16(h, 8), le::u16(h, 10));
    if cap < count {
        return Err(err(at, format!("list count {count} over capacity {cap}")));
    }
    let n = usize::from(count);
    if n == 0 {
        return Ok(Vec::new());
    }
    let p = r.at(le::u64(h, 0), 8 * n)?;
    Ok((0..n).map(|i| le::u64(p, 8 * i)).collect())
}

fn read_models(r: &Resource, list: u64) -> Result<Vec<Model>> {
    pointer_list(r, list)?
        .into_iter()
        .map(|p| {
            let m = r.at(p, 0x30)?;
            let n = usize::from(le::u16(m, 0x10));
            if le::u16(m, 0x12) != le::u16(m, 0x10) || le::u16(m, 0x2E) != le::u16(m, 0x10) {
                return Err(err(p, "model geometry counts differ"));
            }
            let geoms = if n == 0 {
                &[][..]
            } else {
                r.at(le::u64(m, 0x08), 8 * n)?
            };
            let mapping = if n == 0 {
                &[][..]
            } else {
                r.at(le::u64(m, 0x20), 2 * n)?
            };
            let boxes = if n > 1 { n + 1 } else { n };
            let bounds = if n == 0 {
                &[][..]
            } else {
                r.at(le::u64(m, 0x18), 32 * boxes)?
            };
            let geometries = (0..n)
                .map(|i| {
                    let b = if n > 1 { 32 * (i + 1) } else { 0 };
                    read_geometry(
                        r,
                        le::u64(geoms, 8 * i),
                        le::u16(mapping, 2 * i),
                        [le::vec4(bounds, b), le::vec4(bounds, b + 16)],
                    )
                })
                .collect::<Result<_>>()?;
            Ok(Model {
                skeleton_binding: le::u32(m, 0x28),
                render_mask: le::u16(m, 0x2C),
                geometries,
            })
        })
        .collect()
}

fn read_geometry(r: &Resource, p: u64, shader: u16, bounds: [[f32; 4]; 2]) -> Result<Geometry> {
    let g = r.at(p, 0x98)?;
    let index_count = le::u32(g, 0x58);
    if le::u32(g, 0x5C) * 3 != index_count || le::u16(g, 0x62) != 3 {
        return Err(err(
            p,
            format!("{index_count} indices, {} triangles", le::u32(g, 0x5C)),
        ));
    }
    // vertex buffer
    let vb_ptr = le::u64(g, 0x18);
    let vb = r.at(vb_ptr, 0x40)?;
    let vertex_count = le::u32(vb, 0x08);
    let stride = le::u16(vb, 0x0C);
    if u32::from(le::u16(g, 0x60)) != vertex_count & 0xFFFF || le::u16(g, 0x70) != stride {
        return Err(err(
            p,
            "geometry and vertex buffer disagree on vertex count or stride",
        ));
    }
    let decl_ptr = le::u64(vb, 0x38);
    let decl = r.at(decl_ptr, 320)?;
    let mut elements = Vec::new();
    for slot in 0..52 {
        let format = BufferFormat(decl[208 + 52 + slot]);
        if format.0 == 0 {
            continue;
        }
        let offset = le::u32(decl, 4 * slot);
        let size = format.element_size().ok_or_else(|| {
            err(
                decl_ptr,
                format!("{}: vertex format {:#x}", SEMANTICS[slot], format.0),
            )
        })?;
        if u16::from(decl[208 + slot]) != stride || offset as usize + size > usize::from(stride) {
            return Err(err(
                decl_ptr,
                format!("{} at {offset} in a {stride}-byte vertex", SEMANTICS[slot]),
            ));
        }
        elements.push(Element {
            slot: slot as u8,
            format,
            offset,
        });
    }
    let packed = le::u64(decl, 312);
    if (packed >> 2) & 0xFF != u64::from(stride) || packed & 1 != 0 {
        return Err(err(
            decl_ptr,
            format!("declaration word {packed:#x} for a {stride}-byte vertex"),
        ));
    }
    let vertices = r
        .at(le::u64(vb, 0x18), vertex_count as usize * usize::from(stride))?
        .to_vec();
    // index buffer
    let ib_ptr = le::u64(g, 0x38);
    let ib = r.at(ib_ptr, 0x40)?;
    if le::u32(ib, 0x08) != index_count || le::u16(ib, 0x0C) != 2 {
        return Err(err(
            ib_ptr,
            format!(
                "index buffer of {} indices of {} bytes",
                le::u32(ib, 8),
                le::u16(ib, 0x0C)
            ),
        ));
    }
    let raw = r.at(le::u64(ib, 0x18), 2 * index_count as usize)?;
    let indices: Vec<u16> = (0..index_count as usize).map(|i| le::u16(raw, 2 * i)).collect();
    if indices.iter().any(|&i| u32::from(i) >= vertex_count) {
        return Err(err(ib_ptr, "an index past the vertices"));
    }
    Ok(Geometry {
        shader,
        bounds,
        vertex_count,
        stride,
        elements,
        vertices,
        indices,
    })
}

fn read_shader_group(r: &Resource, p: u64) -> Result<(Vec<Shader>, Option<TextureDictionary>)> {
    if p == 0 {
        return Ok((Vec::new(), None));
    }
    let g = r.at(p, 0x40)?;
    let textures = match le::u64(g, 0x08) {
        0 => None,
        t => Some(TextureDictionary::read(r, t)?),
    };
    let n = usize::from(le::u16(g, 0x18));
    if le::u16(g, 0x1A) < le::u16(g, 0x18) {
        return Err(err(p, "shader count over capacity"));
    }
    let ptrs = if n == 0 {
        &[][..]
    } else {
        r.at(le::u64(g, 0x10), 8 * n)?
    };
    let shaders = (0..n)
        .map(|i| read_shader(r, le::u64(ptrs, 8 * i)))
        .collect::<Result<_>>()?;
    Ok((shaders, textures))
}

fn read_shader(r: &Resource, p: u64) -> Result<Shader> {
    let s = r.at(p, 0x40)?;
    let infos_ptr = le::u64(s, 0x20);
    let h = r.at(infos_ptr, 8)?;
    let (n_buffers, n_textures, n_other, _n_samplers, n_params, copies) = (
        usize::from(h[0]),
        usize::from(h[1]),
        usize::from(h[2]),
        h[3],
        usize::from(h[4]),
        usize::from(h[7]),
    );
    if h[5] != 0 || h[6] != 0 || copies == 0 {
        return Err(err(infos_ptr, format!("parameter info header {:02x?}", h)));
    }
    let infos = r.at(infos_ptr + 8, 8 * n_params)?;
    // constant buffers: the first copy's pointers (the next copy's first pointer ends the last buffer)
    let buffers: Vec<u64> = if n_buffers == 0 {
        Vec::new()
    } else {
        let b = r.at(le::u64(s, 0x08), 8 * (n_buffers + 1).min(n_buffers * copies))?;
        (0..b.len() / 8).map(|i| le::u64(b, 8 * i)).collect()
    };
    let texture_refs: Vec<u64> = if n_textures == 0 {
        Vec::new()
    } else {
        let t = r.at(le::u64(s, 0x10), 8 * n_textures)?;
        (0..n_textures).map(|i| le::u64(t, 8 * i)).collect()
    };
    let mut params = Vec::with_capacity(n_params);
    for i in 0..n_params {
        let (name, word) = (le::u32(infos, 8 * i), le::u32(infos, 8 * i + 4));
        let value = match word & 3 {
            0 => {
                let index = ((word >> 2) & 0xFF) as usize;
                let ptr = *texture_refs
                    .get(index)
                    .ok_or_else(|| err(infos_ptr, format!("texture index {index} of {n_textures}")))?;
                ParamValue::Texture(match ptr {
                    0 => None,
                    t => Some(Texture::read_base(r, t)?.0.name),
                })
            }
            1 => {
                let index = ((word >> 2) & 0xFF) as u8;
                if usize::from(index) >= n_other {
                    return Err(err(infos_ptr, format!("other index {index} of {n_other}")));
                }
                ParamValue::Other(index)
            }
            2 => ParamValue::Sampler(((word >> 2) & 0xFF) as u8),
            _ => {
                let cb = ((word >> 2) & 0x3F) as usize;
                let (offset, length) = (u64::from((word >> 8) & 0xFFF), ((word >> 20) & 0xFFF) as usize);
                if cb >= n_buffers || length % 4 != 0 {
                    return Err(err(
                        infos_ptr,
                        format!("cbuffer {cb} of {n_buffers}, length {length}"),
                    ));
                }
                if buffers.len() > cb + 1 && offset + length as u64 > buffers[cb + 1] - buffers[cb] {
                    return Err(err(infos_ptr, "parameter runs past its constant buffer"));
                }
                let v = r.at(buffers[cb] + offset, length)?;
                ParamValue::Floats((0..length / 4).map(|j| le::f32(v, 4 * j)).collect())
            }
        };
        params.push(Param {
            name,
            info: word,
            value,
        });
    }
    Ok(Shader {
        name: le::u32(s, 0x00),
        preset: le::u32(s, 0x04),
        render_bucket: s[0x39],
        bucket_mask: le::u32(s, 0x3C),
        params,
    })
}

/// The resource version of Enhanced's fragments (yft).
pub const FRAGMENT_VERSION: u32 = 171;

/// A fragment (yft): its name, main drawable (the one the world draws), its extra drawables (by name; those without a
/// shader group use the main drawable's shaders) and its cloth drawable. Physics, glass and bones are not read yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    pub name: String,
    pub drawable: Option<Drawable>,
    pub extra: Vec<(String, Drawable)>,
    pub cloth: Option<Drawable>,
    /// The bones' pose, by bone index: three rows `[x, y, z, translation]` (`world = row . (p, 1)`).
    pub pose: Vec<[[f32; 4]; 3]>,
}

impl Fragment {
    /// A yft's fragment, after checking the resource is in Enhanced's layout. Layout: resource file header (16),
    /// bounding sphere 0x20, main drawable 0x30, extra drawables 0x38 with their names 0x40 and count 0x48, name
    /// 0x58, bone pose 0xA8 (32-byte header with the count at 0x10, then 48 bytes per bone), cloth drawable 0xF8.
    pub fn read_resource(r: &Resource) -> Result<Fragment> {
        if r.version != FRAGMENT_VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's yft layout is {FRAGMENT_VERSION}",
                    r.version
                ),
            ));
        }
        let h = r.at(0x5000_0000, 0x130)?;
        let opt = |p: u64| -> Result<Option<Drawable>> {
            match p {
                0 => Ok(None),
                p => Drawable::read(r, p).map(Some),
            }
        };
        let n = le::u32(h, 0x48) as usize;
        let mut extra = Vec::with_capacity(n);
        if n > 0 {
            let ptrs = r.at(le::u64(h, 0x38), 8 * n)?;
            let names = r.at(le::u64(h, 0x40), 8 * n)?;
            for i in 0..n {
                let name = match le::u64(names, 8 * i) {
                    0 => String::new(),
                    p => crate::texture::c_string(r, p)?,
                };
                if let Some(d) = opt(le::u64(ptrs, 8 * i))? {
                    extra.push((name, d));
                }
            }
        }
        // extra and cloth drawables without their own shaders use the main drawable's
        let owner = le::u64(h, 0x30);
        let main_shaders = opt(owner)?.map_or(0, |d| d.shaders.len());
        for (_, d) in &extra {
            if d.shaders.is_empty() {
                check_shaders(owner, &d.lods, main_shaders)?;
            }
        }
        let pose = match le::u64(h, 0xA8) {
            0 => Vec::new(),
            p => {
                let n = usize::from(r.at(p, 0x20)?[0x10]);
                let rows = r.at(p + 0x20, 48 * n)?;
                (0..n)
                    .map(|i| [0, 1, 2].map(|j| le::vec4(rows, 48 * i + 16 * j)))
                    .collect()
            }
        };
        Ok(Fragment {
            pose,
            name: match le::u64(h, 0x58) {
                0 => String::new(),
                p => crate::texture::c_string(r, p)?,
            },
            drawable: opt(le::u64(h, 0x30))?,
            extra,
            cloth: opt(le::u64(h, 0xF8))?,
        })
    }
}

/// A drawable dictionary (ydd): drawables sorted by name hash.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawableDictionary {
    /// Name hashes as stored (the joaat of the lowercase name), parallel to `drawables`.
    pub hashes: Vec<u32>,
    pub drawables: Vec<Drawable>,
}

impl DrawableDictionary {
    /// A ydd's dictionary, after checking the resource is in Enhanced's layout (159, as drawables). Layout: resource
    /// file header (16), hash list 0x20, drawable pointer list 0x30.
    pub fn read_resource(r: &Resource) -> Result<DrawableDictionary> {
        if r.version != VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's ydd layout is {VERSION}",
                    r.version
                ),
            ));
        }
        let d = r.at(0x5000_0000, 0x40)?;
        let hash_list = pointer_list_u32(r, &d[0x20..0x30])?;
        let ptrs = pointer_list(r, 0x5000_0030)?;
        if hash_list.len() != ptrs.len() {
            return Err(err(
                0,
                format!("{} name hashes for {} drawables", hash_list.len(), ptrs.len()),
            ));
        }
        if hash_list.windows(2).any(|w| w[0] >= w[1]) {
            return Err(err(0, "name hashes not strictly ascending"));
        }
        let drawables = ptrs
            .into_iter()
            .map(|p| Drawable::read(r, p))
            .collect::<Result<_>>()?;
        Ok(DrawableDictionary {
            hashes: hash_list,
            drawables,
        })
    }

    /// The drawable of this name hash.
    pub fn get(&self, hash: u32) -> Option<&Drawable> {
        self.hashes.binary_search(&hash).ok().map(|i| &self.drawables[i])
    }
}

/// A simple list of u32 (pointer, count, capacity) given its 16 header bytes.
fn pointer_list_u32(r: &Resource, h: &[u8]) -> Result<Vec<u32>> {
    let (count, cap) = (le::u16(h, 8), le::u16(h, 10));
    if cap < count {
        return Err(err(0, format!("list count {count} over capacity {cap}")));
    }
    let n = usize::from(count);
    if n == 0 {
        return Ok(Vec::new());
    }
    let d = r.at(le::u64(h, 0), 4 * n)?;
    Ok((0..n).map(|i| le::u32(d, 4 * i)).collect())
}
