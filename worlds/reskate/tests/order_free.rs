//! What the writer emits does not depend on the order things reach it (the groundwork for writing a map in parts):
//! the lights a cap keeps and their numbering, and instanced prototypes named by content, not by the scene's ids.
//! Synthetic inputs, made here.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Instance, Mesh, Name, Primitive, Semantic, Stream, Surfaces, Topology, Transform, World,
};
use primase_meshops::vector::Vec3;
use primase_reskate::prepare::lights::keep_lights;
use primase_reskate::prepare::{RaiseOptions, SnapReskateMarkers};
use primase_reskate::scene::PlacedLight;
use primase_reskate::{EncodingOptions, WriterChoices};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-order-free-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

fn light(x: f32, y: f32, priority: f32, energy: f32) -> PlacedLight {
    PlacedLight {
        spot: false,
        pos: Vec3::new(x, y, 3.0),
        dir: Vec3::new(0.0, 0.0, -1.0),
        tan: Vec3::new(1.0, 0.0, 0.0),
        r: 1.0,
        g: 1.0,
        b: 1.0,
        energy,
        range: 10.0,
        radius: 0.1,
        spot_size: 0.0,
        spot_blend: 0.0,
        time_of_day: 0,
        cast_shadow: false,
        priority,
        group: String::new(),
        source: String::new(),
    }
}

/// Two lights per cell tie on priority at the cap's edge, and two share a position: whatever order they arrive in, the
/// same two per cell are kept, in the same order.
#[test]
fn lights_kept_by_a_cap_do_not_depend_on_their_order() {
    let lights = vec![
        light(10.0, 10.0, 1.0, 5.0),
        light(20.0, 10.0, 2.0, 5.0),
        light(30.0, 10.0, 2.0, 5.0),
        light(30.0, 10.0, 2.0, 7.0),
        light(250.0, 10.0, 1.0, 5.0),
        light(260.0, 10.0, 1.0, 6.0),
        light(270.0, 10.0, 1.0, 6.0),
    ];
    let kept = |ls: &[PlacedLight]| -> Vec<String> {
        keep_lights(ls, 2, 200.0)
            .iter()
            .map(|&i| format!("x {} energy {}", ls[i].pos.x, ls[i].energy))
            .collect()
    };
    let forward = kept(&lights);
    let mut reversed = lights.clone();
    reversed.reverse();
    let mut rotated = lights.clone();
    rotated.rotate_left(3);
    assert_eq!(forward.len(), 4);
    assert_eq!(forward, kept(&reversed));
    assert_eq!(forward, kept(&rotated));
    // the cell at x 0..200 has three priority-2 lights for two places: by position, x 20 then the x 30 ones by energy
    // (5 before 7); 200..400 has three equal priorities: the two lowest positions
    assert_eq!(
        forward,
        [
            "x 20 energy 5",
            "x 30 energy 5",
            "x 250 energy 5",
            "x 260 energy 6",
        ]
    );
}

/// A 10 x 10 quad grid (200 triangles, over the instancing threshold of 150) drawn with `material`.
fn grid(material: primase_core::Entity) -> Mesh {
    let mut pos = Vec::new();
    let mut uv = Vec::new();
    for j in 0..=10 {
        for i in 0..=10 {
            pos.push([i as f32 * 0.1, 0.0, -(j as f32) * 0.1]);
            uv.push([i as f32 * 0.1, j as f32 * 0.1]);
        }
    }
    let mut idx = Vec::new();
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

/// Prototypes "bench" and "bench copy" draw the same triangles with the same material; "red bench" the same triangles
/// with another. Each is placed three times (instanced). The map has one prototype file per drawn content, two, named
/// by content: the two alike share one.
#[test]
fn instanced_prototypes_are_named_by_what_they_draw() {
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
        m
    };
    let grey = material(&mut w, "grey", [0.5, 0.5, 0.5, 1.0]);
    let red = material(&mut w, "red", [0.8, 0.1, 0.1, 1.0]);
    let mut x = 0.0;
    for (name, m) in [("bench", grey), ("bench copy", grey), ("red bench", red)] {
        let proto = w.spawn();
        w.insert(proto, Name(name.into()));
        w.insert(proto, grid(m));
        for _ in 0..3 {
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
            x += 3.0;
        }
    }
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [-50.0, 0.0, 50.0],
            [50.0, 0.0, 50.0],
            [-50.0, 0.0, -50.0],
            [50.0, 0.0, -50.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    // the spawn: the writer makes none of its own
    let spawn = w.spawn();
    w.insert(
        spawn,
        primase_core::Marker {
            kind: "spawn".into(),
            ground: Some([0.0, 0.0]),
            yaw: None,
        },
    );
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "benches".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let assets = tmp("assets");
    primase_reskate::interchange::prepare_assets(&mut w, &assets).unwrap();
    let scene = tmp("scene");
    primase_reskate::interchange::write(&w, &scene).unwrap();
    let mut w = World::new();
    primase_reskate::interchange::read(&scene, &mut w).unwrap();
    primase_reskate::interchange::load(&scene, &mut w).unwrap();
    // the markers snapped before the writer, which snaps nothing
    SnapReskateMarkers::new()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let out_root = tmp("out");
    let report = primase_reskate::write_map(
        &w,
        &scene,
        &EncodingOptions {
            out_root: out_root.clone(),
            ..EncodingOptions::default()
        },
        &WriterChoices::default(),
    )
    .unwrap();
    assert_eq!(report.instances, 9);
    let mut files: Vec<String> = std::fs::read_dir(out_root.join("benches/meshes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|f| f.starts_with("p_"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");
    // 32 hex digits: 128 bits of content key
    assert!(files.iter().all(|f| f.len() == "p_.glb".len() + 32), "{files:?}");
}
