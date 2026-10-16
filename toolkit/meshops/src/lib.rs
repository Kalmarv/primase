//! Deterministic mesh operations with explicit float precision and stable topology.
mod distance;
mod layered;
pub use distance::{closest_point_on_triangle, point_triangle_distance_squared};
pub use layered::strongest_layer_parts;
mod hull;
mod primitive;
mod texcoords;
pub mod vector;
pub use hull::{convex_hull, hull_step};
pub use primitive::{CompactedPrimitive, compact_primitive, subset_primitive};
use std::collections::BTreeMap;
pub use texcoords::{BakeTextureTransforms, bake_texture_transforms};
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
/// Position-only weld. Subtract origin.x/y (z is unchanged), multiply by 1000f,
/// and round grid keys to even. Keep the first position at each key. Drop only
/// duplicate-index triangles; retain vertices created by subsequently dropped faces.
/// Keys outside signed 32-bit range are errors. Other attributes are not carried.
pub fn weld_millimetres(mesh: &Mesh, origin_xy: [f32; 2]) -> Result<Mesh> {
    weld_grid(mesh, origin_xy, 1000.)
}
/// Weld loose triangle corners using ties-to-even grid keys at the given positive
/// scale, with no origin subtraction. First representatives and face order survive.
pub fn weld_quantized(corners: &[[f32; 3]], scale: f32) -> Result<Mesh> {
    if corners.len() > u32::MAX as usize {
        return Err(Error("too many corners".into()));
    }
    let mesh = Mesh {
        positions: corners.to_vec(),
        indices: (0..corners.len() as u32).collect(),
        ..Mesh::default()
    };
    weld_grid(&mesh, [0., 0.], scale)
}
fn weld_grid(mesh: &Mesh, origin_xy: [f32; 2], scale: f32) -> Result<Mesh> {
    if !scale.is_finite() || scale <= 0. {
        return Err(Error("invalid weld grid scale".into()));
    }
    mesh.validate()?;
    finite(&origin_xy)?;
    let mut out = Mesh::default();
    let mut map = BTreeMap::<[i32; 3], u32>::new();
    for tri in mesh.indices.as_chunks::<3>().0 {
        let mut idx = [0; 3];
        for k in 0..3 {
            let p = mesh.positions[tri[k] as usize];
            let p = [p[0] - origin_xy[0], p[1] - origin_xy[1], p[2]];
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
/// Transform using column basis and translation. Classification uses f32 dot
/// products and the shared length. Orthogonal normal axes divide by max(length²,1e-12);
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
    if lengths.iter().any(|&l| l < 1e-6) || det.abs() <= 1e-6 * lengths[0] * lengths[1] * lengths[2] {
        return Err(Error("degenerate basis".into()));
    }
    let sheared = dot(ex, ey).abs() > 1e-4 * lengths[0] * lengths[1]
        || dot(ey, ez).abs() > 1e-4 * lengths[1] * lengths[2]
        || dot(ez, ex).abs() > 1e-4 * lengths[2] * lengths[0];
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
