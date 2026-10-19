//! Gate of `lower-layered-reskate` and the writer's vertex-blend output on real scenes.
//!
//! For every scene folder under `PRIMASE_SCENES_GTA` (filtered by `PRIMASE_SCENE_FILTER`) with a `core.layered`
//! material, after the default level-of-detail choice: building without the pass is refused, naming it; with the pass (after loading the meshes) the build
//! writes, for every map material with `vertex_blend`, a `_RESKATE_BLEND` float per vertex in 0..1 on every GLB
//! primitive drawing it (and on no other primitive); `vertex_blend.json` lists exactly those materials, each with
//! the (mesh, primitive) pairs of map.json objects drawing it. The pass's counts (base and overlay triangles,
//! darkening, scaled weights) are printed: the cost of the overlays.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_core::pass::{Pass, PassLog};
use primase_core::{LayeredMaterial, Mesh, World};
use primase_reskate::EncodingOptions;
use primase_reskate::layered::{BlendOpacity, LowerLayeredReskate, OverlayLimit};
use primase_reskate::prepare::{PlanReskateBlendBudget, PrepareReskateBlendOrder, PrepareReskateOverlays};

/// The grid the blend chains are cut on: Studio's WorldPartition cell.
const GRID: f32 = 200.0;
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

/// The writer's default level of detail, as a pass (before the lowering: the writer refuses levels of detail).
fn choose_lod(w: &mut World) {
    primase_reskate::select_lod(w, primase_core::LodChoice::Default).unwrap();
}

/// After lowering: the overlay triangles that another overlay of the same base also draws. A base's overlays are the
/// primitives with opacities after it.
fn shared_overlay_triangles(w: &World) -> usize {
    let mut shared = 0;
    for (e, mesh) in w.iter::<Mesh>() {
        let Some(op) = w.get::<BlendOpacity>(e) else {
            continue;
        };
        let mut seen: BTreeSet<[u32; 3]> = BTreeSet::new();
        for (p, o) in mesh.primitives().iter().zip(&op.0) {
            if o.is_none() {
                seen.clear();
                continue;
            }
            for t in p.indices.as_deref().unwrap().as_chunks::<3>().0 {
                let mut k = *t;
                k.sort_unstable();
                shared += usize::from(!seen.insert(k));
            }
        }
    }
    shared
}

/// The box of every primitive's positions in a GLB (from the accessors' min and max).
fn glb_box(json: &Value) -> [f64; 6] {
    let mut b = [f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN];
    for p in json["meshes"][0]["primitives"].as_array().unwrap() {
        let a = &json["accessors"][p["attributes"]["POSITION"].as_u64().unwrap() as usize];
        for k in 0..3 {
            b[k] = b[k].min(a["min"][k].as_f64().unwrap());
            b[k + 3] = b[k + 3].max(a["max"][k].as_f64().unwrap());
        }
    }
    b
}

/// A (tile, cell, axis)'s rank objects: rank -> (translation, mesh).
type RankObjects = BTreeMap<u8, ([f32; 3], String)>;

/// Design A' ([`primase_reskate::blend_order`]): every chain rank of a tile's overlays is an object of its own per
/// cell of the blend grid, `<tile>_c<column>_<row>_v<axis><rank>`, placed at its tile's translation lifted along the
/// axis (the top rank lowest), in no tile mesh; the ranks of a (tile, cell, axis) have the same bounds, and those
/// bounds lie inside one cell of the grid (`grid` metres, Studio's frame x and z), so Studio has nothing to cut.
fn check_chain_objects(
    map: &Value,
    out: &Path,
    by_mesh: &BTreeMap<String, (Value, Vec<u8>)>,
    stats: &primase_reskate::blend_order::Stats,
    grid: f32,
) {
    let mut tile_at: BTreeMap<String, [f32; 3]> = BTreeMap::new();
    let mut ranks: BTreeMap<(String, u8), RankObjects> = BTreeMap::new();
    let (mut chain_mats, mut tile_mats) = (BTreeSet::new(), BTreeSet::new());
    for obj in map["objects"].as_array().unwrap() {
        let (Some(mesh), Some(mat)) = (obj["mesh"].as_str(), obj["material"].as_str()) else {
            continue;
        };
        let t: Vec<f32> = obj["transform"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap() as f32)
            .collect();
        let at = [t[9], t[10], t[11]];
        let stem = mesh.trim_start_matches("meshes/").trim_end_matches(".glb");
        if !stem.starts_with("t_") {
            continue;
        }
        match stem.rsplit_once("_v") {
            Some((group, ar)) if ar.len() == 2 && ar.bytes().all(|c| c.is_ascii_digit()) => {
                let (axis, rank) = (ar.as_bytes()[0] - b'0', ar.as_bytes()[1] - b'0');
                chain_mats.insert(mat.to_owned());
                let prev = ranks
                    .entry((group.to_owned(), axis))
                    .or_default()
                    .insert(rank, (at, mesh.to_owned()));
                assert!(prev.is_none_or(|(p, _)| p == at), "{stem}: sections placed apart");
            }
            _ => {
                tile_mats.insert(mat.to_owned());
                assert!(
                    tile_at.insert(stem.to_owned(), at).is_none_or(|p| p == at),
                    "{stem}"
                );
            }
        }
    }
    let both: Vec<_> = chain_mats.intersection(&tile_mats).collect();
    assert!(
        both.is_empty(),
        "chain overlay materials also in tile meshes: {both:?}"
    );
    assert_eq!(stats.tiles_shared, 0, "the cap was reached: {stats:?}");
    assert_eq!(stats.tiles_unliftable, 0, "tiles without a lift step: {stats:?}");
    let meshes: BTreeSet<&String> = ranks.values().flat_map(|r| r.values().map(|(_, m)| m)).collect();
    assert_eq!(meshes.len(), stats.objects, "objects added");
    let mut placements = BTreeSet::new();
    for ((group, axis), by_rank) in &ranks {
        // `<tile>_c<column>_<row>` with a grid, `<tile>` without
        let tile = group.rsplit_once("_c").map_or(group.as_str(), |(t, _)| t);
        let base = tile_at[tile];
        // Studio's frame (x, z, -y) back to the writer's
        let n = primase_reskate::blend_order::axis_vector(*axis);
        let n = [n.x, n.z, -n.y];
        let mut last = f32::INFINITY;
        let mut boxes = BTreeSet::new();
        for (rank, (at, mesh)) in by_rank {
            let d: Vec<f32> = (0..3).map(|k| at[k] - base[k]).collect();
            let along: f32 = (0..3).map(|k| d[k] * n[k]).sum();
            for k in 0..3 {
                assert!(n[k] != 0.0 || d[k] == 0.0, "{mesh}: lifted off its axis {d:?}");
            }
            assert!(
                along > 0.0 && along < last,
                "{mesh}: rank {rank} lift {along} after {last}"
            );
            last = along;
            assert!(
                placements.insert((group.clone(), at.map(f32::to_bits))),
                "{mesh}: shares a placement"
            );
            let json = match by_mesh.get(mesh) {
                Some((j, _)) => j.clone(),
                None => glb(&std::fs::read(out.join(mesh)).unwrap()).0,
            };
            let b = glb_box(&json);
            if grid > 0.0 {
                // inside one cell in Studio's horizontal axes (x, z), from the tile's (unlifted) translation
                for k in [0, 2] {
                    let (lo, hi) = (b[k] + f64::from(base[k]), b[k + 3] + f64::from(base[k]));
                    let cell = (lo / f64::from(grid)).floor();
                    assert!(
                        hi <= (cell + 1.0) * f64::from(grid),
                        "{mesh}: bounds {lo}..{hi} cross a {grid} m cell border"
                    );
                }
            }
            boxes.insert(b.map(f64::to_bits));
        }
        assert_eq!(boxes.len(), 1, "{group} axis {axis}: ranks with different bounds");
    }
    println!("blend order: {stats:?}");
}

#[test]
fn layered_scenes_build_with_vertex_blend_overlays() {
    let Some(root) = primase_testdata::real_data("PRIMASE_SCENES_GTA") else {
        return;
    };
    let out_root = std::env::var_os("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("reskate-layered");
    let mut built = 0;
    for scene in scenes(&root) {
        let mut w = World::new();
        primase_reskate::interchange::read(&scene, &mut w).unwrap();
        if w.iter::<LayeredMaterial>().count() == 0 {
            continue;
        }
        let mut o = EncodingOptions {
            out_root: out_root.clone(),
            ..EncodingOptions::default()
        };
        choose_lod(&mut w);
        let refused = primase_reskate::write_map(&w, &scene, &o)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            refused.contains("lower-layered-reskate"),
            "{}: {refused}",
            scene.display()
        );
        // OverlayLimit::One (not the default) on a second copy: a base's overlays share no triangle, and the
        // overlapping area it counts is the default's
        let mut one = World::new();
        primase_reskate::interchange::read(&scene, &mut one).unwrap();
        choose_lod(&mut one);
        primase_gltf::load_mesh_files(&mut one, &scene).unwrap();
        let mut one_log = PassLog::default();
        LowerLayeredReskate {
            overlays: OverlayLimit::One,
        }
        .run(&mut one, &mut one_log)
        .unwrap();
        println!(
            "{} with one overlay per triangle: {:?}",
            scene.display(),
            one_log.counts
        );
        // the pass's own triangle counts are not checked against a recount of its own output (both would move
        // together); what is checked is the contract: no triangle carries two overlays
        let shared = shared_overlay_triangles(&one);
        assert_eq!(shared, 0, "{}: triangles with two overlays", scene.display());
        // the default draws every layer
        primase_gltf::load_mesh_files(&mut w, &scene).unwrap();
        let mut log = PassLog::default();
        LowerLayeredReskate::default().run(&mut w, &mut log).unwrap();
        assert_eq!(w.iter::<LayeredMaterial>().count(), 0);
        println!("{}: {:?}", scene.display(), log.counts);
        assert_eq!(
            log.counts["area with overlapping overlays (m2)"],
            one_log.counts["area limited to one overlay (m2)"]
        );
        // the scene's warnings, with nothing left out (needs the lowered scene)
        o.scene_warnings = Some(
            primase_reskate::leave_out_optional(&mut w, &scene, None)
                .unwrap()
                .0,
        );
        // the writer's former stack and chains, made by their passes (4 mm, Studio's 200 m cells)
        PrepareReskateOverlays::new(0.004)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        PrepareReskateBlendOrder::new(GRID)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        PlanReskateBlendBudget::pool()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        let report = primase_reskate::write_map(&w, &scene, &o).unwrap();
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
        check_chain_objects(&map, &out, &by_mesh, &report.blend, GRID);
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
