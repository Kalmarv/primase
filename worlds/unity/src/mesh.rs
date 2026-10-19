//! Unity meshes: vertex channels in interleaved streams (decoded through `primase_vertex` layouts), or a compressed
//! mesh (bit-packed quantised floats); an index buffer of 16- or 32-bit indices; submeshes.
//!
//! Vertex data layout (Unity 5 and later): the channels name a stream, an offset in it, a format and a dimension; a
//! stream's stride is the sum of its channels' sizes, and the streams follow each other, each starting 16-byte
//! aligned. The bytes are in the mesh, or streamed from a resource file (`m_StreamData`).
//!
//! A compressed mesh's floats are `PackedBitVector`s: `n` items of `bits` bits each, little-endian bit order, each
//! `start + item * (range / (2^bits - 1))` in f32; normals and tangents keep x and y, z is rebuilt from the unit
//! length with a stored sign; its indices are packed integers (`m_Triangles`).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use primase_core::{Primitive, Semantic, Stream, Topology};
use primase_meshops::compact_primitive;
use primase_vertex::{Element, Format, Stream as VertexStream, decode_stream, resolve_layout};

use crate::level::UNITY;
use crate::player::{ObjRef, Player};
use crate::resource::StreamingInfo;
use crate::value::{Array, Value};
use crate::{Error, Result, invalid};

/// Unity's shader channels, in the order of `m_Channels`.
pub const CHANNELS: [&str; 14] = [
    "Vertex",
    "Normal",
    "Tangent",
    "Color",
    "TexCoord0",
    "TexCoord1",
    "TexCoord2",
    "TexCoord3",
    "TexCoord4",
    "TexCoord5",
    "TexCoord6",
    "TexCoord7",
    "BlendWeight",
    "BlendIndices",
];

/// One decoded channel: `dim` components per vertex.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub dim: usize,
    pub values: Vec<f32>,
}

impl Channel {
    pub fn vertex(&self, i: usize) -> &[f32] {
        &self.values[i * self.dim..(i + 1) * self.dim]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubMesh {
    /// Into the index buffer, in bytes.
    pub first_byte: u32,
    pub index_count: u32,
    /// Unity's MeshTopology (0: triangles).
    pub topology: i64,
    pub base_vertex: u32,
}

/// A mesh as stored, in Unity's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct UnityMeshData {
    pub name: String,
    pub vertex_count: usize,
    /// By shader channel (see [`CHANNELS`]); blend weights and indices are not decoded (skinning).
    pub channels: [Option<Channel>; 12],
    pub compressed: bool,
    /// The index buffer, decoded.
    pub indices: Vec<u32>,
    pub index_size: u32,
    pub submeshes: Vec<SubMesh>,
}

fn int(v: &Value, k: &str) -> Result<i64> {
    v.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("mesh: no integer {k}")))
}

fn bytes<'a>(v: &'a Value, k: &str) -> Result<&'a [u8]> {
    match v.get(k) {
        Some(Value::Array(Array::U8(b))) => Ok(b),
        _ => Err(invalid(format!("mesh: no bytes {k}"))),
    }
}

/// Unity's VertexFormat component size.
fn component_size(format: i64) -> Result<usize> {
    Ok(match format {
        0 | 10 | 11 => 4,
        1 | 4 | 5 | 8 | 9 => 2,
        2 | 3 | 6 | 7 => 1,
        f => return Err(invalid(format!("vertex format {f}"))),
    })
}

/// A Unity (format, dimension) as a vertex layout format.
fn vertex_format(format: i64, dim: usize) -> Option<Format> {
    Some(match (format, dim) {
        (0, 2) => Format::Float2,
        (0, 3) => Format::Float3,
        (0, 4) => Format::Float4,
        (1, 2) => Format::Half2,
        (1, 4) => Format::Half4,
        (2, 4) => Format::Unorm8x4,
        (4, 2) => Format::Unorm16x2,
        (5, 2) => Format::Snorm16x2,
        (5, 4) => Format::Snorm16x4,
        _ => return None,
    })
}

/// A PackedBitVector's items as integers.
fn unpack_ints(v: &Value, start: usize, count: usize) -> Result<Vec<u32>> {
    let bits = int(v, "m_BitSize")? as usize;
    let data = bytes(v, "m_Data")?;
    if bits > 32 {
        return Err(invalid(format!("packed bit size {bits}")));
    }
    if bits == 0 {
        return Ok(vec![0; count]);
    }
    let end_bit = (start + count) * bits;
    if end_bit.div_ceil(8) > data.len() {
        return Err(invalid(format!(
            "packed items {start}+{count} of {bits} bits past {} bytes",
            data.len()
        )));
    }
    let mut out = Vec::with_capacity(count);
    let mut bit = start * bits;
    for _ in 0..count {
        let mut value = 0u64;
        let mut got = 0;
        while got < bits {
            let (byte, shift) = (bit / 8, bit % 8);
            let take = (bits - got).min(8 - shift);
            let part = (u64::from(data[byte]) >> shift) & ((1 << take) - 1);
            value |= part << got;
            got += take;
            bit += take;
        }
        out.push(value as u32);
    }
    Ok(out)
}

/// A PackedBitVector's items as floats: `start + item * (range / (2^bits - 1))`, in f32.
fn unpack_floats(v: &Value, start: usize, count: usize) -> Result<Vec<f32>> {
    let bits = int(v, "m_BitSize")? as u32;
    let range = v
        .get("m_Range")
        .and_then(Value::as_f32)
        .ok_or_else(|| invalid("packed floats: no m_Range"))?;
    let first = v
        .get("m_Start")
        .and_then(Value::as_f32)
        .ok_or_else(|| invalid("packed floats: no m_Start"))?;
    if bits == 0 {
        return Ok(vec![first; count]);
    }
    let scale = range / ((1u64 << bits) - 1) as f32;
    Ok(unpack_ints(v, start, count)?
        .into_iter()
        .map(|x| x as f32 * scale + first)
        .collect())
}

fn items(v: &Value) -> Result<usize> {
    Ok(int(v, "m_NumItems")? as usize)
}

/// x, y and a stored z sign to a unit vector: z = sqrt(1 - x^2 - y^2), or (x, y, 0) normalised when that is
/// negative.
fn rebuild_z(x: f32, y: f32, positive: bool) -> [f32; 3] {
    let zsq = 1.0 - x * x - y * y;
    let [x, y, z] = if zsq >= 0.0 {
        [x, y, zsq.sqrt()]
    } else {
        let l = (x * x + y * y).sqrt();
        [x / l, y / l, 0.0]
    };
    [x, y, if positive { z } else { -z }]
}

impl UnityMeshData {
    /// Decode the Mesh object `r` of `player` (its vertex data in the mesh or streamed beside its file).
    pub fn read(player: &Player, r: &ObjRef) -> Result<Self> {
        let m = player.decode(r)?;
        let name = m.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned();
        let vd = m
            .get("m_VertexData")
            .ok_or_else(|| invalid("mesh: no m_VertexData"))?;
        let cm = m
            .get("m_CompressedMesh")
            .ok_or_else(|| invalid("mesh: no m_CompressedMesh"))?;
        let compressed = items(cm.get("m_Vertices").ok_or_else(|| invalid("no m_Vertices"))?)? > 0;
        let mut channels: [Option<Channel>; 12] = Default::default();
        let vertex_count;
        if compressed {
            let get = |k: &str| {
                cm.get(k)
                    .ok_or_else(|| invalid(format!("compressed mesh: no {k}")))
            };
            let pv = get("m_Vertices")?;
            vertex_count = items(pv)? / 3;
            channels[0] = Some(Channel {
                dim: 3,
                values: unpack_floats(pv, 0, vertex_count * 3)?,
            });
            let uv = get("m_UV")?;
            if items(uv)? > 0 {
                let info = int(cm, "m_UVInfo")? as u32;
                if info == 0 {
                    return Err(Error::Unsupported("compressed mesh UVs without UV info".into()));
                }
                let mut at = 0;
                for ch in 0..8 {
                    let b = (info >> (ch * 4)) & 0xF;
                    if b & 4 != 0 {
                        let dim = 1 + (b & 3) as usize;
                        channels[4 + ch] = Some(Channel {
                            dim,
                            values: unpack_floats(uv, at, vertex_count * dim)?,
                        });
                        at += vertex_count * dim;
                    }
                }
            }
            let n = get("m_Normals")?;
            if items(n)? > 0 {
                let xy = unpack_floats(n, 0, vertex_count * 2)?;
                let signs = unpack_ints(get("m_NormalSigns")?, 0, vertex_count)?;
                let values = (0..vertex_count)
                    .flat_map(|i| rebuild_z(xy[2 * i], xy[2 * i + 1], signs[i] != 0))
                    .collect();
                channels[1] = Some(Channel { dim: 3, values });
            }
            let t = get("m_Tangents")?;
            if items(t)? > 0 {
                let xy = unpack_floats(t, 0, vertex_count * 2)?;
                let signs = unpack_ints(get("m_TangentSigns")?, 0, vertex_count * 2)?;
                let values = (0..vertex_count)
                    .flat_map(|i| {
                        let [x, y, z] = rebuild_z(xy[2 * i], xy[2 * i + 1], signs[2 * i] != 0);
                        [x, y, z, if signs[2 * i + 1] > 0 { 1.0 } else { -1.0 }]
                    })
                    .collect();
                channels[2] = Some(Channel { dim: 4, values });
            }
            let c = get("m_FloatColors")?;
            if items(c)? > 0 {
                channels[3] = Some(Channel {
                    dim: 4,
                    values: unpack_floats(c, 0, vertex_count * 4)?,
                });
            }
        } else {
            vertex_count = int(vd, "m_VertexCount")? as usize;
            let chans = match vd.get("m_Channels") {
                Some(Value::Array(Array::Values(c))) => c,
                _ => return Err(invalid("mesh: no m_Channels")),
            };
            // stream offsets and strides
            let info: Vec<(usize, usize, i64, usize)> = chans
                .iter()
                .map(|c| {
                    Ok((
                        int(c, "stream")? as usize,
                        int(c, "offset")? as usize,
                        int(c, "format")?,
                        (int(c, "dimension")? & 0xF) as usize,
                    ))
                })
                .collect::<Result<_>>()?;
            let streams = 1 + info.iter().filter(|c| c.3 > 0).map(|c| c.0).max().unwrap_or(0);
            let mut stream_at = Vec::with_capacity(streams);
            let mut offset = 0usize;
            for s in 0..streams {
                let mut stride = 0;
                for &(cs, _, f, d) in &info {
                    if cs == s && d > 0 {
                        stride += d * component_size(f)?;
                    }
                }
                stream_at.push((offset, stride));
                offset += vertex_count * stride;
                offset = offset.next_multiple_of(16);
            }
            let stream_data = m
                .get("m_StreamData")
                .map(StreamingInfo::of)
                .transpose()?
                .flatten();
            let data: Vec<u8> = match stream_data {
                Some(s) => {
                    let dir = player
                        .folders
                        .get(&r.file)
                        .ok_or_else(|| invalid(format!("no folder for {}", r.file)))?;
                    s.read(dir)?
                }
                None => bytes(vd, "m_DataSize")?.to_vec(),
            };
            for (ch, &(s, off, f, d)) in info.iter().enumerate().take(12) {
                if d == 0 {
                    continue;
                }
                let format = vertex_format(f, d).ok_or_else(|| {
                    Error::Unsupported(format!("{} channel format {f} x {d}", CHANNELS[ch]))
                })?;
                let layout = resolve_layout(&[Element {
                    semantic: CHANNELS[ch].to_owned(),
                    index: 0,
                    format,
                    slot: s as u32,
                    offset: Some(off),
                }])
                .map_err(|e| invalid(format!("{}: {e}", CHANNELS[ch])))?;
                let (base, stride) = stream_at[s];
                let v = decode_stream(
                    &layout[0],
                    VertexStream {
                        bytes: &data,
                        stride,
                        offset: base,
                    },
                    0,
                    vertex_count,
                )
                .map_err(|e| invalid(format!("{} {}: {e}", name, CHANNELS[ch])))?;
                channels[ch] = Some(Channel {
                    dim: d,
                    values: v.iter().flat_map(|x| x[..d].to_vec()).collect(),
                });
            }
        }
        // indices
        let index_size = match int(&m, "m_IndexFormat")? {
            0 => 2,
            1 => 4,
            f => return Err(invalid(format!("index format {f}"))),
        };
        let ib = bytes(&m, "m_IndexBuffer")?;
        if ib.len() % index_size as usize != 0 {
            return Err(invalid(format!("index buffer of {} bytes", ib.len())));
        }
        // a compressed mesh keeps its indices bit-packed (`m_Triangles`); its submeshes still count index bytes
        let packed = cm.get("m_Triangles").map(items).transpose()?.unwrap_or(0);
        let indices: Vec<u32> = if compressed && packed > 0 {
            unpack_ints(cm.get("m_Triangles").expect("checked"), 0, packed)?
        } else if index_size == 2 {
            ib.as_chunks::<2>()
                .0
                .iter()
                .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
                .collect()
        } else {
            ib.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        };
        let submeshes = match m.get("m_SubMeshes") {
            Some(Value::Array(Array::Values(s))) => s
                .iter()
                .map(|s| {
                    Ok(SubMesh {
                        first_byte: int(s, "firstByte")? as u32,
                        index_count: int(s, "indexCount")? as u32,
                        topology: int(s, "topology")?,
                        base_vertex: int(s, "baseVertex")? as u32,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            _ => return Err(invalid("mesh: no m_SubMeshes")),
        };
        Ok(UnityMeshData {
            name,
            vertex_count,
            channels,
            compressed,
            indices,
            index_size,
            submeshes,
        })
    }
}

/// Unity's MeshTopology as core's, with how a submesh's indices become core's: triangles (0), lines (3) and points (5)
/// as stored, quads (2) as two triangles each (a, b, c and a, c, d), a line strip (4) as its segments; both exact.
fn topology(t: i64) -> Result<(Topology, Option<&'static str>)> {
    Ok(match t {
        0 => (Topology::Triangles, None),
        2 => (
            Topology::Triangles,
            Some("quad submeshes read as triangles, two per quad"),
        ),
        3 => (Topology::Lines, None),
        4 => (
            Topology::Lines,
            Some("line-strip submeshes read as lines, one per segment"),
        ),
        5 => (Topology::Points, None),
        t => return Err(Error::Unsupported(format!("mesh topology {t}"))),
    })
}

/// A submesh's indices as `topology` gives them to core ([`topology`]).
fn core_indices(topology: i64, indices: Vec<u32>) -> Vec<u32> {
    match topology {
        2 => indices
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
            .collect(),
        4 => indices.windows(2).flat_map(|s| [s[0], s[1]]).collect(),
        _ => indices,
    }
}

/// What reading a submesh could not put into core as stored, by what: counts for [`crate::level::LevelReport::notices`].
pub type MeshNotices = std::collections::BTreeMap<&'static str, u64>;

fn note(notices: &mut MeshNotices, what: &'static str, n: u64) {
    if n > 0 {
        *notices.entry(what).or_default() += n;
    }
}

/// A submesh's primitive made finite, as core requires, by what each stream is, judged on the vertices its indices
/// use (`used`): a non-finite position leaves the primitive out (core cannot place it; `false`), a non-finite normal or
/// tangent takes its stream out (a zero direction is not one; passes can derive it again), a non-finite texture
/// coordinate or colour reads as 0. Each counted in `notices`. The other vertices' non-finite values read as 0 too,
/// uncounted: compaction leaves them out.
fn make_finite(p: &mut Primitive, used: &BTreeSet<u32>, notices: &mut MeshNotices) -> bool {
    fn values(s: &Stream, i: usize) -> &[f32] {
        match s {
            Stream::F32x2(v) => &v[i],
            Stream::F32x3(v) => &v[i],
            Stream::F32x4(v) => &v[i],
            _ => &[],
        }
    }
    let bad = |s: &Stream| -> u64 {
        used.iter()
            .map(|&i| values(s, i as usize).iter().filter(|x| !x.is_finite()).count() as u64)
            .sum()
    };
    if p.streams.get(&Semantic::Position).is_some_and(|s| bad(s) > 0) {
        note(notices, "primitives with non-finite positions left out", 1);
        return false;
    }
    for (semantic, what) in [
        (
            Semantic::Normal,
            "primitives' normal streams with non-finite values left out",
        ),
        (
            Semantic::Tangent,
            "primitives' tangent streams with non-finite values left out",
        ),
    ] {
        if p.streams.get(&semantic).is_some_and(|s| bad(s) > 0) {
            p.streams.remove(&semantic);
            note(notices, what, 1);
        }
    }
    for (semantic, stream) in p.streams.iter_mut() {
        let what = match semantic {
            Semantic::TexCoord(_) => Some("non-finite texture coordinates read as 0"),
            Semantic::Color(_) => Some("non-finite vertex colour values read as 0"),
            _ => None,
        };
        if let Some(what) = what {
            note(notices, what, bad(stream));
        }
        let zero = |x: &mut f32| {
            if !x.is_finite() {
                *x = 0.0;
            }
        };
        match stream {
            Stream::F32x2(v) => v.iter_mut().flatten().for_each(zero),
            Stream::F32x3(v) => v.iter_mut().flatten().for_each(zero),
            Stream::F32x4(v) => v.iter_mut().flatten().for_each(zero),
            _ => {}
        }
    }
    true
}

impl UnityMeshData {
    /// The submeshes `range` as world primitives, each with its submesh index, compacted to the vertices it uses,
    /// without materials; a submesh without indices draws nothing and is left out. Unity's frame is converted ([`UNITY`]): positions and directions mirrored, triangle winding reversed
    /// so front faces stay front, and a tangent's w negated, since a mirror flips the bitangent cross(n, t) * w (both
    /// conventions point the bitangent along the normal map's +Y). Texture coordinates move to the world's top-left
    /// origin, v becoming 1 - v; a channel of 3 or 4 components gives its u and v. Normals keep three components
    /// (stored as half4, the fourth is padding). Quads become triangles and line strips lines ([`topology`]); each
    /// primitive is made finite as core requires ([`make_finite`]). What could not be read as stored is counted in the
    /// returned notices, over the primitives returned (and those left out).
    pub fn primitives(&self, range: Range<usize>) -> Result<(Vec<(usize, Primitive)>, MeshNotices)> {
        let mut notices = MeshNotices::new();
        let n = self.vertex_count;
        let mut streams = BTreeMap::new();
        for (ch, c) in self.channels.iter().enumerate() {
            let Some(c) = c else { continue };
            let xyz = |i: usize| -> [f32; 3] {
                let v = c.vertex(i);
                [v[0], v[1], v[2]]
            };
            let (semantic, stream) = match (ch, c.dim) {
                (0, 3..) => (
                    Semantic::Position,
                    Stream::F32x3((0..n).map(|i| UNITY.point(xyz(i))).collect()),
                ),
                (1, 3..) => (
                    Semantic::Normal,
                    Stream::F32x3((0..n).map(|i| UNITY.direction(xyz(i))).collect()),
                ),
                (2, 4) => (
                    Semantic::Tangent,
                    Stream::F32x4(
                        (0..n)
                            .map(|i| {
                                let [x, y, z] = UNITY.direction(xyz(i));
                                [x, y, z, -c.vertex(i)[3]]
                            })
                            .collect(),
                    ),
                ),
                (3, 4) => (
                    Semantic::Color(0),
                    Stream::F32x4(
                        (0..n)
                            .map(|i| {
                                let v = c.vertex(i);
                                [v[0], v[1], v[2], v[3]]
                            })
                            .collect(),
                    ),
                ),
                // u and v; components beyond them are not read (counted below, per primitive)
                (4..12, 2..=4) => (
                    Semantic::TexCoord(ch as u8 - 4),
                    Stream::F32x2(
                        (0..n)
                            .map(|i| {
                                let v = c.vertex(i);
                                [v[0], 1.0 - v[1]]
                            })
                            .collect(),
                    ),
                ),
                (_, d) => {
                    return Err(Error::Unsupported(format!(
                        "{} channel of {d} components",
                        CHANNELS[ch]
                    )));
                }
            };
            streams.insert(semantic, stream);
        }
        let subs = self.submeshes.get(range.clone()).ok_or_else(|| {
            invalid(format!(
                "{}: submeshes {range:?} of {}",
                self.name,
                self.submeshes.len()
            ))
        })?;
        let mut full = Primitive {
            topology: Topology::Triangles,
            streams,
            indices: None,
            material: None,
        };
        let mut out = Vec::with_capacity(subs.len());
        for (k, s) in subs.iter().enumerate() {
            let k = range.start + k;
            if s.index_count == 0 {
                continue;
            }
            let what = || format!("{} submesh {k}", self.name);
            let (core_topology, conversion) = topology(s.topology)?;
            full.topology = core_topology;
            if let Some(what) = conversion {
                note(&mut notices, what, 1);
            }
            if s.first_byte % self.index_size != 0 {
                return Err(invalid(format!("{}: first byte {}", what(), s.first_byte)));
            }
            let first = (s.first_byte / self.index_size) as usize;
            let indices = self
                .indices
                .get(first..first + s.index_count as usize)
                .ok_or_else(|| invalid(format!("{}: indices past the buffer", what())))?
                .iter()
                .map(|&i| i.checked_add(s.base_vertex))
                .collect::<Option<Vec<u32>>>()
                .ok_or_else(|| invalid(format!("{}: base vertex overflows", what())))?;
            let mut indices = core_indices(s.topology, indices);
            if full.topology == Topology::Triangles && UNITY.flips_winding() {
                for t in indices.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            let used: BTreeSet<u32> = indices.iter().copied().collect();
            let mut part = Primitive {
                indices: Some(indices),
                ..full.clone()
            };
            if !make_finite(&mut part, &used, &mut notices) {
                continue;
            }
            let compacted = compact_primitive(&part)
                .map_err(|e| invalid(format!("{}: {}", what(), e.0)))?
                .primitive;
            let vertices = used.len() as u64;
            let wide_uvs = self.channels[4..12]
                .iter()
                .flatten()
                .filter(|c| c.dim > 2)
                .count() as u64;
            note(
                &mut notices,
                "texture coordinates of 3 or 4 components: those beyond u and v not read (vertices)",
                vertices * wide_uvs,
            );
            out.push((k, compacted));
        }
        Ok((out, notices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(dim: usize, values: &[f32]) -> Option<Channel> {
        Some(Channel {
            dim,
            values: values.to_vec(),
        })
    }

    #[test]
    fn primitives_convert_unity_frame() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        // four vertices, the first unused by the drawn submesh
        channels[0] = channel(3, &[9.0, 9.0, 9.0, 0.0, 0.0, 1.0, 1.0, 0.0, 2.0, 0.0, 1.0, 3.0]);
        channels[1] = channel(3, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
        channels[2] = channel(
            4,
            &[
                1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 1.0, 1.0,
            ],
        );
        channels[4] = channel(2, &[0.5, 0.5, 0.0, 0.0, 1.0, 0.25, 0.0, 1.0]);
        let m = UnityMeshData {
            name: "t".into(),
            vertex_count: 4,
            channels,
            compressed: false,
            indices: vec![0, 1, 2],
            index_size: 2,
            submeshes: vec![
                SubMesh {
                    first_byte: 0,
                    index_count: 0,
                    topology: 0,
                    base_vertex: 0,
                },
                SubMesh {
                    first_byte: 0,
                    index_count: 3,
                    topology: 0,
                    base_vertex: 1,
                },
            ],
        };
        let (prims, notices) = m.primitives(0..2).unwrap();
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(prims.len(), 1, "the empty submesh is left out");
        let (k, p) = &prims[0];
        assert_eq!(*k, 1);
        // Unity's triangle 1, 2, 3 reversed to 1, 3, 2, then compacted in first use
        assert_eq!(p.indices, Some(vec![0, 1, 2]));
        assert_eq!(
            p.positions().unwrap(),
            &[[0.0, 0.0, -1.0], [0.0, 1.0, -3.0], [1.0, 0.0, -2.0]]
        );
        assert_eq!(
            p.streams[&Semantic::Normal],
            Stream::F32x3(vec![[0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]])
        );
        assert_eq!(
            p.streams[&Semantic::Tangent],
            Stream::F32x4(vec![
                [1.0, 0.0, 0.0, -1.0],
                [0.0, 0.0, -1.0, -1.0],
                [0.0, 0.0, -1.0, 1.0]
            ])
        );
        assert_eq!(
            p.streams[&Semantic::TexCoord(0)],
            Stream::F32x2(vec![[0.0, 1.0], [0.0, 0.0], [1.0, 0.75]])
        );
        assert!(m.primitives(1..3).is_err(), "submeshes past the mesh");
    }

    fn mesh(channels: [Option<Channel>; 12], indices: Vec<u32>, topology: i64) -> UnityMeshData {
        UnityMeshData {
            name: "t".into(),
            vertex_count: channels[0].as_ref().map_or(0, |c| c.values.len() / c.dim),
            channels,
            compressed: false,
            index_size: 2,
            submeshes: vec![SubMesh {
                first_byte: 0,
                index_count: indices.len() as u32,
                topology,
                base_vertex: 0,
            }],
            indices,
        }
    }

    /// A UV channel of four components gives u and v, its other two not read (counted per vertex); a non-finite u or
    /// v reads as 0 (counted); a second, two-component channel is untouched.
    #[test]
    fn texture_coordinates_of_four_components_and_non_finite_values() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        channels[4] = channel(
            4,
            &[
                0.25,
                0.5,
                7.0,
                8.0,
                f32::NAN,
                1.0,
                7.0,
                8.0,
                1.0,
                f32::INFINITY,
                7.0,
                8.0,
            ],
        );
        channels[5] = channel(2, &[0.0, 0.0, 0.5, 0.5, 1.0, 1.0]);
        let (prims, notices) = mesh(channels, vec![0, 1, 2], 0).primitives(0..1).unwrap();
        let p = &prims[0].1;
        let Stream::F32x2(uv) = &p.streams[&Semantic::TexCoord(0)] else {
            panic!("two components")
        };
        let mut uv = uv.clone();
        uv.sort_by(|a, b| a.partial_cmp(b).unwrap());
        // v becomes 1 - v: the infinite v's 1 - v is -inf, read as 0
        assert_eq!(uv, vec![[0.0, 0.0], [0.25, 0.5], [1.0, 0.0]]);
        assert!(p.streams.contains_key(&Semantic::TexCoord(1)));
        assert_eq!(
            notices,
            MeshNotices::from([
                ("non-finite texture coordinates read as 0", 2),
                (
                    "texture coordinates of 3 or 4 components: those beyond u and v not read (vertices)",
                    3
                ),
            ])
        );
    }

    /// A quad submesh is two triangles per quad and a line strip one line per segment, both counted.
    #[test]
    fn quads_and_line_strips_become_triangles_and_lines() {
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        let (prims, notices) = mesh(channels.clone(), vec![0, 1, 2, 3], 2)
            .primitives(0..1)
            .unwrap();
        let p = &prims[0].1;
        assert_eq!(p.topology, Topology::Triangles);
        assert_eq!(p.indices.as_ref().map(Vec::len), Some(6));
        assert_eq!(
            notices,
            MeshNotices::from([("quad submeshes read as triangles, two per quad", 1)])
        );
        let (prims, notices) = mesh(channels, vec![0, 1, 2, 3], 4).primitives(0..1).unwrap();
        let p = &prims[0].1;
        assert_eq!(p.topology, Topology::Lines);
        assert_eq!(p.indices.as_ref().map(Vec::len), Some(6));
        assert_eq!(
            notices,
            MeshNotices::from([("line-strip submeshes read as lines, one per segment", 1)])
        );
    }

    /// A non-finite tangent takes the primitive's tangent stream out; a non-finite position leaves the primitive out;
    /// a non-finite colour reads as 0; each counted.
    #[test]
    fn non_finite_vertex_values_by_what_they_are() {
        let tri = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut channels: [Option<Channel>; 12] = Default::default();
        channels[0] = channel(3, &tri);
        channels[1] = channel(3, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
        channels[2] = channel(
            4,
            &[1.0, 0.0, 0.0, 1.0, f32::NAN, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
        );
        channels[3] = channel(
            4,
            &[
                1.0,
                1.0,
                1.0,
                1.0,
                f32::INFINITY,
                1.0,
                1.0,
                1.0,
                1.0,
                1.0,
                1.0,
                1.0,
            ],
        );
        let (prims, notices) = mesh(channels, vec![0, 1, 2], 0).primitives(0..1).unwrap();
        let p = &prims[0].1;
        assert!(!p.streams.contains_key(&Semantic::Tangent));
        assert!(p.streams.contains_key(&Semantic::Normal));
        assert_eq!(
            notices,
            MeshNotices::from([
                ("non-finite vertex colour values read as 0", 1),
                ("primitives' tangent streams with non-finite values left out", 1),
            ])
        );
        let mut channels: [Option<Channel>; 12] = Default::default();
        let mut bad = tri;
        bad[4] = f32::NAN;
        channels[0] = channel(3, &bad);
        let (prims, notices) = mesh(channels, vec![0, 1, 2], 0).primitives(0..1).unwrap();
        assert!(prims.is_empty());
        assert_eq!(
            notices,
            MeshNotices::from([("primitives with non-finite positions left out", 1)])
        );
    }
}
