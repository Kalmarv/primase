use super::Model;
use crate::{Result, invalid};
use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Primitive, Semantic, Stream, Topology};
use primase_meshops::{Mesh, compact_primitive, vector::Vec3};
use std::collections::BTreeMap;

const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

/// Geometry-affecting material decisions, supplied by the material resolver.
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
#[derive(Clone, Debug)]
pub struct Part {
    pub source_mesh: usize,
    pub material: usize,
    pub mesh: Mesh,
    pub winding_flipped: bool,
    /// Stored tangent frame after coordinate conversion, in compacted vertex order.
    pub tangents: Vec<[f32; 4]>,
}

impl Model {
    /// LOD0 geometry in the world frame. Compact before dropping zero-area faces;
    /// determine each part's winding from its stored normals. Layer baking is not
    /// performed here: it changes part boundaries and texture coordinates.
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
            let Some(f) = m.faces.iter().find(|f| f.is_lod0()) else {
                continue;
            };
            let mut indices = f.triangulate();
            indices.truncate(indices.len() / 3 * 3);
            if indices.is_empty() {
                continue;
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
                let tangents =
                    m.vertices
                        .iter()
                        .map(|v| {
                            v.tangents.first().copied().ok_or_else(|| {
                                invalid("FLVER geometry", source_mesh, "partial tangent stream")
                            })
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
            for uv in &mut mesh.uv {
                uv[0] *= mat.uv_scale[0];
                uv[1] *= mat.uv_scale[1];
            }
            let mut kept = Vec::new();
            let mut score = 0f64;
            for t in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = [
                    mesh.positions[t[0] as usize],
                    mesh.positions[t[1] as usize],
                    mesh.positions[t[2] as usize],
                ];
                let ab = Vec3::from(b) - Vec3::from(a);
                let ac = Vec3::from(c) - Vec3::from(a);
                let cross = ab.cross(ac);
                let area_squared = cross.length_squared();
                if !area_squared.is_finite() {
                    return Err(invalid("FLVER geometry", source_mesh, "non-finite face area"));
                }
                if area_squared <= 1e-14 {
                    continue;
                }
                let normal = std::array::from_fn(|i| {
                    mesh.normals[t[0] as usize][i]
                        + mesh.normals[t[1] as usize][i]
                        + mesh.normals[t[2] as usize][i]
                });
                score += f64::from(cross.dot(Vec3::from(normal)));
                kept.extend_from_slice(t);
            }
            if !score.is_finite() {
                return Err(invalid("FLVER geometry", source_mesh, "non-finite winding score"));
            }
            if kept.is_empty() {
                continue;
            }
            if score < 0. {
                for t in kept.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            mesh.indices = kept;
            mesh.validate()
                .map_err(|e| invalid("FLVER geometry", source_mesh, e))?;
            parts.push(Part {
                source_mesh,
                material: m.material,
                mesh,
                winding_flipped: score < 0.,
                tangents,
            });
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
