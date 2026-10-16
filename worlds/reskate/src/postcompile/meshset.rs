//! MeshSet resources (type 0x49B156D4): the LODs, their sections and vertex declarations, and where each section's
//! vertices and indices lie in the LOD's geometry chunk.
//!
//! Offsets in the payload are stored relative to byte 16. Layout as measured on every MeshSet of the base game's
//! San Vansterdam level and on Studio's output (the census gate checks each relation):
//! - header: LOD record pointers (u64) from 0x30, LOD count (u16) at 0xb4;
//! - LOD record: section count (u32) +0x08, section array pointer (u64) +0x0c, geometry chunk GUID +0x74, index-buffer
//!   bytes (u32) +0x58, vertex-buffer bytes (u32) +0x5c, a multiple of 16, index format (u32) +0x54 (0x21: 16-bit,
//!   0x2e: 32-bit indices); the chunk holds the vertex buffer, then the index buffer;
//! - section (384 bytes): primitive count +0x20, start index +0x24 (again at +0x140), vertex offset in bytes +0x28
//!   (again at +0x54), vertex count +0x2c, flags +0x1b, total vertex stride +0x1e, two vertex declarations (100
//!   bytes each: 16 elements of usage, format, offset, stream; 16 stream strides of two bytes; element count; stream
//!   count): the stored one at +0x70 and the draw one at +0xd4 (usually equal; see `check_draw` for how it may
//!   differ: unbound elements, aliases into stored streams, streams filled at run time); material
//!   key (u64) +0x138. A section's stored streams lie one after another from its vertex offset, each
//!   `stride * vertex count` bytes.

use crate::postcompile::{Rd, Result, bad, capacity};

const W: &str = "meshset";
pub const SECTION_SIZE: usize = 384;
pub const DECL_SIZE: usize = 100;
pub const DECL_AT: usize = 0x70;
pub const DECL2_AT: usize = 0xd4;
pub const KEY_AT: usize = 0x138;

/// Element usages seen in skate.'s declarations (Frostbite's VertexElementUsage).
pub mod usage {
    pub const POSITION: u8 = 1;
    pub const COLOR0: u8 = 30;
    pub const COLOR1: u8 = 31;
    pub const TEXCOORD0: u8 = 33;
    pub const TEXCOORD1: u8 = 34;
    pub const RADIOSITY: u8 = 42;
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

/// A vertex declaration, decoded, and its stored bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub elements: Vec<Element>,
    pub strides: Vec<u8>,
    pub bytes: [u8; DECL_SIZE],
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Offset of the section record in the payload.
    pub at: usize,
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
    pub fn vertex_end(&self) -> usize {
        self.vertex_offset as usize + self.decl.stride() * self.vertices as usize
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lod {
    pub at: usize,
    pub chunk: [u8; 16],
    /// Bytes per index: 2 (format 0x21) or 4 (format 0x2e), from the LOD's index format at +0x54.
    pub index_size: u8,
    pub index_bytes: u32,
    pub vertex_bytes: u32,
    pub sections: Vec<Section>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshSet {
    pub lods: Vec<Lod>,
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

/// The MeshSet's LODs and sections. Checks each layout relation the module doc names and fails on the first broken
/// one (strict: a MeshSet this reader accepts can be rewritten section by section).
pub fn read(d: &[u8]) -> Result<MeshSet> {
    let r = Rd::new(d, W);
    let n = r.u16le(0xb4)? as usize;
    let mut lods = Vec::with_capacity(n);
    for l in 0..n {
        let at = ptr(&r, 0x30 + 8 * l)?;
        let count = r.u32le(at + 8)? as usize;
        let first = ptr(&r, at + 0x0c)?;
        let mut lod = Lod {
            at,
            chunk: r.arr(at + 0x74)?,
            index_size: match r.u32le(at + 0x54)? {
                0x21 => 2,
                0x2e => 4,
                f => return Err(bad(W, at + 0x54, format!("unknown index format {f:#x}"))),
            },
            index_bytes: r.u32le(at + 0x58)?,
            vertex_bytes: r.u32le(at + 0x5c)?,
            sections: Vec::with_capacity(capacity(W, count, SECTION_SIZE, d.len())?),
        };
        for s in 0..count {
            let o = first + SECTION_SIZE * s;
            let decl = Decl::read(&r.arr(o + DECL_AT)?)?;
            let draw_decl = Decl::read(&r.arr(o + DECL2_AT)?)?;
            check_draw(&decl, &draw_decl).map_err(|d| bad(W, o, d))?;
            let sec = Section {
                at: o,
                key: r.u64le(o + KEY_AT)?,
                decl,
                draw_decl,
                flags: r.u8(o + 0x1b)?,
                stride: r.u8(o + 0x1e)?,
                primitives: r.u32le(o + 0x20)?,
                start_index: r.u32le(o + 0x24)?,
                vertex_offset: r.u32le(o + 0x28)?,
                vertices: r.u32le(o + 0x2c)?,
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
    Ok(MeshSet { lods })
}
