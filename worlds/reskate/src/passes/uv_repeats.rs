//! `prepare-reskate-uv-repeats{decals=omit|keep,blended=cutout|keep}`: what a map does with triangles whose texture
//! repeats more than Studio draws ([`primase_meshops::MAX_UV_TILES`] tiles over one triangle). Studio cuts a material's
//! triangles at its texture's repeats when the material is a decal, or a blended surface with a base texture, and the
//! texture repeats (its wrap is repeat or mirror): such a triangle spanning many repeats is cut into many.
//!
//! - `decals=omit`: a primitive drawn only by repeating decals (its material and every variant's) has those triangles
//!   left out, counted as a drop of triangles; `keep` leaves them for Studio to cut.
//! - `blended=cutout`: a primitive drawn by a repeating blended material that has such a triangle is drawn alpha
//!   tested instead: each of its blended materials (its own and every variant's: glass or alpha class, blended, with a
//!   base texture) is replaced by a cutout twin (the same material, class cutout with an alpha mask, its cutoff,
//!   sidedness and domain the cutout class's), counted as an approximation, the twin made once per material; its other
//!   materials stay; `keep` leaves it blended.
//!
//! The map writer used to make both choices itself (omit, cutout) while it built its meshes; the writer now writes the
//! prepared materials and triangles as given.

use std::collections::BTreeMap;

use primase_core::ledger::Measure;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, MaterialVariants, Mesh, Name, Semantic, Stream, World};

use crate::components::{AlphaMode, MaterialClass, SkateMaterial};
use crate::scene::{cut_at_repeats, repeats_texture};

/// What `decals=` does with repeating decal triangles over too many repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatDecals {
    Omit,
    Keep,
}

/// What `blended=` does with a blended primitive that has such a triangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatBlended {
    Cutout,
    Keep,
}

/// See the module documentation. Made by [`PrepareReskateUvRepeats::new`]; from options by the same call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepareReskateUvRepeats {
    decals: RepeatDecals,
    blended: RepeatBlended,
}

impl PrepareReskateUvRepeats {
    /// What repeating decals and blended surfaces get: both stated.
    pub fn new(decals: RepeatDecals, blended: RepeatBlended) -> PrepareReskateUvRepeats {
        PrepareReskateUvRepeats { decals, blended }
    }

    /// From its options: `decals=omit|keep` and `blended=cutout|keep`, both required.
    pub fn from_options(o: &Options) -> Result<PrepareReskateUvRepeats, String> {
        if let Some(k) = o.keys().find(|k| !["decals", "blended"].contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let decals = match o.get("decals").map(String::as_str) {
            Some("omit") => RepeatDecals::Omit,
            Some("keep") => RepeatDecals::Keep,
            Some(v) => return Err(format!("decals={v}: omit or keep")),
            None => return Err("decals is required: omit or keep".into()),
        };
        let blended = match o.get("blended").map(String::as_str) {
            Some("cutout") => RepeatBlended::Cutout,
            Some("keep") => RepeatBlended::Keep,
            Some(v) => return Err(format!("blended={v}: cutout or keep")),
            None => return Err("blended is required: cutout or keep".into()),
        };
        Ok(PrepareReskateUvRepeats::new(decals, blended))
    }
}

/// [`PrepareReskateUvRepeats::from_options`] as a pass factory.
pub fn prepare_reskate_uv_repeats(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateUvRepeats::from_options(o)?))
}

/// Whether the triangle `t` of texture coordinates `uv` spans more repeats than Studio draws.
fn over(uv: &[[f32; 2]], t: [u32; 3]) -> Result<bool, String> {
    let c = |i: u32| {
        uv.get(i as usize)
            .copied()
            .ok_or_else(|| format!("index {i} past {} texture coordinates", uv.len()))
    };
    Ok(
        primase_meshops::uv_tiles([c(t[0])?, c(t[1])?, c(t[2])?]).map_err(|e| e.to_string())?
            > primase_meshops::MAX_UV_TILES,
    )
}

/// The cutout twin of material `m` (made once, `twins`): the same entity's components, its skate.material keyed
/// anew, class cutout with an alpha mask, its cutoff, sidedness and domain the cutout class's; named as the original
/// is labelled (its name, else its key), so the map names it as it named the writer's own cutout form.
fn twin(w: &mut World, twins: &mut BTreeMap<Entity, Entity>, m: Entity) -> Result<Entity, String> {
    if let Some(&t) = twins.get(&m) {
        return Ok(t);
    }
    let original = w
        .get::<SkateMaterial>(m)
        .cloned()
        .ok_or("a material without skate.material")?;
    let t = w.duplicate(m).ok_or("a material entity that is not alive")?;
    let taken: Vec<String> = w
        .iter::<SkateMaterial>()
        .map(|(_, s)| s.key.to_lowercase())
        .collect();
    let mut key = format!("{}_cutout", original.key);
    let mut n = 2;
    while taken.contains(&key.to_lowercase()) {
        key = format!("{}_cutout{n}", original.key);
        n += 1;
    }
    if w.get::<Name>(t).is_none() && original.base.is_none() {
        w.insert(t, Name(original.key.clone()));
    }
    w.insert(
        t,
        SkateMaterial {
            key,
            class: MaterialClass::Cutout,
            alpha: Some(AlphaMode::Mask),
            cutoff: None,
            two_sided: None,
            domain: None,
            ..original
        },
    );
    twins.insert(m, t);
    Ok(t)
}

impl Pass for PrepareReskateUvRepeats {
    fn name(&self) -> &'static str {
        "prepare-reskate-uv-repeats"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "decals".to_owned(),
                match self.decals {
                    RepeatDecals::Omit => "omit",
                    RepeatDecals::Keep => "keep",
                }
                .to_owned(),
            ),
            (
                "blended".to_owned(),
                match self.blended {
                    RepeatBlended::Cutout => "cutout",
                    RepeatBlended::Keep => "keep",
                }
                .to_owned(),
            ),
        ])
    }
    // materials and meshes are optional: a map without them has nothing to change
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            "core.mesh",
            "core.material_variants",
            "skate.material",
            "core.name",
        ]
    }
    fn approximates(&self) -> &'static str {
        "blended surfaces with a triangle over too many texture repeats drawn alpha tested"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let rule = |w: &World, m: Option<Entity>| {
            m.and_then(|m| w.get::<SkateMaterial>(m))
                .map(|s| {
                    let (decal, blended) = cut_at_repeats(s);
                    let repeats = repeats_texture(s);
                    (decal && repeats, blended && repeats)
                })
                .unwrap_or((false, false))
        };
        // the prototypes' meshes, as the writer builds them
        let meshes: Vec<Entity> = w
            .iter::<Mesh>()
            .map(|(e, _)| e)
            .filter(|&e| w.has::<crate::components::SkatePrototype>(e))
            .collect();
        let mut twins: BTreeMap<Entity, Entity> = BTreeMap::new();
        let (mut omitted, mut masked) = (0u64, 0u64);
        for e in meshes {
            let n = w.get::<Mesh>(e).map_or(0, |m| m.primitives().len());
            for i in 0..n {
                let (material, uv, tris) = {
                    let p = &w.get::<Mesh>(e).expect("listed").primitives()[i];
                    let uv: Vec<[f32; 2]> = match p.streams.get(&Semantic::TexCoord(0)) {
                        Some(Stream::F32x2(v)) => v.clone(),
                        _ => continue,
                    };
                    let tris: Vec<[u32; 3]> = match &p.indices {
                        Some(ix) => ix.as_chunks::<3>().0.to_vec(),
                        None => (0..p.vertex_count() as u32 / 3)
                            .map(|t| [3 * t, 3 * t + 1, 3 * t + 2])
                            .collect(),
                    };
                    (p.material, uv, tris)
                };
                let variants: Vec<Entity> = w
                    .get::<MaterialVariants>(e)
                    .and_then(|v| v.0.get(i))
                    .map(|m| m.values().copied().collect())
                    .unwrap_or_default();
                // the primitive's materials: every variant's and its own, as the writer listed a part's
                let keys: Vec<Entity> = variants.iter().copied().chain(material).collect();
                let rules: Vec<(bool, bool)> = keys.iter().map(|&m| rule(w, Some(m))).collect();
                // decals: every material of the primitive a repeating decal (the writer's rule: none at all counts)
                if self.decals == RepeatDecals::Omit && rules.iter().all(|r| r.0) {
                    let mut kept: Vec<u32> = Vec::with_capacity(tris.len() * 3);
                    for &t in &tris {
                        if !over(&uv, t)? {
                            kept.extend(t);
                        }
                    }
                    let left = (tris.len() - kept.len() / 3) as u64;
                    if left > 0 {
                        omitted += left;
                        let mesh = w.get_mut::<Mesh>(e).expect("listed");
                        *mesh.primitive_mut(i).expect("in range").indices_mut() = Some(kept);
                        continue;
                    }
                }
                // blended: any material of the primitive a repeating blended surface, with a triangle over the limit
                if self.blended == RepeatBlended::Cutout && rules.iter().any(|r| r.1) {
                    let mut hit = false;
                    for &t in &tris {
                        if over(&uv, t)? {
                            hit = true;
                            break;
                        }
                    }
                    if hit {
                        masked += 1;
                        // the blended ones of its materials take the cutout form
                        let blended = |w: &World, m: Entity| {
                            w.get::<SkateMaterial>(m).is_some_and(|s| cut_at_repeats(s).1)
                        };
                        if let Some(m) = material.filter(|&m| blended(w, m)) {
                            let t = twin(w, &mut twins, m)?;
                            let mesh = w.get_mut::<Mesh>(e).expect("listed");
                            *mesh.primitive_mut(i).expect("in range").material_mut() = Some(t);
                        }
                        let blended_variants: Vec<Entity> =
                            variants.iter().copied().filter(|&m| blended(w, m)).collect();
                        let mapped: Vec<(Entity, Entity)> = blended_variants
                            .into_iter()
                            .map(|m| twin(w, &mut twins, m).map(|t| (m, t)))
                            .collect::<Result<_, _>>()?;
                        if let Some(v) = w.get_mut::<MaterialVariants>(e)
                            && let Some(row) = v.0.get_mut(i)
                        {
                            for m in row.values_mut() {
                                if let Some(&(_, t)) = mapped.iter().find(|(o, _)| o == m) {
                                    *m = t;
                                }
                            }
                        }
                    }
                }
            }
        }
        log.dropped(
            Measure::Triangles,
            "decal triangles over more texture repeats than Studio draws",
            omitted,
        );
        if masked > 0 {
            log.approximate(
                "blended primitives with a triangle over more texture repeats than Studio draws, drawn alpha tested",
                masked,
            );
            log.count("cutout materials made", twins.len() as u64);
        }
        Ok(())
    }
}
