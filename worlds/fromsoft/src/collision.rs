//! Sekiro collision metadata. The tagfile and mesh mechanism live in the toolkit;
//! fsnp material tables and source-frame conventions belong here.
use crate::pose::{pose, pose_columns, posed};
use crate::{Result, invalid};
use primase_havok::{Compendium, ConvexCore, CustomKind, Tagfile, Value};
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
    /// Triangle-data rows no shape key takes, at the end or among the others.
    pub unkeyed_triangle_data: usize,
    /// The shape classes of bodies [`read_map`] has no decoder for, one per body: listed, not read.
    pub unread_bodies: Vec<String>,
    /// Custom primitives whose data is not an extern shape reference ([`primase_havok::CustomKind::Unread`]): not read.
    pub unread_custom_primitives: usize,
}

/// Shape key to material, the count of all-ones (invalid) keys, and the count of triangle-data rows no key takes.
///
/// The key table is indexed like the triangle data, which may hold more rows than it: rows no key takes, at the end
/// (Elden Ring stores some tables shorter than their data) or among the others (one Sekiro castle mesh has 24 such rows
/// in its first 8,000). A row's vertex ids are unique within its primitive-data group, so a `(group, id)` names one
/// position: a key takes the next row whose ids agree with the positions its triangle and the rows taken before gave
/// them, skipping at most as many rows as the data has beyond the table. A key no such row fits is an error, never a
/// silent shift. A triangle whose key the table does not map has no material (-1) and is counted as unmapped.
fn material_keys(
    shape: Value<'_, '_>,
    triangles: &[primase_havok::Triangle],
) -> Result<(BTreeMap<u32, i32>, usize, usize)> {
    let parameter = shape.field("pParam")?.dereference()?;
    let triangle_data = parameter.field("triangleDataArray")?.array()?;
    let primitive_data = parameter.field("primitiveDataArray")?.array()?;
    let keys = shape.field("triangleIndexToShapeKey")?.array()?;
    let bits = shape.field("numShapeKeyBits")?.unsigned()?;
    // A mesh of no triangles has no key bits: its rows identify nothing, and are counted as no key's.
    if bits == 0 && triangles.is_empty() && keys.len() <= triangle_data.len() {
        return Ok((BTreeMap::new(), 0, triangle_data.len()));
    }
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
    let keyed = if triangle_data.is_empty() { 0 } else { keys.len() };
    let beyond = triangle_data.len() - keyed;
    let geometry: BTreeMap<u32, [[f32; 3]; 3]> = triangles.iter().map(|t| (t.key, t.vertices)).collect();
    let row = |i: usize| -> Result<(u64, [u64; 3])> {
        let r = triangle_data.get(i)?;
        Ok((
            r.field("primitiveDataIndex")?.unsigned()?,
            [
                r.field("vertexIndexA")?.unsigned()?,
                r.field("vertexIndexB")?.unsigned()?,
                r.field("vertexIndexC")?.unsigned()?,
            ],
        ))
    };
    // The position each (group, vertex id) has been given, as f32 bits.
    let mut positions: BTreeMap<(u64, u64), [u32; 3]> = BTreeMap::new();
    let fits =
        |positions: &BTreeMap<(u64, u64), [u32; 3]>, (group, ids): (u64, [u64; 3]), at: &[[f32; 3]; 3]| {
            ids.iter().zip(at).all(|(&id, p)| {
                positions
                    .get(&(group, id))
                    .is_none_or(|known| *known == p.map(f32::to_bits))
            })
        };
    let mut result = BTreeMap::new();
    let mut invalid_keys = 0;
    let mut skipped = 0;
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
        let mut r = row(i + skipped)?;
        if let Some(at) = geometry.get(&key) {
            while !fits(&positions, r, at) {
                if skipped == beyond {
                    return Err(invalid(
                        "collision",
                        shape.offset,
                        format!(
                            "key {key:x}: no triangle-data row from {i} on has vertex ids that agree with its triangle"
                        ),
                    ));
                }
                skipped += 1;
                r = row(i + skipped)?;
            }
            let (group, ids) = r;
            for (&id, p) in ids.iter().zip(at) {
                positions.entry((group, id)).or_insert(p.map(f32::to_bits));
            }
        }
        let material = primitive_data
            .get(r.0 as usize)?
            .field("materialNameData")?
            .unsigned()?;
        let material = i32::try_from(material)
            .map_err(|_| invalid("collision", shape.offset, "material exceeds i32"))?;
        if result.insert(key, material).is_some_and(|m| m != material) {
            return Err(invalid(
                "collision",
                shape.offset,
                format!(
                    "conflicting shape-key materials: key {key:x}, row {}, new {material}",
                    i + skipped
                ),
            ));
        }
    }
    Ok((result, invalid_keys, beyond))
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
                if mesh.custom.iter().any(|c| c.kind != CustomKind::Unread) {
                    return Err(invalid(
                        "collision",
                        shape.offset,
                        "a map mesh referencing extern shapes is not read",
                    ));
                }
                soup.unread_custom_primitives += mesh.custom.len();
                let (keys, invalid_keys, unkeyed) = material_keys(shape, &mesh.triangles)?;
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
    /// A convex shape (box, polytope, capsule, cylinder, plain convex, triangle, sphere): its stored core and convex radius in its own space,
    /// a capsule's stored segment, the shape's placement in the body's space where a compressed mesh holds it as an
    /// extern shape (`instance`, columns: rotation x, y and z, then the translation), and the body's pose in the file
    /// frame. It stores no material.
    Convex {
        core: ConvexCore,
        /// A capsule's segment end points `a`, `b` (its radius is the core's).
        capsule: Option<[[f32; 3]; 2]>,
        instance: Option<[[f32; 3]; 4]>,
        position: [f32; 3],
        orientation: [f32; 4],
    },
}

/// The convex shape classes a body or an extern shape instance may be, read as their core ([`ConvexCore::read`]).
const CONVEX_SHAPES: [&str; 7] = [
    "hknpBoxShape",
    "hknpConvexPolytopeShape",
    "hknpCapsuleShape",
    "hknpCylinderShape",
    "hknpConvexShape",
    "hknpTriangleShape",
    "hknpSphereShape",
];

/// The affine map (columns) from a convex shape's space to the file frame: its extern instance's transform, if any,
/// then the body's pose; composed in f64.
pub fn shape_to_file(
    instance: Option<&[[f32; 3]; 4]>,
    position: [f32; 3],
    orientation: [f32; 4],
) -> [[f32; 3]; 4] {
    let body = pose_columns(position, orientation);
    match instance {
        None => body,
        Some(inner) => {
            let wide = |m: &[[f32; 3]; 4]| m.map(|c| c.map(f64::from));
            crate::pose::compose(&wide(&body), &wide(inner)).map(|c| c.map(|x| x as f32))
        }
    }
}

/// A convex shape's geometry: its core, a capsule's segment, its extern `instance` placement and the body's pose.
fn convex(
    shape: Value<'_, '_>,
    p: [f32; 4],
    q: [f32; 4],
    instance: Option<[[f32; 3]; 4]>,
) -> Result<BodyGeometry> {
    if !CONVEX_SHAPES.contains(&shape.type_name()) {
        return Err(invalid(
            "collision",
            shape.offset,
            format!("unsupported extern shape {}", shape.type_name()),
        ));
    }
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
    Ok(BodyGeometry::Convex {
        core: ConvexCore::read(shape)?,
        capsule,
        instance,
        position: [p[0], p[1], p[2]],
        orientation: q,
    })
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
                instance,
                position,
                orientation,
                ..
            } => {
                let pose = shape_to_file(instance.as_ref(), *position, *orientation);
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
                let name = body.field("name")?.string()?;
                let motion_type = body.field("motionType")?.unsigned()?;
                let mut add = |shape: Value<'_, '_>, geometry| {
                    result.push(Body {
                        name: name.clone(),
                        motion_type,
                        shape: shape.type_name().into(),
                        geometry,
                    })
                };
                match shape.type_name() {
                    "fsnpCustomParamCompressedMeshShape" | "hknpCompressedMeshShape" => {
                        let mesh = primase_havok::compressed_mesh(shape.field("data")?.dereference()?)?;
                        let instances = primase_havok::extern_shapes(shape, &mesh)?;
                        let (keys, invalid_keys, unkeyed) =
                            if shape.type_name() == "fsnpCustomParamCompressedMeshShape" {
                                material_keys(shape, &mesh.triangles)?
                            } else {
                                (BTreeMap::new(), 0, 0)
                            };
                        let mut soup = Soup {
                            invalid_material_keys: invalid_keys,
                            unkeyed_triangle_data: unkeyed,
                            dead_primitives: mesh.dead_primitives,
                            unread_custom_primitives: mesh
                                .custom
                                .iter()
                                .filter(|c| c.kind == CustomKind::Unread)
                                .count(),
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
                        // a mesh made of extern shapes only is those shapes, not an empty mesh besides them
                        if !soup.triangles.is_empty() || instances.is_empty() {
                            add(shape, BodyGeometry::Mesh(soup));
                        }
                        for instance in instances {
                            add(
                                instance.shape,
                                convex(instance.shape, p, q, Some(instance.transform))?,
                            );
                        }
                    }
                    kind if CONVEX_SHAPES.contains(&kind) => add(shape, convex(shape, p, q, None)?),
                    other => {
                        return Err(invalid(
                            "collision",
                            shape.offset,
                            format!("unsupported object shape {other}"),
                        ));
                    }
                }
            }
        }
    }
    Ok(result)
}
