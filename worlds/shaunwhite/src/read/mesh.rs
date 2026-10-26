//! A Mesh object's compiled geometry: the CompiledMesh it holds, read into core primitives.
//!
//! A Mesh object's body holds its CompiledMesh inline: the CompiledMesh class (`0xFC9E1595`, CRC-32 of
//! `CompiledMesh`), its Data array's length (u32), then the Data (little-endian):
//! - a header of 12 u32: 16, the number `m` of further vertex streams (0 in 6,231 of the 6,301 distinct Meshes of the
//!   game's cells; 1 in 57, 2 in 12, 3 in one), the vertex stride, the vertex buffer's bytes, the stride times
//!   `m + 1`, the vertex bytes times `m + 1`, the index buffer's bytes, 0, the vertex flags, the standard primitive
//!   count, the shadow primitive count, and a size (the buffers' and the primitive rows' bytes);
//! - the vertex buffer (`m + 1` of them back to back, each `vertex bytes` long), the index buffer (u16, indexing the
//!   whole vertex buffer, the same for every stream), then a 24-byte row per primitive,
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
//!
//! **Further streams** (`m > 0`) are further states of the same vertices: each stream is a whole vertex of the one
//! layout (the first stream's), the same count, with its own positions and normals (and in 30 of the 84 further streams
//! its own texture coordinates or colours; the other bytes equal the first stream's). The first stream is the mesh as
//! drawn; the others are shapes it can be morphed to: the meshes are the gameplay "influence" shapes (`InfGP_Vert4m`,
//! `Walling10x2m`, `PoolMorph`, `CableJunctionMorph`, `FFGround_10mX10mPlatform`), whose names say a shape changes.
//! The flags bit 4 (a second texture-coordinate set in a one-stream mesh) adds nothing to the stride of 24 of these
//! meshes: the strides are 24 and 28, the position, normal, colour (flag 64) and one set of texture coordinates and no
//! more, so that is the layout of a mesh with further streams, and any other stride or flag is refused.
//! [`CompiledMesh::morph_primitives`] decodes the further streams like the first.
//!
//! A vertex whose texture coordinates are both stored as NaN (38 of `GFX_InfGP_Bump8m`'s 502, every one with colour
//! alpha 0) gets (0, 0) and is counted ([`Primitives::zeroed`]): the stored value has no meaning, and a core stream is
//! finite. A non-finite position or normal still refuses the mesh.

use std::collections::BTreeMap;

use primase_bytes::{Field, FieldError, View};
use primase_core::morphs::Deltas;
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
    /// The vertex buffers of the further streams, each as long as `vertices`.
    pub morphs: Vec<Vec<u8>>,
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

fn layout(flags: u32, stride: usize, morphs: bool) -> Option<Layout> {
    if morphs {
        // position, normal, colour (flag 64) and one set of texture coordinates; bit 4 adds nothing here
        let color = flags & 64 != 0;
        if flags & !(64 | 4) != 0 || stride != 16 + usize::from(color) * 4 + 8 {
            return None;
        }
        return Some(Layout {
            tangent: None,
            color: color.then_some(16),
            uv0: 16 + usize::from(color) * 4,
            uv1: None,
        });
    }
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
        let extra = word(1)? as usize;
        let streams = extra.saturating_add(1);
        if word(0)? != 16
            || extra > 3
            || word(4)? as usize != stride.saturating_mul(streams)
            || word(5)? as usize != vertex_bytes.saturating_mul(streams)
        {
            return Err(bad(format!(
                "a header of words {:?}: not a layout read (16, 0 to 3 further streams, the totals)",
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
        let morphs = (1..streams)
            .map(|k| {
                Ok(
                    v.slice(vat.saturating_add(k.saturating_mul(vertex_bytes)), vertex_bytes)
                        .field("CompiledMesh further stream")?
                        .to_vec(),
                )
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let iat = vat.saturating_add(vertex_bytes.saturating_mul(streams));
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
            morphs,
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
    ) -> Result<Primitives, Error> {
        self.decode(&self.vertices, material)
    }

    /// The primitives of each further stream, decoded like [`CompiledMesh::primitives`] (the same rows, indices and
    /// count of vertices), in the order of the streams.
    pub fn morph_primitives(&self) -> Result<Vec<Primitives>, Error> {
        self.morphs.iter().map(|v| self.decode(v, |_| None)).collect()
    }

    fn decode(
        &self,
        vertices: &[u8],
        material: impl Fn(usize) -> Option<primase_core::Entity>,
    ) -> Result<Primitives, Error> {
        let l = layout(self.flags, self.stride, !self.morphs.is_empty()).ok_or_else(|| {
            Error::Layout(format!(
                "flags {:#x} with stride {}: no vertex layout read",
                self.flags, self.stride
            ))
        })?;
        let mut zeroed = 0usize;
        let count = vertices.len() / self.stride;
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
                vertices
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
            // a vertex whose texture coordinates are both stored NaN gets (0, 0), counted; any other non-finite value
            // means the layout read is not this mesh's: refused, named
            for sem in [Semantic::TexCoord(0), Semantic::TexCoord(1)] {
                if let Some(Stream::F32x2(v)) = streams.get_mut(&sem) {
                    for uv in v.iter_mut().filter(|uv| uv.iter().all(|x| x.is_nan())) {
                        *uv = [0.0; 2];
                        zeroed += 1;
                    }
                }
            }
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
        Ok(Primitives { list: out, zeroed })
    }
}

/// What a further stream changes: its position and normal deltas against the first stream's, per primitive (`None` where
/// a primitive does not move), and whether any texture coordinate or colour differs (not carried: core's deltas hold
/// positions, normals and tangents).
#[derive(Clone, Debug, PartialEq)]
pub struct StreamDeltas {
    pub primitives: Vec<Option<Deltas>>,
    pub other_differs: bool,
}

/// The deltas of `state` against `base` (the same rows of the same mesh, in the same frame).
pub fn deltas(base: &[Primitive], state: &[Primitive]) -> StreamDeltas {
    let mut other_differs = false;
    let primitives = base
        .iter()
        .zip(state)
        .map(|(b, s)| {
            for sem in [Semantic::TexCoord(0), Semantic::TexCoord(1), Semantic::Color(0)] {
                other_differs |= b.streams.get(&sem) != s.streams.get(&sem);
            }
            let diff = |sem: Semantic| match (b.streams.get(&sem), s.streams.get(&sem)) {
                (Some(Stream::F32x3(a)), Some(Stream::F32x3(c))) if a != c => Some(
                    a.iter()
                        .zip(c)
                        .map(|(a, c)| [c[0] - a[0], c[1] - a[1], c[2] - a[2]])
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            };
            let (positions, normals) = (diff(Semantic::Position), diff(Semantic::Normal));
            (positions.is_some() || normals.is_some()).then_some(Deltas {
                indices: None,
                positions,
                normals,
                tangents: None,
            })
        })
        .collect();
    StreamDeltas {
        primitives,
        other_differs,
    }
}

/// The primitives of one vertex stream and how many vertices had their NaN texture coordinates zeroed.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitives {
    pub list: Vec<Primitive>,
    pub zeroed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vertex of stride 24 (position, normal, one set of texture coordinates) or 28 (and a colour at 16).
    fn vertex(pos: [f32; 3], uv: [f32; 2], color: Option<[u8; 4]>) -> Vec<u8> {
        let mut v = Vec::new();
        pos.iter().for_each(|x| v.extend(x.to_le_bytes()));
        v.extend([128, 128, 255, 0]);
        v.extend(
            color
                .unwrap_or_default()
                .iter()
                .take(usize::from(color.is_some()) * 4),
        );
        uv.iter().for_each(|x| v.extend(x.to_le_bytes()));
        v
    }

    /// A CompiledMesh Data of one triangle: the streams (each a list of vertices), the flags and the stride.
    fn data(streams: &[Vec<Vec<u8>>], flags: u32, extra_word: Option<u32>) -> Vec<u8> {
        let stride = streams[0][0].len();
        let vb = streams[0].len() * stride;
        let m = streams.len() - 1;
        let words = [
            16,
            extra_word.unwrap_or(m as u32),
            stride as u32,
            vb as u32,
            (stride * (m + 1)) as u32,
            (vb * (m + 1)) as u32,
            6,
            0,
            flags,
            1,
            0,
            0,
        ];
        let mut d: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        for s in streams {
            s.iter().for_each(|v| d.extend(v));
        }
        [0u16, 1, 2].iter().for_each(|i| d.extend(i.to_le_bytes()));
        // one row: a triangle list over the three vertices, one triangle
        [4u32, 0, 3, 0, 1, 0]
            .iter()
            .for_each(|w| d.extend(w.to_le_bytes()));
        d
    }

    fn tri(z: f32, uv_x: f32, color: Option<[u8; 4]>) -> Vec<Vec<u8>> {
        (0..3)
            .map(|i| vertex([i as f32, 0.0, z], [uv_x + i as f32, 0.5], color))
            .collect()
    }

    /// Further streams are read as further states of the same vertices: the first stream draws, the second is
    /// decoded like it, the bit 4 of the flags adds no second texture-coordinate set, and the deltas are the position
    /// change; a texture-coordinate change is reported, not carried.
    #[test]
    fn a_further_stream_is_a_morph_state() {
        let d = data(&[tri(0.0, 0.0, None), tri(2.0, 0.0, None)], 4, None);
        let m = CompiledMesh::parse(&d).unwrap();
        assert_eq!((m.stride, m.morphs.len()), (24, 1));
        let base = m.primitives(|_| None).unwrap();
        let p = &base.list[0];
        assert_eq!(p.positions().unwrap()[1], [1.0, 0.0, 0.0]);
        assert!(!p.streams.contains_key(&Semantic::TexCoord(1)));
        let states = m.morph_primitives().unwrap();
        let d = deltas(&base.list, &states[0].list);
        let z = d.primitives[0].as_ref().unwrap().positions.as_ref().unwrap();
        assert_eq!(z, &vec![[0.0, 0.0, 2.0]; 3]);
        assert!(!d.other_differs);

        let moved = data(&[tri(0.0, 0.0, None), tri(2.0, 1.0, None)], 4, None);
        let m = CompiledMesh::parse(&moved).unwrap();
        let base = m.primitives(|_| None).unwrap();
        let state = m.morph_primitives().unwrap();
        assert!(deltas(&base.list, &state[0].list).other_differs);
    }

    /// A colour (flag 64) sits at 16 and the texture coordinates after it.
    #[test]
    fn colour_and_texture_coordinates_follow_the_normal() {
        let c = Some([1, 2, 3, 4]);
        let d = data(&[tri(0.0, 0.0, c), tri(1.0, 0.0, c)], 64, None);
        let m = CompiledMesh::parse(&d).unwrap();
        let p = &m.primitives(|_| None).unwrap().list[0];
        assert_eq!(
            p.streams.get(&Semantic::Color(0)),
            Some(&Stream::U8x4(vec![[1, 2, 3, 4]; 3]))
        );
        assert_eq!(
            p.streams.get(&Semantic::TexCoord(0)),
            Some(&Stream::F32x2(vec![[0.0, 0.5], [1.0, 0.5], [2.0, 0.5]]))
        );
    }

    /// What no layout read gives is refused, named: more streams than seen, totals that disagree, a stride the
    /// morph layout does not have, a flag it does not have.
    #[test]
    fn what_the_header_does_not_hold_is_refused() {
        let ok = data(&[tri(0.0, 0.0, None), tri(1.0, 0.0, None)], 0, None);
        assert!(CompiledMesh::parse(&ok).is_ok());
        let err = |d: &[u8]| CompiledMesh::parse(d).unwrap_err().to_string();
        assert!(err(&data(&[tri(0.0, 0.0, None)], 0, Some(4))).contains("not a layout read"));
        let mut bad = ok.clone();
        bad[16..20].copy_from_slice(&47u32.to_le_bytes());
        assert!(err(&bad).contains("not a layout read"));
        // a stride of 28 without the colour flag, and an unknown flag, have no morph layout
        let m = CompiledMesh::parse(&data(
            &[tri(0.0, 0.0, Some([0; 4])), tri(1.0, 0.0, Some([0; 4]))],
            0,
            None,
        ))
        .unwrap();
        assert!(
            m.primitives(|_| None)
                .unwrap_err()
                .to_string()
                .contains("no vertex layout")
        );
        let m = CompiledMesh::parse(&data(&[tri(0.0, 0.0, None), tri(1.0, 0.0, None)], 2, None)).unwrap();
        assert!(
            m.primitives(|_| None)
                .unwrap_err()
                .to_string()
                .contains("no vertex layout")
        );
    }

    /// Texture coordinates stored NaN in both components become (0, 0) and are counted; a NaN position refuses.
    #[test]
    fn nan_texture_coordinates_are_zeroed_and_counted() {
        let mut t = tri(0.0, 0.0, None);
        t[1] = vertex([1.0, 0.0, 0.0], [f32::NAN; 2], None);
        let m = CompiledMesh::parse(&data(&[t.clone()], 0, None)).unwrap();
        let read = m.primitives(|_| None).unwrap();
        assert_eq!(read.zeroed, 1);
        assert_eq!(
            read.list[0].streams.get(&Semantic::TexCoord(0)),
            Some(&Stream::F32x2(vec![[0.0, 0.5], [0.0, 0.0], [2.0, 0.5]]))
        );
        t[2] = vertex([f32::NAN, 0.0, 0.0], [0.0; 2], None);
        let m = CompiledMesh::parse(&data(&[t], 0, None)).unwrap();
        assert!(
            m.primitives(|_| None)
                .unwrap_err()
                .to_string()
                .contains("not finite")
        );
    }
}
