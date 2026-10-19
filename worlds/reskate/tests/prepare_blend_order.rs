//! `prepare-reskate-blend-order`: a tile's vertex-blend chain overlays split into objects of their own before the
//! writer, which builds them from the record and chooses none; worked on hand-made blends.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Instance, Layer, LayerWeights, LayeredMaterial, Mesh, Name, Primitive, Semantic, Stream, Topology,
    Transform, WeightMapping, World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{
    MarkerKind, Other, SkateBlendBudget, SkateBlendChains, SkateLight, SkateMarker,
};
use primase_reskate::passes::SummarizeReskateMap;
use primase_reskate::passes::layered::{LowerLayeredReskate, OverlayLimit};
use primase_reskate::passes::{
    PlanReskateBlendBudget, PrepareReskateBlendOrder, PrepareReskateOverlays, RaiseOptions,
    plan_reskate_blend_budget, prepare_reskate_blend_order,
};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!(
            "primase-reskate-prepare-blend-order-{}",
            std::process::id()
        ))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// One blended (stone and moss) primitive with these corners, placed at `at` (scene), raised and lowered: its moss
/// layer is a chain overlay.
fn world(corners: [[f32; 3]; 4], at: [f32; 3]) -> World {
    let mut w = World::new();
    let layer = |w: &mut World, name: &str, base: [f32; 4]| {
        let m = w.spawn();
        w.insert(m, Name(name.into()));
        w.insert(
            m,
            PbrMaterial {
                base_color: base,
                metallic: 0.0,
                ..PbrMaterial::default()
            },
        );
        Some(Layer {
            material: m,
            texcoord: None,
            transform: None,
        })
    };
    let layers = vec![
        layer(&mut w, "stone", [0.5, 0.5, 0.5, 1.0]),
        layer(&mut w, "moss", [0.2, 0.5, 0.2, 1.0]),
    ];
    let blend = w.spawn();
    w.insert(blend, Name("stone and moss".into()));
    w.insert(
        blend,
        LayeredMaterial {
            layers,
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let proto = w.spawn();
    w.insert(proto, Name("blended".into()));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            material: Some(blend),
            indices: Some(vec![0, 1, 2, 2, 1, 3]),
            streams: BTreeMap::from([
                (Semantic::Position, Stream::F32x3(corners.to_vec())),
                (
                    Semantic::TexCoord(0),
                    Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.], [1., 1.]]),
                ),
                (
                    Semantic::Color(1),
                    Stream::F32x4(vec![
                        [1., 0., 0., 0.],
                        [0.5, 0.5, 0., 0.],
                        [1., 0., 0., 0.],
                        [0.5, 0.5, 0., 0.],
                    ]),
                ),
            ]),
        }]),
    );
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = at;
    w.insert(placed, t);
    primase_reskate::passes::raise_skate(&mut w, &RaiseOptions::new("chains").unwrap()).unwrap();
    LowerLayeredReskate::new(OverlayLimit::All)
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([at[0], at[2]], Some(at[1]))),
            yaw: None,
            m: None,
            blueprint: None,
            snap: None,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    w
}

/// A flat 20 m square on the ground from east 190 to 210: across the 200 m cell border, in the tile at east 100.
fn across_the_border() -> World {
    world(
        [[0., 0., 0.], [20., 0., 0.], [0., 0., -10.], [20., 0., -10.]],
        [190.0, 0.0, -50.0],
    )
}

type Files = BTreeMap<String, Vec<u8>>;

fn write(w: &World, tag: &str, e: &EncodingOptions) -> Result<(primase_reskate::BuildReport, Files), String> {
    let out_root = tmp(tag);
    let r = primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("chains".into()),
            ..e.clone()
        },
    )
    .map_err(|e| e.to_string())?;
    Ok((r, files(&out_root.join("chains"))))
}

/// The folder's files but stats.json's run time.
fn files(root: &Path) -> Files {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let mut bytes = std::fs::read(&p).unwrap();
            if rel == "stats.json" {
                let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                v["totals"].as_object_mut().map(|t| t.remove("seconds"));
                bytes = serde_json::to_vec(&v).unwrap();
            }
            out.insert(rel, bytes);
        }
    }
    out
}

/// The chain crossing the border is clipped into both cells, each cell's ranks padded to one box and lifted along the
/// axis; the writer builds exactly that from the record (its own grouping checked against it); without the pass it
/// splits nothing and says so once.
#[test]
fn a_chain_across_a_cell_border_is_cut_padded_and_lifted() {
    let mut w = across_the_border();
    // prepared as build_map prepares it: the overlay stack (its relations order the tile), then the chains
    PrepareReskateOverlays::new(0.004)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let mut log = PassLog::default();
    PrepareReskateBlendOrder::new(200.0)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    // and their budget: the cull-ID pool's room
    SummarizeReskateMap::for_encoding(&EncodingOptions::default())
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    PlanReskateBlendBudget::pool()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let rec = w.resource::<SkateBlendChains>().unwrap().clone();
    assert_eq!(rec.tiles.len(), 1, "{rec:?}");
    let tile = rec.tiles.values().next().unwrap();
    assert!(tile.steps.is_some());
    let cells: std::collections::BTreeSet<(i32, i32)> =
        tile.groups.keys().map(|&(cell, _, _)| cell).collect();
    assert_eq!(cells.len(), 2, "two cells: {:?}", tile.groups.keys());
    for g in tile.groups.values() {
        assert!(g.clipped > 0, "each cell has clipped triangles: {g:?}");
        assert!(g.pad.is_some());
    }
    assert!(log.counts["chain triangles clipped at the grid"] > 0);
    let (r, chained) = write(&w, "chained", &EncodingOptions::default()).unwrap();
    assert_eq!(r.blend.tiles_split, 1);
    assert_eq!(r.blend.objects, tile.groups.len());
    assert!(r.blend.triangles_clipped > 0);
    // the pass counts the triangles it cut as the writer's stats count them, once each
    assert_eq!(
        log.counts["chain triangles clipped at the grid"],
        r.blend.triangles_clipped as u64
    );
    let map: serde_json::Value = serde_json::from_slice(&chained["map.json"]).unwrap();
    let names: Vec<&str> = map["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["name"].as_str())
        .filter(|n| n.contains("_c0_") || n.contains("_c1_"))
        .collect();
    assert_eq!(names.len(), tile.groups.len(), "{names:?}");
    // no pass: the chain overlays stay in their tile, reported once
    let (r, _) = write(&across_the_border(), "plain", &EncodingOptions::default()).unwrap();
    assert_eq!(r.blend.objects, 0);
    assert!(
        r.unsupported
            .iter()
            .any(|(what, n)| what.contains("prepare-reskate-blend-order") && *n == 1),
        "{:?}",
        r.unsupported
    );
}

/// The writer refuses a record for other tiles: another grouping, or a tile whose objects differ from its own.
#[test]
fn a_record_for_other_tiles_is_refused() {
    let mut w = across_the_border();
    PrepareReskateBlendOrder::new(200.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let e = write(
        &w,
        "other-tile",
        &EncodingOptions {
            tile: 100.0,
            ..EncodingOptions::default()
        },
    )
    .unwrap_err();
    assert!(e.contains("chosen for the writer settings"), "{e}");
    // one object's clipping changed: not the writer's tile
    let rec = w.resource_mut::<SkateBlendChains>().unwrap();
    let g = rec
        .tiles
        .values_mut()
        .next()
        .unwrap()
        .groups
        .values_mut()
        .next()
        .unwrap();
    g.clipped += 1;
    let e = write(&w, "tampered", &EncodingOptions::default()).unwrap_err();
    assert!(
        e.contains("chosen for other pieces, objects, clipping or boxes"),
        "{e}"
    );
}

/// A wall facing east a thousand kilometres out: no lift step holds as an f32 there, so the pass refuses the tile (an
/// approximation it reports) and the writer leaves its chain overlays in the tile.
#[test]
fn an_unliftable_tile_keeps_its_chain_overlays() {
    let far = EncodingOptions {
        limit: 2.0e6,
        ..EncodingOptions::default()
    };
    let mut w = world(
        [[0., 0., 0.], [0., 0., -10.], [0., 10., 0.], [0., 10., -10.]],
        [1.0e6, 0.0, -50.0],
    );
    let mut log = PassLog::default();
    PrepareReskateBlendOrder::new(200.0)
        .unwrap()
        .for_encoding(&far)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    let rec = w.resource::<SkateBlendChains>().unwrap();
    assert_eq!(
        rec.tiles.values().filter(|t| t.steps.is_none()).count(),
        1,
        "{rec:?}"
    );
    assert_eq!(
        log.approximations.values().sum::<u64>(),
        1,
        "{:?}",
        log.approximations
    );
    let (r, _) = write(&w, "far", &far).unwrap();
    assert_eq!((r.blend.tiles_unliftable, r.blend.objects), (1, 0));
}

/// The grid is required; every setting prints back and reads back the same; bad values and unknown keys are refused.
#[test]
fn the_blend_order_options_round_trip_and_are_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    for given in [
        o(&[("grid", "200")]),
        o(&[("grid", "0"), ("tile", "100"), ("limit", "2000")]),
    ] {
        let pass = prepare_reskate_blend_order(&given).unwrap();
        assert_eq!(
            prepare_reskate_blend_order(&pass.options()).unwrap().options(),
            pass.options()
        );
    }
    for bad in [
        o(&[]),
        o(&[("grid", "-1")]),
        o(&[("grid", "inf")]),
        o(&[("grid", "200"), ("tile", "0")]),
        o(&[("grid", "200"), ("step", "0.004")]),
    ] {
        assert!(prepare_reskate_blend_order(&bad).is_err(), "{bad:?}");
    }
}

/// The border chain prepared up to its budget: the overlays stacked, the chains split.
fn chained() -> World {
    let mut w = across_the_border();
    PrepareReskateOverlays::new(0.004)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    PrepareReskateBlendOrder::new(200.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    w
}

/// A fixed count is decided by the pass: too few objects for the tile's chain objects leave it plain (an approximation
/// the pass reports), enough give it the chain form; the writer follows the plan and decides nothing.
#[test]
fn a_fixed_budget_is_decided_by_the_pass() {
    let objects = chained()
        .resource::<SkateBlendChains>()
        .unwrap()
        .tiles
        .values()
        .next()
        .unwrap()
        .groups
        .len();
    for (n, split) in [(objects - 1, 0), (objects, 1)] {
        let mut w = chained();
        let mut log = PassLog::default();
        PlanReskateBlendBudget::objects(n).run(&mut w, &mut log).unwrap();
        let plan = w.resource::<SkateBlendBudget>().unwrap();
        assert_eq!((plan.ranked.len(), plan.chosen.len()), (1, split));
        assert_eq!(
            log.approximations.values().sum::<u64>(),
            1 - split as u64,
            "{:?}",
            log.approximations
        );
        let (r, _) = write(&w, &format!("objects-{n}"), &EncodingOptions::default()).unwrap();
        assert_eq!((r.blend.tiles_split, r.blend.tiles_shared), (split, 1 - split));
        assert!(
            r.unsupported.iter().all(|(what, _)| !what.contains("blend")),
            "{:?}",
            r.unsupported
        );
    }
}

/// The chained map's summary and pool budget for an encoding with `cull_ids`.
fn pool_planned(cull_ids: usize) -> (World, primase_reskate::components::SkateMapSummary, PassLog) {
    let mut w = chained();
    SummarizeReskateMap::for_encoding(&EncodingOptions {
        cull_ids,
        ..EncodingOptions::default()
    })
    .unwrap()
    .run(&mut w, &mut PassLog::default())
    .unwrap();
    let summary = w
        .resource::<primase_reskate::components::SkateMapSummary>()
        .unwrap()
        .clone();
    let mut log = PassLog::default();
    PlanReskateBlendBudget::pool().run(&mut w, &mut log).unwrap();
    (w, summary, log)
}

/// In pool mode the chain form is decided before writing, from the map's summary: the room is the encoding's cull IDs
/// less what the rest of the map takes, counted as the writer counts it (its stats name the same number). With room for
/// the tile's chain objects it takes the chain form; one ID short it is planned plain, reported by the plan. The writer
/// replays the decision and refuses a map written with another basis.
#[test]
fn the_pool_is_decided_before_writing_and_replayed() {
    let (w, summary, _) = pool_planned(EncodingOptions::default().cull_ids);
    let (r, _) = write(&w, "pool-room", &EncodingOptions::default()).unwrap();
    assert_eq!((r.blend.tiles_split, r.blend.tiles_shared), (1, 0));
    // the summary's count is the writer's own
    assert_eq!(summary.other_cull_ids, r.blend.other_cull_ids);
    assert!(summary.other_cull_ids > 0);
    let groups = r.blend.objects;
    for (cull_ids, split) in [
        (summary.other_cull_ids + groups, 1),
        (summary.other_cull_ids + groups - 1, 0),
    ] {
        let (w, _, log) = pool_planned(cull_ids);
        let plan = w.resource::<SkateBlendBudget>().unwrap();
        assert_eq!(plan.chosen.len(), split, "{cull_ids}");
        assert_eq!(
            log.approximations.values().sum::<u64>(),
            1 - split as u64,
            "{:?}",
            log.approximations
        );
        let e = EncodingOptions {
            cull_ids,
            ..EncodingOptions::default()
        };
        let (r, _) = write(&w, &format!("pool-{cull_ids}"), &e).unwrap();
        assert_eq!((r.blend.tiles_split, r.blend.tiles_shared), (split, 1 - split));
        assert!(
            r.unsupported.iter().all(|(what, _)| !what.contains("blend")),
            "{:?}",
            r.unsupported
        );
    }
    // the rest of the map's content takes IDs: one more light, and the room that held the chain form no longer does
    let lit = || {
        let mut w = chained();
        let light = w.spawn();
        w.insert(
            light,
            SkateLight {
                spot: false,
                position: [10.0, 1.0, -10.0],
                direction: [0.0, -1.0, 0.0],
                tangent: [0.0; 3],
                color: [1.0; 3],
                energy: 1.0,
                range: 5.0,
                radius: 0.0,
                spot_size: 0.0,
                spot_blend: 0.0,
                time_of_day: 0x7F,
                cast_shadow: false,
                priority: None,
                group: String::new(),
                unknown_flags: 0,
            },
        );
        w
    };
    let room = EncodingOptions {
        cull_ids: summary.other_cull_ids + groups,
        ..EncodingOptions::default()
    };
    let mut w2 = lit();
    SummarizeReskateMap::for_encoding(&room)
        .unwrap()
        .run(&mut w2, &mut PassLog::default())
        .unwrap();
    let lit_summary = w2
        .resource::<primase_reskate::components::SkateMapSummary>()
        .unwrap()
        .clone();
    assert_eq!(lit_summary.other_cull_ids, summary.other_cull_ids + 1);
    PlanReskateBlendBudget::pool()
        .run(&mut w2, &mut PassLog::default())
        .unwrap();
    assert!(w2.resource::<SkateBlendBudget>().unwrap().chosen.is_empty());
    let (r, _) = write(&w2, "pool-lit", &room).unwrap();
    assert_eq!((r.blend.tiles_split, r.blend.tiles_shared), (0, 1));
    // decided for one encoding, written with another
    let other = EncodingOptions {
        cull_ids: 0,
        ..EncodingOptions::default()
    };
    let e = write(&w, "pool-other-basis", &other).unwrap_err();
    assert!(e.contains("the blend budget was decided for"), "{e}");
}

/// The writer chooses no form of its own: blend chains with no budget are refused, as is a plan for other tiles;
/// objects=0 writes every tile plain; the pool without the map's summary is refused by the plan.
#[test]
fn the_writer_replays_only() {
    let (mut w, _, _) = pool_planned(EncodingOptions::default().cull_ids);
    w.resource_mut::<SkateBlendBudget>()
        .unwrap()
        .ranked
        .push("t_9_9".into());
    let e = write(&w, "other-plan", &EncodingOptions::default()).unwrap_err();
    assert!(e.contains("ranks other tiles"), "{e}");
    let e = write(&chained(), "unplanned", &EncodingOptions::default()).unwrap_err();
    assert!(e.contains("no budget decides their form"), "{e}");
    let mut w = chained();
    PlanReskateBlendBudget::objects(0)
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let (r, _) = write(&w, "objects-zero-plain", &EncodingOptions::default()).unwrap();
    assert_eq!((r.blend.tiles_split, r.blend.tiles_shared), (0, 1));
    let e = PlanReskateBlendBudget::pool()
        .run(&mut chained(), &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("run summarize-reskate-map first"), "{e}");
}

/// The budget is required, a count or the pool; it prints back and reads back the same.
#[test]
fn the_budget_options_round_trip_and_are_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    for given in [o(&[("objects", "pool")]), o(&[("objects", "12")])] {
        let pass = plan_reskate_blend_budget(&given).unwrap();
        assert_eq!(pass.options(), given);
    }
    for bad in [
        o(&[]),
        o(&[("objects", "-1")]),
        o(&[("objects", "all")]),
        o(&[("objects", "1"), ("grid", "200")]),
    ] {
        assert!(plan_reskate_blend_budget(&bad).is_err(), "{bad:?}");
    }
}
