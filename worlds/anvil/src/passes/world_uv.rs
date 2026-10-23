//! `lower-anvil-world-uv`: the texture coordinates of what is drawn with a material mapped by the world position
//! ([`AnvilWorldUv`]: MAD_Outskirt's, lowered by `lower-anvil-material`), written per placement: each vertex's
//! `TexCoord(0)` the material's rows applied to the vertex through its placement's transform. A prototype placed more
//! than once is copied for each placement after its first (its level-of-detail forms with it), so each copy carries
//! its own placement's coordinates. Run after the meshes, materials and levels of detail are core's.
//!
//! Only primitives drawn with such materials alone are rewritten (one also drawn with another material, through its
//! variants, keeps its coordinates, counted; one drawn with two world-mapped materials of other rows likewise). What
//! isn't the game's: the normal map's tangent frame, which a consumer builds from these coordinates, where the shader
//! takes the mesh's stored tangents (reported).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::{
    Component, Entity, Instance, LodForm, Lods, MaterialVariants, Mesh, Semantic, Stream, Transform, World,
};

use crate::components::AnvilWorldUv;

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerAnvilWorldUv;

/// The rows of the world-mapped material every drawing of primitive `i` of `e` uses, if all its materials are one such
/// material's rows (`Err` names a mixed primitive).
fn primitive_rows(
    w: &World,
    e: Entity,
    i: usize,
    material: Option<Entity>,
) -> Result<Option<[[f64; 4]; 2]>, &'static str> {
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
    let rows: Vec<Option<[[f64; 4]; 2]>> = drawn
        .iter()
        .map(|m| w.get::<AnvilWorldUv>(*m).map(|u| u.rows))
        .collect();
    if rows.iter().all(Option::is_none) {
        return Ok(None);
    }
    match rows.first().copied().flatten() {
        Some(r) if rows.iter().all(|x| *x == Some(r)) => Ok(Some(r)),
        _ => Err("primitives drawn with a world-mapped material and another: texture coordinates as stored"),
    }
}

/// Whether `e`'s mesh draws anything with a world-mapped material.
fn world_mapped(w: &World, e: Entity) -> bool {
    w.get::<Mesh>(e).is_some_and(|mesh| {
        mesh.primitives()
            .iter()
            .enumerate()
            .any(|(i, p)| primitive_rows(w, e, i, p.material).is_ok_and(|r| r.is_some()))
    })
}

/// `e`'s mesh with each world-mapped primitive's `TexCoord(0)` its rows at the vertices through `t`.
fn rewrite(w: &mut World, e: Entity, t: &Transform, log: &mut PassLog) -> Result<(), String> {
    let Some(mesh) = w.get::<Mesh>(e) else {
        return Ok(());
    };
    let mut writes = Vec::new();
    for (i, p) in mesh.primitives().iter().enumerate() {
        let rows = match primitive_rows(w, e, i, p.material) {
            Ok(Some(r)) => r,
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
                rows.map(|row| (row[0] * world[0] + row[1] * world[1] + row[2] * world[2] + row[3]) as f32)
            })
            .collect();
        writes.push((i, uvs));
    }
    let mesh = w.get_mut::<Mesh>(e).ok_or("listed")?;
    for (i, uvs) in writes {
        let mut p = mesh.primitive_mut(i).ok_or("listed")?;
        p.streams_mut().insert(Semantic::TexCoord(0), Stream::F32x2(uvs));
        log.count(
            "world-mapped primitives given their placement's texture coordinates",
            1,
        );
    }
    Ok(())
}

/// `lower-anvil-world-uv`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by the game's own rule.
pub const LOWER_ANVIL_WORLD_UV: PassSpec = PassSpec {
    name: "lower-anvil-world-uv",
    options: &[],
    make: |_| Ok(Box::new(LowerAnvilWorldUv)),
};

impl Pass for LowerAnvilWorldUv {
    fn name(&self) -> &'static str {
        "lower-anvil-world-uv"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilWorldUv::NAME, Mesh::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Instance::NAME, Lods::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilWorldUv::NAME]
    }
    fn approximates(&self) -> &'static str {
        "world-mapped materials' normal maps in the tangent frame of their new texture coordinates"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // each prototype drawing a world-mapped material (itself or a level-of-detail form), and its placements
        let mut placements: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
        let instances: Vec<(Entity, Entity)> = w.iter::<Instance>().map(|(p, i)| (p, i.prototype)).collect();
        for (p, prototype) in instances {
            let forms: Vec<Entity> = w
                .get::<Lods>(prototype)
                .map(|l| l.forms.iter().map(|f| f.form).collect())
                .unwrap_or_default();
            if world_mapped(w, prototype) || forms.iter().any(|&f| world_mapped(w, f)) {
                placements.entry(prototype).or_default().push(p);
            }
        }
        let mut copied = 0u64;
        for (prototype, placed) in placements {
            for (k, &p) in placed.iter().enumerate() {
                let t = *w
                    .get::<Transform>(p)
                    .ok_or("a world-mapped placement without a transform")?;
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
                log.count("world-mapped placements", 1);
            }
        }
        if copied > 0 {
            log.count(
                "world-mapped prototypes copied for a placement of their own",
                copied,
            );
        }
        let materials: BTreeSet<Entity> = w.iter::<AnvilWorldUv>().map(|(e, _)| e).collect();
        for m in materials {
            w.remove::<AnvilWorldUv>(m);
        }
        log.approximate(
            "world-mapped materials' normal maps in the tangent frame of their new texture coordinates (the shader's \
             is the mesh's stored tangents)",
            1,
        );
        Ok(())
    }
}
