//! The blend-chain objects are decided in finish, in key order, within a budget: `--blend-objects` when given, else
//! the level's share of the cull-ID pool less what the rest of the map takes. Tiles past it get one plain overlay
//! object each. Synthetic layered scene, made here, written from memory as the convert writer does.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Instance, Layer, LayerWeights, LayeredMaterial, Mesh, Name, Primitive, Semantic, Stream,
    Surfaces, Topology, Transform, WeightMapping, World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{
    BlendBudgetMode, CellCount, CellsCounted, SkateBlendBudget, SkateCellCounts, SkateMapSummary,
};
use primase_reskate::passes::SummarizeReskateMap;
use primase_reskate::passes::layered::{LowerLayeredReskate, OverlayLimit};
use primase_reskate::passes::{
    FinalizeReskateBlendBudget, PlanReskateBlendBudget, PrepareReskateBlendOrder, RaiseOptions,
    SnapReskateMarkers,
};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-blend-budget-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// Two layered triangles (layer 1 half visible at one corner), one in tile (0, 0) and one in tile (1, 0), over a
/// ground that spans both; raised and lowered (`lower-layered-reskate`), ready for the writer.
fn world() -> World {
    let mut w = World::new();
    let material = |w: &mut World, name: &str, base: [f32; 4]| {
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
        material(&mut w, "stone", [0.5, 0.5, 0.5, 1.0]),
        material(&mut w, "moss", [0.2, 0.5, 0.2, 1.0]),
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
    for (name, x) in [("left", 10.0f32), ("right", 210.0)] {
        let proto = w.spawn();
        w.insert(proto, Name(name.into()));
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                material: Some(blend),
                indices: None,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
                    ),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]),
                    ),
                    (
                        Semantic::Color(1),
                        Stream::F32x4(vec![[1., 0., 0., 0.], [1., 0., 0., 0.], [0.5, 0.5, 0., 0.]]),
                    ),
                ]),
            }]),
        );
        let at = w.spawn();
        w.insert(
            at,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [x, 0.0, -5.0];
        w.insert(at, t);
    }
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [-10.0, 0.0, 0.0],
            [390.0, 0.0, 0.0],
            [-10.0, 0.0, -50.0],
            [390.0, 0.0, -50.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    // on the ground beside the left triangle (east 20, north 20 = scene x 20, z -20)
    let spawn = w.spawn();
    w.insert(
        spawn,
        primase_core::Marker {
            kind: "spawn".into(),
            ground: Some([20.0, 20.0]),
            yaw: None,
        },
    );
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("budget", primase_reskate::passes::LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    LowerLayeredReskate::new(OverlayLimit::All)
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    // the markers snapped before the writer, which snaps nothing
    SnapReskateMarkers::for_default_encoding()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    // the chains split before the writer, which splits nothing: Studio's 200 m cells
    PrepareReskateBlendOrder::new(200.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    w
}

/// The chain-object budget `blend_objects` (`None`: the cull-ID pool) planned on `w`, as a program plans it.
fn plan_budget(w: &mut World, blend_objects: Option<usize>) {
    match blend_objects {
        Some(n) => PlanReskateBlendBudget::objects(n),
        None => {
            // the pool's room is decided from the map's summary
            SummarizeReskateMap::for_encoding(&EncodingOptions::default())
                .unwrap()
                .run(w, &mut PassLog::default())
                .unwrap();
            PlanReskateBlendBudget::pool()
        }
    }
    .run(w, &mut PassLog::default())
    .unwrap();
}

/// The map's object names, its stats' blend_order record and its mesh files, written with `blend_objects`.
fn build(blend_objects: Option<usize>, tag: &str) -> (Vec<String>, serde_json::Value, Vec<String>) {
    let out_root = tmp(tag);
    let mut w = world();
    plan_budget(&mut w, blend_objects);
    primase_reskate::write_map(
        &w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("budget".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    let read = |f: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(out_root.join("budget").join(f)).unwrap()).unwrap()
    };
    let names = read("map.json")["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["name"].as_str().map(str::to_owned))
        .collect();
    let files = std::fs::read_dir(out_root.join("budget/meshes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    (names, read("stats.json")["blend_order"].clone(), files)
}

#[test]
fn chain_objects_fill_the_budget_in_key_order_and_the_rest_are_plain() {
    // one object per tile's chain (one cell, one axis, one rank): a budget of one takes tile (0, 0), the first in key
    // order; tile (1, 0) gets one plain object
    let (names, stats, files) = build(Some(1), "capped");
    let chain = |tile: &str| names.iter().any(|n| n.starts_with(tile) && n.contains("_v"));
    let plain = |tile: &str| names.iter().any(|n| n.starts_with(&format!("{tile}_overlays")));
    assert!(chain("t_0_0_") && !plain("t_0_0"), "{names:?}");
    assert!(plain("t_1_0") && !chain("t_1_0_"), "{names:?}");
    // both forms were written per tile; the one not taken is gone
    let has = |prefix: &str| files.iter().any(|f| f.starts_with(prefix));
    assert!(!has("t_0_0_overlays") && has("t_1_0_overlays"), "{files:?}");
    assert!(has("t_0_0_c") && !has("t_1_0_c"), "{files:?}");
    assert_eq!(
        (
            &stats["cap"],
            &stats["cap_from_pool"],
            &stats["objects"],
            &stats["tiles_split"],
            &stats["tiles_shared"]
        ),
        (
            &serde_json::json!(1),
            &serde_json::json!(false),
            &serde_json::json!(1),
            &serde_json::json!(1),
            &serde_json::json!(1)
        ),
        "{stats}"
    );
    // the pool's budget (209,715 less the rest of this small map) takes both
    let (names, stats, _) = build(None, "pool");
    let chain = |tile: &str| names.iter().any(|n| n.starts_with(tile) && n.contains("_v"));
    assert!(chain("t_0_0_") && chain("t_1_0_"), "{names:?}");
    assert_eq!(stats["cap_from_pool"], serde_json::json!(true));
    let other = stats["other_cull_ids"].as_u64().unwrap();
    assert!(other > 0, "{stats}");
    assert_eq!(stats["cap"].as_u64().unwrap(), 209_715 - other);
    assert_eq!(stats["tiles_shared"], serde_json::json!(0));
}

/// The world with its map summarized for `cull_ids` (its collision cells counted, as a whole map's are).
fn summarized(cull_ids: usize) -> World {
    let mut w = world();
    SummarizeReskateMap::for_encoding(&EncodingOptions {
        cull_ids,
        ..EncodingOptions::default()
    })
    .unwrap()
    .run(&mut w, &mut PassLog::default())
    .unwrap();
    w
}

/// The budget `pass` planned on `w` as a program runs it (the pass manager checks what it requires), and what it
/// logged.
fn planned(pass: impl Pass + 'static, mut w: World) -> Result<(SkateBlendBudget, PassLog), String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            &mut w,
            &Pipeline {
                passes: vec![Box::new(pass)],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    let log = report.steps.pop().unwrap().log;
    Ok((w.resource::<SkateBlendBudget>().unwrap().clone(), log))
}

/// The whole map's pool budget on `w`.
fn pool_plan(w: World) -> Result<(SkateBlendBudget, PassLog), String> {
    planned(PlanReskateBlendBudget::pool(), w)
}

/// The pool budget of a map in parts, from its finalize stage's `w`.
fn pool_finalize(w: World) -> Result<(SkateBlendBudget, PassLog), String> {
    planned(FinalizeReskateBlendBudget::pool(), w)
}

/// What a map in parts' finalize stage holds for the map `whole` summarizes: its summary with `cells` cull IDs of it
/// counted apart, and the collision cells' counts holding them (split over two cells).
fn finalize_world(whole: &SkateMapSummary, cells: usize) -> World {
    let mut w = World::new();
    w.insert_resource(SkateMapSummary {
        summed: whole.summed - cells,
        cells: CellsCounted::Apart,
        ..whole.clone()
    });
    w.insert_resource(SkateCellCounts {
        cells: BTreeMap::from([
            (
                (false, 0, 0),
                CellCount {
                    pieces: 1,
                    cull_ids: cells / 2,
                },
            ),
            (
                (true, 1, 0),
                CellCount {
                    pieces: 1,
                    cull_ids: cells - cells / 2,
                },
            ),
        ]),
    });
    w
}

/// A map in parts decides its budget, in the finalize stage, as the whole map does: from its summary (the parts'
/// content, their tiles with a chain form) and its cells' counts, the same decision with the same basis, and the same
/// approximation logged, with room for every tile's chain objects, one ID short of it, and with none.
#[test]
fn the_finalize_stage_decides_as_the_whole_map() {
    let whole = summarized(1_000_000);
    let summary = whole.resource::<SkateMapSummary>().unwrap().clone();
    assert_eq!(summary.cells, CellsCounted::Included);
    assert_eq!(
        summary.chains.keys().collect::<Vec<_>>(),
        ["t_0_0", "t_1_0"],
        "the summary carries the chain tiles: {summary:?}"
    );
    let other = summary.other_cull_ids();
    assert!(other > 2, "{summary:?}");
    // the boundary: the fewest cull IDs with which both tiles take the chain form
    let chosen = |cull_ids: usize| {
        pool_plan(summarized(cull_ids))
            .unwrap()
            .0
            .decision
            .unwrap()
            .chosen
            .len()
    };
    let mut boundary = other;
    while chosen(boundary) < 2 {
        boundary += 1;
    }
    let mut saturated = boundary;
    while chosen(saturated) > 0 {
        saturated -= 1;
    }
    for (cull_ids, takes) in [(boundary, 2), (boundary - 1, 1), (saturated, 0)] {
        let w = summarized(cull_ids);
        let s = w.resource::<SkateMapSummary>().unwrap().clone();
        let (expected, expected_log) = pool_plan(w).unwrap();
        let d = expected.decision.as_ref().unwrap();
        assert_eq!((d.ranked.len(), d.chosen.len()), (2, takes), "{cull_ids}: {d:?}");
        assert_eq!(
            expected_log.approximations.values().sum::<u64>(),
            2 - takes as u64,
            "{cull_ids}: {expected_log:?}"
        );
        let (made, log) = pool_finalize(finalize_world(&s, 2)).unwrap();
        assert_eq!(made, expected, "{cull_ids}");
        assert_eq!(log.approximations, expected_log.approximations, "{cull_ids}");
        assert_eq!(log.counts, expected_log.counts, "{cull_ids}");
    }
    // a fixed count over parts: the same tiles, no basis
    let (made, log) = planned(
        FinalizeReskateBlendBudget::objects(1),
        finalize_world(&summary, 2),
    )
    .unwrap();
    let (expected, expected_log) = planned(PlanReskateBlendBudget::objects(1), world()).unwrap();
    assert_eq!(made, expected);
    assert_eq!(log.approximations, expected_log.approximations);
    assert_eq!(made.decision.unwrap().chosen.len(), 1);
}

/// The budget is decided over the whole map only: a part is refused by both passes, the finalize pass refuses
/// summaries without the cells' counts and a whole map's summary, and the whole map's pass a part's summary.
#[test]
fn the_budget_refuses_a_part_and_summaries_without_cell_counts() {
    let scope = primase_core::convert::PartScope {
        roots: Default::default(),
        shared: Default::default(),
        context: Default::default(),
        whole: false,
        area: None,
        first_made: None,
        placed: BTreeMap::new(),
    };
    let mut part = summarized(1_000_000);
    part.insert_resource(scope.clone());
    let e = pool_plan(part).unwrap_err();
    assert!(
        e.contains("decided once over the whole map, by finalize-reskate-blend-budget"),
        "{e}"
    );
    let summary = summarized(1_000_000)
        .resource::<SkateMapSummary>()
        .unwrap()
        .clone();
    let mut part = finalize_world(&summary, 2);
    part.insert_resource(scope);
    let e = pool_finalize(part).unwrap_err();
    assert!(
        e.contains("decided once over the whole map, by finalize-reskate-blend-budget"),
        "{e}"
    );
    let mut no_cells = finalize_world(&summary, 2);
    no_cells.remove_resource::<SkateCellCounts>();
    let e = pool_finalize(no_cells).unwrap_err();
    assert!(e.contains("requires `skate.cell_counts`"), "{e}");
    let mut whole_summary = finalize_world(&summary, 2);
    whole_summary.insert_resource(summary.clone());
    let e = pool_finalize(whole_summary).unwrap_err();
    assert!(
        e.contains("it is a whole map's, budgeted by plan-reskate-blend-budget"),
        "{e}"
    );
    let mut apart = summarized(1_000_000);
    apart.insert_resource(SkateMapSummary {
        cells: CellsCounted::Apart,
        ..summary
    });
    let e = pool_plan(apart).unwrap_err();
    assert!(e.contains("budgeted by finalize-reskate-blend-budget"), "{e}");
    let e = pool_plan(World::new()).unwrap_err();
    assert!(e.contains("requires `skate.scene`"), "{e}");
}

/// The writer replays a decided budget and decides none: a budget with its mode and no decision is refused.
#[test]
fn a_writer_given_an_undecided_budget_refuses() {
    let mut w = world();
    w.insert_resource(SkateBlendBudget {
        mode: BlendBudgetMode::Pool,
        decision: None,
    });
    let e = primase_reskate::write_map(
        &w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: tmp("undecided"),
            name: Some("budget".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("states its mode and no decision"), "{e}");
}

/// A map's cull IDs are counted without its spawn (a part of a map in several may hold none, and a marker takes no cull
/// ID), and the writer still refuses a map without one.
#[test]
fn the_summary_counts_without_a_spawn_and_the_writer_refuses_none() {
    let mut w = world();
    let spawns: Vec<_> = w
        .iter::<primase_reskate::components::SkateMarker>()
        .filter(|(_, m)| m.kind == primase_reskate::components::MarkerKind::Spawn)
        .map(|(e, _)| e)
        .collect();
    assert!(!spawns.is_empty());
    let counted = summarized(1_000_000)
        .resource::<SkateMapSummary>()
        .unwrap()
        .clone();
    for e in spawns {
        w.despawn(e);
    }
    SummarizeReskateMap::for_encoding(&EncodingOptions {
        cull_ids: 1_000_000,
        ..EncodingOptions::default()
    })
    .unwrap()
    .run(&mut w, &mut PassLog::default())
    .unwrap();
    assert_eq!(w.resource::<SkateMapSummary>(), Some(&counted));
    PlanReskateBlendBudget::pool()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let e = primase_reskate::write_map(
        &w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: tmp("no-spawn"),
            name: Some("budget".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("the map has no spawn marker"), "{e}");
}
