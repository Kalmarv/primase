//! `mark-shaunwhite-decals{gap=M,area=A,share=F}`: the blended materials the game lays on a surface, marked as decals
//! (`core.draw_order`'s decal layer).
//!
//! The game states no decal. A graffiti, a road marking and a pane of glass are the same blended Material (blend 4,
//! alpha blending on, depth write off, the same stored `ekgeneric` render states; none of the six levels' shader
//! records sets a depth bias or slope bias), and no field separates them: the one that correlates,
//! `InfluenceHidePixelMode`, is the restoration mechanic's pixel kill (18 of the 95 decal-named Materials of the six
//! levels lack it; water splashes and window lights have it). The game draws every blended Material after the opaque
//! surfaces with depth write off, so a decal laid on the floor in the same plane shows over it with no depth fight
//! whatever the depth values (the metro's blue arrow lies from 0.1 mm under to 0.6 mm over its floor tile). A
//! destination that draws a blended surface as a mask (skate's compiled shaders draw no blended world surface of
//! their own: `prepare-reskate-blended-surfaces`) writes its depth, and the two coplanar surfaces fight: the decal
//! dissolves into fragments of the floor.
//!
//! **Which materials.** A candidate is a Material whose `pbr.material` is blended, which is not water or vegetation
//! (`core.material_usage`) or a `core.layered` blend, and for which the source states no draw order.
//!
//! **The rule.** A placed triangle of a candidate is *laid on* a surface when its centroid, projected along a surface
//! triangle's normal, falls inside that triangle, the two face the same way (normals within 8 degrees), and the
//! centroid lies within `gap` metres of the triangle's plane on either side (a decal 1.5 mm under the floor fights as
//! it does 1.5 mm over it). A surface is a triangle of a primitive every material of which is opaque (no blending, no
//! alpha test, not a decal; a `core.layered` blend counting as its base layer's material), in the world frame (a
//! placement is an entity with `core.instance`; a mesh no placement takes is placed by its own `core.transform`).
//! A candidate is marked when its triangles laid on a surface add up to at least `area` square metres, summed over
//! every placement, and to at least `share` of its placed area (a pane of glass that touches a frame in a few places
//! is not a decal: no stored field or gap tells them apart, so the share keeps a material that lies on a surface in
//! passing out): the Material is marked whole, as `core.draw_order` belongs to the material, and a placement of it
//! that lies on nothing is drawn as a decal too. A marked Material becomes [`DrawLayer::Decal`] at [`DECAL_QUEUE`]
//! without depth write ([`DrawOrder::conventional`]) and stays a blended `pbr.material`.
//!
//! `gap` is the depth the destination cannot tell apart (the program's choice: millimetres; Studio's overlay stack
//! is 4 mm apart). Decals the game offsets by centimetres do not fight there and are not found by it.
//!
//! **What this is not.** A judgement by placement, not a stored fact, and an approximation of the game's own order (blended
//! after the opaque pass, depth write off). A glass pane or glow laid on a wall within `gap` is marked too, as it is
//! drawn over the wall the same way. A decal laid on a surface that is itself alpha-tested or blended is not found (it
//! is not a surface here), nor one whose placement mirrors the winding (its normal reverses against the surface's).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Component, DrawLayer, DrawOrder, Entity, Instance, LayeredMaterial, MaterialUsage, MaterialVariants,
    Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World,
};

/// The queue a decal comes at: after the opaque and cutout layers, before the blended one ([`DrawOrder`]); the
/// convention of every other lowering that marks decals.
pub const DECAL_QUEUE: i32 = 2500;
/// The least cosine between a candidate's normal and its surface's (8 degrees).
const FACING: f64 = 0.99;
/// The hash cell of the candidates' centroids, metres.
const CELL: f64 = 1.0;
/// Barycentric slack of the projected centroid being inside a surface triangle (a centroid on a shared edge belongs to
/// both triangles).
const INSIDE: f64 = 1.0e-6;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkShaunwhiteDecals {
    gap: f64,
    area: f64,
    share: f64,
}

impl MarkShaunwhiteDecals {
    /// Candidates with at least `area` square metres (finite, above 0) of placed triangles within `gap` metres (finite,
    /// above 0) of a surface, which are at least `share` (in (0, 1]) of the material's placed area, are marked.
    pub fn new(gap: f64, area: f64, share: f64) -> Result<MarkShaunwhiteDecals, String> {
        if !(gap.is_finite() && gap > 0.0) {
            return Err(format!("gap={gap}: finite metres above 0"));
        }
        if !(area.is_finite() && area > 0.0) {
            return Err(format!("area={area}: finite square metres above 0"));
        }
        if !(share.is_finite() && share > 0.0 && share <= 1.0) {
            return Err(format!("share={share}: a fraction in (0, 1]"));
        }
        Ok(MarkShaunwhiteDecals { gap, area, share })
    }

    fn from_given(g: &Given) -> Result<MarkShaunwhiteDecals, String> {
        let number = |name: &str| -> Result<f64, String> {
            let v = g.required(name);
            v.parse().map_err(|_| format!("{name}={v} is not a number"))
        };
        MarkShaunwhiteDecals::new(number("gap")?, number("area")?, number("share")?)
    }
}

/// `mark-shaunwhite-decals`' declaration: both numbers are the program's choice, no default.
pub const MARK_SHAUNWHITE_DECALS: PassSpec = PassSpec {
    name: "mark-shaunwhite-decals",
    options: &[
        OptionSpec::required(
            "gap",
            "0.005",
            "metres, finite and above 0: how far from an opaque surface's plane, either side, a blended material's triangle lies to be laid on it",
        ),
        OptionSpec::required(
            "area",
            "1",
            "square metres, finite and above 0: the least area of a blended material's placed triangles laid on a surface, over every placement, for it to be marked",
        ),
        OptionSpec::required(
            "share",
            "0.1",
            "in (0, 1]: the least share of a blended material's placed area that is laid on a surface for it to be marked",
        ),
    ],
    make: |g| Ok(Box::new(MarkShaunwhiteDecals::from_given(g)?)),
};

/// How much of a candidate material is placed, and how much of that is laid on a surface within `gap`.
#[derive(Clone, Debug, PartialEq)]
pub struct Survey {
    pub material: Entity,
    pub name: String,
    /// Square metres placed (every placement counted).
    pub area: f64,
    /// Of which, square metres laid on a surface.
    pub laid: f64,
}

impl Survey {
    /// The share of the placed area that is laid on a surface (0 for a material with no area).
    pub fn share(&self) -> f64 {
        if self.area > 0.0 {
            self.laid / self.area
        } else {
            0.0
        }
    }
}

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

/// Whether a candidate may be marked: blended, not water or vegetation, not layered, and with no stated draw order.
fn candidate(w: &World, m: Entity) -> bool {
    w.get::<PbrMaterial>(m)
        .is_some_and(|p| p.alpha_mode == AlphaMode::Blend)
        && !w.has::<MaterialUsage>(m)
        && !w.has::<LayeredMaterial>(m)
        && !w.has::<DrawOrder>(m)
}

/// Whether a material is a surface a decal lies on: opaque, not a decal, a layered blend as its base layer.
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

/// Every material some drawing of `primitive` (the `index`th of `mesh`) uses: its own and its variants'.
fn drawn_with(w: &World, mesh: Entity, index: usize, primitive: &Primitive) -> Vec<Entity> {
    primitive
        .material
        .into_iter()
        .chain(
            w.get::<MaterialVariants>(mesh)
                .and_then(|v| v.0.get(index))
                .into_iter()
                .flat_map(|v| v.values().copied()),
        )
        .collect()
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

/// The placed triangles (world frame, f64) of a triangle-list primitive, with a unit normal and the area; degenerate
/// ones are left out.
fn triangles(p: &Primitive, t: &Transform) -> Vec<([V3; 3], V3, f64)> {
    if p.topology != Topology::Triangles {
        return Vec::new();
    }
    let Some(Stream::F32x3(pos)) = p.streams.get(&Semantic::Position) else {
        return Vec::new();
    };
    let ix: Vec<u32> = match &p.indices {
        Some(ix) => ix.clone(),
        None => (0..pos.len() as u32).collect(),
    };
    let mut out = Vec::new();
    for c in ix.as_chunks::<3>().0 {
        let Some(corners) = c
            .iter()
            .map(|&i| pos.get(i as usize).map(|&x| t.apply(x).map(f64::from)))
            .collect::<Option<Vec<V3>>>()
        else {
            continue;
        };
        let c = [corners[0], corners[1], corners[2]];
        let n = cross(sub(c[1], c[0]), sub(c[2], c[0]));
        let len = dot(n, n).sqrt();
        if len <= 1e-12 || !len.is_finite() || !c.iter().flatten().all(|x| x.is_finite()) {
            continue;
        }
        out.push((c, n.map(|x| x / len), len / 2.0));
    }
    out
}

/// A placed triangle of a candidate: its centroid, unit normal, area, and the candidate materials it is of.
struct Placed {
    centroid: V3,
    n: V3,
    area: f64,
    materials: Vec<Entity>,
    laid: bool,
}

/// How much of each candidate material of `w` is placed and how much of that is laid on a surface within `gap`
/// metres either side of it: the numbers [`MarkShaunwhiteDecals`] judges. Materials with no placed
/// triangle are not listed.
pub fn survey(w: &World, gap: f64) -> Vec<Survey> {
    let placed = placements(w);
    let mut is_candidate: HashMap<Entity, bool> = HashMap::new();
    let mut is_surface: HashMap<Entity, bool> = HashMap::new();
    let mut cands: Vec<Placed> = Vec::new();
    // pass 1: the candidates' triangles (few), hashed by the cell of their centroid
    for &(mesh, t) in &placed {
        let Some(m) = w.get::<Mesh>(mesh) else { continue };
        for (k, p) in m.primitives().iter().enumerate() {
            let drawn = drawn_with(w, mesh, k, p);
            let materials: Vec<Entity> = drawn
                .into_iter()
                .filter(|&e| *is_candidate.entry(e).or_insert_with(|| candidate(w, e)))
                .collect();
            if materials.is_empty() {
                continue;
            }
            for (c, n, area) in triangles(p, &t) {
                cands.push(Placed {
                    centroid: std::array::from_fn(|a| (c[0][a] + c[1][a] + c[2][a]) / 3.0),
                    n,
                    area,
                    materials: materials.clone(),
                    laid: false,
                });
            }
        }
    }
    let cell = |x: f64| (x / CELL).floor() as i64;
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        grid.entry((cell(c.centroid[0]), cell(c.centroid[1]), cell(c.centroid[2])))
            .or_default()
            .push(i as u32);
    }
    // pass 2: every surface triangle looks up the candidates near it
    let reach = gap;
    if !cands.is_empty() {
        for &(mesh, t) in &placed {
            let Some(m) = w.get::<Mesh>(mesh) else { continue };
            for (k, p) in m.primitives().iter().enumerate() {
                let drawn = drawn_with(w, mesh, k, p);
                if drawn.is_empty()
                    || !drawn
                        .iter()
                        .all(|&e| *is_surface.entry(e).or_insert_with(|| surface(w, e)))
                {
                    continue;
                }
                for (c, n, _) in triangles(p, &t) {
                    let lo: [i64; 3] = std::array::from_fn(|a| {
                        cell(c.iter().map(|x| x[a]).fold(f64::INFINITY, f64::min) - reach)
                    });
                    let hi: [i64; 3] = std::array::from_fn(|a| {
                        cell(c.iter().map(|x| x[a]).fold(f64::NEG_INFINITY, f64::max) + reach)
                    });
                    let cells = (hi[0] - lo[0] + 1)
                        .saturating_mul(hi[1] - lo[1] + 1)
                        .saturating_mul(hi[2] - lo[2] + 1);
                    let mut near: Vec<u32> = Vec::new();
                    if cells as usize > cands.len() {
                        // a triangle covering more cells than there are candidates: look at each candidate
                        near.extend(0..cands.len() as u32);
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
                        let cand = &mut cands[j as usize];
                        if cand.laid || dot(n, cand.n) < FACING {
                            continue;
                        }
                        let d = dot(n, sub(cand.centroid, c[0]));
                        if !(-gap..=gap).contains(&d) {
                            continue;
                        }
                        let q = sub(cand.centroid, c[0]);
                        let (s, tt) = (dot(cross(q, e2), normal) / nn, dot(cross(e1, q), normal) / nn);
                        if s >= -INSIDE && tt >= -INSIDE && s + tt <= 1.0 + INSIDE {
                            cand.laid = true;
                        }
                    }
                }
            }
        }
    }
    let mut by: BTreeMap<Entity, Survey> = BTreeMap::new();
    for c in &cands {
        for &m in &c.materials {
            let s = by.entry(m).or_insert_with(|| Survey {
                material: m,
                name: w.get::<Name>(m).map(|n| n.0.clone()).unwrap_or_default(),
                area: 0.0,
                laid: 0.0,
            });
            s.area += c.area;
            if c.laid {
                s.laid += c.area;
            }
        }
    }
    by.into_values().collect()
}

impl Pass for MarkShaunwhiteDecals {
    fn name(&self) -> &'static str {
        "mark-shaunwhite-decals"
    }
    fn options(&self) -> Options {
        Options::from([
            ("gap".into(), self.gap.to_string()),
            ("area".into(), self.area.to_string()),
            ("share".into(), self.share.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[DrawOrder::NAME]
    }
    fn approximates(&self) -> &'static str {
        "blended materials are marked as decals by where they are placed (laid on an opaque surface), not by a stored field: the game states none and draws every blended material after the opaque ones with depth write off; in the six levels' census five marked materials are not decals but lie on a surface the same way (the window glows 02_Building_TimeSquare01B_ShellLight01 and Gen_Window_Downtown02_ShellLight02, GEN_GFX_Glow_Line, the ground light projection GEN_GFX_INF_Ministry_Proj01 and GFX_GlassBulb), drawn as decals too"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        for s in survey(w, self.gap) {
            if s.laid >= self.area && s.share() >= self.share {
                w.insert(s.material, DrawOrder::conventional(DrawLayer::Decal, DECAL_QUEUE));
                log.approximate_on(
                    s.material,
                    "blended materials laid on a surface marked as decals (by placement; the game draws them blended after the opaque pass)",
                    1,
                );
            } else {
                log.count("blended materials not laid on a surface, not marked", 1);
            }
        }
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

    fn material(w: &mut World, name: &str, mode: AlphaMode) -> Entity {
        let m = w.spawn();
        w.insert(
            m,
            PbrMaterial {
                alpha_mode: mode,
                ..PbrMaterial::default()
            },
        );
        w.insert(m, Name(name.into()));
        m
    }

    /// A `size` metre square facing up, its corner at (x, y, 0), in the world by its own transform.
    fn quad(w: &mut World, material: Entity, x: f32, y: f32, size: f32) -> Entity {
        let p = Primitive {
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
        };
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![p]));
        w.insert(
            e,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [x, y, 0.]],
            },
        );
        e
    }

    fn marked(w: &World, m: Entity) -> bool {
        DrawOrder::of(w, m).layer == DrawLayer::Decal
    }

    fn run(w: &mut World, options: &str) -> RunReport {
        let mut registry = PassRegistry::new();
        crate::register(&mut registry);
        let pipeline = registry
            .parse(&format!("mark-shaunwhite-decals{{{options}}}"))
            .unwrap();
        let mut report = RunReport::default();
        PassManager::new(registry)
            .with_pass_checks(true)
            .run(w, &pipeline, &mut report)
            .unwrap();
        report
    }

    const LEVEL: &str = "gap=0.005,area=1,share=0.1";

    /// A blended material 0.1 mm over a floor, and one 1.5 mm under it (the metro's decals lie either side of
    /// their floor), is marked; the floor, which is opaque, is not; the report names what was marked as an
    /// approximation, and the decal stays blended.
    #[test]
    fn a_blended_material_in_the_floors_plane_is_a_decal() {
        let mut w = World::new();
        let floor = material(&mut w, "floor", AlphaMode::Opaque);
        let over = material(&mut w, "over", AlphaMode::Blend);
        let under = material(&mut w, "under", AlphaMode::Blend);
        quad(&mut w, floor, 0.0, 0.0, 10.0);
        quad(&mut w, over, 2.0, 0.0001, 1.0);
        quad(&mut w, under, 5.0, -0.0015, 1.0);
        let report = run(&mut w, LEVEL);
        assert!(marked(&w, over) && marked(&w, under) && !marked(&w, floor));
        let d = DrawOrder::of(&w, over);
        assert_eq!((d.queue, d.depth_write), (DECAL_QUEUE, false));
        assert!(w.get::<PbrMaterial>(over).unwrap().alpha_mode == AlphaMode::Blend);
        let log = &report.steps[0].log;
        let note = log
            .approximated_on
            .iter()
            .find(|(k, _)| k.starts_with("blended materials laid on a surface"))
            .unwrap();
        assert_eq!(note.1.len(), 2);
    }

    /// Glass past the reach, or facing away from the surface (a mirrored placement), is not laid on it.
    #[test]
    fn glass_that_is_not_laid_on_a_surface_is_not_marked() {
        let mut w = World::new();
        let floor = material(&mut w, "floor", AlphaMode::Opaque);
        let far = material(&mut w, "far", AlphaMode::Blend);
        let behind = material(&mut w, "behind", AlphaMode::Blend);
        let away = material(&mut w, "away", AlphaMode::Blend);
        quad(&mut w, floor, 0.0, 0.0, 10.0);
        quad(&mut w, far, 1.0, 0.02, 1.0);
        quad(&mut w, behind, 3.0, -0.02, 1.0);
        // the same square mirrored on its x axis: its winding, so its normal, reverses
        let e = quad(&mut w, away, 6.0, 0.0001, 1.0);
        w.insert(
            e,
            Transform {
                cols: [[-1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [7.0, 0.0001, 0.]],
            },
        );
        run(&mut w, LEVEL);
        assert!(!marked(&w, far) && !marked(&w, behind) && !marked(&w, away));
    }

    /// The area laid on a surface, over every placement, decides: a square metre of a material that is also placed
    /// in the air marks it, half a square metre does not.
    #[test]
    fn the_area_laid_on_a_surface_decides() {
        let mut w = World::new();
        let floor = material(&mut w, "floor", AlphaMode::Opaque);
        let some = material(&mut w, "some", AlphaMode::Blend);
        let little = material(&mut w, "little", AlphaMode::Blend);
        let off = material(&mut w, "off", AlphaMode::Blend);
        quad(&mut w, floor, 0.0, 0.0, 4.0);
        quad(&mut w, some, 1.0, 0.001, 1.0);
        quad(&mut w, some, 20.0, 0.001, 1.0);
        quad(&mut w, little, 1.0, 0.001, 0.7);
        quad(&mut w, off, 30.0, 0.001, 1.0);
        let all = survey(&w, 0.005);
        let find = |m| all.iter().find(|s| s.material == m).unwrap();
        assert!((find(some).area - 2.0).abs() < 1e-9 && (find(some).laid - 1.0).abs() < 1e-9);
        assert!((find(some).share() - 0.5).abs() < 1e-9);
        assert!((find(little).laid - 0.49).abs() < 1e-6 && find(off).laid == 0.0);
        let report = run(&mut w, LEVEL);
        assert!(marked(&w, some) && !marked(&w, little) && !marked(&w, off));
        assert_eq!(
            report.steps[0].log.counts["blended materials not laid on a surface, not marked"],
            2
        );
    }

    /// A material that touches a surface in passing (a square metre laid among thousands placed) is not a decal.
    #[test]
    fn a_share_too_small_leaves_a_material_unmarked() {
        let mut w = World::new();
        let floor = material(&mut w, "floor", AlphaMode::Opaque);
        let glass = material(&mut w, "glass", AlphaMode::Blend);
        quad(&mut w, floor, 0.0, 0.0, 4.0);
        quad(&mut w, glass, 1.0, 0.001, 1.0);
        quad(&mut w, glass, 20.0, 5.0, 4.0);
        quad(&mut w, glass, 30.0, 5.0, 4.0);
        let s = survey(&w, 0.005);
        assert!((s[0].laid - 1.0).abs() < 1e-9 && s[0].share() < 0.1, "{s:?}");
        run(&mut w, LEVEL);
        assert!(!marked(&w, glass));
        run(&mut w, "gap=0.005,area=1,share=0.03");
        assert!(marked(&w, glass));
    }

    /// Only an opaque, undecorated surface is a surface: a decal over an alpha-tested or blended one is laid on
    /// nothing; vegetation, water and a material with a stated draw order are not candidates.
    #[test]
    fn what_is_not_a_surface_or_not_a_candidate_is_left_alone() {
        let mut w = World::new();
        let mask = material(&mut w, "mask", AlphaMode::Mask { cutoff: 0.5 });
        let glass = material(&mut w, "glass", AlphaMode::Blend);
        let on_mask = material(&mut w, "on_mask", AlphaMode::Blend);
        let on_glass = material(&mut w, "on_glass", AlphaMode::Blend);
        quad(&mut w, mask, 0.0, 0.0, 4.0);
        quad(&mut w, on_mask, 1.0, 0.001, 1.5);
        quad(&mut w, glass, 10.0, 0.0, 4.0);
        quad(&mut w, on_glass, 11.0, 0.001, 1.5);
        let floor = material(&mut w, "floor", AlphaMode::Opaque);
        quad(&mut w, floor, 20.0, 0.0, 10.0);
        let leaf = material(&mut w, "leaf", AlphaMode::Blend);
        w.insert(leaf, MaterialUsage::Foliage);
        let water = material(&mut w, "water", AlphaMode::Blend);
        w.insert(water, MaterialUsage::Water);
        let stated = material(&mut w, "stated", AlphaMode::Blend);
        let order = DrawOrder::conventional(DrawLayer::Transparent, 3100);
        w.insert(stated, order);
        for (k, m) in [leaf, water, stated].into_iter().enumerate() {
            quad(&mut w, m, 21.0 + 2.5 * k as f32, 0.001, 1.5);
        }
        run(&mut w, LEVEL);
        for m in [on_mask, on_glass, leaf, water] {
            assert!(!marked(&w, m));
        }
        assert_eq!(DrawOrder::of(&w, stated), order);
    }

    /// A placement is an entity with `core.instance`: a decal prototype placed standing on a wall, and again placed
    /// away from it, is judged by where it is placed, not where the prototype is.
    #[test]
    fn placements_are_judged_in_the_world_frame() {
        let mut w = World::new();
        let wall = material(&mut w, "wall", AlphaMode::Opaque);
        let tag = material(&mut w, "tag", AlphaMode::Blend);
        let proto = quad(&mut w, tag, 0.0, 0.0, 1.0);
        w.remove::<Transform>(proto);
        // the quad stood up: its normal (+y) to +z, so it faces the viewer of a wall in the plane z = 0
        let stand = |t: [f32; 3]| Transform {
            cols: [[1., 0., 0.], [0., 0., 1.], [0., -1., 0.], t],
        };
        let wall_mesh = quad(&mut w, wall, 0.0, 0.0, 10.0);
        w.insert(wall_mesh, stand([0., 0., 0.]));
        let place = |w: &mut World, t: Transform| {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(e, t);
        };
        // a tag 0.5 mm in front of the wall, a metre square on it (the standing quad spans y t.y - 1 to t.y)
        place(&mut w, stand([2.0, -2.0, 0.0005]));
        let all = survey(&w, 0.005);
        assert!((all[0].area - 1.0).abs() < 1e-9, "{all:?}");
        assert!((all[0].laid - 1.0).abs() < 1e-9, "{all:?}");
        let mut apart = w.clone();
        run(&mut w, LEVEL);
        assert!(marked(&w, tag));
        // the same tag 3 m off the wall lies on nothing
        let (e, _) = apart.iter::<Instance>().next().unwrap();
        apart.insert(e, stand([2.0, -2.0, 3.0]));
        assert_eq!(survey(&apart, 0.005)[0].laid, 0.0);
        run(&mut apart, LEVEL);
        assert!(!marked(&apart, tag));
    }

    #[test]
    fn the_options_are_validated() {
        for (gap, area, share) in [
            (0.0, 1.0, 0.1),
            (f64::NAN, 1.0, 0.1),
            (0.005, 0.0, 0.1),
            (0.005, f64::INFINITY, 0.1),
            (-1.0, 1.0, 0.1),
            (0.005, 1.0, 0.0),
            (0.005, 1.0, 1.5),
        ] {
            assert!(
                MarkShaunwhiteDecals::new(gap, area, share).is_err(),
                "{gap} {area} {share}"
            );
        }
        assert!(MarkShaunwhiteDecals::new(0.005, 1.0, 1.0).is_ok());
        let mut registry = PassRegistry::new();
        crate::register(&mut registry);
        assert!(
            registry
                .parse("mark-shaunwhite-decals{gap=0.005,area=1}")
                .is_err()
        );
        assert!(
            registry
                .parse("mark-shaunwhite-decals{gap=x,area=1,share=0.1}")
                .is_err()
        );
    }
}
