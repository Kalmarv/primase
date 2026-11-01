//! What the writer writes of lights reads back: each `KHR_lights_punctual` light a `core.light` equal to the one
//! written (kind, colour, intensity, range, a spot's cones) at its node's world transform. An optional extension or an
//! `extras` key the reader does not read is counted by name, never refused; a required extension is refused, and a
//! light the extension states outside its ranges is an error naming it.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;

use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Light, LightKind, Mesh, Name, Transform, World};
use primase_gltf::{ReadReport, WriteOptions, read_glb, write};
use serde_json::{Value, json};

/// A turn of `angle` about y, at `at`.
fn placed(angle: f32, at: [f32; 3]) -> Transform {
    let (s, c) = angle.sin_cos();
    Transform {
        cols: [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], at],
    }
}

/// A prototype placed twice; a point light, a spot and a sun (glTF states no light's shadows: none is stated).
fn world() -> World {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, Mesh::new(vec![common::rich_primitive(Some(m), 0.0)]));
    for (name, x) in [("proxy", 0.0), ("plain", 2.0)] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(e, placed(0.0, [x, 0.0, 0.0]));
        w.insert(e, Name(name.to_owned()));
    }
    let lights = [
        (
            "torch",
            Light {
                kind: LightKind::Point,
                color: [0.9, 0.5, 0.25],
                intensity: 0.5,
                range: Some(8.0),
                cast_shadows: None,
                radius: None,
                specular: None,
            },
            placed(0.0, [1.0, 2.0, 3.0]),
        ),
        (
            "lamp",
            Light {
                kind: LightKind::Spot {
                    inner_cone: 0.25,
                    outer_cone: 0.5,
                },
                color: [1.0; 3],
                intensity: 40.0,
                range: Some(12.0),
                cast_shadows: None,
                radius: None,
                specular: None,
            },
            placed(1.0, [-4.0, 5.0, 0.5]),
        ),
        (
            "sun",
            Light {
                kind: LightKind::Directional,
                color: [0.96, 1.0, 0.98],
                intensity: 0.4,
                range: None,
                cast_shadows: None,
                radius: None,
                specular: None,
            },
            placed(-0.5, [0.0; 3]),
        ),
    ];
    for (name, light, at) in lights {
        let e = w.spawn();
        w.insert(e, light);
        w.insert(e, at);
        w.insert(e, Name(name.to_owned()));
    }
    w
}

fn glb(w: &World) -> Vec<u8> {
    write(w, &WriteOptions::default()).unwrap().0
}

/// The document of a GLB, and the GLB of a document with the same binary chunk.
fn document(glb: &[u8]) -> (Value, Vec<u8>) {
    let chunks = primase_glb::parse(glb).unwrap();
    (
        serde_json::from_slice(chunks.json).unwrap(),
        chunks.bin.unwrap_or_default().to_vec(),
    )
}

fn assembled(doc: &Value, bin: &[u8]) -> Vec<u8> {
    primase_glb::assemble(&serde_json::to_vec(doc).unwrap(), bin).unwrap()
}

fn read(glb: &[u8]) -> Result<(World, ReadReport), String> {
    let mut w = World::new();
    let report = read_glb(glb, "readback.glb", &mut w).map_err(|e| e.to_string())?;
    Ok((w, report))
}

/// Each named entity's component `T`, by name.
fn by_name<T: primase_core::Component + Clone>(w: &World) -> BTreeMap<String, T> {
    w.iter::<T>()
        .map(|(e, c)| (w.get::<Name>(e).unwrap().0.clone(), c.clone()))
        .collect()
}

#[test]
fn lights_read_back_equal_at_their_place() {
    let original = world();
    let (back, report) = read(&glb(&original)).unwrap();
    assert_eq!(
        (
            report.lights,
            report.lights_unplaced,
            report.lights_at_stored_pose
        ),
        (3, 0, 0)
    );
    assert_eq!(by_name::<Light>(&back), by_name::<Light>(&original));
    let placed_at = |w: &World| -> BTreeMap<String, Transform> {
        w.iter::<Light>()
            .map(|(e, _)| {
                (
                    w.get::<Name>(e).unwrap().0.clone(),
                    *w.get::<Transform>(e).unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(placed_at(&back), placed_at(&original));
    assert!(
        report.extensions_not_read.is_empty(),
        "{:?}",
        report.extensions_not_read
    );
}

/// The writer's group node (one per prototype by default) is read as nothing of its own: counted, with a notice; its
/// placements are where it put them. Written without groups, every node is read as something.
#[test]
fn group_nodes_are_counted_as_not_read() {
    let original = world();
    let (back, report) = read(&glb(&original)).unwrap();
    assert_eq!(report.nodes_not_read, 1);
    assert!(
        report
            .notices()
            .iter()
            .any(|n| n.reason.contains("no joint, rig, mesh, light or camera") && n.count == 1),
        "{:?}",
        report.notices()
    );
    assert_eq!(by_name::<Transform>(&back), by_name::<Transform>(&original));
    let options = WriteOptions {
        group_by: primase_gltf::GroupBy::None,
        ..WriteOptions::default()
    };
    let (_, report) = read(&write(&original, &options).unwrap().0).unwrap();
    assert_eq!(report.nodes_not_read, 0);
}

/// Every optional extension the reader does not read is counted by name, wherever the reader has a home for it; the
/// document still reads, with its lights.
#[test]
fn unknown_optional_extensions_are_counted_not_refused() {
    let (mut doc, bin) = document(&glb(&world()));
    doc["extensions"]["EXT_document"] = json!({});
    doc["scenes"][0]["extensions"] = json!({"EXT_scene": {}});
    let light_node = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|n| n["extensions"].get("KHR_lights_punctual").is_some())
        .unwrap();
    doc["nodes"][light_node]["extensions"]["EXT_node"] = json!({"x": 1});
    doc["meshes"][0]["extensions"] = json!({"EXT_mesh_gpu_instancing": {"attributes": {}}});
    doc["meshes"][0]["primitives"][0]["extensions"] = json!({"EXT_primitive": {}});
    doc["extensions"]["KHR_lights_punctual"]["lights"][0]["extensions"] = json!({"EXT_light": {}});
    let (back, report) = read(&assembled(&doc, &bin)).unwrap();
    assert_eq!(report.lights, 3);
    assert_eq!(back.iter::<Light>().count(), 3);
    assert_eq!(
        report.extensions_not_read.into_iter().collect::<Vec<_>>(),
        [
            "EXT_document",
            "EXT_light",
            "EXT_mesh_gpu_instancing",
            "EXT_node",
            "EXT_primitive",
            "EXT_scene"
        ]
        .map(|n| (n.to_owned(), 1))
    );
    // a notice each
    let notices = ReadReport {
        extensions_not_read: BTreeMap::from([("EXT_node".to_owned(), 2)]),
        ..ReadReport::default()
    }
    .notices();
    assert!(
        notices
            .iter()
            .any(|n| n.reason.contains("EXT_node") && n.count == 2),
        "{notices:?}"
    );
    // required: refused
    doc["extensionsRequired"] = json!(["EXT_node"]);
    let e = read(&assembled(&doc, &bin)).unwrap_err();
    assert!(e.contains("extensionsRequired"), "{e}");
}

/// A light the extension states wrongly, or a node naming one that does not exist, is an error naming it; a light no
/// scene node names is counted.
#[test]
fn lights_are_read_strictly() {
    let (doc, bin) = document(&glb(&world()));
    let refused = |edit: &dyn Fn(&mut Value), says: &str| {
        let mut d = doc.clone();
        edit(&mut d);
        let e = read(&assembled(&d, &bin)).unwrap_err();
        assert!(e.contains(says), "{says}: {e}");
    };
    fn lights(d: &mut Value) -> &mut Value {
        &mut d["extensions"]["KHR_lights_punctual"]["lights"]
    }
    refused(
        &|d| lights(d)[0]["type"] = json!("area"),
        "is not directional, point or spot",
    );
    refused(&|d| lights(d)[0]["intensity"] = json!(-1.0), "intensity -1");
    refused(&|d| lights(d)[0]["range"] = json!(0.0), "range 0");
    refused(
        &|d| lights(d)[0]["power"] = json!(3),
        "unsupported property \"power\"",
    );
    refused(
        &|d| {
            let spot = lights(d)
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|l| l["type"] == "spot")
                .unwrap();
            spot["spot"]["outerConeAngle"] = json!(2.0);
        },
        "cones",
    );
    refused(
        &|d| {
            for n in d["nodes"].as_array_mut().unwrap() {
                if let Some(l) = n
                    .get_mut("extensions")
                    .and_then(|e| e.get_mut("KHR_lights_punctual"))
                {
                    l["light"] = json!(9);
                }
            }
        },
        "lights[9]: does not exist",
    );
    // a light no node names: counted, not read
    let mut d = doc.clone();
    lights(&mut d)
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "point"}));
    let (_, report) = read(&assembled(&d, &bin)).unwrap();
    assert_eq!((report.lights, report.lights_unplaced), (3, 1));
}
