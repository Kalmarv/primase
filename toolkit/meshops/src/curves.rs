//! Polylines from loose segments, and the collision surface nearest a point: mechanisms a program uses to prepare
//! a source's curves (ledge edges stored as separate segments) for a destination that wants joined curves with a
//! surface.
use crate::{Error, Result};
use primase_core::Collision;
use std::collections::{BTreeMap, VecDeque};

/// One polyline [`chain`] joined: its points in order, whether it closes on itself (the repeated end point is then
/// left out), and its length in metres.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
    pub length: f64,
}

/// Join segments whose end points are within `tolerance` metres into polylines. Stable first-use order: each
/// unused segment in order seeds a chain, which grows at its back, then at its front, by the unused segment of
/// lowest index touching the end; branches are resolved by that order and every segment is used once. A chain of
/// more than three points whose ends meet is closed.
pub fn chain(segments: &[[[f32; 3]; 2]], tolerance: f64) -> Result<Vec<Chain>> {
    if !tolerance.is_finite()
        || tolerance <= 0.
        || segments
            .iter()
            .flatten()
            .flatten()
            .any(|v| !v.is_finite() || f64::from(v.abs()) > 1e16)
    {
        return Err(Error("nonfinite segment or invalid tolerance".into()));
    }
    let scale = 1. / tolerance;
    let key = |p: [f32; 3]| p.map(|x| (f64::from(x) * scale).floor() as i64);
    let nearby = |a: [f32; 3], b: [f32; 3]| {
        a.into_iter()
            .zip(b)
            .map(|(a, b)| (f64::from(a) - f64::from(b)).powi(2))
            .sum::<f64>()
            <= tolerance.powi(2)
    };
    let mut ends = BTreeMap::<[i64; 3], Vec<usize>>::new();
    for (i, s) in segments.iter().enumerate() {
        for &p in s {
            ends.entry(key(p)).or_default().push(i);
        }
    }
    let mut used = vec![false; segments.len()];
    let mut chains = vec![];
    for i in 0..segments.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut line = VecDeque::from(segments[i]);
        for front in [false, true] {
            loop {
                let end = if front {
                    *line.front().expect("seed segment has two endpoints")
                } else {
                    *line.back().expect("seed segment has two endpoints")
                };
                let cell = key(end);
                let mut next = None;
                for x in -1..=1 {
                    for y in -1..=1 {
                        for z in -1..=1 {
                            if let Some(candidates) = ends.get(&[cell[0] + x, cell[1] + y, cell[2] + z]) {
                                for &j in candidates {
                                    if !used[j]
                                        && segments[j].iter().any(|&p| nearby(p, end))
                                        && next.is_none_or(|old| j < old)
                                    {
                                        next = Some(j);
                                    }
                                }
                            }
                        }
                    }
                }
                let Some(next) = next else {
                    break;
                };
                used[next] = true;
                let s = segments[next];
                let other = if nearby(s[0], end) { s[1] } else { s[0] };
                if front {
                    line.push_front(other);
                } else {
                    line.push_back(other);
                }
            }
        }
        let mut points = line.into_iter().collect::<Vec<_>>();
        let length = points
            .windows(2)
            .map(|p| {
                p[0].into_iter()
                    .zip(p[1])
                    .map(|(a, b)| (f64::from(a) - f64::from(b)).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .sum::<f64>();
        let closed =
            points.len() > 3 && nearby(points[0], *points.last().expect("seed segment has two endpoints"));
        if closed {
            points.pop();
        }
        chains.push(Chain {
            points,
            closed,
            length,
        });
    }
    Ok(chains)
}

/// Collision triangles indexed by 2 m cells of the ground plane (x and z), to find the surface under a point.
#[derive(Debug, Default)]
pub struct SurfaceIndex {
    triangles: Vec<([[f64; 3]; 3], u16)>,
    cells: BTreeMap<[i64; 2], Vec<usize>>,
}

impl SurfaceIndex {
    /// Index every triangle of `soups`, each listed in every cell its xz box touches.
    pub fn new(soups: &[&Collision]) -> Result<Self> {
        let mut out = Self::default();
        for soup in soups {
            for (i, t) in soup.triangles.iter().enumerate() {
                let mut points = [[0.; 3]; 3];
                for (p, &index) in points.iter_mut().zip(t) {
                    *p = soup
                        .positions
                        .get(index as usize)
                        .ok_or_else(|| Error(format!("surface index: triangle {i} index")))?
                        .map(f64::from);
                }
                if points.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(Error(format!("surface index: nonfinite triangle {i}")));
                }
                let surface = *soup
                    .surfaces
                    .get(i)
                    .ok_or_else(|| Error(format!("surface index: triangle {i} has no surface")))?;
                let bounds: [(i64, i64); 2] = [0, 2].map(|axis| {
                    (
                        (points.iter().map(|v| v[axis]).fold(f64::INFINITY, f64::min) / 2.).floor() as i64,
                        (points.iter().map(|v| v[axis]).fold(f64::NEG_INFINITY, f64::max) / 2.).floor()
                            as i64,
                    )
                });
                let count = (i128::from(bounds[0].1) - i128::from(bounds[0].0) + 1)
                    * (i128::from(bounds[1].1) - i128::from(bounds[1].0) + 1);
                if count > 1_000_000 {
                    return Err(Error(format!("surface index: triangle {i} spans too many cells")));
                }
                let index = out.triangles.len();
                out.triangles.push((points, surface));
                for x in bounds[0].0..=bounds[0].1 {
                    for z in bounds[1].0..=bounds[1].1 {
                        out.cells.entry([x, z]).or_default().push(index);
                    }
                }
            }
        }
        Ok(out)
    }

    /// The surface of the triangle nearest `p` among those listed in `p`'s own cell (the first of equally near
    /// ones); `None` when the cell lists none.
    pub fn surface_at(&self, p: [f32; 3]) -> Option<u16> {
        let cell = [
            (f64::from(p[0]) / 2.).floor() as i64,
            (f64::from(p[2]) / 2.).floor() as i64,
        ];
        let mut best = f64::INFINITY;
        let mut surface = None;
        for &i in self.cells.get(&cell)? {
            let (t, s) = self.triangles[i];
            let d = crate::point_triangle_distance_squared(p.map(f64::from), t);
            if d < best {
                best = d;
                surface = Some(s);
            }
        }
        surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soup(triangles: &[([[f32; 3]; 3], u16)]) -> Collision {
        let mut c = Collision::default();
        for (t, s) in triangles {
            let start = c.positions.len() as u32;
            c.positions.extend(t);
            c.triangles.push([start, start + 1, start + 2]);
            c.surfaces.push(*s);
        }
        c
    }

    #[test]
    fn joins_by_distance_across_cells_not_cell_identity() {
        // 0.009 and 0.011 are in different 1 cm cells, 2 mm apart: joined
        let joined = chain(
            &[[[-1., 0., 0.], [0.009, 0., 0.]], [[0.011, 0., 0.], [1., 0., 0.]]],
            0.01,
        )
        .unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].points, [[-1., 0., 0.], [0.009, 0., 0.], [1., 0., 0.]]);
        // the same cell, but 1.39 cm apart: two chains
        let separate = chain(
            &[
                [[-1., 0., 0.], [0.001, 0.001, 0.001]],
                [[0.009, 0.009, 0.009], [1., 0., 0.]],
            ],
            0.01,
        )
        .unwrap();
        assert_eq!(separate.len(), 2);
        // a triangle of segments closes with its three corners
        let triangle = chain(
            &[
                [[0., 1., 0.], [1., 1., 0.]],
                [[1., 1., 0.], [1., 1., 1.]],
                [[1., 1., 1.], [0., 1., 0.]],
            ],
            0.01,
        )
        .unwrap();
        assert_eq!(triangle.len(), 1);
        assert!(triangle[0].closed);
        assert_eq!(triangle[0].points.len(), 3);
    }

    #[test]
    fn closes_loops_keeps_order_and_measures_length() {
        let square = [
            [[0., 0., 0.], [1., 0., 0.]],
            [[1., 0., 0.], [1., 0., 1.]],
            [[1., 0., 1.], [0., 0., 1.]],
            [[0., 0., 1.], [0., 0., 0.]],
            [[5., 0., 0.], [5., 0., 3.]],
        ];
        let chains = chain(&square, 0.01).unwrap();
        assert_eq!(chains.len(), 2);
        assert!(chains[0].closed);
        assert_eq!(chains[0].points.len(), 4);
        assert_eq!(chains[0].length, 4.);
        assert_eq!(chains[0].points[0], [0., 0., 0.]);
        assert!(!chains[1].closed);
        assert_eq!(chains[1].length, 3.);
        // a branch goes to the lower segment index; the other stays a chain of its own
        let branch = [
            [[0., 0., 0.], [1., 0., 0.]],
            [[1., 0., 0.], [2., 0., 0.]],
            [[1., 0., 0.], [1., 0., 1.]],
        ];
        let chains = chain(&branch, 0.01).unwrap();
        assert_eq!(chains.len(), 2);
        assert_eq!(chains[0].points, [[0., 0., 0.], [1., 0., 0.], [2., 0., 0.]]);
        assert_eq!(chains[1].points, [[1., 0., 0.], [1., 0., 1.]]);
        assert!(chain(&[[[f32::NAN, 0., 0.], [0.; 3]]], 0.01).is_err());
        assert!(chain(&[], 0.).is_err());
    }

    #[test]
    fn surface_is_the_nearest_triangle_in_the_points_cell_only() {
        let wood = 4;
        let stone = 1;
        let floor = soup(&[
            ([[0., 0., 0.], [1.9, 0., 0.], [0., 0., 1.9]], stone),
            ([[0., 1., 0.], [1.9, 1., 0.], [0., 1., 1.9]], wood),
        ]);
        let index = SurfaceIndex::new(&[&floor]).unwrap();
        assert_eq!(index.surface_at([0.5, 0.9, 0.5]), Some(wood));
        assert_eq!(index.surface_at([0.5, 0.1, 0.5]), Some(stone));
        // a point in an empty cell has none, even with a triangle a metre away in the next cell
        assert_eq!(index.surface_at([2.5, 0., 0.5]), None);
        assert!(
            SurfaceIndex::new(&[&Collision::default()])
                .unwrap()
                .surface_at([0.; 3])
                .is_none()
        );
        // a triangle larger than a cell is listed in every cell it covers
        let large = soup(&[([[-5., 0., -5.], [5., 0., -5.], [0., 0., 5.]], wood)]);
        let index = SurfaceIndex::new(&[&large]).unwrap();
        assert_eq!(index.surface_at([0.5, 1., 0.5]), Some(wood));
        assert_eq!(index.surface_at([-3.5, 1., -4.]), Some(wood));
    }
}
