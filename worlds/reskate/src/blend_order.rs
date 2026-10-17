//! Draw order of a lowered blend's overlays: each chain rank of a tile's overlays as an object of its own.
//!
//! The lerp chain of [`crate::layered`] needs a surface's overlays drawn in chain order where they overlap. skate.
//! draws the sections of one MeshSet in an order that changes from launch to launch, so overlays left in their
//! tile's mesh mix at random there. Separate objects are sorted back to front by a coarse depth of their bounds, and
//! objects that tie there are drawn farthest last (measured on ReSkate v2.1.0-pre, work/skate-vcolor/
//! S3_ORDER_DESIGN.md: with millimetre steps the order flips between 4 and 8 m; with 0.2 mm steps it held in every
//! frame of the distance sweeps, camera 0.5 to 200 m). So the writer gives each (dominant axis, chain rank) of a
//! tile's overlays its own object:
//! - the same local geometry as the tile mesh (same vertex positions, so the post-compile stage's half-float
//!   rounding is unchanged), placed at the tile's translation plus a lift along the axis, `(MAX_RANK + 1 - rank)`
//!   steps: the top of the chain lowest, so it ties and is drawn last;
//! - every rank of a (tile, axis) padded to the same bounds by two small zero-weight triangles at the corners of
//!   their common box, so the ranks differ in depth only by their lifts (objects whose bounds differ by more sort
//!   back to front instead, view-dependent).
//!
//! Known risk: the tie order is measured, not explained by any code read. A runtime that changes it reorders the
//! chain again (the mix stays smooth, as with shared MeshSets).

use std::collections::BTreeMap;

use primase_meshops::vector::Vec3;

/// The lift between neighbouring ranks, in metres (validated: 0.2 mm).
pub const LIFT_STEP: f32 = 0.0002;
/// The largest step [`lift_step`] widens to: the top of a chain then sits at most `MAX_RANK` x 2.5 mm = 1 cm off its
/// surface. Past it (or a translation that is not finite) a tile keeps its chain overlays in its mesh.
pub const MAX_LIFT_STEP: f32 = 0.0025;
/// The highest chain rank ([`crate::layered::VertexBlend::chain_rank`]: slots 1..=3, the darkening 4).
pub const MAX_RANK: u8 = 4;
/// The padding triangles' legs, in metres (shorter where the box is thinner).
const PAD: f32 = 0.01;

/// What the split did over a map (`stats.json`'s `blend_order`).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Stats {
    /// The cap on objects added ([`crate::map::WriterOptions::blend_objects`]).
    pub cap: usize,
    /// Objects added: one per (tile, axis, rank), each a MeshSet of its own.
    pub objects: usize,
    pub tiles_split: usize,
    pub overlay_triangles_split: usize,
    /// Tiles whose chain overlays stayed in the tile mesh (drawn in an unstable order) because the cap was reached,
    /// and their overlay triangles.
    pub tiles_shared: usize,
    pub overlay_triangles_shared: usize,
    /// Tiles whose chain overlays stayed in the tile mesh because no lift up to [`MAX_LIFT_STEP`] stays distinct as
    /// an f32 at their translation (or it is not finite), and their overlay triangles.
    pub tiles_unliftable: usize,
    pub overlay_triangles_unliftable: usize,
    /// (tile, axis) groups whose lift step had to grow past [`LIFT_STEP`] to stay distinct as f32, and the largest
    /// step used, in metres.
    pub steps_widened: usize,
    pub max_step_m: f32,
}

/// Triangles with their vertices: positions (xyz), normals (xyz), UVs (uv), blend weights (one per vertex) and
/// indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    pub pos: Vec<f32>,
    pub nrm: Vec<f32>,
    pub uv: Vec<f32>,
    pub blend: Vec<f32>,
    pub idx: Vec<u32>,
}

impl Geometry {
    fn vertex(&self, i: u32) -> Vec3 {
        Vec3::at(&self.pos, i as usize * 3)
    }
}

/// A triangle's dominant axis in the writer's frame: 0 +x, 1 -x, 2 +y, 3 -y, 4 +z, 5 -z (degenerate: +z).
pub fn dominant_axis(g: &Geometry, t: &[u32; 3]) -> u8 {
    let [a, b, c] = t.map(|i| g.vertex(i));
    let n = (b - a).cross(c - a);
    let m = [n.x, n.y, n.z];
    if m.iter().all(|&x| x == 0.0) || m.iter().any(|x| !x.is_finite()) {
        return 4;
    }
    // the largest magnitude; ties to the lower axis, so the choice is deterministic
    let mut i = 0;
    for k in 1..3 {
        if m[k].abs() > m[i].abs() {
            i = k;
        }
    }
    (2 * i + usize::from(m[i] < 0.0)) as u8
}

/// The unit vector of an axis ([`dominant_axis`]).
pub fn axis_vector(axis: u8) -> Vec3 {
    let s = if axis.is_multiple_of(2) { 1.0 } else { -1.0 };
    match axis / 2 {
        0 => Vec3::new(s, 0.0, 0.0),
        1 => Vec3::new(0.0, s, 0.0),
        _ => Vec3::new(0.0, 0.0, s),
    }
}

/// A part's triangles grouped by [`dominant_axis`], each group with only the vertices it draws (in first use order).
pub fn split_by_axis(g: &Geometry) -> BTreeMap<u8, Geometry> {
    let nv = g.pos.len() / 3;
    let mut out: BTreeMap<u8, (Geometry, Vec<u32>)> = BTreeMap::new();
    for t in g.idx.as_chunks::<3>().0 {
        let (h, remap) = out
            .entry(dominant_axis(g, t))
            .or_insert_with(|| (Geometry::default(), vec![u32::MAX; nv]));
        for &v in t {
            let vi = v as usize;
            if remap[vi] == u32::MAX {
                remap[vi] = (h.pos.len() / 3) as u32;
                h.pos.extend_from_slice(&g.pos[vi * 3..vi * 3 + 3]);
                h.nrm.extend_from_slice(&g.nrm[vi * 3..vi * 3 + 3]);
                h.uv.extend_from_slice(&g.uv[vi * 2..vi * 2 + 2]);
                if !g.blend.is_empty() {
                    h.blend.push(g.blend[vi]);
                }
            }
            h.idx.push(remap[vi]);
        }
    }
    out.into_iter().map(|(k, (h, _))| (k, h)).collect()
}

/// The box around every vertex of `gs` (`None`: no vertex).
pub fn bounds<'a>(gs: impl IntoIterator<Item = &'a Geometry>) -> Option<(Vec3, Vec3)> {
    let mut b: Option<(Vec3, Vec3)> = None;
    for g in gs {
        for p in g.pos.as_chunks::<3>().0 {
            let v = Vec3::new(p[0], p[1], p[2]);
            b = Some(match b {
                None => (v, v),
                Some((lo, hi)) => (
                    Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)),
                    Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)),
                ),
            });
        }
    }
    b
}

/// Two small zero-weight triangles at the box's two corners, inside it, facing along `axis`: every rank's object of
/// a group gets them, so they all have the box as their bounds.
pub fn pad(g: &mut Geometry, lo: Vec3, hi: Vec3, axis: u8) {
    // e1 x e2 = the axis direction
    let (e1, e2) = match axis / 2 {
        0 => (Vec3::UNIT_Y, Vec3::UNIT_Z),
        1 => (Vec3::UNIT_Z, Vec3::UNIT_X),
        _ => (Vec3::UNIT_X, Vec3::UNIT_Y),
    };
    let (e1, e2) = if axis.is_multiple_of(2) {
        (e1, e2)
    } else {
        (e2, e1)
    };
    let ext = hi - lo;
    let (s1, s2) = (PAD.min(ext.dot(e1).abs()), PAD.min(ext.dot(e2).abs()));
    let n = axis_vector(axis);
    for (corner, sign) in [(lo, 1.0f32), (hi, -1.0)] {
        let base = (g.pos.len() / 3) as u32;
        for v in [corner, corner + e1 * (sign * s1), corner + e2 * (sign * s2)] {
            g.pos.extend_from_slice(&[v.x, v.y, v.z]);
            g.nrm.extend_from_slice(&[n.x, n.y, n.z]);
            g.uv.extend_from_slice(&[0.0, 0.0]);
            g.blend.push(0.0);
        }
        g.idx.extend_from_slice(&[base, base + 1, base + 2]);
    }
}

/// The lift of `rank` (1..=[`MAX_RANK`]) along the axis in steps of `step`.
pub fn lift(rank: u8, step: f32) -> f32 {
    assert!(
        (1..=MAX_RANK).contains(&rank),
        "chain rank {rank} outside 1..={MAX_RANK}"
    );
    f32::from(MAX_RANK + 1 - rank) * step
}

/// The step for objects translated to `t` (writer frame) and lifted along `axis`: [`LIFT_STEP`], doubled until the
/// lifted translations, as the f32 the map stores, are distinct, in rank order, and in front of `t` (far from the
/// origin an f32 cannot hold 0.2 mm). `None` when `t` is not finite or the step would pass [`MAX_LIFT_STEP`].
pub fn lift_step(t: Vec3, axis: u8) -> Option<f32> {
    if ![t.x, t.y, t.z].iter().all(|x| x.is_finite()) {
        return None;
    }
    let n = axis_vector(axis);
    let along = |v: Vec3| v.dot(n);
    let mut step = LIFT_STEP;
    while step <= MAX_LIFT_STEP {
        let at: Vec<f32> = (1..=MAX_RANK).map(|r| along(t + n * lift(r, step))).collect();
        // rank 1 farthest out, the top rank nearest t but still in front of it
        if at.windows(2).all(|w| w[0] > w[1]) && at[at.len() - 1] > along(t) {
            return Some(step);
        }
        step *= 2.0;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Geometry {
        Geometry {
            pos: vec![0., 0., 0., 1., 0., 0., 0., 1., 0., 1., 1., 0.],
            nrm: [0., 0., 1.].repeat(4),
            uv: vec![0.; 8],
            blend: vec![0.5; 4],
            idx: vec![0, 1, 2, 2, 1, 3],
        }
    }

    #[test]
    fn axes_of_up_down_and_side_triangles() {
        let g = quad();
        assert_eq!(dominant_axis(&g, &[0, 1, 2]), 4);
        assert_eq!(dominant_axis(&g, &[0, 2, 1]), 5);
        let wall = Geometry {
            pos: vec![0., 0., 0., 0., 1., 0., 0., 0., 1.],
            ..Geometry::default()
        };
        assert_eq!(dominant_axis(&wall, &[0, 1, 2]), 0);
        assert_eq!(dominant_axis(&wall, &[0, 2, 1]), 1);
        assert_eq!(dominant_axis(&wall, &[0, 0, 1]), 4, "degenerate");
    }

    #[test]
    fn split_keeps_each_groups_vertices_only() {
        let mut g = quad();
        // the second triangle facing down
        g.idx = vec![0, 1, 2, 2, 3, 1];
        let s = split_by_axis(&g);
        assert_eq!(s.keys().copied().collect::<Vec<_>>(), vec![4, 5]);
        assert_eq!(s[&4].idx, vec![0, 1, 2]);
        assert_eq!(s[&4].pos.len(), 9);
        assert_eq!(s[&5].idx, vec![0, 1, 2]);
        assert_eq!(&s[&5].pos[3..6], &[1., 1., 0.]);
        assert_eq!(s[&5].blend, vec![0.5; 3]);
    }

    #[test]
    fn padding_sets_the_box_and_faces_the_axis() {
        let mut g = quad();
        let (lo, hi) = (Vec3::new(-2., -3., -0.5), Vec3::new(4., 5., 0.25));
        pad(&mut g, lo, hi, 4);
        assert_eq!(bounds([&g]), Some((lo, hi)));
        let t = g.idx.len() / 3;
        for k in [t - 2, t - 1] {
            let tri = [g.idx[k * 3], g.idx[k * 3 + 1], g.idx[k * 3 + 2]];
            assert_eq!(dominant_axis(&g, &tri), 4);
        }
        assert_eq!(&g.blend[4..], &[0.0; 6]);
        for axis in 0..6 {
            let mut h = Geometry::default();
            pad(&mut h, lo, hi, axis);
            assert_eq!(dominant_axis(&h, &[0, 1, 2]), axis);
            assert_eq!(dominant_axis(&h, &[3, 4, 5]), axis);
        }
    }

    #[test]
    fn no_lift_for_a_translation_that_is_not_finite_or_too_far_out() {
        for t in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(lift_step(Vec3::new(t, 0.0, 0.0), 0), None);
            assert_eq!(lift_step(Vec3::new(0.0, 0.0, t), 4), None);
        }
        // 1e6 m out an f32 step is 6 cm: no step up to 2.5 mm stays distinct
        assert_eq!(lift_step(Vec3::new(1.0e6, 0.0, 0.0), 0), None);
        // across the axis the translation's size does not matter
        assert_eq!(lift_step(Vec3::new(1.0e6, 0.0, 0.0), 4), Some(LIFT_STEP));
    }

    #[test]
    #[should_panic(expected = "outside 1..=4")]
    fn a_rank_outside_the_chain_has_no_lift() {
        lift(0, LIFT_STEP);
    }

    #[test]
    fn lifts_put_the_top_rank_lowest_and_survive_f32() {
        assert!(lift(MAX_RANK, LIFT_STEP) < lift(1, LIFT_STEP));
        assert_eq!(lift_step(Vec3::new(100.0, -900.0, 0.0), 4), Some(LIFT_STEP));
        // 3 km out along x a f32 holds steps of 0.24 mm: 0.2 mm would collapse, so the step grows
        let far = Vec3::new(3000.5, 0.0, 0.0);
        let s = lift_step(far, 0).unwrap();
        assert!(s > LIFT_STEP);
        let at: Vec<f32> = (1..=MAX_RANK).map(|r| far.x + lift(r, s)).collect();
        assert!(at.windows(2).all(|w| w[0] > w[1]) && at[3] > far.x);
        let s = lift_step(far, 1).unwrap();
        let at: Vec<f32> = (1..=MAX_RANK).map(|r| far.x - lift(r, s)).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]) && at[3] < far.x);
    }
}
