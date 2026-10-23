//! Parametric data lowered for outputs that need triangles or polylines, against hand-worked values.

#![allow(clippy::unwrap_used)]

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Collider, Collision, Curve, Diagonal, Heightfield, Shape, Surfaces, World};

fn world() -> World {
    let mut w = World::new();
    let mut s = Surfaces::default();
    s.index("concrete").unwrap();
    s.index("grass").unwrap();
    w.insert_resource(s);
    w
}

/// Runs `text` through the pass manager, so the passes' declarations are checked too.
fn run(w: &mut World, text: &str) -> RunReport {
    let mut reg = PassRegistry::new();
    primase_meshops::register(&mut reg);
    let pipeline = reg.parse(text).unwrap();
    let mut report = RunReport::default();
    PassManager::new(reg).run(w, &pipeline, &mut report).unwrap();
    report
}

#[test]
fn a_box_becomes_its_twelve_triangles_after_the_existing_soup() {
    let mut w = world();
    let e = w.spawn();
    let mut existing = Collision::default();
    existing.extend(
        &[[9.0; 3], [9.0, 9.0, 8.0], [8.0, 9.0, 9.0]],
        &[[0, 1, 2]],
        1,
        |p| p,
    );
    w.insert(e, existing);
    w.insert(
        e,
        Collider {
            shape: Shape::Box {
                center: [0.0, 1.0, 0.0],
                half_extents: [0.5, 1.0, 2.0],
            },
            surface: 0,
        },
    );
    let report = run(&mut w, "tessellate-colliders");
    assert!(!w.has::<Collider>(e));
    let c = w.get::<Collision>(e).unwrap();
    // the existing triangle, then 8 corners and 12 triangles indexed after its 3 positions
    assert_eq!((c.positions.len(), c.triangles.len()), (3 + 8, 1 + 12));
    assert!(c.triangles[1..].iter().flatten().all(|&i| (3..11).contains(&i)));
    assert_eq!(c.surfaces, [vec![1], vec![0; 12]].concat());
    // the corners are the box's: x +-0.5, y 0 or 2, z +-2
    for p in &c.positions[3..] {
        assert!(
            p[0].abs() == 0.5 && (p[1] == 0.0 || p[1] == 2.0) && p[2].abs() == 2.0,
            "{p:?}"
        );
    }
    assert_eq!(report.steps[0].log.counts["boxes"], 1);
}

#[test]
fn a_heightfield_becomes_two_upward_triangles_per_cell_but_its_holes() {
    let mut w = world();
    let e = w.spawn();
    // 3 x 2 samples, 1 m along x and 2 m along z: cells 0 and 1, cell 1 a hole
    w.insert(
        e,
        Heightfield {
            columns: 3,
            rows: 2,
            spacing: [1.0, 2.0],
            heights: vec![0.0, 1.0, 2.0, 0.5, 1.5, 2.5],
            diagonal: Diagonal::Main,
            holes: Some(vec![false, true]),
            surfaces: Some(vec![1, 0]),
        },
    );
    w.insert(
        e,
        Collider {
            shape: Shape::Heightfield,
            surface: 0,
        },
    );
    run(&mut w, "tessellate-colliders");
    let c = w.get::<Collision>(e).unwrap();
    assert_eq!(
        c.positions,
        [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0, 0.5, 2.0],
            [1.0, 1.5, 2.0],
            [2.0, 2.5, 2.0]
        ]
    );
    // cell 0's corners a 0, b 1, d 3, e 4, split a-e
    assert_eq!(c.triangles, [[0, 4, 1], [0, 3, 4]]);
    assert_eq!(c.surfaces, [1, 1]);
    for t in &c.triangles {
        let [a, b, d] = t.map(|i| c.positions[i as usize]);
        let (u, v) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [d[0] - a[0], d[1] - a[1], d[2] - a[2]],
        );
        let up = u[2] * v[0] - u[0] * v[2];
        assert!(up > 0.0, "{t:?} faces down");
    }
    // the heightfield stays: it is also a drawn surface
    assert!(w.has::<Heightfield>(e) && !w.has::<Collider>(e));
}

#[test]
fn a_sphere_is_tessellated_and_counted_as_an_approximation() {
    let mut w = world();
    let e = w.spawn();
    w.insert(
        e,
        Collider {
            shape: Shape::Sphere {
                center: [0.0; 3],
                radius: 1.0,
            },
            surface: 0,
        },
    );
    let report = run(&mut w, "tessellate-colliders");
    let c = w.get::<Collision>(e).unwrap();
    assert!(
        c.positions
            .iter()
            .all(|p| ((p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 1.0).abs() < 1e-6)
    );
    assert_eq!(report.steps[0].log.approximations["spheres tessellated"], 1);
}

/// A quarter of the unit circle in the xz plane as one Bezier segment: handles at kappa = 4/3 (sqrt 2 - 1), whose
/// midpoint lies on the circle at (sqrt 1/2, 0, sqrt 1/2) and whose radius stays within 2.8e-4 of 1.
fn quarter(closed: bool) -> Curve {
    let k = 4.0 / 3.0 * (2f32.sqrt() - 1.0);
    Curve {
        kind: "cable".into(),
        points: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        closed,
        tangents: Some(vec![
            [[1.0, 0.0, -k], [1.0, 0.0, k]],
            [[k, 0.0, 1.0], [-k, 0.0, 1.0]],
        ]),
        up: Some(vec![[0.0, 1.0, 0.0], [0.0, 1.0, 0.0]]),
    }
}

#[test]
fn a_bezier_curve_becomes_a_polyline_within_the_tolerance() {
    let mut w = world();
    let e = w.spawn();
    w.insert(e, quarter(false));
    let flat_line = w.spawn();
    let line = Curve {
        tangents: None,
        up: None,
        ..quarter(false)
    };
    w.insert(flat_line, line.clone());
    run(&mut w, "flatten-curves{tolerance=0.005}");
    let c = w.get::<Curve>(e).unwrap();
    assert!(c.tangents.is_none());
    assert_eq!(c.points.first(), Some(&[1.0, 0.0, 0.0]));
    assert_eq!(c.points.last(), Some(&[0.0, 0.0, 1.0]));
    // an even split puts the curve's midpoint on a vertex
    let mid = c.points[(c.points.len() - 1) / 2];
    let h = 0.5f32.sqrt();
    assert!(
        (mid[0] - h).abs() < 1e-6 && mid[1] == 0.0 && (mid[2] - h).abs() < 1e-6,
        "{mid:?}"
    );
    // every vertex on the circle (within the Bezier's own 2.8e-4), every edge's midpoint within the tolerance of it
    let r = |p: [f32; 3]| (p[0] * p[0] + p[2] * p[2]).sqrt();
    assert!(c.points.iter().all(|&p| (r(p) - 1.0).abs() < 3e-4));
    for e in c.points.windows(2) {
        let m = [(e[0][0] + e[1][0]) / 2.0, 0.0, (e[0][2] + e[1][2]) / 2.0];
        assert!(1.0 - r(m) < 0.005 + 3e-4, "{e:?}");
    }
    assert_eq!(c.up.as_ref().unwrap().len(), c.points.len());
    // a polyline is left as it is
    assert_eq!(w.get::<Curve>(flat_line), Some(&line));
}

#[test]
fn a_closed_bezier_curve_does_not_repeat_its_first_point() {
    let mut w = world();
    let e = w.spawn();
    w.insert(e, quarter(true));
    run(&mut w, "flatten-curves{tolerance=0.005}");
    let c = w.get::<Curve>(e).unwrap();
    assert!(c.closed);
    assert_ne!(c.points.last(), c.points.first());
    assert_eq!(c.up.as_ref().unwrap().len(), c.points.len());
}

/// How closely a polyline follows its curve is the program's: a pipeline without the tolerance does not parse.
#[test]
fn flatten_curves_needs_its_tolerance() {
    let mut reg = PassRegistry::new();
    primase_meshops::register(&mut reg);
    let error = reg.parse("flatten-curves").expect_err("no default").to_string();
    assert!(error.contains("tolerance is required"), "{error}");
    assert!(reg.parse("flatten-curves{tolerance=0.01}").is_ok());
}
