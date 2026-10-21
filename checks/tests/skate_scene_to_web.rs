//! A skate-scene folder converted to web tiles end to end on a synthetic skate scene (a core world raised and written
//! as one): every scene light reaches its cell's tile, in the compressed set and in the plain set (a conversion of its
//! own), and no light is left unlowered.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("skate-scene-to-web")
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// A triangle placed in two cells, a point light beside each and one in a cell with nothing else (a metadata-only
/// tile), as a skate scene folder named `name` (one per test: tests run in parallel).
fn scene(name: &str) -> PathBuf {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, Name("plain".into()));
    w.insert(
        m,
        PbrMaterial {
            metallic: 0.0,
            ..PbrMaterial::default()
        },
    );
    let proto = w.spawn();
    w.insert(proto, Name("tri".into()));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]]),
                ),
                (
                    Semantic::TexCoord(0),
                    Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
                ),
            ]),
            indices: None,
            material: Some(m),
        }]),
    );
    let mut at = |x: f32, light: bool, placed: bool| {
        if placed {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3] = [x, 0.0, -10.0];
            w.insert(e, t);
        }
        if light {
            let e = w.spawn();
            w.insert(
                e,
                Light {
                    kind: LightKind::Point,
                    color: [1.0; 3],
                    intensity: 10.0,
                    range: Some(8.0),
                    cast_shadows: None,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3] = [x + 2.0, 3.0, -12.0];
            w.insert(e, t);
        }
    };
    at(10.0, true, true);
    at(310.0, true, true);
    at(710.0, true, false);
    primase_reskate::prepare::raise_skate(
        &mut w,
        &primase_reskate::prepare::RaiseOptions::new(
            "lights",
            primase_reskate::passes::LightCalibration::Standard,
        )
        .unwrap(),
    )
    .unwrap();
    let dir = tmp(name);
    primase_reskate::write::interchange::write(&w, &dir).unwrap();
    dir
}

/// Every crate's passes this conversion names, registered as a program registers them.
fn registry() -> primase_core::pass::PassRegistry {
    let mut r = primase_core::pass::PassRegistry::new();
    primase_core::register(&mut r);
    primase_image::register(&mut r);
    primase_meshops::register(&mut r);
    primase_reskate::register(&mut r);
    primase_gltf::register(&mut r);
    r
}

/// The skate scene's web tiles, as a program converts them (`plain`: the plain comparison set): the skate-scene
/// reader, a selection and passes, the web writer. Returns the conversion's notices as text.
fn convert(scene: &Path, out: &Path, plain: bool) -> String {
    let mut options = primase_web::Options::new(primase_web::TileSize::new(200.0).unwrap());
    options.tile_byte_budget = Some(96 << 20);
    if plain {
        options.compress = false;
        options.gpu_instancing = false;
    }
    let (mut parts, writer) = primase_web::convert::tiles(vec![(options, out.to_path_buf())]).unwrap();
    let registry = registry();
    let select = registry
        .parse("select-lod{form=finest}, lower-skate-lights")
        .unwrap();
    let passes = registry
        .parse(
            "lower-skate-pbr, normalize-normals, fill-missing-texcoords, \
             prepare-gltf-materials{base-color=0.8/0.8/0.8/1,metallic=0,roughness=0.85}",
        )
        .unwrap();
    let mut reader = primase_reskate::SceneReader::new(scene.to_path_buf());
    let report = primase_core::convert::convert_parts(
        &mut reader,
        &mut parts,
        Box::new(writer),
        &primase_core::pass::PassManager::new(registry),
        &select,
        &passes,
    )
    .unwrap();
    report.notices().iter().map(|n| format!("{n:?}\n")).collect()
}

fn tiles(set: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(set.join("tiles.json")).unwrap()).unwrap()
}

fn lights(index: &serde_json::Value) -> u64 {
    index["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["lights"].as_u64().unwrap_or(0))
        .sum()
}

#[test]
fn every_scene_light_reaches_its_tile() {
    let folder = scene("scene");
    let out = tmp("tiles");
    let printed = convert(&folder, &out, false);
    // a light lower-skate-lights could not lower would be an unsupported notice
    assert!(!printed.contains("lights not lowered"), "{printed}");
    let index = tiles(&out);
    assert_eq!(lights(&index), 3);
    assert!(
        index["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["lights"] == 1 && t["content"] == "metadata_only"),
        "the light in a cell with nothing else gets a metadata-only tile"
    );
}

/// The plain set is its own conversion: the same tiles and lights, uncompressed and not instanced.
#[test]
fn the_plain_set_is_a_conversion_of_its_own() {
    let folder = scene("scene_plain");
    let (out, plain) = (tmp("tiles_main"), tmp("tiles_plain"));
    convert(&folder, &out, false);
    convert(&folder, &plain, true);
    let (a, b) = (tiles(&out), tiles(&plain));
    assert_eq!(lights(&b), 3);
    let cells = |i: &serde_json::Value| -> Vec<(i64, i64)> {
        i["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["col"].as_i64().unwrap(), t["row"].as_i64().unwrap()))
            .collect()
    };
    assert_eq!(cells(&a), cells(&b));
    for t in b["tiles"].as_array().unwrap() {
        let file = t["file"].as_str().unwrap();
        let bytes = std::fs::read(plain.join(file)).unwrap();
        assert!(
            !bytes.windows(23).any(|w| w == b"EXT_meshopt_compression"),
            "{file}: the plain set is not compressed"
        );
    }
}
