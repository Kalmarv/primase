//! Collider gate against an independent reader's scan (`PRIMASE_UNITY_SCAN`: `colliders`, the colliding ones, each
//! `src` (scene or tree), `type` (BoxCollider, SphereCollider, CapsuleCollider, MeshCollider), `path`, `layer`, `M`
//! rounded to 6 decimals, `rigidbody` (null, "kinematic", "dynamic"), `physmat`, `friction`, and `size` and `center`,
//! `radius`, `height`, `direction`, or `mesh` and `convex`; `stats` with `collider_inactive` and `collider_trigger`),
//! for level `level1` of `PRIMASE_UNITY_DATA` read with `PRIMASE_UNITY_TPK`.
//!
//! Every collider that is not inactive or a trigger: its shape, physic material, rigidbody state and layer exactly,
//! its world matrix within the scan's rounding plus half an f32 step at the entry (our transforms are f32); the counts
//! of inactive and trigger colliders. Colliders pair by path, type and order.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_core::{Name, Transform, World};
use primase_unity::TypeTreePackage;
use primase_unity::components::{UnityCollider, UnityColliderShape, UnityTreeInstance};
use primase_unity::level::{UNITY, read_level};
use primase_unity::player::Player;
use serde_json::{Value as Json, json};

fn xyz(v: [f32; 3]) -> Json {
    json!({"x": f64::from(v[0]), "y": f64::from(v[1]), "z": f64::from(v[2])})
}

#[test]
fn colliders_match_the_scan() {
    let (Some(data), Some(tpk), Some(scan)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_SCAN"),
    ) else {
        return;
    };
    let scan: Json = serde_json::from_slice(&std::fs::read(&scan).unwrap()).unwrap();
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    let rep = read_level(&player, scan["level"].as_str().unwrap(), &mut w).unwrap();
    println!(
        "level: {} colliding, {} by trees; not colliding {:?}",
        rep.colliders, rep.tree_colliders, rep.colliders_not_colliding
    );
    assert_eq!(
        rep.colliders_not_colliding
            .get("inactive or disabled")
            .copied()
            .unwrap_or(0),
        scan["stats"]["collider_inactive"].as_u64().unwrap()
    );
    assert_eq!(
        rep.colliders_not_colliding.get("trigger").copied().unwrap_or(0),
        scan["stats"]["collider_trigger"].as_u64().unwrap()
    );
    // ours, colliding, by (path, type) in order
    let mut ours: BTreeMap<(String, &str), Vec<primase_core::Entity>> = BTreeMap::new();
    for (e, c) in w.iter::<UnityCollider>() {
        if !c.active || c.trigger {
            continue;
        }
        let kind = match c.shape {
            UnityColliderShape::Box { .. } => "BoxCollider",
            UnityColliderShape::Sphere { .. } => "SphereCollider",
            UnityColliderShape::Capsule { .. } => "CapsuleCollider",
            UnityColliderShape::Mesh { .. } => "MeshCollider",
        };
        ours.entry((w.get::<Name>(e).unwrap().0.clone(), kind))
            .or_default()
            .push(e);
    }
    let theirs = scan["colliders"].as_array().unwrap();
    assert_eq!(
        ours.values().map(Vec::len).sum::<usize>(),
        theirs.len(),
        "colliding colliders"
    );
    let mut fields = GateReport::new(
        "colliders: shape, physic material, rigidbody, layer",
        Tolerance::Exact,
    );
    let mut matrices = GateReport::new(
        "colliders: world matrix (scan rounding + half an f32 step)",
        Tolerance::Exact,
    );
    let mut taken: BTreeMap<(String, String), usize> = BTreeMap::new();
    for t in theirs {
        let key = (
            t["path"].as_str().unwrap().to_owned(),
            t["type"].as_str().unwrap().to_owned(),
        );
        let n = taken.entry(key.clone()).or_default();
        let Some(&e) = ours.get(&(key.0.clone(), key.1.as_str())).and_then(|v| v.get(*n)) else {
            fields.fail(&key.0, format!("{} not read", key.1));
            continue;
        };
        *n += 1;
        let c: &UnityCollider = w.get(e).unwrap();
        let tree = w.has::<UnityTreeInstance>(e);
        let mut mine = json!({
            "src": if tree { "tree" } else { "scene" }, "layer": c.layer,
            "rigidbody": match c.rigidbody { None => Json::Null, Some(true) => json!("kinematic"), Some(false) => json!("dynamic") },
            "physmat": c.physic_material.as_ref().map(|p| p.name.clone()),
            "friction": c.physic_material.as_ref().map(|p| f64::from(p.dynamic_friction)),
        });
        let o = mine.as_object_mut().unwrap();
        match &c.shape {
            UnityColliderShape::Box { center, size } => {
                o.insert("size".into(), xyz(*size));
                o.insert("center".into(), xyz(*center));
            }
            UnityColliderShape::Sphere { center, radius } => {
                o.insert("radius".into(), json!(f64::from(*radius)));
                o.insert("center".into(), xyz(*center));
            }
            UnityColliderShape::Capsule {
                center,
                radius,
                height,
                direction,
            } => {
                o.insert("radius".into(), json!(f64::from(*radius)));
                o.insert("height".into(), json!(f64::from(*height)));
                o.insert("direction".into(), json!(direction));
                o.insert("center".into(), xyz(*center));
            }
            UnityColliderShape::Mesh { mesh, convex, .. } => {
                o.insert(
                    "mesh".into(),
                    mesh.as_ref().map_or(Json::Null, |m| json!([m.file, m.path_id])),
                );
                o.insert("convex".into(), json!(convex));
            }
        }
        let mut want = t.clone();
        let wo = want.as_object_mut().unwrap();
        for k in ["path", "type", "M"] {
            wo.remove(k);
        }
        if mine == want {
            fields.record(&key.0, 0.0);
        } else {
            fields.fail(&key.0, format!("{mine} vs {want}"));
        }
        let back = UNITY.transform_to_source(w.get::<Transform>(e).unwrap());
        let mut beyond = Vec::new();
        for i in 0..3 {
            for j in 0..4 {
                let (a, b) = (f64::from(back.cols[j][i]), t["M"][i][j].as_f64().unwrap());
                if (a - b).abs() > 5e-7 + b.abs() * f64::from(f32::EPSILON) / 2.0 {
                    beyond.push(format!("[{i}][{j}] {a} vs {b}"));
                }
            }
        }
        if beyond.is_empty() {
            matrices.record(&key.0, 0.0);
        } else {
            matrices.fail(&key.0, beyond.join(", "));
        }
    }
    println!("{fields}\n{matrices}");
    fields.assert_passed();
    matrices.assert_passed();
}

/// The collision lowering checked against each collider's definition, in world space: a box's vertices are its
/// corners through the collider's matrix; a sphere's lie at the PhysX-scaled radius from its centre; a capsule's at the
/// scaled radius from its axis segment; every closed shape's triangles face outward; a mesh collider has its source
/// triangles; every terrain collider its heightfield's cells. Bounds: 8 f32 steps at the coordinates' magnitude.
#[test]
fn collision_matches_the_colliders() {
    use primase_core::{Collision, Surfaces, World};
    use primase_unity::lower_collision::lower_unity_collision;
    let (Some(data), Some(tpk), Some(scan)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_SCAN"),
    ) else {
        return;
    };
    let scan: Json = serde_json::from_slice(&std::fs::read(&scan).unwrap()).unwrap();
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    let rep = read_level(&player, scan["level"].as_str().unwrap(), &mut w).unwrap();
    let log = lower_unity_collision(&mut w).unwrap();
    println!("collision lowering: {:?}; {:?}", log.counts, log.approximations);
    assert_eq!(w.verify_components(), Vec::new());
    let surfaces: Vec<String> = w
        .resource::<Surfaces>()
        .unwrap()
        .0
        .iter()
        .map(|s| s.name.clone())
        .collect();
    println!("surfaces: {surfaces:?}");
    let eps = f64::from(f32::EPSILON);
    let mut gate = GateReport::new("collision: shapes against their colliders", Tolerance::Exact);
    let (mut lowered, mut without_mesh) = (0usize, 0usize);
    for (e, c) in w.iter::<UnityCollider>() {
        let col = w.get::<Collision>(e);
        if !c.collides() {
            assert!(
                col.is_none(),
                "{e:?}: collision for a collider that does not collide"
            );
            continue;
        }
        let Some(col) = col else {
            // a mesh collider with no mesh set has nothing to collide with
            if !matches!(c.shape, UnityColliderShape::Mesh { geometry: None, .. }) {
                gate.fail(w.get::<Name>(e).unwrap().0.as_str(), "no collision");
            } else {
                without_mesh += 1;
            }
            continue;
        };
        lowered += 1;
        let t = w.get::<Transform>(e).unwrap();
        let world: Vec<[f64; 3]> = col
            .positions
            .iter()
            .map(|p| {
                let mut out = [0f64; 3];
                for (i, o) in out.iter_mut().enumerate() {
                    *o = f64::from(t.cols[3][i])
                        + (0..3)
                            .map(|j| f64::from(t.cols[j][i]) * f64::from(p[j]))
                            .sum::<f64>();
                }
                out
            })
            .collect();
        let magnitude = world.iter().flatten().fold(1f64, |m, x| m.max(x.abs()));
        let bound = 8.0 * eps * magnitude;
        // the collider's matrix in Unity's frame, and a Unity-local point in world space
        let unity = UNITY.transform_to_source(t);
        let to_world = |p: [f64; 3]| {
            let mut q = [0f64; 3];
            for (i, o) in q.iter_mut().enumerate() {
                *o = f64::from(unity.cols[3][i])
                    + (0..3).map(|j| f64::from(unity.cols[j][i]) * p[j]).sum::<f64>();
            }
            [q[0], q[1], -q[2]]
        };
        let col_len = |j: usize| {
            (0..3)
                .map(|i| f64::from(unity.cols[j][i]).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let len = |a: [f64; 3]| dot(a, a).sqrt();
        let mut problems = Vec::new();
        // outward: each triangle's normal points away from `inside(centroid)`
        let outward = |inside: &dyn Fn([f64; 3]) -> [f64; 3]| {
            for tri in &col.triangles {
                let [a, b, cc] = tri.map(|i| world[i as usize]);
                let n = {
                    let (u, v) = (sub(b, a), sub(cc, a));
                    [
                        u[1] * v[2] - u[2] * v[1],
                        u[2] * v[0] - u[0] * v[2],
                        u[0] * v[1] - u[1] * v[0],
                    ]
                };
                let g = [0, 1, 2].map(|k| (a[k] + b[k] + cc[k]) / 3.0);
                if len(n) > 1e-9 && dot(n, sub(g, inside(g))) < 0.0 {
                    return false;
                }
            }
            true
        };
        match &c.shape {
            UnityColliderShape::Box { center, size } => {
                // the 8 corners through the matrix, as a set (box_mesh numbers them in its own basis)
                let c0 = [0, 1, 2].map(|k| f64::from(center[k]));
                let corners: Vec<[f64; 3]> =
                    (0..8)
                        .map(|i| {
                            to_world([0, 1, 2].map(|k| {
                                c0[k] + if i & (1 << k) != 0 { 0.5 } else { -0.5 } * f64::from(size[k])
                            }))
                        })
                        .collect();
                let matched = corners
                    .iter()
                    .all(|q| world.iter().any(|p| len(sub(*p, *q)) <= bound));
                if world.len() != 8 || !matched {
                    problems.push(format!("{} vertices, corners matched: {matched}", world.len()));
                }
                let mid = to_world(c0);
                if !outward(&|_| mid) {
                    problems.push("a triangle faces inward".into());
                }
            }
            UnityColliderShape::Sphere { center, radius } => {
                let c0 = to_world([0, 1, 2].map(|k| f64::from(center[k])));
                let r = f64::from(*radius) * (0..3).map(col_len).fold(0.0, f64::max);
                let worst = world
                    .iter()
                    .map(|p| (len(sub(*p, c0)) - r).abs())
                    .fold(0.0, f64::max);
                if worst > bound + 4.0 * eps * r {
                    problems.push(format!("a vertex {worst:e} off the radius {r}"));
                }
                if !outward(&|_| c0) {
                    problems.push("a triangle faces inward".into());
                }
            }
            UnityColliderShape::Capsule {
                center,
                radius,
                height,
                direction,
            } => {
                let d = (*direction).min(2);
                let c0 = to_world([0, 1, 2].map(|k| f64::from(center[k])));
                let mut e_d = [0f64; 3];
                e_d[d] = 1.0;
                let tip = to_world([0, 1, 2].map(|k| f64::from(center[k]) + e_d[k]));
                let axis = sub(tip, c0);
                let along = len(axis);
                let unit = axis.map(|x| x / along);
                let r = f64::from(*radius) * (0..3).filter(|&j| j != d).map(col_len).fold(0.0, f64::max);
                let half = (f64::from(*height) * along / 2.0 - r).max(0.0);
                let closest = |p: [f64; 3]| {
                    let s = dot(sub(p, c0), unit).clamp(-half, half);
                    [0, 1, 2].map(|k| c0[k] + unit[k] * s)
                };
                let worst = world
                    .iter()
                    .map(|p| (len(sub(*p, closest(*p))) - r).abs())
                    .fold(0.0, f64::max);
                if worst > bound + 4.0 * eps * r {
                    problems.push(format!("a vertex {worst:e} off the radius {r}"));
                }
                if !outward(&closest) {
                    problems.push("a triangle faces inward".into());
                }
            }
            UnityColliderShape::Mesh { geometry, .. } => {
                let g = geometry.as_ref().unwrap();
                if col.triangles.len() != g.triangles.len() || col.positions.len() != g.positions.len() {
                    problems.push(format!(
                        "{} triangles for {}",
                        col.triangles.len(),
                        g.triangles.len()
                    ));
                }
                for (p, s) in world.iter().zip(&g.positions) {
                    let d = len(sub(*p, to_world([0, 1, 2].map(|k| f64::from(s[k])))));
                    if d > bound {
                        problems.push(format!("a vertex off by {d:e}"));
                        break;
                    }
                }
            }
        }
        let name = w.get::<Name>(e).unwrap().0.clone();
        if problems.is_empty() {
            gate.record(&name, 0.0);
        } else {
            gate.fail(&name, problems.join("; "));
        }
    }
    let terrains = w
        .iter::<primase_unity::components::UnityTerrain>()
        .filter(|(e, t)| {
            let n = t.resolution as usize;
            w.get::<Collision>(*e)
                .is_some_and(|c| c.triangles.len() == 2 * (n - 1) * (n - 1))
        })
        .count();
    println!("{gate}\nterrain colliders with every cell: {terrains}");
    assert_eq!(lowered + without_mesh, rep.colliders);
    println!("mesh colliders with no mesh set (no collision): {without_mesh}");
    assert_eq!(terrains, rep.terrains);
    gate.assert_passed();
}
