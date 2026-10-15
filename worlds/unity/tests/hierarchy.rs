//! Gate for a scene's hierarchy against an independent reader's scan (`PRIMASE_UNITY_SCAN`, JSON: `level`, `instances`
//! with `src` "scene" for the drawn mesh renderers in scene order, each `path`, `layer`, `mesh` and `mats` as
//! [file, path id], `M` the world matrix rounded to 6 decimals, `shadows`, `submeshes`; `stats` with
//! `renderer_inactive`, `renderer_lod1plus`, `renderer_kept`), for the player `PRIMASE_UNITY_DATA` read with the type
//! tree package `PRIMASE_UNITY_TPK`.
//!
//! Every drawn renderer: path, layer, mesh, materials, static-batch slice and shadow mode exactly; the world matrix
//! within the scan's rounding (5e-7) plus 1e-12 relative (the two readers sum the products in another order). Then the
//! level in the world: one entity per renderer, its transform converted back to Unity's frame the f32 matrix bit for
//! bit, one prototype per mesh.

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_core::{Instance, Transform, World};
use primase_unity::TypeTreePackage;
use primase_unity::components::{UnityMesh, UnityRenderer};
use primase_unity::level::{UNITY, read_level, transform_of};
use primase_unity::player::{ObjRef, Player};
use primase_unity::scene::Scene;
use serde_json::{Value as Json, json};

fn obj(r: &ObjRef) -> Json {
    json!([r.file, r.path_id])
}

#[test]
fn renderers_match_the_scan() {
    let (Some(data), Some(tpk), Some(scan)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_SCAN"),
    ) else {
        return;
    };
    let scan: Json = serde_json::from_slice(&std::fs::read(&scan).unwrap()).unwrap();
    let level = scan["level"].as_str().unwrap();
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let scene = Scene::read(&player, level).unwrap();
    let (ours, skipped) = scene.renderers(&player).unwrap();
    let theirs: Vec<&Json> = scan["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["src"] == "scene")
        .collect();
    let mut records = GateReport::new(
        "renderers: path, layer, mesh, materials, batch slice, shadows",
        Tolerance::Exact,
    );
    let mut matrices = GateReport::new("renderers: world matrix", Tolerance::Abs(5e-7));
    assert_eq!(ours.len(), theirs.len(), "drawn renderers: ours vs the scan");
    for (r, t) in ours.iter().zip(&theirs) {
        let item = format!("{} ({}/{})", r.path, r.renderer.file, r.renderer.path_id);
        let mats: Vec<Json> = r
            .materials
            .iter()
            .map(|m| m.as_ref().map_or(Json::Null, obj))
            .collect();
        let sub = r.submeshes.map_or(Json::Null, |(a, b)| json!([a, b]));
        let mine = json!({"path": r.path, "layer": r.layer, "mesh": obj(&r.mesh), "mats": mats, "submeshes": sub,
            "shadows": r.cast_shadows});
        let want = json!({"path": t["path"], "layer": t["layer"], "mesh": t["mesh"], "mats": t["mats"],
            "submeshes": t.get("submeshes").cloned().unwrap_or(Json::Null), "shadows": t["shadows"]});
        if mine == want {
            records.record(&item, 0.0);
        } else {
            records.fail(&item, format!("ours {mine} vs {want}"));
        }
        let mut worst = 0f64;
        for i in 0..4 {
            for j in 0..4 {
                let (a, b) = (r.world[i][j], t["M"][i][j].as_f64().unwrap());
                let d = (a - b).abs() - 1e-12 * b.abs();
                worst = worst.max(d.max(0.0));
            }
        }
        matrices.record(&item, worst);
    }
    let stats = &scan["stats"];
    println!("{records}\n{matrices}");
    println!(
        "ours skipped: {skipped:?}; the scan's stats: inactive {}, LOD 1+ {}",
        stats["renderer_inactive"], stats["renderer_lod1plus"]
    );
    assert_eq!(
        skipped.get("inactive or disabled").copied().unwrap_or(0),
        stats["renderer_inactive"].as_u64().unwrap()
    );
    assert_eq!(
        skipped.get("LOD 1+").copied().unwrap_or(0),
        stats["renderer_lod1plus"].as_u64().unwrap()
    );
    records.assert_passed();
    matrices.assert_passed();

    // into the world: each renderer an entity, its transform Unity's matrix (rounded once to f32) converted exactly
    let mut w = World::new();
    let rep = read_level(&player, level, &mut w).unwrap();
    let mut placed = GateReport::new(
        "world: renderer transforms, back in Unity's frame, the f32 matrix bit for bit",
        Tolerance::Exact,
    );
    let entities: Vec<_> = w.iter::<UnityRenderer>().collect();
    assert_eq!(entities.len(), ours.len());
    for ((e, u), r) in entities.iter().zip(&ours) {
        assert_eq!(u.renderer.path_id, r.renderer.path_id);
        let t: &Transform = w.get(*e).unwrap();
        let back = UNITY.transform_to_source(t);
        let want = transform_of(&r.world);
        let same = back
            .cols
            .iter()
            .flatten()
            .zip(want.cols.iter().flatten())
            .all(|(a, b)| a.to_bits() == b.to_bits());
        if same {
            placed.record(&r.path, 0.0);
        } else {
            placed.fail(&r.path, format!("{:?} vs {:?}", back.cols, want.cols));
        }
        let proto = w.get::<Instance>(*e).unwrap().prototype;
        assert_eq!(w.get::<UnityMesh>(proto).unwrap().0.path_id, r.mesh.path_id);
    }
    let meshes: std::collections::BTreeSet<String> = theirs.iter().map(|t| t["mesh"].to_string()).collect();
    println!(
        "{placed}
world: {} renderer entities, {} prototypes ({} meshes in the scan's scene renderers)",
        rep.renderers,
        rep.prototypes,
        meshes.len()
    );
    assert_eq!(rep.prototypes, meshes.len());
    assert!(UNITY.flips_winding(), "Unity's frame mirrors");
    placed.assert_passed();
}
