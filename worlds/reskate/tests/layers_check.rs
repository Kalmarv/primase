//! The layering gate's checks (`primase_reskate::postcompile::layers::check`) on hand-built levels: one floor or wall
//! case per check, failing and passing, with the classes given (no game data).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_reskate::postcompile::layers::{Check, Class, Limits, Report, check, check_threads};
use primase_reskate::postcompile::level::{Level, Mesh, Placement, Section};

const OPAQUE: u64 = 1;
const DECAL: u64 = 2;

/// A horizontal square `side` metres wide at height `y`, facing up, as an `n` x `n` grid.
fn square(side: f32, y: f32, n: u32, key: u64) -> Section {
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            positions.push([side * i as f32 / n as f32, y, -side * j as f32 / n as f32]);
        }
    }
    let mut triangles = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            let (b, c, d) = (a + 1, a + n + 1, a + n + 2);
            triangles.push([a, b, c]);
            triangles.push([b, d, c]);
        }
    }
    Section {
        key,
        half: false,
        position_only: false,
        positions,
        triangles,
    }
}

/// A level of one-LOD meshes, each placed once at its translation.
fn level(meshes: Vec<(&str, Section, [f64; 3])>) -> Level {
    let mut l = Level::default();
    for (i, (name, s, trans)) in meshes.into_iter().enumerate() {
        l.meshes.push(Mesh {
            name: name.to_string(),
            lods: vec![vec![s]],
            lod_distances: None,
            lod_group: None,
            stock: false,
        });
        l.placements.push(Placement {
            mesh: i,
            rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            trans,
            document: "level".into(),
        });
    }
    l
}

fn run(l: &Level) -> Report {
    let classes = BTreeMap::from([(OPAQUE, Class::DepthWriting), (DECAL, Class::Decal)]);
    check(l, &classes, &Limits::default())
}

fn area(r: &Report, c: Check, mesh: &str) -> f64 {
    r.findings
        .get(&c)
        .and_then(|f| f.by_mesh.get(mesh).copied())
        .unwrap_or(0.0)
}

#[test]
fn depth_writing_layers_fight_below_the_clearance() {
    // a 2 x 2 m opaque square over a 4 x 4 m base: re-triangulated (cells of 2/3 m over cells of 1 m), so not a copy
    for (lift, fights) in [(0.0, true), (0.0004, true), (0.0005, false), (0.004, false)] {
        let r = run(&level(vec![
            ("base", square(4.0, 0.0, 4, OPAQUE), [0.0; 3]),
            ("top", square(2.0, lift, 3, OPAQUE), [1.0, 0.0, -1.0]),
        ]));
        let a = area(&r, Check::DepthWriting, "base") + area(&r, Check::DepthWriting, "top");
        assert_eq!(a > 3.9, fights, "lift {lift}: {a}");
        assert_eq!(r.passed(&Limits::default()), !fights, "lift {lift}");
    }
}

#[test]
fn a_bit_identical_copy_is_a_duplicate_not_a_fight() {
    let r = run(&level(vec![
        ("base", square(2.0, 0.0, 2, OPAQUE), [0.0; 3]),
        ("copy", square(2.0, 0.0, 2, OPAQUE), [0.0; 3]),
    ]));
    assert!(!r.findings.contains_key(&Check::DepthWriting));
    assert!(r.findings[&Check::Duplicate].area_m2 > 3.9);
    assert!(r.passed(&Limits::default()));
}

#[test]
fn a_decal_on_its_base_passes_and_under_it_or_a_cover_fails() {
    // on the base, and a little above it (within the lift): clean
    for lift in [0.0, 0.0005, 0.007] {
        let r = run(&level(vec![
            ("base", square(4.0, 0.0, 4, OPAQUE), [0.0; 3]),
            ("decal", square(2.0, lift, 2, DECAL), [1.0, 0.0, -1.0]),
        ]));
        assert!(r.passed(&Limits::default()), "lift {lift}: {:?}", r.findings);
    }
    // under the base: hidden near
    let r = run(&level(vec![
        ("base", square(4.0, 0.0, 4, OPAQUE), [0.0; 3]),
        ("decal", square(2.0, -0.0005, 2, DECAL), [1.0, 0.0, -1.0]),
    ]));
    assert!((area(&r, Check::Behind, "decal") - 4.0).abs() < 1e-6);
    // on its base under a cover 5 cm above (a floor: within the 15 cm reach): leaks far; half covered
    let r = run(&level(vec![
        ("base", square(4.0, 0.0, 4, OPAQUE), [0.0; 3]),
        ("decal", square(2.0, 0.0, 2, DECAL), [1.0, 0.0, -1.0]),
        ("cover", square(1.0, 0.05, 1, OPAQUE), [1.0, 0.0, -1.0]),
    ]));
    assert!(
        (area(&r, Check::Covered, "decal") - 1.0).abs() < 1e-6,
        "{:?}",
        r.findings
    );
    // floating 2 cm over its base, and over nothing
    let r = run(&level(vec![
        ("base", square(4.0, 0.0, 4, OPAQUE), [0.0; 3]),
        ("decal", square(2.0, 0.02, 2, DECAL), [1.0, 0.0, -1.0]),
        ("alone", square(2.0, 0.0, 2, DECAL), [10.0, 0.0, 0.0]),
    ]));
    assert!((area(&r, Check::Floating, "decal") - 4.0).abs() < 1e-6);
    assert!((area(&r, Check::Unbacked, "alone") - 4.0).abs() < 1e-6);
    assert!(!r.passed(&Limits::default()));
}

#[test]
fn overlapping_decals_need_boxes_apart_along_the_normal_behind_the_surface() {
    let base = || ("base", square(4.0, 0.0, 4, OPAQUE), [0.0; 3]);
    // a tie: identical boxes
    let r = run(&level(vec![
        base(),
        ("a", square(2.0, 0.0, 2, DECAL), [1.0, 0.0, -1.0]),
        ("b", square(2.0, 0.0, 1, DECAL), [1.0, 0.0, -1.0]),
    ]));
    assert!(r.findings[&Check::Order].area_m2 > 3.9);
    // boxes padded down below the floor, 1 m apart: one order from every view above it
    let padded = |depth: f32, n: u32| {
        let mut s = square(2.0, 0.0, n, DECAL);
        let k = s.positions.len() as u32;
        s.positions
            .extend([[0.0, -depth, 0.0], [0.05, -depth, 0.0], [0.0, -depth, -0.05]]);
        s.triangles.push([k, k + 1, k + 2]);
        s
    };
    let r = run(&level(vec![
        base(),
        ("a", padded(30.0, 2), [1.0, 0.0, -1.0]),
        ("b", padded(28.0, 1), [1.0, 0.0, -1.0]),
    ]));
    assert!(
        !r.findings.contains_key(&Check::Order),
        "{:?}",
        r.findings.get(&Check::Order)
    );
    // the same boxes lifted ABOVE the floor (padding up): the halfway plane is in front, the order turns with the
    // camera's height
    let up = |height: f32, n: u32| {
        let mut s = square(2.0, 0.0, n, DECAL);
        let k = s.positions.len() as u32;
        s.positions
            .extend([[0.0, height, 0.0], [0.05, height, 0.0], [0.0, height, -0.05]]);
        s.triangles.push([k, k + 1, k + 2]);
        s
    };
    let r = run(&level(vec![
        base(),
        ("a", up(30.0, 2), [1.0, 0.0, -1.0]),
        ("b", up(28.0, 1), [1.0, 0.0, -1.0]),
    ]));
    assert!(r.findings[&Check::Order].area_m2 > 3.9);
}

/// The report does not depend on the thread count: a base spanning nine tiles, decals across tile borders (on it,
/// under it, under a cover, floating, alone), a fight, and a two-LOD decal whose coarse LOD sinks into the base, whose
/// base is found in another tile than the one the coarse pair is checked in.
#[test]
fn the_report_is_the_same_on_one_thread_and_many() {
    let mut l = level(vec![
        ("base", square(150.0, 0.0, 30, OPAQUE), [-10.0, 0.0, 10.0]),
        ("on", square(4.0, 0.0, 2, DECAL), [62.0, 0.0, -10.0]),
        ("under", square(3.0, -0.0005, 3, DECAL), [126.0, 0.0, -62.0]),
        ("covered", square(2.0, 0.0, 2, DECAL), [20.0, 0.0, -63.0]),
        ("cover", square(1.0, 0.05, 1, OPAQUE), [20.5, 0.0, -63.5]),
        ("floating", square(2.0, 0.02, 2, DECAL), [100.0, 0.0, -100.0]),
        ("alone", square(2.0, 0.0, 2, DECAL), [300.0, 0.0, 0.0]),
        ("top", square(2.0, 0.0002, 3, OPAQUE), [63.0, 0.0, -127.0]),
    ]);
    // a decal at LOD 0 on the base, its LOD 1 a little under it, straddling the tile border at x = 64
    l.meshes.push(Mesh {
        name: "lodded".into(),
        lods: vec![
            vec![square(4.0, 0.001, 2, DECAL)],
            vec![square(4.0, -0.01, 1, DECAL)],
        ],
        lod_distances: Some(vec![30.0]),
        lod_group: None,
        stock: false,
    });
    l.placements.push(Placement {
        mesh: l.meshes.len() - 1,
        rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        trans: [62.5, 0.0, -30.0],
        document: "level".into(),
    });
    let classes = BTreeMap::from([(OPAQUE, Class::DepthWriting), (DECAL, Class::Decal)]);
    let one = check_threads(&l, &classes, &Limits::default(), 1);
    for threads in [2, 8] {
        assert_eq!(
            check_threads(&l, &classes, &Limits::default(), threads),
            one,
            "{threads} threads"
        );
    }
    for c in [
        Check::Behind,
        Check::Covered,
        Check::Floating,
        Check::Unbacked,
        Check::DepthWriting,
        Check::Lod,
    ] {
        assert!(one.findings.contains_key(&c), "{c:?}: {:?}", one.findings.keys());
    }
}
