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
/// tile), as a skate scene folder.
fn scene() -> PathBuf {
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
        Mesh {
            primitives: vec![Primitive {
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
            }],
        },
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
    primase_skate_scene::raise_skate(
        &mut w,
        &primase_skate_scene::RaiseOptions {
            name: "lights".into(),
            surfaces: BTreeMap::new(),
        },
    )
    .unwrap();
    primase_skate_scene::prepare_scene_assets(&mut w, &tmp("assets")).unwrap();
    let dir = tmp("scene");
    primase_skate_scene::write(&w, &dir).unwrap();
    dir
}

#[test]
fn every_scene_light_reaches_its_tile() {
    let folder = scene();
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
