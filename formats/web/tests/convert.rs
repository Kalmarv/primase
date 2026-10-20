//! Web tiles through core's conversion in parts ([`primase_core::convert::convert_parts`]): each cell's world, as its
//! passes prepared it, is what its tiles are written from.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_core::convert::{NoticeKind, Reader, Report, Result, convert_parts};
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
        &["core", "test"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// A triangle 1 m wide.
fn triangle() -> Mesh {
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]]),
        )]),
        indices: Some(vec![0, 1, 2]),
        material: None,
    }])
}

/// One prototype, a triangle 1 m wide, placed at each of `xs` (metres east).
fn world(xs: &[f32]) -> (World, Vec<Entity>) {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, triangle());
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

/// Each placement turned into a marker where it is: its instance taken away, a marker put on it.
struct PlacementsToMarkers;
impl Pass for PlacementsToMarkers {
    fn name(&self) -> &'static str {
        "placements-to-markers"
    }
    fn provides(&self) -> &[&'static str] {
        &[Marker::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[Instance::NAME]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let placed: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        for e in placed {
            w.remove::<Instance>(e);
            w.insert(
                e,
                Marker {
                    kind: "travel".into(),
                    ground: None,
                    yaw: None,
                },
            );
        }
        Ok(())
    }
}

/// Each marker turned into a mesh placed as itself: its marker taken away, a triangle put on it.
struct MarkersToMeshes;
impl Pass for MarkersToMeshes {
    fn name(&self) -> &'static str {
        "markers-to-meshes"
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[Marker::NAME]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let marked: Vec<Entity> = w.iter::<Marker>().map(|(e, _)| e).collect();
        for e in marked {
            w.remove::<Marker>(e);
            w.insert(e, triangle());
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

/// `world` converted to web tiles in `out` with `options`, `passes` run on each cell; what was reported.
fn convert(
    world: World,
    options: Options,
    out: &Path,
    passes: Vec<Box<dyn Pass>>,
) -> primase_core::convert::ConvertReport {
    let (mut parts, writer) = primase_web::convert::tiles(vec![(options, out.to_path_buf())]).unwrap();
    convert_parts(
        &mut Given(world),
        &mut parts,
        Box::new(writer),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Pipeline { passes },
    )
    .unwrap()
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

/// The viewer records the published tiles hold: each its tile's column, kind and anchor.
fn records(out: &Path) -> Vec<(i64, serde_json::Value, serde_json::Value)> {
    let (index, docs) = tiles(out);
    let mut records = Vec::new();
    for (tile, (doc, _)) in index["tiles"].as_array().unwrap().iter().zip(&docs) {
        for node in doc["nodes"].as_array().unwrap() {
            if let Some(v) = node["extras"]["primase"]["web.viewer"]["value"].as_object() {
                records.push((
                    tile["col"].as_i64().unwrap(),
                    v["kind"].clone(),
                    v["anchor"].clone(),
                ));
            }
        }
    }
    records
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
    // one marker for each placement, written with its placement's cell
    assert_eq!(
        records(&out),
        [
            (0, "marker".into(), serde_json::json!([10., 2., 0.])),
            (2, "marker".into(), serde_json::json!([450., 2., 0.])),
        ]
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_placement_a_cells_pass_turns_into_a_marker_is_written_as_the_marker() {
    let (w, _) = world(&[10., 450.]);
    let out = folder("to-marker");
    let options = Options {
        compress: false,
        ..Options::new(TileSize::new(200.).unwrap())
    };
    convert(w, options, &out, vec![Box::new(PlacementsToMarkers)]);
    // each cell's placement, now a marker where it was, and nothing drawn
    assert_eq!(
        records(&out),
        [
            (0, "marker".into(), serde_json::json!([10., 0., 0.])),
            (2, "marker".into(), serde_json::json!([450., 0., 0.])),
        ]
    );
    let (index, _) = tiles(&out);
    assert!(
        index["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["placements"] == 0),
        "{index}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_marker_a_cells_pass_turns_into_a_mesh_is_drawn_as_placed() {
    let mut w = World::new();
    for x in [10., 450.] {
        let e = w.spawn();
        w.insert(
            e,
            Marker {
                kind: "travel".into(),
                ground: None,
                yaw: None,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [x, 0., 0.];
        w.insert(e, t);
    }
    let out = folder("to-mesh");
    let options = Options {
        compress: false,
        ..Options::new(TileSize::new(200.).unwrap())
    };
    convert(w, options, &out, vec![Box::new(MarkersToMeshes)]);
    // each cell's marker, now a triangle placed as itself where it was, and no record
    assert_eq!(records(&out), []);
    let (index, _) = tiles(&out);
    let drawn: Vec<(i64, u64, Option<f64>)> = index["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["col"].as_i64().unwrap(),
                t["placements"].as_u64().unwrap(),
                t["min"][0].as_f64(),
            )
        })
        .collect();
    assert_eq!(drawn, [(0, 1, Some(10.)), (2, 1, Some(450.))]);
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

#[test]
fn a_material_without_pbr_is_reported_as_unsupported() {
    use primase_core::{Opaque, OpaqueData};
    let (mut w, _) = world(&[10., 450.]);
    // the material is a source's own, not lowered to pbr.material: the tiles draw its primitives with none
    let material = w.spawn();
    w.insert_opaque(
        material,
        "test.shader",
        Opaque {
            version: 1,
            data: OpaqueData::Json(serde_json::json!({"shader": "glass"})),
        },
    );
    let proto = w.iter::<Mesh>().map(|(e, _)| e).next().unwrap();
    *w.get_mut::<Mesh>(proto)
        .unwrap()
        .primitive_mut(0)
        .unwrap()
        .material_mut() = Some(material);
    let out = folder("material");
    let options = Options {
        compress: false,
        ..Options::new(TileSize::new(200.).unwrap())
    };
    let report = convert(w, options, &out, Vec::new());
    let (index, _) = tiles(&out);
    let files: Vec<String> = index["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["file"].as_str().unwrap().to_owned())
        .collect();
    let notice = report
        .reported
        .iter()
        .find(|n| n.reason.starts_with("materials without pbr.material"))
        .unwrap_or_else(|| panic!("no notice of the material: {:?}", report.reported));
    // once in each of the two tiles that draw it
    assert_eq!(
        (notice.stage.as_str(), notice.kind, notice.count),
        ("web", NoticeKind::Unsupported, 2)
    );
    assert_eq!(notice.examples, files);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn tile_sets_must_be_given_and_planned_alike() {
    let options = Options::new(TileSize::new(200.).unwrap());
    let err = primase_web::convert::tiles(Vec::new()).unwrap_err();
    assert!(err.to_string().contains("no tile set"), "{err}");
    // 100 m and 200 m cells are two plans: one traversal cannot write both
    let finer = Options::new(TileSize::new(100.).unwrap());
    let err =
        primase_web::convert::tiles(vec![(options.clone(), "a".into()), (finer, "b".into())]).unwrap_err();
    assert!(err.to_string().contains("tile size"), "{err}");
    let recorded = Options {
        viewer_items: vec![primase_web::ViewerItem {
            name: "own".into(),
            kind: "marker".into(),
            anchor: [450., 0., 0.],
            data: serde_json::Value::Null,
        }],
        ..options.clone()
    };
    let err =
        primase_web::convert::tiles(vec![(options.clone(), "a".into()), (recorded, "b".into())]).unwrap_err();
    assert!(err.to_string().contains("viewer records"), "{err}");
}

#[test]
fn tile_sets_differing_in_encoding_alone_share_the_plan() {
    let (w, _) = world(&[10., 20., 450.]);
    let (a, b) = (folder("set-a"), folder("set-b"));
    let compressed = Options::new(TileSize::new(200.).unwrap());
    let plain = Options {
        compress: false,
        gpu_instancing: false,
        tile_byte_budget: Some(1),
        ..compressed.clone()
    };
    let (mut parts, writer) =
        primase_web::convert::tiles(vec![(compressed, a.clone()), (plain, b.clone())]).unwrap();
    convert_parts(
        &mut Given(w),
        &mut parts,
        Box::new(writer),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Pipeline { passes: Vec::new() },
    )
    .unwrap();
    let cells = |out: &Path| -> Vec<(i64, i64, u64)> {
        let (index, _) = tiles(out);
        index["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["col"].as_i64().unwrap(),
                    t["row"].as_i64().unwrap(),
                    t["placements"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    // the same cells; the plain set divides its first cell under its own budget
    assert_eq!(cells(&a), [(0, 0, 2), (2, 0, 1)]);
    assert_eq!(cells(&b), [(0, 0, 1), (0, 0, 1), (2, 0, 1)]);
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}
