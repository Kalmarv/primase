//! `prepare-reskate-grinds`: each grind curve's prism built before the writer, which writes prisms and builds none;
//! worked by hand on a synthetic world.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Entity, Surfaces, World};
use primase_reskate::EncodingOptions;
use primase_reskate::dialect::{MarkerKind, Other, SkateGrindCurve, SkateGrindPrism, SkateMarker};
use primase_reskate::grind::PrismRules;
use primase_reskate::prepare::{PrepareReskateGrinds, RaiseOptions, prepare_reskate_grinds};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-prepare-grinds-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

fn curve(w: &mut World, points: Vec<[f32; 3]>, surface: u32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        SkateGrindCurve {
            points,
            closed: false,
            kind: 0,
            radius: 0.0,
            surface,
            unknown_flags: 0,
        },
    );
    e
}

/// Flat concrete at height 0 over 100 x 100 m, a spawn at a stated height, raised; three curves: a straight rail
/// 1 m up along east (scene x) 0..2, a rail with a right-angled corner, and one of two equal points.
fn world() -> (World, [Entity; 3]) {
    let mut w = World::new();
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
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "grinds".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([0.0, 0.0], Some(0.0))),
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
    let straight = curve(&mut w, vec![[0.0, 1.0, 0.0], [2.0, 1.0, 0.0]], 0);
    let corner = curve(&mut w, vec![[0.0, 1.0, 5.0], [3.0, 1.0, 5.0], [3.0, 1.0, 1.0]], 3);
    let point = curve(&mut w, vec![[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]], 1);
    (w, [straight, corner, point])
}

/// The straight rail: one run 2 m long, two rings of 8 (16 points, 16 triangles), the first point of each ring on the
/// line (the prism hangs under it, 30 mm in radius); the corner: two runs, 3 + 4 m; the point: an empty prism.
#[test]
fn curves_get_studios_prisms() {
    let (mut w, [straight, corner, point]) = world();
    let mut log = PassLog::default();
    PrepareReskateGrinds::new().run(&mut w, &mut log).unwrap();
    let s = w.get::<SkateGrindPrism>(straight).unwrap();
    assert_eq!(
        (s.positions.len(), s.triangles.len(), s.pieces, s.length),
        (16, 16, 1, 2.0)
    );
    let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-6);
    assert!(
        near(s.positions[0], [0.0, 1.0, 0.0]) && near(s.positions[8], [2.0, 1.0, 0.0]),
        "{:?}",
        s.positions
    );
    // every ring point within the 30 mm radius of the ring's centre, 60 mm across, none above the line
    for p in &s.positions {
        assert!(p[1] <= 1.0 + 1e-6 && p[1] >= 1.0 - 0.06 - 1e-6, "{p:?}");
    }
    let c = w.get::<SkateGrindPrism>(corner).unwrap();
    assert_eq!((c.pieces, c.length), (2, 7.0));
    assert_eq!(c.triangles.len(), 32);
    let p = w.get::<SkateGrindPrism>(point).unwrap();
    assert!(p.positions.is_empty() && p.triangles.is_empty());
    assert_eq!(log.counts["grind curves made prisms"], 2);
    assert_eq!(log.counts["grind curves with no usable points: empty prisms"], 1);
    assert_eq!(log.counts["grind prism runs"], 3);
    // a second run is refused: the prisms are made
    let e = PrepareReskateGrinds::new()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("already has its prism"), "{e}");
    // a surface Studio has no smooth-grind surface for is refused
    let (mut w, _) = world();
    curve(&mut w, vec![[0.0, 1.0, 0.0], [1.0, 1.0, 0.0]], 5);
    let e = PrepareReskateGrinds::new()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("surface 5"), "{e}");
}

fn write(w: &World, tag: &str) -> (primase_reskate::BuildReport, serde_json::Value) {
    let out_root = tmp(tag);
    let r = primase_reskate::write_map(
        w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("grinds".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    let map = serde_json::from_slice(&std::fs::read(out_root.join("grinds/map.json")).unwrap()).unwrap();
    (r, map)
}

fn grind_objects(map: &serde_json::Value) -> Vec<String> {
    map["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["grind_curve"] == serde_json::Value::Bool(true))
        .map(|o| o["name"].as_str().unwrap().to_owned())
        .collect()
}

/// The writer writes the prisms it is given, numbered by what they are (surface, then geometry: the straight metal rail
/// 0, the empty one on surface 1 is 1 and warned of, the concrete corner 2), and builds none: the curves of a world not
/// through the pass are reported unsupported and not written.
#[test]
fn the_writer_writes_prisms_and_builds_none() {
    let (mut w, _) = world();
    PrepareReskateGrinds::new()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let (r, map) = write(&w, "prepared");
    assert_eq!(grind_objects(&map), ["grind_00000", "grind_00002"]);
    assert!(
        r.warnings
            .iter()
            .any(|m| m == "grind curve 1 has no usable points"),
        "{:?}",
        r.warnings
    );
    assert!(
        r.unsupported.iter().all(|(what, _)| !what.contains("grind")),
        "{:?}",
        r.unsupported
    );
    let (w, _) = world();
    let (r, map) = write(&w, "unprepared");
    assert!(grind_objects(&map).is_empty());
    assert!(
        r.unsupported
            .iter()
            .any(|(what, n)| what.starts_with("skate.grind_curve") && *n == 3),
        "{:?}",
        r.unsupported
    );
}

/// A world without grind curves, run through the pass manager as a program runs it: nothing to make, nothing made.
#[test]
fn a_world_without_curves_gets_nothing() {
    let (mut w, curves) = world();
    for c in curves {
        w.despawn(c);
    }
    let mut report = primase_core::pass::RunReport::default();
    primase_core::pass::PassManager::new(primase_core::pass::PassRegistry::new())
        .run(
            &mut w,
            &primase_core::pass::Pipeline {
                passes: vec![Box::new(PrepareReskateGrinds::new())],
            },
            &mut report,
        )
        .unwrap();
    assert_eq!(w.iter::<SkateGrindPrism>().count(), 0);
    let (r, map) = write(&w, "no-curves");
    assert!(grind_objects(&map).is_empty());
    assert!(
        r.unsupported.iter().all(|(what, _)| !what.contains("grind")),
        "{:?}",
        r.unsupported
    );
}

/// Every rule prints back and reads back the same; bad values and unknown keys are refused.
#[test]
fn the_grind_options_round_trip_and_are_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    for given in [
        o(&[]),
        o(&[
            ("sides", "6"),
            ("tolerance", "0.01"),
            ("min_spacing", "0"),
            ("corner_degrees", "45"),
        ]),
    ] {
        let pass = prepare_reskate_grinds(&given).unwrap();
        assert_eq!(
            prepare_reskate_grinds(&pass.options()).unwrap().options(),
            pass.options()
        );
    }
    assert_eq!(PrepareReskateGrinds::new().rules(), PrismRules::STUDIO);
    for bad in [
        o(&[("sides", "2")]),
        o(&[("sides", "8.5")]),
        o(&[("tolerance", "-1")]),
        o(&[("min_spacing", "inf")]),
        o(&[("corner_degrees", "200")]),
        o(&[("radius", "0.03")]),
    ] {
        assert!(prepare_reskate_grinds(&bad).is_err(), "{bad:?}");
    }
}
