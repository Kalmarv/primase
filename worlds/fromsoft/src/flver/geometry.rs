use super::Model;
use crate::{Result, invalid};
use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Primitive, Semantic, Stream, Topology};
use primase_meshops::{Mesh, compact_primitive, vector::Vec3};
use std::collections::BTreeMap;

const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

/// Geometry-affecting material decisions of the legacy composition ([`Model::geometry`]).
#[derive(Clone, Copy, Debug)]
pub struct MaterialGeometry {
    pub skip: bool,
    pub uv_scale: [f32; 2],
}
impl Default for MaterialGeometry {
    fn default() -> Self {
        Self {
            skip: false,
            uv_scale: [1.; 2],
        }
    }
}

/// What a face set is for, from its flags: a level of detail (0 the finest) or the motion-blur variant of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FaceSetKind {
    Lod(u8),
    MotionBlur(u8),
}

impl super::FaceSet {
    /// Flags `0x0100_0000` and `0x0200_0000` are levels 1 and 2, `0x8000_0000` the motion-blur variant; edge
    /// compression is refused when the model is read.
    pub fn kind(&self) -> FaceSetKind {
        let level = match self.flags & 0x0300_0000 {
            0 => 0,
            0x0100_0000 => 1,
            0x0200_0000 => 2,
            _ => 3,
        };
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

impl Part {
    /// Multiply UV0 by `scale` (a material's tiling, applied to the coordinates instead of the texture binding).
    pub fn apply_uv_scale(&mut self, scale: [f32; 2]) -> Result<()> {
        if !scale.iter().all(|f| f.is_finite()) {
            return Err(invalid("FLVER geometry", self.source_mesh, "non-finite UV scale"));
        }
        for uv in &mut self.mesh.uv {
            uv[0] *= scale[0];
            uv[1] *= scale[1];
        }
        Ok(())
    }

    /// The triangles' areas (squared, doubled) and their agreement with the stored normals, in triangle order.
    fn scores(&self) -> Result<Vec<(f32, f32)>> {
        let m = &self.mesh;
        m.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| Vec3::from(m.positions[i as usize]));
                let cross = (b - a).cross(c - a);
                let area_squared = cross.length_squared();
                if !area_squared.is_finite() {
                    return Err(invalid(
                        "FLVER geometry",
                        self.source_mesh,
                        "non-finite face area",
                    ));
                }
                let normal = std::array::from_fn(|i| {
                    m.normals[t[0] as usize][i] + m.normals[t[1] as usize][i] + m.normals[t[2] as usize][i]
                });
                Ok((area_squared, cross.dot(Vec3::from(normal))))
            })
            .collect()
    }

    /// Remove triangles whose doubled area squared is at most 1e-14 m^4; returns how many went. The vertices stay.
    pub fn drop_degenerates(&mut self) -> Result<usize> {
        let scores = self.scores()?;
        let mut kept = Vec::with_capacity(self.mesh.indices.len());
        for (t, (area_squared, _)) in self.mesh.indices.as_chunks::<3>().0.iter().zip(&scores) {
            if *area_squared > 1e-14 {
                kept.extend_from_slice(t);
            }
        }
        let dropped = scores.len() - kept.len() / 3;
        self.mesh.indices = kept;
        Ok(dropped)
    }

    /// Reverse every triangle when, over the triangles that are not degenerate, the stored normals point against
    /// the faces' winding on balance (an inferred repair: the file's own winding is the default).
    pub fn repair_winding(&mut self) -> Result<bool> {
        let mut score = 0f64;
        for (area_squared, agreement) in self.scores()? {
            if area_squared > 1e-14 {
                score += f64::from(agreement);
            }
        }
        if !score.is_finite() {
            return Err(invalid(
                "FLVER geometry",
                self.source_mesh,
                "non-finite winding score",
            ));
        }
        if score < 0. {
            for t in self.mesh.indices.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
            self.winding_flipped = true;
        }
        Ok(self.winding_flipped)
    }
}

impl Model {
    /// Decode face set `face_set` of mesh `source_mesh` ([`Part`]): no triangle is dropped or reordered, and no
    /// material decision applies. `None` when it has no whole triangle.
    pub fn decode(&self, source_mesh: usize, face_set: usize) -> Result<Option<Part>> {
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

    /// The legacy composition: each mesh's first level-0 face set, meshes whose material is skipped left out, the
    /// material's UV scale applied, degenerate triangles dropped, winding repaired from the stored normals, parts
    /// left without triangles omitted. The `lower-fromsoft-geometry` pass makes each of these choices explicit.
    pub fn geometry(&self, materials: &[MaterialGeometry]) -> Result<Vec<Part>> {
        if materials.len() != self.materials.len() {
            return Err(invalid("FLVER geometry", 0, "material count"));
        }
        let mut parts = Vec::new();
        for (source_mesh, m) in self.meshes.iter().enumerate() {
            let mat = materials
                .get(m.material)
                .ok_or_else(|| invalid("FLVER geometry", source_mesh, "material index"))?;
            if mat.skip {
                continue;
            }
            if !mat.uv_scale.iter().all(|f| f.is_finite()) {
                return Err(invalid("FLVER geometry", source_mesh, "non-finite UV scale"));
            }
            let Some(face_set) = m.faces.iter().position(|f| f.is_lod0()) else {
                continue;
            };
            let Some(mut part) = self.decode(source_mesh, face_set)? else {
                continue;
            };
            part.apply_uv_scale(mat.uv_scale)?;
            part.repair_winding()?;
            part.drop_degenerates()?;
            if part.mesh.indices.is_empty() {
                continue;
            }
            part.mesh
                .validate()
                .map_err(|e| invalid("FLVER geometry", source_mesh, e))?;
            parts.push(part);
        }
        Ok(parts)
    }
}

/// Normalize the stored tangent and reflect its handedness with the scene frame.
/// Unlike a UV-derived frame this retains the basis used by the game shaders.
pub fn tangent_to_scene(t: [f32; 4]) -> [f32; 4] {
    let direction: [f32; 3] = Vec3::from(FRAME.direction([t[0], t[1], t[2]])).normalize().into();
    [direction[0], direction[1], direction[2], -t[3]]
}
