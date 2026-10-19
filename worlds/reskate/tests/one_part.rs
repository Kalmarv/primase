//! A ReSkate map is one part: the passes that work on the whole map (floor, region, spawn, marker snaps) refuse a
//! part of a world planned in two cells with a halo, and run on the whole world, as a conversion gives them each.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::convert::{
    Part, PartScope, PartSource, PartitionGroup, PartitionPlan, PlannedParts, WholeWorld,
};
use primase_core::pass::{Pass, PassLog};
use primase_core::{
    Collision, Entity, Instance, Mesh, Primitive, Semantic, Stream, Surfaces, Topology, Transform, World,
};
use primase_reskate::prepare::{
    ClipReskateRegion, PlaceReskateSpawn, PrepareReskateFloor, RaiseOptions, SnapReskateMarkers,
};

/// Flat concrete over east 0..512, north 0..256 at height 0; one prototype placed in each 256 m cell (east 10 and
/// east 300, north 10), raised.
fn world() -> (World, [Entity; 2]) {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [0.0, 0.0, 0.0],
            [512.0, 0.0, 0.0],
            [0.0, 0.0, -256.0],
            [512.0, 0.0, -256.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            )]),
            indices: None,
            material: None,
        }]),
    );
    let place = |w: &mut World, east: f32| {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 0.0, -10.0];
        w.insert(e, t);
        e
    };
    let placed = [place(&mut w, 10.0), place(&mut w, 300.0)];
    primase_reskate::prepare::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "parts".into(),
            ..Default::default()
        },
    )
    .unwrap();
    (w, placed)
}

/// The two cells, each with a 100 m halo, the west cell owning everything but the east placement.
fn plan(w: &World, [west, east]: [Entity; 2]) -> PartitionPlan {
    let cell = |key: &str, roots: BTreeSet<Entity>, x0: f64| PartitionGroup {
        key: key.into(),
        roots,
        context: BTreeSet::new(),
        area: Some([x0 - 100.0, -100.0, x0 + 356.0, 356.0]),
    };
    let mut west_roots: BTreeSet<Entity> = w.entities().collect();
    west_roots.remove(&east);
    assert!(west_roots.contains(&west));
    PartitionPlan {
        by: "two cells".into(),
        groups: vec![
            cell("cell 0,0", west_roots, 0.0),
            cell("cell 1,0", BTreeSet::from([east]), 256.0),
        ],
    }
}

fn parts(source: &mut dyn PartSource, w: &World) -> Vec<Part> {
    source
        .plan(w, &|_| Some([[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]))
        .unwrap();
    std::iter::from_fn(|| source.next_part().unwrap()).collect()
}

fn passes() -> Vec<Box<dyn Pass>> {
    vec![
        Box::new(PrepareReskateFloor::new(100.0, 250.0).unwrap()),
        Box::new(ClipReskateRegion::new([0.0, 0.0, 512.0, 256.0]).unwrap()),
        Box::new(PlaceReskateSpawn::centre()),
        Box::new(SnapReskateMarkers::new()),
    ]
}

/// Each pass on a fresh world given `scope` (and the plan, when `with_plan`): its error, if it refused.
fn run_each(scope: &Part, with_plan: bool) -> Vec<(&'static str, Result<(), String>)> {
    passes()
        .into_iter()
        .map(|pass| {
            let (mut w, placed) = world();
            // the spawn the snap takes, as place-reskate-spawn makes it before the part is scoped
            if pass.name() == "snap-reskate-markers" {
                PlaceReskateSpawn::centre()
                    .run(&mut w, &mut PassLog::default())
                    .unwrap();
            }
            if with_plan {
                let p = plan(&w, placed);
                w.insert_resource(p);
            }
            w.insert_resource(PartScope {
                roots: scope.roots.clone(),
                context: scope.context.clone(),
            });
            (pass.name(), pass.run(&mut w, &mut PassLog::default()))
        })
        .collect()
}

#[test]
fn a_part_of_a_map_is_refused_and_the_whole_map_is_taken() {
    // the planned parts: the west cell has the east placement as context (its box reaches the halo); the east
    // cell's halo does not reach the west placement
    let (mut w, placed) = world();
    let p = plan(&w, placed);
    w.insert_resource(p);
    let planned = parts(&mut PlannedParts::default(), &w);
    assert_eq!(planned.len(), 2);
    assert_eq!(planned[0].context, BTreeSet::from([placed[1]]));
    assert!(planned[1].context.is_empty());
    // a part with context, the plan not in its world: refused for its context
    for (name, r) in run_each(&planned[0], false) {
        let e = r.unwrap_err();
        assert!(e.starts_with(name) && e.contains("context"), "{e}");
    }
    // a part without context, in a world planned in two parts: refused for the plan
    for (name, r) in run_each(&planned[1], true) {
        let e = r.unwrap_err();
        assert!(e.starts_with(name) && e.contains("planned in 2 parts"), "{e}");
    }
    // the whole world, one part: each runs
    let (w, _) = world();
    let whole = parts(&mut WholeWorld::default(), &w);
    assert_eq!(whole.len(), 1);
    assert!(whole[0].context.is_empty());
    for (name, r) in run_each(&whole[0], false) {
        assert!(r.is_ok(), "{name}: {r:?}");
    }
}
