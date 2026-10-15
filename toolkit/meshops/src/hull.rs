//! Convex hulls of point sets.
use crate::{Error, Mesh, Result, finite};
use std::collections::{HashMap, HashSet};

/// A point on the integer grid.
type G = [i64; 3];

/// The sign of the volume of (a, b, c, d): positive when d is on the side of the plane (a, b, c) that the triangle's
/// counter-clockwise normal points to. Exact: grid coordinates within 2^24 make every product fit an i128.
fn orient(a: G, b: G, c: G, d: G) -> i128 {
    let u = [0, 1, 2].map(|k| i128::from(b[k] - a[k]));
    let v = [0, 1, 2].map(|k| i128::from(c[k] - a[k]));
    let w = [0, 1, 2].map(|k| i128::from(d[k] - a[k]));
    u[0] * (v[1] * w[2] - v[2] * w[1]) - u[1] * (v[0] * w[2] - v[2] * w[0])
        + u[2] * (v[0] * w[1] - v[1] * w[0])
}

/// A hull face: its corners (counter-clockwise from outside) and the points outside it that it holds.
struct Face {
    v: [usize; 3],
    outside: Vec<usize>,
}

impl Face {
    fn edges(&self) -> [(usize, usize); 3] {
        let v = self.v;
        [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])]
    }
}

/// The convex hull of `points`: its vertices and outward, counter-clockwise triangles over them.
///
/// The points are snapped to the f32 grid at the set's largest coordinate magnitude (a power-of-two step, so every
/// grid point is an f32 exactly, each point moved by at most half a step) and every orientation is decided exactly on
/// it: the hull is exactly convex in the coordinates it returns, which are the snapped points. Quickhull: an initial tetrahedron from extreme points; each face
/// holds the points outside it; the farthest point outside a face is added next, its visible region grown from that
/// face across neighbours, replaced by a fan from the region's horizon, and the region's points handed to the new faces.
/// Points on a face's plane are not outside it, so coplanar points do not split faces. Refuses fewer than four points,
/// non-finite ones, and sets with no volume on the grid. Normals and texture coordinates are left empty.
pub fn convex_hull(points: &[[f32; 3]]) -> Result<Mesh> {
    for q in points {
        finite(q)?;
    }
    if points.len() < 4 {
        return Err(Error("a hull needs four points".into()));
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for q in points {
        for k in 0..3 {
            lo[k] = lo[k].min(f64::from(q[k]));
            hi[k] = hi[k].max(f64::from(q[k]));
        }
    }
    let flat = || Error("points with no volume have no hull".into());
    let largest = points
        .iter()
        .flatten()
        .fold(0f64, |m, &x| m.max(f64::from(x).abs()));
    if largest == 0.0 {
        return Err(flat());
    }
    // f32's step at the largest magnitude: grid coordinates within 2^24, every grid point an f32
    let step = 2f64.powi(largest.log2().floor() as i32 - 23);
    let g: Vec<G> = points
        .iter()
        .map(|q| q.map(|x| (f64::from(x) / step).round() as i64))
        .collect();
    let f = |i: usize| g[i].map(|x| x as f64);
    let by = |key: &dyn Fn(usize) -> f64| {
        (0..g.len())
            .max_by(|&a, &b| key(a).total_cmp(&key(b)))
            .unwrap_or(0)
    };
    // the initial tetrahedron: extreme along the widest axis, farthest from their line, farthest from their plane
    let axis = (0..3)
        .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
        .unwrap_or(0);
    let i0 = by(&|i| -f(i)[axis]);
    let i1 = by(&|i| f(i)[axis]);
    let line = [0, 1, 2].map(|k| f(i1)[k] - f(i0)[k]);
    let off_line = |i: usize| {
        let d = [0, 1, 2].map(|k| f(i)[k] - f(i0)[k]);
        let c = [
            line[1] * d[2] - line[2] * d[1],
            line[2] * d[0] - line[0] * d[2],
            line[0] * d[1] - line[1] * d[0],
        ];
        c[0] * c[0] + c[1] * c[1] + c[2] * c[2]
    };
    let i2 = by(&off_line);
    let i3 = by(&|i| orient(g[i0], g[i1], g[i2], g[i]).unsigned_abs() as f64);
    if orient(g[i0], g[i1], g[i2], g[i3]) == 0 {
        return Err(flat());
    }
    let mut faces: Vec<Option<Face>> = Vec::new();
    let mut edge_face: HashMap<(usize, usize), usize> = HashMap::new();
    let add =
        |faces: &mut Vec<Option<Face>>, edge_face: &mut HashMap<(usize, usize), usize>, v: [usize; 3]| {
            let id = faces.len();
            let face = Face {
                v,
                outside: Vec::new(),
            };
            for e in face.edges() {
                edge_face.insert(e, id);
            }
            faces.push(Some(face));
            id
        };
    // faces of the tetrahedron, each oriented away from the fourth corner
    for (v, other) in [
        ([i0, i1, i2], i3),
        ([i0, i1, i3], i2),
        ([i0, i2, i3], i1),
        ([i1, i2, i3], i0),
    ] {
        let v = if orient(g[v[0]], g[v[1]], g[v[2]], g[other]) > 0 {
            [v[0], v[2], v[1]]
        } else {
            v
        };
        add(&mut faces, &mut edge_face, v);
    }
    let above = |faces: &[Option<Face>], f: usize, i: usize| {
        faces[f]
            .as_ref()
            .map_or(0, |f| orient(g[f.v[0]], g[f.v[1]], g[f.v[2]], g[i]))
    };
    let assign = |faces: &mut Vec<Option<Face>>, ids: &[usize], points: &mut dyn Iterator<Item = usize>| {
        for i in points {
            if let Some(&f) = ids.iter().find(|&&f| above(faces, f, i) > 0) {
                faces[f].as_mut().expect("live").outside.push(i);
            }
        }
    };
    let seed = [i0, i1, i2, i3];
    assign(
        &mut faces,
        &[0, 1, 2, 3],
        &mut (0..g.len()).filter(|i| !seed.contains(i)),
    );
    let mut work: Vec<usize> = (0..faces.len()).collect();
    while let Some(start) = work.pop() {
        let Some(face) = faces[start].as_ref() else {
            continue;
        };
        // the farthest point outside the face: the largest volume over it (its area is common to all)
        let Some(&apex) = face.outside.iter().max_by_key(|&&i| above(&faces, start, i)) else {
            continue;
        };
        let mut visible = vec![start];
        let mut seen = HashSet::from([start]);
        let mut k = 0;
        while k < visible.len() {
            for (a, b) in faces[visible[k]].as_ref().expect("live").edges() {
                if let Some(&n) = edge_face.get(&(b, a))
                    && !seen.contains(&n)
                    && above(&faces, n, apex) > 0
                {
                    seen.insert(n);
                    visible.push(n);
                }
            }
            k += 1;
        }
        let mut horizon = Vec::new();
        let mut orphans = Vec::new();
        for &v in &visible {
            let face = faces[v].take().expect("live");
            for (a, b) in face.edges() {
                if edge_face.get(&(a, b)) == Some(&v) {
                    edge_face.remove(&(a, b));
                }
                if edge_face.get(&(b, a)).is_some_and(|n| !seen.contains(n)) {
                    horizon.push((a, b));
                }
            }
            orphans.extend(face.outside.into_iter().filter(|&i| i != apex));
        }
        let new: Vec<usize> = horizon
            .into_iter()
            .map(|(a, b)| add(&mut faces, &mut edge_face, [a, b, apex]))
            .collect();
        assign(&mut faces, &new, &mut orphans.into_iter());
        work.extend(new);
    }
    let mut index = vec![u32::MAX; points.len()];
    let mut m = Mesh::default();
    for face in faces.iter().flatten() {
        for &v in &face.v {
            if index[v] == u32::MAX {
                index[v] = m.positions.len() as u32;
                m.positions.push(g[v].map(|x| (x as f64 * step) as f32));
            }
            m.indices.push(index[v]);
        }
    }
    m.validate()?;
    Ok(m)
}
