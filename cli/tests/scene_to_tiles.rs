//! The `scene_to_tiles` driver end to end on a synthetic skate scene (a core world raised and written as one): every
//! scene light reaches its cell's tile, in the compressed set and the plain one.

#![allow(clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "../examples/scene_to_tiles.rs"]
mod driver;

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("scene-to-tiles")
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
        &primase_reskate::prepare::RaiseOptions::new("lights").unwrap(),
    )
    .unwrap();
    primase_reskate::interchange::prepare_assets(&mut w, &tmp(&format!("{name}_assets"))).unwrap();
    let dir = tmp(name);
    primase_reskate::interchange::write(&w, &dir).unwrap();
    dir
}

#[test]
fn every_scene_light_reaches_its_tile() {
    let folder = scene("scene");
    let out = tmp("tiles");
    let index = driver::run([folder.display().to_string(), out.display().to_string()]).unwrap();
    let lights = |tiles: &[primase_web::TileInfo]| tiles.iter().map(|t| t.lights).sum::<u64>();
    assert_eq!(lights(&index.tiles), 3);
    assert!(
        index
            .tiles
            .iter()
            .any(|t| t.lights == 1 && t.content == primase_web::TileContent::MetadataOnly),
        "the light in a cell with nothing else gets a metadata-only tile"
    );
    for set in [out.clone(), out.join("plain")] {
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(set.join("tiles.json")).unwrap()).unwrap();
        let n: u64 = written["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["lights"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(n, 3, "{}", set.display());
    }
}

#[test]
fn no_plain_writes_only_the_tile_set() {
    let folder = scene("scene_no_plain");
    let out = tmp("tiles_no_plain");
    let with = tmp("tiles_with_plain");
    let index = driver::run([
        folder.display().to_string(),
        out.display().to_string(),
        "--no-plain".into(),
    ])
    .unwrap();
    driver::run([folder.display().to_string(), with.display().to_string()]).unwrap();
    assert!(!out.join("plain").exists());
    // the tile set itself is the same with or without the plain set beside it
    assert_eq!(
        std::fs::read(out.join("tiles.json")).unwrap(),
        std::fs::read(with.join("tiles.json")).unwrap()
    );
    for t in &index.tiles {
        assert_eq!(
            std::fs::read(out.join(&t.file)).unwrap(),
            std::fs::read(with.join(&t.file)).unwrap()
        );
    }
}
