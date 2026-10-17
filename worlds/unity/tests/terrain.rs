//! Terrain gate against an independent reader's scan (`PRIMASE_UNITY_SCAN`: `terrains` each `name`, `path`, `M` (the
//! world matrix), `data` and `mat` as [file, path id], `size`, `collider`, `physmat` (the physic material's name),
//! `draw_trees`, `tree_colliders`), for the player `PRIMASE_UNITY_DATA` read with `PRIMASE_UNITY_TPK`: every drawn
//! terrain's path, data, material, size, collider, physic material and tree flags exactly, its position within the
//! scan's 6 decimals. Also reports, per terrain whose collider has its own TerrainData, whether its heights are the
//! drawn ones.

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::real_data;
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

fn read_f32(path: &std::path::Path) -> Vec<f32> {
    std::fs::read(path)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// Terrain lowering against another exporter's terrain pieces (`PRIMASE_UNITY_PORT_TERRAIN`: `index.json` with
/// `lift` and `pieces`, each `name`, `origin` (the terrain's x and z), `piece` (column and row of 128-cell pieces),
/// `verts`, `tris`, `texture`, and files `<name>.pos` / `.nrm` (f32 x, y, z in the world frame, y lifted by `lift`),
/// `.idx` (u32 triangles); `PRIMASE_UNITY_PORT`: its output folder, for `textures/<texture>`, the piece's base colour
/// tile at the alphamap's resolution, rows top first; `layers.json`, its layers' mean colours): triangles as sets
/// (winding kept) exactly; vertices within one f32 step of their lifted coordinate (they round after the lift); normals;
/// layer means within 1/255 of their float64 means (BC1 palettes rounded otherwise); base colour texels baked from
/// the means they used (float32 sums) within 1 of theirs, which checks the bake and its alignment; ours against theirs
/// reported.
#[test]
fn terrain_geometry_matches_the_port() {
    use primase_core::pbr::PbrMaterial;
    use primase_core::{Instance, Mesh, Semantic, Stream, Texture, Transform, World};
    use primase_unity::components::UnityTerrain;
    use primase_unity::level::read_level;
    use primase_unity::lower_terrain::lower_unity_terrain;
    use std::collections::BTreeSet;
    use std::path::Path;
    let (Some(data), Some(tpk), Some(dir), Some(port)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_PORT_TERRAIN"),
        real_data("PRIMASE_UNITY_PORT"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_level(&player, "level1", &mut w).unwrap();
    as_stored(&mut w);
    let log = lower_unity_terrain(&mut w).unwrap();
    println!("terrain lowering: {:?}; {:?}", log.counts, log.approximations);
    let dir = Path::new(&dir);
    let index: Json = serde_json::from_slice(&std::fs::read(dir.join("index.json")).unwrap()).unwrap();
    let lift = index["lift"].as_f64().unwrap();
    // layer mean colours: ours (from our decodes) against theirs (`layers.json`: each `file`, `path_id`, `mean` in
    // f64 and `mean_f32` as they bake it, float32 pixels averaged in float32)
    let layers: Json = serde_json::from_slice(&std::fs::read(dir.join("layers.json")).unwrap()).unwrap();
    let mut their_mean = std::collections::BTreeMap::new();
    let mut mean_max = 0f64;
    for (le, l) in w.iter::<primase_unity::components::UnityTerrainLayer>() {
        let theirs = layers["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["file"] == l.layer.file.as_str() && x["path_id"] == l.layer.path_id)
            .unwrap();
        let baked: [f64; 3] = [0, 1, 2].map(|c| theirs["mean_f32"][c].as_f64().unwrap());
        let theirs: [f64; 3] = [0, 1, 2].map(|c| theirs["mean"][c].as_f64().unwrap());
        let t: &Texture = w.get(l.diffuse.unwrap()).unwrap();
        let px = primase_texcodec::TexDecoder::decode_rgba8(
            &primase_texcodec::Decoder::default(),
            t.format,
            t.width,
            t.height,
            &t.levels[0],
        )
        .unwrap();
        let mut sum = [0f64; 3];
        for q in px.as_chunks::<4>().0 {
            for c in 0..3 {
                let x = f64::from(q[c]) / 255.0;
                sum[c] += if t.color_space == primase_core::ColorSpace::Srgb {
                    primase_image::srgb_to_linear(x)
                } else {
                    x
                };
            }
        }
        let n = (px.len() / 4) as f64;
        for c in 0..3 {
            mean_max = mean_max.max((sum[c] / n * f64::from(l.diffuse_remap[1][c]) - theirs[c]).abs());
        }
        their_mean.insert(le, baked);
    }
    // our terrains by their world translation
    let terrains: Vec<(primase_core::Entity, [f64; 3], &UnityTerrain)> = w
        .iter::<UnityTerrain>()
        .map(|(e, t)| {
            let c = w.get::<Transform>(e).unwrap().cols[3];
            (e, [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])], t)
        })
        .collect();
    let (mut pos_max, mut pos_at, mut nrm_max) = (0f64, String::new(), 0f64);
    let mut gate = GateReport::new(
        "terrain pieces: vertex grid, triangles (as sets) and counts",
        Tolerance::Exact,
    );
    // reported, not gated: their layer means carry float32 summation error (the mean and bake checks cover ours)
    let mut texels_max = 0u8;
    let mut formula = GateReport::new(
        "terrain pieces: base colour texels baked from their layer means (the bake and alignment)",
        Tolerance::Abs(1.0),
    );
    let mut beyond_step = 0u64;
    for p in index["pieces"].as_array().unwrap() {
        let name = p["name"].as_str().unwrap();
        let (ox, oz) = (p["origin"][0].as_f64().unwrap(), p["origin"][1].as_f64().unwrap());
        let &(e, at, t) = terrains
            .iter()
            .find(|(_, c, _)| (c[0] - ox).abs() < 0.5 && (-c[2] - oz).abs() < 0.5)
            .unwrap();
        let n = t.resolution as usize;
        let (x0, z0) = (
            p["piece"][0].as_u64().unwrap() as usize * 128,
            p["piece"][1].as_u64().unwrap() as usize * 128,
        );
        let (x1, z1) = ((x0 + 128).min(n - 1), (z0 + 128).min(n - 1));
        let nx = x1 - x0 + 1;
        let proto = w.get::<Instance>(e).unwrap().prototype;
        let prim = &w.get::<Mesh>(proto).unwrap().primitives()[0];
        let (Some(Stream::F32x3(pos)), Some(Stream::F32x3(nrm))) = (
            prim.streams.get(&Semantic::Position),
            prim.streams.get(&Semantic::Normal),
        ) else {
            panic!("streams")
        };
        let theirs = read_f32(&dir.join(format!("{name}.pos")));
        let their_n = read_f32(&dir.join(format!("{name}.nrm")));
        let mut problems = Vec::new();
        if theirs.len() != 3 * nx * (z1 - z0 + 1) {
            problems.push(format!(
                "{} vertices for {}",
                theirs.len() / 3,
                nx * (z1 - z0 + 1)
            ));
        }
        for z in z0..=z1 {
            for x in x0..=x1 {
                let k = (z - z0) * nx + x - x0;
                let g = z * n + x;
                for c in 0..3 {
                    let ours = at[c] + f64::from(pos[g][c]);
                    let want = f64::from(theirs[3 * k + c]) - if c == 1 { lift } else { 0.0 };
                    let d = (ours - want).abs();
                    // they round the lifted coordinate to f32: within one f32 step there
                    let step =
                        f64::from(f32::EPSILON) * (want.abs() + if c == 1 { lift } else { 0.0 }).max(1.0);
                    if d > step {
                        beyond_step += 1;
                    }
                    if d > pos_max {
                        pos_max = d;
                        pos_at = format!("{name} sample ({x}, {z}) axis {c}: {ours} vs {want}");
                    }
                    nrm_max = nrm_max.max((f64::from(nrm[g][c]) - f64::from(their_n[3 * k + c])).abs());
                }
            }
        }
        // triangles over the piece's cells, as grid corners rotated to start at the smallest
        let canon = |t: [u32; 3]| {
            let i = (0..3).min_by_key(|&i| t[i]).unwrap();
            [t[i], t[(i + 1) % 3], t[(i + 2) % 3]]
        };
        let to_grid = |k: u32| ((k as usize / nx + z0) * n + k as usize % nx + x0) as u32;
        let idx: Vec<u32> = std::fs::read(dir.join(format!("{name}.idx")))
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        let their_tris: BTreeSet<[u32; 3]> = idx
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| canon(t.map(to_grid)))
            .collect();
        let in_piece = |v: u32| {
            let (x, z) = (v as usize % n, v as usize / n);
            (x0..=x1).contains(&x) && (z0..=z1).contains(&z)
        };
        let our_tris: BTreeSet<[u32; 3]> = prim
            .indices
            .as_ref()
            .unwrap()
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| t.iter().all(|&v| in_piece(v)))
            .map(|t| canon(*t))
            .collect();
        if our_tris != their_tris {
            problems.push(format!(
                "triangles: {} ours, {} theirs, {} in common",
                our_tris.len(),
                their_tris.len(),
                our_tris.intersection(&their_tris).count()
            ));
        }
        if problems.is_empty() {
            gate.record(name, 0.0);
        } else {
            gate.fail(name, problems.join("; "));
        }
        // base colour texels: their tile is [a0, a1) x [b0, b1) of the alphamap, rows top first
        let m = prim.material.unwrap();
        let tex: &Texture = w
            .get(
                w.get::<PbrMaterial>(m)
                    .unwrap()
                    .base_color_texture
                    .unwrap()
                    .texture,
            )
            .unwrap();
        let ares = tex.width as usize;
        let a = |c: usize| ((c as f64 / (n - 1) as f64) * ares as f64).round() as usize;
        let (a0, a1, b0, b1) = (a(x0), a(x1), a(z0), a(z1));
        let file = Path::new(&port)
            .join("textures")
            .join(p["texture"].as_str().unwrap());
        let png = primase_image::read_png(&std::fs::read(file).unwrap())
            .unwrap()
            .image;
        let primase_image::Samples::U8(tile) = png.samples else {
            panic!("16-bit")
        };
        let ch = tile.len() / (png.width * png.height) as usize;
        // the same bake from their layer means, over our alphamap (rows top first)
        let alpha: &Texture = w.get(t.alphamaps[0].unwrap()).unwrap();
        let weights = &alpha.levels[0];
        let means: Vec<[f64; 3]> = t.layers.iter().map(|l| their_mean[&l.unwrap()]).collect();
        let (mut worst, mut worst_formula) = (0u8, 0u8);
        for ty in 0..b1 - b0 {
            for tx in 0..a1 - a0 {
                let row = ares - b1 + ty;
                let at = row * ares + a0 + tx;
                for c in 0..3 {
                    let ours = tex.levels[0][at * 4 + c];
                    let theirs = tile[(ty * (a1 - a0) + tx) * ch + c];
                    worst = worst.max(ours.abs_diff(theirs));
                    let lin: f64 = means
                        .iter()
                        .enumerate()
                        .map(|(l, m)| f64::from(weights[at * 4 + l]) / 255.0 * m[c])
                        .sum();
                    let baked = (primase_image::linear_to_srgb(lin).clamp(0.0, 1.0) * 255.0).round() as u8;
                    worst_formula = worst_formula.max(baked.abs_diff(theirs));
                }
            }
        }
        texels_max = texels_max.max(worst);
        formula.record(name, f64::from(worst_formula));
    }
    println!("{gate}\n{formula}");
    println!(
        "base colour texels, ours against theirs (their layer means summed in float32): largest difference {texels_max}"
    );
    println!("layer means, ours against their f64 means: largest difference {mean_max:e} (linear)");
    println!(
        "positions: largest difference {pos_max:e} m ({pos_at}); normals: largest difference {nrm_max:e}"
    );
    gate.assert_passed();
    formula.assert_passed();
    assert_eq!(
        beyond_step, 0,
        "positions beyond one f32 step of the lifted coordinate"
    );
    // a BC1 palette rounded otherwise moves each texel by at most 1, so the mean by at most 1/255
    assert!(mean_max <= 1.0 / 255.0, "layer means differ by {mean_max}");
}

/// Terrain trees against the scan's (`instances` with `src` "tree": `path`, `mesh`, `mats`, `M` rounded to 6
/// decimals): every tree renderer placed, with its mesh, materials, and its world matrix back in Unity's frame within
/// the scan's rounding (5e-7) plus half an f32 step at the entry (our transforms are f32).
#[test]
fn trees_match_the_scan() {
    use primase_core::{Instance, Name, Transform, World};
    use primase_unity::components::{UnityMesh, UnityRenderer};
    use primase_unity::level::{UNITY, read_level};
    use std::collections::BTreeMap;
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
    as_stored(&mut w);
    let ours: BTreeMap<&str, primase_core::Entity> = w
        .iter::<UnityRenderer>()
        .filter_map(|(e, _)| {
            let n = &w.get::<Name>(e).unwrap().0;
            n.contains("/tree").then_some((n.as_str(), e))
        })
        .collect();
    let theirs: Vec<&Json> = scan["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["src"] == "tree")
        .collect();
    println!(
        "tree renderers read: {} (conditioned {:?}); placed as stored: {}",
        rep.tree_renderers,
        rep.conditioned.get("tree renderers"),
        ours.len()
    );
    assert_eq!(ours.len(), theirs.len(), "tree renderers placed as stored");
    let mut records = GateReport::new("trees: path, mesh, materials", Tolerance::Exact);
    let mut matrices = GateReport::new(
        "trees: world matrix (scan rounding + half an f32 step)",
        Tolerance::Exact,
    );
    let mut worst = 0f64;
    for t in theirs {
        let path = t["path"].as_str().unwrap();
        let Some(&e) = ours.get(path) else {
            records.fail(path, "not placed");
            continue;
        };
        let u: &UnityRenderer = w.get(e).unwrap();
        let proto = w.get::<Instance>(e).unwrap().prototype;
        let m = w.get::<UnityMesh>(proto).unwrap();
        let mats: Vec<Json> = u
            .materials
            .iter()
            .map(|m| m.as_ref().map_or(Json::Null, |m| json!([m.file, m.path_id])))
            .collect();
        let mine = json!({"mesh": [m.mesh.file, m.mesh.path_id], "mats": mats});
        let want = json!({"mesh": t["mesh"], "mats": t["mats"]});
        if mine == want {
            records.record(path, 0.0);
        } else {
            records.fail(path, format!("{mine} vs {want}"));
        }
        let back = UNITY.transform_to_source(w.get::<Transform>(e).unwrap());
        let mut beyond = Vec::new();
        for i in 0..3 {
            for j in 0..4 {
                let ours = f64::from(back.cols[j][i]);
                let theirs = t["M"][i][j].as_f64().unwrap();
                let bound = 5e-7 + theirs.abs() * f64::from(f32::EPSILON) / 2.0;
                let d = (ours - theirs).abs();
                worst = worst.max(d / bound);
                if d > bound {
                    beyond.push(format!("[{i}][{j}] {ours} vs {theirs}"));
                }
            }
        }
        if beyond.is_empty() {
            matrices.record(path, 0.0);
        } else {
            matrices.fail(path, beyond.join(", "));
        }
    }
    println!("{records}\n{matrices}\nworst difference: {worst:.3} of the bound");
    println!(
        "level: {} tree renderers; not reproduced {:?}",
        rep.tree_renderers, rep.not_reproduced
    );
    records.assert_passed();
    matrices.assert_passed();
}

/// The level as stored: `resolve-state` with the defaults, then LOD 0 of every prototype.
fn as_stored(w: &mut primase_core::World) {
    let mut run = primase_core::pass::RunReport::default();
    primase_core::pass::PassManager::new(primase_core::pass::PassRegistry::new())
        .run(
            w,
            &primase_core::pass::Pipeline {
                passes: vec![
                    Box::new(primase_core::state::ResolveState::default()),
                    Box::new(primase_core::SelectLod {
                        choice: primase_core::LodChoice::Finest,
                    }),
                ],
            },
            &mut run,
        )
        .unwrap();
}
