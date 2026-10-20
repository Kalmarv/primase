//! Unity colliders lowered to collision shapes (pass `lower-unity-collision`): every `unity.collider` that collides
//! (not a trigger, not moved by physics) and every enabled terrain collider becomes a `core.collider` (a mesh collider:
//! `core.collision` triangles), made of the surface named by its physic material (`default` without one). Outputs that
//! need triangles plan `tessellate-colliders`.
//!
//! Shapes follow PhysX as Unity drives it:
//! - a box is a `core.collider` box in the collider's space, so the whole matrix applies (exact);
//! - a sphere's radius scales by the largest absolute axis scale of its matrix, a capsule's height by its axis's scale
//!   and its radius by the largest of the other two (the PhysX radius rule, counted). Such a shape is not one under its
//!   GameObject's matrix (a non-uniform scale would make an ellipsoid), so it is a `core.collider` on an entity of its
//!   own: translated to the shape's world centre, the shape world-aligned around it, linked to its collider by
//!   [`UnityColliderOf`] and carrying the collider's state condition;
//! - a convex mesh collider is a convex `core.collider` of its triangles' vertices (exact; PhysX cooks at most 255
//!   polygons, larger hulls are counted); a concave one is its source mesh's triangles as `core.collision`;
//! - a terrain collides with its heightfield: a `core.heightfield` with a heightfield `core.collider` on an entity of
//!   its own (the terrain's grid runs along Unity's +Z, the world's -Z, so its rows are reversed and it starts at the
//!   terrain's far edge), every cell split along the drawn terrain's diagonal. The collider's own heights, when they
//!   differ from the drawn ones, are not read: counted.

use primase_core::pass::{Pass, PassLog};
use primase_core::state::Condition;
use primase_core::{
    Collider, Collision, Diagonal, Entity, Heightfield, Name, Shape, Surfaces, Transform, World,
};
use primase_meshops::convex_hull;

use crate::components::UNITY;
use crate::components::{UnityCollider, UnityColliderOf, UnityColliderShape, UnityTerrain};
use crate::{Result, invalid};

type M3 = [[f64; 3]; 3];

/// The entity's matrix (world axes): its linear part and translation, in f64.
fn matrix(t: &Transform) -> (M3, [f64; 3]) {
    let mut l = [[0.0; 3]; 3];
    for (j, col) in t.cols.iter().take(3).enumerate() {
        for (i, &x) in col.iter().enumerate() {
            l[i][j] = f64::from(x);
        }
    }
    (l, t.cols[3].map(f64::from))
}

fn apply(l: &M3, o: [f64; 3], p: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| l[i][0] * p[0] + l[i][1] * p[1] + l[i][2] * p[2] + o[i])
}

fn column_length(l: &M3, j: usize) -> f64 {
    (l[0][j] * l[0][j] + l[1][j] * l[1][j] + l[2][j] * l[2][j]).sqrt()
}

/// A size `core.collider` accepts: finite and above zero.
fn positive(x: f32) -> bool {
    x.is_finite() && x > 0.0
}

fn f32s(p: [f64; 3]) -> [f32; 3] {
    p.map(|x| x as f32)
}

fn f64s(p: [f32; 3]) -> [f64; 3] {
    p.map(f64::from)
}

/// What one collider lowers to.
enum Lowered {
    /// A shape in the collider entity's own space.
    Here(Shape),
    /// A shape world-aligned around the world point `at`, on an entity of its own.
    Apart { at: [f64; 3], shape: Shape },
    /// Triangles in the collider entity's own space.
    Triangles(Vec<[f32; 3]>, Vec<[u32; 3]>),
}

/// What one collider lowers to, or `None` when it has nothing to collide with.
fn lower(c: &UnityCollider, t: &Transform, log: &mut PassLog) -> Option<Lowered> {
    let (l, o) = matrix(t);
    Some(match &c.shape {
        UnityColliderShape::Box { center, size } => {
            let half_extents = size.map(|s| (s / 2.0).abs());
            if !half_extents.iter().all(|&h| positive(h)) {
                log.count("box colliders without volume (a zero size)", 1);
                return None;
            }
            log.count("box colliders", 1);
            Lowered::Here(Shape::Box {
                center: UNITY.point(*center),
                half_extents,
            })
        }
        UnityColliderShape::Sphere { center, radius } => {
            let scale = (0..3).map(|j| column_length(&l, j)).fold(0.0, f64::max);
            let r = (f64::from(*radius) * scale) as f32;
            if !positive(r) {
                log.count("sphere colliders without volume (a zero radius)", 1);
                return None;
            }
            log.count("sphere colliders", 1);
            log.count("sphere radius times the largest axis scale (PhysX)", 1);
            Lowered::Apart {
                at: apply(&l, o, f64s(UNITY.point(*center))),
                shape: Shape::Sphere {
                    center: [0.0; 3],
                    radius: r,
                },
            }
        }
        UnityColliderShape::Capsule {
            center,
            radius,
            height,
            direction,
        } => {
            let d = (*direction).min(2);
            let mut axis = [0.0f32; 3];
            axis[d] = 1.0;
            let axis_world = apply(&l, [0.0; 3], f64s(UNITY.direction(axis)));
            let along = column_length(&l, d);
            let across = (0..3)
                .filter(|&j| j != d)
                .map(|j| column_length(&l, j))
                .fold(0.0, f64::max);
            let r = f64::from(*radius) * across;
            if !positive(r as f32) {
                log.count("capsule colliders without volume (a zero radius)", 1);
                return None;
            }
            let half = (f64::from(*height) * along / 2.0 - r).max(0.0);
            let unit = if along > 0.0 {
                axis_world.map(|x| x / along)
            } else {
                [0.0, 1.0, 0.0]
            };
            log.count("capsule colliders", 1);
            log.count(
                "capsule height times its axis's scale, radius times the largest other (PhysX)",
                1,
            );
            Lowered::Apart {
                at: apply(&l, o, f64s(UNITY.point(*center))),
                shape: Shape::Capsule {
                    a: f32s(unit.map(|x| -x * half)),
                    b: f32s(unit.map(|x| x * half)),
                    radius: r as f32,
                },
            }
        }
        UnityColliderShape::Mesh { convex, geometry, .. } => {
            let Some(g) = geometry else {
                log.count("mesh colliders without a mesh", 1);
                return None;
            };
            if *convex {
                // PhysX collides with the hull of the triangles' vertices
                let used: std::collections::BTreeSet<u32> = g.triangles.iter().flatten().copied().collect();
                let points: Vec<[f32; 3]> = used
                    .iter()
                    .map(|&i| UNITY.point(g.positions[i as usize]))
                    .collect();
                match convex_hull(&points) {
                    Ok(h) => {
                        if h.indices.len() / 3 > 255 {
                            log.approximate(
                                "convex hulls over 255 triangles (PhysX cooks at most 255 polygons)",
                                1,
                            );
                        }
                    }
                    Err(e) => {
                        log.count(&format!("convex mesh colliders without a hull ({})", e.0), 1);
                        return None;
                    }
                }
                log.count("convex mesh colliders", 1);
                return Some(Lowered::Here(Shape::Convex { points }));
            }
            log.count("mesh colliders", 1);
            Lowered::Triangles(
                g.positions.iter().map(|&p| UNITY.point(p)).collect(),
                // Unity's winding reversed for the mirrored frame
                g.triangles.iter().map(|&[a, b, c]| [a, c, b]).collect(),
            )
        }
    })
}

/// An entity of its own for a shape lowered apart from its collider `of`: named after it, at `at`, carrying its
/// state condition, and declared in `log` as made from `of`, so a conversion in parts keeps it with the part that
/// owns `of`.
fn apart(w: &mut World, log: &mut PassLog, of: Entity, at: [f64; 3], collider: Collider) -> Entity {
    let e = w.spawn();
    if let Some(n) = w.get::<Name>(of).cloned() {
        w.insert(e, n);
    }
    if let Some(c) = w.get::<Condition>(of).cloned() {
        w.insert(e, c);
    }
    let mut t = Transform::IDENTITY;
    t.cols[3] = f32s(at);
    w.insert(e, t);
    w.insert(e, collider);
    w.insert(e, UnityColliderOf(of));
    log.place_from(e, of);
    e
}

/// The heightfield terrain `t` collides with, in a space whose origin is the terrain's far corner (Unity's last row):
/// its rows reversed so they run along the world's +Z.
fn heightfield(t: &UnityTerrain) -> Heightfield {
    let n = t.resolution as usize;
    let [sx, sy, sz] = t.scale.map(f64::from);
    let heights = (0..n)
        .rev()
        .flat_map(|z| (0..n).map(move |x| (f64::from(t.heights[z * n + x]) / 32766.0 * sy) as f32))
        .collect();
    // Unity's hole flags are per cell, 0 a hole; cell row k here is Unity's cell row n - 2 - k
    let holes = t.holes.as_ref().map(|h| {
        (0..n - 1)
            .rev()
            .flat_map(|z| (0..n - 1).map(move |x| h[z * (n - 1) + x] == 0))
            .collect()
    });
    Heightfield {
        columns: n as u32,
        rows: n as u32,
        spacing: [sx as f32, sz as f32],
        heights,
        // Unity splits each cell from its (x, z) corner to its (x + 1, z + 1) corner; with the rows reversed that
        // joins the cell's (column, row + 1) and (column + 1, row) corners
        diagonal: Diagonal::Anti,
        holes,
        surfaces: None,
    }
}

/// Lower every collider and terrain collider (see the module documentation).
pub fn lower_unity_collision(w: &mut World) -> Result<PassLog> {
    let mut log = PassLog::default();
    let mut surfaces = w.resource::<Surfaces>().cloned().unwrap_or_default();
    let colliders: Vec<(Entity, UnityCollider)> = w
        .iter::<UnityCollider>()
        .filter(|(_, c)| c.collides())
        .map(|(e, c)| (e, c.clone()))
        .collect();
    log.count("colliders that collide", colliders.len() as u64);
    for (e, c) in colliders {
        let t = *w
            .get::<Transform>(e)
            .ok_or_else(|| invalid("a collider without a transform"))?;
        // its collision is made here, or it has none
        if let Some(u) = w.get_mut::<UnityCollider>(e) {
            u.lowered = true;
        }
        let Some(lowered) = lower(&c, &t, &mut log) else {
            continue;
        };
        let surface = surfaces
            .index(c.physic_material.as_ref().map_or("default", |p| p.name.as_str()))
            .map_err(|e| invalid(e.to_string()))?;
        match lowered {
            Lowered::Here(shape) => {
                w.insert(e, Collider { shape, surface });
            }
            Lowered::Apart { at, shape } => {
                apart(w, &mut log, e, at, Collider { shape, surface });
            }
            Lowered::Triangles(positions, tris) => {
                let mut col = Collision::default();
                col.extend(&positions, &tris, surface, |p| p);
                log.count("collision triangles", tris.len() as u64);
                w.insert(e, col);
            }
        }
    }
    let terrains: Vec<(Entity, UnityTerrain)> = w
        .iter::<UnityTerrain>()
        .filter(|(_, t)| t.collider.as_ref().is_some_and(|c| c.enabled))
        .map(|(e, t)| (e, t.clone()))
        .collect();
    log.count("terrain colliders", terrains.len() as u64);
    for (e, t) in terrains {
        let c = t.collider.as_ref().expect("filtered");
        if c.other_data.is_some() {
            log.approximate(
                "terrain colliders with their own heights: the drawn heights used",
                1,
            );
        }
        let at = w
            .get::<Transform>(e)
            .ok_or_else(|| invalid("a terrain without a transform"))?
            .cols[3]
            .map(f64::from);
        // the heightfield's origin: the terrain's far edge along the world's -Z
        let far = f64::from(t.resolution.saturating_sub(1)) * f64::from(t.scale[2]);
        let surface = surfaces
            .index(c.physic_material.as_deref().unwrap_or("default"))
            .map_err(|e| invalid(e.to_string()))?;
        let field = apart(
            w,
            &mut log,
            e,
            [at[0], at[1], at[2] - far],
            Collider {
                shape: Shape::Heightfield,
                surface,
            },
        );
        w.insert(field, heightfield(&t));
        log.approximate("terrain collider cells split along one diagonal", 1);
    }
    w.insert_resource(surfaces);
    Ok(log)
}

/// [`lower_unity_collision`] as a pass.
#[derive(Debug, Clone, Copy)]
pub struct LowerUnityCollision;

impl Pass for LowerUnityCollision {
    fn name(&self) -> &'static str {
        "lower-unity-collision"
    }
    /// Nothing: it lowers the colliders (`unity.collider`) and the terrain colliders (`unity.terrain`) a world has,
    /// either, both or neither, each counted.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    /// `core.collider` and `core.heightfield`, `core.collision` for concave mesh colliders, `unity.collider_of` on
    /// the entities of shapes lowered apart, and the `core.surfaces` resource they index.
    fn provides(&self) -> &[&'static str] {
        &[
            "core.collider",
            "core.heightfield",
            "core.collision",
            "unity.collider_of",
            "core.surfaces",
            // a shape lowered apart is an entity of its own, named and conditioned as its collider, placed
            "core.name",
            "core.condition",
            "core.transform",
        ]
    }
    /// It lowers Unity colliders and keeps them, marked `lowered`.
    fn legalizes(&self) -> &[&'static str] {
        &["unity.collider"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["unity.collider"]
    }
    fn approximates(&self) -> &'static str {
        "one diagonal per terrain cell; a terrain collider's own heights not read"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = lower_unity_collision(w).map_err(|e| e.to_string())?;
        log.absorb(r);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{ColliderMesh, UnityObject};
    use primase_core::pass::Pass;

    fn collider(shape: UnityColliderShape) -> UnityCollider {
        UnityCollider {
            collider: UnityObject {
                file: "level1".into(),
                path_id: 1,
            },
            layer: 0,
            shape,
            trigger: false,
            physic_material: None,
            rigidbody: None,
            lowered: false,
        }
    }

    #[test]
    fn a_collider_is_dropped_from_mapped_geometry_only_once_nothing_is_lost() {
        use primase_core::{Component, Linear, Linearity};
        let mut c = collider(UnityColliderShape::Sphere {
            center: [0.0; 3],
            radius: 1.0,
        });
        let l = Linear::mirror_x();
        // colliding, not yet lowered: kept (its collision is not made)
        assert!(matches!(c.apply_linear(&l), Linearity::Unsupported));
        c.lowered = true;
        assert!(matches!(c.apply_linear(&l), Linearity::Drop));
        // a trigger makes no collision
        let mut t = collider(UnityColliderShape::Sphere {
            center: [0.0; 3],
            radius: 1.0,
        });
        t.trigger = true;
        assert!(matches!(t.apply_linear(&l), Linearity::Drop));
        // the pass marks every collider that collides
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Transform::IDENTITY);
        w.insert(e, c.clone());
        let mut fresh = c;
        fresh.lowered = false;
        w.insert(e, fresh);
        lower_unity_collision(&mut w).unwrap();
        assert!(w.get::<UnityCollider>(e).unwrap().lowered);
    }

    /// A transform with these columns (world axes) and translation.
    fn transform(cols: [[f32; 3]; 3], at: [f32; 3]) -> Transform {
        Transform {
            cols: [cols[0], cols[1], cols[2], at],
        }
    }

    const I: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    fn lowered(c: &UnityCollider, t: &Transform) -> Option<Lowered> {
        lower(c, t, &mut PassLog::default())
    }

    #[test]
    fn a_box_stays_in_its_space_with_unitys_z_turned() {
        let c = collider(UnityColliderShape::Box {
            center: [1.0, 2.0, 3.0],
            size: [2.0, -4.0, 6.0],
        });
        let Some(Lowered::Here(Shape::Box { center, half_extents })) = lowered(&c, &transform(I, [5.0; 3]))
        else {
            panic!("a box in its own space");
        };
        assert_eq!((center, half_extents), ([1.0, 2.0, -3.0], [1.0, 2.0, 3.0]));
        let flat = collider(UnityColliderShape::Box {
            center: [0.0; 3],
            size: [1.0, 0.0, 1.0],
        });
        assert!(lowered(&flat, &transform(I, [0.0; 3])).is_none());
    }

    #[test]
    fn spheres_and_capsules_take_physx_radii_apart_from_a_non_uniform_matrix() {
        // scale (1, 2, 3) along the world axes, at (10, 0, 0)
        let scaled = transform(
            [[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 3.0]],
            [10.0, 0.0, 0.0],
        );
        let s = collider(UnityColliderShape::Sphere {
            center: [1.0, 1.0, 1.0],
            radius: 0.5,
        });
        // centre: Unity's (1, 1, 1) is the world's local (1, 1, -1), through the matrix (1, 2, -3), translated
        let Some(Lowered::Apart { at, shape }) = lowered(&s, &scaled) else {
            panic!("a sphere apart");
        };
        assert_eq!(at, [11.0, 2.0, -3.0]);
        assert_eq!(
            shape,
            Shape::Sphere {
                center: [0.0; 3],
                radius: 1.5
            }
        );
        // a capsule along Unity's y: height 4 x 2 = 8, radius 0.5 x max(1, 3) = 1.5, so the segment's half is 2.5
        let cap = collider(UnityColliderShape::Capsule {
            center: [0.0; 3],
            radius: 0.5,
            height: 4.0,
            direction: 1,
        });
        let Some(Lowered::Apart { at, shape }) = lowered(&cap, &scaled) else {
            panic!("a capsule apart");
        };
        assert_eq!(at, [10.0, 0.0, 0.0]);
        assert_eq!(
            shape,
            Shape::Capsule {
                a: [0.0, -2.5, 0.0],
                b: [0.0, 2.5, 0.0],
                radius: 1.5
            }
        );
        // along Unity's z, which the world's frame turns: the segment runs along the world's z, reversed
        let along_z = collider(UnityColliderShape::Capsule {
            center: [0.0; 3],
            radius: 0.5,
            height: 3.0,
            direction: 2,
        });
        let Some(Lowered::Apart { shape, .. }) = lowered(&along_z, &transform(I, [0.0; 3])) else {
            panic!("a capsule apart");
        };
        assert_eq!(
            shape,
            Shape::Capsule {
                a: [0.0, 0.0, 1.0],
                b: [0.0, 0.0, -1.0],
                radius: 0.5
            }
        );
        // a mirror changes no radius; a capsule shorter than its diameter has a segment of length 0
        let mirrored = transform([[-2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]], [0.0; 3]);
        let short = collider(UnityColliderShape::Capsule {
            center: [0.0; 3],
            radius: 1.0,
            height: 1.0,
            direction: 0,
        });
        let Some(Lowered::Apart { shape, .. }) = lowered(&short, &mirrored) else {
            panic!("a capsule apart");
        };
        assert_eq!(
            shape,
            Shape::Capsule {
                a: [0.0; 3],
                b: [0.0; 3],
                radius: 2.0
            }
        );
    }

    /// A shape lowered apart is a new entity named after its collider and placed: through the manager, so what it
    /// adds is checked against the declarations.
    #[test]
    fn a_shape_apart_is_a_named_placed_entity_through_the_manager() {
        use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Name("ball".into()));
        w.insert(e, transform(I, [10.0, 0.0, 0.0]));
        w.insert(
            e,
            collider(UnityColliderShape::Sphere {
                center: [0.0; 3],
                radius: 0.5,
            }),
        );
        let mut run = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                &mut w,
                &Pipeline {
                    passes: vec![Box::new(LowerUnityCollision)],
                },
                &mut run,
            )
            .unwrap();
        let (apart, _) = w.iter::<UnityColliderOf>().find(|(_, of)| of.0 == e).unwrap();
        assert_eq!(w.get::<Name>(apart).unwrap().0, "ball");
        assert_eq!(w.get::<Transform>(apart).unwrap().cols[3], [10.0, 0.0, 0.0]);
        assert!(w.has::<Collider>(apart));
    }

    #[test]
    fn convex_mesh_colliders_keep_their_points_and_flat_ones_have_none() {
        let mesh = |positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>| {
            collider(UnityColliderShape::Mesh {
                mesh: None,
                convex: true,
                geometry: Some(std::sync::Arc::new(ColliderMesh { positions, triangles })),
            })
        };
        let tetra = mesh(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [9.0, 9.0, 9.0],
            ],
            vec![[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]],
        );
        let Some(Lowered::Here(Shape::Convex { points })) = lowered(&tetra, &transform(I, [0.0; 3])) else {
            panic!("a convex shape");
        };
        // the vertices the triangles use, Unity's z turned; the unused one is left out
        assert_eq!(
            points,
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, -1.0]
            ]
        );
        let flat = mesh(vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]], vec![[0, 1, 2]]);
        let mut log = PassLog::default();
        assert!(lower(&flat, &transform(I, [0.0; 3]), &mut log).is_none());
        assert!(
            log.counts
                .keys()
                .any(|k| k.starts_with("convex mesh colliders without a hull"))
        );
    }

    /// A 2 x 2 terrain (one cell) at (100, 0, 50), samples 1 m apart, heights 1, 2, 3, 4 m at Unity's (x, z) (0, 0),
    /// (1, 0), (0, 1), (1, 1). Unity draws the cell as (a, c, d) and (a, d, b) with a (0, 0), b (1, 0), c (0, 1),
    /// d (1, 1); in the world Unity's z is -z, so the corners are (100, 1, 50), (101, 2, 50), (100, 3, 49), (101, 4, 49).
    /// The one-cell terrain described above, with its collider enabled, in `w`.
    fn one_cell_terrain(w: &mut World) -> Entity {
        let obj = |path_id| UnityObject {
            file: "level1".into(),
            path_id,
        };
        let terrain = w.spawn();
        w.insert(terrain, transform(I, [100.0, 0.0, 50.0]));
        w.insert(
            terrain,
            UnityTerrain {
                terrain: obj(7),
                data: obj(8),
                resolution: 2,
                scale: [1.0, 32766.0, 1.0],
                heights: vec![1, 2, 3, 4],
                holes: None,
                layers: Vec::new(),
                alphamaps: Vec::new(),
                alphamap_resolution: 0,
                material: None,
                draw_heightmap: true,
                draw_trees: false,
                collider: Some(crate::components::UnityTerrainCollider {
                    collider: obj(9),
                    enabled: true,
                    other_data: None,
                    physic_material: None,
                    tree_colliders: false,
                }),
                tree_prototypes: Vec::new(),
                trees: Vec::new(),
            },
        );
        terrain
    }

    /// A world of one terrain and no collider: the pass requires neither, and lowers the terrain's heightfield.
    #[test]
    fn a_terrain_only_world_is_lowered_through_the_manager() {
        use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
        let mut w = World::new();
        let terrain = one_cell_terrain(&mut w);
        let mut run = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                &mut w,
                &Pipeline {
                    passes: vec![Box::new(LowerUnityCollision)],
                },
                &mut run,
            )
            .unwrap();
        let (field, of) = w.iter::<UnityColliderOf>().next().unwrap();
        assert_eq!(of.0, terrain);
        assert!(w.has::<Heightfield>(field));
        let counts = &run.steps[0].log.counts;
        assert_eq!(counts["colliders that collide"], 0);
        assert_eq!(counts["terrain colliders"], 1);
        // and a world of neither: nothing lowered, both counted
        let mut empty = World::new();
        let mut run = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                &mut empty,
                &Pipeline {
                    passes: vec![Box::new(LowerUnityCollision)],
                },
                &mut run,
            )
            .unwrap();
        let counts = &run.steps[0].log.counts;
        assert_eq!(
            (counts["colliders that collide"], counts["terrain colliders"]),
            (0, 0)
        );
    }

    #[test]
    fn a_terrain_collides_with_unitys_cell_triangles_in_the_world() {
        let mut w = World::new();
        let terrain = one_cell_terrain(&mut w);
        lower_unity_collision(&mut w).unwrap();
        primase_meshops::TessellateColliders
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert!(w.verify_components().is_empty());
        let (e, of) = w.iter::<UnityColliderOf>().next().unwrap();
        assert_eq!(of.0, terrain);
        let t = w.get::<Transform>(e).unwrap();
        let col = w.get::<Collision>(e).unwrap();
        let world = |i: u32| {
            let p = col.positions[i as usize];
            [0, 1, 2].map(|k| p[k] + t.cols[3][k])
        };
        // each triangle as its corners, rotated to start at its smallest, facing up
        let mut ours: Vec<[[f32; 3]; 3]> = col
            .triangles
            .iter()
            .map(|tri| {
                let [a, b, c] = tri.map(world);
                let (u, v) = (
                    [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                    [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
                );
                assert!(u[2] * v[0] - u[0] * v[2] > 0.0, "a triangle faces down");
                let mut r = [a, b, c];
                let k = (0..3).min_by(|&i, &j| r[i].partial_cmp(&r[j]).unwrap()).unwrap();
                r.rotate_left(k);
                r
            })
            .collect();
        ours.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (a, b, c, d) = (
            [100.0, 1.0, 50.0],
            [101.0, 2.0, 50.0],
            [100.0, 3.0, 49.0],
            [101.0, 4.0, 49.0],
        );
        // Unity's (a, c, d) and (a, d, b), each in the world's counter-clockwise order seen from above
        let mut want = vec![[a, d, c], [a, b, d]];
        want.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert_eq!(ours, want);
    }
}
