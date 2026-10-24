//! A prototype nothing places collides nowhere: its `core.collision` is in its own frame, not the world's, so the writer
//! leaves it out (counted) rather than filing it as world collision at the origin. (A GTA prototype every placement of
//! which disables its embedded collision is left so: `lower-gta-embedded-collision` moves the placements to a copy
//! without it.) World collision (a soup with no mesh) is written as before.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::{
    Collision, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Surfaces, Topology, Transform,
    World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{MarkerKind, Other, SkateMarker};
use primase_reskate::passes::RaiseOptions;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-orphan-collision-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// A 2 m square with collision, as a prototype.
fn prop(w: &mut World, name: &str, surface: u16) -> Entity {
    let pos = vec![
        [0.0, 1.0, 0.0],
        [2.0, 1.0, 0.0],
        [0.0, 1.0, -2.0],
        [2.0, 1.0, -2.0],
    ];
    let tris = vec![[0u32, 1, 2], [2, 1, 3]];
    let material = w.spawn();
    w.insert(material, Name(format!("{name} paint")));
    w.insert(material, primase_core::pbr::PbrMaterial::default());
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    w.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(pos.clone()))]),
            indices: Some(tris.iter().flatten().copied().collect()),
            material: Some(material),
        }]),
    );
    let mut c = Collision::default();
    c.extend(&pos, &tris, surface, |p| p);
    w.insert(e, c);
    e
}

/// World ground around east 1000 (a soup, no mesh); a bench placed at east 1000; a second bench prototype nothing
/// places; a spawn.
fn map() -> World {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [900.0, 0.0, 100.0],
            [1100.0, 0.0, 100.0],
            [900.0, 0.0, -100.0],
            [1100.0, 0.0, -100.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    let bench = prop(&mut w, "bench", concrete);
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: bench,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [1000.0, 0.0, 0.0];
    w.insert(placed, t);
    prop(&mut w, "orphan", concrete);
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "orphan",
            primase_reskate::passes::LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([1000.0, 0.0], Some(1.0))),
            yaw: None,
            m: None,
            blueprint: None,
            snap: None,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    w
}

#[test]
fn a_prototype_nothing_places_collides_nowhere() {
    let out = tmp("map");
    let r = primase_reskate::write_map(
        &map(),
        Path::new(""),
        &EncodingOptions {
            out_root: out.clone(),
            name: Some("orphan".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    assert!(
        r.omitted
            .iter()
            .any(|(what, n)| what == "collision of a prototype nothing places" && *n == 1),
        "{:?}",
        r.omitted
    );
    let stats: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("orphan").join("stats.json")).unwrap()).unwrap();
    // the ground soup alone: no soup at the origin, and the map's region is the ground's and the bench's
    assert_eq!(stats["soups"].as_array().unwrap().len(), 1, "{}", stats["soups"]);
    let region = stats["region"].as_array().unwrap();
    assert!(region[0].as_f64().unwrap() >= 899.0, "{region:?}");
    // no collision leaf near the origin
    let meshes: Vec<String> = std::fs::read_dir(out.join("orphan").join("meshes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !meshes
            .iter()
            .any(|m| m.starts_with("c_0_") || m.starts_with("c_-1_")),
        "{meshes:?}"
    );
    assert!(meshes.iter().any(|m| m.starts_with("c_3_")), "{meshes:?}");
}
