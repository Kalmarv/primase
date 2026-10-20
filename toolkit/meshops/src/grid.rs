//! Labelled triangles indexed in cubes, for "which surfaces lie within `d` of this point" ([`TriangleGrid`]).

use std::collections::BTreeMap;

use crate::point_triangle_distance_squared;

/// A triangle in the world frame, f64.
pub type Triangle = [[f64; 3]; 3];

/// Triangles, each with a label (the caller's: a group, a surface), bucketed in cubes of `cell` metres, only within a
/// box of interest (`bounds`): a triangle is listed in every cell its own box touches inside it, one whose box misses
/// it is not kept. A query is answered exactly for a point whose distance ball lies within `bounds` (every triangle
/// that near touches it); beyond that it sees only what the box kept. The distance is
/// [`point_triangle_distance_squared`]. The cell size changes how much each query visits, never its answer.
#[derive(Clone, Debug)]
pub struct TriangleGrid {
    cell: f64,
    triangles: Vec<(Triangle, u32)>,
    buckets: BTreeMap<[i64; 3], Vec<usize>>,
}

/// The most cells one triangle may be listed in: a bound on the index's size, which the box of interest and the cell
/// set (a triangle crossing the whole box is listed in every cell of it).
const MOST_CELLS: i128 = 16_000_000;

impl TriangleGrid {
    /// Index `triangles` (finite, each with its label) in cells of `cell` metres (finite, above 0) within `bounds`
    /// (min and max corners, finite, min not above max). Refused when one triangle would be listed in more than 16
    /// million cells: a box of interest too large for the cell.
    pub fn new(
        triangles: impl IntoIterator<Item = (Triangle, u32)>,
        cell: f64,
        bounds: [[f64; 3]; 2],
    ) -> Result<TriangleGrid, String> {
        if !(cell.is_finite() && cell > 0.0) {
            return Err(format!(
                "surface grid: cell {cell} is not a finite number above 0"
            ));
        }
        let [lo, hi] = bounds;
        if lo.iter().chain(&hi).any(|x| !x.is_finite()) || (0..3).any(|a| lo[a] > hi[a]) {
            return Err(format!("surface grid: bounds {bounds:?} are not a finite box"));
        }
        let key = |v: f64| (v / cell).floor() as i64;
        let mut grid = TriangleGrid {
            cell,
            triangles: Vec::new(),
            buckets: BTreeMap::new(),
        };
        for (n, (tri, label)) in triangles.into_iter().enumerate() {
            if tri.iter().flatten().any(|x| !x.is_finite()) {
                return Err(format!("surface grid: triangle {n} is not finite"));
            }
            let min = |a: usize| tri.iter().map(|p| p[a]).fold(f64::INFINITY, f64::min);
            let max = |a: usize| tri.iter().map(|p| p[a]).fold(f64::NEG_INFINITY, f64::max);
            if (0..3).any(|a| max(a) < lo[a] || min(a) > hi[a]) {
                continue;
            }
            let from: [i64; 3] = std::array::from_fn(|a| key(min(a).max(lo[a])));
            let to: [i64; 3] = std::array::from_fn(|a| key(max(a).min(hi[a])));
            let cells: i128 = (0..3)
                .map(|a| i128::from(to[a]) - i128::from(from[a]) + 1)
                .product();
            if cells > MOST_CELLS {
                return Err(format!(
                    "surface grid: triangle {n} would be listed in {cells} cells of {cell} m: the box of interest is \
                     too large for the cell"
                ));
            }
            let index = grid.triangles.len();
            grid.triangles.push((tri, label));
            for x in from[0]..=to[0] {
                for y in from[1]..=to[1] {
                    for z in from[2]..=to[2] {
                        grid.buckets.entry([x, y, z]).or_default().push(index);
                    }
                }
            }
        }
        Ok(grid)
    }

    /// The triangles kept.
    pub fn len(&self) -> usize {
        self.triangles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// The kept triangles whose cells a ball of radius `d` about `p` can reach (a triangle listed in several of them
    /// is visited once per cell).
    fn near(&self, p: [f64; 3], d: f64) -> impl Iterator<Item = &(Triangle, u32)> {
        let reach = (d / self.cell).ceil().max(0.0) as i64;
        let k: [i64; 3] = std::array::from_fn(|a| (p[a] / self.cell).floor() as i64);
        (k[0] - reach..=k[0] + reach)
            .flat_map(move |x| (k[1] - reach..=k[1] + reach).map(move |y| (x, y)))
            .flat_map(move |(x, y)| (k[2] - reach..=k[2] + reach).map(move |z| [x, y, z]))
            .flat_map(|c| self.buckets.get(&c).into_iter().flatten())
            .map(|&n| &self.triangles[n])
    }

    /// Whether a kept triangle is nearer `p` than `d` (squared distance below `d * d`).
    pub fn within(&self, p: [f64; 3], d: f64) -> bool {
        self.near(p, d)
            .any(|(t, _)| point_triangle_distance_squared(p, *t) < d * d)
    }

    /// For each label with a kept triangle at most `d` from `p`, the squared distance to its nearest such triangle.
    pub fn nearest_by_label(&self, p: [f64; 3], d: f64) -> BTreeMap<u32, f64> {
        let mut out: BTreeMap<u32, f64> = BTreeMap::new();
        for (t, label) in self.near(p, d) {
            let d2 = point_triangle_distance_squared(p, *t);
            if d2 <= d * d {
                out.entry(*label)
                    .and_modify(|best| *best = best.min(d2))
                    .or_insert(d2);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: [[f64; 3]; 2] = [[-10.0; 3], [10.0; 3]];

    fn floor(y: f64) -> Triangle {
        [[-5.0, y, -5.0], [5.0, y, -5.0], [0.0, y, 5.0]]
    }

    #[test]
    fn each_label_gives_its_nearest_triangle_within_d() {
        let grid = TriangleGrid::new([(floor(0.0), 1), (floor(0.5), 2), (floor(0.2), 2)], 0.25, BOX).unwrap();
        let near = grid.nearest_by_label([0.0, 0.3, 0.0], 0.3);
        assert_eq!(near.len(), 2);
        assert!((near[&1] - 0.09).abs() < 1e-12);
        // label 2's nearest is the floor at 0.2, not the one at 0.5
        assert!((near[&2] - 0.01).abs() < 1e-12);
        // at most d: inclusive, unlike within
        assert_eq!(
            grid.nearest_by_label([0.0, -0.5, 0.0], 0.5)
                .keys()
                .collect::<Vec<_>>(),
            [&1]
        );
        assert!(!grid.within([0.0, -0.5, 0.0], 0.5));
        assert!(grid.nearest_by_label([0.0, 3.0, 0.0], 0.5).is_empty());
    }

    #[test]
    fn a_point_is_within_d_of_a_surface_exactly_when_nearer_than_d() {
        let grid = TriangleGrid::new([(floor(0.0), 0)], 1.0, BOX).unwrap();
        assert_eq!(grid.len(), 1);
        assert!(grid.within([0.0, 0.99, 0.0], 1.0));
        assert!(!grid.within([0.0, 1.0, 0.0], 1.0), "at d: not nearer than d");
        assert!(!grid.within([0.0, 1.01, 0.0], 1.0));
        // past an edge, by the distance to the edge, not the plane
        assert!(!grid.within([0.0, 0.0, 5.9], 0.5));
        assert!(grid.within([0.0, 0.0, 5.4], 0.5));
        // a query distance larger than the cell looks as far as it reaches
        assert!(grid.within([0.0, 3.5, 0.0], 4.0));
        assert!(!grid.within([0.0, 4.5, 0.0], 4.0));
    }

    #[test]
    fn triangles_outside_the_box_of_interest_are_not_kept() {
        let far = floor(0.0).map(|p| [p[0] + 100.0, p[1], p[2]]);
        let grid = TriangleGrid::new([(floor(0.0), 0), (far, 0)], 2.0, BOX).unwrap();
        assert_eq!(grid.len(), 1);
        // a triangle crossing the box is kept, listed only in the box's cells
        let wide = [[-1e6, 1.0, -1e6], [1e6, 1.0, -1e6], [0.0, 1.0, 1e6]];
        let grid = TriangleGrid::new([(wide, 0)], 2.0, BOX).unwrap();
        assert!(grid.within([0.0, 2.0, 0.0], 1.5));
        assert!(grid.buckets.len() <= 11 * 11 * 11);
    }

    #[test]
    fn bad_input_is_refused() {
        assert!(TriangleGrid::new([(floor(0.0), 0)], 0.0, BOX).is_err());
        assert!(TriangleGrid::new([(floor(0.0), 0)], f64::NAN, BOX).is_err());
        assert!(TriangleGrid::new([(floor(0.0), 0)], 1.0, [[1.0; 3], [0.0; 3]]).is_err());
        let mut nan = floor(0.0);
        nan[1][1] = f64::NAN;
        assert!(TriangleGrid::new([(nan, 0)], 1.0, BOX).is_err());
        let big = [[-1e9; 3], [1e9; 3]];
        let wide = [[-1e6, 1.0, -1e6], [1e6, 1.0, -1e6], [0.0, 1.0, 1e6]];
        assert!(TriangleGrid::new([(wide, 0)], 0.01, big).is_err());
    }
}
