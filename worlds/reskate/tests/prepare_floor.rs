//! `prepare-reskate-floor`: the floor measured or pinned, geometry below the cutoff left out and every coordinate
//! lifted, worked by hand on a synthetic world.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Collision, Entity, Instance, Mesh, Primitive, Semantic, Stream, Topology, Transform, World,
};
use primase_reskate::dialect::{
    FileRef, MarkerKind, MinHeight, Other, SkateFloorPlan, SkateGrindCurve, SkateLight, SkateMarker,
    SkatePartsOff, SkatePrototype, SkateScene, SkateWater,
};
use primase_reskate::prepare::{PrepareReskateFloor, prepare_reskate_floor};

/// A part spanning heights `y0..y1` in its prototype.
fn part(y0: f32, y1: f32) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0.0, y0, 0.0], [1.0, y0, 0.0], [0.0, y1, 1.0]]),
        )]),
        indices: None,
        material: None,
    }
}

fn prototype(w: &mut World, id: &str, parts: Vec<Primitive>) -> Entity {
    let p = w.spawn();
    w.insert(p, Mesh::new(parts));
    w.insert(
        p,
        SkatePrototype {
            id: id.into(),
            glb: Some(FileRef {
                path: format!("prototypes/{id}.glb"),
                sha256: None,
            }),
            key: None,
            foliage: None,
            ground: None,
            collision: None,
            other: Other::new(),
        },
    );
    p
}

fn place(w: &mut World, proto: Entity, y: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [0.0, y, 0.0];
    w.insert(e, t);
    e
}

struct Fixture {
    w: World,
    p1: Entity,
    p2: Entity,
    p3: Entity,
    soup: Entity,
    deep: Entity,
    light: Entity,
    marker: Entity,
    ground_marker: Entity,
}

/// Part lows: p1 -50 (part a) and -350 (part b), p2 -60, p3 -400; a world soup at -60..-59 and one at -500.
fn fixture() -> Fixture {
    let mut w = World::new();
    w.insert_resource(SkateScene {
        name: "floor".into(),
        min_height: Some(MinHeight {
            height: Some(100.0),
            stub_below: None,
            floor: None,
            other: Other::new(),
        }),
        ..Default::default()
    });
    let two = prototype(&mut w, "two", vec![part(0.0, 1.0), part(-300.0, -299.0)]);
    let one = prototype(&mut w, "one", vec![part(0.0, 2.0)]);
    let (p1, p2, p3) = (
        place(&mut w, two, -50.0),
        place(&mut w, one, -60.0),
        place(&mut w, one, -400.0),
    );
    let triangle = |w: &mut World, y: f32| {
        let e = w.spawn();
        w.insert(
            e,
            Collision {
                positions: vec![[0.0, y, 0.0], [1.0, y, 0.0], [0.0, y + 1.0, 1.0]],
                triangles: vec![[0, 1, 2]],
                surfaces: vec![0],
            },
        );
        e
    };
    let (soup, deep) = (triangle(&mut w, -60.0), triangle(&mut w, -500.0));
    let light = w.spawn();
    w.insert(
        light,
        SkateLight {
            spot: false,
            position: [0.0, 0.0, 0.0],
            direction: [0.0, -1.0, 0.0],
            tangent: [0.0; 3],
            color: [1.0; 3],
            energy: 1.0,
            range: 5.0,
            radius: 0.0,
            spot_size: 0.0,
            spot_blend: 0.0,
            time_of_day: 0x7F,
            cast_shadow: false,
            priority: None,
            group: String::new(),
            unknown_flags: 0,
        },
    );
    let marker = |w: &mut World, height: Option<f32>| {
        let e = w.spawn();
        w.insert(
            e,
            SkateMarker {
                kind: MarkerKind::Travel,
                position: Some(([0.0, 0.0], height)),
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
        e
    };
    let (marker, ground_marker) = (marker(&mut w, Some(5.0)), marker(&mut w, None));
    let curve = w.spawn();
    w.insert(
        curve,
        SkateGrindCurve {
            points: vec![[0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            closed: false,
            kind: 0,
            radius: 0.0,
            surface: 0,
            unknown_flags: 0,
        },
    );
    let water = w.spawn();
    w.insert(
        water,
        SkateWater {
            corners: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            height: Some(2.0),
            other: Other::new(),
        },
    );
    Fixture {
        w,
        p1,
        p2,
        p3,
        soup,
        deep,
        light,
        marker,
        ground_marker,
    }
}

/// Pinned at -60 with a 250 m stub: the cutoff is -310, so p1's part b (top -349) and p3 (top -398) go, p3 whole,
/// and the soup at -500; everything else rises 100 - (-60) = 160 m.
#[test]
fn a_pinned_floor_cuts_and_lifts_everything() {
    let Fixture {
        mut w,
        p1,
        p2,
        p3,
        soup,
        deep,
        light,
        marker,
        ground_marker,
    } = fixture();
    let mut log = PassLog::default();
    PrepareReskateFloor::new(100.0, 250.0)
        .unwrap()
        .with_pinned_floor(-60.0)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    let plan = w.resource::<SkateFloorPlan>().unwrap();
    assert_eq!(
        (
            plan.floor,
            plan.lift,
            plan.drop_below,
            plan.placements_removed,
            plan.parts_removed
        ),
        (Some(-60.0), 160.0, Some(-310.0), 1, 2)
    );
    assert_eq!(w.get::<SkatePartsOff>(p1).unwrap().0, vec![false, true]);
    assert!(!w.has::<SkatePartsOff>(p2));
    assert!(!w.has::<Instance>(p3));
    let y = |e| w.get::<Transform>(e).unwrap().cols[3][1];
    assert_eq!((y(p1), y(p2)), (110.0, 100.0));
    assert_eq!(w.get::<Collision>(soup).unwrap().positions[0][1], 100.0);
    assert!(!w.has::<Collision>(deep));
    assert_eq!(w.get::<SkateLight>(light).unwrap().position[1], 160.0);
    assert_eq!(
        w.get::<SkateMarker>(marker).unwrap().position,
        Some(([0.0, 0.0], Some(165.0)))
    );
    assert_eq!(
        w.get::<SkateMarker>(ground_marker).unwrap().position,
        Some(([0.0, 0.0], None))
    );
    let curve = w.iter::<SkateGrindCurve>().next().unwrap().1;
    assert_eq!(curve.points[0][1], 161.0);
    assert_eq!(w.iter::<SkateWater>().next().unwrap().1.height, Some(162.0));
    // the scene's own floor statement is applied, not left for anything to apply again
    assert!(w.resource::<SkateScene>().unwrap().min_height.is_none());
    assert_eq!(
        log.counts["placements with every part below the cutoff, removed"],
        1
    );
    assert_eq!(
        log.counts["world collision triangles below the cutoff, left out"],
        1
    );
    // a second run is refused: the floor is prepared
    let e = PrepareReskateFloor::new(100.0, 250.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("already prepared"), "{e}");
}

/// Measured: the 1st percentile of the parts' lows (here, of four, the lowest: -400), so with a 250 m stub nothing
/// goes and everything rises 500 m; a floor already at the height lifts nothing.
#[test]
fn a_measured_floor_is_the_parts_first_percentile() {
    let Fixture { mut w, p2, .. } = fixture();
    PrepareReskateFloor::new(100.0, 250.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let plan = w.resource::<SkateFloorPlan>().unwrap();
    assert_eq!(
        (plan.floor, plan.lift, plan.placements_removed),
        (Some(-400.0), 500.0, 0)
    );
    assert_eq!(w.get::<Transform>(p2).unwrap().cols[3][1], 440.0);
    let Fixture { mut w, p2, .. } = fixture();
    PrepareReskateFloor::new(-1000.0, 250.0)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    assert_eq!(w.resource::<SkateFloorPlan>().unwrap().lift, 0.0);
    assert_eq!(w.get::<Transform>(p2).unwrap().cols[3][1], -60.0);
}

/// The height and the stub are the program's: required, finite; the options print back as given.
#[test]
fn the_floor_options_are_required_and_checked() {
    let o = |pairs: &[(&str, &str)]| -> Options {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    };
    let pass = prepare_reskate_floor(&o(&[
        ("min_height", "100"),
        ("stub_below", "250"),
        ("floor", "-60"),
    ]))
    .unwrap();
    assert_eq!(
        prepare_reskate_floor(&pass.options()).unwrap().options(),
        pass.options()
    );
    for bad in [
        o(&[("min_height", "100")]),
        o(&[("min_height", "nan"), ("stub_below", "250")]),
        o(&[("min_height", "100"), ("stub_below", "-1")]),
        o(&[("min_height", "100"), ("stub_below", "250"), ("floor", "inf")]),
        o(&[("min_height", "100"), ("stub_below", "250"), ("lift", "1")]),
    ] {
        assert!(prepare_reskate_floor(&bad).is_err(), "{bad:?}");
    }
}
