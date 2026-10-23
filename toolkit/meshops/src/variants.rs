//! `specialize-material-variants{unused=keep|discard}`: placements that use different material variants of a prototype
//! ([`MaterialVariants`]: per primitive, variant -> material, overriding the primitive's material for the placements
//! whose [`Instance::variant`] matches) get prototypes of their own, each drawing its variant's materials as its
//! primitives' own. A placement whose variant draws the prototype's own materials stays on it, choosing what it drew:
//! its own variant while the prototype keeps its variants (variant 0 is not neutral: a map may list it), variant 0
//! once they are discarded (none is left to choose). Each other form used
//! gets one copy of the prototype (its components copied, its mesh drawing the variant's materials, no variants, no
//! mesh file: the file names the prototype's own materials), named `<name> variant <v>`, declared as made from the
//! prototype ([`PassLog::place_from`]), and its placements point at it with variant 0. A destination whose coordinates
//! serve every material a primitive can be drawn with (one texture transform per primitive, `bake-texture-transforms`)
//! can then prepare each form apart.
//!
//! What becomes of the forms nobody places is the program's choice: `unused=keep` loses nothing (each prototype keeps
//! its own materials and every variant); `unused=discard` removes every prototype's variants, and a prototype none of
//! whose placements draws its own materials draws its first used variant itself instead of being copied (none is left
//! behind unplaced); each prototype that so loses a form nobody places is counted as an omission. A prototype with
//! variants and levels of detail ([`Lods`]) is refused (each level would need specializing), as is a placed one whose
//! variants do not list a map per primitive of its loaded mesh.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Entity, Instance, Lods, MaterialVariants, Mesh, MeshFile, Name, World};

/// What becomes of the material variants nobody places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnusedVariants {
    /// Kept, as they are.
    Keep,
    /// Removed, counted per prototype that loses a form.
    Discard,
}

impl Choice for UnusedVariants {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("keep", UnusedVariants::Keep),
        ("discard", UnusedVariants::Discard),
    ];
}

/// See the module documentation. Made by [`SpecializeMaterialVariants::new`]; from options
/// ([`SPECIALIZE_MATERIAL_VARIANTS`]) by the same call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecializeMaterialVariants {
    unused: UnusedVariants,
}

impl SpecializeMaterialVariants {
    pub fn new(unused: UnusedVariants) -> SpecializeMaterialVariants {
        SpecializeMaterialVariants { unused }
    }

    pub fn unused(&self) -> UnusedVariants {
        self.unused
    }
}

/// `specialize-material-variants`' declaration ([`primase_core::pass::PassRegistry::add`]): what becomes of the
/// variants nobody places is the program's, no default.
pub const SPECIALIZE_MATERIAL_VARIANTS: PassSpec = PassSpec {
    name: "specialize-material-variants",
    options: &[OptionSpec::choice::<UnusedVariants>(
        "unused",
        "the variants nobody places kept as they are, or removed and counted",
    )],
    make: |g| Ok(Box::new(SpecializeMaterialVariants::new(g.choice("unused")))),
};

/// The materials prototype `mesh`'s primitives draw for `variant`: each primitive's override, else its own.
fn resolved(mesh: &Mesh, variants: &MaterialVariants, variant: u32) -> Vec<Option<Entity>> {
    mesh.primitives()
        .iter()
        .zip(&variants.0)
        .map(|(p, choices)| choices.get(&variant).copied().or(p.material))
        .collect()
}

/// Every form a prototype can be drawn in: its own materials, and each variant's.
fn forms(mesh: &Mesh, variants: &MaterialVariants) -> BTreeSet<Vec<Option<Entity>>> {
    let own = mesh.primitives().iter().map(|p| p.material).collect();
    let keys: BTreeSet<u32> = variants.0.iter().flat_map(|m| m.keys().copied()).collect();
    std::iter::once(own)
        .chain(keys.into_iter().map(|v| resolved(mesh, variants, v)))
        .collect()
}

impl Pass for SpecializeMaterialVariants {
    fn name(&self) -> &'static str {
        "specialize-material-variants"
    }
    fn options(&self) -> Options {
        Options::from([("unused".to_owned(), self.unused.text().to_owned())])
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.instance", "core.mesh", "core.name"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.material_variants", "core.mesh_file"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // each prototype with variants: its placements by variant
        let mut used: BTreeMap<Entity, BTreeMap<u32, Vec<Entity>>> = BTreeMap::new();
        for (e, i) in w.iter::<Instance>() {
            if w.has::<MaterialVariants>(i.prototype) {
                used.entry(i.prototype)
                    .or_default()
                    .entry(i.variant)
                    .or_default()
                    .push(e);
            }
        }
        // checked before anything changes
        for (p, variants) in w.iter::<MaterialVariants>() {
            if w.has::<Lods>(p) {
                return Err(format!(
                    "specialize-material-variants: prototype {p:?} has material variants and levels of detail: \
                     select a level first"
                ));
            }
            let parts = w.get::<Mesh>(p).map(|m| m.primitives().len());
            if used.contains_key(&p) && parts != Some(variants.0.len()) {
                return Err(format!(
                    "specialize-material-variants: prototype {p:?}'s variants list {} primitives, its mesh {}",
                    variants.0.len(),
                    parts.map_or_else(|| "is not loaded".to_owned(), |n| n.to_string())
                ));
            }
        }
        // the prototypes that lose a form nobody places when the variants are discarded
        let mut losing = 0u64;
        if self.unused == UnusedVariants::Discard {
            for (p, variants) in w.iter::<MaterialVariants>() {
                let placed: BTreeSet<Vec<Option<Entity>>> = match (used.get(&p), w.get::<Mesh>(p)) {
                    (Some(by_variant), Some(mesh)) => {
                        by_variant.keys().map(|&v| resolved(mesh, variants, v)).collect()
                    }
                    _ => BTreeSet::new(),
                };
                let all = w
                    .get::<Mesh>(p)
                    .map_or_else(BTreeSet::new, |m| forms(m, variants));
                if placed.is_empty() || !all.is_subset(&placed) {
                    losing += 1;
                }
            }
        }
        for (proto, by_variant) in used {
            let mesh = w.get::<Mesh>(proto).expect("checked").clone();
            let variants = w.get::<MaterialVariants>(proto).expect("listed").clone();
            let own: Vec<Option<Entity>> = mesh.primitives().iter().map(|p| p.material).collect();
            let name = w.get::<Name>(proto).map(|n| n.0.clone());
            // the forms other than the prototype's own, each with its placements, in variant order
            let mut others: BTreeMap<Vec<Option<Entity>>, (u32, Vec<Entity>)> = BTreeMap::new();
            let mut stays = false;
            for (variant, placements) in by_variant {
                let materials = resolved(&mesh, &variants, variant);
                if materials == own {
                    stays = true;
                    // kept, the variants still resolve: the placement's own variant draws what it drew, where
                    // variant 0 may draw another form; discarded, none is left and variant 0 draws the own materials
                    if self.unused == UnusedVariants::Discard {
                        for e in placements {
                            w.insert(
                                e,
                                Instance {
                                    prototype: proto,
                                    variant: 0,
                                },
                            );
                        }
                    }
                    continue;
                }
                others
                    .entry(materials)
                    .or_insert_with(|| (variant, Vec::new()))
                    .1
                    .extend(placements);
            }
            let mut others: Vec<(u32, Vec<Option<Entity>>, Vec<Entity>)> =
                others.into_iter().map(|(m, (v, p))| (v, m, p)).collect();
            others.sort_by_key(|(v, _, _)| *v);
            let mut reuse = self.unused == UnusedVariants::Discard && !stays;
            for (variant, materials, placements) in others {
                let target = if reuse {
                    reuse = false;
                    proto
                } else {
                    let copy = w.spawn();
                    w.copy_components(proto, copy);
                    w.remove::<MaterialVariants>(copy);
                    log.place_from(copy, proto);
                    log.count("prototype copies drawing a material variant", 1);
                    copy
                };
                let mut m = mesh.clone();
                for (mut part, material) in m.primitives_mut().zip(materials) {
                    *part.material_mut() = material;
                }
                w.insert(target, m);
                w.remove::<MeshFile>(target);
                if let Some(n) = &name {
                    w.insert(target, Name(format!("{n} variant {variant}")));
                }
                for e in placements {
                    w.insert(
                        e,
                        Instance {
                            prototype: target,
                            variant: 0,
                        },
                    );
                    log.count("placements given their variant's prototype", 1);
                }
            }
        }
        if self.unused == UnusedVariants::Discard {
            let with: Vec<Entity> = w.iter::<MaterialVariants>().map(|(e, _)| e).collect();
            for e in with {
                w.remove::<MaterialVariants>(e);
            }
            if losing > 0 {
                log.omit(
                    "prototypes' material forms nobody places (unused=discard)",
                    losing,
                );
            }
        }
        Ok(())
    }
}
