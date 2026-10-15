use super::Model;
use crate::{Result, invalid};
use primase_core::frame::{Axis, SourceFrame};
use primase_meshops::{Mesh, compact, vector::Vec3};

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
            let raw = Mesh {
                positions: m.vertices.iter().map(|v| v.position).collect(),
                normals: m.vertices.iter().map(|v| v.normal).collect(),
                uv: m
                    .vertices
                    .iter()
                    .map(|v| v.uvs.first().map_or([0.; 2], |uv| [uv[0], uv[1]]))
                    .collect(),
                indices,
            };
            let mut mesh = compact(&raw).map_err(|e| invalid("FLVER geometry", source_mesh, e))?;
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
            });
        }
        Ok(parts)
    }
}
