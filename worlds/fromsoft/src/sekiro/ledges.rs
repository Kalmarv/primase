//! Ledge chaining is a lowering policy: one-centimetre endpoint distance, one-metre
//! minimum length and wood/concrete selected from game-map collision only.
use crate::{Result, invalid};
use primase_skate_scene::{SkateGrindCurve, SkateSoup};
use std::collections::{BTreeMap, VecDeque};
fn key(p: [f32; 3]) -> [i64; 3] {
    p.map(|x| (f64::from(x) * 100.).floor() as i64)
}
fn nearby(a: [f32; 3], b: [f32; 3]) -> bool {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum::<f64>()
        <= 0.01f64.powi(2)
}
#[derive(Debug)]
pub struct GameSurfaceIndex {
    triangles: Vec<([[f64; 3]; 3], u8)>,
    cells: BTreeMap<[i64; 2], Vec<usize>>,
}
impl GameSurfaceIndex {
    /// The caller supplies map soups, before adding object collision.
    pub fn new(soups: &[&SkateSoup]) -> Result<Self> {
        let mut out = Self {
            triangles: vec![],
            cells: BTreeMap::new(),
        };
        for soup in soups {
            for (i, t) in soup.triangles.iter().enumerate() {
                let mut points = [[0.; 3]; 3];
                for (p, &index) in points.iter_mut().zip(t) {
                    *p = soup
                        .positions
                        .get(index as usize)
                        .ok_or_else(|| invalid("ledge surface", i, "triangle index"))?
                        .map(f64::from);
                }
                if points.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid("ledge surface", i, "nonfinite triangle"));
                }
                let surface = *soup
                    .surfaces
                    .get(i)
                    .ok_or_else(|| invalid("ledge surface", i, "missing material"))?;
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
                    return Err(invalid("ledge surface", i, "triangle spans too many index cells"));
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
    pub fn wood_at(&self, p: [f32; 3]) -> bool {
        let cell = [
            (f64::from(p[0]) / 2.).floor() as i64,
            (f64::from(p[2]) / 2.).floor() as i64,
        ];
        let mut best = f64::INFINITY;
        let mut wood = false;
        if let Some(candidates) = self.cells.get(&cell) {
            for &i in candidates {
                let (t, surface) = self.triangles[i];
                let d = primase_meshops::point_triangle_distance_squared(p.map(f64::from), t);
                if d < best {
                    best = d;
                    wood = surface == 4;
                }
            }
        }
        wood
    }
}
/// Input segments and region are already in the scene frame. Stable first-use
/// chaining resolves branches by original segment order, retaining all segments.
pub fn chain(
    segments: &[[[f32; 3]; 2]],
    minimum: f64,
    inside: impl Fn([f32; 3]) -> bool,
    surface: &GameSurfaceIndex,
) -> Result<Vec<SkateGrindCurve>> {
    if !minimum.is_finite()
        || minimum < 0.
        || segments
            .iter()
            .flatten()
            .flatten()
            .any(|v| !v.is_finite() || f64::from(v.abs()) > 1e16)
    {
        return Err(invalid("ledge policy", 0, "nonfinite segment or invalid minimum"));
    }
    let mut ends = BTreeMap::<[i64; 3], Vec<usize>>::new();
    for (i, s) in segments.iter().enumerate() {
        for &p in s {
            ends.entry(key(p)).or_default().push(i);
        }
    }
    let mut used = vec![false; segments.len()];
    let mut curves = vec![];
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
        if length < minimum || !points.iter().any(|&p| inside(p)) {
            continue;
        }
        let material = if surface.wood_at(points[points.len() / 2]) {
            4
        } else {
            3
        };
        let closed =
            points.len() > 3 && nearby(points[0], *points.last().expect("seed segment has two endpoints"));
        if closed {
            points.pop();
        }
        curves.push(SkateGrindCurve {
            points,
            closed,
            kind: 1,
            radius: 0.03,
            surface: material,
            unknown_flags: 0,
        });
    }
    Ok(curves)
}
