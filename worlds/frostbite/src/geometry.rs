//! A MeshSet section's vertices and triangles from its LOD's geometry chunk ([`decode`]): the chunk holds the vertex
//! buffer then the index buffer; a section's vertices lie from its vertex offset as one array per stream of its stored
//! declaration (`stride * vertices` bytes each, one after another), its indices from `start_index` (in indices), local
//! to the section's own vertices, three a triangle.
//!
//! What is decoded, and the evidence for it:
//! - **Position** (usage 1): Half4 (the w lane is 1.0 in every vertex read; anything else is refused) or Float3, kept
//!   as stored: model space, Y up, metres. Evidence: the vertices of a MeshSet's sections fill the bounding box the
//!   MeshSet header stores at 0x10 (minimum) and 0x20 (maximum), within the Half4 precision; and a mesh's vertex and
//!   triangle counts are those ReSkate Studio reports for it (`reskate_cli meshes`).
//! - **Texture coordinates** (usages 33 and 34, `TexCoord(0)` and `TexCoord(1)`): Half2 or Float2, as stored. That
//!   the origin is the image's top-left corner (as D3D's) is not verified.
//! - **Colour** (usages 30 and 31, `Color(0)` and `Color(1)`): UByte4N as its four bytes, or Float4, as stored (the
//!   channel order is not verified).
//!
//! - **Blend indices and weights** (usages 2 and 4): four little-endian u16 ([`format::USHORT4`]) and four
//!   unsigned-normalised bytes ([`format::UBYTE4N`]), read from their [`Raw`] elements by [`Raw::joints`] and
//!   [`Raw::weights`], not decoded into streams: core takes joint and weight streams only with a `core.skin`, which
//!   names the skeleton, and the skeleton the indices address is not read. Evidence (the hat bucket of skate., 5 meshes
//!   of 84 to 1,150 vertices): the vertex programs read the two as `TEXCOORD0` `uint4` and `TEXCOORD1` `float4`, take
//!   three palette rows at three times the first and sum the palette words by the second; the input layouts of the 30
//!   permutations of the bucket give DXGI 12 (`R16G16B16A16_UINT`) for usage 2 and 28 (`R8G8B8A8_UNORM`) for usage 4;
//!   the indices run to 107 and the weights are 255 on one lane (lane 3) with at most 2 on the others.
//!
//! Every other element (the sub-material index of usage 51, the tangent space of usage 52, the radiosity of usage 42,
//! and the blend indices and weights above) is kept as stored bytes ([`Raw`]): what the bytes of the rest mean is not
//! established here. A declaration or element the decoder does not know is refused naming what it is.

use std::collections::BTreeMap;

use half::f16;
use primase_core::{Semantic, Stream};

use crate::meshset::{Lod, Section, format, usage};
use crate::{Error, Result};

/// One vertex element kept as stored: its usage and format as the declaration states them, its size in bytes and the
/// bytes of every vertex of the section in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raw {
    pub usage: u8,
    pub format: u8,
    /// Bytes per vertex.
    pub size: usize,
    pub data: Vec<u8>,
}

impl Raw {
    /// The vertices `from` names (one new vertex per entry, a copy of that old one), or an error naming the first
    /// index the data does not have.
    pub fn gather(&self, from: &[u32]) -> std::result::Result<Raw, String> {
        let count = self.data.len().checked_div(self.size).unwrap_or(0);
        let mut data = Vec::with_capacity(from.len() * self.size);
        for &v in from {
            let at = (v as usize)
                .checked_mul(self.size)
                .filter(|_| (v as usize) < count)
                .ok_or_else(|| format!("vertex {v} of {count}"))?;
            data.extend_from_slice(&self.data[at..at + self.size]);
        }
        Ok(Raw { data, ..self.clone() })
    }
}

impl Raw {
    /// The blend indices of a usage 2 element stored as four u16 ([`format::USHORT4`]), one list of four a vertex;
    /// `None` for any other element.
    pub fn joints(&self) -> Option<Vec<[u16; 4]>> {
        (self.usage == usage::BLEND_INDICES && self.format == format::USHORT4 && self.size == 8).then(|| {
            self.data
                .as_chunks::<8>()
                .0
                .iter()
                .map(|v| std::array::from_fn(|k| u16::from_le_bytes([v[2 * k], v[2 * k + 1]])))
                .collect()
        })
    }

    /// The blend weights of a usage 4 element stored as four unsigned-normalised bytes ([`format::UBYTE4N`]) as
    /// stored (each over 255), one list of four a vertex; `None` for any other element.
    pub fn weights(&self) -> Option<Vec<[u8; 4]>> {
        (self.usage == usage::BLEND_WEIGHTS && self.format == format::UBYTE4N && self.size == 4)
            .then(|| self.data.as_chunks::<4>().0.to_vec())
    }
}

/// A section decoded: the streams core knows, the triangles' indices (local to the section) and the elements kept raw.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub streams: BTreeMap<Semantic, Stream>,
    pub indices: Vec<u32>,
    pub raw: Vec<Raw>,
}

fn refused(what: impl std::fmt::Display) -> Error {
    Error::Unsupported(format!("mesh geometry: {what}"))
}

fn slice<'a>(chunk: &'a [u8], at: usize, len: usize, what: &str) -> Result<&'a [u8]> {
    at.checked_add(len)
        .and_then(|end| chunk.get(at..end))
        .ok_or_else(|| {
            refused(format_args!(
                "{what}: {len} bytes at {at} in a chunk of {}",
                chunk.len()
            ))
        })
}

fn f32le(b: &[u8]) -> f32 {
    f32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn half_le(b: &[u8]) -> f32 {
    f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32()
}

/// Section `section` of `lod`, from the LOD's geometry chunk `chunk` (the vertex buffer then the index buffer).
pub fn decode(chunk: &[u8], lod: &Lod, section: &Section) -> Result<Decoded> {
    let n = section.vertices as usize;
    let mut streams = BTreeMap::new();
    let mut raw = Vec::new();
    for e in &section.decl.elements {
        let stride = usize::from(section.decl.strides[usize::from(e.stream)]);
        let size = format::size(e.format)
            .ok_or_else(|| refused(format_args!("element format {} of usage {}", e.format, e.usage)))?;
        let stream_at = section.stream_offset(usize::from(e.stream));
        // the whole stream lies in the vertex buffer
        slice(chunk, stream_at, stride * n, "a vertex stream")?;
        // each vertex's element: `size` bytes at `base + v * stride`
        let base = stream_at + usize::from(e.offset);
        let element = |v: usize| -> Result<&[u8]> { slice(chunk, base + v * stride, size, "vertex data") };
        let decoded = match (e.usage, e.format) {
            (usage::POSITION, format::HALF4) => {
                let mut out = Vec::with_capacity(n);
                for v in 0..n {
                    let b = element(v)?;
                    let w = half_le(&b[6..8]);
                    if w != 1.0 {
                        return Err(refused(format_args!("vertex {v}: position w {w}, not 1")));
                    }
                    out.push([half_le(&b[0..2]), half_le(&b[2..4]), half_le(&b[4..6])]);
                }
                Some((Semantic::Position, Stream::F32x3(out)))
            }
            (usage::POSITION, format::FLOAT3) => {
                let mut out = Vec::with_capacity(n);
                for v in 0..n {
                    let b = element(v)?;
                    out.push([f32le(&b[0..4]), f32le(&b[4..8]), f32le(&b[8..12])]);
                }
                Some((Semantic::Position, Stream::F32x3(out)))
            }
            (usage::TEXCOORD0 | usage::TEXCOORD1, format::HALF2 | format::FLOAT2) => {
                let set = u8::from(e.usage == usage::TEXCOORD1);
                let mut out = Vec::with_capacity(n);
                for v in 0..n {
                    let b = element(v)?;
                    out.push(if e.format == format::HALF2 {
                        [half_le(&b[0..2]), half_le(&b[2..4])]
                    } else {
                        [f32le(&b[0..4]), f32le(&b[4..8])]
                    });
                }
                Some((Semantic::TexCoord(set), Stream::F32x2(out)))
            }
            (usage::COLOR0 | usage::COLOR1, format::UBYTE4N) => {
                let set = u8::from(e.usage == usage::COLOR1);
                let mut out = Vec::with_capacity(n);
                for v in 0..n {
                    let b = element(v)?;
                    out.push([b[0], b[1], b[2], b[3]]);
                }
                Some((Semantic::Color(set), Stream::U8x4(out)))
            }
            (usage::COLOR0 | usage::COLOR1, format::FLOAT4) => {
                let set = u8::from(e.usage == usage::COLOR1);
                let mut out = Vec::with_capacity(n);
                for v in 0..n {
                    let b = element(v)?;
                    out.push([
                        f32le(&b[0..4]),
                        f32le(&b[4..8]),
                        f32le(&b[8..12]),
                        f32le(&b[12..16]),
                    ]);
                }
                Some((Semantic::Color(set), Stream::F32x4(out)))
            }
            (usage::POSITION | usage::TEXCOORD0 | usage::TEXCOORD1 | usage::COLOR0 | usage::COLOR1, f) => {
                return Err(refused(format_args!("usage {} stored with format {f}", e.usage)));
            }
            _ => None,
        };
        match decoded {
            Some((semantic, stream)) => {
                if streams.insert(semantic, stream).is_some() {
                    return Err(refused(format_args!("two elements for {semantic:?}")));
                }
            }
            None => {
                let mut data = Vec::with_capacity(n * size);
                for v in 0..n {
                    data.extend_from_slice(element(v)?);
                }
                raw.push(Raw {
                    usage: e.usage,
                    format: e.format,
                    size,
                    data,
                });
            }
        }
    }
    if !streams.contains_key(&Semantic::Position) {
        return Err(refused("a section with no position element"));
    }
    // the indices: triangles from `start_index`, in the index buffer after the vertex buffer, local to the section
    let isize = usize::from(lod.index_size);
    let count = section.primitives as usize * 3;
    let at = lod.vertex_bytes as usize + section.start_index as usize * isize;
    let bytes = slice(chunk, at, count * isize, "the index buffer")?;
    let mut indices = Vec::with_capacity(count);
    for i in 0..count {
        let b = &bytes[i * isize..(i + 1) * isize];
        let v = if isize == 2 {
            u32::from(u16::from_le_bytes([b[0], b[1]]))
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        };
        if v as usize >= n {
            return Err(refused(format_args!("index {i} is vertex {v} of {n}")));
        }
        indices.push(v);
    }
    Ok(Decoded {
        streams,
        indices,
        raw,
    })
}
