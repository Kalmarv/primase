//! The writer takes geometry as `core.mesh`: a scene read from its folder and loaded (`interchange::load`) writes the
//! same files as the world it was written from, and one read but not loaded is refused with its reason; a prototype
//! without geometry that collides is written as collision. A synthetic scene, made here.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Surfaces, Topology,
    Transform, World,
};
use primase_reskate::BuildOptions;
use primase_reskate::prepare::RaiseOptions;

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

/// A spawn snapped to the ground at (east, north) (the writer makes none of its own).
fn spawn_at(w: &mut World, at: [f32; 2]) {
    let spawn = w.spawn();
    w.insert(
        spawn,
        primase_core::Marker {
            kind: "spawn".into(),
            ground: Some(at),
            yaw: None,
        },
    );
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
fn a_loaded_scene_writes_what_its_world_writes() {
    let mut w = world();
    spawn_at(&mut w, [0.0, 0.0]);
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "mesh_input".into(),
            surfaces: BTreeMap::new(),
            ..Default::default()
        },
    )
    .unwrap();
    let assets = tmp("assets");
    primase_reskate::interchange::prepare_assets(&mut w, &assets).unwrap();
    let scene = tmp("scene");
    primase_reskate::interchange::write(&w, &scene).unwrap();
    // read but not loaded: the writer reads no GLB
    let mut unloaded = World::new();
    primase_reskate::interchange::read(&scene, &mut unloaded).unwrap();
    let e = primase_reskate::build(
        &unloaded,
        &scene,
        &BuildOptions {
            out_root: tmp("unloaded"),
            ..BuildOptions::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("has no core.mesh"), "{e}");
    let build = |source: Option<&World>| {
        let out_root = tmp(if source.is_some() { "world" } else { "loaded" });
        let mut read = World::new();
        let w = match source {
            Some(w) => w,
            None => {
                primase_reskate::interchange::read(&scene, &mut read).unwrap();
                let r = primase_reskate::interchange::load(&scene, &mut read).unwrap();
                assert_eq!(r.meshes.meshes, 1);
                &read
            }
        };
        primase_reskate::build(
            w,
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
    let (from_world, loaded) = (build(Some(&w)), build(None));
    assert!(
        from_world.keys().any(|k| k.ends_with(".glb")),
        "{:?}",
        from_world.keys()
    );
    assert_eq!(
        from_world.keys().collect::<Vec<_>>(),
        loaded.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in &from_world {
        assert!(bytes == &loaded[name], "{name} differs");
    }
}

/// A spawn snapped to the ground lands on it (the 100 x 100 m ground at height 0, plus the 0.3 m lift); one whose snap
/// finds no ground and that has no height is refused, not written at a guessed height.
#[test]
fn a_spawn_over_no_ground_is_refused() {
    let build = |at: [f32; 2], tag: &str| {
        let mut w = world();
        // the spawn as place-spawn puts it at selection: a ground marker at (east, north)
        let spawn = w.spawn();
        w.insert(
            spawn,
            primase_core::Marker {
                kind: "spawn".into(),
                ground: Some(at),
                yaw: None,
            },
        );
        primase_reskate::prepare::raise_skate(
            &mut w,
            &RaiseOptions {
                name: "spawned".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let assets = tmp(&format!("{tag}-assets"));
        primase_reskate::interchange::prepare_assets(&mut w, &assets).unwrap();
        let scene = tmp(&format!("{tag}-scene"));
        primase_reskate::interchange::write(&w, &scene).unwrap();
        let mut w = World::new();
        primase_reskate::interchange::read(&scene, &mut w).unwrap();
        primase_reskate::interchange::load(&scene, &mut w).unwrap();
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

/// A prototype that collides without drawing (collision, no mesh) is placed as collision: its triangles in the map's
/// collision, counted, and nothing drawn for it.
#[test]
fn a_prototype_that_collides_without_drawing_is_written_as_collision() {
    let mut w = world();
    spawn_at(&mut w, [0.0, 0.0]);
    let fence = w.spawn();
    w.insert(fence, Name("fence".into()));
    let concrete = w.resource_mut::<Surfaces>().unwrap().index("concrete").unwrap();
    let mut c = Collision::default();
    c.extend(
        &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
        &[[0, 1, 2]],
        concrete,
        |p| p,
    );
    w.insert(fence, c);
    let at = w.spawn();
    w.insert(
        at,
        Instance {
            prototype: fence,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [10.0, 0.0, 10.0];
    w.insert(at, t);
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "collides".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let assets = tmp("collides-assets");
    primase_reskate::interchange::prepare_assets(&mut w, &assets).unwrap();
    let scene = tmp("collides-scene");
    primase_reskate::interchange::write(&w, &scene).unwrap();
    let mut w = World::new();
    primase_reskate::interchange::read(&scene, &mut w).unwrap();
    primase_reskate::interchange::load(&scene, &mut w).unwrap();
    let out_root = tmp("collides-out");
    primase_reskate::build(
        &w,
        &scene,
        &BuildOptions {
            out_root: out_root.clone(),
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let stats: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out_root.join("collides/stats.json")).unwrap()).unwrap();
    assert_eq!(
        stats["totals"]["placements_collision_only"], 1,
        "{}",
        stats["totals"]
    );
    assert_eq!(stats["totals"]["placements_with_empty_mesh"], 0);
    assert_eq!(
        stats["prop_collision_top"]["fence"], 1,
        "{}",
        stats["prop_collision_top"]
    );
    // drawn: the wall's two placements only
    assert!(
        !files(&out_root).keys().any(|k| k.contains("fence")),
        "{:?}",
        files(&out_root).keys()
    );
}
