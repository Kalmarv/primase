//! `lower-fromsoft-effects`: the effects an area or a viewer read (`fromsoft.effect`, placed by their transforms) as
//! `core.particles`. The read keeps the effect files as stored (`fromsoft.effect_file`) and the textures and models they name
//! (`fromsoft.effect_resource`); this pass turns each placed effect's node tree into emitters ([`lower`]), the effects it
//! spawns into systems of their own that its emitters name, and counts what the core description has no place for.
//!
//! What the fxr project says of a parameter is the meaning used (sourced); the lowering's approximations, and the actions and
//! parameters it leaves out, are in the pass log by reason, with the files' own numbers (`Notes`).
mod cx;
mod draws;
mod gpu;
mod lower;
mod units;
pub mod value;

use super::lights::Disabled;
use super::settings::Factor;
use crate::sekiro::components::effect::{Effect, EffectFile, EffectResource, ResourceKind};
pub use cx::{NoFiles, Resolver};
use fromsoft_formats::fxr::resources::TextureKind;
pub use lower::{Lowered, NO_LIMIT, lower};
use primase_core::particles::{Draw, ParticleSystem};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, World};
use std::collections::{BTreeMap, BTreeSet};

/// Options: `candela-per-power` (required: candela per unit of the stored diffuse power, the program's calibration, as for
/// `lower-fromsoft-lights`; the effect lights the lowering keeps as light draws are scaled by it), `seed` (the seed of the
/// random draws of every system; default 0) and `include-disabled` (whether the effects of regions that start disabled are
/// lowered; default off, they are left out, counted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerFromSoftEffects {
    candela_per_power: Factor,
    seed: u64,
    include_disabled: Disabled,
}

impl LowerFromSoftEffects {
    /// The lowering at the program's calibration, seed 0, regions that start disabled left out.
    pub fn new(candela_per_power: Factor) -> Self {
        LowerFromSoftEffects { candela_per_power, seed: 0, include_disabled: Disabled::Skip }
    }
    /// The seed of every system's random draws.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
    /// Whether the effects of regions that start disabled are lowered.
    pub fn with_include_disabled(mut self, include_disabled: Disabled) -> Self {
        self.include_disabled = include_disabled;
        self
    }
    pub fn candela_per_power(&self) -> Factor {
        self.candela_per_power
    }
    pub fn seed(&self) -> u64 {
        self.seed
    }
    pub fn include_disabled(&self) -> Disabled {
        self.include_disabled
    }

    fn from_given(g: &Given) -> Result<Self, String> {
        let mut pass = Self::new(Factor::parse("candela-per-power", g.required("candela-per-power"))?);
        if let Some(text) = g.optional("seed") {
            pass = pass.with_seed(text.parse::<u64>().map_err(|_| format!("seed={text} is not a whole number"))?);
        }
        if let Some(d) = g.optional_choice::<Disabled>("include-disabled") {
            pass = pass.with_include_disabled(d);
        }
        Ok(pass)
    }
}

/// `lower-fromsoft-effects`' declaration ([`primase_core::pass::PassRegistry::add`]).
pub const LOWER_FROMSOFT_EFFECTS: PassSpec = PassSpec {
    name: "lower-fromsoft-effects",
    options: &[
        OptionSpec::required(
            "candela-per-power",
            "1",
            "at least 0: the candela of one unit of stored light power, the program's calibration (as lower-fromsoft-lights)",
        ),
        OptionSpec::optional("seed", "0", "a whole number: the seed of every system's random draws", "0"),
        OptionSpec::optional_choice::<Disabled>(
            "include-disabled",
            "lower the effects of regions that start disabled",
            "off: they are left out, counted by reason",
        ),
    ],
    make: |g| Ok(Box::new(LowerFromSoftEffects::from_given(g)?)),
};

/// The finder of the files the lowering names: the resource entities of the world and the system entities made for the
/// effects the lowering may name.
struct Finder<'a> {
    resources: &'a BTreeMap<(ResourceKind, i32), Entity>,
    systems: &'a BTreeMap<i32, Entity>,
    /// The effects the one being lowered may not name: those that can reach it (a spawn cycle's closing reference).
    forbidden: BTreeSet<i32>,
}

impl Resolver for Finder<'_> {
    fn texture(&mut self, kind: TextureKind, id: i32) -> Option<Entity> {
        self.resources.get(&(ResourceKind::of_texture(kind), id)).copied()
    }
    fn model(&mut self, id: i32) -> Option<Entity> {
        self.resources.get(&(ResourceKind::Model, id)).copied()
    }
    fn animation(&mut self, id: i32) -> Option<Entity> {
        self.resources.get(&(ResourceKind::Animation, id)).copied()
    }
    fn effect(&mut self, id: i32) -> Option<Entity> {
        if self.forbidden.contains(&id) {
            return None;
        }
        self.systems.get(&id).copied()
    }
}

/// Whether effect `to` is reachable from `from` through spawn references among `files`.
fn reaches(files: &BTreeMap<i32, EffectFile>, from: i32, to: i32) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(id) = stack.pop() {
        if id == to {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(f) = files.get(&id) {
            stack.extend(f.file.spawned());
        }
    }
    false
}

/// A mix of two numbers into a seed.
fn mix(seed: u64, n: u64) -> u64 {
    let z = (seed ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_add(0x9E37_79B9_7F4A_7C15);
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl LowerFromSoftEffects {
    /// Scales the light draws of `system` to candela and returns it.
    fn calibrated(&self, mut system: ParticleSystem) -> ParticleSystem {
        let k = self.candela_per_power.get() as f32;
        for e in &mut system.emitters {
            if let Some(Draw::Light(l)) = &mut e.draw {
                l.intensity = value::scaled(l.intensity.clone(), k);
            }
        }
        system
    }
}

impl Pass for LowerFromSoftEffects {
    fn name(&self) -> &'static str {
        "lower-fromsoft-effects"
    }
    fn options(&self) -> Options {
        let mut options = Options::from([("candela-per-power".to_owned(), self.candela_per_power.to_string())]);
        if self.seed != 0 {
            options.insert("seed".to_owned(), self.seed.to_string());
        }
        if self.include_disabled != Disabled::Skip {
            options.insert("include-disabled".to_owned(), self.include_disabled.text().to_owned());
        }
        options
    }
    /// Nothing is required: a read area may place no effect.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[ParticleSystem::NAME]
    }
    fn approximates(&self) -> &'static str {
        "the fxr project's reading of the parameters (not yet checked against a running game); Hermite properties sampled into lines; a GPU particle system as an ordinary emitter; level-of-detail ranges as ascending distances; soft-particle depth as a fixed distance; effect light colours squared at the keys of their curves"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the files the read kept
        let files: BTreeMap<i32, EffectFile> = world.iter::<EffectFile>().map(|(_, f)| (f.effect, f.clone())).collect();
        let mut resources: BTreeMap<(ResourceKind, i32), Entity> = BTreeMap::new();
        for (e, r) in world.iter::<EffectResource>() {
            if resources.insert((r.kind, r.id), e).is_some() {
                return Err(format!("{e:?}: the effect resource {:?} {} is held by two entities", r.kind, r.id));
            }
        }
        // the placements, and the effects they need with the effects those spawn
        let mut placements: Vec<(Entity, i32)> = Vec::new();
        for (e, p) in world.iter::<Effect>() {
            if !world.has::<primase_core::Transform>(e) {
                return Err(format!("{e:?}: a placed effect without a transform"));
            }
            if p.starts_disabled && self.include_disabled == Disabled::Skip {
                log.omit("effects of regions that start disabled, left out", 1);
                continue;
            }
            if !files.contains_key(&p.effect) {
                log.omit("placed effects whose file the read did not keep", 1);
                continue;
            }
            placements.push((e, p.effect));
        }
        let mut wanted: BTreeSet<i32> = placements.iter().map(|(_, id)| *id).collect();
        let mut stack: Vec<i32> = wanted.iter().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(f) = files.get(&id) else { continue };
            for r in f.file.spawned() {
                if files.contains_key(&r) && wanted.insert(r) {
                    stack.push(r);
                }
            }
        }
        if wanted.is_empty() {
            return Ok(());
        }
        // an entity for each effect's system first (a system names the systems it spawns by them), then each effect lowered once
        let mut systems: BTreeMap<i32, Entity> = BTreeMap::new();
        for &id in &wanted {
            systems.insert(id, world.spawn());
        }
        let mut lowered: BTreeMap<i32, ParticleSystem> = BTreeMap::new();
        for &id in &wanted {
            let Some(f) = files.get(&id) else { continue };
            // an effect may name another unless that one reaches it and has the lower number: a cycle is cut where it closes downward
            let forbidden: BTreeSet<i32> = wanted.iter().copied().filter(|&other| other <= id && reaches(&files, other, id)).collect();
            let mut finder = Finder { resources: &resources, systems: &systems, forbidden };
            let l = lower::lower(&f.file, f.game, mix(self.seed, u64::try_from(id).unwrap_or(0)), &mut finder)
                .map_err(|e| format!("effect {id}: {e}"))?;
            for (reason, n) in &l.notes.lost {
                log.omit(reason, u64::from(*n));
            }
            for (reason, n) in &l.notes.approximated {
                log.approximate(reason, u64::from(*n));
            }
            log.count("effects lowered", 1);
            lowered.insert(id, self.calibrated(l.system));
        }
        for (id, e) in &systems {
            if let Some(system) = lowered.get(id) {
                world.insert(*e, system.clone());
            }
        }
        for (e, id) in placements {
            if let Some(system) = lowered.get(&id) {
                let mut system = system.clone();
                system.seed = mix(system.seed, u64::from(e.index()));
                world.insert(e, system);
                log.count("placed effects lowered", 1);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
