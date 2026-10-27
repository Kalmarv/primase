//! The layering gate: whether a compiled level draws every layer the way skate.'s renderer keeps it apart, checked
//! on what the Patch holds after every post-compile step ([`Level`]), with each section's class from the
//! fixed-function state of its material's programs ([`frostbite_formats::permutation::FixedState`]).
//!
//! **How skate. draws layers** (measured: the programs' pipeline states as the game hands them to D3D12, and a fixture
//! of every case compiled by Studio and looked at in the game; the limits name their evidence). Opaque and alpha-tested
//! surfaces write depth (a reversed float depth buffer, GreaterEqual) and set a stencil bit. Every layer drawn on
//! another surface is *decal class*: no depth write, GreaterEqual with a depth bias (200 and slope 0.1), drawn only
//! where the stencil bit is set, blended into the G-buffer after the opaque pass. So:
//!
//! - two depth-writing surfaces closer than the depth buffer can tell apart fight ([`Check::DepthWriting`]): they need
//!   a clearance ([`Limits::clearance`]); two copies of the very same triangles draw one of them cleanly (which one is
//!   the build's draw order) and are counted apart ([`Check::Duplicate`]);
//! - a decal-class layer shows over its base when it lies on it or slightly above it; it hides where a depth-writing
//!   surface lies in front of it by more than the bias ([`Check::Behind`] near it, [`Check::Covered`] further), and it
//!   shows *through* such a surface where the surface is within the bias: the leak ([`Check::Covered`]). One with no
//!   depth-writing surface under it is invisible (stencil, [`Check::Unbacked`]); one floating far above its base drifts
//!   against it with the view ([`Check::Floating`]);
//! - decal-class layers never fight each other: what shows is their draw order, back to front by bounds between
//!   objects and launch-dependent between sections of one MeshSet ([`Check::Order`]);
//! - all of it holds for every pair of LODs that can be drawn together ([`Check::Lod`]), where the bias has grown with
//!   the distance at which the pair draws.
//!
//! What the game itself places (its own MeshSets, [`Stock`]) is judged only against what the build put beside it.
//! Positions are measured as stored (half floats as their values), so what storing moved is already in them. The
//! checks look at pairs of same-facing triangles ([`primase_meshops::layers`]) whose projections overlap, in the
//! level's frame, tile by tile (a triangle belongs to the tile of its centroid; its neighbours are gathered from the
//! tile padded by the reach). Areas are square metres of the overlap.

use std::collections::{BTreeMap, BTreeSet};

use frostbite_formats::MESHSET;
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::Source;
use frostbite_formats::listing::Bundle;
use frostbite_formats::lookup::{self, ProgramRecord};
use frostbite_formats::permutation::FixedState;
use frostbite_formats::scope::Scope;
use primase_frostbite::read::program::program_states;
use primase_meshops::layers::{
    Frame, Index, Over, Poly, Tri, area, clip_all, disjoint, dot, edges, subtract,
};

use crate::postcompile::level::{Level, Placement, ROOT_TOC};
use crate::postcompile::{Error, Result};

/// What a section draws as: from the fixed-function states of its material's programs' raster permutations, and its
/// layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Some program tests and writes depth: opaque, alpha-tested.
    DepthWriting,
    /// No program writes depth; one draws biased and stencil-tested: decals, blended surfaces on the decal row.
    Decal,
    /// A section of positions alone: a depth or shadow copy of a drawn section, whose triangles it repeats.
    DepthOnly,
    /// Anything else (blended without a bias, ...): not checked.
    Other,
    /// A program index the game has no program for (a program a Patch carries of its own): not drawable by a runtime
    /// that loads the game's programs.
    Unknown,
}

/// The class of a material whose programs' raster states are `states` (`None`: a program the game does not have).
pub fn class_of(states: Option<&[FixedState]>) -> Class {
    let Some(states) = states else {
        return Class::Unknown;
    };
    if states.iter().any(|s| s.depth_test && s.depth_write) {
        Class::DepthWriting
    } else if states.iter().any(|s| s.depth_bias > 0 && s.stencil_test) {
        Class::Decal
    } else {
        Class::Other
    }
}

/// The gate's thresholds, metres unless stated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Two depth-writing surfaces closer than this fight. In the game 0 mm fought at every distance and 0.5 mm was clean
    /// from 7 to 80 m (fixture column 0), so this sits under 0.5 mm by more than float positions' error.
    pub clearance: f64,
    /// Two triangles whose corners agree within this are copies of one another.
    pub duplicate: f64,
    /// How far behind a decal-class layer a depth-writing surface may lie and still be its base.
    pub reach: f64,
    /// How far a depth-writing surface may lie in front of a decal-class layer and still count as under it: the
    /// decal row's constant depth bias at the nearest camera, 2 m ([`Limits::bias_per_metre`]).
    pub behind: f64,
    /// The decal row's constant depth bias in metres per metre of camera distance, at its smallest: 200 steps of the
    /// float depth buffer (D32, reversed) at its depth's exponent, half a step's worth below the depth itself, so
    /// `200 x 2^-24` (the slope-scaled part only adds, and is nil seen square on).
    pub bias_per_metre: f64,
    /// How far above its base a decal-class layer may float.
    pub lift: f64,
    /// How far in front of a decal-class layer a depth-writing surface is looked for: the depth bias reaches this far
    /// at the views that matter (floors, `|normal.y| >= 0.7`; walls).
    pub cover_floor: f64,
    pub cover_wall: f64,
    /// The least cosine between two layered triangles' normals.
    pub facing: f64,
    /// Overlapping decal-class layers of two objects keep one order for every view above this angle from their
    /// surface: the tangent of that angle.
    pub grazing: f64,
    /// The least distance between two overlapping decal-class objects' box centres along the layer's normal for their
    /// order to be fixed (`ChainOrder::Stacked` keeps them 2 m apart).
    pub order_gap: f64,
    /// Unbacked fails for a mesh whose unbacked area reaches this, square metres (layers no one sees; less is a sliver,
    /// such as the bounds pads that order a stack).
    pub unbacked: f64,
    /// A check fails when its area reaches this, square metres.
    pub fail_area: f64,
    /// The side of a tile, metres.
    pub tile: f64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            clearance: 0.000_45,
            duplicate: 1.0e-6,
            reach: 0.05,
            behind: 2.0 * 200.0 * 2f64.powi(-24),
            bias_per_metre: 200.0 * 2f64.powi(-24),
            lift: 0.008,
            cover_floor: 0.15,
            cover_wall: 0.004,
            facing: 8f64.to_radians().cos(),
            grazing: 1f64.to_radians().tan(),
            order_gap: 0.1,
            unbacked: 0.01,
            fail_area: 1.0e-4,
            tile: 64.0,
        }
    }
}

/// The checks ([`Report::findings`]), each with the rule it measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    /// Depth-writing over depth-writing within the clearance: they fight.
    DepthWriting,
    /// Two copies of the same triangles, both depth-writing: no fight; which one shows is the build's draw order.
    Duplicate,
    /// A decal-class layer with a depth-writing surface in front of it by up to the lift: hidden near, shown far.
    Behind,
    /// A decal-class layer with a depth-writing surface in front of it beyond the lift, within the cover reach: the
    /// layer shows through that surface where the depth bias exceeds the gap (the leak).
    Covered,
    /// A decal-class layer whose nearest surface behind is further than the lift.
    Floating,
    /// A decal-class layer with no depth-writing surface behind it within the reach: the stencil hides it. Fails per
    /// mesh ([`Limits::unbacked`]).
    Unbacked,
    /// Overlapping decal-class layers whose draw order is not fixed: sections of one MeshSet, or objects whose boxes do
    /// not set one order for every view in front (`Tile::order_flips`).
    Order,
    /// Sections whose material names a program the game does not have.
    UnknownProgram,
    /// Any of the depth checks failing only between LODs other than both finest.
    Lod,
}

impl Check {
    /// Whether this check failing fails the gate.
    pub fn fails(self) -> bool {
        self != Check::Duplicate
    }
}

/// One offending pair (or triangle), for the report.
#[derive(Clone, Debug, PartialEq)]
pub struct Offender {
    pub area_m2: f64,
    /// The measured value: a separation or height in metres, or an angle's tangent.
    pub value: f64,
    /// The overlap's first corner, level frame.
    pub at: [f64; 3],
    pub document: String,
    pub mesh: String,
    pub lod: usize,
    pub key: u64,
    /// The other triangle's, when the finding is a pair.
    pub other: Option<(String, String, usize, u64)>,
}

/// What one check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Finding {
    pub area_m2: f64,
    pub pairs: u64,
    /// The area by mesh (the offending layer's MeshSet name).
    pub by_mesh: BTreeMap<String, f64>,
    /// The largest offenders by area, at most [`TOP`].
    pub top: Vec<Offender>,
}

/// How many offenders a finding keeps.
pub const TOP: usize = 20;

/// The gate's result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub findings: BTreeMap<Check, Finding>,
    /// Triangles checked, by class.
    pub triangles: BTreeMap<Class, u64>,
    /// Placements not resolved and sections not read ([`Level`]).
    pub unresolved: BTreeMap<String, u64>,
    /// Meshes with several LODs whose distances the Patch does not state: every LOD pair counted as drawable.
    pub unknown_lod_distances: u64,
    /// Triangles too big for the index (they find no neighbours).
    pub unindexed: u64,
}

impl Report {
    /// Whether no failing check reached [`Limits::fail_area`] (for [`Check::Unbacked`], [`Limits::unbacked`] in one
    /// mesh).
    pub fn passed(&self, limits: &Limits) -> bool {
        self.failing(limits).is_empty()
    }

    /// The checks that fail.
    pub fn failing(&self, limits: &Limits) -> Vec<Check> {
        self.findings
            .iter()
            .filter(|(c, f)| {
                c.fails()
                    && match c {
                        Check::Unbacked => f.by_mesh.values().any(|a| *a >= limits.unbacked),
                        _ => f.area_m2 >= limits.fail_area,
                    }
            })
            .map(|(c, _)| *c)
            .collect()
    }
}

/// A placed triangle and what the checks need of it.
struct Item {
    tri: Tri,
    placement: u32,
    lod: u8,
    section: u16,
    triangle: u32,
    class: Class,
    key: u64,
    /// Whether its mesh is the game's: two stock triangles are the game's own business, not the build's.
    stock: bool,
}

impl Item {
    /// Its place in the level, for a fixed order between two items.
    fn id(&self) -> (u32, u8, u16, u32) {
        (self.placement, self.lod, self.section, self.triangle)
    }
}

/// What the checks need of a placement: its box (level frame, every LOD), its order reference point, and per LOD
/// the camera distances at which it draws.
struct Placed {
    lo: [f64; 3],
    hi: [f64; 3],
    centre: [f64; 3],
    ranges: Vec<(f64, f64)>,
}

fn placed(level: &Level, p: &Placement, unknown: &mut u64) -> Placed {
    let mesh = &level.meshes[p.mesh];
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let (mut lo0, mut hi0) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for (li, lod) in mesh.lods.iter().enumerate() {
        for s in lod {
            for &v in &s.positions {
                let w = p.apply(v);
                for a in 0..3 {
                    lo[a] = lo[a].min(w[a]);
                    hi[a] = hi[a].max(w[a]);
                    if li == 0 {
                        lo0[a] = lo0[a].min(w[a]);
                        hi0[a] = hi0[a].max(w[a]);
                    }
                }
            }
        }
    }
    let n = mesh.lods.len();
    let ranges = match &mesh.lod_distances {
        Some(d) => (0..n)
            .map(|l| {
                let a = if l == 0 {
                    0.0
                } else {
                    d.get(l - 1).copied().unwrap_or(f64::INFINITY)
                };
                let b = if l + 1 == n {
                    f64::INFINITY
                } else {
                    d.get(l).copied().unwrap_or(f64::INFINITY)
                };
                (a, b)
            })
            .collect(),
        None => {
            if n > 1 {
                *unknown += 1;
            }
            vec![(0.0, f64::INFINITY); n]
        }
    };
    Placed {
        lo,
        hi,
        centre: std::array::from_fn(|a| (lo0[a] + hi0[a]) / 2.0),
        ranges,
    }
}

/// Whether LOD `la` of `a` and LOD `lb` of `b` can be drawn at once: some camera distance to `a` in `la`'s range has
/// a distance to `b` in `lb`'s, the two distances differing by at most the distance between their centres. If so, the
/// least camera distance (to `a`) at which they are.
fn together(a: &Placed, la: usize, b: &Placed, lb: usize) -> Option<f64> {
    let d = dot(
        std::array::from_fn(|k| a.centre[k] - b.centre[k]),
        std::array::from_fn(|k| a.centre[k] - b.centre[k]),
    )
    .sqrt();
    let (a0, a1) = a.ranges[la];
    let (b0, b1) = b.ranges[lb];
    (a0 < b1 + d && b0 < a1 + d).then(|| a0.max(b0 - d))
}

/// A bounded list of the largest offenders.
fn keep(top: &mut Vec<Offender>, o: Offender) {
    top.push(o);
    if top.len() >= 4 * TOP {
        prune(top);
    }
}

fn prune(top: &mut Vec<Offender>) {
    top.sort_by(|a, b| {
        b.area_m2
            .total_cmp(&a.area_m2)
            .then_with(|| a.document.cmp(&b.document))
            .then_with(|| a.at.map(f64::to_bits).cmp(&b.at.map(f64::to_bits)))
    });
    top.truncate(TOP);
}

/// The total area of convex pieces, overlaps counted once (`t`'s coordinates).
fn union_area(pieces: Vec<Poly>, min: f64) -> f64 {
    disjoint(pieces, min, usize::MAX)
        .unwrap_or_default()
        .iter()
        .map(|p| area(p))
        .sum()
}

/// Whether two triangles have the same corners within `tol` (in any order).
fn same_corners(a: &Tri, b: &Tri, tol: f64) -> bool {
    a.c.iter()
        .all(|p| b.c.iter().any(|q| (0..3).all(|k| (p[k] - q[k]).abs() <= tol)))
}

/// Runs every check over `level`, each section's class taken from `classes` by its material key (a key absent from
/// it: [`Class::Other`]); see the module documentation.
pub fn check(level: &Level, classes: &BTreeMap<u64, Class>, limits: &Limits) -> Report {
    let mut report = Report {
        unresolved: level
            .unresolved
            .iter()
            .chain(&level.unread)
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
        ..Report::default()
    };
    let placements: Vec<Placed> = level
        .placements
        .iter()
        .map(|p| placed(level, p, &mut report.unknown_lod_distances))
        .collect();
    let pad = limits.cover_floor.max(limits.reach).max(limits.clearance);
    // the tiles each placement's box (padded) reaches, horizontally
    let tile = |x: f64| (x / limits.tile).floor() as i64;
    let mut tiles: BTreeMap<(i64, i64), Vec<u32>> = BTreeMap::new();
    for (i, p) in placements.iter().enumerate() {
        if !p.lo[0].is_finite() {
            continue;
        }
        for tx in tile(p.lo[0] - pad)..=tile(p.hi[0] + pad) {
            for tz in tile(p.lo[2] - pad)..=tile(p.hi[2] + pad) {
                tiles.entry((tx, tz)).or_default().push(i as u32);
            }
        }
    }
    let class_of_key = |k: u64| classes.get(&k).copied().unwrap_or(Class::Other);
    let mut tally = Tally::default();
    let (mut near, mut deferred) = (Vec::new(), Vec::new());
    for ((tx, tz), members) in &tiles {
        let (x0, z0) = (*tx as f64 * limits.tile, *tz as f64 * limits.tile);
        let (x1, z1) = (x0 + limits.tile, z0 + limits.tile);
        // the triangles reaching the padded tile
        let mut items: Vec<Item> = Vec::new();
        for &pi in members {
            let p = &level.placements[pi as usize];
            for (li, lod) in level.meshes[p.mesh].lods.iter().enumerate() {
                for (si, s) in lod.iter().enumerate() {
                    let class = if s.position_only {
                        Class::DepthOnly
                    } else {
                        class_of_key(s.key)
                    };
                    if !matches!(class, Class::DepthWriting | Class::Decal | Class::Unknown) {
                        continue;
                    }
                    for (ti, t) in s.triangles.iter().enumerate() {
                        let c = t.map(|v| p.apply(s.positions[v as usize]));
                        let span = |a: usize| {
                            let v = c.map(|q| q[a]);
                            (
                                v.iter().copied().fold(f64::INFINITY, f64::min),
                                v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                            )
                        };
                        let ((lx, hx), (lz, hz)) = (span(0), span(2));
                        if hx < x0 - pad || lx > x1 + pad || hz < z0 - pad || lz > z1 + pad {
                            continue;
                        }
                        let Some(tri) = Tri::new(c) else { continue };
                        items.push(Item {
                            tri,
                            placement: pi,
                            lod: li as u8,
                            section: si as u16,
                            triangle: ti as u32,
                            class,
                            key: s.key,
                            stock: level.meshes[p.mesh].stock,
                        });
                    }
                }
            }
        }
        // a tile of the game's meshes alone holds nothing the build put there
        if items.iter().all(|i| i.stock) {
            continue;
        }
        let tris: Vec<Tri> = items.iter().map(|i| i.tri).collect();
        let index = Index::new(&tris, 1.0, pad);
        let here = Tile {
            level,
            placements: &placements,
            items,
            tris,
            index,
            limits,
        };
        deferred.clear();
        for (i, t) in here.items.iter().enumerate() {
            let centroid: [f64; 3] =
                std::array::from_fn(|a| (t.tri.c[0][a] + t.tri.c[1][a] + t.tri.c[2][a]) / 3.0);
            if !(x0..x1).contains(&centroid[0]) || !(z0..z1).contains(&centroid[2]) {
                continue;
            }
            *report.triangles.entry(t.class).or_default() += 1;
            if !here.index.listed(i) {
                report.unindexed += 1;
            }
            here.check_one(i, &mut near, &mut tally, &mut deferred);
        }
        // a decal-class layer against depth-writing surfaces at LODs other than both finest, once the tile has found
        // which placements are its bases
        for &(i, j) in &deferred {
            here.lod_pair(i, j, &mut tally);
        }
    }
    for f in tally.findings.values_mut() {
        prune(&mut f.top);
    }
    report.findings = tally.findings;
    report
}

/// What the checks gather.
#[derive(Default)]
struct Tally {
    findings: BTreeMap<Check, Finding>,
    /// (decal-class placement, depth-writing placement) where the second backs the first at both finest LODs.
    bases: BTreeSet<(u32, u32)>,
}

/// One tile's triangles (every one reaching the padded tile) and their index.
struct Tile<'a> {
    level: &'a Level,
    placements: &'a [Placed],
    items: Vec<Item>,
    tris: Vec<Tri>,
    index: Index,
    limits: &'a Limits,
}

impl Tile<'_> {
    /// Records a finding of triangle `t`, against `other` when it is a pair.
    #[allow(clippy::too_many_arguments)]
    fn add(
        &self,
        tally: &mut Tally,
        t: &Item,
        check: Check,
        area_m2: f64,
        value: f64,
        at: [f64; 3],
        other: Option<&Item>,
    ) {
        let level = self.level;
        let names = |it: &Item| {
            let p = &level.placements[it.placement as usize];
            (
                p.document.clone(),
                level.meshes[p.mesh].name.clone(),
                usize::from(it.lod),
                it.key,
            )
        };
        let (document, mesh, lod, key) = names(t);
        let f = tally.findings.entry(check).or_default();
        f.area_m2 += area_m2;
        f.pairs += 1;
        *f.by_mesh.entry(mesh.clone()).or_default() += area_m2;
        keep(
            &mut f.top,
            Offender {
                area_m2,
                value,
                at,
                document,
                mesh,
                lod,
                key,
                other: other.map(names),
            },
        );
    }

    /// Whether `t` and `u` can be drawn at once: not two LODs (or the same section) of one placement, and two
    /// placements' LODs only at camera distances both allow; and not both the game's.
    fn drawn_together(&self, t: &Item, u: &Item) -> bool {
        if t.stock && u.stock {
            return false;
        }
        if u.placement == t.placement {
            return u.lod == t.lod && u.section != t.section;
        }
        together(
            &self.placements[t.placement as usize],
            usize::from(t.lod),
            &self.placements[u.placement as usize],
            usize::from(u.lod),
        )
        .is_some()
    }

    /// Whether overlapping decal-class layers `t` and `u` keep one draw order from every view in front of `t`. Sections
    /// of one MeshSet do not (their order changes from launch to launch). Two objects are drawn back to front by their
    /// bounds; the order flips where the camera crosses the plane halfway between the two boxes' centres, square to
    /// the line joining them. It stays one order when that line runs along `t`'s normal (within the grazing angle),
    /// the centres lie at least the order gap apart, and the halfway plane lies at or behind `t`'s surface. Returns the
    /// failing line's tangent (infinite when the gap or the plane fails), `None` when the order is fixed.
    fn order_flips(&self, t: &Item, u: &Item) -> Option<f64> {
        if u.placement == t.placement {
            return Some(f64::INFINITY);
        }
        let (a, b) = (
            &self.placements[t.placement as usize],
            &self.placements[u.placement as usize],
        );
        let delta: [f64; 3] = std::array::from_fn(|k| b.centre[k] - a.centre[k]);
        let along = dot(delta, t.tri.n).abs();
        let lateral = (dot(delta, delta) - along * along).max(0.0).sqrt();
        let middle: [f64; 3] = std::array::from_fn(|k| (a.centre[k] + b.centre[k]) / 2.0 - t.tri.c[0][k]);
        if along < self.limits.order_gap || dot(middle, t.tri.n) > 0.0 {
            return Some(f64::INFINITY);
        }
        let tangent = lateral / along;
        (tangent > self.limits.grazing).then_some(tangent)
    }

    /// Triangle `i` against its neighbours: every check at both finest LODs and between depth-writing LODs; a
    /// decal-class triangle's pairs with depth-writing surfaces at other LODs are left in `deferred` for
    /// [`Tile::lod_pair`].
    fn check_one(
        &self,
        i: usize,
        near: &mut Vec<u32>,
        tally: &mut Tally,
        deferred: &mut Vec<(usize, usize)>,
    ) {
        let limits = self.limits;
        let t = &self.items[i];
        let frame = Frame::of(&t.tri);
        let scale = frame.scale;
        // pieces under a square millimetre are not counted
        let min = 1.0e-6 / scale;
        let whole = scale / 2.0;
        if t.class == Class::Unknown {
            self.add(tally, t, Check::UnknownProgram, whole, 0.0, t.tri.c[0], None);
            return;
        }
        let cover = if t.tri.n[1].abs() >= 0.7 {
            limits.cover_floor
        } else {
            limits.cover_wall
        };
        // a decal-class triangle's pieces: depth-writing surfaces just in front, further in front, behind (its base),
        // deeper
        let (mut behind, mut covered, mut backed, mut deep) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // the surface of the largest piece in front, with the piece's area and its distance in front: what a finding
        // names
        let mut front: [Option<(f64, f64, usize)>; 2] = [None, None];
        self.index.near(&self.tris, i, near);
        for &j in near.iter() {
            let u = &self.items[j as usize];
            if !self.drawn_together(t, u) {
                continue;
            }
            let Some(d) = Over::corner_distances(&t.tri, &u.tri, limits.facing) else {
                continue;
            };
            let lo = d.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = d.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let finest = t.lod == 0 && u.lod == 0;
            match (t.class, u.class) {
                (Class::DepthWriting, Class::DepthWriting) => {
                    if lo >= limits.clearance || hi < 0.0 {
                        continue;
                    }
                    if same_corners(&t.tri, &u.tri, limits.duplicate) {
                        if t.id() < u.id() {
                            self.add(tally, t, Check::Duplicate, whole, 0.0, t.tri.c[0], Some(u));
                        }
                        continue;
                    }
                    let Some(over) = Over::of(&frame, &u.tri, d, min) else {
                        continue;
                    };
                    let (rlo, rhi) = over.range();
                    // a coplanar pair is seen from both sides: counted from the first
                    if rlo.abs() <= 1.0e-9 && rhi.abs() <= 1.0e-9 && t.id() > u.id() {
                        continue;
                    }
                    if let Some(piece) = over.band(0.0, limits.clearance, min) {
                        let check = if finest { Check::DepthWriting } else { Check::Lod };
                        self.add(
                            tally,
                            t,
                            check,
                            area(&piece) * scale,
                            rlo.max(0.0),
                            frame.point(piece[0]),
                            Some(u),
                        );
                    }
                }
                (Class::Decal, Class::DepthWriting) if !finest => {
                    if lo <= cover.max(limits.reach) && hi >= -limits.reach {
                        deferred.push((i, j as usize));
                    }
                }
                (Class::Decal, Class::DepthWriting) => {
                    if lo > cover || hi < -limits.reach {
                        continue;
                    }
                    let Some(over) = Over::of(&frame, &u.tri, d, min) else {
                        continue;
                    };
                    for (k, (lo, hi, out)) in [
                        (limits.behind, limits.lift, &mut behind),
                        (limits.lift, cover, &mut covered),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if let Some(piece) = over.band(lo, hi, min) {
                            let a = area(&piece);
                            if front[k].is_none_or(|(b, _, _)| a > b) {
                                front[k] = Some((a, over.range().0.max(lo), j as usize));
                            }
                            out.push(piece);
                        }
                    }
                    if let Some(piece) = over.band(-limits.lift, limits.behind, min) {
                        tally.bases.insert((t.placement, u.placement));
                        backed.push(piece);
                    }
                    deep.extend(over.band(-limits.reach, -limits.lift, min));
                }
                (Class::Decal, Class::Decal) => {
                    if lo > limits.reach || hi < -limits.reach || t.id() > u.id() {
                        continue;
                    }
                    let Some(over) = Over::of(&frame, &u.tri, d, min) else {
                        continue;
                    };
                    let Some(piece) = over.band(-limits.reach, limits.reach, min) else {
                        continue;
                    };
                    if let Some(tangent) = self.order_flips(t, u) {
                        self.add(
                            tally,
                            t,
                            Check::Order,
                            area(&piece) * scale,
                            tangent,
                            frame.point(piece[0]),
                            Some(u),
                        );
                    }
                }
                _ => {}
            }
        }
        if t.class != Class::Decal || t.lod != 0 {
            return;
        }
        let at = t.tri.c[0];
        for (check, pieces, front) in [
            (Check::Behind, &behind, front[0]),
            (Check::Covered, &covered, front[1]),
        ] {
            let a = union_area(pieces.clone(), min) * scale;
            if let (true, Some((_, gap, u))) = (a > 0.0, front) {
                self.add(tally, t, check, a, gap, at, Some(&self.items[u]));
            }
        }
        // a stock layer is judged only against the build's surfaces (in front of it, above), not for its own base
        if t.stock {
            return;
        }
        // what no surface backs or lies in front of (those draw the stencil there; in front is reported above):
        // floating where a surface lies deeper than the lift, else unbacked
        let mut rest: Vec<Poly> = vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]];
        for b in backed.iter().chain(&behind).chain(&covered) {
            let mut next = Vec::new();
            for r in rest {
                subtract(r, b, min, &mut next);
            }
            rest = next;
        }
        let rest_area: f64 = rest.iter().map(|r| area(r)).sum();
        if rest_area * scale <= 1.0e-6 {
            return;
        }
        let deep = disjoint(deep, min, usize::MAX).unwrap_or_default();
        let floating: f64 = rest
            .iter()
            .flat_map(|r| deep.iter().filter_map(|d| clip_all(r, edges(d), min)))
            .map(|p| area(&p))
            .sum::<f64>()
            * scale;
        if floating > 0.0 {
            self.add(tally, t, Check::Floating, floating, 0.0, at, None);
        }
        let unbacked = rest_area * scale - floating;
        if unbacked > 1.0e-6 {
            self.add(tally, t, Check::Unbacked, unbacked, 0.0, at, None);
        }
    }

    /// Decal-class triangle `i` against depth-writing triangle `j` at LODs other than both finest. Against a base (a
    /// placement that backs the layer at their finest LODs) the base must not come in front of the layer by more than
    /// the bias at the nearest camera that draws the two LODs together (up to the reach): there it hides the layer. A
    /// coarser base dropping below the layer is not checked: the layer still draws over it (the base sets the stencil
    /// behind it), and from that far its lift does not show. Against any other surface, one in front within the cover
    /// reach leaks through it, as at the finest LODs.
    fn lod_pair(&self, i: usize, j: usize, tally: &mut Tally) {
        let limits = self.limits;
        let (t, u) = (&self.items[i], &self.items[j]);
        let frame = Frame::of(&t.tri);
        let min = 1.0e-6 / frame.scale;
        let Some(d) = Over::corner_distances(&t.tri, &u.tri, limits.facing) else {
            return;
        };
        let Some(over) = Over::of(&frame, &u.tri, d, min) else {
            return;
        };
        let cover = if t.tri.n[1].abs() >= 0.7 {
            limits.cover_floor
        } else {
            limits.cover_wall
        };
        let (a, b) = (
            &self.placements[t.placement as usize],
            &self.placements[u.placement as usize],
        );
        let nearest = if t.placement == u.placement {
            a.ranges[usize::from(t.lod)].0
        } else {
            together(a, usize::from(t.lod), b, usize::from(u.lod)).unwrap_or(0.0)
        };
        let hidden = limits.behind.max(limits.bias_per_metre * nearest);
        let reach = if tally.bases.contains(&(t.placement, u.placement)) {
            limits.reach
        } else {
            cover
        };
        if let Some(piece) = over.band(hidden, reach, min) {
            self.add(
                tally,
                t,
                Check::Lod,
                area(&piece) * frame.scale,
                over.range().1,
                frame.point(piece[0]),
                Some(u),
            );
        }
    }
}

/// What the gate reads of the game: the bundles of `dingolevel_root` and `globals.toc` (where a program's
/// permutations are looked up), the program lookup of `dingolevel_root` (its records and its rows), and the name of
/// every MeshSet of every TOC (lower case: a mesh of the game, [`Mesh::stock`](crate::postcompile::level::Mesh)).
#[derive(Debug)]
pub struct Stock {
    pub bundles: Vec<Bundle>,
    pub records: Vec<ProgramRecord>,
    pub rows: BTreeMap<u64, Vec<u32>>,
    pub meshes: BTreeSet<String>,
}

impl Stock {
    /// Reads it from the game's data (`game`).
    pub fn read(game: &Source) -> Result<Stock> {
        let root = game.listing(ROOT_TOC)?;
        let database = root
            .entries()
            .find(|e| e.kind == Kind::Res && e.resource_type == Some(lookup::PROGRAM_LOOKUP))
            .ok_or_else(|| Error::Missing(format!("{ROOT_TOC} holds no program lookup")))?;
        let home = root
            .bundles
            .iter()
            .find(|b| b.name == database.bundle)
            .ok_or_else(|| Error::Missing(format!("{ROOT_TOC} has no bundle {}", database.bundle)))?;
        let payload = game.payload(database.file, database.size as usize)?;
        let meta = home.manifest.res[database.index].rmeta;
        let records = lookup::read_records(&payload, &meta)?;
        let rows = lookup::read_programs(&payload, &meta)?.rows;
        let mut meshes = BTreeSet::new();
        for toc in &game.folder.tocs {
            for b in &game.listing(toc)?.bundles {
                meshes.extend(
                    b.manifest
                        .res
                        .iter()
                        .filter(|a| a.rtype == MESHSET)
                        .map(|a| a.name.to_ascii_lowercase()),
                );
            }
        }
        let mut bundles = root.bundles;
        bundles.extend(game.listing("Win32/globals.toc")?.bundles);
        Ok(Stock {
            bundles,
            records,
            rows,
            meshes,
        })
    }

    /// The class of every material key of `level` (see [`class_of`]): each program of the key's row, its raster
    /// permutations' states; a program index past the records is one the game does not have ([`Class::Unknown`]).
    /// A section of positions alone is a depth copy whatever its key ([`check`]).
    pub fn classes(&self, game: &Source, level: &Level) -> Result<BTreeMap<u64, Class>> {
        let scope = Scope::new(&self.bundles);
        let mut states: BTreeMap<u32, Vec<FixedState>> = BTreeMap::new();
        let mut out = BTreeMap::new();
        for (key, row) in &level.rows {
            let mut all = Vec::new();
            let mut known = true;
            for &p in row {
                let Some(record) = self.records.get(p as usize) else {
                    known = false;
                    break;
                };
                if let std::collections::btree_map::Entry::Vacant(e) = states.entry(p) {
                    e.insert(program_states(game, &scope, record)?);
                }
                all.extend(states[&p].iter().copied());
            }
            out.insert(*key, class_of(known.then_some(all.as_slice())));
        }
        Ok(out)
    }
}

/// The gate on the Patch of `src` (its base: the game): reads what it draws ([`Level`]), classes its materials from
/// the game's programs ([`Stock`]) and runs every check ([`check`]).
pub fn gate(src: &Source, limits: &Limits) -> Result<Report> {
    let folder = src
        .base
        .ok_or_else(|| Error::Missing("the game's data (the Patch's base)".into()))?;
    let game = Source {
        folder,
        base: Some(folder),
        oodle: src.oodle,
    };
    let stock = Stock::read(&game)?;
    let level = Level::read(src, &stock.rows, &stock.meshes)?;
    let classes = stock.classes(&game, &level)?;
    Ok(check(&level, &classes, limits))
}
