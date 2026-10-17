//! Vertex arithmetic preserves f32 operation order; no fused multiply-add or implicit axis conversion.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::BTreeMap;
#[derive(Debug, thiserror::Error)]
#[error("invalid vertex input: {0}")]
pub struct Error(pub String);
pub type Result<T> = std::result::Result<T, Error>;
fn finite(v: &[f32]) -> Result<()> {
    if v.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(Error("non-finite value".into()))
    }
}
/// Supported DXGI element encodings. Unknown numeric IDs are rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Format {
    Float4 = 2,
    Float3 = 6,
    Half4 = 10,
    Snorm16x4 = 13,
    Float2 = 16,
    Unorm10x3A2 = 24,
    Unorm8x4 = 28,
    Half2 = 34,
    Unorm16x2 = 35,
    Snorm16x2 = 37,
}
impl Format {
    pub fn from_dxgi(id: u32) -> Result<Self> {
        Ok(match id {
            2 => Self::Float4,
            6 => Self::Float3,
            10 => Self::Half4,
            13 => Self::Snorm16x4,
            16 => Self::Float2,
            24 => Self::Unorm10x3A2,
            28 => Self::Unorm8x4,
            34 => Self::Half2,
            35 => Self::Unorm16x2,
            37 => Self::Snorm16x2,
            _ => return Err(Error(format!("unsupported DXGI format {id}"))),
        })
    }
    pub fn size(self) -> usize {
        match self {
            Self::Float4 => 16,
            Self::Float3 => 12,
            Self::Half4 | Self::Snorm16x4 | Self::Float2 => 8,
            _ => 4,
        }
    }
    /// Decode exactly one element; missing components are zero. Reject NaN/infinity.
    pub fn decode(self, data: &[u8]) -> Result<[f32; 4]> {
        if data.len() != self.size() {
            return Err(Error(format!(
                "expected {} element bytes, got {}",
                self.size(),
                data.len()
            )));
        }
        let mut out = [0.; 4];
        match self {
            Self::Float2 | Self::Float3 | Self::Float4 => {
                for (i, b) in data.as_chunks::<4>().0.iter().enumerate() {
                    out[i] = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                }
            }
            Self::Half2 | Self::Half4 => {
                for (i, b) in data.as_chunks::<2>().0.iter().enumerate() {
                    out[i] = half::f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32();
                }
            }
            Self::Snorm16x2 | Self::Snorm16x4 => {
                for (i, b) in data.as_chunks::<2>().0.iter().enumerate() {
                    out[i] = snorm16(i16::from_le_bytes([b[0], b[1]]));
                }
            }
            Self::Unorm16x2 => {
                for (i, b) in data.as_chunks::<2>().0.iter().enumerate() {
                    out[i] = u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.;
                }
            }
            Self::Unorm8x4 => {
                for i in 0..4 {
                    out[i] = data[i] as f32 / 255.;
                }
            }
            Self::Unorm10x3A2 => {
                let p = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                out = [
                    (p & 1023) as f32 / 1023.,
                    ((p >> 10) & 1023) as f32 / 1023.,
                    ((p >> 20) & 1023) as f32 / 1023.,
                    (p >> 30) as f32 / 3.,
                ];
            }
        }
        finite(&out)?;
        Ok(out)
    }
}
/// D3D signed normalization, including -32768 mapping to -1.
pub fn snorm16(v: i16) -> f32 {
    (v as f32 / 32767.).max(-1.)
}
/// Element offsets are either explicit or appended. Each slot's append cursor sums
/// element sizes in declaration order, independently of explicit offsets. This is
/// the reference layout convention, not D3D's end-of-previous-element convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub semantic: String,
    pub index: u32,
    pub format: Format,
    pub slot: u32,
    pub offset: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedElement {
    pub element: Element,
    pub offset: usize,
}
pub fn resolve_layout(elements: &[Element]) -> Result<Vec<ResolvedElement>> {
    let mut cursors = BTreeMap::<u32, usize>::new();
    let mut result = Vec::new();
    for e in elements {
        let cur = cursors.entry(e.slot).or_default();
        let offset = e.offset.unwrap_or(*cur);
        *cur = cur
            .checked_add(e.format.size())
            .ok_or_else(|| Error("layout offset overflow".into()))?;
        offset
            .checked_add(e.format.size())
            .ok_or_else(|| Error("element end overflow".into()))?;
        result.push(ResolvedElement {
            element: e.clone(),
            offset,
        });
    }
    Ok(result)
}
#[derive(Clone, Copy, Debug)]
pub struct Stream<'a> {
    pub bytes: &'a [u8],
    pub stride: usize,
    pub offset: usize,
}
/// Decode consecutive vertices from a resolved element and its bound stream.
/// All arithmetic and bounds are checked before reading; stream padding is allowed.
pub fn decode_stream(
    element: &ResolvedElement,
    stream: Stream<'_>,
    first: usize,
    count: usize,
) -> Result<Vec<[f32; 4]>> {
    let size = element.element.format.size();
    let end = element
        .offset
        .checked_add(size)
        .ok_or_else(|| Error("offset overflow".into()))?;
    if stream.stride == 0 || end > stream.stride {
        return Err(Error("element extends beyond stride".into()));
    }
    let start = first
        .checked_mul(stream.stride)
        .and_then(|x| x.checked_add(stream.offset))
        .and_then(|x| x.checked_add(element.offset))
        .ok_or_else(|| Error("stream offset overflow".into()))?;
    let last = if count == 0 {
        start
    } else {
        (count - 1)
            .checked_mul(stream.stride)
            .and_then(|x| x.checked_add(start))
            .and_then(|x| x.checked_add(size))
            .ok_or_else(|| Error("stream end overflow".into()))?
    };
    if last > stream.bytes.len() {
        return Err(Error(format!(
            "stream needs {last} bytes, has {}",
            stream.bytes.len()
        )));
    }
    (0..count)
        .map(|i| {
            let off = start + i * stream.stride;
            element.element.format.decode(&stream.bytes[off..off + size])
        })
        .collect()
}
/// Per-axis scale then translation, rounded at each f32 operation.
pub fn scale_translate(v: [f32; 3], scale: [f32; 3], translate: [f32; 3]) -> Result<[f32; 3]> {
    finite(&v)?;
    finite(&scale)?;
    finite(&translate)?;
    let r = std::array::from_fn(|i| v[i] * scale[i] + translate[i]);
    finite(&r)?;
    Ok(r)
}
/// UV transform is [uOffset,uScale,vOffset,vScale], followed by v -> 1-v.
pub fn transform_uv(uv: [f32; 2], t: [f32; 4]) -> Result<[f32; 2]> {
    finite(&uv)?;
    finite(&t)?;
    let r = [uv[0] * t[1] + t[0], 1. - (uv[1] * t[3] + t[2])];
    finite(&r)?;
    Ok(r)
}
/// Normalize in f32; zero vectors remain zero.
pub fn normalize(v: [f32; 3]) -> Result<[f32; 3]> {
    finite(&v)?;
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !len.is_finite() {
        return Err(Error("normal length overflow".into()));
    }
    Ok(if len > 0. { v.map(|x| x / len) } else { v })
}
/// R10G10B10A2 direction: remap xyz to [-1,1], normalize, and set
/// handedness to +1 when bit 31 is set, otherwise -1. Bit 30 is ignored.
pub fn tangent(p: u32) -> Result<[f32; 4]> {
    let n = normalize([
        (p & 1023) as f32 / 1023. * 2. - 1.,
        ((p >> 10) & 1023) as f32 / 1023. * 2. - 1.,
        ((p >> 20) & 1023) as f32 / 1023. * 2. - 1.,
    ])?;
    Ok([n[0], n[1], n[2], if p >> 31 != 0 { 1. } else { -1. }])
}
/// A two-component SNORM normal obtains x from decoded POSITION.w, then normalizes.
pub fn split_normal(position_w: f32, yz: [i16; 2]) -> Result<[f32; 3]> {
    normalize([position_w, snorm16(yz[0]), snorm16(yz[1])])
}
/// Packed 11/11/10 position with section scale and translation.
pub fn packed_position32(p: u32, scale: [f32; 3], offset: [f32; 3]) -> Result<[f32; 3]> {
    scale_translate(
        [(p & 0x7ff) as f32, ((p >> 11) & 0x7ff) as f32, (p >> 22) as f32],
        scale,
        offset,
    )
}
/// Packed 21/21/22 position within an inclusive bounding box. Scale is computed
/// by subtracting box endpoints then dividing by the field maximum, all in f32.
pub fn packed_position64(p: u64, min: [f32; 3], max: [f32; 3]) -> Result<[f32; 3]> {
    finite(&min)?;
    finite(&max)?;
    let scale = [
        (max[0] - min[0]) / 2097151.,
        (max[1] - min[1]) / 2097151.,
        (max[2] - min[2]) / 4194303.,
    ];
    scale_translate(
        [
            (p & 0x1fffff) as f32,
            ((p >> 21) & 0x1fffff) as f32,
            (p >> 42) as f32,
        ],
        scale,
        min,
    )
}
/// Alternating strip winding; duplicate-index joints are dropped without resetting
/// parity. With reverse=true, the first triangle is [v1,v0,v2]. No restart sentinel.
pub fn strip_to_list(strip: &[u32], reverse: bool) -> Vec<u32> {
    let mut out = Vec::new();
    for (i, t) in strip.windows(3).enumerate() {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue;
        }
        if (i % 2 == 1) ^ reverse {
            out.extend_from_slice(&[t[1], t[0], t[2]])
        } else {
            out.extend_from_slice(t)
        }
    }
    out
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexWidth {
    U16,
    U32,
}
/// Choose by the greatest referenced index; 65535 still fits u16.
pub fn index_width(indices: &[u32]) -> IndexWidth {
    if indices.iter().all(|&i| i <= u16::MAX as u32) {
        IndexWidth::U16
    } else {
        IndexWidth::U32
    }
}
/// First-use source vertex IDs and their remapped triangle indices.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Chunk {
    pub vertices: Vec<u32>,
    pub indices: Vec<u32>,
}
/// Reference collision-piece splitting: before each triangle, flush when the
/// current vertex count + 3 exceeds 65535, even if that triangle reuses vertices.
/// Input triangles remain in order and unused source vertices are omitted.
pub fn split_triangles(indices: &[u32], vertex_count: usize) -> Result<Vec<Chunk>> {
    split_by_vertex_budget(indices, vertex_count, 65535)
}
/// First-use remapping with a configurable piece budget. Flush before a triangle
/// whenever current vertices + 3 exceeds max_vertices; budgets must be 3..=65535.
pub fn split_by_vertex_budget(
    indices: &[u32],
    vertex_count: usize,
    max_vertices: usize,
) -> Result<Vec<Chunk>> {
    if !(3..=65535).contains(&max_vertices) {
        return Err(Error("vertex budget must be 3..=65535".into()));
    }
    if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= vertex_count) {
        return Err(Error("invalid triangle indices".into()));
    }
    let mut chunks = Vec::new();
    let mut chunk = Chunk::default();
    let mut map = BTreeMap::new();
    for tri in indices.as_chunks::<3>().0 {
        if chunk.vertices.len() + 3 > max_vertices {
            chunks.push(std::mem::take(&mut chunk));
            map.clear();
        }
        for &v in tri {
            let next = chunk.vertices.len() as u32;
            let n = *map.entry(v).or_insert_with(|| {
                chunk.vertices.push(v);
                next
            });
            chunk.indices.push(n);
        }
    }
    if !chunk.indices.is_empty() {
        chunks.push(chunk)
    }
    Ok(chunks)
}

/// Preserve stored normals whose length differs from one by at most 1e-3.
/// Otherwise divide each component by length when length>1e-12, or use +Z.
/// Length sums in f32, takes f64 sqrt, then rounds to f32, as the reference writer.
pub fn renormalize_stored_normal(n: [f32; 3]) -> Result<[f32; 3]> {
    finite(&n)?;
    let len = ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]) as f64).sqrt() as f32;
    finite(&[len])?;
    Ok(if (len - 1.).abs() > 1e-3 {
        if len > 1e-12 {
            n.map(|v| v / len)
        } else {
            [0., 0., 1.]
        }
    } else {
        n
    })
}
