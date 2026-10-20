//! `fill-ambient-lights` on small synthetic scenes: where the lattice puts its lights, the clearance search keeping
//! them off surfaces (a floor; a closed room whose lights all stand inside it), the light each gets, and its refusals.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::convert::PartScope;
use primase_core::pass::{Options, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{
    Ambient, AmbientLight, Entity, Environment, Instance, Light, LightKind, Mesh, Name, Primitive, Semantic,
    Stream, Topology, Transform, World,
};
use primase_lighting::{FillAmbientLights, FillCells, FillGrid, SurfaceGrid, placed_triangles};

type Tri = [[f32; 3]; 3];

/// A placement of a mesh of `triangles`, translated by `at`.
fn place(w: &mut World, triangles: &[Tri], at: [f32; 3]) -> Entity {
    let prototype = w.spawn();
    w.insert(
        prototype,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(triangles.iter().flatten().copied().collect()),
            )]),
            indices: None,
            material: None,
        }]),
    );
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    w.insert(
        e,
        Transform {
            cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], at],
        },
    );
    e
}

/// Two triangles of the quad a, b, c, d.
fn quad(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> [Tri; 2] {
    [[a, b, c], [a, c, d]]
}

fn ambient(w: &mut World, light: AmbientLight, intensity: f32) {
    w.insert_resource(Environment {
        sun: None,
        ambient: Some(Ambient { intensity, light }),
    });
}

/// Runs the pass through the pass manager (its declarations checked, the world verified) and returns its log.
fn run(w: &mut World, grid: FillGrid) -> Result<PassLog, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(FillAmbientLights::new(grid))],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report.steps.remove(0).log)
}

/// The fill lights made, by name: position and light.
fn fills(w: &World) -> BTreeMap<String, ([f32; 3], Light)> {
    w.iter::<Light>()
        .map(|(e, l)| {
            (
                w.get::<Name>(e).unwrap().0.clone(),
                (w.get::<Transform>(e).unwrap().cols[3], l.clone()),
            )
        })
        .collect()
}

#[test]
fn the_lattice_is_anchored_and_the_cells_are_the_callers_choice() {
    let grid = |bounds, cells| FillGrid::new([1.0, 2.0, 3.0], 10.0, bounds, cells, 0.0, 15.0).unwrap();
    let positions = |g: &FillGrid| g.points().into_iter().map(|(_, p)| p).collect::<Vec<_>>();
    // x: cells -1, 0 and 1 meet [-4, 17]; y: a flat extent meets one cell; z: cells 0 and 1 meet [3, 15]
    let bounds = [[-4.0, 2.0, 3.0], [17.0, 2.0, 15.0]];
    let meeting = grid(bounds, FillCells::Meeting);
    let points = meeting.points();
    assert_eq!(points.len(), 3 * 2);
    assert_eq!(points[0], ([-1, 0, 0], [-4.0, 7.0, 8.0]));
    assert_eq!(points[5], ([1, 0, 1], [16.0, 7.0, 18.0]));
    // inside: a centre past the bounds is not taken (y's 7 is past a flat extent at 2; z's 18 past 15)
    assert!(grid(bounds, FillCells::Inside).points().is_empty());
    let inside = grid([[-4.0, 2.0, 3.0], [17.0, 7.0, 15.0]], FillCells::Inside);
    assert_eq!(
        positions(&inside),
        [[-4.0, 7.0, 8.0], [6.0, 7.0, 8.0], [16.0, 7.0, 8.0]]
    );
    // one anchor, any bounds: the lights of a smaller box are among the larger box's, at the same places
    let small = grid([[5.0, 2.0, 4.0], [9.0, 3.0, 9.0]], FillCells::Meeting);
    assert_eq!(small.points(), [([0, 0, 0], [6.0, 7.0, 8.0])]);
    assert!(points.contains(&small.points()[0]));
}

#[test]
fn lights_near_a_surface_are_left_out_and_the_rest_carry_the_ambient() {
    let mut w = World::new();
    // a 30 m floor at y = 1 (placed by its transform)
    let floor = quad(
        [-11.0, 0.0, -11.0],
        [19.0, 0.0, -11.0],
        [19.0, 0.0, 19.0],
        [-11.0, 0.0, 19.0],
    );
    place(&mut w, &floor, [0.0, 1.0, 0.0]);
    ambient(&mut w, AmbientLight::Flat([0.5, 0.25, 0.0]), 2.0);
    // centres 2 and 6 on each axis; those at y = 2 are 1 m from the floor
    let grid = FillGrid::new([0.0; 3], 4.0, [[0.0; 3], [8.0; 3]], FillCells::Meeting, 1.5, 6.0).unwrap();
    let log = run(&mut w, grid).unwrap();
    assert_eq!(log.counts["ambient fill lights"], 4);
    assert_eq!(
        log.counts["ambient fill lights left out (within the clearance of a surface)"],
        4
    );
    assert_eq!(log.placed.len(), 4);
    assert_eq!(
        log.approximations.keys().collect::<Vec<_>>(),
        ["ambient light as shadowless point lights on a grid"],
        "a flat ambient is one colour: no reduction"
    );
    let lights = fills(&w);
    assert_eq!(
        lights.keys().collect::<Vec<_>>(),
        [
            "ambient fill 0.1.0",
            "ambient fill 0.1.1",
            "ambient fill 1.1.0",
            "ambient fill 1.1.1"
        ]
    );
    assert_eq!(lights["ambient fill 1.1.0"].0, [6.0, 6.0, 2.0]);
    // the mean colour times the intensity is [1, 0.5, 0]: level 1, so that colour; intensity 1 x (4 / 2)^2
    assert_eq!(
        lights["ambient fill 0.1.0"].1,
        Light {
            kind: LightKind::Point,
            color: [1.0, 0.5, 0.0],
            intensity: 4.0,
            range: Some(6.0),
            cast_shadows: Some(false),
        }
    );
    // a gradient: its mean, reported
    let mut w = World::new();
    ambient(
        &mut w,
        AmbientLight::Gradient {
            sky: [0.6, 0.3, 0.3],
            equator: [0.3; 3],
            ground: [0.0; 3],
        },
        1.0,
    );
    let grid = FillGrid::new([0.0; 3], 2.0, [[0.0; 3], [1.0; 3]], FillCells::Meeting, 0.5, 3.0).unwrap();
    let log = run(&mut w, grid).unwrap();
    assert_eq!(
        log.approximations["the ambient's gradient reduced to its mean colour for the fill lights"],
        1
    );
    let (_, light) = fills(&w).into_values().next().unwrap();
    assert!((light.color[0] - 1.0).abs() < 1e-6 && (light.color[1] - 2.0 / 3.0).abs() < 1e-6);
    assert!((light.intensity - 0.3).abs() < 1e-6, "{}", light.intensity);
}

#[test]
fn a_closed_room_gets_lights_only_inside_none_within_the_clearance_of_a_face() {
    let (x, y, z) = (12.0f32, 7.0f32, 20.0f32);
    let p = |a: f32, b: f32, c: f32| [a, b, c];
    let faces: Vec<Tri> = [
        quad(p(0., 0., 0.), p(x, 0., 0.), p(x, 0., z), p(0., 0., z)), // floor
        quad(p(0., y, 0.), p(0., y, z), p(x, y, z), p(x, y, 0.)),     // ceiling
        quad(p(0., 0., 0.), p(0., y, 0.), p(x, y, 0.), p(x, 0., 0.)), // walls
        quad(p(0., 0., z), p(x, 0., z), p(x, y, z), p(0., y, z)),
        quad(p(0., 0., 0.), p(0., 0., z), p(0., y, z), p(0., y, 0.)),
        quad(p(x, 0., 0.), p(x, y, 0.), p(x, y, z), p(x, 0., z)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut w = World::new();
    // the room stands at (100, 50, -40): an interior's own bounds and anchor, not a map's
    let at = [100.0f32, 50.0, -40.0];
    place(&mut w, &faces, at);
    // and a floor outside it, past its walls
    let outside = quad(
        [-50.0, 0.0, -50.0],
        [50.0, 0.0, -50.0],
        [50.0, 0.0, 50.0],
        [-50.0, 0.0, 50.0],
    );
    place(&mut w, &outside, [0.0; 3]);
    ambient(&mut w, AmbientLight::Flat([0.2; 3]), 1.0);
    let lo = at.map(f64::from);
    let hi = [lo[0] + 12.0, lo[1] + 7.0, lo[2] + 20.0];
    let clearance = 1.2;
    // inside, the centres are 1, 3, 5, 7 (on the ceiling: inside, closed) across y; 1 .. 11 across x; 1 .. 19
    // across z. Left out within 1.2 of a face: x 1, 11; y 1, 7; z 1, 19
    let grid = FillGrid::new(lo, 2.0, [lo, hi], FillCells::Inside, clearance, 3.0).unwrap();
    assert_eq!(grid.points().len(), 6 * 4 * 10);
    let log = run(&mut w, grid).unwrap();
    assert_eq!(log.counts["ambient fill lights"], 4 * 2 * 8);
    assert_eq!(
        log.counts["ambient fill lights left out (within the clearance of a surface)"],
        6 * 4 * 10 - 4 * 2 * 8
    );
    let triangles = placed_triangles(&w).unwrap();
    assert_eq!(triangles.len(), 12 + 2);
    for (name, (q, _)) in fills(&w) {
        let q = q.map(f64::from);
        assert!(
            (0..3).all(|a| lo[a] < q[a] && q[a] < hi[a]),
            "{name} at {q:?} is not inside the room"
        );
        for t in &triangles {
            let d = primase_meshops::point_triangle_distance_squared(q, *t).sqrt();
            assert!(d >= clearance, "{name} at {q:?} is {d} m from a face");
        }
    }
    // the same cells by "meeting" add none here (the room is whole cells high), but a 4.5 m room would take a centre
    // at 5, half a metre above its ceiling: "inside" is the interior's choice
    let low = [lo, [hi[0], lo[1] + 4.5, hi[2]]];
    let meeting = FillGrid::new(lo, 2.0, low, FillCells::Meeting, 0.0, 3.0).unwrap();
    assert!(meeting.points().iter().any(|(_, q)| q[1] > lo[1] + 4.5));
    let inside = FillGrid::new(lo, 2.0, low, FillCells::Inside, 0.0, 3.0).unwrap();
    assert!(inside.points().iter().all(|(_, q)| q[1] <= lo[1] + 4.5));
}

#[test]
fn no_ambient_or_a_black_one_makes_no_lights_and_a_part_is_refused() {
    let grid = || FillGrid::new([0.0; 3], 4.0, [[0.0; 3], [8.0; 3]], FillCells::Meeting, 1.0, 6.0).unwrap();
    let mut w = World::new();
    let log = run(&mut w, grid()).unwrap();
    assert_eq!(log.counts["scenes without an ambient: no fill lights"], 1);
    w.insert_resource(Environment::default());
    let log = run(&mut w, grid()).unwrap();
    assert_eq!(log.counts["scenes without an ambient: no fill lights"], 1);
    ambient(&mut w, AmbientLight::Flat([0.0; 3]), 1.0);
    let log = run(&mut w, grid()).unwrap();
    assert_eq!(log.counts["scenes with a black ambient: no fill lights"], 1);
    assert!(fills(&w).is_empty());
    ambient(&mut w, AmbientLight::Flat([0.5; 3]), 1.0);
    let mut part = w.clone();
    part.insert_resource(PartScope {
        roots: Default::default(),
        shared: Default::default(),
        context: Default::default(),
        whole: false,
        area: None,
        first_made: None,
        placed: Default::default(),
    });
    let refused = run(&mut part, grid()).unwrap_err();
    assert!(refused.contains("owned region"), "{refused}");
    // the whole map as one part is fine
    let mut whole = w.clone();
    whole.insert_resource(PartScope {
        roots: Default::default(),
        shared: Default::default(),
        context: Default::default(),
        whole: true,
        area: None,
        first_made: None,
        placed: Default::default(),
    });
    assert_eq!(run(&mut whole, grid()).unwrap().counts["ambient fill lights"], 8);
}

#[test]
fn the_grid_refuses_values_that_place_no_sensible_grid() {
    let ok = [[0.0; 3], [10.0; 3]];
    assert!(FillGrid::new([0.0; 3], 0.0, ok, FillCells::Meeting, 1.0, 1.0).is_err());
    assert!(FillGrid::new([0.0; 3], 1.0, [[1.0; 3], [0.0; 3]], FillCells::Meeting, 1.0, 1.0).is_err());
    assert!(FillGrid::new([0.0; 3], 1.0, ok, FillCells::Meeting, -1.0, 1.0).is_err());
    assert!(FillGrid::new([0.0; 3], 1.0, ok, FillCells::Meeting, 1.0, 0.0).is_err());
    assert!(FillGrid::new([f64::NAN, 0.0, 0.0], 1.0, ok, FillCells::Meeting, 1.0, 1.0).is_err());
    assert!(
        FillGrid::new(
            [0.0; 3],
            0.001,
            [[0.0; 3], [1e4; 3]],
            FillCells::Meeting,
            1.0,
            1.0
        )
        .is_err()
    );
}

#[test]
fn the_registered_pass_takes_every_choice_from_its_options() {
    let mut reg = PassRegistry::new();
    primase_lighting::register(&mut reg);
    let options: Options = [
        ("anchor", "1;2;3"),
        ("spacing", "15"),
        ("bounds", "-10;0;-20;40;12;30"),
        ("cells", "inside"),
        ("clearance", "3.75"),
        ("reach", "22.5"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let pass = reg.make("fill-ambient-lights", &options).unwrap();
    assert_eq!(pass.options(), options);
    for leave_out in ["anchor", "cells", "clearance"] {
        let mut o = options.clone();
        o.remove(leave_out);
        assert!(reg.make("fill-ambient-lights", &o).is_err(), "{leave_out}");
    }
    let mut o = options.clone();
    o.insert("cells".into(), "all".into());
    assert!(reg.make("fill-ambient-lights", &o).is_err());
}

#[test]
fn the_surface_grid_reads_the_worlds_placements() {
    let mut w = World::new();
    let floor = quad([0.0; 3], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]);
    place(&mut w, &floor, [10.0, 0.0, 0.0]);
    let grid = SurfaceGrid::of_world(&w, 1.0, [[0.0; 3], [20.0; 3]]).unwrap();
    assert_eq!(grid.len(), 2);
    assert!(grid.within([12.0, 0.5, 2.0], 1.0));
    assert!(
        !grid.within([2.0, 0.5, 2.0], 1.0),
        "the mesh is placed at x 10, not at its own origin"
    );
}
