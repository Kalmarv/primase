// Test code: helpers outside #[test] functions unwrap too.
#![allow(clippy::unwrap_used)]

use std::collections::HashSet;

use primase_meshops::{Mesh, convex_hull};

fn noise(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 11) as f64 / (1u64 << 53) as f64
        })
        .collect()
}

/// A closed, oriented triangle surface: every directed edge's reverse is used once, V - E + F = 2.
fn closed(m: &Mesh) -> bool {
    let tris: Vec<[u32; 3]> = m.indices.as_chunks::<3>().0.to_vec();
    let edges: Vec<(u32, u32)> = tris
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .collect();
    let set: HashSet<(u32, u32)> = edges.iter().copied().collect();
    set.len() == edges.len()
        && edges.iter().all(|&(a, b)| set.contains(&(b, a)))
        && m.positions.len() as i64 - (edges.len() / 2) as i64 + tris.len() as i64 == 2
}

/// Every point on or inside every face plane (within `eps`), faces outward from the hull's centroid.
fn contains(m: &Mesh, points: &[[f32; 3]], eps: f64) -> bool {
    let p = |i: u32| m.positions[i as usize].map(f64::from);
    let n = m.positions.len() as f64;
    let c = [0, 1, 2].map(|k| m.positions.iter().map(|q| f64::from(q[k])).sum::<f64>() / n);
    m.indices.as_chunks::<3>().0.iter().all(|t| {
        let (a, b, cc) = (p(t[0]), p(t[1]), p(t[2]));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [cc[0] - a[0], cc[1] - a[1], cc[2] - a[2]];
        let nn = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let l = (nn[0] * nn[0] + nn[1] * nn[1] + nn[2] * nn[2]).sqrt();
        let side = |q: [f64; 3]| ((q[0] - a[0]) * nn[0] + (q[1] - a[1]) * nn[1] + (q[2] - a[2]) * nn[2]) / l;
        side(c) < 0.0 && points.iter().all(|q| side(q.map(f64::from)) <= eps)
    })
}

#[test]
fn a_cube_and_its_inside() {
    let mut pts: Vec<[f32; 3]> = (0..8)
        .map(|i| [0, 1, 2].map(|k| if i & (1 << k) != 0 { 2.0 } else { -1.0 }))
        .collect();
    let r = noise(300, 3);
    pts.extend(
        r.as_chunks::<3>()
            .0
            .iter()
            .map(|q| q.map(|x| (x * 2.9 - 0.95) as f32)),
    );
    // and points a hair outside a face's plane, within the tolerance
    pts.push([2.0 + 1e-7, 0.5, 0.5]);
    let h = convex_hull(&pts).unwrap();
    assert_eq!(h.positions.len(), 8);
    assert_eq!(h.indices.len(), 36);
    assert!(closed(&h));
    assert!(contains(&h, &pts, 1e-5));
}

#[test]
fn points_on_a_sphere() {
    let r = noise(1500, 11);
    let pts: Vec<[f32; 3]> = r
        .as_chunks::<3>()
        .0
        .iter()
        .map(|q| {
            let v = q.map(|x| x * 2.0 - 1.0);
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            v.map(|x| (x / l * 5.0 + 10.0) as f32)
        })
        .collect();
    let h = convex_hull(&pts).unwrap();
    assert!(
        closed(&h),
        "{} vertices, {} triangles",
        h.positions.len(),
        h.indices.len() / 3
    );
    assert!(contains(&h, &pts, 1e-5 * 10.0));
    assert!(h.positions.len() > 400, "most points of a sphere are on its hull");
}

#[test]
fn sets_without_volume_are_refused() {
    let flat: Vec<[f32; 3]> = (0..10).map(|i| [i as f32, (i * i) as f32, 0.0]).collect();
    assert!(convex_hull(&flat).is_err(), "coplanar");
    let line: Vec<[f32; 3]> = (0..10).map(|i| [i as f32; 3]).collect();
    assert!(convex_hull(&line).is_err(), "collinear");
    assert!(
        convex_hull(&[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]).is_err(),
        "three points"
    );
    let mut bad = flat.clone();
    bad[3][2] = f32::NAN;
    assert!(convex_hull(&bad).is_err(), "non-finite");
}

#[test]
fn many_points_along_its_edges() {
    // a slanted slab: two parallel faces whose long edges carry many collinear points (in f32), which a
    // tolerance-based hull turns into slivers with flipped orientation
    let (a, b) = ([-14.34f64, -10.6], [0.41f64, -0.6]);
    let mut pts = Vec::new();
    for y in [1.2499f64, -1.2499] {
        for side in [0.0, 0.8] {
            for i in 0..=40 {
                let t = f64::from(i) / 40.0;
                pts.push([
                    (a[0] + (b[0] - a[0]) * t + side * 0.6) as f32,
                    y as f32,
                    (a[1] + (b[1] - a[1]) * t - side * 0.8) as f32,
                ]);
            }
        }
    }
    let r = noise(600, 5);
    pts.extend(r.as_chunks::<3>().0.iter().map(|q| {
        let t = q[0];
        [
            (a[0] + (b[0] - a[0]) * t + q[1] * 0.48) as f32,
            (q[2] * 2.4 - 1.2) as f32,
            (a[1] + (b[1] - a[1]) * t - q[1] * 0.64) as f32,
        ]
    }));
    let h = convex_hull(&pts).unwrap();
    assert!(closed(&h));
    assert!(contains(&h, &pts, 1e-5));
}
