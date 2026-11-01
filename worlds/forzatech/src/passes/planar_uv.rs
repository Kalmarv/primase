//! `lower-forzatech-planar-uv`: the texture coordinates of what is drawn with a material mapped by the position of a
//! surface point in the game's ground plane ([`PlanarUv`]: the terrain families', and the road layers the game maps by
//! the world), written per placement: each vertex's `TexCoord(set)` the point's `(x, z)` in the game's frame, which
//! the material's slots scale and offset as the shader does. A prototype placed more than once is copied for each
//! placement after its first (its level-of-detail forms with it), so each copy carries its own placement's
//! coordinates. Run after the meshes, materials and levels of detail are core's.
//!
//! Only primitives drawn with such materials alone, all of one set, are rewritten (one also drawn with another
//! material, through its variants, keeps its coordinates, counted).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::{
    Component, Entity, Instance, LodForm, Lods, MaterialVariants, Mesh, Semantic, Stream, Transform, World,
};

use crate::components::PlanarUv;

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerForzaPlanarUv;

/// The set every drawing of primitive `i` of `e` takes its planar coordinates in, if all its materials are planar
/// (`Err` names a mixed primitive).
fn primitive_set(
    w: &World,
    e: Entity,
    i: usize,
    material: Option<Entity>,
) -> Result<Option<u8>, &'static str> {
    let variants = w.get::<MaterialVariants>(e);
    let drawn: Vec<Entity> = material
        .into_iter()
        .chain(
            variants
                .and_then(|v| v.0.get(i))
                .into_iter()
                .flat_map(|v| v.values().copied()),
        )
        .collect();
    let sets: Vec<Option<u8>> = drawn
        .iter()
        .map(|m| w.get::<PlanarUv>(*m).map(|u| u.texcoord))
        .collect();
    if sets.iter().all(Option::is_none) {
        return Ok(None);
    }
    match sets.first().copied().flatten() {
        Some(set) if sets.iter().all(|x| *x == Some(set)) => Ok(Some(set)),
        _ => Err("primitives drawn with a planar-mapped material and another: texture coordinates as stored"),
    }
}

/// Whether `e`'s mesh draws anything with a planar-mapped material.
fn planar(w: &World, e: Entity) -> bool {
    w.get::<Mesh>(e).is_some_and(|mesh| {
        mesh.primitives()
            .iter()
            .enumerate()
            .any(|(i, p)| primitive_set(w, e, i, p.material).is_ok_and(|s| s.is_some()))
    })
}

/// `e`'s mesh with each planar primitive's coordinate set the game-frame `(x, z)` of its vertices through `t`.
fn rewrite(w: &mut World, e: Entity, t: &Transform, log: &mut PassLog) -> Result<(), String> {
    let Some(mesh) = w.get::<Mesh>(e) else {
        return Ok(());
    };
    let mut writes = Vec::new();
    for (i, p) in mesh.primitives().iter().enumerate() {
        let set = match primitive_set(w, e, i, p.material) {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(why) => {
                log.omit(why, 1);
                continue;
            }
        };
        let Some(positions) = p.positions() else {
            continue;
        };
        let c = t.cols.map(|c| c.map(f64::from));
        let uvs: Vec<[f32; 2]> = positions
            .iter()
            .map(|q| {
                let q = q.map(f64::from);
                let world: [f64; 3] =
                    std::array::from_fn(|r| c[0][r] * q[0] + c[1][r] * q[1] + c[2][r] * q[2] + c[3][r]);
                // the world's frame is the game's with z negated ([`crate::FRAME`])
                [world[0] as f32, (-world[2]) as f32]
            })
            .collect();
        writes.push((i, set, uvs));
    }
    let mesh = w.get_mut::<Mesh>(e).ok_or("listed")?;
    for (i, set, uvs) in writes {
        let mut p = mesh.primitive_mut(i).ok_or("listed")?;
        p.streams_mut()
            .insert(Semantic::TexCoord(set), Stream::F32x2(uvs));
        log.count(
            "planar-mapped primitives given their placement's texture coordinates",
            1,
        );
    }
    Ok(())
}

/// `lower-forzatech-planar-uv`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const LOWER_FORZATECH_PLANAR_UV: PassSpec = PassSpec {
    name: "lower-forzatech-planar-uv",
    options: &[],
    make: |_| Ok(Box::new(LowerForzaPlanarUv)),
};

impl Pass for LowerForzaPlanarUv {
    fn name(&self) -> &'static str {
        "lower-forzatech-planar-uv"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    /// Its input is the planar materials' record alone: `lower-forzatech-materials` writes it only where a material is
    /// mapped by position, so a read with none has meshes and no record, and the pass is skipped there.
    fn requires(&self) -> &[&'static str] {
        &[PlanarUv::NAME]
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Instance::NAME, Lods::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[PlanarUv::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // each prototype drawing a planar material (itself or a level-of-detail form), and its placements
        let mut placements: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
        let instances: Vec<(Entity, Entity)> = w.iter::<Instance>().map(|(p, i)| (p, i.prototype)).collect();
        for (p, prototype) in instances {
            let forms: Vec<Entity> = w
                .get::<Lods>(prototype)
                .map(|l| l.forms.iter().map(|f| f.form).collect())
                .unwrap_or_default();
            if planar(w, prototype) || forms.iter().any(|&f| planar(w, f)) {
                placements.entry(prototype).or_default().push(p);
            }
        }
        let mut copied = 0u64;
        for (prototype, placed) in placements {
            for (k, &p) in placed.iter().enumerate() {
                let t = *w
                    .get::<Transform>(p)
                    .ok_or("a planar-mapped placement without a transform")?;
                // the first placement keeps the prototype; each other one a copy, its forms copied with it
                let target = if k == 0 {
                    prototype
                } else {
                    let copy = w.duplicate(prototype).ok_or("a dead prototype")?;
                    if let Some(lods) = w.get::<Lods>(copy).cloned() {
                        let mut forms = Vec::with_capacity(lods.forms.len());
                        for f in &lods.forms {
                            forms.push(LodForm {
                                detail: f.detail,
                                form: w.duplicate(f.form).ok_or("a dead form")?,
                            });
                        }
                        w.insert(copy, Lods { forms, ..lods });
                    }
                    if let Some(i) = w.get_mut::<Instance>(p) {
                        i.prototype = copy;
                    }
                    copied += 1;
                    copy
                };
                rewrite(w, target, &t, log)?;
                let forms: Vec<Entity> = w
                    .get::<Lods>(target)
                    .map(|l| l.forms.iter().map(|f| f.form).collect())
                    .unwrap_or_default();
                for f in forms {
                    rewrite(w, f, &t, log)?;
                }
                log.count("planar-mapped placements", 1);
            }
        }
        if copied > 0 {
            log.count(
                "planar-mapped prototypes copied for a placement of their own",
                copied,
            );
        }
        let materials: BTreeSet<Entity> = w.iter::<PlanarUv>().map(|(e, _)| e).collect();
        for m in materials {
            w.remove::<PlanarUv>(m);
        }
        Ok(())
    }
}
