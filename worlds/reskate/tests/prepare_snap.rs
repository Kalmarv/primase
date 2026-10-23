//! `snap-reskate-markers`: the markers that ask for a snap put on the map's collision before the writer, which
//! snaps nothing; worked on a synthetic world over flat ground. That the pass's snap is the writer's former one, bit
//! for bit, is the collision module's own test; here: what the markers become and what the writer makes of them.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Entity, Surfaces, World};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{MarkerKind, Other, SkateMarker, SkateMarkerSnaps, Snap, SnapMode};
use primase_reskate::passes::{RaiseOptions, SNAP_RESKATE_MARKERS, SnapReskateMarkers};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-prepare-snap-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

fn marker(w: &mut World, kind: MarkerKind, position: ([f32; 2], Option<f32>), snap: Option<Snap>) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        SkateMarker {
            kind,
            position: Some(position),
            yaw: None,
            m: None,
            blueprint: None,
            snap,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    e
}

struct Fixture {
    w: World,
    spawn: Entity,
    on_ground: Entity,
    off_ground: Entity,
    pinned: Entity,
}

/// Flat concrete at scene height 3 over east -100..300, north 0..300; a spawn without a height (a street snap), a
/// travel point snapped to the ground from 10 m, one off the ground without a height (its snap finds nothing) and
/// one with a height and no snap.
fn fixture() -> Fixture {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [-100.0, 3.0, 0.0],
            [300.0, 3.0, 0.0],
            [-100.0, 3.0, -300.0],
            [300.0, 3.0, -300.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("snap", primase_reskate::passes::LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    let spawn = marker(&mut w, MarkerKind::Spawn, ([20.0, -20.0], None), None);
    let on_ground = marker(
        &mut w,
        MarkerKind::Travel,
        ([40.0, -30.0], Some(10.0)),
        Some(Snap::Mode(SnapMode::Ground)),
    );
    let off_ground = marker(&mut w, MarkerKind::Travel, ([900.0, -900.0], None), None);
    let pinned = marker(&mut w, MarkerKind::Travel, ([50.0, -50.0], Some(7.0)), None);
    Fixture {
        w,
        spawn,
        on_ground,
        off_ground,
        pinned,
    }
}

fn snapped(w: &World, e: Entity) -> &SkateMarker {
    w.get::<SkateMarker>(e).unwrap()
}

/// Flat ground: the spawn lands at height 3 with a spawn's 0.3 m lift written out, the ground travel point at 3, the
/// one off the ground keeps its place, its snap stated as none; the marker that asked for nothing is untouched. The writer
/// takes them as they are and reports nothing unresolved.
#[test]
fn markers_are_put_on_the_ground_and_the_writer_takes_them() {
    let Fixture {
        mut w,
        spawn,
        on_ground,
        off_ground,
        pinned,
    } = fixture();
    let before_pinned = snapped(&w, pinned).clone();
    let mut log = PassLog::default();
    SnapReskateMarkers::for_default_encoding()
        .run(&mut w, &mut log)
        .unwrap();
    let s = snapped(&w, spawn);
    let ([_, _], h) = s.position.unwrap();
    assert_eq!((h, s.snap.as_ref(), s.lift), (Some(3.0), None, Some(0.3)));
    assert_eq!(snapped(&w, on_ground).position, Some(([40.0, -30.0], Some(3.0))));
    assert!(snapped(&w, on_ground).snap.is_none());
    let off = snapped(&w, off_ground);
    assert_eq!(
        (off.position, off.snap.as_ref()),
        (Some(([900.0, -900.0], None)), Some(&Snap::Mode(SnapMode::None)))
    );
    assert_eq!(snapped(&w, pinned), &before_pinned);
    assert_eq!(log.counts["markers snapped"], 2);
    assert_eq!(
        log.counts["markers whose snap found nothing: kept where they are"],
        1
    );
    assert_eq!(
        w.resource::<SkateMarkerSnaps>().unwrap(),
        SnapReskateMarkers::for_default_encoding().basis()
    );
    // the writer: the spawn 0.3 m over the ground, the ground travel point 0.05 m (its default lift) over it
    let out_root = tmp("snapped");
    let r = primase_reskate::write_map(
        &w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: out_root.clone(),
            name: Some("snap".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    assert!(
        r.omitted
            .iter()
            .chain(&r.approximated)
            .all(|(what, _)| !what.contains("snap")),
        "{:?} {:?}",
        r.omitted,
        r.approximated
    );
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out_root.join("snap/map.json")).unwrap()).unwrap();
    let y = |v: &serde_json::Value| v["position"][1].as_f64().unwrap() as f32;
    assert_eq!(y(&map["spawn"]), 3.0 + 0.3);
    let travel = map["travel_points"].as_array().unwrap();
    assert_eq!(
        travel.len(),
        2,
        "the one off the ground has no height and is left out"
    );
    assert_eq!((y(&travel[0]), y(&travel[1])), (3.0 + 0.05, 7.0 + 0.05));
    // a second run is refused: the markers are snapped
    let e = SnapReskateMarkers::for_default_encoding()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("already snapped"), "{e}");
}

/// Without the pass the writer snaps nothing and says so; with the pass at other settings than its own it refuses
/// the map.
#[test]
fn the_writer_reports_unsnapped_markers_and_refuses_other_settings() {
    let Fixture { mut w, .. } = fixture();
    // the spawn has no height and nothing put it anywhere: the writer has no height for it
    let e = primase_reskate::write_map(
        &w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: tmp("unsnapped"),
            name: Some("snap".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("no height"), "{e}");
    SnapReskateMarkers::for_default_encoding()
        .with_limit(1000.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let e = primase_reskate::write_map(
        &w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: tmp("other"),
            name: Some("snap".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("snapped for the writer settings"), "{e}");
}

/// Every setting prints back and reads back the same; bad values and unknown keys are refused.
#[test]
fn the_snap_options_round_trip_and_are_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    for given in [
        o(&[]),
        o(&[
            ("limit", "1500"),
            ("offset_x", "-12.5"),
            ("offset_y", "40"),
            ("min_instances", "4"),
            ("min_instance_tris", "0"),
            ("max_render_edge", "60"),
        ]),
    ] {
        let pass = SNAP_RESKATE_MARKERS.instance(&given).unwrap();
        assert_eq!(
            SNAP_RESKATE_MARKERS.instance(&pass.options()).unwrap().options(),
            pass.options()
        );
    }
    assert_eq!(
        SNAP_RESKATE_MARKERS.instance(&o(&[])).unwrap().options(),
        SnapReskateMarkers::for_default_encoding().options()
    );
    for bad in [
        o(&[("limit", "20")]),
        o(&[("limit", "nan")]),
        o(&[("offset_x", "inf")]),
        o(&[("min_instances", "-1")]),
        o(&[("min_instance_tris", "1.5")]),
        o(&[("max_render_edge", "-2")]),
        o(&[("snap", "street")]),
    ] {
        assert!(SNAP_RESKATE_MARKERS.instance(&bad).is_err(), "{bad:?}");
    }
}
