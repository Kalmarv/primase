//! Resolve collision references without baking placements, constructing hulls or tessellating shapes.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::frame::SourceFrame;
use primase_core::{Entity, Instance, Name, Shape as CoreShape, Transform, World};

use crate::components::{CollisionPlacedBy, CollisionSource, CollisionSurfaces};
use anvil_formats::shaunwhite::collision::{Child, SURFACE_MATERIAL, Shape};
use anvil_formats::shaunwhite::scene;

use super::level::Index;

const FRAME: SourceFrame = SourceFrame::ENU;
type Leaf = (CollisionSource, Transform);
type Prototype = (Entity, Transform);

pub(super) struct CollisionReader<'a> {
    index: &'a Index,
    prototypes: BTreeMap<u32, Vec<Prototype>>,
    placed: BTreeSet<(u32, u32)>,
    surfaces: CollisionSurfaces,
}

impl<'a> CollisionReader<'a> {
    pub(super) fn new(index: &'a Index) -> Self {
        Self {
            index,
            prototypes: BTreeMap::new(),
            placed: BTreeSet::new(),
            surfaces: CollisionSurfaces::default(),
        }
    }
    pub(super) fn place(
        &mut self,
        world: &mut World,
        cell: &str,
        owner: u32,
        shape: u32,
        bump: &mut impl FnMut(String),
    ) {
        // Shared cell directories can name the same entity. One entity/shape pair is one physical placement.
        if !self.placed.insert((owner, shape)) {
            return;
        }
        let Some((object, body)) = self.index.get(owner).filter(|(o, _)| o.class == scene::ENTITY) else {
            bump(format!("collision entity {owner:#010x} not found"));
            return;
        };
        let entity = match scene::Entity::parse(body) {
            Ok(e) => e,
            Err(e) => {
                bump(format!("collision entity not read: {e}"));
                return;
            }
        };
        if !self.prototypes.contains_key(&shape) {
            let leaves = match referenced(self.index, shape, 0) {
                Ok(leaves) => leaves,
                Err(e) => {
                    bump(format!("collision shape {shape:#010x} not read: {e}"));
                    return;
                }
            };
            let name = self
                .index
                .get(shape)
                .map(|(o, _)| o.name.clone())
                .unwrap_or_default();
            let mut prototypes = Vec::new();
            for (source, local) in leaves {
                let ids = match &source {
                    CollisionSource::Mesh { surfaces, .. } => surfaces.clone(),
                    CollisionSource::Shape { surface, .. } => vec![*surface],
                };
                for id in ids {
                    self.surfaces.0.entry(id).or_insert_with(|| {
                        self.index
                            .get(id)
                            .filter(|(o, _)| o.class == SURFACE_MATERIAL && !o.name.is_empty())
                            .map(|(o, _)| o.name.clone())
                            .unwrap_or_else(|| format!("source:{id:#010x}"))
                    });
                }
                let p = world.spawn();
                world.insert(p, source);
                world.insert(p, Name(name.clone()));
                prototypes.push((p, local));
                bump("collision prototypes".into());
            }
            self.prototypes.insert(shape, prototypes);
        }
        let transform = FRAME.transform(&matrix(entity.matrix));
        for &(prototype, local) in self.prototypes.get(&shape).into_iter().flatten() {
            let e = world.spawn();
            world.insert(
                e,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            world.insert(e, transform.compose(&local));
            world.insert(e, Name(object.name.clone()));
            world.insert(
                e,
                CollisionPlacedBy {
                    cell: cell.into(),
                    entity: owner,
                    shape,
                },
            );
            bump("collision placements".into());
        }
    }
    pub(super) fn finish(self, world: &mut World) {
        if !self.prototypes.is_empty() {
            world.insert_resource(self.surfaces);
        }
    }
}

fn matrix(m: [f32; 16]) -> Transform {
    Transform {
        cols: [
            [m[0], m[1], m[2]],
            [m[4], m[5], m[6]],
            [m[8], m[9], m[10]],
            [m[12], m[13], m[14]],
        ],
    }
}
fn point(v: [f32; 4]) -> [f32; 3] {
    FRAME.point([v[0], v[1], v[2]])
}

fn referenced(index: &Index, id: u32, depth: usize) -> Result<Vec<Leaf>, String> {
    if depth > 64 {
        return Err("collision reference depth exceeds 64 (possible cycle)".into());
    }
    let (_, bytes) = index
        .get(id)
        .ok_or_else(|| format!("shape reference {id:#010x} not found"))?;
    leaves(index, Shape::parse(bytes).map_err(|e| e.to_string())?, depth)
}

fn leaves(index: &Index, shape: Shape<'_>, depth: usize) -> Result<Vec<Leaf>, String> {
    if depth > 64 {
        return Err("collision list depth exceeds 64".into());
    }
    let mut local = Transform::IDENTITY;
    let source = match shape {
        Shape::Mesh(m) => CollisionSource::Mesh {
            positions: m.vertices.into_iter().map(point).collect(),
            triangles: m.triangles.iter().map(|t| t.indices).collect(),
            surfaces: m.triangles.iter().map(|t| t.surface_id).collect(),
        },
        Shape::Capsule {
            bottom,
            top,
            radius,
            surface_id,
            ..
        } => CollisionSource::Shape {
            shape: CoreShape::Capsule {
                a: point(bottom),
                b: point(top),
                radius,
            },
            surface: surface_id,
        },
        Shape::Sphere {
            center,
            radius,
            surface_id,
            ..
        } => CollisionSource::Shape {
            shape: CoreShape::Sphere {
                center: point(center),
                radius,
            },
            surface: surface_id,
        },
        Shape::Box {
            half_extents,
            matrix: m,
            surface_id,
            ..
        } => {
            local = FRAME.transform(&matrix(m));
            CollisionSource::Shape {
                shape: CoreShape::Box {
                    center: [0.; 3],
                    half_extents: [half_extents[0], half_extents[2], half_extents[1]],
                },
                surface: surface_id,
            }
        }
        Shape::Convex {
            vertices,
            matrix: m,
            surface_id,
            ..
        } => {
            local = FRAME.transform(&matrix(m));
            CollisionSource::Shape {
                shape: CoreShape::Convex {
                    points: vertices.into_iter().map(point).collect(),
                },
                surface: surface_id,
            }
        }
        Shape::List { children, .. } => {
            let mut out = Vec::new();
            for child in children {
                out.extend(match child {
                    Child::Inline(shape) => leaves(index, *shape, depth.saturating_add(1))?,
                    Child::Reference { id, .. } => referenced(index, id, depth.saturating_add(1))?,
                    Child::Null => Vec::new(),
                });
            }
            return Ok(out);
        }
        Shape::MultiMesh { .. } => {
            return Err("MultiMeshShape must be placed by its contained entity/shape pairs".into());
        }
    };
    Ok(vec![(source, local)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_box_matrix_stays_local_and_composes_with_the_entity() {
        // Source: quarter-turn about Z, then (10,20,30). Its local (1,3,2) becomes (7,21,32).
        let m = [0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 1., 0., 10., 20., 30., 1.];
        let shape = Shape::Box {
            id: 7,
            half_extents: [1., 3., 2., 0.],
            matrix: m,
            surface_id: 8,
        };
        let result = leaves(&Index::default(), shape, 0).unwrap();
        assert_eq!(
            result[0].0,
            CollisionSource::Shape {
                shape: CoreShape::Box {
                    center: [0.; 3],
                    half_extents: [1., 2., 3.]
                },
                surface: 8,
            }
        );
        assert_eq!(result[0].1.apply([1., 2., -3.]), [7., 32., -21.]);
        let mut entity = Transform::IDENTITY;
        entity.cols[3] = [100., 300., -200.];
        assert_eq!(
            entity.compose(&result[0].1).apply([1., 2., -3.]),
            [107., 332., -221.]
        );
    }

    #[test]
    fn list_capsules_keep_endpoints_and_no_tessellation() {
        let shape = Shape::List {
            id: 1,
            flag: 0,
            children: vec![
                Child::Null,
                Child::Inline(Box::new(Shape::Capsule {
                    id: 2,
                    bottom: [1., 2., 3., 0.],
                    top: [1., 2., 5., 0.],
                    radius: 0.5,
                    surface_id: 9,
                })),
            ],
        };
        let result = leaves(&Index::default(), shape, 0).unwrap();
        assert_eq!(
            result,
            vec![(
                CollisionSource::Shape {
                    shape: CoreShape::Capsule {
                        a: [1., 3., -2.],
                        b: [1., 5., -2.],
                        radius: 0.5
                    },
                    surface: 9,
                },
                Transform::IDENTITY
            )]
        );
    }
}
