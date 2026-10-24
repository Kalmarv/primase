//! The steep ground's charts: the part of `lower-anvil-terrain`'s ground that is steeper than a stated angle, laid out
//! in a vertical plane so a texel is a fixed length of the surface however steep it is (`steep=chart`).
//!
//! A top-down texture spends `pixel / cos(slope)` of surface on a pixel (8.8 cm at 45 degrees, 36 cm at 80, for a 6.25
//! cm pixel), and a face steeper than that has too few pixels, stretched along its fall line. The game's own near
//! colour on such a face is drawn by the vertical-triangle pixel program (ps 1924, docs/terrain.md 3.10 (3)), which
//! addresses the details in the vertical planes (x, z) and (y, z), not in (x, y). Here the steep triangles are taken out
//! of their node's top-down primitive and put in a primitive of their own, each connected face a **chart** in the plane
//! it looks along (a face whose normal points mostly along y in (x, z); mostly along x in (y, z)), the charts packed in
//! one atlas per node at the same pixel size as the top-down bake, each texel baked at the point of the surface it maps
//! to ([`super::splat::Baker`], with [`super::splat::DetailPlane::Vertical`] at the point's normal).
//!
//! **Ours, not the game's:** the threshold angle (the game's draw lists split the terrain into flat, mixed and vertical
//! triangles by a rule on the CPU that is not read; the vertex stage carries no class), the laying out (the game draws
//! the terrain as one mesh with no charts), and the padding of two pixels around a chart.
//!
//! A chart whose faces overlap in its plane (a fold of the surface) gives the texel to the first and counts it; an atlas
//! that would pass `steep-max-atlas` pixels on a side (the program's choice: its destination's texture cap) is built at
//! twice the pixel size, again and again until it fits, and the
//! halving counted.

use std::collections::BTreeMap;

/// Pixels of padding around a chart: the texels beyond its faces take the nearest face's edge.
pub(crate) const PAD: u32 = 2;

/// The vertical plane a chart is laid out in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Plane {
    /// (u, v) = (x, z): the faces look along y.
    Xz,
    /// (u, v) = (y, z): the faces look along x.
    Yz,
}

impl Plane {
    pub fn project(self, p: [f64; 3]) -> [f64; 2] {
        match self {
            Plane::Xz => [p[0], p[2]],
            Plane::Yz => [p[1], p[2]],
        }
    }
}

/// One connected steep face laid out in a plane.
#[derive(Clone, Debug)]
pub(crate) struct Chart {
    pub plane: Plane,
    /// Its triangles, by their index in the list the split was made from.
    pub triangles: Vec<usize>,
    /// The least (u, v) of its vertices.
    pub min: [f64; 2],
    /// Its size in pixels, padding included.
    pub size: [u32; 2],
    /// Its corner in the atlas, in pixels.
    pub at: [u32; 2],
}

/// The steep ground of one piece: which triangles are steep, and their charts in one atlas.
#[derive(Clone, Debug)]
pub(crate) struct Split {
    /// Per triangle: whether it is steeper than the threshold.
    pub steep: Vec<bool>,
    pub charts: Vec<Chart>,
    /// The atlas size in pixels, and the pixel's size in source units (the stated one, doubled `halvings` times).
    pub size: [u32; 2],
    pub pixel: f64,
    pub halvings: u32,
}

/// A triangle's normal in the terrain's source frame, pointing up.
fn normal_of(p: [[f64; 3]; 3]) -> [f64; 3] {
    let (a, b) = (
        [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]],
        [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]],
    );
    let n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    if n[2] < 0.0 { n.map(|c| -c) } else { n }
}

/// The slope of a triangle in degrees from the horizontal.
pub(crate) fn slope_degrees(p: [[f64; 3]; 3]) -> f64 {
    let n = normal_of(p);
    n[0].hypot(n[1]).atan2(n[2]).to_degrees()
}

/// The plane and side a steep triangle looks along: the larger horizontal component of its normal, and its sign.
fn facing(p: [[f64; 3]; 3]) -> (Plane, bool) {
    let n = normal_of(p);
    if n[0].abs() >= n[1].abs() {
        (Plane::Yz, n[0] >= 0.0)
    } else {
        (Plane::Xz, n[1] >= 0.0)
    }
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The triangles `tris` (vertex numbers into `point`, the source frame) split into steep ones (steeper than
/// `threshold` degrees) and the rest, the steep ones into charts (connected by shared edges, one plane and side each),
/// the charts laid out at `pixel` source units a texel in an atlas of at most `max_atlas` pixels a side (the pixel
/// doubled until they fit). `None` where none is steep.
pub(crate) fn split(
    point: &dyn Fn(u32) -> [f64; 3],
    tris: &[[u32; 3]],
    threshold: f64,
    pixel: f64,
    max_atlas: u32,
) -> Option<Split> {
    let corners = |t: &[u32; 3]| t.map(point);
    let steep: Vec<bool> = tris
        .iter()
        .map(|t| slope_degrees(corners(t)) > threshold)
        .collect();
    if !steep.iter().any(|&s| s) {
        return None;
    }
    // components: steep triangles sharing an edge and facing alike
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    let kinds: Vec<Option<(Plane, bool)>> = tris
        .iter()
        .zip(&steep)
        .map(|(t, &s)| s.then(|| facing(corners(t))))
        .collect();
    let mut edges: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (i, t) in tris.iter().enumerate() {
        if kinds[i].is_none() {
            continue;
        }
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let key = (a.min(b), a.max(b));
            match edges.get(&key) {
                Some(&j) if kinds[j] == kinds[i] => {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    parent[ri] = rj;
                }
                Some(_) => {}
                None => {
                    edges.insert(key, i);
                }
            }
        }
    }
    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, kind) in kinds.iter().enumerate() {
        if kind.is_some() {
            let r = find(&mut parent, i);
            by_root.entry(r).or_default().push(i);
        }
    }
    let mut halvings = 0u32;
    loop {
        let px = pixel * f64::from(1u32 << halvings);
        let mut charts: Vec<Chart> = by_root
            .values()
            .map(|members| {
                let plane = kinds[members[0]].map_or(Plane::Xz, |k| k.0);
                let mut lo = [f64::INFINITY; 2];
                let mut hi = [f64::NEG_INFINITY; 2];
                for &i in members {
                    for v in tris[i] {
                        let uv = plane.project(point(v));
                        for k in 0..2 {
                            lo[k] = lo[k].min(uv[k]);
                            hi[k] = hi[k].max(uv[k]);
                        }
                    }
                }
                let size = [0, 1].map(|k| ((hi[k] - lo[k]) / px).ceil().max(1.0) as u32 + 2 * PAD);
                Chart {
                    plane,
                    triangles: members.clone(),
                    min: lo,
                    size,
                    at: [0, 0],
                }
            })
            .collect();
        if let Some(size) = pack(&mut charts, max_atlas) {
            return Some(Split {
                steep,
                charts,
                size,
                pixel: px,
                halvings,
            });
        }
        halvings += 1;
        assert!(halvings < 16, "a chart cannot be packed at any size");
    }
}

/// Shelf packing of `charts` into the smallest atlas by area of those of width a power of two from the widest chart's
/// up to `max_atlas` (the last, `max_atlas` itself) that keep its height within `max_atlas`; the corners set in
/// `charts`; `None` where none fits.
fn pack(charts: &mut [Chart], max_atlas: u32) -> Option<[u32; 2]> {
    let widest = charts.iter().map(|c| c.size[0]).max()?;
    let tallest = charts.iter().map(|c| c.size[1]).max()?;
    if widest > max_atlas || tallest > max_atlas {
        return None;
    }
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(charts[i].size[1]),
            std::cmp::Reverse(charts[i].size[0]),
            i,
        )
    });
    let mut best: Option<(u64, u32, Vec<[u32; 2]>, u32)> = None;
    let mut width = widest.next_power_of_two().max(16).min(max_atlas);
    loop {
        let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
        let mut at = vec![[0u32; 2]; charts.len()];
        for &i in &order {
            let [w, h] = charts[i].size;
            if x + w > width {
                y += shelf;
                x = 0;
                shelf = 0;
            }
            at[i] = [x, y];
            x += w;
            shelf = shelf.max(h);
        }
        let height = y + shelf;
        if height <= max_atlas {
            let area = u64::from(width) * u64::from(height);
            if best.as_ref().is_none_or(|b| area < b.0) {
                best = Some((area, width, at, height));
            }
        }
        if width >= max_atlas {
            break;
        }
        width = (width * 2).min(max_atlas);
    }
    let (_, width, at, height) = best?;
    for (c, a) in charts.iter_mut().zip(at) {
        c.at = a;
    }
    Some([width, height])
}

/// One chart texel's point on the surface and its normal (source frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sample {
    pub point: [f64; 3],
    pub normal: [f64; 3],
}

/// A chart's texels, row by row from its corner: the point of the surface each maps to (`None` where no face is within
/// the padding), and how many were claimed by a second face (a fold).
pub(crate) struct Texels {
    pub samples: Vec<Option<Sample>>,
    pub overlaps: u64,
}

/// `n` of unit length ([`primase_core::vector::unit`]); a normal with no direction (a triangle with no area) reads
/// as up, z.
fn unit(n: [f64; 3]) -> [f64; 3] {
    primase_core::vector::unit(n).unwrap_or([0.0, 0.0, 1.0])
}

/// The barycentric coordinates of the point of triangle `(a, b, c)` nearest `p` (Ericson, real-time collision detection).
fn nearest(a: [f64; 2], b: [f64; 2], c: [f64; 2], p: [f64; 2]) -> [f64; 3] {
    let sub = |x: [f64; 2], y: [f64; 2]| [x[0] - y[0], x[1] - y[1]];
    let dot = |x: [f64; 2], y: [f64; 2]| x[0] * y[0] + x[1] * y[1];
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    [1.0 - v - w, v, w]
}

/// Chart `chart`'s texels at `pixel` source units: each texel centre inside a face takes the point of that face under
/// it (the first face to claim it keeps it, a second counted), then each remaining texel within the padding the nearest
/// point of the nearest face.
pub(crate) fn texels(
    chart: &Chart,
    tris: &[[u32; 3]],
    point: &dyn Fn(u32) -> [f64; 3],
    normal: &dyn Fn(u32) -> [f64; 3],
    pixel: f64,
) -> Texels {
    let [w, h] = chart.size;
    let mut samples: Vec<Option<Sample>> = vec![None; w as usize * h as usize];
    let mut taken = vec![false; samples.len()];
    let mut overlaps = 0u64;
    let centre = |i: u32, j: u32| {
        [
            chart.min[0] + (f64::from(i) + 0.5 - f64::from(PAD)) * pixel,
            chart.min[1] + (f64::from(j) + 0.5 - f64::from(PAD)) * pixel,
        ]
    };
    let at = |lambda: [f64; 3], p: [[f64; 3]; 3], n: [[f64; 3]; 3]| Sample {
        point: std::array::from_fn(|k| lambda[0] * p[0][k] + lambda[1] * p[1][k] + lambda[2] * p[2][k]),
        normal: unit(std::array::from_fn(|k| {
            lambda[0] * n[0][k] + lambda[1] * n[1][k] + lambda[2] * n[2][k]
        })),
    };
    // the texel range a triangle's projection (grown by `grow` pixels) covers
    let range = |uv: [[f64; 2]; 3], grow: f64| {
        let lo = [0, 1].map(|k| uv[0][k].min(uv[1][k]).min(uv[2][k]));
        let hi = [0, 1].map(|k| uv[0][k].max(uv[1][k]).max(uv[2][k]));
        let from = [0, 1].map(|k| {
            ((lo[k] - chart.min[k]) / pixel + f64::from(PAD) - grow)
                .floor()
                .max(0.0) as u32
        });
        let to = [0, 1].map(|k| {
            (((hi[k] - chart.min[k]) / pixel + f64::from(PAD) + grow)
                .ceil()
                .max(0.0) as u32)
                .min(chart.size[k] - 1)
        });
        (from, to)
    };
    let epsilon = 1e-9;
    // inside a face
    for &ti in &chart.triangles {
        let t = tris[ti];
        let p = t.map(point);
        let n = t.map(normal);
        let uv = p.map(|q| chart.plane.project(q));
        let area =
            (uv[1][0] - uv[0][0]) * (uv[2][1] - uv[0][1]) - (uv[2][0] - uv[0][0]) * (uv[1][1] - uv[0][1]);
        if area == 0.0 {
            continue;
        }
        let (from, to) = range(uv, 0.0);
        for j in from[1]..=to[1] {
            for i in from[0]..=to[0] {
                let q = centre(i, j);
                let l0 =
                    ((uv[1][0] - q[0]) * (uv[2][1] - q[1]) - (uv[2][0] - q[0]) * (uv[1][1] - q[1])) / area;
                let l1 =
                    ((uv[2][0] - q[0]) * (uv[0][1] - q[1]) - (uv[0][0] - q[0]) * (uv[2][1] - q[1])) / area;
                let l2 = 1.0 - l0 - l1;
                if l0 < -epsilon || l1 < -epsilon || l2 < -epsilon {
                    continue;
                }
                let k = j as usize * w as usize + i as usize;
                if taken[k] {
                    overlaps += 1;
                } else {
                    taken[k] = true;
                    samples[k] = Some(at([l0, l1, l2], p, n));
                }
            }
        }
    }
    // the padding: the nearest face's nearest point
    let mut nearest_distance = vec![f64::INFINITY; samples.len()];
    let reach = f64::from(PAD) * pixel;
    for &ti in &chart.triangles {
        let t = tris[ti];
        let p = t.map(point);
        let n = t.map(normal);
        let uv = p.map(|q| chart.plane.project(q));
        let (from, to) = range(uv, f64::from(PAD));
        for j in from[1]..=to[1] {
            for i in from[0]..=to[0] {
                let k = j as usize * w as usize + i as usize;
                if taken[k] {
                    continue;
                }
                let q = centre(i, j);
                let lambda = nearest(uv[0], uv[1], uv[2], q);
                let on = [0, 1].map(|c| lambda[0] * uv[0][c] + lambda[1] * uv[1][c] + lambda[2] * uv[2][c]);
                let d = (on[0] - q[0]).hypot(on[1] - q[1]);
                if d <= reach && d < nearest_distance[k] {
                    nearest_distance[k] = d;
                    samples[k] = Some(at(lambda, p, n));
                }
            }
        }
    }
    Texels { samples, overlaps }
}

/// The atlas coordinates of a point of the surface `p` in chart `chart` (row 0 at the least v, as the top-down bake's
/// row 0 is at its least y).
pub(crate) fn uv_of(chart: &Chart, atlas: [u32; 2], pixel: f64, p: [f64; 3]) -> [f32; 2] {
    let q = chart.plane.project(p);
    [
        ((f64::from(chart.at[0]) + f64::from(PAD) + (q[0] - chart.min[0]) / pixel) / f64::from(atlas[0]))
            as f32,
        ((f64::from(chart.at[1]) + f64::from(PAD) + (q[1] - chart.min[1]) / pixel) / f64::from(atlas[1]))
            as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vertices (x, y, h(x, y)) over `columns` by `rows` samples 0.5 apart, row by row, and the two triangles of each
    /// square as the lowering makes them.
    fn grid(columns: usize, rows: usize, h: impl Fn(f64, f64) -> f64) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let mut points = Vec::new();
        for r in 0..rows {
            for c in 0..columns {
                let (x, y) = (c as f64 * 0.5, r as f64 * 0.5);
                points.push([x, y, h(x, y)]);
            }
        }
        let mut tris = Vec::new();
        for r in 0..rows - 1 {
            for c in 0..columns - 1 {
                let a = (r * columns + c) as u32;
                let (b, d) = (a + 1, a + columns as u32);
                let f = d + 1;
                tris.push([a, b, f]);
                tris.push([a, f, d]);
            }
        }
        (points, tris)
    }

    #[test]
    fn a_triangle_rising_one_in_one_is_45_degrees() {
        let s = slope_degrees([[0.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 1.0, 0.0]]);
        assert!((s - 45.0).abs() < 1e-9, "{s}");
        // either winding
        let s = slope_degrees([[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 1.0]]);
        assert!((s - 45.0).abs() < 1e-9, "{s}");
        assert_eq!(
            slope_degrees([[0.0, 0.0, 3.0], [1.0, 0.0, 3.0], [0.0, 1.0, 3.0]]),
            0.0
        );
    }

    #[test]
    fn a_ramp_is_one_chart_in_the_plane_it_looks_along_and_each_triangle_is_in_one_place() {
        // rising 6 m a metre along x (80.5 degrees) between x = 2 and 5; flat either side
        let ramp = |x: f64, _y: f64| 6.0 * (x.clamp(2.0, 5.0) - 2.0);
        let (points, tris) = grid(16, 8, ramp);
        let split = split(&|v| points[v as usize], &tris, 60.0, 0.25, 4096).expect("steep");
        let steep: Vec<usize> = (0..tris.len()).filter(|&i| split.steep[i]).collect();
        // 6 squares of the 7 rows: 2 triangles each
        assert_eq!(steep.len(), 6 * 7 * 2);
        assert_eq!(split.charts.len(), 1);
        let chart = &split.charts[0];
        // the ramp rises along +x: its normal points to -x, so the faces look along x and lie in (y, z)
        assert_eq!(chart.plane, Plane::Yz);
        let mut in_chart = chart.triangles.clone();
        in_chart.sort_unstable();
        assert_eq!(in_chart, steep);
        // the extent: 3.5 m of y (seven squares), 18 m of z at 0.25 m, plus the padding
        assert_eq!(chart.size, [14 + 2 * PAD, 72 + 2 * PAD]);
        assert_eq!(split.halvings, 0);
    }

    #[test]
    fn faces_looking_along_y_lie_in_x_z_and_a_ridge_is_two_charts() {
        let (points, tris) = grid(8, 16, |_, y| 6.0 * (y.clamp(2.0, 5.0) - 2.0));
        let s = split(&|v| points[v as usize], &tris, 60.0, 0.25, 4096).expect("steep");
        assert_eq!((s.charts.len(), s.charts[0].plane), (1, Plane::Xz));
        // a ridge: up along x to x = 3 and down after, the two sides share the crest's vertices but look opposite ways
        let (points, tris) = grid(14, 4, |x, _| (6.0 * x.min(6.0 - x)).clamp(0.0, 18.0));
        let s = split(&|v| points[v as usize], &tris, 60.0, 0.25, 4096).expect("steep");
        assert_eq!(
            s.charts.len(),
            2,
            "{:?}",
            s.charts.iter().map(|c| c.triangles.len()).collect::<Vec<_>>()
        );
        assert!(s.charts.iter().all(|c| c.plane == Plane::Yz));
        // the charts' triangles are the steep ones, each in exactly one
        let mut all: Vec<usize> = s
            .charts
            .iter()
            .flat_map(|c| c.triangles.iter().copied())
            .collect();
        all.sort_unstable();
        let steep: Vec<usize> = (0..tris.len()).filter(|&i| s.steep[i]).collect();
        assert_eq!(all, steep);
        // and the atlas holds them apart
        let [a, b] = [&s.charts[0], &s.charts[1]];
        let apart = a.at[0] + a.size[0] <= b.at[0]
            || b.at[0] + b.size[0] <= a.at[0]
            || a.at[1] + a.size[1] <= b.at[1]
            || b.at[1] + b.size[1] <= a.at[1];
        assert!(apart && a.at[0] + a.size[0] <= s.size[0] && b.at[1] + b.size[1] <= s.size[1]);
    }

    #[test]
    fn nothing_steeper_than_the_threshold_has_no_charts() {
        let (points, tris) = grid(8, 8, |x, _| x);
        assert!(split(&|v| points[v as usize], &tris, 60.0, 0.25, 4096).is_none());
        // the same ground at 40 degrees: all steep
        let s = split(&|v| points[v as usize], &tris, 40.0, 0.25, 4096).expect("steep");
        assert!(s.steep.iter().all(|&s| s));
    }

    #[test]
    fn a_texel_maps_back_to_the_point_of_the_surface_it_was_baked_at() {
        let ramp = |x: f64, _y: f64| 6.0 * (x.clamp(2.0, 5.0) - 2.0);
        let (points, tris) = grid(16, 8, ramp);
        let point = |v: u32| points[v as usize];
        let normal = |_: u32| [-0.986, 0.0, 0.164];
        let s = split(&point, &tris, 60.0, 0.25, 4096).expect("steep");
        let chart = &s.charts[0];
        let found = texels(chart, &tris, &point, &normal, s.pixel);
        assert_eq!(found.overlaps, 0);
        let [w, h] = chart.size;
        let mut interior = 0;
        for (idx, sample) in found.samples.iter().enumerate() {
            let (i, j) = (idx as u32 % w, idx as u32 / w);
            let inside = (PAD..w - PAD).contains(&i) && (PAD..h - PAD).contains(&j);
            let Some(sample) = sample else {
                assert!(!inside, "texel ({i}, {j}) of the face has no point");
                continue;
            };
            if inside {
                interior += 1;
                // the texel's centre in the plane, back from the point it maps to, to 1e-9 m
                let q = chart.plane.project(sample.point);
                let want = [
                    chart.min[0] + (f64::from(i) + 0.5 - f64::from(PAD)) * s.pixel,
                    chart.min[1] + (f64::from(j) + 0.5 - f64::from(PAD)) * s.pixel,
                ];
                assert!(
                    (q[0] - want[0]).abs() < 1e-9 && (q[1] - want[1]).abs() < 1e-9,
                    "({i}, {j}) {q:?} {want:?}"
                );
                // and the point is on the ramp's surface
                assert!((sample.point[2] - ramp(sample.point[0], sample.point[1])).abs() < 1e-9);
                // the atlas coordinates of the point are the texel's centre
                let uv = uv_of(chart, s.size, s.pixel, sample.point);
                let at = [
                    (f64::from(chart.at[0] + i) + 0.5) / f64::from(s.size[0]),
                    (f64::from(chart.at[1] + j) + 0.5) / f64::from(s.size[1]),
                ];
                assert!((f64::from(uv[0]) - at[0]).abs() < 1e-6 && (f64::from(uv[1]) - at[1]).abs() < 1e-6);
            }
        }
        assert_eq!(interior, (w - 2 * PAD) * (h - 2 * PAD));
        // the padding takes the nearest face's edge: a texel just outside the face has the point of its edge
        let corner = found.samples[(PAD - 1) as usize * w as usize + (PAD - 1) as usize].expect("padding");
        assert!(corner.point[2].is_finite());
    }

    #[test]
    fn a_chart_wider_than_the_atlas_is_built_at_twice_the_pixel_until_it_fits() {
        // a face 1 m wide and 1,000 m tall: 10,000 pixels at 0.1 m, 5,000 at 0.2, 2,500 at 0.4
        let (points, tris) = grid(3, 3, |x, _| 1000.0 * x);
        let s = split(&|v| points[v as usize], &tris, 60.0, 0.1, 4096).expect("steep");
        assert_eq!((s.halvings, s.pixel), (2, 0.4));
        assert!(s.size[0] <= 4096 && s.size[1] <= 4096);
    }

    #[test]
    fn shelf_packing_keeps_the_charts_apart() {
        let sizes = [[100, 50], [60, 200], [300, 10], [40, 40], [40, 40], [250, 60]];
        let mut charts: Vec<Chart> = sizes
            .iter()
            .map(|&size| Chart {
                plane: Plane::Xz,
                triangles: Vec::new(),
                min: [0.0; 2],
                size,
                at: [0, 0],
            })
            .collect();
        let [aw, ah] = pack(&mut charts, 4096).expect("fits");
        for (i, a) in charts.iter().enumerate() {
            assert!(a.at[0] + a.size[0] <= aw && a.at[1] + a.size[1] <= ah);
            for b in &charts[i + 1..] {
                let apart = a.at[0] + a.size[0] <= b.at[0]
                    || b.at[0] + b.size[0] <= a.at[0]
                    || a.at[1] + a.size[1] <= b.at[1]
                    || b.at[1] + b.size[1] <= a.at[1];
                assert!(apart, "{a:?} {b:?}");
            }
        }
    }
}
