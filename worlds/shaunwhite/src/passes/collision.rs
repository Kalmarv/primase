//! Preserve authored triangles and implicit shapes; replace source surface ids with the world's surface indices.
use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Collider, Collision, Component, Resource, Surfaces, World};

use crate::components::{CollisionSource, CollisionSurfaces};

#[derive(Clone, Copy, Debug)]
pub struct LowerShaunwhiteCollision;

pub const LOWER_SHAUNWHITE_COLLISION: PassSpec = PassSpec {
    name: "lower-shaunwhite-collision",
    options: &[],
    make: |_| Ok(Box::new(LowerShaunwhiteCollision)),
};

impl Pass for LowerShaunwhiteCollision {
    fn name(&self) -> &'static str {
        "lower-shaunwhite-collision"
    }
    fn requires(&self) -> &[&'static str] {
        &[CollisionSource::NAME, CollisionSurfaces::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME, Collider::NAME, Surfaces::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[CollisionSource::NAME, CollisionSurfaces::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let names = &w
            .resource::<CollisionSurfaces>()
            .ok_or("missing collision surface names")?
            .0;
        let mut surfaces = w.resource::<Surfaces>().cloned().unwrap_or_default();
        let mut ids = BTreeMap::new();
        for (&id, name) in names {
            ids.insert(id, surfaces.index(name).map_err(|e| e.to_string())?);
        }
        let surface = |id: &u32| {
            ids.get(id)
                .copied()
                .ok_or_else(|| format!("collision surface {id:#010x} has no source name"))
        };
        let all: Vec<_> = w.iter::<CollisionSource>().map(|(e, s)| (e, s.clone())).collect();
        // Validate all references before changing the world.
        for (e, s) in &all {
            if w.has::<Collision>(*e) || w.has::<Collider>(*e) {
                return Err(format!("{e:?}: already has core collision"));
            }
            match s {
                CollisionSource::Mesh { surfaces, .. } => {
                    for id in surfaces {
                        surface(id)?;
                    }
                }
                CollisionSource::Shape { surface: id, .. } => {
                    surface(id)?;
                }
            }
        }
        w.insert_resource(surfaces);
        for (e, s) in all {
            match s {
                CollisionSource::Mesh {
                    positions,
                    triangles,
                    surfaces,
                } => {
                    let surfaces = surfaces.iter().map(surface).collect::<Result<_, _>>()?;
                    log.count("authored collision triangles", triangles.len() as u64);
                    w.insert(
                        e,
                        Collision {
                            positions,
                            triangles,
                            surfaces,
                        },
                    );
                }
                CollisionSource::Shape { shape, surface: id } => {
                    w.insert(
                        e,
                        Collider {
                            shape,
                            surface: surface(&id)?,
                        },
                    );
                    log.count("authored implicit colliders", 1);
                }
            }
            w.remove::<CollisionSource>(e);
        }
        w.remove_resource::<CollisionSurfaces>();
        log.count(
            "source surfaces (physics parameters not decoded)",
            ids.len() as u64,
        );
        Ok(())
    }
}
