//! `apply-forzatech-emission-variation`: the instance variation of a lowered material's emission, applied per placement as
//! material variants.
//!
//! The shaders of `projection_lod` and (with `LightEmisVariationOnOff`) `environment` multiply the emission by
//! `saturate((y - EmissiveVariationMin) / (EmissiveVariationMax - EmissiveVariationMin))`, `y` the second value of the
//! placement's instance variation (a rule record's second byte over 255). The lowering records the ramp
//! ([`crate::components::EmissionVariation`]) on each material that emits; this pass gives each prototype's placements new
//! material variants, one per distinct (model variant, factor of each ramp the prototype's materials hold), whose
//! materials are copies with the emission scaled by the factor (a factor of 0 removes it). Materials are copied once per
//! (material, factor), so placements with the same factor share them, and a factor of 1 draws the material itself.
//! `specialize-material-variants` then makes a prototype of each distinct set of materials.
//!
//! Run after `select-lod` and `lower-forzatech-materials` (and after `bake-forzatech-swatch-tint`, whose variants it
//! extends) and before `specialize-material-variants`. A prototype with levels of detail is left as it is (counted), as the
//! specializing pass refuses variants beside levels.
//!
//! Left out, counted: a hand-placed instance has no variation that is traced (its transform's tint is not known to be the
//! instance variation), so it draws at a factor of 1; so does an instance with no placement record. Where the ramp is
//! empty or reversed (`EmissiveVariationMax` not above `Min`) the factor is the step at `Min`.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, Instance, Lods, MaterialVariants, Mesh, World};

use crate::components::{EmissionVariation, Placement, PlacementOrigin};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApplyForzaEmissionVariation;

/// `apply-forzatech-emission-variation`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const APPLY_FORZATECH_EMISSION_VARIATION: PassSpec = PassSpec {
    name: "apply-forzatech-emission-variation",
    options: &[],
    make: |_| Ok(Box::new(ApplyForzaEmissionVariation)),
};

/// A ramp's bits: `(min, max)`.
type Ramp = (u32, u32);

/// The factor of `ramp` at the variation `y`.
fn factor(ramp: Ramp, y: f32) -> f32 {
    let (min, max) = (f32::from_bits(ramp.0), f32::from_bits(ramp.1));
    if max > min {
        ((y - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        f32::from(y >= min)
    }
}

/// A material variant a placement draws: the model's variant and each ramp's factor (as bits), in the ramps' order.
type Key = (u32, Vec<u32>);

/// The shared state of one run.
#[derive(Default)]
struct Apply {
    /// The scaled copy of (material, factor).
    copies: BTreeMap<(Entity, u32), Entity>,
    copies_made: u64,
}

impl Apply {
    /// `m` drawn at `factors` (a factor per ramp in `ramps`): itself where the factor is 1 or it holds no ramp, else its
    /// copy with the emission scaled.
    fn drawn(&mut self, w: &mut World, m: Entity, ramps: &[Ramp], factors: &[u32]) -> Result<Entity, String> {
        let Some(v) = w.get::<EmissionVariation>(m) else {
            return Ok(m);
        };
        let ramp = (v.min.to_bits(), v.max.to_bits());
        let at = ramps
            .iter()
            .position(|r| *r == ramp)
            .ok_or("a ramp the prototype was not scanned for")?;
        let f = f32::from_bits(*factors.get(at).ok_or("a ramp with no factor")?);
        if f == 1.0 {
            return Ok(m);
        }
        if let Some(&copy) = self.copies.get(&(m, f.to_bits())) {
            return Ok(copy);
        }
        let copy = w.duplicate(m).ok_or("a dead emissive material")?;
        w.remove::<EmissionVariation>(copy);
        let pbr = w
            .get_mut::<PbrMaterial>(copy)
            .ok_or("an emissive material that is not lowered")?;
        if f == 0.0 {
            pbr.emissive_texture = None;
            pbr.emissive = [0.0; 3];
            pbr.emissive_strength = 1.0;
        } else {
            pbr.emissive = pbr.emissive.map(|c| c * f);
        }
        self.copies.insert((m, f.to_bits()), copy);
        self.copies_made += 1;
        Ok(copy)
    }

    fn prototype(
        &mut self,
        w: &mut World,
        proto: Entity,
        placements: &[Entity],
        log: &mut PassLog,
    ) -> Result<(), String> {
        let Some(mesh) = w.get::<Mesh>(proto) else {
            return Ok(());
        };
        let variants: Vec<BTreeMap<u32, Entity>> = w
            .get::<MaterialVariants>(proto)
            .map(|v| v.0.clone())
            .unwrap_or_default();
        let own: Vec<Option<Entity>> = mesh.primitives().iter().map(|p| p.material).collect();
        let ramps: BTreeSet<Ramp> = own
            .iter()
            .flatten()
            .chain(variants.iter().flat_map(BTreeMap::values))
            .filter_map(|m| w.get::<EmissionVariation>(*m))
            .map(|v| (v.min.to_bits(), v.max.to_bits()))
            .collect();
        if ramps.is_empty() {
            return Ok(());
        }
        let ramps: Vec<Ramp> = ramps.into_iter().collect();
        if w.has::<Lods>(proto) {
            log.omit(
                "emissive prototypes with levels of detail: no instance variation of the emission (select-lod first)",
                1,
            );
            return Ok(());
        }
        let resolved = |i: usize, v: u32| variants.get(i).and_then(|m| m.get(&v)).copied().or(own[i]);

        let mut by_placement: Vec<(Entity, Key)> = Vec::new();
        for &p in placements {
            let variant = w.get::<Instance>(p).map_or(0, |i| i.variant);
            let y = match w.get::<Placement>(p).map(|x| &x.origin) {
                // the shader's instance variation is the byte over 255
                Some(PlacementOrigin::Rule { variation, .. }) => Some(f32::from(variation[1]) / 255.0),
                Some(PlacementOrigin::Hand { .. }) => {
                    log.omit("hand-placed emissive instances whose variation is not traced: drawn at a factor of 1", 1);
                    None
                }
                None => {
                    log.omit(
                        "emissive instances with no placement record: drawn at a factor of 1",
                        1,
                    );
                    None
                }
            };
            let factors: Vec<u32> = ramps
                .iter()
                .map(|r| y.map_or(1.0, |y| factor(*r, y)).to_bits())
                .collect();
            if y.is_some() {
                log.count(
                    if factors.iter().all(|f| f32::from_bits(*f) == 1.0) {
                        "rule-placed emissive instances at an instance variation factor of 1"
                    } else {
                        "rule-placed emissive instances at an instance variation factor below 1"
                    },
                    1,
                );
            }
            by_placement.push((p, (variant, factors)));
        }
        let distinct: BTreeSet<&Key> = by_placement.iter().map(|(_, k)| k).collect();
        if distinct
            .iter()
            .all(|(_, f)| f.iter().all(|f| f32::from_bits(*f) == 1.0))
        {
            return Ok(());
        }
        let ids: BTreeMap<Key, u32> = distinct.into_iter().cloned().zip(0u32..).collect();

        let mut maps: Vec<BTreeMap<u32, Entity>> = vec![BTreeMap::new(); own.len()];
        for ((variant, factors), &id) in &ids {
            for (i, map) in maps.iter_mut().enumerate() {
                if let Some(base) = resolved(i, *variant) {
                    let drawn = self.drawn(w, base, &ramps, factors)?;
                    if Some(drawn) != own[i] {
                        map.insert(id, drawn);
                    }
                }
            }
        }
        let before: BTreeSet<Vec<Option<Entity>>> = ids
            .keys()
            .map(|(v, _)| (0..own.len()).map(|i| resolved(i, *v)).collect())
            .collect();
        let after: BTreeSet<Vec<Option<Entity>>> = ids
            .values()
            .map(|id| {
                maps.iter()
                    .zip(&own)
                    .map(|(m, o)| m.get(id).copied().or(*o))
                    .collect()
            })
            .collect();
        w.insert(proto, MaterialVariants(maps));
        for (p, key) in by_placement {
            if let (Some(&id), Some(i)) = (ids.get(&key), w.get_mut::<Instance>(p)) {
                i.variant = id;
            }
        }
        log.count("emissive prototypes given instance variation variants", 1);
        log.count(
            "prototype copies specialize-material-variants adds for the emission's instance variation",
            after.len().saturating_sub(before.len()) as u64,
        );
        Ok(())
    }
}

impl Pass for ApplyForzaEmissionVariation {
    fn name(&self) -> &'static str {
        "apply-forzatech-emission-variation"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn provides(&self) -> &[&'static str] {
        &[MaterialVariants::NAME, PbrMaterial::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Instance::NAME, MaterialVariants::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[EmissionVariation::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a hand-placed instance draws its emission at the instance variation factor of 1: its variation is not traced"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let varied: Vec<Entity> = w.iter::<EmissionVariation>().map(|(e, _)| e).collect();
        if !varied.is_empty() {
            let varied: BTreeSet<Entity> = varied.into_iter().collect();
            let mut placed: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
            for (p, i) in w.iter::<Instance>() {
                placed.entry(i.prototype).or_default().push(p);
            }
            let mut apply = Apply::default();
            for (proto, placements) in placed {
                apply.prototype(w, proto, &placements, log)?;
            }
            log.count(
                "emissive material copies made, one per material and factor",
                apply.copies_made,
            );
            for e in varied {
                w.remove::<EmissionVariation>(e);
            }
        }
        Ok(())
    }
}
