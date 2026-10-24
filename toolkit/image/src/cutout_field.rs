//! The alpha field of a cut-out texture and the clipping of triangles against the region of it that is drawn
//! (`clip-alpha-cutouts`' mechanism; the pass is in `cutouts.rs`).
//!
//! **The field.** The alpha a draw tests is the bilinear filter of the texels' alpha between texel centres, times the
//! material's base-colour alpha. A grid of nodes at texel centres carries it (the texels as they are, or the box mean
//! of the texels each node stands for when the texture is larger than the stated resolution); beyond the grid a node
//! is read with the material's wrap (the texel across for `Repeat`, the edge texel otherwise). A node is kept when
//! its alpha is at or above the cutoff.
//!
//! **The region.** Each cell between four nodes holds the part of the cell where the alpha, interpolated linearly
//! along the cell's edges, is at or above the cutoff (marching squares): on a cell's edges that is exactly the
//! bilinear field's contour, inside the cell the contour is a chord where the field's is a hyperbola (the error is
//! sub-texel). The kept part of a cell is one convex polygon, or two when only opposite corners are kept and the
//! cell's mean alpha is below the cutoff: cutting corners off a square leaves a convex polygon. A cell with all four
//! corners kept is the whole square, and runs of those are merged into rectangles (rows first, runs of equal extent
//! joined vertically) so a solid area is a few large pieces and only the contour is fine.
//!
//! **Clipping.** A triangle in UV space is clipped (Sutherland-Hodgman, subject order kept, so winding is kept)
//! against each rectangle and each partial cell it overlaps; a triangle that overlaps only whole cells is kept as it
//! is, one that overlaps none is dropped. Pieces are points of the source triangle ([`Bary`](primase_meshops::point_blend::Bary));
//! a cut on a cell's side is computed from the cut edge's ends alone, as `split-uv-tiles` computes the cut at an
//! integer line, so the pieces either side of a cell side and two triangles sharing an edge get the same point.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pbr::Wrap;
use primase_meshops::point_blend::{Corner, tidy};

use crate::sampling::Address;

/// A cell's relation to the cut-out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Empty,
    Full,
    Partial,
}

/// Distances below this in UV (a trillionth of the texture) are on the line.
const EPS: f64 = 1e-12;

/// What clipping one triangle left.
pub(crate) enum Clip {
    /// Every part of the triangle is drawn.
    Whole,
    /// None of it is.
    Nothing,
    /// These convex pieces, in the triangle's winding.
    Pieces(Vec<Vec<Corner>>),
}

pub(crate) struct Field {
    w: i64,
    h: i64,
    /// The alpha at the nodes, row by row.
    values: Vec<f32>,
    cutoff: f32,
    address: [Address; 2],
    /// The cells `-1..w` by `-1..h`.
    kinds: Vec<Kind>,
    /// The rectangle a whole cell belongs to.
    rect_of: Vec<u32>,
    /// Merged rectangles of whole cells: u0, v0, u1, v1.
    rects: Vec<[f64; 4]>,
}

/// The alpha a texel contributes: the material's base-colour alpha times the texture's (8-bit, as a float).
pub(crate) fn effective(factor: f32, alpha: f32) -> f32 {
    factor * alpha / 255.0
}

/// How an axis reads outside the texture: only `Repeat` reads the texel across; `Mirror` and `Clamp` read the edge
/// texel (a mirrored texel beside the edge is the edge texel).
pub(crate) fn address(wrap: Wrap) -> Address {
    match wrap {
        Wrap::Repeat => Address::Wrap,
        Wrap::Clamp | Wrap::Mirror => Address::Clamp,
    }
}

/// The box mean of the 8-bit alpha `alpha` (`width` by `height`) in a grid of at most `resolution` on its longer
/// side: the texels themselves when the texture is not larger. Each grid texel is the mean of the source texels in
/// its bin (`x * width / w` up to `(x + 1) * width / w`, at least one texel).
pub(crate) fn downsample(
    alpha: &[u8],
    width: usize,
    height: usize,
    resolution: usize,
) -> (usize, usize, Vec<f32>) {
    let longer = width.max(height);
    if longer <= resolution {
        return (width, height, alpha.iter().map(|&a| f32::from(a)).collect());
    }
    let w = (width * resolution / longer).max(1);
    let h = (height * resolution / longer).max(1);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let (ya, yb) = (y * height / h, ((y + 1) * height / h).max(y * height / h + 1));
        for x in 0..w {
            let (xa, xb) = (x * width / w, ((x + 1) * width / w).max(x * width / w + 1));
            let mut sum = 0u64;
            for sy in ya..yb {
                for sx in xa..xb {
                    sum += u64::from(alpha[sy * width + sx]);
                }
            }
            out.push(sum as f32 / ((yb - ya) * (xb - xa)) as f32);
        }
    }
    (w, h, out)
}

impl Field {
    /// A field of `w` by `h` nodes holding `values` (effective alpha, row by row).
    pub(crate) fn new(w: usize, h: usize, values: Vec<f32>, cutoff: f32, address: [Address; 2]) -> Field {
        debug_assert_eq!(values.len(), w * h);
        let mut f = Field {
            w: w as i64,
            h: h as i64,
            values,
            cutoff,
            address,
            kinds: Vec::new(),
            rect_of: Vec::new(),
            rects: Vec::new(),
        };
        f.classify();
        f
    }

    fn node(&self, i: i64, j: i64) -> f32 {
        // cells and samples reach one node beyond the grid, never two: there a mirrored node is the edge node, the
        // clamped one (D3D mirrors -2 to 1, which this would get wrong)
        debug_assert!(
            (-1..=self.w).contains(&i) && (-1..=self.h).contains(&j),
            "node ({i}, {j})"
        );
        let ix = match self.address[0] {
            Address::Wrap => i.rem_euclid(self.w),
            // clamp, and mirror (a mirrored node beside the edge is the edge node)
            _ => i.clamp(0, self.w - 1),
        };
        let iy = match self.address[1] {
            Address::Wrap => j.rem_euclid(self.h),
            _ => j.clamp(0, self.h - 1),
        };
        self.values[(iy * self.w + ix) as usize]
    }

    fn kept(&self, i: i64, j: i64) -> bool {
        self.node(i, j) >= self.cutoff
    }

    fn cell(&self, i: i64, j: i64) -> usize {
        ((j + 1) * (self.w + 1) + (i + 1)) as usize
    }

    /// The UV of node `(i, j)`.
    fn at(&self, i: i64, j: i64) -> [f64; 2] {
        [(i as f64 + 0.5) / self.w as f64, (j as f64 + 0.5) / self.h as f64]
    }

    fn classify(&mut self) {
        let cells = ((self.w + 1) * (self.h + 1)) as usize;
        self.kinds = vec![Kind::Empty; cells];
        self.rect_of = vec![u32::MAX; cells];
        for j in -1..self.h {
            for i in -1..self.w {
                let n = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)]
                    .into_iter()
                    .filter(|&(a, b)| self.kept(a, b))
                    .count();
                let c = self.cell(i, j);
                self.kinds[c] = match n {
                    0 => Kind::Empty,
                    4 => Kind::Full,
                    _ => Kind::Partial,
                };
            }
        }
        // runs of whole cells in a row, joined to the run of equal extent in the row above
        let mut active: BTreeMap<(i64, i64), i64> = BTreeMap::new();
        for j in -1..=self.h {
            let mut current = BTreeMap::new();
            if j < self.h {
                let mut i = -1;
                while i < self.w {
                    if self.kinds[self.cell(i, j)] != Kind::Full {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i + 1 < self.w && self.kinds[self.cell(i + 1, j)] == Kind::Full {
                        i += 1;
                    }
                    let run = (start, i);
                    current.insert(run, active.remove(&run).unwrap_or(j));
                    i += 1;
                }
            }
            // the runs that do not continue into row j end at row j - 1
            for ((i0, i1), j0) in std::mem::replace(&mut active, current) {
                let id = self.rects.len() as u32;
                let (a, b) = (self.at(i0, j0), self.at(i1 + 1, j));
                self.rects.push([a[0], a[1], b[0], b[1]]);
                for y in j0..j {
                    for x in i0..=i1 {
                        let c = self.cell(x, y);
                        self.rect_of[c] = id;
                    }
                }
            }
        }
    }

    /// The convex polygons (counterclockwise) of cell `(i, j)` that are kept.
    fn polygons(&self, i: i64, j: i64) -> Vec<Vec<[f64; 2]>> {
        let nodes = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
        let value = nodes.map(|(a, b)| self.node(a, b));
        let kept = value.map(|v| v >= self.cutoff);
        let point = nodes.map(|(a, b)| self.at(a, b));
        // where the contour crosses the edge from corner m to the next; a coordinate both ends share stays exact
        let cross = |m: usize| {
            let n = (m + 1) % 4;
            let t = f64::from((self.cutoff - value[m]) / (value[n] - value[m]));
            [0, 1].map(|k| point[m][k] + (point[n][k] - point[m][k]) * t)
        };
        let saddle = kept[0] == kept[2] && kept[1] == kept[3] && kept[0] != kept[1];
        if saddle && value.iter().sum::<f32>() / 4.0 < self.cutoff {
            // two corners apart: the triangle at each kept corner
            return (0..4)
                .filter(|&m| kept[m])
                .map(|m| vec![cross((m + 3) % 4), point[m], cross(m)])
                .collect();
        }
        let mut poly = Vec::new();
        for m in 0..4 {
            if kept[m] {
                poly.push(point[m]);
            }
            if kept[m] != kept[(m + 1) % 4] {
                poly.push(cross(m));
            }
        }
        if poly.len() < 3 { Vec::new() } else { vec![poly] }
    }

    /// The alpha at `uv`, bilinear between node centres.
    fn sample(&self, uv: [f64; 2]) -> f32 {
        let x = uv[0] * self.w as f64 - 0.5;
        let y = uv[1] * self.h as f64 - 0.5;
        let (fx, fy) = (x.floor(), y.floor());
        let (tx, ty) = ((x - fx) as f32, (y - fy) as f32);
        let (i, j) = (fx as i64, fy as i64);
        let top = self.node(i, j) * (1.0 - tx) + self.node(i + 1, j) * tx;
        let bottom = self.node(i, j + 1) * (1.0 - tx) + self.node(i + 1, j + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// Clip a triangle of three source corners against the region.
    pub(crate) fn clip(&self, tri: [Corner; 3]) -> Clip {
        let uv = tri.map(|c| c.uv);
        let area2 =
            (uv[1][0] - uv[0][0]) * (uv[2][1] - uv[0][1]) - (uv[1][1] - uv[0][1]) * (uv[2][0] - uv[0][0]);
        if area2.abs() < 1e-18 {
            // no area in the texture: it samples one place
            let centre = [0, 1].map(|k| (uv[0][k] + uv[1][k] + uv[2][k]) / 3.0);
            return if self.sample(centre) >= self.cutoff {
                Clip::Whole
            } else {
                Clip::Nothing
            };
        }
        let range = |k: usize, n: i64| {
            let lo = uv.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min);
            let hi = uv.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max);
            let first = ((lo * n as f64 - 0.5).floor() as i64).clamp(-1, n - 1);
            let last = ((hi * n as f64 - 0.5).floor() as i64).clamp(-1, n - 1);
            (first, last)
        };
        let ((i0, i1), (j0, j1)) = (range(0, self.w), range(1, self.h));
        let mut empty_seen = false;
        let mut rects = BTreeSet::new();
        let mut partial = Vec::new();
        for j in j0..=j1 {
            for i in i0..=i1 {
                if !touches(&uv, self.at(i, j), self.at(i + 1, j + 1)) {
                    continue;
                }
                match self.kinds[self.cell(i, j)] {
                    Kind::Empty => empty_seen = true,
                    Kind::Full => {
                        rects.insert(self.rect_of[self.cell(i, j)]);
                    }
                    Kind::Partial => partial.push((i, j)),
                }
            }
        }
        if rects.is_empty() && partial.is_empty() {
            return Clip::Nothing;
        }
        if !empty_seen && partial.is_empty() {
            return Clip::Whole;
        }
        let min_area = 1e-9 / (self.w * self.h) as f64;
        let mut regions: Vec<Vec<[f64; 2]>> = rects
            .into_iter()
            .map(|r| {
                let [u0, v0, u1, v1] = self.rects[r as usize];
                vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
            })
            .collect();
        for (i, j) in partial {
            regions.extend(self.polygons(i, j));
        }
        let mut pieces = Vec::new();
        for region in regions {
            if let Some(piece) = clip_convex(tri.to_vec(), &region)
                && polygon_area(&piece).abs() >= min_area
            {
                pieces.push(piece);
            }
        }
        if pieces.is_empty() {
            Clip::Nothing
        } else {
            Clip::Pieces(pieces)
        }
    }
}

/// The area of a polygon of corners in UV (signed: positive counterclockwise).
pub(crate) fn polygon_area(p: &[Corner]) -> f64 {
    let mut s = 0.0;
    for (k, a) in p.iter().enumerate() {
        let b = &p[(k + 1) % p.len()];
        s += a.uv[0] * b.uv[1] - b.uv[0] * a.uv[1];
    }
    s / 2.0
}

/// Whether triangle `uv` and the rectangle from `lo` to `hi` overlap in more than a point or a line (a separating
/// axis test on the rectangle's sides and the triangle's edges).
fn touches(uv: &[[f64; 2]; 3], lo: [f64; 2], hi: [f64; 2]) -> bool {
    for k in 0..2 {
        let (tmin, tmax) = uv.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |m, p| {
            (m.0.min(p[k]), m.1.max(p[k]))
        });
        if tmax <= lo[k] + EPS || tmin >= hi[k] - EPS {
            return false;
        }
    }
    let corners = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
    for k in 0..3 {
        let (a, b) = (uv[k], uv[(k + 1) % 3]);
        let n = [b[1] - a[1], a[0] - b[0]];
        let len = n[0].hypot(n[1]);
        if len < 1e-15 {
            continue;
        }
        let along = |p: [f64; 2]| (n[0] * (p[0] - a[0]) + n[1] * (p[1] - a[1])) / len;
        // the triangle is on its third corner's side of this edge; the rectangle must reach past the edge toward it
        let toward = along(uv[(k + 2) % 3]).signum();
        let reach = corners
            .iter()
            .map(|&p| along(p) * toward)
            .fold(f64::NEG_INFINITY, f64::max);
        if reach <= EPS {
            return false;
        }
    }
    true
}

/// `subject` (a convex polygon of corners, in order) clipped to the convex polygon `clip` (counterclockwise): the
/// part inside, in the subject's order. `None` when no polygon with area is left.
fn clip_convex(subject: Vec<Corner>, clip: &[[f64; 2]]) -> Option<Vec<Corner>> {
    let mut poly = subject;
    for k in 0..clip.len() {
        let (p, q) = (clip[k], clip[(k + 1) % clip.len()]);
        // The signed distance inside the edge p->q (to its left), and where along an edge the cut falls: on a cell's
        // side it is the exact expression the tile splitter uses, on a chord the ratio of the ends' distances.
        let vertical = p[0] == q[0];
        let horizontal = p[1] == q[1];
        let e = [q[0] - p[0], q[1] - p[1]];
        let len = e[0].hypot(e[1]);
        let distance = move |c: &Corner| -> f64 {
            if vertical {
                e[1].signum() * (p[0] - c.uv[0])
            } else if horizontal {
                e[0].signum() * (c.uv[1] - p[1])
            } else {
                (e[0] * (c.uv[1] - p[1]) - e[1] * (c.uv[0] - p[0])) / len
            }
        };
        let along = move |a: &Corner, b: &Corner| -> f64 {
            if vertical {
                (p[0] - a.uv[0]) / (b.uv[0] - a.uv[0])
            } else if horizontal {
                (p[1] - a.uv[1]) / (b.uv[1] - a.uv[1])
            } else {
                distance(a) / (distance(a) - distance(b))
            }
        };
        let mut out = Vec::with_capacity(poly.len() + 1);
        for (n, cur) in poly.iter().enumerate() {
            let next = &poly[(n + 1) % poly.len()];
            let (sc, sn) = (distance(cur), distance(next));
            if sc >= -EPS {
                out.push(*cur);
            }
            if (sc > EPS && sn < -EPS) || (sc < -EPS && sn > EPS) {
                let mut c = Corner::cut_at(cur, next, along);
                // a cut on a cell's side lies exactly on it
                if vertical {
                    c.uv[0] = p[0];
                } else if horizontal {
                    c.uv[1] = p[1];
                }
                out.push(c);
            }
        }
        poly = tidy(out)?;
    }
    Some(poly)
}
