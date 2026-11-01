//! Where one triangle lies over another: the geometry under `drop-covered-surfaces` and any other operation that asks
//! which surfaces are layered on which (a gate measuring how far a layer floats over its base, a pass cutting what a
//! surface in front covers).
//!
//! **Triangles** ([`Tri`]) are placed (world frame, f64) with their unit normal; a degenerate or non-finite one is
//! refused when made. **The index** ([`Index`]) lists every triangle in the cells of a cubic hash its bounding box,
//! padded by a reach, touches; [`Index::near`] gives the triangles sharing a cell with one, sorted and without
//! repeats, so a caller's walk is in a fixed order. A triangle whose padded box would span more than [`MAX_CELLS`]
//! cells (a far terrain's) is not listed and finds nothing ([`Index::unlisted`] counts them). **The box tree**
//! ([`Boxes`]) answers exactly what a cell answers loosely: [`Boxes::within`] gives the triangles whose bounding boxes
//! come within a distance of a box, so a walk over a dense mesh visits a triangle's neighbours, not all of its cells.
//!
//! **Over** ([`Over::of`]): for a triangle `t` and another `u` facing the same way (unit normals' dot at least
//! `facing`), the part of `t` that `u` covers when projected along `t`'s normal, as a convex polygon in `t`'s own
//! coordinates ([`Uv`]: `s` along its first edge, `t` along its second), and the signed distance from `t`'s plane to
//! `u`'s along `t`'s normal (positive: `u` in front) as an affine function of those coordinates ([`Over::along`]).
//! The distance is measured along `t`'s normal and divided by the normals' dot (the ray from `t` along its normal
//! meets `u`'s plane there). [`Over::band`] is the part where that distance lies in a range, [`Over::range`] its least
//! and greatest over the whole overlap (an affine function is extreme at a corner of the polygon).
//!
//! Areas are in `t`'s coordinates unless they say square metres ([`Frame::scale`] converts: the triangle itself is
//! 0.5 in its own coordinates).

use std::collections::HashMap;

/// The most cells one triangle is listed in.
pub const MAX_CELLS: i64 = 1 << 16;

/// A point in a triangle's own coordinates: `s` along its first edge (corner 0 to 1), `t` along its second (0 to 2).
pub type Uv = [f64; 2];
/// A convex polygon in a triangle's own coordinates, counter-clockwise (the triangle's winding).
pub type Poly = Vec<Uv>;
/// An affine function of a triangle's coordinates, `a s + b t + c`, as `[a, b, c]`: a half-plane where it is >= 0.
pub type Affine = [f64; 3];

pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `f` at `p`.
pub fn value(f: Affine, p: Uv) -> f64 {
    f[0] * p[0] + f[1] * p[1] + f[2]
}

/// The signed area, in the triangle's coordinates (the triangle itself is 0.5).
pub fn area(p: &[Uv]) -> f64 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// The triangle's own edges as half-planes: `s >= 0`, `t >= 0`, `s + t <= 1`.
pub const INSIDE: [Affine; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, -1.0, 1.0]];

/// The half-planes whose intersection is `p` (convex, counter-clockwise): each edge's left side.
pub fn edges(p: &[Uv]) -> impl Iterator<Item = Affine> + '_ {
    (0..p.len()).map(|i| {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        [-dy, dx, dy * a[0] - dx * a[1]]
    })
}

/// `p` where `f >= 0` (Sutherland-Hodgman); fewer than three points when nothing is left.
pub fn clip(p: &[Uv], f: Affine) -> Poly {
    let mut out = Vec::with_capacity(p.len() + 1);
    for (i, &cur) in p.iter().enumerate() {
        let next = p[(i + 1) % p.len()];
        let (c, d) = (value(f, cur), value(f, next));
        if c >= 0.0 {
            out.push(cur);
        }
        if (c < 0.0 && d > 0.0) || (c > 0.0 && d < 0.0) {
            let t = c / (c - d);
            out.push([cur[0] + t * (next[0] - cur[0]), cur[1] + t * (next[1] - cur[1])]);
        }
    }
    out
}

/// `p` cut to every half-plane of `fs`; `None` when under `min` in area.
pub fn clip_all(p: &[Uv], fs: impl IntoIterator<Item = Affine>, min: f64) -> Option<Poly> {
    let mut p = p.to_vec();
    for f in fs {
        p = clip(&p, f);
        if p.len() < 3 {
            return None;
        }
    }
    (area(&p) >= min).then_some(p)
}

/// `p` less the convex polygon `c`: the convex pieces outside each edge of `c` in turn, those of at least `min` in
/// area, pushed to `out`; `p` whole when the two do not overlap by `min`.
pub fn subtract(p: Poly, c: &[Uv], min: f64, out: &mut Vec<Poly>) {
    if clip_all(&p, edges(c), min).is_none() {
        out.push(p);
        return;
    }
    let mut rest = Some(p);
    for f in edges(c) {
        let Some(r) = rest.take() else { break };
        out.extend(clip_all(&r, [[-f[0], -f[1], -f[2]]], min));
        let inside = clip(&r, f);
        rest = (inside.len() >= 3).then_some(inside);
    }
}

/// `pieces` as disjoint convex polygons with the same union (each less the ones before it); `None` past `max`
/// pieces.
pub fn disjoint(pieces: Vec<Poly>, min: f64, max: usize) -> Option<Vec<Poly>> {
    let mut out: Vec<Poly> = Vec::new();
    for p in pieces {
        let mut frags = vec![p];
        for q in &out {
            let mut next = Vec::with_capacity(frags.len());
            for f in frags {
                subtract(f, q, min, &mut next);
            }
            frags = next;
            if frags.is_empty() {
                break;
            }
        }
        out.extend(frags);
        if out.len() > max {
            return None;
        }
    }
    Some(out)
}

/// A placed triangle: corners in the world frame and its unit normal (the corners' winding).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub c: [[f64; 3]; 3],
    pub n: [f64; 3],
}

impl Tri {
    /// `None` for a degenerate triangle (no normal) or a non-finite corner.
    pub fn new(c: [[f64; 3]; 3]) -> Option<Tri> {
        let nn = cross(sub(c[1], c[0]), sub(c[2], c[0]));
        let len = dot(nn, nn).sqrt();
        if len <= 1e-12 || !len.is_finite() || !c.iter().flatten().all(|x| x.is_finite()) {
            return None;
        }
        Some(Tri {
            c,
            n: nn.map(|x| x / len),
        })
    }

    /// Its area, square metres.
    pub fn area_m2(&self) -> f64 {
        Frame::of(self).scale / 2.0
    }
}

/// A triangle's own coordinates: the map from world points to them and the scale of their areas.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    origin: [f64; 3],
    e1: [f64; 3],
    e2: [f64; 3],
    normal: [f64; 3],
    nn: f64,
    /// Square metres per unit of area in the triangle's coordinates (twice the triangle's area).
    pub scale: f64,
}

impl Frame {
    pub fn of(t: &Tri) -> Frame {
        let (e1, e2) = (sub(t.c[1], t.c[0]), sub(t.c[2], t.c[0]));
        let normal = cross(e1, e2);
        let nn = dot(normal, normal);
        Frame {
            origin: t.c[0],
            e1,
            e2,
            normal,
            nn,
            scale: nn.sqrt(),
        }
    }

    /// `x` projected along the normal onto the triangle's plane, in its coordinates.
    pub fn uv(&self, x: [f64; 3]) -> Uv {
        let d = sub(x, self.origin);
        [
            dot(cross(d, self.e2), self.normal) / self.nn,
            dot(cross(self.e1, d), self.normal) / self.nn,
        ]
    }

    /// The world point at `p` on the triangle's plane.
    pub fn point(&self, p: Uv) -> [f64; 3] {
        std::array::from_fn(|a| self.origin[a] + p[0] * self.e1[a] + p[1] * self.e2[a])
    }
}

/// The part of a triangle `t` another triangle `u` lies over (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct Over {
    /// `u` projected onto `t` along `t`'s normal, cut to `t`: convex, counter-clockwise, in `t`'s coordinates.
    pub poly: Poly,
    /// The signed distance from `t`'s plane to `u`'s along `t`'s normal, metres, as a function of `t`'s coordinates.
    pub along: Affine,
}

impl Over {
    /// The signed distances from `t`'s plane to `u`'s at `t`'s three corners (no overlap is computed): the cheap
    /// test before [`Over::of`]. `None` when `u` does not face the way `t` does within `facing` (the least dot of the
    /// two unit normals).
    pub fn corner_distances(t: &Tri, u: &Tri, facing: f64) -> Option<[f64; 3]> {
        let f = dot(u.n, t.n);
        (f >= facing).then(|| std::array::from_fn(|k| dot(u.n, sub(u.c[0], t.c[k])) / f))
    }

    /// What `u` lies over of `t`, given `t`'s frame and the distances [`Over::corner_distances`] gave; `None` when
    /// the overlap is under `min` in area (`t`'s coordinates).
    pub fn of(frame: &Frame, u: &Tri, d: [f64; 3], min: f64) -> Option<Over> {
        let mut shadow: Poly = u.c.iter().map(|&x| frame.uv(x)).collect();
        if area(&shadow) < 0.0 {
            shadow.reverse();
        }
        let poly = clip_all(&shadow, INSIDE, min)?;
        Some(Over {
            poly,
            along: [d[1] - d[0], d[2] - d[0], d[0]],
        })
    }

    /// The part of the overlap where the distance lies in `lo..=hi`; `None` when under `min` in area.
    pub fn band(&self, lo: f64, hi: f64, min: f64) -> Option<Poly> {
        let a = self.along;
        clip_all(
            &self.poly,
            [[a[0], a[1], a[2] - lo], [-a[0], -a[1], hi - a[2]]],
            min,
        )
    }

    /// The least and greatest distance over the overlap.
    pub fn range(&self) -> (f64, f64) {
        self.poly
            .iter()
            .map(|&p| value(self.along, p))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v), hi.max(v))
            })
    }
}

/// The triangles of a set by the cells of a cubic hash their padded bounding boxes touch.
#[derive(Clone, Debug)]
pub struct Index {
    cell: f64,
    reach: f64,
    grid: HashMap<(i64, i64, i64), Vec<u32>>,
    listed: Vec<bool>,
}

impl Index {
    /// Every triangle of `tris` in the cells (of side `cell` metres) its box padded by `reach` metres touches.
    pub fn new(tris: &[Tri], cell: f64, reach: f64) -> Index {
        let mut ix = Index {
            cell,
            reach,
            grid: HashMap::new(),
            listed: vec![false; tris.len()],
        };
        for (i, t) in tris.iter().enumerate() {
            if !Index::lists(t, cell, reach) {
                continue;
            }
            let (lo, hi) = ix.cells(t);
            ix.listed[i] = true;
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        ix.grid.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        ix
    }

    /// Whether an index of cells of side `cell` metres, boxes padded by `reach` metres, lists `t`: its padded box
    /// spans at most [`MAX_CELLS`] cells.
    pub fn lists(t: &Tri, cell: f64, reach: f64) -> bool {
        let (lo, hi) = cells_of(t, cell, reach);
        (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) * (hi[2] - lo[2] + 1) <= MAX_CELLS
    }

    fn cells(&self, t: &Tri) -> ([i64; 3], [i64; 3]) {
        cells_of(t, self.cell, self.reach)
    }

    /// The triangles sharing a cell with triangle `i` of `tris` (itself included), sorted, each once, into `out`
    /// (cleared first); nothing for an unlisted one.
    pub fn near(&self, tris: &[Tri], i: usize, out: &mut Vec<u32>) {
        out.clear();
        if !self.listed[i] {
            return;
        }
        let (lo, hi) = self.cells(&tris[i]);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    out.extend(self.grid.get(&(x, y, z)).map(Vec::as_slice).unwrap_or(&[]));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Whether triangle `i` was listed (not too big).
    pub fn listed(&self, i: usize) -> bool {
        self.listed[i]
    }

    /// How many triangles were too big to list.
    pub fn unlisted(&self) -> u64 {
        self.listed.iter().filter(|l| !**l).count() as u64
    }
}

/// The cells (of side `cell` metres) `t`'s box padded by `reach` metres touches: least and greatest.
fn cells_of(t: &Tri, cell: f64, reach: f64) -> ([i64; 3], [i64; 3]) {
    let at = |x: f64| (x / cell).floor() as i64;
    let (lo, hi) = bounds(t);
    (lo.map(|x| at(x - reach)), hi.map(|x| at(x + reach)))
}

/// A triangle's bounding box: its least and greatest corner.
pub fn bounds(t: &Tri) -> ([f64; 3], [f64; 3]) {
    (
        std::array::from_fn(|a| t.c.iter().map(|c| c[a]).fold(f64::INFINITY, f64::min)),
        std::array::from_fn(|a| t.c.iter().map(|c| c[a]).fold(f64::NEG_INFINITY, f64::max)),
    )
}

/// The most triangles a leaf of [`Boxes`] holds.
const LEAF: usize = 8;

/// Some triangles of a set by their bounding boxes, in a tree of boxes: each node's box holds its triangles' boxes,
/// and a node splits its triangles at the median of their boxes' centres along the axis those centres spread widest.
#[derive(Clone, Debug)]
pub struct Boxes {
    nodes: Vec<Node>,
    /// The triangles in leaf order, each with its box.
    items: Vec<(u32, [f64; 3], [f64; 3])>,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    lo: [f64; 3],
    hi: [f64; 3],
    /// A leaf (`count` above 0): its triangles, `items[first..first + count]`. Otherwise its children: the node after
    /// it and node `first`.
    first: u32,
    count: u32,
}

impl Boxes {
    /// The triangles of `tris` that `listed` keeps.
    pub fn new(tris: &[Tri], listed: impl Fn(usize) -> bool) -> Boxes {
        let mut items: Vec<(u32, [f64; 3], [f64; 3])> = (0..tris.len())
            .filter(|&i| listed(i))
            .map(|i| {
                let (lo, hi) = bounds(&tris[i]);
                (i as u32, lo, hi)
            })
            .collect();
        let mut nodes = Vec::with_capacity(2 * items.len() / LEAF + 1);
        if !items.is_empty() {
            build(&mut nodes, &mut items);
        }
        Boxes { nodes, items }
    }

    /// The triangles whose boxes come within `pad` metres of the box `lo..hi` on every axis, sorted, into `out`
    /// (cleared first).
    pub fn within(&self, lo: [f64; 3], hi: [f64; 3], pad: f64, out: &mut Vec<u32>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let (lo, hi) = (lo.map(|x| x - pad), hi.map(|x| x + pad));
        let meets = |a: &[f64; 3], b: &[f64; 3]| (0..3).all(|k| a[k] <= hi[k] && b[k] >= lo[k]);
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if !meets(&node.lo, &node.hi) {
                continue;
            }
            if node.count == 0 {
                stack.push(node.first as usize);
                stack.push(n + 1);
                continue;
            }
            let first = node.first as usize;
            for (i, a, b) in &self.items[first..first + node.count as usize] {
                if meets(a, b) {
                    out.push(*i);
                }
            }
        }
        out.sort_unstable();
    }
}

/// The node of `items` (`items` reordered into leaf order) and its subtree, pushed to `nodes`; its index.
fn build(nodes: &mut Vec<Node>, items: &mut [(u32, [f64; 3], [f64; 3])]) -> usize {
    build_at(nodes, items, 0)
}

fn build_at(nodes: &mut Vec<Node>, items: &mut [(u32, [f64; 3], [f64; 3])], start: usize) -> usize {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    // twice the centres, which order the same
    let (mut clo, mut chi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for (_, a, b) in items.iter() {
        for k in 0..3 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(b[k]);
            clo[k] = clo[k].min(a[k] + b[k]);
            chi[k] = chi[k].max(a[k] + b[k]);
        }
    }
    let at = nodes.len();
    nodes.push(Node {
        lo,
        hi,
        first: start as u32,
        count: items.len() as u32,
    });
    if items.len() <= LEAF {
        return at;
    }
    let axis = (0..3)
        .max_by(|&a, &b| (chi[a] - clo[a]).total_cmp(&(chi[b] - clo[b])))
        .unwrap_or(0);
    let mid = items.len() / 2;
    items.select_nth_unstable_by(mid, |x, y| {
        (x.1[axis] + x.2[axis])
            .total_cmp(&(y.1[axis] + y.2[axis]))
            .then(x.0.cmp(&y.0))
    });
    let (left, right) = items.split_at_mut(mid);
    build_at(nodes, left, start);
    let second = build_at(nodes, right, start + mid);
    nodes[at].first = second as u32;
    nodes[at].count = 0;
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_tri(y: f64) -> Tri {
        // facing +y: corners counter-clockwise seen from above (x, then -z)
        Tri::new([[0.0, y, 0.0], [0.0, y, 1.0], [1.0, y, 0.0]]).unwrap()
    }

    #[test]
    fn a_triangle_four_millimetres_over_another_is_over_all_of_it_at_four_millimetres() {
        let (t, u) = (square_tri(0.0), square_tri(0.004));
        assert!((t.n[1] - 1.0).abs() < 1e-12);
        let d = Over::corner_distances(&t, &u, 0.99).unwrap();
        let o = Over::of(&Frame::of(&t), &u, d, 1e-9).unwrap();
        assert!((area(&o.poly) - 0.5).abs() < 1e-12);
        let (lo, hi) = o.range();
        assert!((lo - 0.004).abs() < 1e-12 && (hi - 0.004).abs() < 1e-12);
        assert!(o.band(0.0, 0.003, 1e-9).is_none());
        assert!(o.band(0.003, 0.005, 1e-9).is_some());
        // seen from the upper one, the lower is behind
        let back = Over::corner_distances(&u, &t, 0.99).unwrap();
        assert!(back.iter().all(|&x| (x + 0.004).abs() < 1e-12));
        // facing away: not over
        let flipped = Tri::new([t.c[0], t.c[2], t.c[1]]).unwrap();
        assert!(Over::corner_distances(&t, &flipped, 0.99).is_none());
    }

    #[test]
    fn a_tilted_cover_is_in_the_band_over_the_part_where_its_height_is() {
        // the cover rises from 0 at x = 0 to 10 mm at x = 1 (within 8 degrees of the base)
        let t = square_tri(0.0);
        let u = Tri::new([[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.01, 0.0]]).unwrap();
        let d = Over::corner_distances(&t, &u, 0.99).unwrap();
        let o = Over::of(&Frame::of(&t), &u, d, 1e-9).unwrap();
        let (lo, hi) = o.range();
        assert!(lo.abs() < 1e-12 && (hi - 0.01).abs() < 1e-9, "{lo} {hi}");
        // where it is at least 5 mm: x >= 0.5 of the triangle (s + t <= 1 with s the z axis here): a quarter of it
        let half = o.band(0.005, 1.0, 1e-9).unwrap();
        assert!((area(&half) - 0.125).abs() < 1e-9, "{}", area(&half));
    }

    #[test]
    fn the_index_finds_neighbours_in_order_and_skips_a_huge_triangle() {
        let tris = vec![
            square_tri(0.0),
            square_tri(0.004),
            Tri::new([[50.0, 0.0, 0.0], [50.0, 0.0, 1.0], [51.0, 0.0, 0.0]]).unwrap(),
            Tri::new([[0.0, 0.0, 0.0], [0.0, 0.0, 1.0e5], [1.0e5, 0.0, 0.0]]).unwrap(),
        ];
        let ix = Index::new(&tris, 1.0, 0.01);
        let mut near = Vec::new();
        ix.near(&tris, 1, &mut near);
        assert_eq!(near, [0, 1]);
        ix.near(&tris, 3, &mut near);
        assert!(near.is_empty());
        assert_eq!(ix.unlisted(), 1);
    }

    #[test]
    fn the_box_tree_finds_exactly_the_boxes_within_the_pad() {
        // a grid of small triangles, some tilted, at several heights, and one long one across them; against every
        // pair by hand
        let mut tris = Vec::new();
        for i in 0..40 {
            for j in 0..40 {
                let (x, z) = (f64::from(i) * 0.37, f64::from(j) * 0.29);
                let y = f64::from((i * 7 + j * 3) % 11) * 0.013;
                let rise = 0.02 * f64::from(i % 3);
                tris.push(Tri::new([[x, y, z], [x, y + rise, z + 0.2], [x + 0.25, y, z]]).unwrap());
            }
        }
        tris.push(Tri::new([[0.0, 0.05, 0.0], [0.0, 0.05, 0.1], [14.0, 0.06, 11.0]]).unwrap());
        let skip = |i: usize| i % 13 == 5;
        let boxes = Boxes::new(&tris, |i| !skip(i));
        let mut out = Vec::new();
        for pad in [0.0, 0.000_45, 0.05, 0.15] {
            for (i, t) in tris.iter().enumerate() {
                let (lo, hi) = bounds(t);
                boxes.within(lo, hi, pad, &mut out);
                let want: Vec<u32> = (0..tris.len())
                    .filter(|&j| !skip(j))
                    .filter(|&j| {
                        let (a, b) = bounds(&tris[j]);
                        (0..3).all(|k| a[k] <= hi[k] + pad && b[k] >= lo[k] - pad)
                    })
                    .map(|j| j as u32)
                    .collect();
                assert_eq!(out, want, "triangle {i}, pad {pad}");
            }
        }
        Boxes::new(&tris, |_| false).within([0.0; 3], [1.0; 3], 1.0, &mut out);
        assert!(out.is_empty());
    }
}
