//! Unity colliders lowered to collision geometry (pass `lower-unity-collision`): every `unity.collider` that collides
//! (active, not a trigger, not moved by physics) and every enabled terrain collider gets a `core.collision` soup in
//! its entity's space, made of the surface named by its physic material (`default` without one).
//!
//! Shapes follow PhysX as Unity drives it:
//! - a box is its 8 corners and 12 triangles in the collider's space, so the whole matrix applies (exact);
//! - a sphere's radius scales by the largest absolute axis scale, a capsule's height by its axis's scale and its
//!   radius by the largest of the other two; both are built in world space (tessellated by `primase_meshops`, counted)
//!   and mapped back through the inverse of the entity's matrix;
//! - a mesh collider is its source mesh's triangles; a convex one collides with the hull, not yet built: its source
//!   mesh stands in (counted);
//! - a terrain collides with its heightfield (its collider's own heights when they differ from the drawn ones are not
//!   read: counted), every cell split along one diagonal as the drawn terrain.

use primase_core::pass::{Pass, PassLog};
use primase_core::{Collision, Entity, Surfaces, Transform, World};
use primase_meshops::{box_mesh, capsule, sphere};

use crate::components::{UnityCollider, UnityColliderShape, UnityTerrain};
use crate::level::UNITY;
use crate::lower_terrain::terrain_primitive;
use crate::{Result, invalid};

type M3 = [[f64; 3]; 3];
/// Positions and triangles.
type Soup = (Vec<[f32; 3]>, Vec<[u32; 3]>);

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

fn inverse(l: &M3) -> Option<M3> {
    let c = |i: usize, j: usize| {
        let (a, b) = ((i + 1) % 3, (i + 2) % 3);
        let (x, y) = ((j + 1) % 3, (j + 2) % 3);
        l[a][x] * l[b][y] - l[a][y] * l[b][x]
    };
    let det = l[0][0] * c(0, 0) + l[0][1] * c(0, 1) + l[0][2] * c(0, 2);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // inverse = adjugate / det; the adjugate is the transposed cofactor matrix
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            *x = c(j, i) / det;
        }
    }
    Some(out)
}

fn column_length(l: &M3, j: usize) -> f64 {
    (l[0][j] * l[0][j] + l[1][j] * l[1][j] + l[2][j] * l[2][j]).sqrt()
}

fn f32s(p: [f64; 3]) -> [f32; 3] {
    p.map(|x| x as f32)
}

fn f64s(p: [f32; 3]) -> [f64; 3] {
    p.map(f64::from)
}

/// A meshops soup as collision triangles.
fn triangles(indices: &[u32]) -> Vec<[u32; 3]> {
    indices.as_chunks::<3>().0.to_vec()
}

/// The soup of one collider in its entity's space, or `None` when it has none.
fn soup(c: &UnityCollider, t: &Transform, log: &mut PassLog) -> Result<Option<Soup>> {
    let err = |e: primase_meshops::Error| {
        invalid(format!(
            "collider {}/{}: {e}",
            c.collider.file, c.collider.path_id
        ))
    };
    let (l, o) = matrix(t);
    // a world-space soup mapped back into the entity's space
    let back = |positions: Vec<[f32; 3]>| -> Result<Vec<[f32; 3]>> {
        let inv = inverse(&l).ok_or_else(|| invalid("a collider with a singular matrix"))?;
        Ok(positions
            .iter()
            .map(|&p| {
                let p = f64s(p);
                f32s(apply(&inv, [0.0; 3], [p[0] - o[0], p[1] - o[1], p[2] - o[2]]))
            })
            .collect())
    };
    Ok(Some(match &c.shape {
        UnityColliderShape::Box { center, size } => {
            let axes = [0, 1, 2].map(|k| {
                let mut a = [0.0f32; 3];
                a[k] = size[k] / 2.0;
                UNITY.direction(a)
            });
            let m = box_mesh(UNITY.point(*center), axes).map_err(err)?;
            log.count("box colliders", 1);
            (m.positions, triangles(&m.indices))
        }
        UnityColliderShape::Sphere { center, radius } => {
            let scale = (0..3).map(|j| column_length(&l, j)).fold(0.0, f64::max);
            let c0 = apply(&l, o, f64s(UNITY.point(*center)));
            let m = sphere(f32s(c0), (f64::from(*radius) * scale) as f32).map_err(err)?;
            log.count("sphere colliders", 1);
            log.approximate("sphere colliders tessellated", 1);
            (back(m.positions)?, triangles(&m.indices))
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
            let half = (f64::from(*height) * along / 2.0 - r).max(0.0);
            let unit = if along > 0.0 {
                axis_world.map(|x| x / along)
            } else {
                [0.0, 1.0, 0.0]
            };
            let c0 = apply(&l, o, f64s(UNITY.point(*center)));
            let a = [0, 1, 2].map(|i| c0[i] - unit[i] * half);
            let b = [0, 1, 2].map(|i| c0[i] + unit[i] * half);
            let m = capsule(f32s(a), f32s(b), r as f32).map_err(err)?;
            log.count("capsule colliders", 1);
            log.approximate("capsule colliders tessellated", 1);
            (back(m.positions)?, triangles(&m.indices))
        }
        UnityColliderShape::Mesh { convex, geometry, .. } => {
            let Some(g) = geometry else {
                log.count("mesh colliders without a mesh", 1);
                return Ok(None);
            };
            if *convex {
                log.approximate("convex mesh colliders as their source mesh (hull pending)", 1);
            }
            log.count("mesh colliders", 1);
            (
                g.positions.iter().map(|&p| UNITY.point(p)).collect(),
                // Unity's winding reversed for the mirrored frame
                g.triangles.iter().map(|&[a, b, c]| [a, c, b]).collect(),
            )
        }
    }))
}

/// Lower every collider and terrain collider (see the module documentation).
pub fn lower_unity_collision(w: &mut World) -> Result<PassLog> {
    let mut log = PassLog::default();
    let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
    let colliders: Vec<(Entity, UnityCollider)> = w
        .iter::<UnityCollider>()
        .filter(|(_, c)| c.collides())
        .map(|(e, c)| (e, c.clone()))
        .collect();
    for (e, c) in colliders {
        let t = *w
            .get::<Transform>(e)
            .ok_or_else(|| invalid("a collider without a transform"))?;
        let Some((positions, tris)) = soup(&c, &t, &mut log)? else {
            continue;
        };
        let surface = surfaces.index(c.physic_material.as_ref().map_or("default", |p| p.name.as_str()));
        let mut col = Collision::default();
        col.extend(&positions, &tris, surface, |p| p);
        log.count("collision triangles", tris.len() as u64);
        w.insert(e, col);
    }
    let terrains: Vec<(Entity, UnityTerrain)> = w
        .iter::<UnityTerrain>()
        .filter(|(_, t)| t.collider.as_ref().is_some_and(|c| c.enabled))
        .map(|(e, t)| (e, t.clone()))
        .collect();
    for (e, t) in terrains {
        let c = t.collider.as_ref().expect("filtered");
        if c.other_data.is_some() {
            log.approximate(
                "terrain colliders with their own heights: the drawn heights used",
                1,
            );
        }
        let prim = terrain_primitive(&t, None);
        let positions = prim.positions().unwrap_or(&[]).to_vec();
        let tris: Vec<[u32; 3]> = triangles(prim.indices.as_deref().unwrap_or(&[]));
        let surface = surfaces.index(c.physic_material.as_deref().unwrap_or("default"));
        let mut col = Collision::default();
        col.extend(&positions, &tris, surface, |p| p);
        log.count("terrain colliders", 1);
        log.count("collision triangles", tris.len() as u64);
        log.approximate("terrain collider cells split along one diagonal", 1);
        w.insert(e, col);
    }
    w.insert_resource(surfaces);
    Ok(log)
}

/// [`lower_unity_collision`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct LowerUnityCollision;

impl Pass for LowerUnityCollision {
    fn name(&self) -> &'static str {
        "lower-unity-collision"
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.collider"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.collision"]
    }
    fn approximates(&self) -> &'static str {
        "spheres and capsules tessellated; convex mesh colliders as their source mesh; one diagonal per terrain cell"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = lower_unity_collision(w).map_err(|e| e.to_string())?;
        for (k, n) in r.counts {
            log.count(&k, n);
        }
        for (k, n) in r.approximations {
            log.approximate(&k, n);
        }
        Ok(())
    }
}
