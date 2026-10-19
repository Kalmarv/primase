//! The blend-chain objects are decided in finish, in key order, within a budget: `--blend-objects` when given, else
//! the level's share of the cull-ID pool less what the rest of the map takes. Tiles past it get one plain overlay
//! object each. Synthetic layered scene, made here, written from memory as the convert writer does.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Instance, Layer, LayerWeights, LayeredMaterial, Mesh, Name, Primitive, Semantic, Stream,
    Surfaces, Topology, Transform, WeightMapping, World,
};
use primase_reskate::layered::LowerLayeredReskate;
use primase_reskate::prepare::{RaiseOptions, SnapReskateMarkers};
use primase_reskate::{EncodingOptions, WriterChoices};

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
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "budget".into(),
            ..Default::default()
        },
    )
    .unwrap();
    LowerLayeredReskate::default()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    // the markers snapped before the writer, which snaps nothing
    SnapReskateMarkers::new()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    w
}

/// The map's object names, its stats' blend_order record and its mesh files, written with `blend_objects`.
fn build(blend_objects: Option<usize>, tag: &str) -> (Vec<String>, serde_json::Value, Vec<String>) {
    let out_root = tmp(tag);
    primase_reskate::write_map(
        &world(),
        &PathBuf::new(),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("budget".into()),
            ..EncodingOptions::default()
        },
        &WriterChoices {
            blend_objects,
            ..WriterChoices::default()
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
