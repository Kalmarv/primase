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
