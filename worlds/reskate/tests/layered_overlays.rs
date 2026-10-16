//! Gate of `lower-layered-reskate` and the writer's vertex-blend output on real scenes.
//!
//! For every scene folder under `PRIMASE_SCENES_GTA` (filtered by `PRIMASE_SCENE_FILTER`) with a `core.layered`
//! material: building without the pass is refused, naming it; with the pass (after loading the meshes) the build
//! writes, for every map material with `vertex_blend`, a `_RESKATE_BLEND` float per vertex in 0..1 on every GLB
//! primitive drawing it (and on no other primitive); `vertex_blend.json` lists exactly those materials, each with
//! the (mesh, primitive) pairs of map.json objects drawing it. The pass's counts (base and overlay triangles,
//! darkening, scaled weights) are printed: the cost of the overlays.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_core::pass::{Pass, PassLog};
use primase_core::{LayeredMaterial, World};
use primase_reskate::BuildOptions;
use primase_reskate::layered::LowerLayeredReskate;
use serde_json::Value;

fn scenes(root: &Path) -> Vec<PathBuf> {
    let filter = std::env::var("PRIMASE_SCENE_FILTER").unwrap_or_default();
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        if d.join("scene.json").is_file() {
            if d.to_string_lossy().contains(&filter) {
                out.push(d);
            }
            continue;
        }
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// The GLB's JSON and binary chunks.
fn glb(b: &[u8]) -> (Value, Vec<u8>) {
    let n = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    let json = serde_json::from_slice(&b[20..20 + n]).unwrap();
    let m = u32::from_le_bytes(b[20 + n..24 + n].try_into().unwrap()) as usize;
    (json, b[28 + n..28 + n + m].to_vec())
}

fn floats(json: &Value, bin: &[u8], accessor: usize) -> Vec<f32> {
    let a = &json["accessors"][accessor];
    let v = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
    let count = a["count"].as_u64().unwrap() as usize;
    (0..count)
        .map(|i| f32::from_le_bytes(bin[off + 4 * i..off + 4 * i + 4].try_into().unwrap()))
        .collect()
}

#[test]
fn layered_scenes_build_with_vertex_blend_overlays() {
    let Some(root) = primase_core::gate::real_data("PRIMASE_SCENES_GTA") else {
        return;
    };
    let out_root = std::env::var_os("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("reskate-layered");
    let mut built = 0;
    for scene in scenes(&root) {
        let mut w = World::new();
        primase_skate_scene::read(&scene, &mut w).unwrap();
        if w.iter::<LayeredMaterial>().count() == 0 {
            continue;
        }
        let o = BuildOptions {
            out_root: out_root.clone(),
            ..BuildOptions::default()
        };
        let refused = primase_reskate::build(&w, &scene, &o)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            refused.contains("lower-layered-reskate"),
            "{}: {refused}",
            scene.display()
        );
        primase_gltf::load_mesh_files(&mut w, &scene).unwrap();
        let mut log = PassLog::default();
        LowerLayeredReskate.run(&mut w, &mut log).unwrap();
        assert_eq!(w.iter::<LayeredMaterial>().count(), 0);
        println!("{}: {:?}", scene.display(), log.counts);
        let report = primase_reskate::build(&w, &scene, &o).unwrap();
        let out = report.out_dir;
        let map: Value = serde_json::from_slice(&std::fs::read(out.join("map.json")).unwrap()).unwrap();
        let blended: BTreeSet<String> = map["materials"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, m)| m["vertex_blend"] == Value::Bool(true))
            .map(|(k, _)| k.clone())
            .collect();
        assert!(!blended.is_empty(), "no vertex_blend material written");
        // every object's primitive: _RESKATE_BLEND exactly on vertex_blend materials, one float per vertex in 0..1
        let mut checked = (0usize, 0usize);
        let mut by_mesh: BTreeMap<String, (Value, Vec<u8>)> = BTreeMap::new();
        let mut record_want: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
        for obj in map["objects"].as_array().unwrap() {
            let (Some(mesh), Some(prim), Some(mat)) = (
                obj["mesh"].as_str(),
                obj["primitive"].as_u64(),
                obj["material"].as_str(),
            ) else {
                continue;
            };
            let (json, bin) = by_mesh
                .entry(mesh.to_owned())
                .or_insert_with(|| glb(&std::fs::read(out.join(mesh)).unwrap()))
                .clone();
            let p = &json["meshes"][0]["primitives"][prim as usize];
            let count = json["accessors"][p["attributes"]["POSITION"].as_u64().unwrap() as usize]["count"]
                .as_u64()
                .unwrap();
            match p["attributes"].get("_RESKATE_BLEND").and_then(Value::as_u64) {
                Some(a) => {
                    assert!(blended.contains(mat), "{mesh} {prim}: _RESKATE_BLEND on {mat}");
                    let w = floats(&json, &bin, a as usize);
                    assert_eq!(w.len() as u64, count);
                    assert!(w.iter().all(|x| (0.0..=1.0).contains(x)));
                    checked.0 += 1;
                    record_want
                        .entry(mat.to_owned())
                        .or_default()
                        .push((mesh.to_owned(), prim));
                }
                None => {
                    assert!(
                        !blended.contains(mat),
                        "{mesh} {prim}: {mat} has no _RESKATE_BLEND"
                    );
                    checked.1 += 1;
                }
            }
        }
        let record: Value =
            serde_json::from_slice(&std::fs::read(out.join("vertex_blend.json")).unwrap()).unwrap();
        let record: BTreeMap<String, Vec<(String, u64)>> = record["materials"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                let mut l: Vec<(String, u64)> = v
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (p[0].as_str().unwrap().to_owned(), p[1].as_u64().unwrap()))
                    .collect();
                l.sort();
                (k.clone(), l)
            })
            .collect();
        for l in record_want.values_mut() {
            l.sort();
        }
        assert_eq!(record, record_want, "vertex_blend.json vs map.json objects");
        println!(
            "{}: {} vertex_blend materials, {} blended object primitives, {} other",
            scene.display(),
            blended.len(),
            checked.0,
            checked.1
        );
        built += 1;
    }
    println!("{built} layered scenes built");
}
