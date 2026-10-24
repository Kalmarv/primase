//! What the scene a map is written from refuses before it draws a wrong map: two prototypes under one key, parts that
//! do not follow the primitives, and a map name that is not one folder's.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_reskate::components::{SkateMarker, SkateParts};
use primase_reskate::passes::{LightCalibration, RaiseOptions};
use primase_reskate::scene::Scene;
use primase_reskate::{EncodingOptions, write_map};

/// FNV-1a over the UTF-16LE bytes of both ids is 0x6f5692ec (found by search; the writer's prototype key).
const COLLIDING: [&str; 2] = ["6ouxutthcwt1jk", "y4nq1qlwtwsla5"];

fn spawn() -> SkateMarker {
    SkateMarker {
        kind: primase_reskate::components::MarkerKind::Spawn,
        position: Some(([1.0, 1.0], Some(0.0))),
        yaw: None,
        m: None,
        blueprint: None,
        snap: None,
        lift: None,
        shelter: None,
        trainer: None,
        other: primase_reskate::components::Other::new(),
    }
}

/// A raised map of one placed triangle per name, each of its own size and material.
fn raised(names: &[&str]) -> World {
    let mut w = World::new();
    for (k, name) in names.iter().enumerate() {
        let material = w.spawn();
        w.insert(material, Name(format!("m{k}")));
        w.insert(material, PbrMaterial::default());
        let size = 2.0 + 10.0 * k as f32;
        let proto = w.spawn();
        w.insert(proto, Name((*name).to_owned()));
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [size, 0.0, 0.0], [0.0, 0.0, -size]]),
                )]),
                indices: None,
                material: Some(material),
            }]),
        );
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [100.0 * k as f32, 0.0, 0.0];
        w.insert(placement, t);
    }
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "integrity",
            LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
    let marker = w.spawn();
    w.insert(marker, spawn());
    w
}

fn refusal(w: &World) -> String {
    Scene::from_world(w, Path::new(""))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default()
}

/// Two ids whose keys are the same are refused, naming both and the key; distinct ids are read.
#[test]
fn two_prototypes_under_one_key_are_refused_naming_both() {
    let e = refusal(&raised(&COLLIDING));
    assert!(
        e.contains(COLLIDING[0]) && e.contains(COLLIDING[1]) && e.contains("0x6f5692ec"),
        "{e}"
    );
    assert!(Scene::from_world(&raised(&["alpha", "beta"]), Path::new("")).is_ok());
}

/// Parts that do not follow the primitives are refused where the scene is read, not by an index in the writer.
#[test]
fn parts_that_do_not_follow_the_primitives_are_refused() {
    let mut w = raised(&["alpha"]);
    let proto = w.iter::<SkateParts>().next().map(|(e, _)| e).unwrap();
    // parts of another count: skate.parts itself refuses them, before the scene is read
    w.insert(proto, SkateParts(Vec::new()));
    let e = refusal(&w);
    assert!(
        e.contains("skate.parts") && e.contains("0 parts for 1 core.mesh's primitives"),
        "{e}"
    );
    // no parts at all, which no component's check sees: the scene refuses the prototype by its name
    w.remove::<SkateParts>(proto);
    let e = refusal(&w);
    assert!(
        e.contains("alpha") && e.contains("0 skate.parts for 1 primitives"),
        "{e}"
    );
}

/// A map name that is not one folder's is refused by the sink, whichever way it was given.
#[test]
fn a_map_name_that_is_not_one_folder_is_refused() {
    let w = raised(&["alpha"]);
    let root = std::env::temp_dir().join(format!("primase-integrity-{}", std::process::id()));
    for name in ["..", ".", "", "a/b", "a\\b", "../escape"] {
        let e = EncodingOptions {
            out_root: root.clone(),
            name: Some(name.to_owned()),
            ..EncodingOptions::default()
        };
        let err = write_map(&w, Path::new(""), &e)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("not one folder"), "{name:?}: {err}");
    }
    assert!(!root.exists(), "nothing written for a refused name");
}
