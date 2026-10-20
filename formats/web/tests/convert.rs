//! Web tiles through core's conversion in parts ([`primase_core::convert::convert_parts`]): each cell's world, as its
//! passes prepared it, is what its tiles are written from.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_core::convert::{Reader, Report, Result, convert_parts};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline};
use primase_core::{
    Component, Entity, Instance, Marker, Mesh, Primitive, Semantic, Stream, Topology, Transform, World,
};
use primase_web::{Options, TileSize};

/// The world it is given, read whole; a part is the closure of its roots.
struct Given(World);

impl Reader for Given {
    fn read(&mut self) -> Result<(World, Report)> {
        Ok((self.0.clone(), Report::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// One prototype, a triangle 1 m wide, placed at each of `xs` (metres east).
fn world(xs: &[f32]) -> (World, Vec<Entity>) {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]]),
            )]),
            indices: Some(vec![0, 1, 2]),
            material: None,
        }]),
    );
    let placed = xs
        .iter()
        .map(|&x| {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3] = [x, 0., 0.];
            w.insert(e, t);
            e
        })
        .collect();
    (w, placed)
}

/// A marker 2 m above each placement of the part's own, made from that placement.
struct MarkPlacements;
impl Pass for MarkPlacements {
    fn name(&self) -> &'static str {
        "mark-placements"
    }
    fn provides(&self) -> &[&'static str] {
        &[Marker::NAME, Transform::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let placed: Vec<(Entity, Transform)> = w
            .iter::<Instance>()
            .map(|(e, _)| (e, *w.get::<Transform>(e).unwrap()))
            .collect();
        for (p, t) in placed {
            let m = w.spawn();
            w.insert(
                m,
                Marker {
                    kind: "travel".into(),
                    ground: None,
                    yaw: None,
                },
            );
            let mut at = Transform::IDENTITY;
            at.cols[3] = [t.cols[3][0], 2., t.cols[3][2]];
            w.insert(m, at);
            log.place_from(m, p);
        }
        Ok(())
    }
}

/// Every placement moved 1 m east.
struct MoveEast;
impl Pass for MoveEast {
    fn name(&self) -> &'static str {
        "move-east"
    }
    fn modifies(&self) -> &[&'static str] {
        &[Transform::NAME]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let placed: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        for e in placed {
            let mut t = *w.get::<Transform>(e).unwrap();
            t.cols[3][0] += 1.;
            w.insert(e, t);
        }
        Ok(())
    }
}

/// A fresh output folder for `name`.
fn folder(name: &str) -> PathBuf {
    let out = std::env::temp_dir().join(format!("primase-web-convert-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// `world` converted to web tiles in `out` with `options`, `passes` run on each cell.
fn convert(world: World, options: Options, out: &Path, passes: Vec<Box<dyn Pass>>) {
    let (mut parts, writer) = primase_web::convert::tiles(vec![(options, out.to_path_buf())]);
    convert_parts(
        &mut Given(world),
        &mut parts,
        Box::new(writer),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Pipeline { passes },
    )
    .unwrap();
}

/// The published index and each tile's glTF JSON and binary chunk.
fn tiles(out: &Path) -> (serde_json::Value, Vec<(serde_json::Value, Vec<u8>)>) {
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("tiles.json")).unwrap()).unwrap();
    let docs = index["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let bytes = std::fs::read(out.join(t["file"].as_str().unwrap())).unwrap();
            let chunks = primase_glb::parse(&bytes).unwrap();
            (
                serde_json::from_slice(chunks.json).unwrap(),
                chunks.bin.map(<[u8]>::to_vec).unwrap_or_default(),
            )
        })
        .collect();
    (index, docs)
}

/// The float VEC3s of accessor `a`.
fn vec3s(doc: &serde_json::Value, bin: &[u8], a: &serde_json::Value) -> Vec<[f32; 3]> {
    let a = &doc["accessors"][a.as_u64().unwrap() as usize];
    assert_eq!(
        (a["componentType"].as_u64(), a["type"].as_str()),
        (Some(5126), Some("VEC3"))
    );
    let view = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let start =
        view["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
    (0..a["count"].as_u64().unwrap() as usize)
        .map(|i| {
            std::array::from_fn(|k| {
                f32::from_le_bytes(bin[start + 12 * i + 4 * k..][..4].try_into().unwrap())
            })
        })
        .collect()
}

#[test]
fn a_marker_a_cells_pass_places_from_a_placement_is_written() {
    let (w, _) = world(&[10., 450.]);
    let out = folder("marker");
    let options = Options {
        compress: false,
        ..Options::new(TileSize::new(200.).unwrap())
    };
    convert(w, options, &out, vec![Box::new(MarkPlacements)]);
    let (index, docs) = tiles(&out);
    let mut markers = Vec::new();
    for (tile, (doc, _)) in index["tiles"].as_array().unwrap().iter().zip(&docs) {
        for node in doc["nodes"].as_array().unwrap() {
            if let Some(v) = node["extras"]["primase"]["web.viewer"]["value"].as_object() {
                markers.push((
                    tile["col"].as_i64().unwrap(),
                    v["kind"].clone(),
                    v["anchor"].clone(),
                ));
            }
        }
    }
    // one marker for each placement, written with its placement's cell
    assert_eq!(
        markers,
        [
            (0, "marker".into(), serde_json::json!([10., 2., 0.])),
            (2, "marker".into(), serde_json::json!([450., 2., 0.])),
        ]
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn placements_a_cells_pass_moves_are_instanced_where_they_were_moved_to() {
    let (w, _) = world(&[10., 20.]);
    let out = folder("moved");
    let options = Options {
        compress: false,
        ..Options::new(TileSize::new(200.).unwrap())
    };
    convert(w, options, &out, vec![Box::new(MoveEast)]);
    let (index, docs) = tiles(&out);
    let [(doc, bin)] = &docs[..] else {
        panic!("one tile: {index}")
    };
    let tile = &index["tiles"][0];
    assert_eq!(tile["instanced_placements"], 2);
    // the bounds and the instances: the moved transforms, not the planned ones
    assert_eq!(
        (tile["min"][0].as_f64(), tile["max"][0].as_f64()),
        (Some(11.), Some(22.))
    );
    let nodes = doc["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    let attributes = &nodes[0]["extensions"]["EXT_mesh_gpu_instancing"]["attributes"];
    assert!(nodes[0].get("matrix").is_none() && nodes[0].get("translation").is_none());
    assert_eq!(
        vec3s(doc, bin, &attributes["TRANSLATION"]),
        [[11., 0., 0.], [21., 0., 0.]]
    );
    let _ = std::fs::remove_dir_all(&out);
}
