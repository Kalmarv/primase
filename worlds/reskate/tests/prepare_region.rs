//! `clip-reskate-region`: the region the program chooses, the placements outside it marked clipped and the world's
//! collision cut at it, worked by hand on a synthetic world.

#![allow(clippy::unwrap_used)]

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Entity, Instance, Transform, World};
use primase_reskate::dialect::{SkatePlacement, SkateRock, SkateScene};
use primase_reskate::prepare::{ClipReskateRegion, clip_reskate_region};

/// A placement at scene (x, 0, z) of a prototype with nothing on it.
fn place(w: &mut World, proto: Entity, x: f32, z: f32, clip: Option<bool>) -> Entity {
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
    w.insert(
        e,
        SkatePlacement {
            clip,
            ..SkatePlacement::default()
        },
    );
    e
}

/// The region [0, 0, 10, 10) (east, north): scene x in [0, 10), z in (-10, 0].
#[test]
fn placements_are_marked_and_world_collision_is_cut() {
    let mut w = World::new();
    w.insert_resource(SkateScene {
        name: "clip".into(),
        ..Default::default()
    });
    let proto = w.spawn();
    let inside = place(&mut w, proto, 5.0, -5.0, None);
    let east = place(&mut w, proto, 10.0, -5.0, None); // east = 10: outside (half-open)
    let south = place(&mut w, proto, 5.0, 1.0, None); // north = -1
    let kept = place(&mut w, proto, 50.0, 50.0, Some(false)); // states its clip
    // a world soup: one triangle centred inside (east 2, north 2), one outside (east 20), rock on the outside one
    let soup = w.spawn();
    w.insert(
        soup,
        Collision {
            positions: vec![
                [1.0, 0.0, -1.0],
                [3.0, 0.0, -1.0],
                [2.0, 0.0, -4.0],
                [19.0, 0.0, -1.0],
                [21.0, 0.0, -1.0],
                [20.0, 0.0, -4.0],
            ],
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            surfaces: vec![0, 0],
        },
    );
    w.insert(soup, SkateRock(vec![false, true]));
    // a soup wholly outside, placed there by its transform
    let far = w.spawn();
    w.insert(
        far,
        Collision {
            positions: vec![[1.0, 0.0, -1.0], [3.0, 0.0, -1.0], [2.0, 0.0, -4.0]],
            triangles: vec![[0, 1, 2]],
            surfaces: vec![0],
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [100.0, 0.0, 0.0];
    w.insert(far, t);
    let mut log = PassLog::default();
    ClipReskateRegion::new([0.0, 0.0, 10.0, 10.0])
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    assert_eq!(
        w.resource::<SkateScene>().unwrap().region,
        Some([0.0, 0.0, 10.0, 10.0])
    );
    let clip = |e| w.get::<SkatePlacement>(e).unwrap().clip;
    assert_eq!(
        [clip(inside), clip(east), clip(south), clip(kept)],
        [Some(false), Some(true), Some(true), Some(false)]
    );
    assert_eq!(log.counts["placements outside the region, clipped"], 2);
    let c = w.get::<Collision>(soup).unwrap();
    assert_eq!(c.triangles, vec![[0, 1, 2]]);
    assert_eq!(w.get::<SkateRock>(soup).unwrap().0, vec![false]);
    assert!(!w.has::<Collision>(far));
    assert_eq!(log.counts["world collision triangles outside the region, cut"], 2);
}

/// The region is the program's: all four bounds, finite, not empty.
#[test]
fn the_region_is_required_and_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    assert!(clip_reskate_region(&o(&[("x0", "0"), ("y0", "0"), ("x1", "1"), ("y1", "1")])).is_ok());
    for bad in [
        o(&[("x0", "0"), ("y0", "0"), ("x1", "1")]),
        o(&[("x0", "0"), ("y0", "0"), ("x1", "0"), ("y1", "1")]),
        o(&[("x0", "nan"), ("y0", "0"), ("x1", "1"), ("y1", "1")]),
        o(&[("x0", "0"), ("y0", "0"), ("x1", "1"), ("y1", "1"), ("z", "1")]),
    ] {
        assert!(clip_reskate_region(&bad).is_err(), "{bad:?}");
    }
}
