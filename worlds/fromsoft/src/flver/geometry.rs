use super::Model;
use crate::{Result, invalid};
use primase_core::{Primitive, Semantic, Stream, Topology};
use primase_meshops::{Mesh, compact_primitive, vector::Vec3};
use std::collections::BTreeMap;

use crate::pose::FRAME;

/// What a face set is for, from its flags: a level of detail (0 the finest) or the motion-blur variant of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FaceSetKind {
    Lod(u8),
    MotionBlur(u8),
}

impl super::FaceSet {
    /// Bits 24 to 26 of the flags are the level of detail (`0x0100_0000` level 1 ... `0x0400_0000` level 4),
    /// `0x8000_0000` the motion-blur variant; edge compression is refused when the model is read.
    pub fn kind(&self) -> FaceSetKind {
        let level = ((self.flags >> 24) & 7) as u8;
        if self.flags & 0x8000_0000 != 0 {
            FaceSetKind::MotionBlur(level)
        } else {
            FaceSetKind::Lod(level)
        }
    }
}

/// One face set of one mesh, decoded: the vertices its triangles use, compacted in first-use order, in the world
/// frame (positions and directions mirrored in Z, normals unit length, tangent handedness reflected with the frame),
/// UV0 as stored. Triangles in stored order and winding; strips triangulated with the file's alternation.
#[derive(Clone, Debug)]
pub struct Part {
    pub source_mesh: usize,
    pub face_set: usize,
    pub material: usize,
    pub mesh: Mesh,
    /// Stored tangent frame after coordinate conversion, in compacted vertex order; empty without tangents.
    pub tangents: Vec<[f32; 4]>,
    /// Original vertex indices in compacted order, for retained source attributes.
    pub source_vertices: Vec<u32>,
    /// Whether [`Part::repair_winding`] reversed it.
    pub winding_flipped: bool,
}

impl Model {
    /// Decode face set `face_set` of mesh `source_mesh` ([`Part`]): no triangle is dropped or reordered, and no
    /// material decision applies. `None` when it has no whole triangle.
    pub fn decode(&self, source_mesh: usize, face_set: usize) -> Result<Option<Part>> {
        if self.speedtree {
            // its positions and normals are members its game shaders interpret; nothing here reads them as such
            return Err(invalid(
                "FLVER geometry",
                source_mesh,
                "a SpeedTree model's vertices are its shader's members, not positions",
            ));
        }
        let m = self
            .meshes
            .get(source_mesh)
            .ok_or_else(|| invalid("FLVER geometry", source_mesh, "mesh index"))?;
        let f = m
            .faces
            .get(face_set)
            .ok_or_else(|| invalid("FLVER geometry", source_mesh, "face set index"))?;
        let mut indices = f.triangulate();
        indices.truncate(indices.len() / 3 * 3);
        if indices.is_empty() {
            return Ok(None);
        }
        let mut streams = BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(m.vertices.iter().map(|v| v.position).collect()),
            ),
            (
                Semantic::Normal,
                Stream::F32x3(m.vertices.iter().map(|v| v.normal).collect()),
            ),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(
                    m.vertices
                        .iter()
                        .map(|v| v.uvs.first().map_or([0.; 2], |uv| [uv[0], uv[1]]))
                        .collect(),
                ),
            ),
        ]);
        if m.vertices.iter().any(|v| !v.tangents.is_empty()) {
            let tangents = m
                .vertices
                .iter()
                .map(|v| {
                    v.tangents
                        .first()
                        .copied()
                        .ok_or_else(|| invalid("FLVER geometry", source_mesh, "partial tangent stream"))
                })
                .collect::<Result<Vec<_>>>()?;
            streams.insert(Semantic::Tangent, Stream::F32x4(tangents));
        }
        let compacted = compact_primitive(&Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(indices),
            material: None,
        })
        .map_err(|e| invalid("FLVER geometry", source_mesh, e))?;
        let mut streams = compacted.primitive.streams;
        let Some(Stream::F32x3(positions)) = streams.remove(&Semantic::Position) else {
            unreachable!()
        };
        let Some(Stream::F32x3(normals)) = streams.remove(&Semantic::Normal) else {
            unreachable!()
        };
        let Some(Stream::F32x2(uv)) = streams.remove(&Semantic::TexCoord(0)) else {
            unreachable!()
        };
        let mut tangents = match streams.remove(&Semantic::Tangent) {
            Some(Stream::F32x4(v)) => v,
            None => Vec::new(),
            _ => unreachable!(),
        };
        for t in &mut tangents {
            *t = tangent_to_scene(*t);
        }
        let mut mesh = Mesh {
            positions,
            normals,
            uv,
            indices: compacted.primitive.indices.expect("indexed primitive"),
        };
        for p in &mut mesh.positions {
            *p = FRAME.point(*p);
        }
        for n in &mut mesh.normals {
            *n = FRAME.direction(*n);
            *n = Vec3::from(*n).normalize_or(Vec3::UNIT_Y).into();
        }
        Ok(Some(Part {
            source_mesh,
            face_set,
            material: m.material,
            mesh,
            tangents,
            source_vertices: compacted.gather,
            winding_flipped: false,
        }))
    }
}

/// Normalize the stored tangent and reflect its handedness with the scene frame.
/// Unlike a UV-derived frame this retains the basis used by the game shaders.
pub fn tangent_to_scene(t: [f32; 4]) -> [f32; 4] {
    let direction: [f32; 3] = Vec3::from(FRAME.direction([t[0], t[1], t[2]])).normalize().into();
    [direction[0], direction[1], direction[2], -t[3]]
}
