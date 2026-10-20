//! A world's placed surfaces, indexed for "is any surface within `d` of this point" ([`SurfaceGrid`]).

use std::collections::BTreeMap;

use primase_core::{Instance, Mesh, Topology, Transform, World};

/// A triangle in the world frame, f64.
pub type Triangle = [[f64; 3]; 3];

/// Triangles bucketed in cubes of `cell` metres, only within a box of interest (`bounds`): a triangle is listed in
/// every cell its own box touches inside it, one whose box misses it is not kept. A query is answered exactly for a
/// point whose distance ball lies within `bounds` (every triangle that near touches it); beyond that it sees only what
/// the box kept. The distance is meshops' ([`primase_meshops::point_triangle_distance_squared`]).
#[derive(Clone, Debug)]
pub struct SurfaceGrid {
    cell: f64,
    triangles: Vec<Triangle>,
    buckets: BTreeMap<[i64; 3], Vec<usize>>,
}

/// The most cells one triangle may be listed in: a bound on the index's size, which the box of interest and the cell
/// set (a triangle crossing the whole box is listed in every cell of it).
const MOST_CELLS: i128 = 16_000_000;

impl SurfaceGrid {
    /// Index `triangles` (finite) in cells of `cell` metres (finite, above 0) within `bounds` (min and max corners,
    /// finite, min not above max). Refused when one triangle would be listed in more than 16 million cells: a box of
    /// interest too large for the cell.
    pub fn new(
        triangles: impl IntoIterator<Item = Triangle>,
        cell: f64,
        bounds: [[f64; 3]; 2],
    ) -> Result<SurfaceGrid, String> {
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
        let mut grid = SurfaceGrid {
            cell,
            triangles: Vec::new(),
            buckets: BTreeMap::new(),
        };
        for (n, tri) in triangles.into_iter().enumerate() {
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
            grid.triangles.push(tri);
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

    /// The placed triangles of `w` within `bounds` ([`placed_triangles`]), indexed as [`SurfaceGrid::new`] does.
    pub fn of_world(w: &World, cell: f64, bounds: [[f64; 3]; 2]) -> Result<SurfaceGrid, String> {
        SurfaceGrid::new(placed_triangles(w)?, cell, bounds)
    }

    /// The triangles kept.
    pub fn len(&self) -> usize {
        self.triangles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Whether a kept triangle is nearer `p` than `d` (squared distance below `d * d`).
    pub fn within(&self, p: [f64; 3], d: f64) -> bool {
        // the cells a ball of radius d about p can reach
        let reach = (d / self.cell).ceil().max(0.0) as i64;
        let k: [i64; 3] = std::array::from_fn(|a| (p[a] / self.cell).floor() as i64);
        for x in k[0] - reach..=k[0] + reach {
            for y in k[1] - reach..=k[1] + reach {
                for z in k[2] - reach..=k[2] + reach {
                    for &n in self.buckets.get(&[x, y, z]).into_iter().flatten() {
                        if primase_meshops::point_triangle_distance_squared(p, self.triangles[n]) < d * d {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

/// Every triangle the world places: each placement's (`core.instance` with a `core.transform`) prototype mesh's
/// triangle primitives, each corner placed by the transform in f64. A placement without a transform or whose prototype
/// has no mesh places none; lines and points are not surfaces. An index past its primitive's vertices is refused.
pub fn placed_triangles(w: &World) -> Result<Vec<Triangle>, String> {
    let mut out = Vec::new();
    for (e, i) in w.iter::<Instance>() {
        let (Some(t), Some(mesh)) = (w.get::<Transform>(e), w.get::<Mesh>(i.prototype)) else {
            continue;
        };
        let place = |p: [f32; 3]| -> [f64; 3] {
            std::array::from_fn(|r| {
                (0..3)
                    .map(|c| f64::from(t.cols[c][r]) * f64::from(p[c]))
                    .sum::<f64>()
                    + f64::from(t.cols[3][r])
            })
        };
        for (n, p) in mesh
            .primitives()
            .iter()
            .enumerate()
            .filter(|(_, p)| p.topology == Topology::Triangles)
        {
            let Some(pos) = p.positions() else { continue };
            let corner = |k: u32| {
                pos.get(k as usize).copied().map(place).ok_or_else(|| {
                    format!(
                        "placement {e:?}: primitive {n} of mesh {:?} indexes vertex {k} of {}",
                        i.prototype,
                        pos.len()
                    )
                })
            };
            let corners: Vec<u32> = match &p.indices {
                Some(ix) => ix.clone(),
                None => (0..pos.len() as u32).collect(),
            };
            for c in corners.as_chunks::<3>().0 {
                out.push([corner(c[0])?, corner(c[1])?, corner(c[2])?]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: [[f64; 3]; 2] = [[-10.0; 3], [10.0; 3]];

    fn floor() -> Triangle {
        [[-5.0, 0.0, -5.0], [5.0, 0.0, -5.0], [0.0, 0.0, 5.0]]
    }

    #[test]
    fn a_point_is_within_d_of_a_surface_exactly_when_nearer_than_d() {
        let grid = SurfaceGrid::new([floor()], 1.0, BOX).unwrap();
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
        let far = floor().map(|p| [p[0] + 100.0, p[1], p[2]]);
        let grid = SurfaceGrid::new([floor(), far], 2.0, BOX).unwrap();
        assert_eq!(grid.len(), 1);
        // a triangle crossing the box is kept, listed only in the box's cells
        let wide = [[-1e6, 1.0, -1e6], [1e6, 1.0, -1e6], [0.0, 1.0, 1e6]];
        let grid = SurfaceGrid::new([wide], 2.0, BOX).unwrap();
        assert!(grid.within([0.0, 2.0, 0.0], 1.5));
        assert!(grid.buckets.len() <= 11 * 11 * 11);
    }

    #[test]
    fn bad_input_is_refused() {
        assert!(SurfaceGrid::new([floor()], 0.0, BOX).is_err());
        assert!(SurfaceGrid::new([floor()], f64::NAN, BOX).is_err());
        assert!(SurfaceGrid::new([floor()], 1.0, [[1.0; 3], [0.0; 3]]).is_err());
        let mut nan = floor();
        nan[1][1] = f64::NAN;
        assert!(SurfaceGrid::new([nan], 1.0, BOX).is_err());
        let big = [[-1e9; 3], [1e9; 3]];
        let wide = [[-1e6, 1.0, -1e6], [1e6, 1.0, -1e6], [0.0, 1.0, 1e6]];
        assert!(SurfaceGrid::new([wide], 0.01, big).is_err());
    }
}
