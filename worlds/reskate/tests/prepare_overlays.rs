//! `prepare-reskate-overlays`: the decals of the writer's meshes stacked before the writer, which puts the recorded
//! stack on the same pieces again and stacks nothing of its own; worked by hand on one tile.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    DrawLayer, DrawOrder, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform,
    World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{MarkerKind, Other, OverlayPiece, SkateMarker, SkateOverlayStack};
use primase_reskate::passes::{PrepareReskateOverlays, RaiseOptions, prepare_reskate_overlays};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-prepare-overlays-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// A flat `size` x `size` square facing up, drawn with `material`.
fn square(w: &mut World, name: &str, size: f32, material: Entity) -> Entity {
    let p = w.spawn();
    w.insert(p, Name(name.into()));
    w.insert(
        p,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![
                        [0.0, 0.0, 0.0],
                        [size, 0.0, 0.0],
                        [0.0, 0.0, size],
                        [size, 0.0, size],
                    ]),
                ),
                (
                    Semantic::TexCoord(0),
                    Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
                ),
            ]),
            indices: Some(vec![0, 2, 1, 1, 2, 3]),
            material: Some(material),
        }]),
    );
    p
}

fn place(w: &mut World, proto: Entity, x: f32, z: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [x, 0.0, z];
    w.insert(e, t);
    e
}

struct Fixture {
    w: World,
    decal: Entity,
    second: Entity,
}

/// A 4 m stone floor and two 2 m decals over it, overlapping one another, all at height 0 in one 200 m tile; each
/// placed once (merged), raised, with a spawn at a stated height.
fn fixture() -> Fixture {
    let mut w = World::new();
    let material = |w: &mut World, name: &str, decal: bool| {
        let m = w.spawn();
        w.insert(m, Name(name.into()));
        w.insert(m, PbrMaterial::default());
        if decal {
            w.insert(m, DrawOrder::conventional(DrawLayer::Decal, 2500));
        }
        m
    };
    let (stone, dirt, paint) = (
        material(&mut w, "stone", false),
        material(&mut w, "dirt", true),
        material(&mut w, "paint", true),
    );
    let floor = square(&mut w, "floor", 4.0, stone);
    let a = square(&mut w, "dirt", 2.0, dirt);
    let b = square(&mut w, "paint", 2.0, paint);
    place(&mut w, floor, 10.0, -14.0);
    let decal = place(&mut w, a, 10.5, -13.5);
    let second = place(&mut w, b, 11.0, -13.0);
    primase_reskate::passes::raise_skate(&mut w, &RaiseOptions::new("overlays").unwrap()).unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([12.0, 12.0], Some(0.0))),
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
    Fixture { w, decal, second }
}

fn write(w: &World, tag: &str, e: &EncodingOptions) -> Result<(primase_reskate::BuildReport, Files), String> {
    let out_root = tmp(tag);
    let r = primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("overlays".into()),
            ..e.clone()
        },
    )
    .map_err(|e| e.to_string())?;
    Ok((r, files(&out_root.join("overlays"))))
}

type Files = BTreeMap<String, Vec<u8>>;

/// The folder's files but stats.json's run time.
fn files(root: &Path) -> Files {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let mut bytes = std::fs::read(&p).unwrap();
            if rel == "stats.json" {
                let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                v["totals"].as_object_mut().map(|t| t.remove("seconds"));
                bytes = serde_json::to_vec(&v).unwrap();
            }
            out.insert(rel, bytes);
        }
    }
    out
}

/// The decals lie on the floor and on each other: the one under is lifted 4 mm, the one over it 8 mm, and the writer
/// writes those lifts; without the pass it lifts nothing and says so.
#[test]
fn decals_are_stacked_before_the_writer() {
    let Fixture { mut w, .. } = fixture();
    let mut log = PassLog::default();
    PrepareReskateOverlays::new(0.004)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    let rec = w.resource::<SkateOverlayStack>().unwrap().clone();
    let mut lifts: Vec<f32> = rec
        .pieces
        .values()
        .map(|p| p.shift)
        .filter(|&s| s > 0.0)
        .collect();
    lifts.sort_by(f32::total_cmp);
    assert_eq!(lifts.len(), 2, "{rec:?}");
    assert!(
        (lifts[0] - 0.004).abs() < 1e-6 && (lifts[1] - 0.008).abs() < 1e-6,
        "{lifts:?}"
    );
    assert_eq!(rec.under.len(), 1);
    assert_eq!(rec.pieces.len(), 3, "the floor's piece and the decals'");
    // what each piece is, in the world: the floor's centre on the ground, each decal's at its lift
    for (piece, s) in &rec.pieces {
        let OverlayPiece::Placed { part: 0, .. } = piece else {
            panic!("{piece:?}: a merged placement's first part");
        };
        assert_eq!((s.mesh.as_str(), s.tris), ("t_0_0", 2), "{s:?}");
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        if s.overlay {
            let centre = if s.material.contains("dirt") {
                [11.5, -12.5]
            } else {
                [12.0, -12.0]
            };
            assert!(
                near(s.centre[0], centre[0]) && near(s.centre[2], centre[1]) && near(s.centre[1], s.shift),
                "{s:?}"
            );
        } else {
            assert!(
                near(s.centre[0], 12.0)
                    && near(s.centre[1], 0.0)
                    && near(s.centre[2], -12.0)
                    && s.shift == 0.0,
                "{s:?}"
            );
        }
    }
    assert_eq!(log.counts["overlay pieces lifted"], 2);
    let (r, _) = write(&w, "stacked", &EncodingOptions::default()).unwrap();
    let written: Vec<f32> = r
        .lifts
        .values()
        .flatten()
        .flatten()
        .copied()
        .filter(|&l| l > 0.0)
        .collect();
    assert!(
        written.iter().any(|&l| (l - 0.004).abs() < 1e-6)
            && written.iter().any(|&l| (l - 0.008).abs() < 1e-6)
    );
    assert!(
        r.unsupported
            .iter()
            .all(|(what, _)| !what.starts_with("overlay pieces")),
        "{:?}",
        r.unsupported
    );
    // no pass: nothing lifted, the two decals reported
    let Fixture { w: plain, .. } = fixture();
    let (r, _) = write(&plain, "plain", &EncodingOptions::default()).unwrap();
    assert!(r.lifts.is_empty(), "{:?}", r.lifts);
    assert!(
        r.unsupported
            .iter()
            .any(|(what, n)| what.starts_with("overlay pieces") && *n == 2),
        "{:?}",
        r.unsupported
    );
}

/// A stack is refused by a writer whose meshes would differ: other grouping settings, or a piece it was not made for.
#[test]
fn a_stack_for_other_meshes_is_refused() {
    let Fixture { mut w, .. } = fixture();
    PrepareReskateOverlays::new(0.004)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let e = write(
        &w,
        "other-tile",
        &EncodingOptions {
            tile: 100.0,
            ..EncodingOptions::default()
        },
    )
    .unwrap_err();
    assert!(e.contains("stacked for the writer settings"), "{e}");
    // given the writer's grouping, the pass's stack is taken
    let Fixture { mut w, .. } = fixture();
    let tile100 = EncodingOptions {
        tile: 100.0,
        ..EncodingOptions::default()
    };
    PrepareReskateOverlays::new(0.004)
        .unwrap()
        .for_encoding(&tile100)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    write(&w, "tile-100", &tile100).unwrap();
    // a decal placed after the stack was made: a piece the stack does not know
    let Fixture { mut w, decal, .. } = fixture();
    PrepareReskateOverlays::new(0.004)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let proto = w.get::<Instance>(decal).unwrap().prototype;
    place(&mut w, proto, 12.0, -12.5);
    let e = write(&w, "new-piece", &EncodingOptions::default()).unwrap_err();
    assert!(e.contains("other pieces or materials"), "{e}");
}

/// The stack names its pieces by entity: one whose placement leaves the world leaves the stack with it, and the
/// writer takes the rest.
#[test]
fn the_stack_follows_its_pieces_entities() {
    let Fixture { mut w, second, .. } = fixture();
    PrepareReskateOverlays::new(0.004)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let mut refs = Vec::new();
    primase_core::Resource::references(w.resource::<SkateOverlayStack>().unwrap(), &mut refs);
    assert!(refs.contains(&second));
    w.despawn(second);
    let rec = w.resource::<SkateOverlayStack>().unwrap();
    assert_eq!((rec.pieces.len(), rec.under.len()), (2, 0));
    write(&w, "despawned", &EncodingOptions::default()).unwrap();
}

/// The step is required; every setting prints back and reads back the same; bad values and unknown keys are refused.
#[test]
fn the_overlay_options_round_trip_and_are_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    for given in [
        o(&[("step", "0.004")]),
        o(&[
            ("step", "0"),
            ("tile", "100"),
            ("min_instances", "2"),
            ("min_instance_tris", "0"),
            ("max_render_edge", "50"),
            ("limit", "2000"),
            ("offset_x", "-10.5"),
            ("offset_y", "3"),
        ]),
    ] {
        let pass = prepare_reskate_overlays(&given).unwrap();
        assert_eq!(
            prepare_reskate_overlays(&pass.options()).unwrap().options(),
            pass.options()
        );
    }
    for bad in [
        o(&[]),
        o(&[("step", "-0.001")]),
        o(&[("step", "nan")]),
        o(&[("step", "0.004"), ("tile", "0")]),
        o(&[("step", "0.004"), ("min_instances", "-1")]),
        o(&[("step", "0.004"), ("offset_x", "inf")]),
        o(&[("step", "0.004"), ("lift", "1")]),
    ] {
        assert!(prepare_reskate_overlays(&bad).is_err(), "{bad:?}");
    }
}
