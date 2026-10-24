//! Deterministic mesh operations with explicit float precision and stable topology.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod back_faces;
mod clip;
pub use clip::{clip_convex_to_rect, clip_half_plane, twice_signed_area};
pub mod collision_query;
pub use back_faces::{ADD_BACK_FACES, AddBackFaces};
mod curves;
mod distance;
pub mod frames;
mod grid;
pub use grid::{Triangle, TriangleGrid};
mod layered;
pub use curves::{Chain, SurfaceIndex, chain};
pub use distance::{closest_point_on_triangle, point_triangle_distance_squared};
pub use layered::{SelectionPolicy, strongest_layer_parts};
mod layered_plan;
pub use layered_plan::{
    LayeredMeshSource, LayeredPartSource, LayeredPbrPlan, lower_layered_pbr, plan_layered_pbr,
};
mod draw_collision;
pub use draw_collision::{DRAW_COLLISION, DrawCollision};
mod group_primitives;
pub use group_primitives::{GROUP_PRIMITIVES, GroupPrimitives};
mod hull;
mod omit_placements;
pub use omit_placements::{OMIT_PLACEMENTS, OmitPlacements, PlacementCollision};
mod nonrigid;
pub use nonrigid::{BAKE_NONRIGID, BakeNonrigid, Mirrors, bake_nonrigid};
mod parametric;
pub use parametric::{FLATTEN_CURVES, FlattenCurves, TESSELLATE_COLLIDERS, TessellateColliders};

/// Register this crate's passes by their declarations.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    for spec in [
        &BAKE_NONRIGID,
        &BAKE_TEXTURE_TRANSFORMS,
        &TESSELLATE_COLLIDERS,
        &FLATTEN_CURVES,
        &SPLIT_UV_TILES,
        &ADD_BACK_FACES,
        &DRAW_COLLISION,
        &GROUP_PRIMITIVES,
        &OMIT_MATERIALLESS_PRIMITIVES,
        &OMIT_PLACEMENTS,
        &PARTITION_GRID,
        &SET_SHADOW_CASTING,
        &SPLIT_BY_COLLISION,
        &SPECIALIZE_MATERIAL_VARIANTS,
    ] {
        reg.add(spec);
    }
}
mod materialless;
pub use materialless::{OMIT_MATERIALLESS_PRIMITIVES, OmitMateriallessPrimitives};
mod partition;
pub use partition::{PARTITION_GRID, PartitionGrid};
mod primitive;
mod shadow_casting;
pub use shadow_casting::{SET_SHADOW_CASTING, SetShadowCasting};
mod split;
pub use split::{SPLIT_BY_COLLISION, SplitByCollision};
mod texcoords;
mod uv_tiles;
pub use uv_tiles::{MAX_TILES_PER_TRIANGLE, SPLIT_UV_TILES, SplitUvTiles};
mod variants;
pub use variants::{SPECIALIZE_MATERIAL_VARIANTS, SpecializeMaterialVariants, UnusedVariants};
pub mod vector;
pub use hull::{convex_hull, hull_step};
pub use primitive::{CompactedPrimitive, compact_primitive, subset_primitive, subset_primitive_gathered};
use std::collections::BTreeMap;
pub use texcoords::{BAKE_TEXTURE_TRANSFORMS, BakeTextureTransforms, bake_texture_transforms};
#[derive(Debug, thiserror::Error)]
#[error("invalid mesh: {0}")]
pub struct Error(pub String);
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) type V = [f32; 3];
pub(crate) fn add(a: V, b: V) -> V {
    (vector::Vec3::from(a) + vector::Vec3::from(b)).into()
}
pub(crate) fn sub(a: V, b: V) -> V {
    (vector::Vec3::from(a) - vector::Vec3::from(b)).into()
}
pub(crate) fn mul(a: V, s: f32) -> V {
    (vector::Vec3::from(a) * s).into()
}
pub(crate) fn dot(a: V, b: V) -> f32 {
    vector::Vec3::from(a).dot(b.into())
}
pub(crate) fn cross(a: V, b: V) -> V {
    vector::Vec3::from(a).cross(b.into()).into()
}
pub(crate) fn unit(v: V) -> V {
    vector::Vec3::from(v).normalize().into()
}
pub(crate) fn finite(v: &[f32]) -> Result<()> {
    if v.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(Error("non-finite value".into()))
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<V>,
    pub normals: Vec<V>,
    pub uv: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}
impl Mesh {
    pub fn validate(&self) -> Result<()> {
        if self.positions.len() > u32::MAX as usize
            || !self.indices.len().is_multiple_of(3)
            || self.indices.iter().any(|&i| i as usize >= self.positions.len())
        {
            return Err(Error("invalid triangle indices or vertex count".into()));
        }
        for len in [self.normals.len(), self.uv.len()] {
            if len != 0 && len != self.positions.len() {
                return Err(Error("attribute length differs from positions".into()));
            }
        }
        for p in self.positions.iter().chain(&self.normals) {
            finite(p)?;
        }
        for uv in &self.uv {
            finite(uv)?;
        }
        Ok(())
    }
    fn vertex_from(&mut self, source: &Self, i: usize) {
        self.positions.push(source.positions[i]);
        if !source.normals.is_empty() {
            self.normals.push(source.normals[i]);
        }
        if !source.uv.is_empty() {
            self.uv.push(source.uv[i]);
        }
    }
}
/// Compact in first index-use order, preserving every attribute and triangle.
pub fn compact(mesh: &Mesh) -> Result<Mesh> {
    mesh.validate()?;
    let mut out = Mesh::default();
    let mut map = BTreeMap::new();
    for &i in &mesh.indices {
        let n = if let Some(&n) = map.get(&i) {
            n
        } else {
            let n = out.positions.len() as u32;
            out.vertex_from(mesh, i as usize);
            map.insert(i, n);
            n
        };
        out.indices.push(n);
    }
    Ok(out)
}
/// Weld loose triangle corners using ties-to-even grid keys at the given positive
/// scale. First representatives and face order survive; a face whose corners share a cell is dropped, its vertices kept.
pub fn weld_quantized(corners: &[[f32; 3]], scale: f32) -> Result<Mesh> {
    if corners.len() > u32::MAX as usize {
        return Err(Error("too many corners".into()));
    }
    let mesh = Mesh {
        positions: corners.to_vec(),
        indices: (0..corners.len() as u32).collect(),
        ..Mesh::default()
    };
    weld_grid(&mesh, scale)
}
/// Weld loose triangle corners whose positions are bit-identical, keeping the first
/// representative; every face survives, in order, a face with coincident corners
/// included. Non-finite positions are errors.
pub fn weld_exact(corners: &[[f32; 3]]) -> Result<Mesh> {
    if corners.len() > u32::MAX as usize {
        return Err(Error("too many corners".into()));
    }
    if !corners.len().is_multiple_of(3) {
        return Err(Error("corners are not whole triangles".into()));
    }
    let mut out = Mesh::default();
    let mut map = BTreeMap::<[u32; 3], u32>::new();
    for p in corners {
        finite(p)?;
        let n = *map.entry(p.map(f32::to_bits)).or_insert_with(|| {
            out.positions.push(*p);
            (out.positions.len() - 1) as u32
        });
        out.indices.push(n);
    }
    Ok(out)
}
fn weld_grid(mesh: &Mesh, scale: f32) -> Result<Mesh> {
    if !scale.is_finite() || scale <= 0. {
        return Err(Error("invalid weld grid scale".into()));
    }
    mesh.validate()?;
    let mut out = Mesh::default();
    let mut map = BTreeMap::<[i32; 3], u32>::new();
    for tri in mesh.indices.as_chunks::<3>().0 {
        let mut idx = [0; 3];
        for k in 0..3 {
            let p = mesh.positions[tri[k] as usize];
            let mut key = [0; 3];
            for c in 0..3 {
                let q = (p[c] * scale).round_ties_even();
                if !q.is_finite() || (q as f64) < i32::MIN as f64 || (q as f64) > i32::MAX as f64 {
                    return Err(Error("weld grid key overflows i32".into()));
                }
                key[c] = q as i32;
            }
            idx[k] = *map.entry(key).or_insert_with(|| {
                let n = out.positions.len() as u32;
                out.positions.push(p);
                n
            });
        }
        if idx[0] != idx[1] && idx[1] != idx[2] && idx[0] != idx[2] {
            out.indices.extend_from_slice(&idx);
        }
    }
    Ok(out)
}
/// UV box tile count: floor(min+1e-5), max(floor(max-1e-5),lower),
/// then (upper-lower+1) on both axes, multiplied in f32. The repeat budget is 256.
pub fn uv_tiles(uv: [[f32; 2]; 3]) -> Result<f32> {
    for p in uv {
        finite(&p)?;
    }
    let axis = |k: usize| {
        let lo = (uv[0][k].min(uv[1][k].min(uv[2][k])) + 1e-5).floor();
        let hi = (uv[0][k].max(uv[1][k].max(uv[2][k])) - 1e-5).floor().max(lo);
        hi - lo + 1.
    };
    let n = axis(0) * axis(1);
    finite(&[n])?;
    Ok(n)
}
pub const MAX_UV_TILES: f32 = 256.;
/// Duplicate each face's three vertices. Use normalize(cross(b-a,c-a)) when its
/// squared length exceeds 1e-20; otherwise use +Z. Normalization uses the shared
/// f64 norm followed by component division and f32 rounding.
/// Preserve UVs; replace normals.
pub fn flat_normals(mesh: &Mesh) -> Result<Mesh> {
    mesh.validate()?;
    let mut out = Mesh::default();
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
        let n = cross(sub(b, a), sub(c, a));
        let n = if dot(n, n) > 1e-20 { unit(n) } else { [0., 0., 1.] };
        finite(&n)?;
        for &i in tri {
            out.positions.push(mesh.positions[i as usize]);
            out.normals.push(n);
            if !mesh.uv.is_empty() {
                out.uv.push(mesh.uv[i as usize]);
            }
            out.indices.push(out.indices.len() as u32);
        }
    }
    Ok(out)
}
/// What a vertex takes whose area-weighted normal has no direction ([`smooth_normals`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoDirection {
    /// Refused, naming the vertex.
    Refuse,
    /// This normal, the caller's stated stand-in (made unit), the vertex listed in [`SmoothNormals::stand_ins`].
    StandIn([f32; 3]),
}

/// A triangle list's smooth normals ([`smooth_normals`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SmoothNormals {
    /// One unit normal per position.
    pub normals: Vec<[f32; 3]>,
    /// The vertices that took the stand-in, in order.
    pub stand_ins: Vec<u32>,
}

/// The shortest summed face normal that has a direction: [`flat_normals`]' threshold (a squared length of 1e-20).
const SHORTEST_NORMAL: f64 = 1e-10;

/// Area-weighted smooth normals of a triangle list: each vertex's normal is the sum of `cross(b - a, c - a)` (the
/// triangle's unit normal times twice its area) of every triangle that uses it, in f64 in triangle order
/// (`primase_core::vector`), made unit and rounded to f32. A vertex whose sum is no longer than 1e-10 ([`flat_normals`]'
/// threshold: no triangle uses it, its triangles have no area, or their normals cancel) takes `none`. A triangle index
/// past the positions, a position that isn't finite, or a stand-in with no direction is refused.
pub fn smooth_normals(positions: &[V], triangles: &[[u32; 3]], none: NoDirection) -> Result<SmoothNormals> {
    use primase_core::vector::{add, cross, sub, unit, unit_beyond};
    for p in positions {
        finite(p)?;
    }
    let stand_in = match none {
        NoDirection::Refuse => None,
        NoDirection::StandIn(n) => {
            finite(&n)?;
            Some(
                unit(n.map(f64::from))
                    .ok_or_else(|| Error("a stand-in normal with no direction".into()))?
                    .map(|x| x as f32),
            )
        }
    };
    let at = |i: u32| -> Result<[f64; 3]> {
        positions
            .get(i as usize)
            .map(|p| p.map(f64::from))
            .ok_or_else(|| {
                Error(format!(
                    "triangle index {i} past the {} positions",
                    positions.len()
                ))
            })
    };
    let mut sums = vec![[0.0f64; 3]; positions.len()];
    for tri in triangles {
        let [a, b, c] = [at(tri[0])?, at(tri[1])?, at(tri[2])?];
        let face = cross(sub(b, a), sub(c, a));
        for &i in tri {
            sums[i as usize] = add(sums[i as usize], face);
        }
    }
    let mut out = SmoothNormals {
        normals: Vec::with_capacity(positions.len()),
        stand_ins: Vec::new(),
    };
    for (i, sum) in sums.into_iter().enumerate() {
        let n = match (unit_beyond(sum, SHORTEST_NORMAL), stand_in) {
            (Some(n), _) => n.map(|x| x as f32),
            (None, Some(n)) => {
                out.stand_ins.push(i as u32);
                n
            }
            (None, None) => {
                return Err(Error(format!(
                    "vertex {i}: its triangles' normals sum to no direction (none uses it, they have no area, or they cancel)"
                )));
            }
        };
        out.normals.push(n);
    }
    Ok(out)
}
/// Transform using column basis and translation. Classification uses f32 dot
/// products and the shared length, with the thresholds of `primase_core::Transform::classify`. Orthogonal normal axes divide by max(length²,1e-12);
/// sheared axes use cofactors times determinant sign. Normalize above length 1e-8,
/// otherwise +Z. Mirrored bases reverse triangle corners 1/2. Degenerate bases fail.
pub fn transform(mesh: &Mesh, basis: [V; 3], translation: V) -> Result<Mesh> {
    mesh.validate()?;
    for axis in basis {
        finite(&axis)?;
    }
    finite(&translation)?;
    let [ex, ey, ez] = basis;
    let lengths = basis.map(|v| vector::Vec3::from(v).length());
    let det = dot(cross(ex, ey), ez);
    finite(&[lengths[0], lengths[1], lengths[2], det])?;
    // the thresholds core names once, in f32 (both are the f32 nearest their decimal value)
    let (degenerate, shear) = (
        primase_core::DEGENERATE_BASIS as f32,
        primase_core::SHEAR_TOLERANCE as f32,
    );
    if lengths.iter().any(|&l| l < degenerate)
        || det.abs() <= degenerate * lengths[0] * lengths[1] * lengths[2]
    {
        return Err(Error("degenerate basis".into()));
    }
    let sheared = dot(ex, ey).abs() > shear * lengths[0] * lengths[1]
        || dot(ey, ez).abs() > shear * lengths[1] * lengths[2]
        || dot(ez, ex).abs() > shear * lengths[2] * lengths[0];
    let normals = if sheared {
        let sign = if det < 0. { -1. } else { 1. };
        [
            mul(cross(ey, ez), sign),
            mul(cross(ez, ex), sign),
            mul(cross(ex, ey), sign),
        ]
    } else {
        basis.map(|a| {
            let d = dot(a, a).max(1e-12);
            a.map(|v| v / d)
        })
    };
    let mut out = mesh.clone();
    for p in &mut out.positions {
        *p = std::array::from_fn(|k| translation[k] + ex[k] * p[0] + ey[k] * p[1] + ez[k] * p[2]);
        finite(p)?;
    }
    for n in &mut out.normals {
        let v = add(
            add(mul(normals[0], n[0]), mul(normals[1], n[1])),
            mul(normals[2], n[2]),
        );
        let len = vector::Vec3::from(v).length();
        *n = if len > 1e-8 { unit(v) } else { [0., 0., 1.] };
        finite(n)?;
    }
    if det < 0. {
        for tri in out.indices.as_chunks_mut::<3>().0 {
            tri.swap(1, 2);
        }
    }
    Ok(out)
}

/// Split every edge longer than max at shared midpoints. Traverse triangles and
/// children in the reference order; interpolate positions/UVs by (a+b)*0.5 and
/// normalize summed normals above squared length 1e-12 (otherwise retain a's).
/// A caller-provided split limit bounds growth; failure leaves the input untouched.
pub fn split_long(mesh: &Mesh, max: f32, split_limit: usize) -> Result<(Mesh, usize)> {
    mesh.validate()?;
    if !max.is_finite() || max <= 0. || !(max * max).is_finite() || max * max == 0. {
        return Err(Error("invalid edge threshold".into()));
    }
    let mut out = mesh.clone();
    out.indices.clear();
    let mut mids = BTreeMap::new();
    let mut stack: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.iter().rev().copied().collect();
    let mut splits = 0;
    fn mid(out: &mut Mesh, map: &mut BTreeMap<(u32, u32), u32>, i: u32, j: u32) -> Result<u32> {
        let key = (i.min(j), i.max(j));
        if let Some(&v) = map.get(&key) {
            return Ok(v);
        }
        let n = u32::try_from(out.positions.len()).map_err(|_| Error("too many vertices".into()))?;
        let (i, j) = (i as usize, j as usize);
        let p = mul(add(out.positions[i], out.positions[j]), 0.5);
        finite(&p)?;
        out.positions.push(p);
        if !out.normals.is_empty() {
            let sum = add(out.normals[i], out.normals[j]);
            let normal = if dot(sum, sum) > 1e-12 {
                unit(sum)
            } else {
                out.normals[i]
            };
            finite(&normal)?;
            out.normals.push(normal);
        }
        if !out.uv.is_empty() {
            let uv = std::array::from_fn(|k| (out.uv[i][k] + out.uv[j][k]) * 0.5);
            finite(&uv)?;
            out.uv.push(uv);
        }
        map.insert(key, n);
        Ok(n)
    }
    while let Some([a, b, c]) = stack.pop() {
        let long = |i: u32, j: u32| {
            let d = sub(out.positions[i as usize], out.positions[j as usize]);
            dot(d, d) > max * max
        };
        let (ab, bc, ca) = (long(a, b), long(b, c), long(c, a));
        if !ab && !bc && !ca {
            out.indices.extend_from_slice(&[a, b, c]);
            continue;
        }
        if splits == split_limit {
            return Err(Error("edge split limit exceeded".into()));
        }
        splits += 1;
        match (ab, bc, ca) {
            (true, true, true) => {
                let x = mid(&mut out, &mut mids, a, b)?;
                let y = mid(&mut out, &mut mids, b, c)?;
                let z = mid(&mut out, &mut mids, c, a)?;
                stack.extend([[x, y, z], [z, y, c], [x, b, y], [a, x, z]]);
            }
            (true, true, false) => {
                let x = mid(&mut out, &mut mids, a, b)?;
                let y = mid(&mut out, &mut mids, b, c)?;
                stack.extend([[a, y, c], [a, x, y], [x, b, y]]);
            }
            (false, true, true) => {
                let y = mid(&mut out, &mut mids, b, c)?;
                let z = mid(&mut out, &mut mids, c, a)?;
                stack.extend([[a, b, z], [z, b, y], [z, y, c]]);
            }
            (true, false, true) => {
                let z = mid(&mut out, &mut mids, c, a)?;
                let x = mid(&mut out, &mut mids, a, b)?;
                stack.extend([[x, c, z], [x, b, c], [a, x, z]]);
            }
            (true, false, false) => {
                let x = mid(&mut out, &mut mids, a, b)?;
                stack.extend([[x, b, c], [a, x, c]]);
            }
            (false, true, false) => {
                let y = mid(&mut out, &mut mids, b, c)?;
                stack.extend([[a, y, c], [a, b, y]]);
            }
            (false, false, true) => {
                let z = mid(&mut out, &mut mids, c, a)?;
                stack.extend([[a, b, z], [z, b, c]]);
            }
            _ => unreachable!(),
        }
    }
    Ok((out, splits))
}
mod primitives;
pub use primitives::{box_mesh, capsule, cylinder, disc, sphere};

#[cfg(test)]
mod tests {
    use super::{NoDirection, smooth_normals};
    use primase_core::pass::{Options, PassRegistry};

    /// The crate registers exactly these passes by their declarations: those with required options are made only with
    /// them, and every one says what it takes.
    #[test]
    fn the_meshops_passes_register_by_name() {
        let mut reg = PassRegistry::new();
        super::register(&mut reg);
        let names = [
            "add-back-faces",
            "bake-nonrigid",
            "bake-texture-transforms",
            "draw-collision",
            "flatten-curves",
            "group-primitives",
            "omit-materialless-primitives",
            "omit-placements",
            "partition-grid",
            "set-shadow-casting",
            "specialize-material-variants",
            "split-by-collision",
            "split-uv-tiles",
            "tessellate-colliders",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        let free = [
            "add-back-faces",
            "bake-texture-transforms",
            "draw-collision",
            "omit-materialless-primitives",
            "tessellate-colliders",
        ];
        for name in names {
            assert_eq!(reg.requires_options(name), !free.contains(&name), "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            assert_eq!(
                reg.make(name, &Options::new()).is_err(),
                !free.contains(&name),
                "{name}"
            );
        }
        assert!(
            reg.describe("partition-grid")
                .unwrap()
                .starts_with("partition-grid{cell*,origin,halo,region,tile}\n")
        );
    }

    /// A unit square of two triangles in z = 0 and a vertex no triangle uses: the square's vertices face +z, the corner
    /// two triangles share weighs both; a vertex with no triangle, and one whose triangles cancel, takes the stand-in
    /// (listed) or is refused by number.
    #[test]
    fn smooth_normals_are_area_weighted_with_a_stated_stand_in() {
        let square = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [5.0, 5.0, 5.0],
        ];
        let tris = [[0, 1, 2], [0, 2, 3]];
        let up = NoDirection::StandIn([0.0, 2.0, 0.0]);
        let s = smooth_normals(&square, &tris, up).unwrap();
        assert_eq!(&s.normals[..4], &[[0.0, 0.0, 1.0]; 4]);
        assert_eq!(s.normals[4], [0.0, 1.0, 0.0], "the stand-in, made unit");
        assert_eq!(s.stand_ins, [4]);
        let refused = smooth_normals(&square, &tris, NoDirection::Refuse).unwrap_err();
        assert!(refused.0.contains("vertex 4"), "{refused}");
        // a large triangle and a small one at right angles share vertex 0: the sum leans to the larger
        let bent = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 1.0]];
        let s = smooth_normals(&bent, &[[0, 1, 2], [0, 3, 1]], NoDirection::Refuse).unwrap();
        // faces: (0, 0, 16) and (0, 4, 0); their sum (0, 4, 16) made unit
        let l = (16.0f64 * 16.0 + 4.0 * 4.0).sqrt();
        assert_eq!(s.normals[0], [0.0, (4.0 / l) as f32, (16.0 / l) as f32]);
        // two faces wound against each other cancel
        let both = smooth_normals(&square[..3], &[[0, 1, 2], [0, 2, 1]], up).unwrap();
        assert_eq!(both.stand_ins, [0, 1, 2]);
        // an index past the positions, a position that isn't finite, a stand-in with no direction
        assert!(smooth_normals(&square, &[[0, 1, 9]], up).is_err());
        assert!(smooth_normals(&[[f32::NAN, 0.0, 0.0]], &[], up).is_err());
        assert!(smooth_normals(&square, &tris, NoDirection::StandIn([0.0; 3])).is_err());
    }
}
