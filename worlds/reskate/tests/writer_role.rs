//! The map writer chooses no content (PRIMASE-PLAN step 16, the writer's role): given a prepared world and no
//! preparation pass for these, it writes what it is given. Placements below the old floor stay where they are (no
//! lift: `prepare-reskate-floor`), a placement outside the scene's region is written (no clip: `clip-reskate-region`),
//! every light is written (no cap: `select-reskate-lights`), a water outline outside the region is written (no extent:
//! `prepare-reskate-water`), and there is no pause map (`prepare-reskate-pause-map`). Synthetic, made here.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Entity, Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Surfaces,
    Topology, Transform, World,
};
use primase_reskate::dialect::{Other, SkateScene, SkateWater};
use primase_reskate::prepare::{RaiseOptions, SnapReskateMarkers};
use primase_reskate::{EncodingOptions, WriterChoices};

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

fn place(w: &mut World, prototype: Entity, at: [f32; 3]) {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = at;
    w.insert(e, t);
}

/// Region east 0..100, north 0..100. Three benches instanced 500 m down; one sign at east 900; three lights in the
/// first cell; ground at 0 over the region; a water outline at east 2000..2100; the spawn snapped (as a program does).
fn world() -> World {
    let mut w = World::new();
    let grey = w.spawn();
    w.insert(grey, Name("grey".into()));
    w.insert(
        grey,
        PbrMaterial {
            base_color: [0.5, 0.5, 0.5, 1.0],
            metallic: 0.0,
            ..PbrMaterial::default()
        },
    );
    let bench = w.spawn();
    w.insert(bench, Name("bench".into()));
    w.insert(bench, grid(grey));
    for east in [10.0, 20.0, 30.0] {
        place(&mut w, bench, [east, -500.0, -10.0]);
    }
    let sign = w.spawn();
    w.insert(sign, Name("sign".into()));
    w.insert(sign, grid(grey));
    place(&mut w, sign, [900.0, 0.0, -10.0]);
    for east in [20.0, 40.0, 60.0] {
        let l = w.spawn();
        w.insert(
            l,
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: 5.0,
                range: Some(10.0),
                cast_shadows: None,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 3.0, -20.0];
        w.insert(l, t);
    }
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [0.0, 0.0, -100.0],
            [100.0, 0.0, -100.0],
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
            ground: Some([50.0, 50.0]),
            yaw: None,
        },
    );
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "role".into(),
            ..Default::default()
        },
    )
    .unwrap();
    w.resource_mut::<SkateScene>().unwrap().region = Some([0.0, 0.0, 100.0, 100.0]);
    let water = w.spawn();
    w.insert(
        water,
        SkateWater {
            corners: vec![[2000.0, 0.0], [2100.0, 0.0], [2100.0, 100.0], [2000.0, 100.0]],
            height: Some(0.0),
            other: Other::new(),
        },
    );
    SnapReskateMarkers::new()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    w
}

#[test]
fn the_writer_writes_what_it_is_given() {
    let out_root: PathBuf =
        std::env::temp_dir().join(format!("primase-reskate-writer-role-{}", std::process::id()));
    if out_root.exists() {
        std::fs::remove_dir_all(&out_root).unwrap();
    }
    primase_reskate::write_map(
        &world(),
        Path::new(""),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("role".into()),
            ..EncodingOptions::default()
        },
        &WriterChoices::default(),
    )
    .unwrap();
    let dir = out_root.join("role");
    let read = |f: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join(f)).unwrap()).unwrap()
    };
    let (map, stats) = (read("map.json"), read("stats.json"));
    let objects = map["objects"].as_array().unwrap();
    // no lift: the benches stand 500 m down, instanced, their height as given
    assert_eq!(stats["lift_m"].as_f64().unwrap(), 0.0, "{}", stats["lift_m"]);
    let benches: Vec<f64> = objects
        .iter()
        .filter(|o| o["name"].as_str().unwrap().starts_with("i_"))
        .map(|o| o["transform"][10].as_f64().unwrap())
        .collect();
    assert_eq!(benches, [-500.0; 3], "{objects:?}");
    // no region clip: the sign at east 900 is written
    assert_eq!(stats["totals"]["placements_clipped_away"], 0);
    assert!(
        objects
            .iter()
            .any(|o| o["name"].as_str().unwrap().starts_with("t_4_")),
        "{:?}",
        objects.iter().map(|o| &o["name"]).collect::<Vec<_>>()
    );
    // no light cap: all three in one cell
    assert_eq!(stats["totals"]["lights_written"], 3);
    assert_eq!(map["lights"].as_array().unwrap().len(), 3);
    // no water extent: the outline 1900 m beyond the region is written
    assert!(
        objects.iter().any(|o| o["collision_mode"] == "water"),
        "no water object"
    );
    // no pause map
    assert!(
        !dir.join("ui").join("map_image.png").exists(),
        "a pause map was drawn"
    );
    std::fs::remove_dir_all(&out_root).unwrap();
}
