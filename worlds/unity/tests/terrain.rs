//! Terrain gate against an independent reader's scan (`PRIMASE_UNITY_SCAN`: `terrains` each `name`, `path`, `M` (the
//! world matrix), `data` and `mat` as [file, path id], `size`, `collider`, `physmat` (the physic material's name),
//! `draw_trees`, `tree_colliders`), for the player `PRIMASE_UNITY_DATA` read with `PRIMASE_UNITY_TPK`: every drawn
//! terrain's path, data, material, size, collider, physic material and tree flags exactly, its position within the
//! scan's 6 decimals. Also reports, per terrain whose collider has its own TerrainData, whether its heights are the
//! drawn ones.

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_unity::TypeTreePackage;
use primase_unity::player::{ObjRef, Player};
use primase_unity::scene::Scene;
use primase_unity::terrain::{TerrainData, TerrainLayerData};
use primase_unity::value::Value;
use serde_json::{Value as Json, json};

fn obj(r: &Option<ObjRef>) -> Json {
    r.as_ref().map_or(Json::Null, |r| json!([r.file, r.path_id]))
}

#[test]
fn terrains_match_the_scan() {
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
    let scene = Scene::read(&player, scan["level"].as_str().unwrap()).unwrap();
    let ours = scene.terrains(&player).unwrap();
    let theirs = scan["terrains"].as_array().unwrap();
    assert_eq!(ours.len(), theirs.len(), "drawn terrains");
    let mut gate = GateReport::new(
        "terrains: path, data, material, size, collider, physic material, tree flags",
        Tolerance::Exact,
    );
    let mut position = GateReport::new("terrains: position", Tolerance::Abs(5e-7));
    let mut collider_heights = Vec::new();
    for (t, s) in ours.iter().zip(theirs) {
        let td = TerrainData::read(&player, &t.data).unwrap();
        for l in td.layers.iter().flatten() {
            TerrainLayerData::read(&player, l).unwrap();
        }
        let physmat = t.collider.as_ref().and_then(|c| c.material.as_ref()).map(|m| {
            player
                .decode(m)
                .unwrap()
                .get("m_Name")
                .and_then(Value::as_str)
                .unwrap()
                .to_owned()
        });
        let size = td.size();
        let mine = json!({
            "path": t.path, "data": obj(&Some(t.data.clone())), "mat": obj(&t.material),
            "size": size, "collider": t.collider.as_ref().is_some_and(|c| c.enabled),
            "physmat": physmat, "draw_trees": t.draw_trees,
            "tree_colliders": t.collider.as_ref().is_some_and(|c| c.tree_colliders),
        });
        let want = json!({
            "path": s["path"], "data": s["data"], "mat": s["mat"], "size": s["size"], "collider": s["collider"],
            "physmat": s["physmat"], "draw_trees": s["draw_trees"], "tree_colliders": s["tree_colliders"],
        });
        if mine == want {
            gate.record(&t.path, 0.0);
        } else {
            gate.fail(&t.path, format!("{mine} vs {want}"));
        }
        let mut worst = 0f64;
        for i in 0..3 {
            worst = worst.max((t.world[i][3] - s["M"][i][3].as_f64().unwrap()).abs());
        }
        position.record(&t.path, worst);
        if let Some(c) = &t.collider
            && let Some(cd) = &c.data
            && *cd != t.data
        {
            let other = TerrainData::read(&player, cd).unwrap();
            let same =
                other.resolution == td.resolution && other.scale == td.scale && other.heights == td.heights;
            let differ = other
                .heights
                .iter()
                .zip(&td.heights)
                .filter(|(a, b)| a != b)
                .count();
            collider_heights.push(format!(
                "{}: collider data {}/{} {}, {differ} heights differ",
                t.path,
                cd.file,
                cd.path_id,
                if same { "same as drawn" } else { "differs" }
            ));
        }
        println!(
            "{}: {}x{} heights, scale {:?}, {} layers, {} alphamaps at {}, {} tree prototypes, {} trees, holes {}",
            t.path,
            td.resolution,
            td.resolution,
            td.scale,
            td.layers.len(),
            td.alphamaps.len(),
            td.alphamap_resolution,
            td.tree_prototypes.len(),
            td.trees.len(),
            td.holes
                .as_ref()
                .map_or(0, |h| h.iter().filter(|&&x| x == 0).count())
        );
    }
    println!("{gate}\n{position}");
    println!("colliders with their own TerrainData: {collider_heights:#?}");
    gate.assert_passed();
    position.assert_passed();
}
