//! The ReSkate world's layered lowerings on GTA's lookup-blended terrain: Vespucci Beach (`installs.gta`, `keys.gta`),
//! whose `*_cm*` and `*_2tex_blend*` terrain are layered materials, read, selected and lowered as the GTA recipe of
//! `reskate_assembly.rs` does, its per-pixel layer weights baked to vertex weights (`bake-layer-channels`).
//!
//! - `lower-layered-pbr` (the strongest layer per triangle, for a PBR consumer): every source triangle occurs once
//!   after the split and every resulting material is PBR; a prepared cell (the closure of its meshes) is still that
//!   closure after it.
//! - `lower-layered-reskate` and the map writer's vertex blend: building without the pass is refused, naming it;
//!   with one overlay per triangle no triangle carries two overlays, and the area it limits is the area the default
//!   (every layer drawn) counts as overlapping; written, every map material with `vertex_blend` has a
//!   `_RESKATE_BLEND` float per vertex in 0..1 on every GLB primitive drawing it (and no other primitive has one),
//!   `vertex_blend.json` lists exactly those materials with the (mesh, primitive) pairs of the map.json objects
//!   drawing them, and every chain rank of a tile's overlays is an object of its own inside one 200 m cell. The
//!   passes' counts (base and overlay triangles, darkening, scaled weights) are printed: the cost of the overlays.
//!
//! These ran over an earlier exporter's GTA scene folders in worlds/reskate's tests, retired with those folders;
//! reading the install composes the GTA and ReSkate crates, so they live here.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use primase_core::convert::{Finished, Report, Writer, convert};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, RunReport, Target};
use primase_core::pbr::PbrMaterial;
use primase_core::{LayeredMaterial, Mesh, World};
use primase_gta::convert::{GtaOptions, GtaReader};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{MarkerKind, SkateMarker, Snap, SnapMode};
use primase_reskate::passes::LowerLayeredPbr;
use primase_reskate::passes::layered::{BlendOpacity, LowerLayeredReskate, OverlayLimit};
use primase_testdata::{Game, install, keys};
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Vespucci Beach, GTA's west, south, east and north (metres).
const REGION: [f32; 4] = [-1500.0, -1650.0, -1150.0, -1330.0];
const RECT: &str = "x0=-1500,y0=-1650,x1=-1150,y1=-1330";
/// The grid the blend chains are cut on: Studio's WorldPartition cell.
const GRID: f32 = 200.0;
/// The GTA lowering of `reskate_assembly.rs`'s recipe, the layer channels baked to vertex weights, before the
/// ReSkate passes.
const LOWERED: &str = "select-lod{form=1}, lower-gta-embedded-collision{foliage=leave-out,river=leave-out}, \
     lower-gta-surfaces, gta-pose-fragments, lower-gta-pbr{night=false}, gta-winding, gta-tint, gta-terrain-layers, \
     bake-layer-channels, gta-vertex-colours, lower-gta-water, normalize-normals, bake-normal-scale";
/// What the ReSkate writer needs after the layered lowering, as the recipe prepares it.
const PREPARE: &str = "lower-tint-reskate, lower-vertex-alpha-reskate, cap-textures{max=256,normals=box}, \
     prepare-reskate-textures, prepare-reskate-water{extent=all}, prepare-reskate-overlays{step=0.004}, \
     prepare-reskate-blend-order{grid=200}, prepare-reskate-collision{surfaces=coalesce,weld=mm}, \
     summarize-reskate-map, plan-reskate-blend-budget{objects=pool}";

/// Every crate's passes by name, as a program registers them.
fn registry() -> PassRegistry {
    let mut r = PassRegistry::new();
    primase_core::register(&mut r);
    primase_image::register(&mut r);
    primase_meshops::register(&mut r);
    primase_gta::register(&mut r);
    primase_reskate::register(&mut r);
    r
}

/// Keeps a copy of the one part it is given, whatever it holds.
struct Capture(Arc<Mutex<Vec<World>>>);

impl Writer for Capture {
    fn target(&self) -> Target {
        Target::new("capture").carry_unknown()
    }
    fn write(&mut self, part: &World) -> primase_core::convert::Result<()> {
        let copy = part.subset(&part.entities().collect());
        self.0.lock().expect("one writer").push(copy);
        Ok(())
    }
    fn finish(self: Box<Self>) -> primase_core::convert::Result<Finished> {
        Ok(Report::new().into())
    }
}

/// Vespucci as a ReSkate map's region is selected (`reskate_assembly.rs`'s selection: its lights, collision, water and
/// spawn), converted in one part through `passes`; `None` when the install or its keys are not configured.
fn vespucci(m: &PassManager, passes: &str) -> Result<Option<World>> {
    let (Some(install), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return Ok(None);
    };
    let mut reader = GtaReader::open(&GtaOptions {
        install,
        keys,
        region: REGION,
        reach: 600.0,
        shaders: false,
    })?;
    let select = m.registry.parse(&format!(
        "resolve-state{{assign=gta.map:online;gta.hour:12}}, gta-select-region{{{RECT},min_size=0.5,max_priority=3}}, \
         lower-gta-lights{{hour=every,{RECT}}}, gta-select-collision{{detail=plain}}, \
         lower-gta-collision{{{RECT},foliage=leave-out,river=leave-out}}, lower-gta-surfaces, \
         gta-select-water{{{RECT},clip=kept}}, place-spawn{{at=centre}}"
    ))?;
    let passes = m.registry.parse(passes)?;
    let parts = Arc::new(Mutex::new(Vec::new()));
    convert(&mut reader, Box::new(Capture(parts.clone())), m, &select, &passes)?;
    let mut parts = std::mem::take(&mut *parts.lock().expect("done"));
    assert_eq!(parts.len(), 1, "one part");
    Ok(parts.pop())
}

/// The spawn `place-spawn{at=centre}` makes stands at the median of the placements, which may have no street under it to
/// snap to; the writer refuses a spawn over no ground with no height (it used to write one 50 m up, a fall). The
/// vertex blend is under test here, not the spawn: it stands at a stated height, unsnapped.
fn stand_the_spawn_at_a_height(w: &mut World) {
    let spawns: Vec<_> = w
        .iter::<SkateMarker>()
        .filter(|(_, m)| m.kind == MarkerKind::Spawn)
        .map(|(e, _)| e)
        .collect();
    assert!(!spawns.is_empty(), "the selection places a spawn");
    for e in spawns {
        let m = w.get_mut::<SkateMarker>(e).unwrap();
        if let Some((ground, _)) = m.position {
            m.position = Some((ground, Some(0.0)));
        }
        m.snap = Some(Snap::Mode(SnapMode::None));
    }
}

fn triangles(w: &World) -> usize {
    w.iter::<Mesh>()
        .flat_map(|(_, m)| m.primitives())
        .map(|p| {
            p.indices
                .as_ref()
                .map_or_else(|| p.positions().unwrap().len(), Vec::len)
                / 3
        })
        .sum()
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
fn layered_terrain_lowers_to_pbr_keeping_every_triangle() -> Result<()> {
    let m = PassManager::new(registry());
    let Some(mut world) = vespucci(&m, LOWERED)? else {
        return Ok(());
    };
    let layered = world.iter::<LayeredMaterial>().count();
    assert!(layered > 0, "no layered material in Vespucci");
    let before = triangles(&world);
    // the exact-closure contract of a prepared cell
    let roots: Vec<_> = world.iter::<Mesh>().map(|(entity, _)| entity).collect();
    let mut cell = world.subset(&world.closure(roots.iter().copied()));
    LowerLayeredPbr.run(&mut cell, &mut PassLog::default())?;
    assert_eq!(cell.closure(roots).len(), cell.entity_count());
    let mut log = PassLog::default();
    LowerLayeredPbr.run(&mut world, &mut log)?;
    assert_eq!(triangles(&world), before, "triangles after the split");
    for (_, mesh) in world.iter::<Mesh>() {
        for p in mesh.primitives() {
            if let Some(material) = p.material {
                assert!(
                    world.has::<PbrMaterial>(material),
                    "a lowered primitive has no PBR material"
                );
            }
        }
    }
    println!(
        "Vespucci: {layered} layered materials, {before} triangles preserved; {:?}",
        log.counts
    );
    Ok(())
}

#[test]
fn layered_terrain_builds_with_vertex_blend_overlays() -> Result<()> {
    let m = PassManager::new(registry());
    let raised =
        format!("{LOWERED}, raise-skate{{name=vespucci,light_scale=0.0003007,unclassified=default}}");
    let Some(mut w) = vespucci(&m, &raised)? else {
        return Ok(());
    };
    assert!(
        w.iter::<LayeredMaterial>().count() > 0,
        "no layered material in Vespucci"
    );
    stand_the_spawn_at_a_height(&mut w);
    let o = EncodingOptions {
        out_root: primase_testdata::scratch().join("reskate-layered"),
        name: Some("vespucci".into()),
        ..EncodingOptions::default()
    };
    let scene = Path::new("vespucci");
    let refused = primase_reskate::write_map(&w, scene, &o)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(refused.contains("lower-layered-reskate"), "{refused}");
    // OverlayLimit::One (not the default) on a copy: a base's overlays share no triangle, and the overlapping area it
    // counts is the default's
    let mut one = w.subset(&w.entities().collect());
    let mut one_log = PassLog::default();
    LowerLayeredReskate::new(OverlayLimit::One).run(&mut one, &mut one_log)?;
    println!("Vespucci with one overlay per triangle: {:?}", one_log.counts);
    // the pass's own triangle counts are not checked against a recount of its own output (both would move together);
    // what is checked is the contract: no triangle carries two overlays
    assert_eq!(shared_overlay_triangles(&one), 0, "triangles with two overlays");
    // the default draws every layer
    let mut log = PassLog::default();
    LowerLayeredReskate::new(OverlayLimit::All).run(&mut w, &mut log)?;
    assert_eq!(w.iter::<LayeredMaterial>().count(), 0);
    println!("Vespucci: {:?}", log.counts);
    // each limit counts the overlap of its own opacities: a layer's chain opacity (what `All` writes) is never above its
    // two-layer opacity (what `One` writes), and a layer draws where some corner's byte is above zero, so `One` sees at
    // least the overlap `All` draws, more where a corner has a trace of a layer over a nearly pure other (hand-worked in
    // `one_counts_the_overlap_of_a_layer_all_draws_at_a_zero_byte`). On Vespucci's terrain, with its layer channels
    // baked to vertex weights, they differ by 160 m2 of 158,543 (this assertion was first run on these data when the
    // test moved here from the retired scene folders, where the two were equal)
    let (all_area, one_area) = (
        log.counts["area with overlapping overlays (m2)"],
        one_log.counts["area limited to one overlay (m2)"],
    );
    assert!(
        one_area >= all_area,
        "One counts {one_area} m2 of overlap, All {all_area} m2: One must see every overlap All draws"
    );
    assert!(
        log.counts["triangles with overlapping overlays"]
            <= one_log.counts["triangles limited to one overlay"]
    );
    // the rest of the recipe's preparation: the overlay stack (4 mm), the chains (Studio's 200 m cells), their budget
    m.run(&mut w, &m.registry.parse(PREPARE)?, &mut RunReport::default())?;
    let report = primase_reskate::write_map(&w, scene, &o)?;
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
    Ok(())
}
