//! `drop-covered-surfaces`: an opaque floor a deck covers from 3.7 mm above (The Deck's bowl decks over the plaza's
//! concrete) is left out, whole or cut to what the deck does not cover; what is not covered in every placement, or is
//! covered by something that does not hide it, stays. Hand-made squares, y up, facing up.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Entity, Instance, Mesh, MeshFile, Name, Primitive, Semantic, Stream, Topology, Transform, World,
};
use primase_meshops::{DROP_COVERED_SURFACES, DropCoveredSurfaces};

fn material(w: &mut World, name: &str, alpha: AlphaMode, double_sided: bool) -> Entity {
    let m = w.spawn();
    w.insert(m, Name(name.into()));
    w.insert(
        m,
        PbrMaterial {
            alpha_mode: alpha,
            double_sided,
            ..PbrMaterial::default()
        },
    );
    m
}

/// A square of side `s` at `x0`, `z0` and height `y`, facing up, as two triangles.
fn square(m: Entity, x0: f32, z0: f32, s: f32, y: f32) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![
                [x0, y, z0],
                [x0, y, z0 + s],
                [x0 + s, y, z0],
                [x0 + s, y, z0 + s],
            ]),
        )]),
        indices: Some(vec![0, 1, 2, 2, 1, 3]),
        material: Some(m),
    }
}

/// `square` with texture coordinates of its position over 10 (x, z), to see a cut interpolate them.
fn square_uv(m: Entity, x0: f32, z0: f32, s: f32, y: f32) -> Primitive {
    let mut p = square(m, x0, z0, s, y);
    let Stream::F32x3(pos) = &p.streams[&Semantic::Position] else {
        unreachable!()
    };
    let uv = pos.iter().map(|q| [q[0] / 10.0, q[2] / 10.0]).collect();
    p.streams.insert(Semantic::TexCoord(0), Stream::F32x2(uv));
    p
}

/// A square of side `s` at `x0`, `z0` rising along x: height `y0` at x = 0, `slope` metres a metre.
fn tilted(m: Entity, x0: f32, z0: f32, s: f32, y0: f32, slope: f32) -> Primitive {
    let mut p = square(m, x0, z0, s, 0.0);
    if let Some(Stream::F32x3(pos)) = p.streams.get_mut(&Semantic::Position) {
        for q in pos {
            q[1] = y0 + slope * q[0];
        }
    }
    p
}

/// A prototype of `prims`, placed at each of `at`.
fn place(w: &mut World, name: &str, prims: Vec<Primitive>, at: &[[f32; 3]]) -> Entity {
    let proto = w.spawn();
    w.insert(proto, Name(name.into()));
    w.insert(proto, Mesh::new(prims));
    for &a in at {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = a;
        w.insert(e, t);
    }
    proto
}

fn triangles(w: &World, mesh: Entity) -> Vec<usize> {
    w.get::<Mesh>(mesh)
        .unwrap()
        .primitives()
        .iter()
        .map(|p| p.indices.as_ref().map_or(0, |i| i.len() / 3))
        .collect()
}

/// The drawn triangles of a mesh's first primitive: corners (local), area and whether each faces up.
fn drawn(w: &World, mesh: Entity) -> Vec<([[f32; 3]; 3], f64, bool)> {
    let p = &w.get::<Mesh>(mesh).unwrap().primitives()[0];
    let Stream::F32x3(pos) = &p.streams[&Semantic::Position] else {
        unreachable!()
    };
    p.indices
        .as_ref()
        .unwrap()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let c = t.map(|i| pos[i as usize]);
            let a = [0, 1, 2].map(|k| f64::from(c[1][k] - c[0][k]));
            let b = [0, 1, 2].map(|k| f64::from(c[2][k] - c[0][k]));
            let n = [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ];
            (
                c,
                (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0,
                n[1] > 0.0,
            )
        })
        .collect()
}

fn run(w: &mut World, gap: f64) -> PassLog {
    let mut log = PassLog::default();
    DropCoveredSurfaces::new(gap).unwrap().run(w, &mut log).unwrap();
    log
}

/// A 10 m floor under a 12 m deck 3.7 mm above it, another placement: the floor's two triangles go, the deck stays,
/// reported by the floor's material.
#[test]
fn a_floor_a_deck_covers_from_just_above_is_left_out() {
    let mut w = World::new();
    let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
    let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
    let floor = place(
        &mut w,
        "plaza",
        vec![square(concrete, 0.0, 0.0, 10.0, 0.0)],
        &[[100.0, 40.0, 0.0]],
    );
    let deck = place(
        &mut w,
        "bowl",
        vec![square(boards, -1.0, -1.0, 12.0, 0.0037)],
        &[[100.0, 40.0, 0.0]],
    );
    let log = run(&mut w, 0.01);
    assert_eq!(triangles(&w, floor), vec![0]);
    assert_eq!(triangles(&w, deck), vec![2]);
    assert_eq!(
        log.omitted["triangles covered by an opaque surface in front, left out: concrete"], 2,
        "{:?}",
        log.omitted
    );
}

/// A floor half under the deck (x from 5 m, z past both sides) is cut to the other half: 50 m2 left, all of it at x <= 5, facing up as
/// before, its texture coordinates interpolated (still the position over 10), and the cut of the diagonal both
/// triangles share one vertex (4 + 3: at x = 5 on the near edge, the diagonal and the far edge).
#[test]
fn a_floor_half_under_a_deck_is_cut_to_the_uncovered_half() {
    let mut w = World::new();
    let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
    let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
    let floor = place(
        &mut w,
        "plaza",
        vec![square_uv(concrete, 0.0, 0.0, 10.0, 0.0)],
        &[[0.0; 3]],
    );
    let deck = place(
        &mut w,
        "bowl",
        vec![square(boards, 5.0, -1.0, 12.0, 0.0037)],
        &[[0.0; 3]],
    );
    let log = run(&mut w, 0.01);
    let tris = drawn(&w, floor);
    let area: f64 = tris.iter().map(|t| t.1).sum();
    assert!((area - 50.0).abs() < 1e-3, "{area} m2 left: {tris:?}");
    assert!(tris.iter().all(|t| t.2), "winding kept");
    assert!(
        tris.iter().all(|t| t.0.iter().all(|c| c[0] <= 5.0 + 1e-5)),
        "{tris:?}"
    );
    let p = &w.get::<Mesh>(floor).unwrap().primitives()[0];
    let (Stream::F32x3(pos), Stream::F32x2(uv)) = (
        &p.streams[&Semantic::Position],
        &p.streams[&Semantic::TexCoord(0)],
    ) else {
        unreachable!()
    };
    assert_eq!(pos.len(), 7, "{pos:?}");
    for (q, t) in pos.iter().zip(uv) {
        assert!(
            (q[0] / 10.0 - t[0]).abs() < 1e-6 && (q[2] / 10.0 - t[1]).abs() < 1e-6,
            "{q:?} {t:?}"
        );
    }
    assert_eq!(triangles(&w, deck), vec![2]);
    assert_eq!(
        log.omitted["triangles partly covered by an opaque surface in front, cut to the rest: concrete"], 2,
        "{:?}",
        log.omitted
    );
    assert_eq!(log.counts["triangles the cut ones became"], 3, "{:?}", log.counts);
}

/// A deck over the whole floor rising from 2 mm (x = 0) to 22 mm (x = 10) covers it within the 1 cm gap only up to
/// x = 4: the floor keeps x from 4 to 10, 60 m2.
#[test]
fn a_tilted_deck_covers_only_where_it_lies_within_the_gap() {
    let mut w = World::new();
    let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
    let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
    let floor = place(
        &mut w,
        "plaza",
        vec![square(concrete, 0.0, 0.0, 10.0, 0.0)],
        &[[0.0; 3]],
    );
    place(
        &mut w,
        "bowl",
        vec![tilted(boards, -1.0, -1.0, 12.0, 0.002, 0.002)],
        &[[0.0; 3]],
    );
    run(&mut w, 0.01);
    let tris = drawn(&w, floor);
    let area: f64 = tris.iter().map(|t| t.1).sum();
    assert!((area - 60.0).abs() < 1e-3, "{area} m2 left: {tris:?}");
    assert!(
        tris.iter().all(|t| t.0.iter().all(|c| c[0] >= 4.0 - 1e-4)),
        "{tris:?}"
    );
}

/// What stays: a deck 2 cm above with a 1 cm gap, an alpha-tested deck, a double-sided floor (counted), a coincident
/// copy (counted), and a ceiling's underside facing the other way.
#[test]
fn what_is_not_hidden_stays() {
    let cases: [(&str, AlphaMode, bool, f32, bool); 5] = [
        ("too far above", AlphaMode::Opaque, false, 0.02, false),
        (
            "alpha-tested deck",
            AlphaMode::Mask { cutoff: 0.5 },
            false,
            0.0037,
            false,
        ),
        ("double-sided floor", AlphaMode::Opaque, true, 0.0037, false),
        ("coincident copy", AlphaMode::Opaque, false, 0.0, false),
        ("underside of a ceiling", AlphaMode::Opaque, false, 0.0037, true),
    ];
    for (what, deck_alpha, floor_double, height, flipped) in cases {
        let mut w = World::new();
        let concrete = material(&mut w, "concrete", AlphaMode::Opaque, floor_double);
        let boards = material(&mut w, "boards", deck_alpha, false);
        let floor = place(
            &mut w,
            "plaza",
            vec![square(concrete, 0.0, 0.0, 10.0, 0.0)],
            &[[0.0; 3]],
        );
        let mut cover = square(boards, -1.0, -1.0, 12.0, height);
        if flipped {
            cover.indices = Some(vec![0, 2, 1, 2, 3, 1]);
        }
        place(&mut w, "bowl", vec![cover], &[[0.0; 3]]);
        let log = run(&mut w, 0.01);
        assert_eq!(triangles(&w, floor), vec![2], "{what}: {:?}", log.omitted);
        if floor_double {
            assert_eq!(log.counts["double-sided triangles covered (kept)"], 2, "{what}");
        }
        if height == 0.0 {
            // the floor's two and the deck's two: each lies on the other
            assert_eq!(
                log.counts["triangles on a coincident surface (kept)"], 4,
                "{what}"
            );
        }
    }
}

/// A prototype placed twice with a deck over one placement only keeps its triangles (each placement draws the same
/// mesh); with a deck over both they go; with decks over x 0-6 of one and 4-10 of the other (each past both z sides) only x 4-6 goes (80 m2
/// left).
#[test]
fn a_shared_prototype_loses_only_what_every_placement_covers() {
    let mut w = World::new();
    let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
    let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
    let floor = place(
        &mut w,
        "plaza",
        vec![square(concrete, 0.0, 0.0, 10.0, 0.0)],
        &[[0.0; 3], [50.0, 0.0, 0.0]],
    );
    place(
        &mut w,
        "bowl a",
        vec![square(boards, -6.0, -1.0, 12.0, 0.0037)],
        &[[0.0; 3]],
    );
    place(
        &mut w,
        "bowl b",
        vec![square(boards, 54.0, -1.0, 12.0, 0.0037)],
        &[[0.0; 3]],
    );
    run(&mut w, 0.01);
    let tris = drawn(&w, floor);
    let area: f64 = tris.iter().map(|t| t.1).sum();
    assert!((area - 80.0).abs() < 1e-3, "{area} m2 left: {tris:?}");
    assert!(
        tris.iter()
            .all(|t| t.0.iter().all(|c| c[0] <= 4.0 + 1e-4) || t.0.iter().all(|c| c[0] >= 6.0 - 1e-4)),
        "{tris:?}"
    );

    for (decks, left) in [(1usize, 2usize), (2, 0)] {
        let mut w = World::new();
        let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
        let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
        let at = [[0.0, 0.0, 0.0], [50.0, 0.0, 0.0]];
        let floor = place(&mut w, "plaza", vec![square(concrete, 0.0, 0.0, 10.0, 0.0)], &at);
        place(
            &mut w,
            "bowl",
            vec![square(boards, -1.0, -1.0, 12.0, 0.0037)],
            &at[..decks],
        );
        run(&mut w, 0.01);
        assert_eq!(triangles(&w, floor), vec![left], "decks over {decks} placements");
    }
}

/// A floor prototype placed 40 times, each placement under two identical decks over half of it (a deck drawn twice,
/// coincident copies being common): what every placement covers is still that half, found at once (each
/// placement's covered pieces made disjoint before the placements are intersected, so their count does not double
/// per placement), and the floor keeps the other half.
#[test]
fn many_placements_under_doubled_decks_lose_the_half_they_all_cover() {
    let mut w = World::new();
    let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
    let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
    let at: Vec<[f32; 3]> = (0..40).map(|i| [100.0 * i as f32, 0.0, 0.0]).collect();
    let floor = place(&mut w, "plaza", vec![square(concrete, 0.0, 0.0, 10.0, 0.0)], &at);
    for a in &at {
        for copy in ["bowl", "bowl again"] {
            place(
                &mut w,
                copy,
                vec![square(boards, a[0] + 5.0, -1.0, 12.0, 0.0037)],
                &[[0.0; 3]],
            );
        }
    }
    let started = std::time::Instant::now();
    run(&mut w, 0.01);
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    let tris = drawn(&w, floor);
    let area: f64 = tris.iter().map(|t| t.1).sum();
    assert!((area - 50.0).abs() < 1e-3, "{area} m2 left: {tris:?}");
}

/// A mesh still linked to its file loses the link once changed (the pass declares the removal); one left as it was
/// keeps it.
#[test]
fn a_changed_mesh_loses_its_file_link() {
    let mut w = World::new();
    let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
    let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
    let floor = place(
        &mut w,
        "plaza",
        vec![square(concrete, 0.0, 0.0, 10.0, 0.0)],
        &[[0.0; 3]],
    );
    let deck = place(
        &mut w,
        "bowl",
        vec![square(boards, 5.0, -1.0, 6.0, 0.0037)],
        &[[0.0; 3]],
    );
    for (m, mat) in [(floor, concrete), (deck, boards)] {
        w.insert(
            m,
            MeshFile {
                path: "plaza.glb".into(),
                mesh: 0,
                materials: vec![Some(mat)],
            },
        );
    }
    run(&mut w, 0.01);
    assert!(!w.has::<MeshFile>(floor));
    assert!(w.has::<MeshFile>(deck));
    assert!(
        DropCoveredSurfaces::new(0.01)
            .unwrap()
            .removes()
            .contains(&"core.mesh_file")
    );
}

/// The gap is required, finite and above 0.1 mm; it prints back.
#[test]
fn the_gap_is_required_and_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    let p = DROP_COVERED_SURFACES.instance(&o(&[("gap", "0.01")])).unwrap();
    assert_eq!(p.options(), o(&[("gap", "0.01")]));
    for bad in [
        o(&[]),
        o(&[("gap", "0")]),
        o(&[("gap", "inf")]),
        o(&[("gap", "x")]),
    ] {
        assert!(DROP_COVERED_SURFACES.instance(&bad).is_err(), "{bad:?}");
    }
}
