//! The writer's two geometry inputs agree: a scene built from its prototypes' `core.mesh` (each GLB loaded into the
//! world first) writes the same files as the same scene built from the GLB files. A synthetic scene, made here.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Surfaces, Topology,
    Transform, World,
};
use primase_reskate::BuildOptions;
use primase_skate_scene::RaiseOptions;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-mesh-input-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// Two materials on a two-primitive prototype placed twice, ground collision and a spot light.
fn world() -> World {
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
    let (stone, paint) = (
        material(&mut w, "stone", [0.4, 0.4, 0.4, 1.0]),
        material(&mut w, "paint", [0.8, 0.1, 0.1, 1.0]),
    );
    let prim = |m, x: f32| Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![
                    [x, 0.0, 0.0],
                    [x + 1.0, 0.0, 0.0],
                    [x, 1.0, 0.0],
                    [x + 1.0, 1.0, 0.0],
                ]),
            ),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
            ),
        ]),
        indices: Some(vec![0, 1, 2, 2, 1, 3]),
        material: Some(m),
    };
    let proto = w.spawn();
    w.insert(proto, Name("wall".into()));
    w.insert(proto, Mesh::new(vec![prim(stone, 0.0), prim(paint, 2.0)]));
    for x in [0.0, 30.0] {
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
    let lamp = w.spawn();
    w.insert(
        lamp,
        Light {
            kind: LightKind::Spot {
                inner_cone: 0.25,
                outer_cone: 0.5,
            },
            color: [1.0, 0.9, 0.8],
            intensity: 20.0,
            range: Some(10.0),
            cast_shadows: None,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[1] = [0.0, 0.0, -1.0];
    t.cols[2] = [0.0, 1.0, 0.0];
    t.cols[3] = [1.0, 4.0, -5.0];
    w.insert(lamp, t);
    w
}

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[test]
fn core_mesh_input_writes_what_the_file_input_writes() {
    let mut w = world();
    primase_skate_scene::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "mesh_input".into(),
            surfaces: BTreeMap::new(),
            ..Default::default()
        },
    )
    .unwrap();
    let assets = tmp("assets");
    primase_skate_scene::prepare_scene_assets(&mut w, &assets).unwrap();
    let scene = tmp("scene");
    primase_skate_scene::write(&w, &scene).unwrap();
    let build = |from_mesh: bool| {
        let out_root = tmp(if from_mesh { "mesh" } else { "file" });
        let mut w = World::new();
        primase_skate_scene::read(&scene, &mut w).unwrap();
        if from_mesh {
            let r = primase_gltf::load_mesh_files(&mut w, &scene).unwrap();
            assert_eq!(r.meshes, 1);
        }
        primase_reskate::build(
            &w,
            &scene,
            &BuildOptions {
                out_root: out_root.clone(),
                ..BuildOptions::default()
            },
        )
        .unwrap();
        let mut f = files(&out_root);
        // the build's run time is the one value two builds of the same input do not share
        if let Some(stats) = f.get_mut("mesh_input/stats.json") {
            let mut v: serde_json::Value = serde_json::from_slice(stats).unwrap();
            v["totals"].as_object_mut().map(|t| t.remove("seconds"));
            *stats = serde_json::to_vec(&v).unwrap();
        }
        f
    };
    let (by_file, by_mesh) = (build(false), build(true));
    assert!(
        by_file.keys().any(|k| k.ends_with(".glb")),
        "{:?}",
        by_file.keys()
    );
    assert_eq!(
        by_file.keys().collect::<Vec<_>>(),
        by_mesh.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in &by_file {
        assert!(bytes == &by_mesh[name], "{name} differs");
    }
}

/// A spawn snapped to the ground lands on it (the 100 x 100 m ground at height 0, plus the 0.3 m lift); one whose snap
/// finds no ground and that has no height is refused, not written at a guessed height.
#[test]
fn a_spawn_over_no_ground_is_refused() {
    let build = |at: [f32; 2], tag: &str| {
        let mut w = world();
        primase_skate_scene::raise_skate(
            &mut w,
            &RaiseOptions {
                name: "spawned".into(),
                spawn: Some(primase_skate_scene::Spawn::At(at)),
                ..Default::default()
            },
        )
        .unwrap();
        let assets = tmp(&format!("{tag}-assets"));
        primase_skate_scene::prepare_scene_assets(&mut w, &assets).unwrap();
        let scene = tmp(&format!("{tag}-scene"));
        primase_skate_scene::write(&w, &scene).unwrap();
        let mut w = World::new();
        primase_skate_scene::read(&scene, &mut w).unwrap();
        let out_root = tmp(&format!("{tag}-out"));
        primase_reskate::build(
            &w,
            &scene,
            &BuildOptions {
                out_root: out_root.clone(),
                ..BuildOptions::default()
            },
        )
        .map(|_| out_root)
        .map_err(|e| e.to_string())
    };
    let out = build([0.0, 0.0], "on").unwrap();
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("spawned/map.json")).unwrap()).unwrap();
    let y = map["spawn"]["position"][1].as_f64().unwrap();
    assert!((y - 0.3).abs() < 1e-4, "spawn height {y}");
    let e = build([300.0, 300.0], "off").unwrap_err();
    assert!(e.contains("no ground under it"), "{e}");
}
