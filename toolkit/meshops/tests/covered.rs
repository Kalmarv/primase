//! `drop-covered-surfaces`: an opaque floor a deck covers from 3.7 mm above (The Deck's bowl decks over the plaza's
//! concrete) is left out; what is not covered everywhere, or is covered by something that does not hide it, stays.
//! Hand-made squares, y up, facing up.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World};
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
    let floor = place(&mut w, "plaza", vec![square(concrete, 0.0, 0.0, 10.0, 0.0)], &[[100.0, 40.0, 0.0]]);
    let deck = place(&mut w, "bowl", vec![square(boards, -1.0, -1.0, 12.0, 0.0037)], &[[100.0, 40.0, 0.0]]);
    let log = run(&mut w, 0.01);
    assert_eq!(triangles(&w, floor), vec![0]);
    assert_eq!(triangles(&w, deck), vec![2]);
    assert_eq!(
        log.omitted["triangles covered by an opaque surface in front, left out: concrete"],
        2,
        "{:?}",
        log.omitted
    );
}

/// What stays: a floor half under the deck (partly covered, counted), a deck 2 cm above with a 1 cm gap, an
/// alpha-tested deck, a double-sided floor (counted), a coincident copy (counted), and a ceiling's underside facing
/// the other way.
#[test]
fn what_is_not_hidden_everywhere_stays() {
    // half covered
    let mut w = World::new();
    let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
    let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
    let floor = place(&mut w, "plaza", vec![square(concrete, 0.0, 0.0, 10.0, 0.0)], &[[0.0; 3]]);
    place(&mut w, "bowl", vec![square(boards, 5.0, -1.0, 6.0, 0.0037)], &[[0.0; 3]]);
    let log = run(&mut w, 0.01);
    assert_eq!(triangles(&w, floor), vec![2]);
    assert!(log.counts["triangles partly covered (kept)"] >= 1, "{:?}", log.counts);

    let cases: [(&str, AlphaMode, bool, f32, bool); 5] = [
        ("too far above", AlphaMode::Opaque, false, 0.02, false),
        ("alpha-tested deck", AlphaMode::Mask { cutoff: 0.5 }, false, 0.0037, false),
        ("double-sided floor", AlphaMode::Opaque, true, 0.0037, false),
        ("coincident copy", AlphaMode::Opaque, false, 0.0, false),
        ("underside of a ceiling", AlphaMode::Opaque, false, 0.0037, true),
    ];
    for (what, deck_alpha, floor_double, height, flipped) in cases {
        let mut w = World::new();
        let concrete = material(&mut w, "concrete", AlphaMode::Opaque, floor_double);
        let boards = material(&mut w, "boards", deck_alpha, false);
        let floor = place(&mut w, "plaza", vec![square(concrete, 0.0, 0.0, 10.0, 0.0)], &[[0.0; 3]]);
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
            assert_eq!(log.counts["triangles on a coincident surface (kept)"], 4, "{what}");
        }
    }
}

/// A prototype placed twice with a deck over one placement only keeps its triangles (each placement draws the same
/// mesh); with a deck over both they go.
#[test]
fn a_shared_prototype_goes_only_where_every_placement_is_covered() {
    for (decks, left) in [(1usize, 2usize), (2, 0)] {
        let mut w = World::new();
        let concrete = material(&mut w, "concrete", AlphaMode::Opaque, false);
        let boards = material(&mut w, "boards", AlphaMode::Opaque, false);
        let at = [[0.0, 0.0, 0.0], [50.0, 0.0, 0.0]];
        let floor = place(&mut w, "plaza", vec![square(concrete, 0.0, 0.0, 10.0, 0.0)], &at);
        place(&mut w, "bowl", vec![square(boards, -1.0, -1.0, 12.0, 0.0037)], &at[..decks]);
        run(&mut w, 0.01);
        assert_eq!(triangles(&w, floor), vec![left], "decks over {decks} placements");
    }
}

/// The gap is required, finite and above 0.1 mm; it prints back.
#[test]
fn the_gap_is_required_and_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options { pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect() };
    let p = DROP_COVERED_SURFACES.instance(&o(&[("gap", "0.01")])).unwrap();
    assert_eq!(p.options(), o(&[("gap", "0.01")]));
    for bad in [o(&[]), o(&[("gap", "0")]), o(&[("gap", "inf")]), o(&[("gap", "x")])] {
        assert!(DROP_COVERED_SURFACES.instance(&bad).is_err(), "{bad:?}");
    }
}
