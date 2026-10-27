//! Object collision as the objects store it: each object prototype's collision files (`fromsoft.object_collision`:
//! Sekiro's main HKX, Elden Ring's `_h` and `_l` asset files) with their resolution, meshes as their posed triangles,
//! convex shapes as their stored core and radius.
use crate::collision::BodyGeometry;
use crate::pose::FRAME;
use crate::sekiro::source::{ObjectBody, ObjectCollisionFile, ObjectGeometry, Resolution};
use crate::{Result, collision};
use std::collections::BTreeMap;

/// What the reader found of the kept object prototypes' collision.
#[derive(Debug, Default)]
pub struct Report {
    /// Object prototypes kept, and those whose binder holds their main HKX.
    pub models: usize,
    pub models_with_collision: usize,
    /// Their bodies by stored motion type, and by shape class.
    pub bodies_by_motion: BTreeMap<u64, usize>,
    pub bodies_by_shape: BTreeMap<String, usize>,
    /// Compressed-mesh custom primitives of an unknown kind ([`havok_formats::CustomKind::Unread`]): not read.
    pub unread_custom_primitives: usize,
}

/// An object's collision `file` (decoded `bytes`) of `resolution`: every body as stored, in the prototype's space.
pub fn bodies(
    file: &str,
    resolution: Resolution,
    bytes: &[u8],
    report: &mut Report,
) -> Result<ObjectCollisionFile> {
    let bodies = collision::read_bodies(bytes, None)?
        .into_iter()
        .map(|body| {
            *report.bodies_by_motion.entry(body.motion_type).or_default() += 1;
            *report.bodies_by_shape.entry(body.shape.clone()).or_default() += 1;
            let geometry = match body.geometry {
                BodyGeometry::Mesh(soup) => {
                    report.unread_custom_primitives += soup.unread_custom_primitives;
                    ObjectGeometry::Mesh {
                        triangles: soup
                            .triangles
                            .into_iter()
                            .map(|t| t.map(|v| FRAME.point(v)))
                            .collect(),
                        materials: soup.materials,
                    }
                }
                BodyGeometry::Convex {
                    core,
                    capsule,
                    instance,
                    position,
                    orientation,
                } => ObjectGeometry::Convex {
                    core,
                    capsule,
                    // an extern shape's placement and the body's pose, then the file frame's reflection into the
                    // world frame's axes
                    to_prototype: collision::shape_to_file(instance.as_ref(), position, orientation)
                        .map(|c| FRAME.point(c)),
                },
            };
            ObjectBody {
                name: body.name,
                motion_type: body.motion_type,
                shape: body.shape,
                geometry,
            }
        })
        .collect();
    Ok(ObjectCollisionFile {
        file: file.to_owned(),
        resolution,
        bodies,
    })
}
