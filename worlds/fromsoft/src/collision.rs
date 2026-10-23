//! Sekiro collision metadata. The tagfile and mesh mechanism live in the toolkit;
//! fsnp material tables and source-frame conventions belong here.
use crate::pose::{pose, pose_columns, posed};
use crate::{Result, invalid};
use primase_havok::{Compendium, ConvexCore, Tagfile, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Soup {
    /// Triangle-list positions in the file's source frame, after rigid-body pose.
    pub triangles: Vec<[[f32; 3]; 3]>,
    /// HitMtrlParam IDs; -1 is an explicitly counted absent shape-key binding.
    pub materials: Vec<i32>,
    pub unmapped: usize,
    pub dead_primitives: usize,
    pub invalid_material_keys: usize,
    /// Triangle-data rows beyond the shape-key table (Elden Ring stores some tables shorter than their data).
    pub unkeyed_triangle_data: usize,
    /// The shape classes of bodies [`read_map`] has no decoder for, one per body: listed, not read.
    pub unread_bodies: Vec<String>,
}

/// Shape key to material, the count of all-ones (invalid) keys, and the count of triangle-data rows beyond the key
/// table. The key table is indexed like the triangle data and may be shorter; a triangle whose key the table does
/// not map has no material (-1) and is counted as unmapped.
fn material_keys(shape: Value<'_, '_>) -> Result<(BTreeMap<u32, i32>, usize, usize)> {
    let parameter = shape.field("pParam")?.dereference()?;
    let triangle_data = parameter.field("triangleDataArray")?.array()?;
    let primitive_data = parameter.field("primitiveDataArray")?.array()?;
    let keys = shape.field("triangleIndexToShapeKey")?.array()?;
    let bits = shape.field("numShapeKeyBits")?.unsigned()?;
    if !(1..=32).contains(&bits) || (!triangle_data.is_empty() && keys.len() > triangle_data.len()) {
        return Err(invalid(
            "collision",
            shape.offset,
            format!(
                "invalid shape-key table bits={bits} keys={} data={}",
                keys.len(),
                triangle_data.len()
            ),
        ));
    }
    let mut result = BTreeMap::new();
    let mut invalid_keys = 0;
    let keyed = if triangle_data.is_empty() { 0 } else { keys.len() };
    for i in 0..keyed {
        let raw = u32::try_from(keys.get(i)?.unsigned()?)
            .map_err(|_| invalid("collision", shape.offset, "shape key exceeds u32"))?;
        // Invalid shape-key rows do not identify any mesh primitive.
        // Preserve their count instead of overwriting a fictitious key.
        if raw == u32::MAX {
            invalid_keys += 1;
            continue;
        }
        let key = raw >> (32 - bits);
        let pi = triangle_data.get(i)?.field("primitiveDataIndex")?.unsigned()? as usize;
        let material = primitive_data.get(pi)?.field("materialNameData")?.unsigned()?;
        let material = i32::try_from(material)
            .map_err(|_| invalid("collision", shape.offset, "material exceeds i32"))?;
        if result.insert(key, material).is_some_and(|m| m != material) {
            return Err(invalid(
                "collision",
                shape.offset,
                format!("conflicting shape-key materials: key {key:x}, row {i}, new {material}"),
            ));
        }
    }
    Ok((result, invalid_keys, triangle_data.len() - keyed))
}

/// Decode map compressed meshes. A body of another shape class is not read: its
/// class is listed in [`Soup::unread_bodies`], never taken for an empty body.
pub fn read_map(bytes: &[u8], compendium: Option<&Compendium>) -> Result<Soup> {
    let tag = Tagfile::read(bytes, compendium)?;
    let variants = tag.root()?.field("namedVariants")?.array()?;
    let mut soup = Soup::default();
    for i in 0..variants.len() {
        let systems = variants
            .get(i)?
            .field("variant")?
            .dereference()?
            .field("systemDatas")?
            .array()?;
        for j in 0..systems.len() {
            let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
            for k in 0..bodies.len() {
                let body = bodies.get(k)?;
                let p = body.field("position")?.vector::<4>()?;
                let q = body.field("orientation")?.vector::<4>()?;
                if p[..3].iter().chain(q.iter()).any(|f| !f.is_finite()) {
                    return Err(invalid("collision", body.offset, "nonfinite body transform"));
                }
                let shape = body.field("shape")?.dereference()?;
                if shape.type_name() != "fsnpCustomParamCompressedMeshShape" {
                    soup.unread_bodies.push(shape.type_name().to_owned());
                    continue;
                }
                let mesh = primase_havok::compressed_mesh(shape.field("data")?.dereference()?)?;
                let (keys, invalid_keys, unkeyed) = material_keys(shape)?;
                soup.invalid_material_keys += invalid_keys;
                soup.unkeyed_triangle_data += unkeyed;
                soup.dead_primitives += mesh.dead_primitives;
                for triangle in mesh.triangles {
                    let mat = keys.get(&triangle.key).copied().unwrap_or(-1);
                    soup.unmapped += usize::from(mat == -1);
                    soup.materials.push(mat);
                    let vertices = triangle.vertices.map(|v| pose(v, p, q));
                    if vertices.iter().flatten().any(|f| !f.is_finite()) {
                        return Err(invalid(
                            "collision",
                            body.offset,
                            "nonfinite transformed position",
                        ));
                    }
                    soup.triangles.push(vertices);
                }
            }
        }
    }
    Ok(soup)
}
/// A body's geometry as its file stores it.
#[derive(Clone, Debug, PartialEq)]
pub enum BodyGeometry {
    /// A compressed mesh: its triangles posed by the body, in the file frame, and their HitMtrl IDs.
    Mesh(Soup),
    /// A convex shape (box, polytope, capsule): its stored core and convex radius in the body's own space, a capsule's
    /// stored segment, and the body's pose in the file frame. It stores no material.
    Convex {
        core: ConvexCore,
        /// A capsule's segment end points `a`, `b` (its radius is the core's).
        capsule: Option<[[f32; 3]; 2]>,
        position: [f32; 3],
        orientation: [f32; 4],
    },
}

impl BodyGeometry {
    /// Triangles in the file frame: a mesh's as stored; a convex shape's core tessellated as the intersection of its
    /// planes offset by the radius ([`ConvexCore::expanded_triangles`]: sharp where the shape is rounded, a capsule a
    /// box around its core), posed by the body. The convex tessellation is an approximation.
    pub fn triangles(&self) -> Result<Vec<[[f32; 3]; 3]>> {
        match self {
            BodyGeometry::Mesh(soup) => Ok(soup.triangles.clone()),
            BodyGeometry::Convex {
                core,
                position,
                orientation,
                ..
            } => {
                let pose = pose_columns(*position, *orientation);
                Ok(core
                    .expanded_triangles()?
                    .into_iter()
                    .map(|t| t.map(|v| posed(&pose, v)))
                    .collect())
            }
        }
    }
}

#[derive(Debug)]
pub struct Body {
    pub name: String,
    /// Stored motion classification; interpreting 2 as keyframed is policy.
    pub motion_type: u64,
    pub shape: String,
    pub geometry: BodyGeometry,
}

/// Object bodies as their file stores them: compressed meshes as posed triangles in the file frame with their
/// materials (-1 where the shape key binds none; a plain compressed mesh binds none), convex shapes as their stored
/// core, radius and pose ([`BodyGeometry`]). Unsupported geometry is an error, never an empty successful body.
pub fn read_bodies(bytes: &[u8], compendium: Option<&Compendium>) -> Result<Vec<Body>> {
    let tag = Tagfile::read(bytes, compendium)?;
    let variants = tag.root()?.field("namedVariants")?.array()?;
    let mut result = Vec::new();
    for i in 0..variants.len() {
        let variant = variants.get(i)?.field("variant")?.dereference()?;
        if variant.type_name() != "hknpPhysicsSceneData" {
            continue;
        }
        let systems = variant.field("systemDatas")?.array()?;
        for j in 0..systems.len() {
            let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
            for k in 0..bodies.len() {
                let body = bodies.get(k)?;
                let p = body.field("position")?.vector::<4>()?;
                let q = body.field("orientation")?.vector::<4>()?;
                if p[..3].iter().chain(q.iter()).any(|f| !f.is_finite()) {
                    return Err(invalid("collision", body.offset, "nonfinite body transform"));
                }
                let shape = body.field("shape")?.dereference()?;
                let geometry = match shape.type_name() {
                    "fsnpCustomParamCompressedMeshShape" | "hknpCompressedMeshShape" => {
                        let mesh = primase_havok::compressed_mesh(shape.field("data")?.dereference()?)?;
                        let (keys, invalid_keys, unkeyed) =
                            if shape.type_name() == "fsnpCustomParamCompressedMeshShape" {
                                material_keys(shape)?
                            } else {
                                (BTreeMap::new(), 0, 0)
                            };
                        let mut soup = Soup {
                            invalid_material_keys: invalid_keys,
                            unkeyed_triangle_data: unkeyed,
                            dead_primitives: mesh.dead_primitives,
                            ..Soup::default()
                        };
                        for tri in mesh.triangles {
                            let material = keys.get(&tri.key).copied().unwrap_or(-1);
                            soup.unmapped += usize::from(material == -1);
                            soup.materials.push(material);
                            let vertices = tri.vertices.map(|v| pose(v, p, q));
                            if vertices.iter().flatten().any(|f| !f.is_finite()) {
                                return Err(invalid(
                                    "collision",
                                    body.offset,
                                    "nonfinite transformed position",
                                ));
                            }
                            soup.triangles.push(vertices);
                        }
                        BodyGeometry::Mesh(soup)
                    }
                    "hknpBoxShape" | "hknpConvexPolytopeShape" | "hknpCapsuleShape" | "hknpCylinderShape" => {
                        let capsule = if shape.type_name() == "hknpCapsuleShape" {
                            let end = |name: &str| -> Result<[f32; 3]> {
                                let v = shape.field(name)?.vector::<4>()?;
                                if v[..3].iter().all(|f| f.is_finite()) {
                                    Ok([v[0], v[1], v[2]])
                                } else {
                                    Err(invalid("collision", shape.offset, "nonfinite capsule segment"))
                                }
                            };
                            Some([end("a")?, end("b")?])
                        } else {
                            None
                        };
                        BodyGeometry::Convex {
                            core: primase_havok::ConvexCore::read(shape)?,
                            capsule,
                            position: [p[0], p[1], p[2]],
                            orientation: q,
                        }
                    }
                    other => {
                        return Err(invalid(
                            "collision",
                            shape.offset,
                            format!("unsupported object shape {other}"),
                        ));
                    }
                };
                result.push(Body {
                    name: body.field("name")?.string()?,
                    motion_type: body.field("motionType")?.unsigned()?,
                    shape: shape.type_name().into(),
                    geometry,
                });
            }
        }
    }
    Ok(result)
}
