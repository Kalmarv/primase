//! Object collision as the objects store it: each object prototype's main HKX bodies (`fromsoft.object_collision`),
//! meshes as their posed triangles, convex shapes as their stored core and radius.
use crate::collision::BodyGeometry;
use crate::pose::FRAME;
use crate::pose::pose_columns;
use crate::sekiro::source::{ObjectBodies, ObjectBody, ObjectGeometry};
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
}

/// An object's main HKX `file` (decoded `bytes`) as its prototype's `fromsoft.object_collision`: every body as
/// stored, in the prototype's space.
pub fn bodies(file: &str, bytes: &[u8], report: &mut Report) -> Result<ObjectBodies> {
    let bodies = collision::read_bodies(bytes, None)?
        .into_iter()
        .map(|body| {
            *report.bodies_by_motion.entry(body.motion_type).or_default() += 1;
            *report.bodies_by_shape.entry(body.shape.clone()).or_default() += 1;
            let geometry = match body.geometry {
                BodyGeometry::Mesh(soup) => ObjectGeometry::Mesh {
                    triangles: soup
                        .triangles
                        .into_iter()
                        .map(|t| t.map(|v| FRAME.point(v)))
                        .collect(),
                    materials: soup.materials,
                },
                BodyGeometry::Convex {
                    core,
                    capsule,
                    position,
                    orientation,
                } => ObjectGeometry::Convex {
                    core,
                    capsule,
                    // the body's pose, then the file frame's reflection into the world frame's axes
                    to_prototype: pose_columns(position, orientation).map(|c| FRAME.point(c)),
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
    report.models_with_collision += 1;
    Ok(ObjectBodies {
        file: file.to_owned(),
        bodies,
    })
}
