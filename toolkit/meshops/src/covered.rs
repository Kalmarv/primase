//! `drop-covered-surfaces{gap=M}`: what another opaque surface covers from just in front is left out of an opaque
//! triangle (the whole triangle, or the covered part cut away), for a destination whose depth buffer cannot keep the
//! two apart where the source's did.
//!
//! A source drawn with a precise depth buffer (Riders Republic's reversed float depth) can lay one surface a few
//! millimetres over another and show only the upper: The Deck's bowl decks lie 3.7 mm over the plaza's concrete,
//! 440 m2 of it. A destination with less depth precision draws both, fighting in blotches that move with the view
//! (Caleb in skate., 2026-10-25). What the source showed there is the upper surface alone, so the pass leaves the
//! lower out where the upper covers it.
//!
//! **Which triangles.** Every placed triangle of a triangle-list primitive whose materials (its own and every
//! per-placement variant, [`primase_core::MaterialVariants`]) are all *covering*: a `pbr.material` that is opaque (no
//! alpha test, no blending) and single-sided, not on `core.draw_order`'s decal layer; a `core.layered` blend counts as
//! its base layer's material. A placement is an entity with `core.instance` (its prototype's mesh by its
//! `core.transform`); a mesh no placement takes is drawn by its own transform (none: as stored).
//!
//! **Covered.** A point of a triangle is covered in a placement when the ray from it along the triangle's normal meets
//! a covering triangle of another primitive or placement that faces the same way (normals within 8 degrees) in front,
//! more than 0.1 mm and at most `gap` metres away. Nearer than 0.1 mm the two are the same surface drawn twice (which
//! one shows is the source's draw order): counted and kept. What a cover covers of a triangle is a convex polygon in
//! the triangle's own coordinates: the cover projected along the normal onto the triangle's plane, cut to the
//! triangle and to where the cover's distance (affine over the plane) lies in that range. A triangle of a prototype
//! loses what is covered in every placement of it (the intersection of each placement's covered polygons, each
//! placement's made disjoint first so overlapping covers, a surface drawn twice, do not multiply the pieces); one
//! covered in some placements only keeps that part. Covered pieces under [`MIN_AREA`] are not counted as covering.
//! Double-sided triangles cover others but keep everything (seen from behind they are in front), counted.
//!
//! **Cutting.** A triangle covered whole goes. One covered in part is cut to what is not covered: the covered
//! polygons are subtracted one by one, each subtraction leaving the convex pieces outside the polygon's edges
//! (clipped as `split-uv-tiles` clips, [`crate::point_blend`]), and the pieces fanned back into triangles in the
//! triangle's own winding. Pieces under [`MIN_AREA`] (in every placement) go: slivers along a cover's edge. A triangle
//! whose covered part or cut would be more than [`MAX_PIECES`] pieces is kept whole, counted. A cut leaves no crack: a vertex on an
//! edge two triangles of the primitive share is computed from the edge alone ([`crate::point_blend::Corner::cut_at`]) and
//! welded, one vertex.
//!
//! **What changes.** A primitive whose triangles only go keeps its vertices and loses those triangles' indices
//! (unused vertices are harmless to every writer). A primitive with a cut triangle keeps its vertices in their places
//! and gains the cut's new ones, each a combination of its triangle's corners ([`primase_core::VertexBlend`]) with
//! every stream interpolated ([`crate::point_blend::blend_streams`]: normals renormalised, colours rounded); morph
//! targets and other per-vertex components follow ([`primase_core::World::blend_vertices`]). A mesh whose per-vertex
//! components cannot follow a cut keeps its partly covered triangles whole, counted. A changed mesh's file link goes.
//! Collision is not touched: a covered floor still collides where it did.
//!
//! Reported: the triangles left out whole, and those cut, by material; the triangles the cut ones became; those kept
//! because they lie on a coincident surface, are double-sided, would be too many pieces, or their mesh cannot follow.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::skin::Skin;
use primase_core::{
    DrawLayer, DrawOrder, Entity, Instance, LayeredMaterial, Mesh, MeshFile, Name, Semantic, Stream,
    Topology, Transform, VertexBlend, World,
};

use crate::point_blend::{
    Bary, Corner, Key, Replacement, blend_streams, describe, drawn_with, replace_primitives, tidy,
};

/// Below this distance (metres) a covering surface is the same surface drawn twice.
const COINCIDENT: f64 = 1.0e-4;
/// The least cosine between a covered triangle's normal and its cover's (8 degrees).
const FACING: f64 = 0.99;
/// The hash cell, metres.
const CELL: f64 = 1.0;
/// The most cells one triangle is listed in; a bigger triangle (a far terrain's) covers nothing and is counted.
const MAX_CELLS: i64 = 1 << 16;
/// The least area, square metres, of a covered piece that counts and of a piece a cut keeps: 1 mm2.
pub const MIN_AREA: f64 = 1.0e-6;
/// The most pieces one triangle is cut into; a triangle needing more is kept whole, counted.
pub const MAX_PIECES: usize = 1024;
/// The weld of a cut's new vertices: weights rounded to 2^-20, so the same point reached from both triangles of an
/// edge (in each one's own coordinates) is one vertex.
const WELD_BITS: u32 = 20;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropCoveredSurfaces {
    gap: f64,
}

impl DropCoveredSurfaces {
    /// Covering surfaces at most `gap` metres in front (finite, above 0.1 mm).
    pub fn new(gap: f64) -> Result<DropCoveredSurfaces, String> {
        if !(gap.is_finite() && gap > COINCIDENT) {
            return Err(format!("gap={gap}: finite metres above {COINCIDENT}"));
        }
        Ok(DropCoveredSurfaces { gap })
    }

    pub fn gap(&self) -> f64 {
        self.gap
    }

    fn from_given(g: &Given) -> Result<DropCoveredSurfaces, String> {
        let gap = g.required("gap");
        DropCoveredSurfaces::new(gap.parse().map_err(|_| format!("gap={gap} is not a number"))?)
    }
}

/// `drop-covered-surfaces`' declaration ([`primase_core::pass::PassRegistry::add`]): the gap is the program's, no
/// default (how far apart the source's depth buffer kept surfaces that the destination's will not).
pub const DROP_COVERED_SURFACES: PassSpec = PassSpec {
    name: "drop-covered-surfaces",
    options: &[OptionSpec::required(
        "gap",
        "0.01",
        "metres, finite and above 0.0001: how far in front a covering opaque surface may lie",
    )],
    make: |g| Ok(Box::new(DropCoveredSurfaces::from_given(g)?)),
};

/// What a material is to this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cover {
    /// Opaque and single-sided: covers and may be covered.
    Covering,
    /// Opaque but double-sided: covers, is kept.
    DoubleSided,
    /// Alpha-tested, blended, a decal, or not a PBR material: neither.
    No,
}

fn cover_of(w: &World, m: Entity) -> Cover {
    let base = match w.get::<LayeredMaterial>(m) {
        Some(l) => match l.layers.first().and_then(|l| l.as_ref()) {
            Some(layer) => layer.material,
            None => return Cover::No,
        },
        None => m,
    };
    if DrawOrder::of(w, m).layer == DrawLayer::Decal || DrawOrder::of(w, base).layer == DrawLayer::Decal {
        return Cover::No;
    }
    match w.get::<PbrMaterial>(base) {
        Some(p) if p.alpha_mode == AlphaMode::Opaque => {
            if p.double_sided {
                Cover::DoubleSided
            } else {
                Cover::Covering
            }
        }
        _ => Cover::No,
    }
}

/// A placed triangle: its corners (world frame), unit normal, the placement drawing it, and where it is stored.
struct Placed {
    c: [[f64; 3]; 3],
    n: [f64; 3],
    placement: usize,
    mesh: Entity,
    primitive: usize,
    triangle: usize,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A point in a triangle's own coordinates: `s` along its first edge (corner 0 to 1), `t` along its second (0 to 2).
type Uv = [f64; 2];
/// A convex polygon in a triangle's own coordinates, counter-clockwise (the triangle's winding).
type Poly = Vec<Uv>;
/// An affine function of a triangle's coordinates, `a s + b t + c`, as `[a, b, c]`: a half-plane where it is >= 0.
type Affine = [f64; 3];

fn value(f: Affine, p: Uv) -> f64 {
    f[0] * p[0] + f[1] * p[1] + f[2]
}

/// The signed area, in the triangle's coordinates (the triangle itself is 0.5).
fn area(p: &[Uv]) -> f64 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// The triangle's own edges as half-planes: `s >= 0`, `t >= 0`, `s + t <= 1`.
const INSIDE: [Affine; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, -1.0, 1.0]];

/// The half-planes whose intersection is `p` (convex, counter-clockwise): each edge's left side.
fn edges(p: &[Uv]) -> impl Iterator<Item = Affine> + '_ {
    (0..p.len()).map(|i| {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        [-dy, dx, dy * a[0] - dx * a[1]]
    })
}

/// `p` where `f >= 0` (Sutherland-Hodgman); fewer than three points when nothing is left.
fn clip(p: &[Uv], f: Affine) -> Poly {
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
fn clip_all(p: &[Uv], fs: impl IntoIterator<Item = Affine>, min: f64) -> Option<Poly> {
    let mut p = p.to_vec();
    for f in fs {
        p = clip(&p, f);
        if p.len() < 3 {
            return None;
        }
    }
    (area(&p) >= min).then_some(p)
}

/// `p` (a piece being cut, its corners combinations of the triangle's vertices) where `f >= 0`, as `split-uv-tiles`
/// cuts: a point on an edge is computed from the edge alone, so the triangle across a shared edge cuts it alike.
fn clip_corners(p: &[Corner], f: Affine) -> Option<Vec<Corner>> {
    let mut out = Vec::with_capacity(p.len() + 1);
    for (i, cur) in p.iter().enumerate() {
        let next = &p[(i + 1) % p.len()];
        let (c, d) = (value(f, cur.uv), value(f, next.uv));
        if c >= 0.0 {
            out.push(*cur);
        }
        if (c < 0.0 && d > 0.0) || (c > 0.0 && d < 0.0) {
            out.push(Corner::cut_at(cur, next, |a, b| {
                let (fa, fb) = (value(f, a.uv), value(f, b.uv));
                fa / (fa - fb)
            }));
        }
    }
    tidy(out)
}

fn corners_area(p: &[Corner]) -> f64 {
    area(&p.iter().map(|c| c.uv).collect::<Vec<_>>())
}

/// `p` less the convex polygon `c`: the pieces outside each edge of `c` in turn, those of at least `min` in area,
/// pushed to `out`. `p` whole when the two do not overlap by `min`.
fn subtract(p: Vec<Corner>, c: &[Uv], min: f64, out: &mut Vec<Vec<Corner>>) {
    let uv: Poly = p.iter().map(|k| k.uv).collect();
    if clip_all(&uv, edges(c), min).is_none() {
        out.push(p);
        return;
    }
    let mut rest = Some(p);
    for f in edges(c) {
        let Some(r) = rest.take() else { break };
        if let Some(outside) = clip_corners(&r, [-f[0], -f[1], -f[2]])
            && corners_area(&outside) >= min
        {
            out.push(outside);
        }
        rest = clip_corners(&r, f);
    }
    // what is left is inside `c`: covered
}

/// `p` less the convex polygon `c`, in coordinates only (a covered piece less another): as [`subtract`].
fn subtract_uv(p: Poly, c: &[Uv], min: f64, out: &mut Vec<Poly>) {
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

/// `pieces` as disjoint convex polygons with the same union (each less the ones before it), so intersecting two
/// placements' pieces makes no piece twice; `None` past [`MAX_PIECES`].
fn disjoint(pieces: Vec<Poly>, min: f64) -> Option<Vec<Poly>> {
    let mut out: Vec<Poly> = Vec::new();
    for p in pieces {
        let mut frags = vec![p];
        for q in &out {
            let mut next = Vec::with_capacity(frags.len());
            for f in frags {
                subtract_uv(f, q, min, &mut next);
            }
            frags = next;
            if frags.is_empty() {
                break;
            }
        }
        out.extend(frags);
        if out.len() > MAX_PIECES {
            return None;
        }
    }
    Some(out)
}

/// What the placements covering a stored triangle cover of it so far (kept only for a triangle some placement
/// covers).
#[derive(Default)]
struct Hidden {
    /// How many of its placements cover some of it.
    placements: usize,
    /// The part every one of those covers, as disjoint pieces.
    covered: Vec<Poly>,
    /// More pieces than [`MAX_PIECES`] on the way: kept whole.
    intricate: bool,
    /// The largest scale from the triangle's coordinates to square metres over those placements.
    scale: f64,
}

/// What became of one stored triangle.
enum Fate {
    Gone,
    Cut(Vec<Vec<Corner>>),
}

impl Pass for DropCoveredSurfaces {
    fn name(&self) -> &'static str {
        "drop-covered-surfaces"
    }
    fn options(&self) -> Options {
        Options::from([("gap".into(), self.gap.to_string())])
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.morphs", "core.skin"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    /// The file links it removes are a side effect: the changed meshes no longer match their files.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // placements of each mesh, by transform; a mesh no placement takes, by its own
        let mut placements: Vec<(Entity, Transform)> = Vec::new();
        let mut taken: BTreeSet<Entity> = BTreeSet::new();
        for (e, i) in w.iter::<Instance>() {
            if w.has::<Mesh>(i.prototype) {
                placements.push((
                    i.prototype,
                    w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY),
                ));
                taken.insert(i.prototype);
            }
        }
        for (e, _) in w.iter::<Mesh>() {
            if !taken.contains(&e) {
                placements.push((e, w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY)));
            }
        }
        let mut placed: BTreeMap<Entity, usize> = BTreeMap::new();
        for &(mesh, _) in &placements {
            *placed.entry(mesh).or_default() += 1;
        }
        let mut covers: BTreeMap<Entity, Cover> = BTreeMap::new();
        let mut tris: Vec<Placed> = Vec::new();
        // per (mesh, primitive): its cover kind, decided once over every material it is drawn with
        let mut kinds: BTreeMap<(Entity, usize), Cover> = BTreeMap::new();
        for (pi, &(mesh, t)) in placements.iter().enumerate() {
            let m = w.get::<Mesh>(mesh).expect("a placed mesh");
            for (k, p) in m.primitives().iter().enumerate() {
                let kind = *kinds.entry((mesh, k)).or_insert_with(|| {
                    let ms = drawn_with(w, mesh, k, p);
                    if p.topology != Topology::Triangles || ms.is_empty() {
                        return Cover::No;
                    }
                    // the primitive is what its least covering material is
                    let each: Vec<Cover> = ms
                        .iter()
                        .map(|&e| *covers.entry(e).or_insert_with(|| cover_of(w, e)))
                        .collect();
                    if each.contains(&Cover::No) {
                        Cover::No
                    } else if each.contains(&Cover::DoubleSided) {
                        Cover::DoubleSided
                    } else {
                        Cover::Covering
                    }
                });
                if kind == Cover::No {
                    continue;
                }
                let Some(Stream::F32x3(pos)) = p.streams.get(&Semantic::Position) else {
                    continue;
                };
                let ix: Vec<u32> = match &p.indices {
                    Some(ix) => ix.clone(),
                    None => (0..pos.len() as u32).collect(),
                };
                for (ti, c) in ix.as_chunks::<3>().0.iter().enumerate() {
                    let c: [[f64; 3]; 3] =
                        std::array::from_fn(|j| t.apply(pos[c[j] as usize]).map(f64::from));
                    let nn = cross(sub(c[1], c[0]), sub(c[2], c[0]));
                    let len = dot(nn, nn).sqrt();
                    if len <= 1e-12 || !len.is_finite() || !c.iter().flatten().all(|x| x.is_finite()) {
                        continue;
                    }
                    tris.push(Placed {
                        c,
                        n: nn.map(|x| x / len),
                        placement: pi,
                        mesh,
                        primitive: k,
                        triangle: ti,
                    });
                }
            }
        }
        // the hash: every triangle in the cells its box (padded by the gap) touches
        let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
        let cell = |x: f64| (x / CELL).floor() as i64;
        let cells = |t: &Placed| -> ([i64; 3], [i64; 3]) {
            let lo = std::array::from_fn(|a| {
                cell(t.c.iter().map(|c| c[a]).fold(f64::INFINITY, f64::min) - self.gap)
            });
            let hi = std::array::from_fn(|a| {
                cell(t.c.iter().map(|c| c[a]).fold(f64::NEG_INFINITY, f64::max) + self.gap)
            });
            (lo, hi)
        };
        let mut too_big = 0u64;
        let mut listed = vec![false; tris.len()];
        for (i, t) in tris.iter().enumerate() {
            let (lo, hi) = cells(t);
            if (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) * (hi[2] - lo[2] + 1) > MAX_CELLS {
                too_big += 1;
                continue;
            }
            listed[i] = true;
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        grid.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        let mut hidden: BTreeMap<(Entity, usize, usize), Hidden> = BTreeMap::new();
        let (mut coincident, mut double) = (0u64, 0u64);
        let mut near: Vec<u32> = Vec::new();
        for (i, t) in tris.iter().enumerate() {
            let kind = kinds[&(t.mesh, t.primitive)];
            let (e1, e2) = (sub(t.c[1], t.c[0]), sub(t.c[2], t.c[0]));
            let normal = cross(e1, e2);
            let nn = dot(normal, normal);
            // square metres per unit of the triangle's coordinates' area
            let scale = nn.sqrt();
            let min = MIN_AREA / scale;
            let uv_of = |x: [f64; 3]| -> Uv {
                let d = sub(x, t.c[0]);
                [dot(cross(d, e2), normal) / nn, dot(cross(e1, d), normal) / nn]
            };
            near.clear();
            if listed[i] {
                let (lo, hi) = cells(t);
                for x in lo[0]..=hi[0] {
                    for y in lo[1]..=hi[1] {
                        for z in lo[2]..=hi[2] {
                            near.extend(grid.get(&(x, y, z)).map(Vec::as_slice).unwrap_or(&[]));
                        }
                    }
                }
                near.sort_unstable();
                near.dedup();
            }
            let mut covered: Vec<Poly> = Vec::new();
            let mut same = false;
            for &j in &near {
                let u = &tris[j as usize];
                let facing = dot(u.n, t.n);
                if j as usize == i
                    || (u.placement == t.placement && u.primitive == t.primitive && u.mesh == t.mesh)
                    || facing < FACING
                {
                    continue;
                }
                // the distance from the triangle's plane along its normal to the cover's plane, at each corner
                let d: [f64; 3] = std::array::from_fn(|k| dot(u.n, sub(u.c[0], t.c[k])) / facing);
                let (lo, hi) = (
                    d.iter().copied().fold(f64::INFINITY, f64::min),
                    d.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                );
                if lo > self.gap || hi < -COINCIDENT {
                    continue;
                }
                let mut shadow: Poly = u.c.iter().map(|&x| uv_of(x)).collect();
                if area(&shadow) < 0.0 {
                    shadow.reverse();
                }
                let Some(over) = clip_all(&shadow, INSIDE, min) else {
                    continue;
                };
                let along: Affine = [d[1] - d[0], d[2] - d[0], d[0]];
                let above = |x: f64| [along[0], along[1], along[2] - x];
                let below = |x: f64| [-along[0], -along[1], x - along[2]];
                if lo <= COINCIDENT && clip_all(&over, [above(-COINCIDENT), below(COINCIDENT)], min).is_some()
                {
                    same = true;
                }
                if hi > COINCIDENT
                    && let Some(piece) = clip_all(&over, [above(COINCIDENT), below(self.gap)], min)
                {
                    covered.push(piece);
                }
            }
            if same {
                coincident += 1;
            }
            match kind {
                Cover::DoubleSided => {
                    if !covered.is_empty() {
                        double += 1;
                    }
                }
                Cover::Covering if !covered.is_empty() => {
                    let h = hidden.entry((t.mesh, t.primitive, t.triangle)).or_default();
                    let first = h.placements == 0;
                    h.placements += 1;
                    h.scale = h.scale.max(scale);
                    if h.intricate || (!first && h.covered.is_empty()) {
                        continue;
                    }
                    match disjoint(covered, min) {
                        None => h.intricate = true,
                        Some(here) if first => h.covered = here,
                        // covered in every placement: the overlaps of the two disjoint sets, disjoint
                        Some(here) => {
                            let mut both = Vec::new();
                            for a in &h.covered {
                                for b in &here {
                                    both.extend(clip_all(a, edges(b), min));
                                }
                            }
                            if both.len() > MAX_PIECES {
                                h.intricate = true;
                            } else {
                                h.covered = both;
                            }
                        }
                    }
                }
                Cover::Covering | Cover::No => {}
            }
        }
        drop(grid);
        // what each stored triangle becomes: gone, cut, or (absent) as it was
        let mut fates: BTreeMap<(Entity, usize), BTreeMap<usize, Fate>> = BTreeMap::new();
        let mut too_many = 0u64;
        for ((mesh, k, ti), h) in hidden {
            // a placement that covers none of it keeps all of it
            if h.placements < placed[&mesh] || h.covered.is_empty() {
                continue;
            }
            if h.intricate {
                too_many += 1;
                continue;
            }
            let cover = h.covered;
            let min = MIN_AREA / h.scale;
            // the corners as the primitive's vertices, so a cut point is a combination of those
            let p = &w.get::<Mesh>(mesh).expect("a mesh").primitives()[k];
            let v: [u32; 3] = std::array::from_fn(|j| {
                let at = 3 * ti + j;
                p.indices.as_ref().map_or(at as u32, |ix| ix[at])
            });
            let mut kept = vec![
                [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]
                    .into_iter()
                    .zip(v)
                    .map(|(uv, v)| Corner {
                        at: Bary::corner(v),
                        uv,
                    })
                    .collect::<Vec<_>>(),
            ];
            let mut many = false;
            for c in &cover {
                let mut next = Vec::with_capacity(kept.len());
                for p in kept {
                    subtract(p, c, min, &mut next);
                }
                kept = next;
                if kept.len() > MAX_PIECES {
                    many = true;
                    break;
                }
            }
            if many {
                too_many += 1;
                continue;
            }
            let fate = match kept.as_slice() {
                [] => Fate::Gone,
                [one] if one.len() == 3 && (corners_area(one) - 0.5).abs() < 1e-12 => continue,
                _ => Fate::Cut(kept),
            };
            fates.entry((mesh, k)).or_default().insert(ti, fate);
        }
        // the edits, planned first: per primitive its indices, and for a cut one its new vertices
        let (mut gone_by, mut cut_by): (BTreeMap<String, u64>, BTreeMap<String, u64>) = Default::default();
        let (mut became, mut cannot_follow) = (0u64, 0u64);
        let mut index_only: Vec<(Entity, usize, Vec<u32>)> = Vec::new();
        let mut plan: Vec<Replacement> = Vec::new();
        for ((mesh, k), fate) in &fates {
            let m = w.get::<Mesh>(*mesh).expect("a mesh");
            let p = &m.primitives()[*k];
            let n = p.vertex_count() as u32;
            let all: Vec<u32> = p.indices.clone().unwrap_or_else(|| (0..n).collect());
            let name = p
                .material
                .and_then(|m| w.get::<Name>(m).map(|n| n.0.clone()))
                .unwrap_or_else(|| "(unnamed material)".into());
            let only_gone = |fate: &BTreeMap<usize, Fate>| -> Vec<u32> {
                all.as_chunks::<3>()
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(ti, _)| !matches!(fate.get(ti), Some(Fate::Gone)))
                    .flat_map(|(_, c)| c.iter().copied())
                    .collect()
            };
            let gone = fate.values().filter(|f| matches!(f, Fate::Gone)).count() as u64;
            let cut = fate.len() as u64 - gone;
            if cut == 0 {
                index_only.push((*mesh, *k, only_gone(fate)));
                *gone_by.entry(name).or_default() += gone;
                continue;
            }
            // the old vertices in their places, then the cut's, welded
            let mut blends: Vec<VertexBlend> = (0..n).map(VertexBlend::copy).collect();
            let mut weld: HashMap<Key, u32> = HashMap::new();
            let mut indices: Vec<u32> = Vec::with_capacity(all.len());
            let mut pieces_made = 0u64;
            for (ti, c) in all.as_chunks::<3>().0.iter().enumerate() {
                match fate.get(&ti) {
                    None => indices.extend(c),
                    Some(Fate::Gone) => {}
                    Some(Fate::Cut(pieces)) => {
                        for piece in pieces {
                            let ids: Vec<u32> = piece
                                .iter()
                                .map(|corner| {
                                    let key = corner.at.welded_key(WELD_BITS);
                                    // a corner of the triangle (one term left): its own vertex
                                    if key.1[1] == 0 && key.1[2] == 0 {
                                        return key.0[0];
                                    }
                                    *weld.entry(key).or_insert_with(|| {
                                        blends.push(corner.at.blend());
                                        (blends.len() - 1) as u32
                                    })
                                })
                                .collect();
                            for j in 1..ids.len() - 1 {
                                let tri = [ids[0], ids[j], ids[j + 1]];
                                if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                                    indices.extend(tri);
                                    pieces_made += 1;
                                }
                            }
                        }
                    }
                }
            }
            let what = describe(w, *mesh, *k);
            let influences = w
                .get::<Skin>(*mesh)
                .and_then(|s| s.influences.get(*k).copied().flatten());
            let refusal = w.check_blend_vertices(*mesh, *k, &blends).err();
            match refusal {
                Some(_) => {
                    cannot_follow += cut;
                    index_only.push((*mesh, *k, only_gone(fate)));
                    *gone_by.entry(name).or_default() += gone;
                }
                None => {
                    let blended = blend_streams(p, &blends, influences, &what)?;
                    plan.push(Replacement {
                        entity: *mesh,
                        primitive: *k,
                        streams: blended.streams,
                        indices,
                        blends,
                        influences: blended.influences,
                    });
                    *gone_by.entry(name.clone()).or_default() += gone;
                    *cut_by.entry(name).or_default() += cut;
                    became += pieces_made;
                }
            }
        }
        for (mesh, k, kept) in index_only {
            let m = w.get_mut::<Mesh>(mesh).expect("a mesh");
            let mut p = m.primitive_mut(k).expect("its primitive");
            if p.indices.as_ref().map_or(p.vertex_count(), Vec::len) != kept.len() {
                *p.indices_mut() = Some(kept);
                w.remove::<MeshFile>(mesh);
            }
        }
        replace_primitives(w, plan)?;
        for (name, n) in gone_by.into_iter().filter(|(_, n)| *n > 0) {
            log.omit(
                &format!("triangles covered by an opaque surface in front, left out: {name}"),
                n,
            );
        }
        for (name, n) in cut_by {
            log.omit(
                &format!("triangles partly covered by an opaque surface in front, cut to the rest: {name}"),
                n,
            );
        }
        log.count("triangles the cut ones became", became);
        log.count(
            "triangles covered in more than MAX_PIECES pieces (kept whole)",
            too_many,
        );
        log.count(
            "partly covered triangles of meshes whose per-vertex data cannot follow a cut (kept whole)",
            cannot_follow,
        );
        log.count("triangles on a coincident surface (kept)", coincident);
        log.count("double-sided triangles covered (kept)", double);
        log.count("triangles too big to index (kept, cover nothing)", too_big);
        Ok(())
    }
}
