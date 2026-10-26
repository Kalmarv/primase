//! MeshSet resources (type 0x49B156D4): the LODs, their sections and vertex declarations, and where each section's
//! vertices and indices lie in the LOD's geometry chunk. Read ([`read`], [`read_resource`]) and written back
//! ([`write`]): every MeshSet of the game reads and writes back byte-identical, its resource metadata too
//! (`tests/meshset.rs`).
//!
//! A MeshSet is one relocatable block: fixed-size records whose pointers (u64, stored relative to byte 16) name a pool
//! after them, then a relocation table listing every pointer field, then (one development mesh only) data the
//! metadata sizes. The bundle's resource metadata is `{table offset, table bytes, trailing bytes, 0}`.
//!
//! ```text
//! header (0xd0 bytes, 0xe0 for the meshes with bone bounds):
//!   0x00 u32 header bytes - 16   0x04 u32 LOD record bytes (0xb0; 0xb8 or 0xbc, below)   0x08 u32 section bytes (384)
//!   0x10 f32x4 bounds minimum   0x20 f32x4 maximum   0x30 u64[7] LOD records   0x68 u64 name   0x70 u64 its last part
//!   0xb4 u16 LOD count; header 0xe0: 0xc4 u16 bones, 0xc6 u16 bounded bones, 0xc8 u64 their ids (u16 each),
//!   0xd0 u64 bone bounds (f32x4 minimum, maximum; one a bounded bone, else one a bone)
//! LOD record (each 16-aligned): +0x08 u32 sections, +0x0c u64 the LOD's section records, five lists (+0x14 + 12k
//!   u32 count, u64 pointer: section indices, a byte each), +0x54 u32 index format (0x21: 16-bit, 0x2e: 32-bit
//!   indices), +0x58 u32 index-buffer bytes, +0x5c u32 vertex-buffer bytes (a multiple of 16), +0x74 geometry chunk
//!   GUID (the vertex buffer, then the index buffer), +0x8c u64 name, +0x94 u64 a part of it, +0x9c u64 its last part;
//!   record 0xb8: +0xb0 u64 each section's bone mask (24 bytes); record 0xbc: +0xb0 u32 count, +0xb4 u64 a bone map
//!   (u32 each)
//! section (384 bytes): +0x08 u64 material name, +0x10 u64 bone list (u16 each; when +0x18 u16 the bone count is not
//!   zero: the skeleton bone the section's vertices name by that index), +0x1b flags, +0x1e total vertex stride,
//!   +0x20 primitive count, +0x24 start index (again at +0x140), +0x28 vertex offset in bytes (again at +0x54), +0x2c
//!   vertex count, two vertex declarations (100 bytes each: 16 elements of usage, format, offset, stream; 16 stream
//!   strides of two bytes; element count; stream count): the stored one at +0x70 and the draw one at +0xd4 (usually
//!   equal; see `check_draw` for how they may differ: unbound elements, aliases into stored streams, streams filled
//!   at run time); +0x138 u64 material key
//! ```
//!
//! Order of the block: header, LOD records, every LOD's section records, then the pool: the bone lists (16-aligned
//! after), the strings (per LOD its section names then its name, then the MeshSet's name; each name's parts point into
//! it, or, in ReSkate Studio's meshes, at copies of their own right after it; 16-aligned after), the
//! five lists of each LOD (16-aligned after), each LOD's bone masks (16-aligned) or bone map (4-aligned), the bone ids
//! and the bone bounds (each 16-aligned), 16-aligned; the relocation table (one u32 a pointer, its field's offset - 16:
//! per LOD its section pointer, each section's name and bone list, the five lists, the three names, the masks or map;
//! then the header's LOD records, names, bone ids and bounds), 16-aligned; the trailing data.
//!
//! A section's stored streams lie one after another from its vertex offset, each `stride * vertex count` bytes.

use crate::{Rd, Result, bad, capacity};

const W: &str = "meshset";
pub const SECTION_SIZE: usize = 384;
pub const DECL_SIZE: usize = 100;
pub const DECL_AT: usize = 0x70;
pub const DECL2_AT: usize = 0xd4;
const KEY_AT: usize = 0x138;
/// The LOD record pointers (u64) lie from 0x30 up to the name at 0x68 (Studio refuses more: `mesh LOD count must be
/// 1..7`).
pub const MAX_LODS: usize = (0x68 - 0x30) / 8;

/// Element usages seen in skate.'s declarations (Frostbite's VertexElementUsage).
pub mod usage {
    pub const POSITION: u8 = 1;
    pub const BLEND_INDICES: u8 = 2;
    pub const BLEND_WEIGHTS: u8 = 4;
    pub const COLOR0: u8 = 30;
    pub const COLOR1: u8 = 31;
    pub const TEXCOORD0: u8 = 33;
    pub const TEXCOORD1: u8 = 34;
    pub const SUBMATERIAL: u8 = 51;
    pub const TANGENT_SPACE: u8 = 52;
}

/// Element formats (Frostbite's VertexElementFormat, numbered as FrostySdk names them) and their sizes in bytes. The
/// census gate checks every element of the base game against its stream stride with these sizes.
pub mod format {
    pub const FLOAT2: u8 = 0x02;
    pub const FLOAT3: u8 = 0x03;
    pub const FLOAT4: u8 = 0x04;
    pub const HALF2: u8 = 0x06;
    pub const HALF4: u8 = 0x08;
    pub const UBYTE4: u8 = 0x0c;
    pub const UBYTE4N: u8 = 0x0d;
    pub const USHORT4: u8 = 0x17;
    pub fn size(f: u8) -> Option<usize> {
        Some(match f {
            0x32 => 1,               // UByteN
            0x05 | 0x0e | 0x12 => 2, // Half, Short, ShortN
            0x07 | 0x10 | 0x14 => 6, // Half3, Short3, Short3N
            0x01
            | 0x06
            | 0x0a..=0x0d
            | 0x0f
            | 0x13
            | 0x16
            | 0x18
            | 0x1a
            | 0x1d
            | 0x20
            | 0x23
            | 0x26..=0x31 => 4,
            0x02 | 0x08 | 0x11 | 0x15 | 0x17 | 0x19 | 0x1b | 0x1e | 0x21 | 0x24 => 8,
            0x03 | 0x33 | 0x34 => 12,               // Float3, Int3, UInt3
            0x04 | 0x1c | 0x1f | 0x22 | 0x25 => 16, // Float4, Int4, Int4N, UInt4, UInt4N
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Element {
    pub usage: u8,
    pub format: u8,
    pub offset: u8,
    pub stream: u8,
}

/// A vertex declaration, decoded, and its stored bytes ([`write`] writes the bytes; the elements and strides are
/// their reading).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub elements: Vec<Element>,
    pub strides: Vec<u8>,
    pub bytes: [u8; DECL_SIZE],
}

impl Default for Decl {
    fn default() -> Self {
        Self {
            elements: Vec::new(),
            strides: Vec::new(),
            bytes: [0; DECL_SIZE],
        }
    }
}

impl Decl {
    pub fn read(b: &[u8; DECL_SIZE]) -> Result<Self> {
        let (ne, ns) = (b[96] as usize, b[97] as usize);
        if ne > 16 || ns > 16 {
            return Err(bad(W, 96, format!("declaration of {ne} elements, {ns} streams")));
        }
        let elements = (0..ne)
            .map(|e| Element {
                usage: b[4 * e],
                format: b[4 * e + 1],
                offset: b[4 * e + 2],
                stream: b[4 * e + 3],
            })
            .collect();
        let strides = (0..ns).map(|j| b[64 + 2 * j]).collect();
        Ok(Self {
            elements,
            strides,
            bytes: *b,
        })
    }
    pub fn stride(&self) -> usize {
        self.strides.iter().map(|&s| s as usize).sum()
    }
    pub fn element(&self, usage: u8) -> Option<Element> {
        self.elements.iter().copied().find(|e| e.usage == usage)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    /// Offset of the section record in the payload it was read from.
    pub at: usize,
    /// The record as stored ([`SECTION_SIZE`] bytes), its pointers zero: [`write`] writes its pointers and the fields
    /// below over it.
    pub record: Vec<u8>,
    /// The material name (+0x08).
    pub name: String,
    pub key: u64,
    /// The stored vertex layout (what the chunk holds).
    pub decl: Decl,
    /// The draw layout: `decl` plus any streams filled at run time.
    pub draw_decl: Decl,
    pub flags: u8,
    pub stride: u8,
    pub primitives: u32,
    pub start_index: u32,
    pub vertex_offset: u32,
    pub vertices: u32,
    /// The bone list (the module documentation's section +0x10): empty for a section with no bones.
    pub bones: Vec<u16>,
}

impl Section {
    /// Where stream `j` starts in the chunk.
    pub fn stream_offset(&self, j: usize) -> usize {
        self.vertex_offset as usize
            + self.decl.strides[..j]
                .iter()
                .map(|&s| s as usize * self.vertices as usize)
                .sum::<usize>()
    }
    /// One past the section's last vertex byte.
    fn vertex_end(&self) -> usize {
        self.vertex_offset as usize + self.decl.stride() * self.vertices as usize
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lod {
    /// Offset of the LOD record in the payload it was read from.
    pub at: usize,
    /// The record as stored (the header's LOD record size, 16-aligned), its pointers zero: [`write`] writes its
    /// pointers and the fields below over it.
    pub record: Vec<u8>,
    /// The LOD's name (+0x8c), and its parts +0x94 and +0x9c.
    pub name: String,
    pub name_parts: [NamePart; 2],
    /// The five lists of section indices (+0x14 + 12k).
    pub lists: [Vec<u8>; LISTS],
    pub chunk: [u8; 16],
    /// Bytes per index: 2 (format 0x21) or 4 (format 0x2e), from the LOD's index format at +0x54.
    pub index_size: u8,
    pub index_bytes: u32,
    pub vertex_bytes: u32,
    pub sections: Vec<Section>,
    /// LOD record 0xb8: each section's bone mask (+0xb0).
    pub bone_masks: Option<Vec<[u8; MASK]>>,
    /// LOD record 0xbc: the bone map (+0xb0 count, +0xb4).
    pub bone_map: Option<Vec<u32>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeshSet {
    /// The header as stored ([`HEADER`] or [`HEADER_BONES`] bytes), its pointers zero: [`write`] writes its counts
    /// and pointers over it.
    pub header: Vec<u8>,
    /// The MeshSet's name (0x68), and its last part (0x70).
    pub name: String,
    pub short_name: NamePart,
    pub lods: Vec<Lod>,
    /// Header 0xe0: the bounded bones (0xc8, as many as 0xc6 counts) and the bone bounds (0xd0, as stored: f32x4
    /// minimum, f32x4 maximum; [`bounds`]): one a bounded bone, else one a bone (0xc4).
    pub bone_ids: Option<Vec<u16>>,
    pub bone_bounds: Option<Vec<[u8; BOUNDS]>>,
    /// The data after the relocation table (the metadata's third word), which only [`read_resource`] reads.
    pub tail: Vec<u8>,
}

impl MeshSet {
    /// The header's LOD record size (0x04).
    pub fn lod_size(&self) -> usize {
        word(&self.header, 4) as usize
    }
}

/// Header sizes: without and with bone bounds.
pub const HEADER: usize = 0xd0;
pub const HEADER_BONES: usize = 0xe0;
/// LOD record sizes: plain, with each section's bone mask, with a bone map.
pub const LOD_PLAIN: usize = 0xb0;
pub const LOD_MASKS: usize = 0xb8;
pub const LOD_MAP: usize = 0xbc;
pub const LISTS: usize = 5;
pub const MASK: usize = 24;
pub const BOUNDS: usize = 32;

/// A stored bone bound as its minimum and maximum.
pub fn bounds(b: &[u8; BOUNDS]) -> [[f32; 4]; 2] {
    let f = |i: usize| f32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
    [[f(0), f(1), f(2), f(3)], [f(4), f(5), f(6), f(7)]]
}

fn word(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .and_then(|w| w.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

/// The draw declaration against the stored one: the same streams first (equal strides), every stored element
/// present, and every other element either unbound (stream 255: the shader reads a default), reading inside a stored
/// stream (an alias, e.g. a second texture coordinate on the first one's stream) or in a stream after the stored ones
/// (filled at run time). Element order may differ.
fn check_draw(stored: &Decl, draw: &Decl) -> std::result::Result<(), String> {
    if !draw.strides.starts_with(&stored.strides) {
        return Err("the draw declaration's streams do not start with the stored ones".into());
    }
    if let Some(e) = stored.elements.iter().find(|e| !draw.elements.contains(e)) {
        return Err(format!("stored element {e:?} missing from the draw declaration"));
    }
    for e in draw.elements.iter().filter(|e| !stored.elements.contains(e)) {
        if e.stream == 255 {
            continue;
        }
        let size = format::size(e.format).ok_or_else(|| format!("unknown element format {}", e.format))?;
        let stride = *draw
            .strides
            .get(e.stream as usize)
            .ok_or("draw element stream out of range")?;
        if e.offset as usize + size > stride as usize {
            return Err(format!("draw element {e:?} runs past its stream"));
        }
    }
    Ok(())
}

fn ptr(r: &Rd, at: usize) -> Result<usize> {
    let p = r.u64le(at)?;
    usize::try_from(p)
        .ok()
        .and_then(|p| p.checked_add(16))
        .ok_or_else(|| bad(W, at, "pointer out of range"))
}

/// A record's bytes with its pointer fields (u64 at `fields`) zero: where they point is the layout's, which
/// [`write`] lays out anew.
fn without_pointers(b: &[u8], fields: &[usize]) -> Vec<u8> {
    let mut v = b.to_vec();
    for &f in fields {
        v[f..f + 8].fill(0);
    }
    v
}

/// A pointer field that may be null.
fn opt_ptr(r: &Rd, at: usize) -> Result<Option<usize>> {
    Ok(if r.u64le(at)? == 0 {
        None
    } else {
        Some(ptr(r, at)?)
    })
}

/// What a name's part pointer names: a place in the name (the game's own meshes: the part's first byte) or a string
/// of its own (ReSkate Studio's meshes, the part's text again right after the name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamePart {
    At(usize),
    Own(String),
}

impl Default for NamePart {
    fn default() -> Self {
        Self::At(0)
    }
}

/// The name the pointer at `at` names, and what the pointers at `parts` name.
fn name<const N: usize>(r: &Rd, at: usize, parts: [usize; N]) -> Result<(String, [NamePart; N])> {
    let a = ptr(r, at)?;
    let s = r.cstr(a)?;
    let mut out: [NamePart; N] = std::array::from_fn(|_| NamePart::default());
    for (o, &p) in out.iter_mut().zip(&parts) {
        let b = ptr(r, p)?;
        *o = match b.checked_sub(a).filter(|&o| o <= s.len()) {
            Some(o) => NamePart::At(o),
            None => NamePart::Own(r.cstr(b)?),
        };
    }
    Ok((s, out))
}

/// `n` items of `size` bytes from `at`, read by `item`.
fn items<T>(r: &Rd, at: usize, n: usize, size: usize, item: impl Fn(usize) -> Result<T>) -> Result<Vec<T>> {
    (0..capacity(W, n, size, r.data.len())?)
        .map(|i| item(at + size * i))
        .collect()
}

/// The MeshSet's records, names, lists and bone tables (everything but the data after the relocation table, which
/// [`read_resource`] adds). Checks each layout relation the module doc names and fails on the first broken one
/// (strict: a MeshSet this reader accepts can be rewritten section by section).
pub fn read(d: &[u8]) -> Result<MeshSet> {
    let r = Rd::new(d, W);
    let header_size = r.u32le(0)? as usize + 16;
    if header_size != HEADER && header_size != HEADER_BONES {
        return Err(bad(W, 0, format!("header of {header_size:#x} bytes")));
    }
    let lod_size = r.u32le(4)? as usize;
    if ![LOD_PLAIN, LOD_MASKS, LOD_MAP].contains(&lod_size) {
        return Err(bad(W, 4, format!("LOD records of {lod_size:#x} bytes")));
    }
    if r.u32le(8)? as usize != SECTION_SIZE {
        return Err(bad(W, 8, format!("section records of {:#x} bytes", r.u32le(8)?)));
    }
    let n = r.u16le(0xb4)? as usize;
    if n > MAX_LODS {
        return Err(bad(
            W,
            0xb4,
            format!("{n} LODs, but only {MAX_LODS} record pointers fit between 0x30 and the name"),
        ));
    }
    let (name_, [short_name]) = name(&r, 0x68, [0x70])?;
    let mut lods = Vec::with_capacity(n);
    for l in 0..n {
        let at = ptr(&r, 0x30 + 8 * l)?;
        let count = r.u32le(at + 8)? as usize;
        let first = ptr(&r, at + 0x0c)?;
        let (lod_name, name_parts) = name(&r, at + 0x8c, [at + 0x94, at + 0x9c])?;
        let mut lists: [Vec<u8>; LISTS] = Default::default();
        for (k, list) in lists.iter_mut().enumerate() {
            let c = r.u32le(at + 0x14 + 12 * k)? as usize;
            *list = r.bytes(ptr(&r, at + 0x18 + 12 * k)?, c)?.to_vec();
        }
        let bone_masks = match lod_size {
            LOD_MASKS => opt_ptr(&r, at + 0xb0)?
                .map(|p| items(&r, p, count, MASK, |a| r.arr(a)))
                .transpose()?,
            _ => None,
        };
        let bone_map = match lod_size {
            LOD_MAP => opt_ptr(&r, at + 0xb4)?
                .map(|p| items(&r, p, r.u32le(at + 0xb0)? as usize, 4, |a| r.u32le(a)))
                .transpose()?,
            _ => None,
        };
        let mut lod = Lod {
            at,
            record: without_pointers(
                r.bytes(at, crate::align(lod_size, 16))?,
                &[0x0c, 0x18, 0x24, 0x30, 0x3c, 0x48, 0x8c, 0x94, 0x9c]
                    .into_iter()
                    .chain(match lod_size {
                        LOD_MASKS => Some(0xb0),
                        LOD_MAP => Some(0xb4),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            name: lod_name,
            name_parts,
            lists,
            chunk: r.arr(at + 0x74)?,
            index_size: match r.u32le(at + 0x54)? {
                0x21 => 2,
                0x2e => 4,
                f => return Err(bad(W, at + 0x54, format!("unknown index format {f:#x}"))),
            },
            index_bytes: r.u32le(at + 0x58)?,
            vertex_bytes: r.u32le(at + 0x5c)?,
            sections: Vec::with_capacity(capacity(W, count, SECTION_SIZE, d.len())?),
            bone_masks,
            bone_map,
        };
        for s in 0..count {
            let o = first + SECTION_SIZE * s;
            let decl = Decl::read(&r.arr(o + DECL_AT)?)?;
            let draw_decl = Decl::read(&r.arr(o + DECL2_AT)?)?;
            check_draw(&decl, &draw_decl).map_err(|d| bad(W, o, d))?;
            let sec = Section {
                at: o,
                record: without_pointers(r.bytes(o, SECTION_SIZE)?, &[0x08, 0x10]),
                name: r.cstr(ptr(&r, o + 8)?)?,
                key: r.u64le(o + KEY_AT)?,
                decl,
                draw_decl,
                flags: r.u8(o + 0x1b)?,
                stride: r.u8(o + 0x1e)?,
                primitives: r.u32le(o + 0x20)?,
                start_index: r.u32le(o + 0x24)?,
                vertex_offset: r.u32le(o + 0x28)?,
                vertices: r.u32le(o + 0x2c)?,
                bones: match r.u16le(o + 0x18)? as usize {
                    0 => Vec::new(),
                    c => items(&r, ptr(&r, o + 0x10)?, c, 2, |a| r.u16le(a))?,
                },
            };
            if sec.stride as usize != sec.decl.stride() {
                return Err(bad(
                    W,
                    o,
                    format!(
                        "stride byte {} != declared streams {}",
                        sec.stride,
                        sec.decl.stride()
                    ),
                ));
            }
            if r.u32le(o + 0x54)? != sec.vertex_offset || r.u32le(o + 0x140)? != sec.start_index {
                return Err(bad(W, o, "repeated vertex offset / start index differ"));
            }
            for e in &sec.decl.elements {
                let size = format::size(e.format)
                    .ok_or_else(|| bad(W, o, format!("unknown element format {}", e.format)))?;
                let stride = *sec
                    .decl
                    .strides
                    .get(e.stream as usize)
                    .ok_or_else(|| bad(W, o, "element stream out of range"))?;
                if e.offset as usize + size > stride as usize {
                    return Err(bad(W, o, "element runs past its stream"));
                }
            }
            lod.sections.push(sec);
        }
        let vend = lod.sections.iter().map(Section::vertex_end).max().unwrap_or(0);
        if !lod.vertex_bytes.is_multiple_of(16)
            || (lod.vertex_bytes as usize) < vend
            || lod.vertex_bytes as usize - vend >= 16
        {
            return Err(bad(
                W,
                at,
                format!(
                    "vertex buffer {} for vertex data ending at {vend}",
                    lod.vertex_bytes
                ),
            ));
        }
        let iend = lod
            .sections
            .iter()
            .map(|s| usize::from(lod.index_size) * (s.start_index as usize + 3 * s.primitives as usize))
            .max()
            .unwrap_or(0);
        if (lod.index_bytes as usize) < iend || lod.index_bytes as usize - iend >= 4 {
            return Err(bad(
                W,
                at,
                format!(
                    "index buffer {} for {}-byte indices ending at {iend}",
                    lod.index_bytes, lod.index_size
                ),
            ));
        }
        lods.push(lod);
    }
    let (mut bone_ids, mut bone_bounds) = (None, None);
    if header_size == HEADER_BONES {
        let (bones, bounded) = (r.u16le(0xc4)? as usize, r.u16le(0xc6)? as usize);
        if let Some(p) = opt_ptr(&r, 0xc8)? {
            bone_ids = Some(items(&r, p, bounded, 2, |a| r.u16le(a))?);
        }
        if let Some(p) = opt_ptr(&r, 0xd0)? {
            let c = if bone_ids.is_some() { bounded } else { bones };
            bone_bounds = Some(items(&r, p, c, BOUNDS, |a| r.arr(a))?);
        }
    }
    Ok(MeshSet {
        header: without_pointers(
            r.bytes(0, header_size)?,
            &(0..MAX_LODS)
                .map(|l| 0x30 + 8 * l)
                .chain([0x68, 0x70])
                .chain(if header_size == HEADER_BONES {
                    vec![0xc8, 0xd0]
                } else {
                    vec![]
                })
                .collect::<Vec<_>>(),
        ),
        name: name_,
        short_name,
        lods,
        bone_ids,
        bone_bounds,
        tail: Vec::new(),
    })
}

/// Every pointer field of the records placed at `lod_at` and `sec_at`, in the relocation table's order (the module
/// documentation's).
fn pointer_fields(m: &MeshSet, lod_at: &[usize], sec_at: &[Vec<usize>]) -> Vec<usize> {
    let mut f = Vec::new();
    for (l, lod) in m.lods.iter().enumerate() {
        let at = lod_at[l];
        f.push(at + 0x0c);
        for (s, sec) in lod.sections.iter().enumerate() {
            f.push(sec_at[l][s] + 8);
            if !sec.bones.is_empty() {
                f.push(sec_at[l][s] + 0x10);
            }
        }
        f.extend((0..LISTS).map(|k| at + 0x18 + 12 * k));
        f.extend([at + 0x8c, at + 0x94, at + 0x9c]);
        if lod.bone_masks.is_some() {
            f.push(at + 0xb0);
        }
        if lod.bone_map.is_some() {
            f.push(at + 0xb4);
        }
    }
    f.extend((0..m.lods.len()).map(|l| 0x30 + 8 * l));
    f.extend([0x68, 0x70]);
    if m.bone_ids.is_some() {
        f.push(0xc8);
    }
    if m.bone_bounds.is_some() {
        f.push(0xd0);
    }
    f
}

/// [`read`], and the data after the relocation table that the resource metadata `meta` sizes. Refused when the
/// metadata does not place the relocation table where it lies, or the table does not list exactly the records'
/// pointers (in any order: the game's own files list them in the module documentation's order, which [`write`]
/// keeps; ReSkate Studio's in its template's).
pub fn read_resource(d: &[u8], meta: &[u8; 16]) -> Result<MeshSet> {
    let mut m = read(d)?;
    let w = |i: usize| word(meta, 4 * i) as usize;
    let (table, size, trailing) = (w(0), w(1), w(2));
    let end = crate::align(table + size, 16);
    if w(3) != 0 || !size.is_multiple_of(4) || end.checked_add(trailing) != Some(d.len()) {
        return Err(bad(
            W,
            table,
            format!("metadata {} for {} bytes", crate::hex(meta), d.len()),
        ));
    }
    let r = Rd::new(d, W);
    let lod_at: Vec<usize> = m.lods.iter().map(|l| l.at).collect();
    let sec_at: Vec<Vec<usize>> = m
        .lods
        .iter()
        .map(|l| l.sections.iter().map(|s| s.at).collect())
        .collect();
    let fields = pointer_fields(&m, &lod_at, &sec_at);
    let mut stored = items(&r, table, size / 4, 4, |a| r.u32le(a).map(|v| v as usize + 16))?;
    let mut fields = fields;
    stored.sort_unstable();
    fields.sort_unstable();
    if stored != fields {
        return Err(bad(
            W,
            table,
            format!(
                "relocation table of {} entries, not the records' {} pointers",
                stored.len(),
                fields.len()
            ),
        ));
    }
    m.tail = d[end..].to_vec();
    Ok(m)
}

fn put(out: &mut [u8], at: usize, v: &[u8]) {
    out[at..at + v.len()].copy_from_slice(v);
}

fn pad(out: &mut Vec<u8>, a: usize) {
    out.resize(crate::align(out.len(), a), 0);
}

/// A name and its parts, appended (each a zero-terminated string): where the name and each part start. Refused: a
/// part [`NamePart::At`] past the name's end.
fn push_named<const N: usize>(
    out: &mut Vec<u8>,
    s: &str,
    parts: &[NamePart; N],
) -> Result<(usize, [usize; N])> {
    let at = push_name(out, s)?;
    let mut places = [0; N];
    for (place, part) in places.iter_mut().zip(parts) {
        *place = match part {
            NamePart::At(o) if *o <= s.len() => at + o,
            NamePart::At(o) => return Err(bad(W, at, format!("part at {o} of {s:?}, past its end"))),
            NamePart::Own(p) => push_name(out, p)?,
        };
    }
    Ok((at, places))
}

/// A name and its terminating zero, appended; where it starts.
fn push_name(out: &mut Vec<u8>, s: &str) -> Result<usize> {
    let at = out.len();
    let b = crate::latin1(s, W)?;
    if b.contains(&0) {
        return Err(bad(W, 0, format!("name {s:?} holds a zero")));
    }
    out.extend(b);
    out.push(0);
    Ok(at)
}

fn u32_of(n: usize, what: &str) -> Result<[u8; 4]> {
    u32::try_from(n)
        .map(u32::to_le_bytes)
        .map_err(|_| bad(W, 0, format!("{n} {what}")))
}

fn u16_of(n: usize, at: usize, what: &str) -> Result<[u8; 2]> {
    u16::try_from(n)
        .map(u16::to_le_bytes)
        .map_err(|_| bad(W, at, format!("{n} {what}")))
}

/// Write the pointer field `at` naming `target`.
fn pointer(out: &mut [u8], at: usize, target: usize) {
    put(out, at, &(target as u64 - 16).to_le_bytes());
}

/// The MeshSet written in the layout of the module documentation (the game's own order), and its resource
/// metadata: the records as [`MeshSet`] holds them, with their counts, pointers and typed fields written over them.
/// Refused: records of the wrong size, bone tables the header or LOD record has no room for or that do not match
/// their counts, counts past their fields, name parts outside their names.
pub fn write(m: &MeshSet) -> Result<(Vec<u8>, [u8; 16])> {
    let hs = m.header.len();
    if (hs != HEADER && hs != HEADER_BONES) || word(&m.header, 0) as usize + 16 != hs {
        return Err(bad(W, 0, format!("a header of {hs:#x} bytes")));
    }
    let lod_size = m.lod_size();
    if ![LOD_PLAIN, LOD_MASKS, LOD_MAP].contains(&lod_size) || word(&m.header, 8) as usize != SECTION_SIZE {
        return Err(bad(W, 4, "LOD or section records of an unknown size"));
    }
    if m.lods.len() > MAX_LODS {
        return Err(bad(W, 0xb4, format!("{} LODs", m.lods.len())));
    }
    if hs == HEADER && (m.bone_ids.is_some() || m.bone_bounds.is_some()) {
        return Err(bad(W, 0xc8, "bone bounds need the 0xe0 header"));
    }
    let lod_stride = crate::align(lod_size, 16);
    let mut out = m.header.clone();
    let mut lod_at = Vec::with_capacity(m.lods.len());
    for (l, lod) in m.lods.iter().enumerate() {
        let refuse = |what: &str| Err(bad(W, l, format!("LOD {l}: {what}")));
        if lod.record.len() != lod_stride {
            return refuse("a record of another size than the header's");
        }
        if (lod.bone_masks.is_some() && lod_size != LOD_MASKS)
            || (lod.bone_map.is_some() && lod_size != LOD_MAP)
        {
            return refuse("a bone table its record has no room for");
        }
        if lod
            .bone_masks
            .as_ref()
            .is_some_and(|b| b.len() != lod.sections.len())
        {
            return refuse("bone masks for other than its sections");
        }
        lod_at.push(out.len());
        out.extend(&lod.record);
    }
    let mut sec_at = Vec::with_capacity(m.lods.len());
    for (l, lod) in m.lods.iter().enumerate() {
        let mut row = Vec::with_capacity(lod.sections.len());
        for (s, sec) in lod.sections.iter().enumerate() {
            if sec.record.len() != SECTION_SIZE {
                return Err(bad(
                    W,
                    l,
                    format!("LOD {l} section {s}: a record of {} bytes", sec.record.len()),
                ));
            }
            row.push(out.len());
            out.extend(&sec.record);
        }
        sec_at.push(row);
    }
    // the pool
    let mut bones_at = Vec::new();
    for sec in m.lods.iter().flat_map(|l| &l.sections) {
        bones_at.push(out.len());
        sec.bones.iter().for_each(|b| out.extend(b.to_le_bytes()));
    }
    pad(&mut out, 16);
    let (mut names_at, mut lod_name_at) = (Vec::new(), Vec::new());
    for lod in &m.lods {
        for sec in &lod.sections {
            names_at.push(push_name(&mut out, &sec.name)?);
        }
        lod_name_at.push(push_named(&mut out, &lod.name, &lod.name_parts)?);
    }
    let (name_at, [short_at]) = push_named(&mut out, &m.name, std::array::from_ref(&m.short_name))?;
    pad(&mut out, 16);
    let mut lists_at = Vec::new();
    for list in m.lods.iter().flat_map(|l| &l.lists) {
        lists_at.push(out.len());
        out.extend(list);
    }
    pad(&mut out, 16);
    let mut tables_at = Vec::new();
    for lod in &m.lods {
        if let Some(masks) = &lod.bone_masks {
            pad(&mut out, 16);
            tables_at.push(out.len());
            masks.iter().for_each(|b| out.extend(b));
        } else if let Some(map) = &lod.bone_map {
            pad(&mut out, 4);
            tables_at.push(out.len());
            map.iter().for_each(|b| out.extend(b.to_le_bytes()));
        } else {
            tables_at.push(0);
        }
    }
    let ids_at = m.bone_ids.as_ref().map(|ids| {
        pad(&mut out, 16);
        let at = out.len();
        ids.iter().for_each(|b| out.extend(b.to_le_bytes()));
        at
    });
    let bounds_at = m.bone_bounds.as_ref().map(|bounds| {
        pad(&mut out, 16);
        let at = out.len();
        bounds.iter().for_each(|b| out.extend(b));
        at
    });
    pad(&mut out, 16);
    // the records' fields
    let mut s_i = 0;
    for (l, lod) in m.lods.iter().enumerate() {
        let at = lod_at[l];
        put(&mut out, at + 8, &u32_of(lod.sections.len(), "sections")?);
        let first = sec_at[l].first().copied().unwrap_or(16);
        pointer(&mut out, at + 0x0c, first);
        for (k, list) in lod.lists.iter().enumerate() {
            put(&mut out, at + 0x14 + 12 * k, &u32_of(list.len(), "list entries")?);
            pointer(&mut out, at + 0x18 + 12 * k, lists_at[LISTS * l + k]);
        }
        let format: u32 = match lod.index_size {
            2 => 0x21,
            4 => 0x2e,
            n => return Err(bad(W, at + 0x54, format!("LOD {l}: {n}-byte indices"))),
        };
        put(&mut out, at + 0x54, &format.to_le_bytes());
        put(&mut out, at + 0x58, &lod.index_bytes.to_le_bytes());
        put(&mut out, at + 0x5c, &lod.vertex_bytes.to_le_bytes());
        put(&mut out, at + 0x74, &lod.chunk);
        let (name_at, [path_at, last_at]) = lod_name_at[l];
        pointer(&mut out, at + 0x8c, name_at);
        pointer(&mut out, at + 0x94, path_at);
        pointer(&mut out, at + 0x9c, last_at);
        match (lod_size, &lod.bone_masks, &lod.bone_map) {
            (LOD_MASKS, Some(_), _) => pointer(&mut out, at + 0xb0, tables_at[l]),
            (LOD_MASKS, None, _) => put(&mut out, at + 0xb0, &[0; 8]),
            (LOD_MAP, _, Some(map)) => {
                put(&mut out, at + 0xb0, &u32_of(map.len(), "mapped bones")?);
                pointer(&mut out, at + 0xb4, tables_at[l]);
            }
            (LOD_MAP, _, None) => put(&mut out, at + 0xb0, &[0; 12]),
            _ => {}
        }
        for (s, sec) in lod.sections.iter().enumerate() {
            let o = sec_at[l][s];
            pointer(&mut out, o + 8, names_at[s_i]);
            if sec.bones.is_empty() {
                put(&mut out, o + 0x10, &[0; 8]);
            } else {
                pointer(&mut out, o + 0x10, bones_at[s_i]);
            }
            put(&mut out, o + 0x18, &u16_of(sec.bones.len(), o, "bones")?);
            out[o + 0x1b] = sec.flags;
            out[o + 0x1e] = sec.stride;
            put(&mut out, o + 0x20, &sec.primitives.to_le_bytes());
            put(&mut out, o + 0x24, &sec.start_index.to_le_bytes());
            put(&mut out, o + 0x140, &sec.start_index.to_le_bytes());
            put(&mut out, o + 0x28, &sec.vertex_offset.to_le_bytes());
            put(&mut out, o + 0x54, &sec.vertex_offset.to_le_bytes());
            put(&mut out, o + 0x2c, &sec.vertices.to_le_bytes());
            put(&mut out, o + DECL_AT, &sec.decl.bytes);
            put(&mut out, o + DECL2_AT, &sec.draw_decl.bytes);
            put(&mut out, o + KEY_AT, &sec.key.to_le_bytes());
            s_i += 1;
        }
    }
    for l in 0..MAX_LODS {
        match lod_at.get(l) {
            Some(&at) => pointer(&mut out, 0x30 + 8 * l, at),
            None => put(&mut out, 0x30 + 8 * l, &[0; 8]),
        }
    }
    put(&mut out, 0xb4, &u16_of(m.lods.len(), 0xb4, "LODs")?);
    pointer(&mut out, 0x68, name_at);
    pointer(&mut out, 0x70, short_at);
    if hs == HEADER_BONES {
        let bones = u16::from_le_bytes([out[0xc4], out[0xc5]]) as usize;
        let bounded = m.bone_ids.as_ref().map_or(0, Vec::len);
        let bounds_for = if m.bone_ids.is_some() { bounded } else { bones };
        if m.bone_bounds.as_ref().is_some_and(|b| b.len() != bounds_for) {
            return Err(bad(
                W,
                0xd0,
                format!("bone bounds for other than {bounds_for} bones"),
            ));
        }
        put(&mut out, 0xc6, &u16_of(bounded, 0xc6, "bounded bones")?);
        match ids_at {
            Some(at) => pointer(&mut out, 0xc8, at),
            None => put(&mut out, 0xc8, &[0; 8]),
        }
        match bounds_at {
            Some(at) => pointer(&mut out, 0xd0, at),
            None => put(&mut out, 0xd0, &[0; 8]),
        }
    }
    // the relocation table
    let table = out.len();
    for f in pointer_fields(m, &lod_at, &sec_at) {
        out.extend(u32_of(f - 16, "bytes")?);
    }
    let size = out.len() - table;
    pad(&mut out, 16);
    out.extend(&m.tail);
    let mut meta = [0u8; 16];
    put(&mut meta, 0, &u32_of(table, "bytes")?);
    put(&mut meta, 4, &u32_of(size, "bytes")?);
    put(&mut meta, 8, &u32_of(m.tail.len(), "bytes")?);
    Ok((out, meta))
}
