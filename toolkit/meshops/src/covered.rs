//! `drop-covered-surfaces{gap=M}`: opaque triangles another opaque surface covers from just in front are left out, for
//! a destination whose depth buffer cannot keep the two apart where the source's did.
//!
//! A source drawn with a precise depth buffer (Riders Republic's reversed float depth) can lay one surface a few
//! millimetres over another and show only the upper: The Deck's bowl decks lie 3.7 mm over the plaza's concrete,
//! 440 m2 of it. A destination with less depth precision draws both, fighting in blotches that move with the view
//! (Caleb in skate., 2026-10-25). What the source showed there is the upper surface alone, so the pass leaves the
//! lower out where the upper covers it.
//!
//! **Which triangles.** Every placed triangle of a triangle-list primitive whose materials (its own and every
//! per-placement variant, [`MaterialVariants`]) are all *covering*: a `pbr.material` that is opaque (no alpha test, no
//! blending) and single-sided, not on `core.draw_order`'s decal layer; a `core.layered` blend counts as its base
//! layer's material. A placement is an entity with `core.instance` (its prototype's mesh by its `core.transform`); a
//! mesh no placement takes is drawn by its own transform (none: as stored).
//!
//! **Covered.** A triangle is covered in a placement when each of four points of it (its centroid, and the points a
//! tenth of the way from each corner towards the centroid) lies behind a covering triangle of another primitive or
//! placement: that triangle faces the same way (normals within 8 degrees), and the point's ray along its own normal
//! meets it in front, more than 0.1 mm and at most `gap` metres away. Nearer than 0.1 mm the two are the same surface
//! drawn twice (which one shows is the source's draw order), counted and kept. A triangle of a prototype is left out
//! when it is covered in every placement of it; one covered at some of its points only is kept (a strip may still
//! fight along the upper surface's edge), counted. Double-sided triangles are kept (seen from behind they are in
//! front), counted.
//!
//! **What changes.** Only the primitives' indices: the triangles left out go, the vertices stay (unused ones are
//! harmless to every writer; per-vertex components keep their places). A primitive with every triangle left out keeps
//! no triangles. The mesh's file link goes. Collision is not touched: a covered floor still collides where it did.
//!
//! Reported: the triangles left out by material, those partly covered, coincident or double-sided (kept).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    DrawLayer, DrawOrder, Entity, Instance, LayeredMaterial, MaterialVariants, Mesh, MeshFile, Name, Semantic,
    Stream, Topology, Transform, World,
};

/// Below this distance (metres) a covering surface is the same surface drawn twice.
const COINCIDENT: f64 = 1.0e-4;
/// The least cosine between a covered triangle's normal and its cover's (8 degrees).
const FACING: f64 = 0.99;
/// The hash cell, metres.
const CELL: f64 = 1.0;
/// The most cells one triangle is listed in; a bigger triangle (a far terrain's) covers nothing and is counted.
const MAX_CELLS: i64 = 1 << 16;

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
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Where the ray from `p` along `d` meets triangle `t` (corners `c`, unit normal `n`): the distance along it, when the
/// point met lies inside `t` (its edges included, within a micrometre).
fn meet(p: [f64; 3], d: [f64; 3], c: &[[f64; 3]; 3], n: [f64; 3]) -> Option<f64> {
    let denom = dot(d, n);
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = dot(sub(c[0], p), n) / denom;
    let q = [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
    for k in 0..3 {
        let (a, b) = (c[k], c[(k + 1) % 3]);
        // inside: q on the inner side of every edge (the normal's side), within a micrometre
        let edge = sub(b, a);
        let side = dot(cross(edge, sub(q, a)), n);
        let len = dot(edge, edge).sqrt();
        if side < -1e-6 * len {
            return None;
        }
    }
    Some(t)
}

impl Pass for DropCoveredSurfaces {
    fn name(&self) -> &'static str {
        "drop-covered-surfaces"
    }
    fn options(&self) -> Options {
        Options::from([("gap".into(), self.gap.to_string())])
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // placements of each mesh, by transform; a mesh no placement takes, by its own
        let mut placements: Vec<(Entity, Transform)> = Vec::new();
        let mut taken: BTreeSet<Entity> = BTreeSet::new();
        for (e, i) in w.iter::<Instance>() {
            if w.has::<Mesh>(i.prototype) {
                placements.push((i.prototype, w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY)));
                taken.insert(i.prototype);
            }
        }
        for (e, _) in w.iter::<Mesh>() {
            if !taken.contains(&e) {
                placements.push((e, w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY)));
            }
        }
        let mut covers: BTreeMap<Entity, Cover> = BTreeMap::new();
        let mut cover = |w: &World, m: Entity| *covers.entry(m).or_insert_with(|| cover_of(w, m));
        let mut tris: Vec<Placed> = Vec::new();
        // per (mesh, primitive): its cover kind, decided once over every material it is drawn with
        let mut kinds: BTreeMap<(Entity, usize), Cover> = BTreeMap::new();
        for (pi, &(mesh, t)) in placements.iter().enumerate() {
            let m = w.get::<Mesh>(mesh).expect("a placed mesh");
            for (k, p) in m.primitives().iter().enumerate() {
                let kind = *kinds.entry((mesh, k)).or_insert_with(|| {
                    if p.topology != Topology::Triangles {
                        return Cover::No;
                    }
                    let mut ms: BTreeSet<Entity> = p.material.into_iter().collect();
                    if let Some(v) = w.get::<MaterialVariants>(mesh).and_then(|v| v.0.get(k)) {
                        ms.extend(v.values().copied());
                    }
                    if ms.is_empty() {
                        return Cover::No;
                    }
                    // the primitive is what its least covering material is
                    let each: Vec<Cover> = ms.iter().map(|&e| cover(w, e)).collect();
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
                let Some(Stream::F32x3(pos)) = p.streams.get(&Semantic::Position) else { continue };
                let ix: Vec<u32> = match &p.indices {
                    Some(ix) => ix.clone(),
                    None => (0..pos.len() as u32).collect(),
                };
                for (ti, c) in ix.as_chunks::<3>().0.iter().enumerate() {
                    let c: [[f64; 3]; 3] = std::array::from_fn(|j| t.apply(pos[c[j] as usize]).map(f64::from));
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
        let mut too_big = 0u64;
        for (i, t) in tris.iter().enumerate() {
            let lo: [i64; 3] = std::array::from_fn(|a| cell(t.c.iter().map(|c| c[a]).fold(f64::INFINITY, f64::min) - self.gap));
            let hi: [i64; 3] =
                std::array::from_fn(|a| cell(t.c.iter().map(|c| c[a]).fold(f64::NEG_INFINITY, f64::max) + self.gap));
            let count = (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) * (hi[2] - lo[2] + 1);
            if count > MAX_CELLS {
                too_big += 1;
                continue;
            }
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        grid.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        // per stored triangle: covered in every placement so far (None: not yet seen), and the partly covered
        let mut hidden: BTreeMap<(Entity, usize, usize), bool> = BTreeMap::new();
        let (mut partly, mut coincident, mut double) = (0u64, 0u64, 0u64);
        for (i, t) in tris.iter().enumerate() {
            let kind = kinds[&(t.mesh, t.primitive)];
            let centre: [f64; 3] = std::array::from_fn(|a| (t.c[0][a] + t.c[1][a] + t.c[2][a]) / 3.0);
            let points = [
                centre,
                std::array::from_fn(|a| t.c[0][a] + 0.1 * (centre[a] - t.c[0][a])),
                std::array::from_fn(|a| t.c[1][a] + 0.1 * (centre[a] - t.c[1][a])),
                std::array::from_fn(|a| t.c[2][a] + 0.1 * (centre[a] - t.c[2][a])),
            ];
            let mut covered = 0;
            let mut same = false;
            for p in points {
                let key = (cell(p[0]), cell(p[1]), cell(p[2]));
                let mut hit = false;
                for &j in grid.get(&key).map(Vec::as_slice).unwrap_or(&[]) {
                    let u = &tris[j as usize];
                    if j as usize == i
                        || (u.placement == t.placement && u.primitive == t.primitive && u.mesh == t.mesh)
                        || dot(u.n, t.n) < FACING
                    {
                        continue;
                    }
                    if let Some(d) = meet(p, t.n, &u.c, u.n) {
                        if d.abs() <= COINCIDENT {
                            same = true;
                        } else if d > COINCIDENT && d <= self.gap {
                            hit = true;
                            break;
                        }
                    }
                }
                if hit {
                    covered += 1;
                }
            }
            let all = covered == points.len();
            if all && kind == Cover::DoubleSided {
                double += 1;
            }
            if covered > 0 && !all {
                partly += 1;
            }
            if same && !all {
                coincident += 1;
            }
            let here = all && kind == Cover::Covering;
            let e = hidden.entry((t.mesh, t.primitive, t.triangle)).or_insert(true);
            *e = *e && here;
        }
        // a stored triangle a placement never drew is not hidden (it had no placement: not in `hidden`)
        let mut by_material: BTreeMap<String, u64> = BTreeMap::new();
        let mut drops: BTreeMap<(Entity, usize), BTreeSet<usize>> = BTreeMap::new();
        for (&(mesh, k, ti), &h) in &hidden {
            if h {
                drops.entry((mesh, k)).or_default().insert(ti);
            }
        }
        for ((mesh, k), tis) in &drops {
            let name = {
                let m = w.get::<Mesh>(*mesh).expect("a mesh");
                let p = &m.primitives()[*k];
                p.material
                    .and_then(|m| w.get::<Name>(m).map(|n| n.0.clone()))
                    .unwrap_or_else(|| "(unnamed material)".into())
            };
            *by_material.entry(name).or_default() += tis.len() as u64;
            let m = w.get_mut::<Mesh>(*mesh).expect("a mesh");
            let mut p = m.primitive_mut(*k).expect("its primitive");
            let n = p
                .streams_mut()
                .get(&Semantic::Position)
                .map_or(0, Stream::len) as u32;
            let ix = p.indices_mut();
            let all: Vec<u32> = ix.take().unwrap_or_else(|| (0..n).collect());
            let kept: Vec<u32> = all
                .as_chunks::<3>()
                .0
                .iter()
                .enumerate()
                .filter(|(ti, _)| !tis.contains(ti))
                .flat_map(|(_, c)| c.iter().copied())
                .collect();
            *ix = Some(kept);
        }
        for (mesh, _) in drops.keys() {
            w.remove::<MeshFile>(*mesh);
        }
        for (name, n) in by_material {
            log.omit(&format!("triangles covered by an opaque surface in front, left out: {name}"), n);
        }
        log.count("triangles partly covered (kept)", partly);
        log.count("triangles on a coincident surface (kept)", coincident);
        log.count("double-sided triangles covered (kept)", double);
        log.count("triangles too big to index (kept, cover nothing)", too_big);
        Ok(())
    }
}
