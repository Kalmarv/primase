//! `prepare-reskate-laid-layers{gap=M,share=F}`: the layers a world lays on its surfaces drawn as decals, whatever read
//! the world.
//!
//! skate. draws a decal (Studio's decal class) with depth test GreaterEqual, no depth write and a depth bias (200,
//! slope 0.1), after the surfaces beneath it, so a layer drawn as a decal in its surface's plane shows over it at every
//! distance. Every other class writes depth: an alpha-tested or blended layer in its surface's plane fights it, the
//! nearer of the two winning pixel by pixel (the layering fixture's columns 1, 8 and 9, seen in game; the stored
//! positions of an alpha-tested section round by up to centimetres, so a few millimetres apart is not enough). Sources
//! seldom say which layers lie on a surface (most draw them blended after the opaque pass, or offset them by a depth
//! bias of their own), so this pass finds them by where they are placed: no reader need state anything for a map to
//! be drawn without that fight.
//!
//! **Layers and surfaces.** A *layer* is a material that is blended or alpha-tested (`pbr.material`), not water
//! (`core.material_usage`), not a blend of layers (`core.layered`: `lower-layered-reskate` draws those) and not a decal
//! already (`core.draw_order`; any other stated draw order is replaced, counted). A *surface* is a triangle of a
//! primitive every material of which is opaque and not a decal (a `core.layered` blend counting as its base layer's
//! material). Both are taken in the world frame: a placement is an entity with `core.instance`, a mesh no placement
//! takes is placed by its own `core.transform`.
//!
//! **Laid on.** A placed triangle of a layer is laid on a surface when its centroid, projected along a surface
//! triangle's normal, falls inside that triangle, the two face the same way (normals within 8 degrees), and the
//! centroid lies within `gap` metres of the triangle's plane, on either side. A mesh's triangle is drawn as a decal when
//! it is laid in at least `share` of the mesh's placements (a prototype placed both on a floor and in the air goes by
//! the share).
//!
//! **What changes.** It decides triangle by triangle, not material by material, so the result does not depend on how a
//! reader grouped triangles into primitives or materials. A primitive all of whose triangles are drawn as decals is
//! drawn with its layers' decal forms. A primitive with only some is split in two: the rest stays in place, and the laid
//! triangles become a new primitive after the mesh's others. Both have their vertices compacted, per-primitive and
//! per-vertex components follow, and the mesh's file link goes. A layer every drawing of which is drawn as a decal
//! becomes its own decal form. A layer also drawn elsewhere keeps that use and gets a copy (`<name>_laid`) for the laid
//! triangles. A decal form is `core.draw_order` decal at the conventional queue ([`DECAL_QUEUE`]) without depth write.
//! An alpha-tested one is drawn blended by its texture alpha, the form proven in game (Studio's decal class blends);
//! that is logged on the material as an approximation.
//!
//! **Report.** Each material drawn as a decal is logged by name with the area laid (over every placement) and the
//! largest distance of a laid triangle from its surface, and the totals are counted. The record is the resource
//! `skate.laid_layers` ([`SkateLaidLayers`]). `raise-skate` and `prepare-reskate-blended-surfaces` require it, so no
//! program raises a world into skate's dialect, or turns its blended surfaces into cutouts, before this pass has run.
//!
//! **Bounds.** A world written in parts is judged part by part, on the placements and surfaces each part holds. A layer
//! laid on a surface that is itself alpha-tested or blended is not found (it is not a surface here), nor one whose
//! placement mirrors the winding (its normal reverses against the surface's). A layer within `gap` *under* its surface
//! becomes a decal the depth test hides; the layering gate reports it as behind (lifting it onto its surface is not
//! this pass's work). A world with levels of detail (`core.lods`) is refused: one form per prototype is chosen first
//! (`select-lod`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Component, DrawLayer, DrawOrder, Entity, Instance, LayeredMaterial, Lods, MaterialUsage,
    MaterialVariants, Mesh, MeshFile, Name, Primitive, Resource, Semantic, Stream, Topology, Transform,
    World,
};

use crate::components::{LaidLayer, SkateLaidLayers};

/// The queue a decal form comes at: after the opaque and cutout layers, before the blended one ([`DrawOrder`]).
pub const DECAL_QUEUE: i32 = 2500;
/// The default `gap`, metres: Studio's overlay stack (4 mm) and a margin.
pub const DEFAULT_GAP: f64 = 0.005;
/// The default `share`: a triangle laid in at least half its mesh's placements.
pub const DEFAULT_SHARE: f64 = 0.5;
/// The least cosine between a layer's normal and its surface's (8 degrees).
const FACING: f64 = 0.99;
/// The hash cell of the layers' centroids, metres.
const CELL: f64 = 1.0;
/// Barycentric slack of the projected centroid being inside a surface triangle (a centroid on a shared edge belongs to
/// both triangles).
const INSIDE: f64 = 1.0e-6;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepareReskateLaidLayers {
    gap: f64,
    share: f64,
}

impl Default for PrepareReskateLaidLayers {
    fn default() -> Self {
        PrepareReskateLaidLayers {
            gap: DEFAULT_GAP,
            share: DEFAULT_SHARE,
        }
    }
}

impl PrepareReskateLaidLayers {
    /// Layers' triangles within `gap` metres (finite, above 0) of a surface in at least `share` (in (0, 1]) of their
    /// mesh's placements are drawn as decals.
    pub fn new(gap: f64, share: f64) -> Result<PrepareReskateLaidLayers, String> {
        if !(gap.is_finite() && gap > 0.0) {
            return Err(format!("gap={gap}: finite metres above 0"));
        }
        if !(share.is_finite() && share > 0.0 && share <= 1.0) {
            return Err(format!("share={share}: a fraction in (0, 1]"));
        }
        Ok(PrepareReskateLaidLayers { gap, share })
    }

    fn from_given(g: &Given) -> Result<PrepareReskateLaidLayers, String> {
        let number = |name: &str, default: f64| -> Result<f64, String> {
            g.optional(name).map_or(Ok(default), |v| {
                v.parse().map_err(|_| format!("{name}={v} is not a number"))
            })
        };
        PrepareReskateLaidLayers::new(number("gap", DEFAULT_GAP)?, number("share", DEFAULT_SHARE)?)
    }
}

/// `prepare-reskate-laid-layers`' declaration: both numbers have defaults, so a plan may choose it.
pub const PREPARE_RESKATE_LAID_LAYERS: PassSpec = PassSpec {
    name: "prepare-reskate-laid-layers",
    options: &[
        OptionSpec::optional(
            "gap",
            "0.005",
            "metres, finite and above 0: how far from a surface's plane, either side, a layer's triangle lies to be laid \
             on it",
            "0.005",
        ),
        OptionSpec::optional(
            "share",
            "0.5",
            "in (0, 1]: the least share of its mesh's placements in which a layer's triangle is laid for it to be drawn \
             as a decal",
            "0.5",
        ),
    ],
    make: |g| Ok(Box::new(PrepareReskateLaidLayers::from_given(g)?)),
};

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Point `p` placed by `t`, in f64 ([`Transform::apply`] without its rounding to f32: a placement kilometres out keeps
/// its sub-millimetre gaps).
fn place(t: &Transform, p: [f32; 3]) -> V3 {
    std::array::from_fn(|i| {
        f64::from(t.cols[3][i])
            + (0..3)
                .map(|j| f64::from(t.cols[j][i]) * f64::from(p[j]))
                .sum::<f64>()
    })
}

/// Whether `m` is a layer (see the module documentation).
fn layer(w: &World, m: Entity) -> bool {
    w.get::<PbrMaterial>(m)
        .is_some_and(|p| matches!(p.alpha_mode, AlphaMode::Blend | AlphaMode::Mask { .. }))
        && w.get::<MaterialUsage>(m).copied() != Some(MaterialUsage::Water)
        && !w.has::<LayeredMaterial>(m)
        && DrawOrder::of(w, m).layer != DrawLayer::Decal
}

/// Whether `m` draws a surface a layer lies on: opaque, not a decal, a layered blend as its base layer.
fn surface(w: &World, m: Entity) -> bool {
    let base = match w.get::<LayeredMaterial>(m) {
        Some(l) => match l.layers.first().and_then(|l| l.as_ref()) {
            Some(layer) => layer.material,
            None => return false,
        },
        None => m,
    };
    DrawOrder::of(w, m).layer != DrawLayer::Decal
        && DrawOrder::of(w, base).layer != DrawLayer::Decal
        && w.get::<PbrMaterial>(base)
            .is_some_and(|p| p.alpha_mode == AlphaMode::Opaque)
}

/// Every material some drawing of `mesh`'s primitive `index` uses: its own and its variants'.
fn drawn_with(w: &World, mesh: Entity, index: usize, primitive: &Primitive) -> Vec<Entity> {
    let mut out: Vec<Entity> = primitive.material.into_iter().collect();
    if let Some(v) = w.get::<MaterialVariants>(mesh).and_then(|v| v.0.get(index)) {
        out.extend(v.values().copied());
    }
    out.sort();
    out.dedup();
    out
}

/// The meshes with the transform each is placed by: a placement is an entity with `core.instance`, a mesh no
/// placement takes is placed by its own transform.
fn placements(w: &World) -> Vec<(Entity, Transform)> {
    let mut out = Vec::new();
    let mut taken = BTreeSet::new();
    for (e, i) in w.iter::<Instance>() {
        if w.has::<Mesh>(i.prototype) {
            out.push((
                i.prototype,
                w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY),
            ));
            taken.insert(i.prototype);
        }
    }
    for (e, _) in w.iter::<Mesh>() {
        if !taken.contains(&e) {
            out.push((e, w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY)));
        }
    }
    out
}

/// A primitive's float3 positions and its triangles as index triples.
type Triangles<'a> = (&'a [[f32; 3]], Vec<[u32; 3]>);

/// A triangle-list primitive's float3 positions and its triangles as index triples, in order; `None` for one that is
/// not (lines, points, other positions). A triple naming a vertex past the positions is refused by the caller.
fn triangles(p: &Primitive) -> Option<Triangles<'_>> {
    if p.topology != Topology::Triangles {
        return None;
    }
    let Some(Stream::F32x3(pos)) = p.streams.get(&Semantic::Position) else {
        return None;
    };
    let ix: Vec<u32> = match &p.indices {
        Some(ix) => ix.clone(),
        None => (0..pos.len() as u32).collect(),
    };
    Some((pos, ix.as_chunks::<3>().0.to_vec()))
}

/// A triangle placed: its corners, unit normal and area; `None` when degenerate or not finite.
fn placed(pos: &[[f32; 3]], tri: [u32; 3], t: &Transform) -> Option<([V3; 3], V3, f64)> {
    let c: [V3; 3] = [
        place(t, *pos.get(tri[0] as usize)?),
        place(t, *pos.get(tri[1] as usize)?),
        place(t, *pos.get(tri[2] as usize)?),
    ];
    let n = cross(sub(c[1], c[0]), sub(c[2], c[0]));
    let len = dot(n, n).sqrt();
    if len <= 1e-12 || !len.is_finite() || !c.iter().flatten().all(|x| x.is_finite()) {
        return None;
    }
    Some((c, n.map(|x| x / len), len / 2.0))
}

/// A primitive drawn with a layer: its mesh and index, its triangles, and per triangle what its placements found.
struct Drawn {
    mesh: Entity,
    primitive: usize,
    triangles: Vec<[u32; 3]>,
    /// Per triangle: placements in which it is laid, its area laid and placed (summed over placements, square
    /// metres), and the largest distance from its surface where laid (metres).
    stats: Vec<Stat>,
}

#[derive(Clone, Copy, Default)]
struct Stat {
    laid: u32,
    laid_area: f64,
    gap: f64,
}

/// A placed triangle of a layer: which [`Drawn`] and triangle it is, its centroid, unit normal and area, and the
/// nearest distance from a surface it is laid on.
struct Probe {
    drawn: u32,
    triangle: u32,
    centroid: V3,
    n: V3,
    area: f64,
    laid: Option<f64>,
}

/// What [`survey`] found: the primitives drawn with a layer, and each mesh's number of placements.
struct Survey {
    drawn: Vec<Drawn>,
    placed: HashMap<Entity, u32>,
}

/// Every layer's placed triangle, tested against every surface's placed triangle near it.
fn survey(w: &World, gap: f64) -> Result<Survey, String> {
    let placed_meshes = placements(w);
    let mut is_layer: HashMap<Entity, bool> = HashMap::new();
    let mut is_surface: HashMap<Entity, bool> = HashMap::new();
    let mut index: HashMap<(Entity, usize), u32> = HashMap::new();
    let mut drawn: Vec<Drawn> = Vec::new();
    let mut probes: Vec<Probe> = Vec::new();
    let mut placed_count: HashMap<Entity, u32> = HashMap::new();
    // pass 1: the layers' triangles (few), hashed by the cell of their centroids
    for &(mesh, t) in &placed_meshes {
        *placed_count.entry(mesh).or_default() += 1;
        let Some(m) = w.get::<Mesh>(mesh) else { continue };
        for (k, p) in m.primitives().iter().enumerate() {
            if !drawn_with(w, mesh, k, p)
                .into_iter()
                .any(|e| *is_layer.entry(e).or_insert_with(|| layer(w, e)))
            {
                continue;
            }
            let Some((pos, tris)) = triangles(p) else { continue };
            let d = *index.entry((mesh, k)).or_insert_with(|| {
                drawn.push(Drawn {
                    mesh,
                    primitive: k,
                    stats: vec![Stat::default(); tris.len()],
                    triangles: tris.clone(),
                });
                (drawn.len() - 1) as u32
            });
            for (i, &tri) in tris.iter().enumerate() {
                if let Some(v) = tri.iter().find(|&&v| v as usize >= pos.len()) {
                    return Err(format!(
                        "{mesh:?} primitive {k}: a triangle names vertex {v} of {}",
                        pos.len()
                    ));
                }
                let Some((c, n, area)) = placed(pos, tri, &t) else {
                    continue;
                };
                probes.push(Probe {
                    drawn: d,
                    triangle: i as u32,
                    centroid: std::array::from_fn(|a| (c[0][a] + c[1][a] + c[2][a]) / 3.0),
                    n,
                    area,
                    laid: None,
                });
            }
        }
    }
    let cell = |x: f64| (x / CELL).floor() as i64;
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for (i, c) in probes.iter().enumerate() {
        grid.entry((cell(c.centroid[0]), cell(c.centroid[1]), cell(c.centroid[2])))
            .or_default()
            .push(i as u32);
    }
    // pass 2: every surface triangle looks up the layers' triangles near it
    if !probes.is_empty() {
        for &(mesh, t) in &placed_meshes {
            let Some(m) = w.get::<Mesh>(mesh) else { continue };
            for (k, p) in m.primitives().iter().enumerate() {
                let materials = drawn_with(w, mesh, k, p);
                if materials.is_empty()
                    || !materials
                        .iter()
                        .all(|&e| *is_surface.entry(e).or_insert_with(|| surface(w, e)))
                {
                    continue;
                }
                let Some((pos, tris)) = triangles(p) else { continue };
                for tri in tris {
                    let Some((c, n, _)) = placed(pos, tri, &t) else {
                        continue;
                    };
                    let lo: [i64; 3] = std::array::from_fn(|a| {
                        cell(c.iter().map(|x| x[a]).fold(f64::INFINITY, f64::min) - gap)
                    });
                    let hi: [i64; 3] = std::array::from_fn(|a| {
                        cell(c.iter().map(|x| x[a]).fold(f64::NEG_INFINITY, f64::max) + gap)
                    });
                    let cells = (hi[0] - lo[0] + 1)
                        .saturating_mul(hi[1] - lo[1] + 1)
                        .saturating_mul(hi[2] - lo[2] + 1);
                    let mut near: Vec<u32> = Vec::new();
                    if cells as usize > probes.len() {
                        // a triangle covering more cells than there are probes: look at each probe
                        near.extend(0..probes.len() as u32);
                    } else {
                        for x in lo[0]..=hi[0] {
                            for y in lo[1]..=hi[1] {
                                for z in lo[2]..=hi[2] {
                                    near.extend(grid.get(&(x, y, z)).map(Vec::as_slice).unwrap_or(&[]));
                                }
                            }
                        }
                    }
                    let (e1, e2) = (sub(c[1], c[0]), sub(c[2], c[0]));
                    let normal = cross(e1, e2);
                    let nn = dot(normal, normal);
                    for j in near {
                        let probe = &mut probes[j as usize];
                        if dot(n, probe.n) < FACING {
                            continue;
                        }
                        let q = sub(probe.centroid, c[0]);
                        let d = dot(n, q).abs();
                        if d > gap || probe.laid.is_some_and(|l| l <= d) {
                            continue;
                        }
                        let (s, tt) = (dot(cross(q, e2), normal) / nn, dot(cross(e1, q), normal) / nn);
                        if s >= -INSIDE && tt >= -INSIDE && s + tt <= 1.0 + INSIDE {
                            probe.laid = Some(d);
                        }
                    }
                }
            }
        }
    }
    for p in &probes {
        if let Some(d) = p.laid {
            let s = &mut drawn[p.drawn as usize].stats[p.triangle as usize];
            s.laid += 1;
            s.laid_area += p.area;
            s.gap = s.gap.max(d);
        }
    }
    Ok(Survey {
        drawn,
        placed: placed_count,
    })
}

/// One primitive's verdict: the triangles drawn as decals and the rest, by triangle index.
struct Verdict {
    drawn: usize,
    decal: Vec<usize>,
    rest: Vec<usize>,
}

impl Pass for PrepareReskateLaidLayers {
    fn name(&self) -> &'static str {
        "prepare-reskate-laid-layers"
    }
    fn options(&self) -> Options {
        Options::from([
            ("gap".into(), self.gap.to_string()),
            ("share".into(), self.share.to_string()),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[DrawOrder::NAME, SkateLaidLayers::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            Mesh::NAME,
            MaterialVariants::NAME,
            PbrMaterial::NAME,
            DrawOrder::NAME,
            Name::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    /// The file links it removes are a side effect: the split meshes no longer match their files.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "layers are found by where they are placed (laid on an opaque surface within the gap), not by a stored field, \
         and drawn as decals; alpha-tested ones are drawn blended by their texture alpha instead of cut at their cutoff"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.iter::<Lods>().next().is_some() {
            return Err(
                "levels of detail (core.lods): choose one form per prototype first (select-lod)".into(),
            );
        }
        let survey = survey(w, self.gap)?;
        // the verdicts, primitive by primitive
        let mut verdicts: Vec<Verdict> = Vec::new();
        for (i, d) in survey.drawn.iter().enumerate() {
            let placements = f64::from(survey.placed.get(&d.mesh).copied().unwrap_or(0));
            let (decal, rest): (Vec<usize>, Vec<usize>) = (0..d.triangles.len())
                .partition(|&t| d.stats[t].laid > 0 && f64::from(d.stats[t].laid) >= self.share * placements);
            if !decal.is_empty() {
                verdicts.push(Verdict {
                    drawn: i,
                    decal,
                    rest,
                });
            }
        }
        // every primitive drawing each layer, to tell a layer drawn only as a decal from one also drawn elsewhere
        let mut uses: BTreeMap<Entity, BTreeSet<(Entity, usize)>> = BTreeMap::new();
        for (e, mesh) in w.iter::<Mesh>() {
            for (k, p) in mesh.primitives().iter().enumerate() {
                for m in drawn_with(w, e, k, p) {
                    if layer(w, m) {
                        uses.entry(m).or_default().insert((e, k));
                    }
                }
            }
        }
        let whole: BTreeSet<(Entity, usize)> = verdicts
            .iter()
            .filter(|v| v.rest.is_empty())
            .map(|v| (survey.drawn[v.drawn].mesh, survey.drawn[v.drawn].primitive))
            .collect();
        // each layer some verdict draws as a decal, with what it found
        let mut found: BTreeMap<Entity, LaidLayer> = BTreeMap::new();
        for v in &verdicts {
            let d = &survey.drawn[v.drawn];
            let p = &w.get::<Mesh>(d.mesh).expect("surveyed").primitives()[d.primitive];
            for m in drawn_with(w, d.mesh, d.primitive, p) {
                if !layer(w, m) {
                    continue;
                }
                let name = w.get::<Name>(m).map_or_else(|| format!("{m:?}"), |n| n.0.clone());
                let f = found.entry(m).or_insert_with(|| LaidLayer {
                    material: name.clone(),
                    decal: name,
                    triangles: 0,
                    area: 0.0,
                    gap: 0.0,
                    alpha_tested: matches!(
                        w.get::<PbrMaterial>(m).map(|p| p.alpha_mode),
                        Some(AlphaMode::Mask { .. })
                    ),
                });
                for &t in &v.decal {
                    f.triangles += 1;
                    f.area += d.stats[t].laid_area;
                    f.gap = f.gap.max(d.stats[t].gap);
                }
            }
        }
        log.count(
            "layers not laid on a surface, kept as they are",
            uses.keys().filter(|m| !found.contains_key(m)).count() as u64,
        );
        // each layer's decal form: itself when every drawing of it is drawn as a decal, else a copy
        let mut forms: BTreeMap<Entity, Entity> = BTreeMap::new();
        for (&m, f) in &mut found {
            let only_decal = uses.get(&m).is_some_and(|u| u.is_subset(&whole));
            let form = if only_decal {
                m
            } else {
                let copy = w
                    .duplicate(m)
                    .ok_or_else(|| format!("{}: not alive", f.material))?;
                f.decal = format!("{}_laid", f.material);
                w.insert(copy, Name(f.decal.clone()));
                copy
            };
            if w.has::<DrawOrder>(form) {
                log.count("stated draw orders replaced by a decal's (laid on a surface)", 1);
            }
            w.insert(form, DrawOrder::conventional(DrawLayer::Decal, DECAL_QUEUE));
            let pbr = w.get_mut::<PbrMaterial>(form).expect("a layer");
            if matches!(pbr.alpha_mode, AlphaMode::Mask { .. }) {
                pbr.alpha_mode = AlphaMode::Blend;
                log.approximate_on(
                    form,
                    "alpha-tested layers laid on a surface drawn as decals, blended by their texture alpha instead of \
                     cut at their cutoff",
                    1,
                );
            }
            forms.insert(m, form);
        }
        // the meshes: whole primitives take their layers' forms, split ones keep the rest and gain the laid triangles
        let mut by_mesh: BTreeMap<Entity, Vec<&Verdict>> = BTreeMap::new();
        for v in &verdicts {
            by_mesh.entry(survey.drawn[v.drawn].mesh).or_default().push(v);
        }
        let form_of = |m: Entity| forms.get(&m).copied().unwrap_or(m);
        for (e, list) in by_mesh {
            let mesh = w.get::<Mesh>(e).expect("surveyed").clone();
            let mut primitives: Vec<Primitive> = mesh.primitives().to_vec();
            let mut from: Vec<usize> = (0..primitives.len()).collect();
            let mut remaps: Vec<(usize, Vec<u32>)> = Vec::new();
            let mut retargeted: Vec<usize> = Vec::new();
            for v in &list {
                let d = &survey.drawn[v.drawn];
                let k = d.primitive;
                if v.rest.is_empty() {
                    primitives[k].material = primitives[k].material.map(form_of);
                    retargeted.push(k);
                    continue;
                }
                let flat = |ts: &[usize]| -> Vec<u32> { ts.iter().flat_map(|&t| d.triangles[t]).collect() };
                let source = &mesh.primitives()[k];
                let (rest, rest_from) = primase_meshops::subset_primitive_gathered(source, &flat(&v.rest))
                    .map_err(|why| format!("{e:?} primitive {k}: {why}"))?;
                let (mut laid, laid_from) =
                    primase_meshops::subset_primitive_gathered(source, &flat(&v.decal))
                        .map_err(|why| format!("{e:?} primitive {k}: {why}"))?;
                w.check_remap_vertices(e, k, &rest_from)?;
                w.check_remap_vertices(e, k, &laid_from)?;
                laid.material = laid.material.map(form_of);
                primitives[k] = rest;
                remaps.push((k, rest_from));
                remaps.push((primitives.len(), laid_from));
                retargeted.push(primitives.len());
                primitives.push(laid);
                from.push(k);
            }
            let split = from.len() > mesh.primitives().len();
            w.edit_mesh(e, |m| {
                *m = Mesh::new(primitives);
                Ok(split.then_some(from))
            })?;
            for (k, map) in remaps {
                w.remap_vertices(e, k, &map)?;
            }
            if let Some(variants) = w.get_mut::<MaterialVariants>(e) {
                for k in retargeted {
                    if let Some(row) = variants.0.get_mut(k) {
                        for m in row.values_mut() {
                            *m = form_of(*m);
                        }
                    }
                }
            }
            if split {
                w.remove::<MeshFile>(e);
                log.count(
                    "primitives split: laid triangles drawn as decals, the rest kept",
                    list.iter().filter(|v| !v.rest.is_empty()).count() as u64,
                );
            }
        }
        let mut layers: Vec<LaidLayer> = found.into_values().collect();
        layers.sort_by(|a, b| a.material.cmp(&b.material));
        // one line per name (sources reuse names: GTA's are its shaders'): how many materials, how many of them also
        // drawn where they are not laid (copied), their area laid and largest gap; the count is their triangles
        let mut by_name: BTreeMap<&str, (u64, u64, u64, f64, f64)> = BTreeMap::new();
        for l in &layers {
            let n = by_name.entry(l.material.as_str()).or_default();
            n.0 += 1;
            n.1 += u64::from(l.decal != l.material);
            n.2 += l.triangles;
            n.3 += l.area;
            n.4 = n.4.max(l.gap);
        }
        for (name, (materials, copies, triangles, area, gap)) in by_name {
            log.count(
                &format!(
                    "layer {name} drawn as a decal ({materials} material(s), {copies} also drawn elsewhere and copied \
                     as {name}_laid): {area:.3} m2 laid within {:.2} mm of a surface",
                    gap * 1000.0
                ),
                triangles,
            );
        }
        log.count("layers drawn as decals (laid on a surface)", layers.len() as u64);
        log.count(
            "layer triangles drawn as decals (laid on a surface)",
            layers.iter().map(|l| l.triangles).sum(),
        );
        w.insert_resource(SkateLaidLayers {
            gap: self.gap,
            share: self.share,
            layers,
        });
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "tests lay hand-made worlds"
)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};

    fn material(w: &mut World, name: &str, alpha_mode: AlphaMode) -> Entity {
        let m = w.spawn();
        w.insert(
            m,
            PbrMaterial {
                alpha_mode,
                ..PbrMaterial::default()
            },
        );
        w.insert(m, Name(name.into()));
        m
    }

    const MASK: AlphaMode = AlphaMode::Mask { cutoff: 0.5 };

    /// A square of `size` metres facing local +y from its corner at the origin (two triangles), drawn with `material`.
    fn square(material: Entity, size: f32) -> Primitive {
        Primitive {
            topology: Topology::Triangles,
            streams: [(
                Semantic::Position,
                Stream::F32x3(vec![
                    [0.0, 0.0, 0.0],
                    [size, 0.0, 0.0],
                    [0.0, 0.0, size],
                    [size, 0.0, size],
                ]),
            )]
            .into(),
            indices: Some(vec![0, 2, 1, 1, 2, 3]),
            material: Some(material),
        }
    }

    /// The primitive with every position moved `by` metres along local `axis`.
    fn shifted(mut p: Primitive, axis: usize, by: f32) -> Primitive {
        if let Some(Stream::F32x3(v)) = p.streams.get_mut(&Semantic::Position) {
            v.iter_mut().for_each(|x| x[axis] += by);
        }
        p
    }

    /// A frame tilted `degrees` about x (its +y, the squares' normal, leaning toward +z), at `origin`.
    fn tilted(degrees: f32, origin: [f32; 3]) -> Transform {
        let (s, c) = degrees.to_radians().sin_cos();
        Transform {
            cols: [[1., 0., 0.], [0., c, s], [0., -s, c], origin],
        }
    }

    /// A mesh of `primitives` placed by its own transform `t`.
    fn mesh(w: &mut World, primitives: Vec<Primitive>, t: Transform) -> Entity {
        let e = w.spawn();
        w.insert(e, Mesh::new(primitives));
        w.insert(e, t);
        e
    }

    fn decal(w: &World, m: Entity) -> bool {
        DrawOrder::of(w, m).layer == DrawLayer::Decal
    }

    fn run(w: &mut World, pipeline: &str) -> Result<RunReport, String> {
        let mut registry = PassRegistry::new();
        crate::register(&mut registry);
        let pipeline = registry.parse(pipeline).map_err(|e| e.to_string())?;
        let mut report = RunReport::default();
        PassManager::new(registry)
            .with_pass_checks(true)
            .run(w, &pipeline, &mut report)
            .map_err(|e| e.to_string())?;
        Ok(report)
    }

    fn counts(report: &RunReport) -> &BTreeMap<String, u64> {
        &report.steps[0].log.counts
    }

    /// The layering fixture's columns 1, 8 and 9: alpha-tested strips at 0 and 4 mm over a floor, a wall leaning
    /// 15 degrees and a 10 degree ramp, and a blended one 0.1 mm over the floor. Each becomes a decal at the decal
    /// queue without depth write; the alpha-tested ones blend by their texture alpha (logged on the material); each is
    /// logged by name with its area and gap, and the record lists them.
    #[test]
    fn layers_in_their_surfaces_plane_are_decals_whatever_the_slope() {
        let mut w = World::new();
        let ground = material(&mut w, "ground", AlphaMode::Opaque);
        let mut strips = Vec::new();
        for (i, degrees) in [0.0f32, 15.0, 10.0].into_iter().enumerate() {
            let origin = [20.0 * i as f32, 0.0, 0.0];
            mesh(&mut w, vec![square(ground, 10.0)], tilted(degrees, origin));
            for (k, mm) in [0.0f32, 4.0].into_iter().enumerate() {
                let m = material(&mut w, &format!("cut{i}_{mm}mm"), MASK);
                mesh(
                    &mut w,
                    vec![shifted(
                        shifted(square(m, 1.0), 1, mm / 1000.0),
                        0,
                        2.0 + 3.0 * k as f32,
                    )],
                    tilted(degrees, origin),
                );
                strips.push(m);
            }
        }
        let glaze = material(&mut w, "glaze", AlphaMode::Blend);
        mesh(
            &mut w,
            vec![shifted(square(glaze, 2.0), 1, 0.0001)],
            tilted(0.0, [0.0, 0.0, 5.0]),
        );
        strips.push(glaze);
        let report = run(&mut w, "prepare-reskate-laid-layers").unwrap();
        for &m in &strips {
            assert!(decal(&w, m), "{:?}", w.get::<Name>(m));
            let d = DrawOrder::of(&w, m);
            assert_eq!((d.queue, d.depth_write), (DECAL_QUEUE, false));
            assert_eq!(w.get::<PbrMaterial>(m).unwrap().alpha_mode, AlphaMode::Blend);
        }
        assert!(!decal(&w, ground));
        let c = counts(&report);
        assert_eq!(c["layers drawn as decals (laid on a surface)"], 7);
        assert_eq!(c["layer triangles drawn as decals (laid on a surface)"], 14);
        assert_eq!(
            c["layer cut1_4mm drawn as a decal (1 material(s), 0 also drawn elsewhere and copied as cut1_4mm_laid): \
               1.000 m2 laid within 4.00 mm of a surface"],
            2
        );
        assert_eq!(
            c["layer glaze drawn as a decal (1 material(s), 0 also drawn elsewhere and copied as glaze_laid): \
               4.000 m2 laid within 0.10 mm of a surface"],
            2
        );
        let alpha_tested = report.steps[0]
            .log
            .approximated_on
            .iter()
            .find(|(k, _)| k.starts_with("alpha-tested layers laid on a surface"))
            .unwrap();
        assert_eq!(alpha_tested.1.len(), 6);
        let record = w.resource::<SkateLaidLayers>().unwrap();
        assert_eq!(
            (record.gap, record.share, record.layers.len()),
            (DEFAULT_GAP, DEFAULT_SHARE, 7)
        );
        let cut = record.layers.iter().find(|l| l.material == "cut2_4mm").unwrap();
        assert!(cut.alpha_tested && cut.decal == "cut2_4mm" && cut.triangles == 2);
        assert!(
            (cut.gap - 0.004).abs() < 1e-6 && (cut.area - 1.0).abs() < 1e-6,
            "{cut:?}"
        );
    }

    /// An alpha-tested layer just beyond the gap (5.5 mm over a 5 mm gap) stays a cutout, untouched; one just inside it
    /// (4.9 mm) is a decal. One facing away from the floor (a mirrored placement) is not laid on it, nor is a blended
    /// one 2 cm under it.
    #[test]
    fn a_layer_just_beyond_the_gap_stays_a_cutout() {
        let mut w = World::new();
        let ground = material(&mut w, "ground", AlphaMode::Opaque);
        mesh(&mut w, vec![square(ground, 10.0)], Transform::IDENTITY);
        let beyond = material(&mut w, "beyond", MASK);
        let inside = material(&mut w, "inside", MASK);
        let away = material(&mut w, "away", MASK);
        let under = material(&mut w, "under", AlphaMode::Blend);
        mesh(
            &mut w,
            vec![shifted(square(beyond, 1.0), 1, 0.0055)],
            tilted(0.0, [1.0, 0.0, 1.0]),
        );
        mesh(
            &mut w,
            vec![shifted(square(inside, 1.0), 1, 0.0049)],
            tilted(0.0, [3.0, 0.0, 1.0]),
        );
        // the same square mirrored on its x axis: its winding, so its normal, reverses
        mesh(
            &mut w,
            vec![square(away, 1.0)],
            Transform {
                cols: [[-1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [6.0, 0.0001, 1.0]],
            },
        );
        mesh(
            &mut w,
            vec![shifted(square(under, 1.0), 1, -0.02)],
            tilted(0.0, [7.0, 0.0, 1.0]),
        );
        let report = run(&mut w, "prepare-reskate-laid-layers{gap=0.005}").unwrap();
        assert!(decal(&w, inside));
        for m in [beyond, away, under] {
            assert!(!w.has::<DrawOrder>(m), "{:?}", w.get::<Name>(m));
        }
        assert_eq!(w.get::<PbrMaterial>(beyond).unwrap().alpha_mode, MASK);
        assert_eq!(w.get::<PbrMaterial>(away).unwrap().alpha_mode, MASK);
        assert_eq!(
            counts(&report)["layers not laid on a surface, kept as they are"],
            3
        );
        let record = w.resource::<SkateLaidLayers>().unwrap();
        assert_eq!(
            record
                .layers
                .iter()
                .map(|l| l.material.as_str())
                .collect::<Vec<_>>(),
            ["inside"]
        );
    }

    /// A layer drawn both laid on the floor and as a free-standing card keeps the card and gets a copy for the laid
    /// square; the copy is the decal, named after it.
    #[test]
    fn a_layer_also_drawn_elsewhere_gets_a_decal_copy() {
        let mut w = World::new();
        let ground = material(&mut w, "ground", AlphaMode::Opaque);
        mesh(&mut w, vec![square(ground, 10.0)], Transform::IDENTITY);
        let leaves = material(&mut w, "leaves", MASK);
        let laid = mesh(&mut w, vec![square(leaves, 1.0)], tilted(0.0, [1.0, 0.002, 1.0]));
        let card = mesh(&mut w, vec![square(leaves, 1.0)], tilted(90.0, [4.0, 0.5, 1.0]));
        let report = run(&mut w, "prepare-reskate-laid-layers").unwrap();
        assert!(!decal(&w, leaves));
        assert_eq!(w.get::<PbrMaterial>(leaves).unwrap().alpha_mode, MASK);
        assert_eq!(
            w.get::<Mesh>(card).unwrap().primitives()[0].material,
            Some(leaves)
        );
        let copy = w.get::<Mesh>(laid).unwrap().primitives()[0].material.unwrap();
        assert_ne!(copy, leaves);
        assert!(decal(&w, copy));
        assert_eq!(w.get::<Name>(copy).unwrap().0, "leaves_laid");
        assert_eq!(w.get::<PbrMaterial>(copy).unwrap().alpha_mode, AlphaMode::Blend);
        assert!(counts(&report).contains_key(
            "layer leaves drawn as a decal (1 material(s), 1 also drawn elsewhere and copied as leaves_laid): 1.000 m2 \
             laid within 2.00 mm of a surface"
        ));
    }

    /// One primitive holding a laid square and a square a metre over the floor is split: the floating square stays in
    /// place, the laid one becomes a primitive after it, each with its own four vertices; the variant row follows, its
    /// layer retargeted to the decal copy on the laid primitive only; the file link goes.
    #[test]
    fn a_primitive_partly_laid_is_split() {
        let mut w = World::new();
        let ground = material(&mut w, "ground", AlphaMode::Opaque);
        mesh(&mut w, vec![square(ground, 10.0)], Transform::IDENTITY);
        let grime = material(&mut w, "grime", MASK);
        let rusty = material(&mut w, "rusty", MASK);
        let mut both = shifted(square(grime, 1.0), 1, 0.001);
        if let Some(Stream::F32x3(p)) = both.streams.get_mut(&Semantic::Position) {
            let floating: Vec<[f32; 3]> = p.iter().map(|x| [x[0] + 3.0, 1.0, x[2]]).collect();
            p.extend(floating);
        }
        both.indices = Some(vec![4, 6, 5, 5, 6, 7, 0, 2, 1, 1, 2, 3]);
        let e = mesh(&mut w, vec![both], tilted(0.0, [1.0, 0.0, 1.0]));
        w.insert(e, MaterialVariants(vec![BTreeMap::from([(1, rusty)])]));
        w.insert(
            e,
            MeshFile {
                path: "grime.glb".into(),
                mesh: 0,
                materials: vec![Some(grime)],
            },
        );
        let report = run(&mut w, "prepare-reskate-laid-layers").unwrap();
        let m = w.get::<Mesh>(e).unwrap();
        assert_eq!(m.primitives().len(), 2);
        let (rest, laid) = (&m.primitives()[0], &m.primitives()[1]);
        assert_eq!(rest.material, Some(grime));
        assert_eq!(
            (rest.vertex_count(), rest.indices.as_ref().unwrap().len()),
            (4, 6)
        );
        assert_eq!(
            (laid.vertex_count(), laid.indices.as_ref().unwrap().len()),
            (4, 6)
        );
        let Some(Stream::F32x3(lp)) = laid.streams.get(&Semantic::Position) else {
            panic!("positions")
        };
        assert!(lp.iter().all(|x| (x[1] - 0.001).abs() < 1e-6), "{lp:?}");
        let grime_laid = laid.material.unwrap();
        assert_eq!(w.get::<Name>(grime_laid).unwrap().0, "grime_laid");
        let variants = &w.get::<MaterialVariants>(e).unwrap().0;
        assert_eq!(variants[0][&1], rusty);
        let rusty_laid = variants[1][&1];
        assert_eq!(w.get::<Name>(rusty_laid).unwrap().0, "rusty_laid");
        assert!(decal(&w, grime_laid) && decal(&w, rusty_laid));
        assert!(!decal(&w, grime) && !decal(&w, rusty));
        assert!(!w.has::<MeshFile>(e));
        assert_eq!(
            counts(&report)["primitives split: laid triangles drawn as decals, the rest kept"],
            1
        );
    }

    /// A prototype placed once on the floor and once a metre over it is laid in half its placements: drawn as a decal
    /// at the default share (0.5), not at 0.6.
    #[test]
    fn the_share_decides_a_prototype_placed_on_and_off_a_surface() {
        for (share, decided) in [("0.5", true), ("0.6", false)] {
            let mut w = World::new();
            let ground = material(&mut w, "ground", AlphaMode::Opaque);
            mesh(&mut w, vec![square(ground, 10.0)], Transform::IDENTITY);
            let sticker = material(&mut w, "sticker", MASK);
            let proto = w.spawn();
            w.insert(proto, Mesh::new(vec![square(sticker, 1.0)]));
            for (x, y) in [(1.0, 0.001), (4.0, 1.0)] {
                let p = w.spawn();
                w.insert(
                    p,
                    Instance {
                        prototype: proto,
                        variant: 0,
                    },
                );
                w.insert(p, tilted(0.0, [x, y, 1.0]));
            }
            run(&mut w, &format!("prepare-reskate-laid-layers{{share={share}}}")).unwrap();
            assert_eq!(decal(&w, sticker), decided, "share {share}");
        }
    }

    /// Raising, and turning blended surfaces into cutouts, need the laid layers judged first: either pass without this
    /// one before it is refused, naming what it lacks; with it, it runs.
    #[test]
    fn raising_and_cutouts_need_the_layers_judged_first() {
        let blended = "prepare-reskate-blended-surfaces{cutoff=0.5,materials=all,uniform=cutout}";
        let world = || {
            let mut w = World::new();
            let ground = material(&mut w, "ground", AlphaMode::Opaque);
            mesh(&mut w, vec![square(ground, 10.0)], Transform::IDENTITY);
            let glass = material(&mut w, "glass", AlphaMode::Blend);
            mesh(&mut w, vec![square(glass, 1.0)], tilted(90.0, [1.0, 0.0, 1.0]));
            w
        };
        let refused = run(&mut world(), blended).unwrap_err();
        assert!(refused.contains("skate.laid_layers"), "{refused}");
        let refused = run(
            &mut world(),
            "raise-skate{name=t,light_scale=standard,unclassified=leave-out}",
        )
        .unwrap_err();
        assert!(refused.contains("skate.laid_layers"), "{refused}");
        run(&mut world(), &format!("prepare-reskate-laid-layers, {blended}")).unwrap();
    }

    /// Levels of detail are refused: one form per prototype is chosen first.
    #[test]
    fn levels_of_detail_are_refused() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(
            e,
            Lods {
                detail: 0,
                forms: Vec::new(),
                default: None,
            },
        );
        let refused = PrepareReskateLaidLayers::default()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(refused.contains("select-lod"), "{refused}");
    }
}
