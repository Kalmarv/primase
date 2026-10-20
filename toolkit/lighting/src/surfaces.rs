//! A world's placed surfaces, indexed for "is any surface within `d` of this point" ([`SurfaceGrid`]).

use primase_core::{Instance, Mesh, Topology, Transform, World};

pub use primase_meshops::Triangle;
use primase_meshops::TriangleGrid;

/// Triangles bucketed in cubes of `cell` metres, only within a box of interest (`bounds`): meshops' [`TriangleGrid`]
/// without labels. A query is answered exactly for a point whose distance ball lies within `bounds` (every triangle
/// that near touches it); beyond that it sees only what the box kept.
#[derive(Clone, Debug)]
pub struct SurfaceGrid(TriangleGrid);

impl SurfaceGrid {
    /// Index `triangles` (finite) in cells of `cell` metres (finite, above 0) within `bounds` (min and max corners,
    /// finite, min not above max), as [`TriangleGrid::new`] does.
    pub fn new(
        triangles: impl IntoIterator<Item = Triangle>,
        cell: f64,
        bounds: [[f64; 3]; 2],
    ) -> Result<SurfaceGrid, String> {
        TriangleGrid::new(triangles.into_iter().map(|t| (t, 0)), cell, bounds).map(SurfaceGrid)
    }

    /// The placed triangles of `w` within `bounds` ([`placed_triangles`]), indexed as [`SurfaceGrid::new`] does.
    pub fn of_world(w: &World, cell: f64, bounds: [[f64; 3]; 2]) -> Result<SurfaceGrid, String> {
        SurfaceGrid::new(placed_triangles(w)?, cell, bounds)
    }

    /// The triangles kept.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether a kept triangle is nearer `p` than `d` (squared distance below `d * d`).
    pub fn within(&self, p: [f64; 3], d: f64) -> bool {
        self.0.within(p, d)
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
