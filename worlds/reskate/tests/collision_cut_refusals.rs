//! The passes that cut or measure a world's collision soups check what they index: a soup whose surfaces, rock flags or
//! indices do not fit its triangles is a named error, not a panic (a pass runs on a world it may not have verified).

#![allow(clippy::unwrap_used)]

use primase_core::pass::{Pass, PassLog};
use primase_core::{Collision, World};
use primase_reskate::components::{SkateRock, SkateScene};
use primase_reskate::passes::{ClipReskateRegion, SummarizeReskateFloor};

fn world(c: Collision, rock: Option<Vec<bool>>) -> World {
    let mut w = World::new();
    w.insert_resource(SkateScene {
        name: "t".into(),
        ..Default::default()
    });
    let e = w.spawn();
    w.insert(e, c);
    if let Some(r) = rock {
        w.insert(e, SkateRock(r));
    }
    w
}

fn soup() -> Collision {
    Collision {
        positions: vec![
            [1.0, 0.0, -1.0],
            [3.0, 0.0, -1.0],
            [2.0, 0.0, -4.0],
            [30.0, 0.0, -1.0],
        ],
        triangles: vec![[0, 1, 2], [1, 2, 3]],
        surfaces: vec![0, 0],
    }
}

fn region(w: &mut World) -> Result<(), String> {
    ClipReskateRegion::new([0.0, 0.0, 10.0, 10.0], 5.0)
        .unwrap()
        .run(w, &mut PassLog::default())
}

fn floor(w: &mut World) -> Result<(), String> {
    SummarizeReskateFloor.run(w, &mut PassLog::default())
}

/// A well-formed soup is still cut and measured.
#[test]
fn a_well_formed_soup_is_cut_and_measured() {
    let mut w = world(soup(), Some(vec![false, true]));
    region(&mut w).unwrap();
    let e = w.iter::<Collision>().next().unwrap().0;
    assert_eq!(w.get::<Collision>(e).unwrap().triangles, vec![[0, 1, 2]]);
    assert_eq!(w.get::<SkateRock>(e).unwrap().0, vec![false]);
    let mut w = world(soup(), None);
    floor(&mut w).unwrap();
}

/// More surfaces than triangles: the cut walked a keep list of one mark a triangle and unwrapped past its end.
#[test]
fn surfaces_that_do_not_match_the_triangles_are_refused() {
    let mut c = soup();
    c.surfaces.push(0);
    for run in [region, floor] {
        let mut w = world(c.clone(), None);
        let e = run(&mut w).unwrap_err();
        assert!(e.contains("3 surfaces for 2 triangles"), "{e}");
    }
}

#[test]
fn rock_flags_that_do_not_match_the_triangles_are_refused() {
    let mut w = world(soup(), Some(vec![true]));
    let e = region(&mut w).unwrap_err();
    assert!(e.contains("1 rock flags for 2 triangles"), "{e}");
}

/// A triangle naming a vertex the soup does not have.
#[test]
fn an_index_past_the_vertices_is_refused() {
    let mut c = soup();
    c.triangles[1] = [1, 2, 9];
    for run in [region, floor] {
        let mut w = world(c.clone(), None);
        let e = run(&mut w).unwrap_err();
        assert!(e.contains("names vertex 9 of 4"), "{e}");
    }
}
