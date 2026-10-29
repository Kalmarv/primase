//! A map does not depend on the order its world was built in (PRIMASE-PLAN step 9): the same prepared world, its
//! materials, prototypes, placements, lights and grind curves made in three orders (so every entity id differs), goes
//! through the ReSkate preparation passes in the pass manager and the writer, and the three map folders are the same
//! bytes. The world has what an order could reach: a prototype instanced in several tiles, a second prototype with the
//! same content and material, a material shared by prototypes, layered blends in two tiles and one chain across the
//! 200 m cell border, a blend budget too small for every tile's chain (some tiles take the plain form), a tile with
//! only an empty placement, lights past a per-cell cap and grind curves. Synthetic, made here.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pass::{Pass, PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Entity, Instance, Layer, LayerWeights, LayeredMaterial, Light, LightKind, Mesh, Name,
    Primitive, Semantic, Stream, Surfaces, Topology, Transform, WeightMapping, World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::SkateGrindCurve;
use primase_reskate::passes::SummarizeReskateMap;
use primase_reskate::passes::{PlanReskateBlendBudget, RaiseOptions};

/// The orders a list of `n` things is made in: as listed, reversed, and rotated by one.
#[derive(Clone, Copy, Debug)]
enum Order {
    Listed,
    Reversed,
    Rotated,
}

impl Order {
    fn of(self, n: usize) -> Vec<usize> {
        let mut v: Vec<usize> = (0..n).collect();
        match self {
            Order::Listed => {}
            Order::Reversed => v.reverse(),
            Order::Rotated => v.rotate_left(1),
        }
        v
    }
}

fn material(w: &mut World, name: &str, base: [f32; 4]) -> Entity {
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
    m
}

/// A layered strip `length` m long (east) and 1 m wide, cut into 1 m squares, drawn with `blend`: layer 1 half
/// visible along its north edge.
fn strip(blend: Entity, length: u32) -> Mesh {
    let (mut pos, mut uv, mut weights, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..=length {
        for (j, w1) in [(0u32, 0.0f32), (1, 0.5)] {
            pos.push([i as f32, 0.0, -(j as f32)]);
            uv.push([i as f32, j as f32]);
            weights.push([1.0 - w1, w1, 0.0, 0.0]);
        }
    }
    for i in 0..length {
        let a = i * 2;
        idx.extend_from_slice(&[a, a + 2, a + 1, a + 1, a + 2, a + 3]);
    }
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        material: Some(blend),
        indices: Some(idx),
        streams: BTreeMap::from([
            (Semantic::Position, Stream::F32x3(pos)),
            (Semantic::TexCoord(0), Stream::F32x2(uv)),
            (Semantic::Color(1), Stream::F32x4(weights)),
        ]),
    }])
}

/// A 10 x 10 quad grid (200 triangles, over the instancing threshold of 150) drawn with `material`.
fn grid(material: Entity) -> Mesh {
    let (mut pos, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=10 {
        for i in 0..=10 {
            pos.push([i as f32 * 0.1, 0.0, -(j as f32) * 0.1]);
            uv.push([i as f32 * 0.1, j as f32 * 0.1]);
        }
    }
    for j in 0..10u32 {
        for i in 0..10u32 {
            let a = j * 11 + i;
            idx.extend_from_slice(&[a, a + 1, a + 11, a + 11, a + 1, a + 12]);
        }
    }
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (Semantic::Position, Stream::F32x3(pos)),
            (Semantic::TexCoord(0), Stream::F32x2(uv)),
        ]),
        indices: Some(idx),
        material: Some(material),
    }])
}

/// What a prototype draws.
#[derive(Clone, Copy)]
enum Draws {
    /// A layered strip this many metres long.
    Blend(u32),
    /// The 10 x 10 grid with the named material.
    Grid(&'static str),
    /// Nothing.
    Empty,
}

/// The prototypes, by name.
const PROTOTYPES: [(&str, Draws); 7] = [
    ("left", Draws::Blend(4)),
    ("right", Draws::Blend(4)),
    ("border", Draws::Blend(20)),
    ("bench", Draws::Grid("grey")),
    ("bench copy", Draws::Grid("grey")),
    ("red bench", Draws::Grid("red")),
    ("empty", Draws::Empty),
];

/// The placements: prototype name, east, north. Tile (0, 0) has "left", tile (1, 0) "right", "border" runs from
/// east 190 to 210 across the cell border; the benches are instanced over tiles 0 to 2; tile (4, 0) has only the
/// empty placement.
const PLACEMENTS: [(&str, f32, f32); 12] = [
    ("left", 10.0, 5.0),
    ("right", 260.0, 5.0),
    ("border", 190.0, 20.0),
    ("bench", 30.0, 40.0),
    ("bench", 230.0, 40.0),
    ("bench", 430.0, 40.0),
    ("bench copy", 60.0, 60.0),
    ("bench copy", 270.0, 60.0),
    ("bench copy", 470.0, 60.0),
    ("red bench", 90.0, 80.0),
    ("red bench", 120.0, 80.0),
    ("empty", 810.0, 5.0),
];

/// Point lights, east, north, intensity: four in the first 200 m cell (two past a cap of two, two of them alike but
/// for their intensity), one in the second.
const LIGHTS: [(f32, f32, f32); 5] = [
    (20.0, 20.0, 5.0),
    (40.0, 20.0, 5.0),
    (40.0, 20.0, 7.0),
    (60.0, 20.0, 3.0),
    (250.0, 20.0, 5.0),
];

/// The world, made in `order`, raised: ground over east -10..1000, north -10..100, the spawn beside "left".
fn world(order: Order) -> World {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let mut materials: BTreeMap<&str, Entity> = BTreeMap::new();
    let listed = [
        ("stone", [0.5, 0.5, 0.5, 1.0]),
        ("moss", [0.2, 0.5, 0.2, 1.0]),
        ("grey", [0.6, 0.6, 0.6, 1.0]),
        ("red", [0.8, 0.1, 0.1, 1.0]),
    ];
    for i in order.of(listed.len()) {
        let (name, base) = listed[i];
        materials.insert(name, material(&mut w, name, base));
    }
    let blend = w.spawn();
    w.insert(blend, Name("stone and moss".into()));
    w.insert(
        blend,
        LayeredMaterial {
            layers: ["stone", "moss"]
                .map(|n| {
                    Some(Layer {
                        material: materials[n],
                        texcoord: None,
                        transform: None,
                    })
                })
                .to_vec(),
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let mut prototypes: BTreeMap<&str, Entity> = BTreeMap::new();
    for i in order.of(PROTOTYPES.len()) {
        let (name, draws) = PROTOTYPES[i];
        let p = w.spawn();
        w.insert(p, Name(name.into()));
        w.insert(
            p,
            match draws {
                Draws::Blend(length) => strip(blend, length),
                Draws::Grid(m) => grid(materials[m]),
                Draws::Empty => Mesh::new(Vec::new()),
            },
        );
        prototypes.insert(name, p);
    }
    for i in order.of(PLACEMENTS.len()) {
        let (name, east, north) = PLACEMENTS[i];
        let at = w.spawn();
        w.insert(
            at,
            Instance {
                prototype: prototypes[name],
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 0.0, -north];
        w.insert(at, t);
    }
    for i in order.of(LIGHTS.len()) {
        let (east, north, intensity) = LIGHTS[i];
        let l = w.spawn();
        w.insert(
            l,
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity,
                range: Some(10.0),
                cast_shadows: None,
                radius: None,
                specular: None,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 3.0, -north];
        w.insert(l, t);
    }
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [-10.0, 0.0, 10.0],
            [1000.0, 0.0, 10.0],
            [-10.0, 0.0, -100.0],
            [1000.0, 0.0, -100.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    let spawn = w.spawn();
    w.insert(
        spawn,
        primase_core::Marker {
            kind: "spawn".into(),
            ground: Some([20.0, 30.0]),
            yaw: None,
        },
    );
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "orders",
            primase_reskate::passes::LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
    // two rails 1 m up, along east (scene x), in the first and second cells
    let rails = [
        vec![[100.0, 1.0, -50.0], [104.0, 1.0, -50.0]],
        vec![[300.0, 1.0, -50.0], [306.0, 1.0, -50.0]],
    ];
    for i in order.of(rails.len()) {
        let e = w.spawn();
        w.insert(
            e,
            SkateGrindCurve {
                points: rails[i].clone(),
                closed: false,
                kind: 0,
                radius: 0.0,
                surface: 0,
                unknown_flags: 0,
            },
        );
    }
    w
}

/// The ReSkate recipe a program runs on the raised world, in the pass manager.
const RECIPE: &str = "lower-layered-reskate{overlays=all}, prepare-reskate-textures, prepare-reskate-grinds, snap-reskate-markers, \
                      prepare-reskate-water{extent=content,margin=400}, \
                      prepare-reskate-pause-map{from=geometry,height=256,extent=content,leave-out=nothing,colour=mean}, \
                      select-reskate-lights{per_cell=2,cell=200}, prepare-reskate-overlays{step=0.004}, \
                      prepare-reskate-blend-order{grid=200,order=tie}";

/// Every file of a folder, by its path below it.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// The map folder of the world made in `order`, prepared and written with at most `blend_objects` blend-chain
/// objects, as files; each order written to the same folder in turn, so no path differs.
fn map(order: Order, blend_objects: Option<usize>) -> BTreeMap<String, Vec<u8>> {
    let mut w = world(order);
    let mut registry = PassRegistry::new();
    primase_reskate::register(&mut registry);
    let pipeline = registry.parse(RECIPE).unwrap();
    PassManager::new(registry)
        .run(&mut w, &pipeline, &mut RunReport::default())
        .unwrap();
    // the budget, planned last
    match blend_objects {
        Some(n) => PlanReskateBlendBudget::objects(n),
        None => {
            // the pool's room is decided from the map's summary
            SummarizeReskateMap::for_encoding(&EncodingOptions::default())
                .unwrap()
                .run(&mut w, &mut primase_core::pass::PassLog::default())
                .unwrap();
            PlanReskateBlendBudget::pool()
        }
    }
    .run(&mut w, &mut primase_core::pass::PassLog::default())
    .unwrap();
    let out_root: PathBuf =
        std::env::temp_dir().join(format!("primase-reskate-orders-{}", std::process::id()));
    if out_root.exists() {
        std::fs::remove_dir_all(&out_root).unwrap();
    }
    primase_reskate::write_map(
        &w,
        Path::new(""),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("orders".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    let f = files(&out_root.join("orders"));
    std::fs::remove_dir_all(&out_root).unwrap();
    f
}

fn json(files: &BTreeMap<String, Vec<u8>>, name: &str) -> serde_json::Value {
    serde_json::from_slice(&files[name]).unwrap()
}

/// Where two JSON values differ, as paths (at most `max`).
fn json_paths(a: &serde_json::Value, b: &serde_json::Value, at: String, out: &mut Vec<String>, max: usize) {
    if out.len() >= max || a == b {
        return;
    }
    match (a, b) {
        (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let null = serde_json::Value::Null;
                json_paths(
                    x.get(k).unwrap_or(&null),
                    y.get(k).unwrap_or(&null),
                    format!("{at}/{k}"),
                    out,
                    max,
                );
            }
        }
        (serde_json::Value::Array(x), serde_json::Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                json_paths(p, q, format!("{at}/{i}"), out, max);
            }
        }
        _ => out.push(format!("{at}: {a} / {b}")),
    }
}

/// What differs in the JSON files two map folders both have.
fn json_differences(
    a: &BTreeMap<String, Vec<u8>>,
    b: &BTreeMap<String, Vec<u8>>,
    files: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for f in files.iter().filter(|f| f.ends_with(".json")) {
        if let (Some(x), Some(y)) = (a.get(f), b.get(f)) {
            let mut paths = Vec::new();
            json_paths(
                &serde_json::from_slice(x).unwrap(),
                &serde_json::from_slice(y).unwrap(),
                f.clone(),
                &mut paths,
                16,
            );
            out.extend(paths);
        }
    }
    out
}

/// The files that differ between two map folders, or are in one only.
fn differing(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut names: Vec<&String> = a.keys().chain(b.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|n| a.get(*n) != b.get(*n))
        .cloned()
        .collect()
}

#[test]
fn the_map_is_the_same_whatever_order_its_world_was_made_in() {
    for budget in [Some(1), None] {
        let listed = map(Order::Listed, budget);
        let stats = json(&listed, "stats.json");
        let blend = &stats["blend_order"];
        // what the world was made to reach
        if budget.is_some() {
            assert!(blend["tiles_shared"].as_u64().unwrap() > 0, "saturated: {blend}");
        }
        assert!(blend["tiles_split"].as_u64().unwrap() > 0, "{blend}");
        assert!(
            listed.keys().any(|f| f.starts_with("meshes/p_")),
            "an instanced prototype"
        );
        for order in [Order::Reversed, Order::Rotated] {
            let other = map(order, budget);
            let other_blend = json(&other, "stats.json")["blend_order"].clone();
            for k in ["objects", "tiles_split", "tiles_shared"] {
                assert_eq!(
                    blend[k], other_blend[k],
                    "{order:?}, budget {budget:?}: blend_order {k}"
                );
            }
            let d = differing(&listed, &other);
            assert!(
                d.is_empty(),
                "{order:?}, budget {budget:?}: {} files differ: {d:?}; {:#?}",
                d.len(),
                json_differences(&listed, &other, &d)
            );
        }
    }
}
