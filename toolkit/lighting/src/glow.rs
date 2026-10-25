//! How bright emissive surfaces draw (pass `scale-emission`): each emissive material's `emissive_strength` times a
//! factor the program states, by the name of the mesh that draws it or else of the material, so a surface's glow (its
//! bloom) is set apart from the light made of it (`emissive-lights`, run before).
//!
//! - **Factors.** `prototypes=PREFIX:FACTOR;...` states a factor for every primitive of a mesh (a prototype's, or a
//!   placed entity's own) whose name starts with a prefix (the longest that matches); `materials=PREFIX:FACTOR;...` for
//!   every other primitive whose material's name starts with one. A primitive neither names keeps its emission. Each
//!   factor finite and above 0; `none` states none of either.
//! - **Copies.** A material that every primitive drawing it scales by one factor is scaled in place. One drawn at
//!   several factors (a lamp's material on a bollard too, each its own) is copied once per factor its primitives need
//!   ([`World::duplicate`], named `NAME xFACTOR`), the copy scaled and those primitives drawn with it; the original
//!   keeps the primitives at factor 1, or, when none is, the most common factor in place. Only emissive materials
//!   (a factor above 0 and a strength above 0) are touched.
//!
//! Reports (counted): the materials scaled in place, the copies made, the primitives drawn with a scaled emission, and
//! the prefixes of either list that no name starts with.

use std::collections::BTreeMap;

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, Mesh, Name, World};

/// `scale-emission` (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct ScaleEmission {
    /// Mesh name prefixes and their factors.
    pub prototypes: BTreeMap<String, f64>,
    /// Material name prefixes and their factors, for a primitive no prototype prefix names.
    pub materials: BTreeMap<String, f64>,
}

impl ScaleEmission {
    /// The pass, its factors checked: each prefix nonempty without `;` or `:`, each factor finite and above 0.
    pub fn new(
        prototypes: BTreeMap<String, f64>,
        materials: BTreeMap<String, f64>,
    ) -> Result<ScaleEmission, String> {
        for (what, list) in [("prototypes", &prototypes), ("materials", &materials)] {
            for (prefix, factor) in list {
                if prefix.is_empty() || prefix.contains([';', ':']) {
                    return Err(format!(
                        "scale-emission: a {what} prefix {prefix:?} is empty or holds ';' or ':'"
                    ));
                }
                if !(factor.is_finite() && *factor > 0.0) {
                    return Err(format!(
                        "scale-emission: {what} {prefix}:{factor}: a factor is a number above 0"
                    ));
                }
            }
        }
        Ok(ScaleEmission {
            prototypes,
            materials,
        })
    }

    fn from_given(g: &Given) -> Result<ScaleEmission, String> {
        let list = |k: &str| -> Result<BTreeMap<String, f64>, String> {
            match g.required(k) {
                "none" => Ok(BTreeMap::new()),
                v => v
                    .split(';')
                    .map(|item| {
                        let (prefix, factor) = item
                            .split_once(':')
                            .ok_or_else(|| format!("{k}={v}: PREFIX:FACTOR;... or none"))?;
                        let factor = factor
                            .parse::<f64>()
                            .map_err(|_| format!("{k}={v}: {factor} is not a number"))?;
                        Ok((prefix.to_owned(), factor))
                    })
                    .collect(),
            }
        };
        ScaleEmission::new(list("prototypes")?, list("materials")?)
    }
}

/// `scale-emission`'s declaration ([`primase_core::pass::PassRegistry::add`]): both lists required.
pub const SCALE_EMISSION: PassSpec = PassSpec {
    name: "scale-emission",
    options: &[
        OptionSpec::required(
            "prototypes",
            "none",
            "PREFIX:FACTOR;...: the emission of every primitive of a mesh whose name starts with a prefix times its \
             factor; or none",
        ),
        OptionSpec::required(
            "materials",
            "none",
            "PREFIX:FACTOR;...: the emission of every other primitive whose material's name starts with a prefix \
             times its factor; or none",
        ),
    ],
    make: |g| Ok(Box::new(ScaleEmission::from_given(g)?)),
};

/// The factor of the longest prefix of `list` that `name` starts with, noting the prefix used.
fn longest<'a>(
    list: &'a BTreeMap<String, f64>,
    name: &str,
    used: &mut std::collections::BTreeSet<&'a str>,
) -> Option<f64> {
    list.iter()
        .filter(|(prefix, _)| name.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(prefix, factor)| {
            used.insert(prefix.as_str());
            *factor
        })
}

fn emits(m: &PbrMaterial) -> bool {
    m.emissive.iter().any(|&c| c > 0.0) && m.emissive_strength > 0.0
}

impl Pass for ScaleEmission {
    fn name(&self) -> &'static str {
        "scale-emission"
    }
    fn options(&self) -> Options {
        let text = |list: &BTreeMap<String, f64>| {
            if list.is_empty() {
                "none".to_owned()
            } else {
                list.iter()
                    .map(|(k, v)| format!("{k}:{v}"))
                    .collect::<Vec<_>>()
                    .join(";")
            }
        };
        let mut o = Options::new();
        o.insert("prototypes".into(), text(&self.prototypes));
        o.insert("materials".into(), text(&self.materials));
        o
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[Mesh::NAME, PbrMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, Mesh::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut used_p = std::collections::BTreeSet::new();
        let mut used_m = std::collections::BTreeSet::new();
        // each emissive material's primitives, (mesh holder, primitive index), by factor (its bits, 1 included)
        let mut users: BTreeMap<Entity, BTreeMap<u64, Vec<(Entity, usize)>>> = BTreeMap::new();
        let holders: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
        for &holder in &holders {
            let mesh_name = w.get::<Name>(holder).map(|n| n.0.clone()).unwrap_or_default();
            let Some(mesh) = w.get::<Mesh>(holder) else {
                continue;
            };
            for (i, prim) in mesh.primitives().iter().enumerate() {
                let Some(m) = prim.material else { continue };
                if !w.get::<PbrMaterial>(m).is_some_and(emits) {
                    continue;
                }
                let material_name = w.get::<Name>(m).map(|n| n.0.as_str()).unwrap_or_default();
                let factor = longest(&self.prototypes, &mesh_name, &mut used_p)
                    .or_else(|| longest(&self.materials, material_name, &mut used_m))
                    .unwrap_or(1.0);
                users
                    .entry(m)
                    .or_default()
                    .entry(factor.to_bits())
                    .or_default()
                    .push((holder, i));
            }
        }
        let one = 1.0f64.to_bits();
        let mut retarget: BTreeMap<Entity, Vec<(usize, Entity)>> = BTreeMap::new();
        for (m, by_factor) in &users {
            // the factor the original keeps: 1 when some primitive is at 1, else the most common (the lowest on a tie)
            let kept = if by_factor.contains_key(&one) {
                one
            } else {
                *by_factor
                    .iter()
                    .max_by(|a, b| a.1.len().cmp(&b.1.len()).then(b.0.cmp(a.0)))
                    .expect("a material here has users")
                    .0
            };
            for (&bits, prims) in by_factor {
                if bits == kept {
                    continue;
                }
                let factor = f64::from_bits(bits);
                let copy = w
                    .duplicate(*m)
                    .ok_or_else(|| format!("scale-emission: material {m:?} is not alive"))?;
                let name = w
                    .get::<Name>(*m)
                    .map(|n| n.0.clone())
                    .unwrap_or_else(|| format!("{m:?}"));
                w.insert(copy, Name(format!("{name} x{factor}")));
                if let Some(p) = w.get_mut::<PbrMaterial>(copy) {
                    p.emissive_strength = (f64::from(p.emissive_strength) * factor) as f32;
                }
                log.count("emissive materials copied for another factor", 1);
                log.count("primitives drawn with a scaled emission", prims.len() as u64);
                for &(holder, i) in prims {
                    retarget.entry(holder).or_default().push((i, copy));
                }
            }
            if kept != one {
                let factor = f64::from_bits(kept);
                if let Some(p) = w.get_mut::<PbrMaterial>(*m) {
                    p.emissive_strength = (f64::from(p.emissive_strength) * factor) as f32;
                }
                log.count("emissive materials scaled in place", 1);
                log.count(
                    "primitives drawn with a scaled emission",
                    by_factor[&kept].len() as u64,
                );
            }
        }
        for (holder, edits) in retarget {
            w.edit_mesh(holder, |mesh| {
                for (i, copy) in &edits {
                    let mut prim = mesh
                        .primitive_mut(*i)
                        .ok_or_else(|| format!("scale-emission: {holder:?} has no primitive {i}"))?;
                    *prim.fields().material = Some(*copy);
                }
                Ok(None)
            })?;
        }
        log.count(
            "prototypes prefixes that no mesh's name starts with",
            self.prototypes
                .keys()
                .filter(|k| !used_p.contains(k.as_str()))
                .count() as u64,
        );
        log.count(
            "materials prefixes that no emissive material's name starts with",
            self.materials
                .keys()
                .filter(|k| !used_m.contains(k.as_str()))
                .count() as u64,
        );
        Ok(())
    }
}
