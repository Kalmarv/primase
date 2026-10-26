//! A Mesh object's compiled geometry: the CompiledMesh it holds, read into core primitives.
//!
//! A Mesh object's body holds its CompiledMesh inline: the CompiledMesh class (`0xFC9E1595`, CRC-32 of
//! `CompiledMesh`), its Data array's length (u32), then the Data (little-endian):
//! - a header of 12 u32: 16, 0, the vertex stride, the vertex buffer's bytes, the stride again, the vertex bytes again,
//!   the index buffer's bytes, 0, the vertex flags, the standard primitive count, the shadow primitive count, and a
//!   size (the buffers' and the primitive rows' bytes);
//! - the vertex buffer, the index buffer (u16, indexing the whole vertex buffer), then a 24-byte row per primitive,
//!   the standard primitives first: six u32: the primitive type (4: a triangle list, as Direct3D numbers it), the first
//!   vertex, the vertex count, the first index, the triangle count, and a value whose meaning is not read; then 120
//!   bytes not read.
//!
//! After the Data: PlatformVersion (u32, 16) and SDKVersion (u32, 32), then the Mesh's CompiledMeshMaterials: a
//! count (u32) and that many Material ids (u32), the Mesh body's end. Standard primitive `i` draws with material `i`.
//!
//! A vertex is, in order: the position (3 x f32), the normal (4 x u8, `(b - 127.5) / 127.5`, the fourth unused), a
//! tangent (4 x u8, as the normal; present when the stride has room for it), a colour (4 x u8, when the flags hold
//! 64), the first texture coordinates (2 x f32), the second (when the flags hold 4), 8 bytes more when the flags hold
//! 2 and 8 when they hold 1 (skinning, not read); a stride 4 bytes longer still holds 4 bytes before the skinning
//! that are not read. The layout is read from the flags and the stride: a stride the rule
//! does not give is refused, naming both. The standard primitives become core primitives; the shadow primitives are
//! the same buffers' depth-only draws and are not read.

use std::collections::BTreeMap;

use primase_bytes::{Field, FieldError, View};
use primase_core::{Primitive, Semantic, Stream, Topology};

const LE: primase_bytes::Endian = primase_bytes::Endian::Little;

/// CRC-32 of `CompiledMesh`.
pub const COMPILED_MESH: u32 = 0xFC9E_1595;
/// CRC-32 of `Mesh`.
pub const MESH: u32 = 0x415D_9568;

/// Why a Mesh's geometry can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Short(#[from] FieldError),
    #[error("no CompiledMesh in the Mesh object")]
    NoCompiledMesh,
    #[error("CompiledMesh: {0}")]
    Layout(String),
}

/// One primitive row as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimitiveRow {
    pub kind: u32,
    pub first_vertex: u32,
    pub vertices: u32,
    pub first_index: u32,
    pub triangles: u32,
    pub unknown: u32,
}

/// A CompiledMesh as stored: its header's values, buffers and rows.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledMesh {
    pub stride: usize,
    pub flags: u32,
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
    pub standard: Vec<PrimitiveRow>,
    pub shadow: Vec<PrimitiveRow>,
    /// The Mesh's CompiledMeshMaterials: standard primitive `i`'s Material id is `materials[i]`.
    pub materials: Vec<u32>,
}

/// Where each vertex element is, from the flags and the stride.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    tangent: Option<usize>,
    color: Option<usize>,
    uv0: usize,
    uv1: Option<usize>,
}

fn layout(flags: u32, stride: usize) -> Option<Layout> {
    let color = flags & 64 != 0;
    let uv1 = flags & 4 != 0;
    let extra = if flags & 2 != 0 { 8 } else { 0 } + if flags & 1 != 0 { 8 } else { 0 };
    let without = 12 + 4 + usize::from(color) * 4 + 8 + usize::from(uv1) * 8 + extra;
    let tangent = match stride.checked_sub(without)? {
        0 => false,
        4 | 8 => true,
        _ => return None,
    };
    let mut at = 16;
    let tangent_at = tangent.then(|| {
        at += 4;
        at - 4
    });
    let color_at = color.then(|| {
        at += 4;
        at - 4
    });
    let uv0 = at;
    at += 8;
    let uv1_at = uv1.then_some(at);
    Some(Layout {
        tangent: tangent_at,
        color: color_at,
        uv0,
        uv1: uv1_at,
    })
}

impl CompiledMesh {
    /// The CompiledMesh of the Mesh object body `body` (the first one it holds: the module documentation).
    pub fn in_mesh(body: &[u8]) -> Result<CompiledMesh, Error> {
        let at = body
            .windows(4)
            .position(|w| w == COMPILED_MESH.to_le_bytes())
            .ok_or(Error::NoCompiledMesh)?;
        let v = View::new(body);
        let length = v
            .u32(at.saturating_add(4), LE)
            .field("CompiledMesh.Data length")? as usize;
        let data = v.slice(at.saturating_add(8), length).field("CompiledMesh.Data")?;
        let mut mesh = CompiledMesh::parse(data)?;
        let mat = at.saturating_add(8).saturating_add(length).saturating_add(8);
        let n = v.u32(mat, LE).field("Mesh.CompiledMeshMaterials count")? as usize;
        mesh.materials = (0..n)
            .map(|k| {
                v.u32(mat.saturating_add(4).saturating_add(k.saturating_mul(4)), LE)
                    .field("a Material id")
            })
            .collect::<Result<_, _>>()?;
        Ok(mesh)
    }

    /// A CompiledMesh's Data (the module documentation).
    pub fn parse(data: &[u8]) -> Result<CompiledMesh, Error> {
        let v = View::new(data);
        let word = |i: usize| -> Result<u32, Error> {
            Ok(v.u32(i.saturating_mul(4), LE).field("CompiledMesh header")?)
        };
        let bad = |m: String| Error::Layout(m);
        let (stride, vertex_bytes, index_bytes) = (word(2)? as usize, word(3)? as usize, word(6)? as usize);
        // a second word other than 0 marks a layout of several vertex streams, not read here
        if word(0)? != 16 || word(1)? != 0 || word(4)? as usize != stride || word(5)? as usize != vertex_bytes
        {
            return Err(bad(format!(
                "a header of words {:?}: not the one-stream layout read",
                [word(0)?, word(1)?]
            )));
        }
        let flags = word(8)?;
        let (standard, shadow) = (word(9)? as usize, word(10)? as usize);
        if stride == 0 || vertex_bytes % stride != 0 || index_bytes % 2 != 0 {
            return Err(bad(format!(
                "stride {stride}, {vertex_bytes} vertex bytes, {index_bytes} index bytes"
            )));
        }
        let vat = 48usize;
        let vertices = v
            .slice(vat, vertex_bytes)
            .field("CompiledMesh vertices")?
            .to_vec();
        let iat = vat.saturating_add(vertex_bytes);
        let indices = v
            .slice(iat, index_bytes)
            .field("CompiledMesh indices")?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let mut rows = Vec::new();
        let mut at = iat.saturating_add(index_bytes);
        for _ in 0..standard.saturating_add(shadow) {
            let r = |k: usize| -> Result<u32, Error> {
                Ok(v.u32(at.saturating_add(k.saturating_mul(4)), LE)
                    .field("a primitive row")?)
            };
            rows.push(PrimitiveRow {
                kind: r(0)?,
                first_vertex: r(1)?,
                vertices: r(2)?,
                first_index: r(3)?,
                triangles: r(4)?,
                unknown: r(5)?,
            });
            at = at.saturating_add(24);
        }
        let shadow_rows = rows.split_off(standard.min(rows.len()));
        Ok(CompiledMesh {
            stride,
            flags,
            vertices,
            indices,
            standard: rows,
            shadow: shadow_rows,
            materials: Vec::new(),
        })
    }

    /// The standard primitives as core primitives in the source's own frame and units, one per row, each with the
    /// vertices its row names (re-based to 0); `material` is the caller's per row (`None` for none).
    pub fn primitives(
        &self,
        material: impl Fn(usize) -> Option<primase_core::Entity>,
    ) -> Result<Vec<Primitive>, Error> {
        let l = layout(self.flags, self.stride).ok_or_else(|| {
            Error::Layout(format!(
                "flags {:#x} with stride {}: no vertex layout read",
                self.flags, self.stride
            ))
        })?;
        let count = self.vertices.len() / self.stride;
        let mut out = Vec::with_capacity(self.standard.len());
        for (k, row) in self.standard.iter().enumerate() {
            if row.kind != 4 {
                return Err(Error::Layout(format!(
                    "primitive {k}: type {}, not a triangle list",
                    row.kind
                )));
            }
            let (first, n) = (row.first_vertex as usize, row.vertices as usize);
            if first.checked_add(n).is_none_or(|e| e > count) {
                return Err(Error::Layout(format!(
                    "primitive {k}: vertices {first}+{n} of {count}"
                )));
            }
            let (fi, ni) = (
                row.first_index as usize,
                (row.triangles as usize).saturating_mul(3),
            );
            let idx = self.indices.get(fi..fi.saturating_add(ni)).ok_or_else(|| {
                Error::Layout(format!(
                    "primitive {k}: indices {fi}+{ni} of {}",
                    self.indices.len()
                ))
            })?;
            let mut indices = Vec::with_capacity(ni);
            for &i in idx {
                let i = i as usize;
                let local = i.checked_sub(first).filter(|&x| x < n).ok_or_else(|| {
                    Error::Layout(format!("primitive {k}: index {i} outside vertices {first}+{n}"))
                })?;
                indices.push(local as u32);
            }
            let vert = |i: usize| {
                self.vertices
                    .get((first + i) * self.stride..(first + i + 1) * self.stride)
                    .unwrap_or_default()
            };
            let f32x = |b: &[u8], at: usize| {
                f32::from_le_bytes(
                    b.get(at..at + 4)
                        .and_then(|s| s.try_into().ok())
                        .unwrap_or([0; 4]),
                )
            };
            let unorm = |b: &[u8], at: usize| -> [f32; 3] {
                std::array::from_fn(|c| (f32::from(b.get(at + c).copied().unwrap_or(128)) - 127.5) / 127.5)
            };
            let norm = |v: [f32; 3]| {
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if l > 0.0 {
                    v.map(|x| x / l)
                } else {
                    [0.0, 0.0, 1.0]
                }
            };
            let mut streams = BTreeMap::new();
            streams.insert(
                Semantic::Position,
                Stream::F32x3(
                    (0..n)
                        .map(|i| {
                            let b = vert(i);
                            [f32x(b, 0), f32x(b, 4), f32x(b, 8)]
                        })
                        .collect(),
                ),
            );
            streams.insert(
                Semantic::Normal,
                Stream::F32x3((0..n).map(|i| norm(unorm(vert(i), 12))).collect()),
            );
            streams.insert(
                Semantic::TexCoord(0),
                Stream::F32x2(
                    (0..n)
                        .map(|i| {
                            let b = vert(i);
                            [f32x(b, l.uv0), f32x(b, l.uv0 + 4)]
                        })
                        .collect(),
                ),
            );
            if let Some(at) = l.uv1 {
                streams.insert(
                    Semantic::TexCoord(1),
                    Stream::F32x2(
                        (0..n)
                            .map(|i| {
                                let b = vert(i);
                                [f32x(b, at), f32x(b, at + 4)]
                            })
                            .collect(),
                    ),
                );
            }
            if let Some(at) = l.color {
                streams.insert(
                    Semantic::Color(0),
                    Stream::U8x4(
                        (0..n)
                            .map(|i| {
                                let b = vert(i);
                                std::array::from_fn(|c| b.get(at + c).copied().unwrap_or(255))
                            })
                            .collect(),
                    ),
                );
            }
            // a non-finite value means the layout read is not this mesh's: refused, named
            for (sem, st) in &streams {
                let finite = match st {
                    Stream::F32x2(v) => v.iter().flatten().all(|x| x.is_finite()),
                    Stream::F32x3(v) => v.iter().flatten().all(|x| x.is_finite()),
                    _ => true,
                };
                if !finite {
                    return Err(Error::Layout(format!(
                        "flags {:#x} with stride {}: {sem:?} not finite in the layout read",
                        self.flags, self.stride
                    )));
                }
            }
            out.push(Primitive {
                topology: Topology::Triangles,
                streams,
                indices: Some(indices),
                material: material(k),
            });
        }
        Ok(out)
    }
}
