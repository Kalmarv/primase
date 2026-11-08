//! `derive-shaunwhite-albedo`: each resolved material's albedo derived from one of its stored forward Colour records
//! ([`crate::shaders::albedo`]), with the base UVs from the same record's stored vertex program
//! ([`crate::shaders::base_uv`]), as `core.shader_derivation`.
//!
//! The record is a selection: the lowest SDB2 record number among the material's exact compatible forward Colour
//! variants whose restored RGB slice has fully resolved material constants, textures and supported raw varyings. The
//! game picks a variant at run time from draw and frame state this does not model. The result is a derivation, not
//! the runtime pass selection or a stored albedo: lighting, fog, influence colour control, tone mapping and source
//! alpha coverage are cut. A material none of whose records derives keeps the last reason as the derivation's refusal.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use iridia_analysis::WordsRead;
use iridia_analysis::flow::{Node, Values};
use iridia_interface::{ResourceId, ResourceKind, Stage};
use iridia_ir::Program;
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::shader::{
    Attribute, ConstantRole, DerivedInput, DerivedStage, InputBinding, InputRole, LaneStream,
    MaterialPrograms, Shader, ShaderDerivation, StageBinding, TextureSource,
};
use primase_core::{Component, Entity, World};

use super::resolve_shaders::RECORD_FIELD;
use crate::components::{MaterialRef, ShaderInputs, ShaderLibrary};
use crate::shaders::albedo::{self, DerivedAlbedo, Influence};
use crate::shaders::base_uv;
use anvil_formats::shaunwhite::shaders::Database;

#[derive(Clone, Copy, Debug)]
pub struct DeriveShaunwhiteAlbedo {
    pub influence: Influence,
}

pub const DERIVE_SHAUNWHITE_ALBEDO: PassSpec = PassSpec {
    name: "derive-shaunwhite-albedo",
    options: &[OptionSpec::required(
        "influence",
        "restored",
        "restored or unrestored: the world-influence state the derivation is made for (its influence colour control \
         is cut either way, and recorded)",
    )],
    make: |g| {
        let influence = match g.required("influence") {
            "restored" => Influence::Restored,
            "unrestored" => Influence::Unrestored,
            other => return Err(format!("influence must be restored or unrestored, not {other}")),
        };
        Ok(Box::new(DeriveShaunwhiteAlbedo { influence }))
    },
};

const APPROXIMATES: &str = "albedo derived from the lowest exact compatible forward Colour record whose restored RGB \
    slice has fully resolved material constants, textures and supported raw varyings (base UVs from the same \
    record's vertex program when read): a derivation, not the runtime pass selection or a stored albedo; lighting, \
    fog, influence colour control, tone mapping and source alpha coverage are omitted";

fn influence_name(influence: Influence) -> &'static str {
    match influence {
        Influence::Restored => "restored",
        Influence::Unrestored => "unrestored",
    }
}

/// One record's derivation, made once for the first material that reaches it.
struct Template {
    derived: DerivedAlbedo,
    program: Arc<Program>,
    reads: WordsRead,
    textures: BTreeSet<ResourceId>,
    inputs: Vec<InputBinding>,
}

/// Whether the material's diffuse slot is sampled untransformed; `None` when it has none.
fn identity_base(material: &MaterialRef) -> Option<bool> {
    material
        .slots
        .get("diffuse")
        .map(|s| s.scale == [1.; 2] && s.offset == [0.; 2] && s.rotation == 0.)
}

fn template(
    db: &Database,
    record: u32,
    stage: &StageBinding,
    influence: Influence,
) -> Result<Template, String> {
    let derived = albedo::derive(db, record, stage, influence)?;
    let values = Values::of_program(&derived.program)?;
    let reached = values.reached_from(values.roots());
    let mut inputs = BTreeSet::new();
    let mut textures = BTreeSet::new();
    for n in reached {
        match values.node(n) {
            Node::Input { input, lane } => {
                let v = &derived.program.interface.inputs[*input];
                if v.label.name == "TEXCOORD" && v.label.index == 7 && *lane >= 2 {
                    return Err("base UV invocation supplies TEXCOORD7.xy only".into());
                }
                inputs.insert(*input);
            }
            Node::Texel { resource: id, .. } => {
                textures.insert(*id);
            }
            _ => {}
        }
    }
    let mut roles = Vec::new();
    for input in inputs {
        let v = &derived.program.interface.inputs[input];
        let role = match (v.label.name.as_str(), v.label.index) {
            ("COLOR", 0) => InputRole::Stream(Attribute::Color(0)),
            ("TEXCOORD", 0) => InputRole::Lanes(vec![
                LaneStream {
                    semantic: Attribute::TexCoord(0),
                    first: 0,
                    count: 2,
                    component: 0,
                },
                LaneStream {
                    semantic: Attribute::TexCoord(1),
                    first: 2,
                    count: 2,
                    component: 0,
                },
            ]),
            ("TEXCOORD", 7) => InputRole::Unknown {
                why: "supplied by the exact record's game-owned base-UV invocation".into(),
            },
            _ => {
                return Err(format!(
                    "varying {}{} belongs to the unlifted VS",
                    v.label.name, v.label.index
                ));
            }
        };
        roles.push(InputBinding {
            label: v.label.clone(),
            role,
        });
    }
    let reads = WordsRead::of_program(&derived.program)?;
    for r in &derived.program.interface.resources {
        if matches!(r.kind, ResourceKind::ConstantBuffer { .. }) && reads.indexed(r.id) {
            return Err("indexed material constants are not resolved".into());
        }
    }
    let program = Arc::new(derived.program.clone());
    Ok(Template {
        derived,
        program,
        reads,
        textures,
        inputs: roles,
    })
}

/// The material's stage bound for the derived program: its outputs, the input roles the derivation reads, every
/// material word it reads stated, and a neutral binding for each texture it does not read.
fn filled(t: &Template, stage: &StageBinding) -> Result<StageBinding, String> {
    let mut bound = stage.clone();
    bound.outputs = t.derived.binding.outputs.clone();
    for resource in t.reads.buffers() {
        for word in t.reads.words(resource) {
            let at = word * 4;
            let known = bound.constants.iter().any(|c| {
                c.resource == resource
                    && (c.offset..c.offset + c.size).contains(&at)
                    && matches!(c.role, ConstantRole::Material(_))
            });
            if !known {
                return Err(format!("read material word at byte{at} remains unresolved"));
            }
        }
    }
    for role in &t.inputs {
        if let Some(input) = bound.inputs.iter_mut().find(|i| i.label == role.label) {
            *input = role.clone();
        } else {
            bound.inputs.push(role.clone());
        }
    }
    for texture in &mut bound.textures {
        if !t.textures.contains(&texture.resource) {
            texture.source = TextureSource::Default {
                value: [0.; 4],
                why: "forward texture not used by derived RGB; neutral consumer binding".into(),
            };
        } else if !matches!(
            texture.source,
            TextureSource::Texture { .. } | TextureSource::Default { .. }
        ) {
            return Err("read material texture remains unresolved".into());
        }
    }
    if t.textures
        .iter()
        .any(|r| !bound.textures.iter().any(|b| b.resource == *r))
    {
        return Err("read material texture has no binding".into());
    }
    Ok(bound)
}

/// What became of one material: its derivation, or why it has none and whether any record was tried.
enum Outcome {
    Derived(Box<DerivedStage>),
    /// No record was tried: the material has no compatible record, or the library was not read.
    NotTried(String),
    /// Every compatible record was tried and refused; the last reason.
    Refused(String),
}

impl DeriveShaunwhiteAlbedo {
    fn derive(
        &self,
        w: &World,
        db: Option<&Database>,
        inputs: &ShaderInputs,
        entity: Entity,
        templates: &mut BTreeMap<u32, Result<Template, String>>,
        log: &mut PassLog,
    ) -> Outcome {
        let material = &inputs.material;
        if inputs.bindings.stages.is_empty() {
            let reasons = w
                .get::<MaterialPrograms>(entity)
                .map(|p| {
                    p.0.iter()
                        .filter_map(|f| f.not_lifted.clone())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            return Outcome::NotTried(if reasons.is_empty() {
                "source supplied no shader resolution diagnostic".into()
            } else {
                reasons.into_iter().collect::<Vec<_>>().join("; ")
            });
        }
        let Some(db) = db else {
            return Outcome::NotTried("shader database not read".into());
        };
        let mut stages = inputs
            .bindings
            .stages
            .iter()
            .filter(|s| {
                s.stage == Stage::Pixel && s.variant.as_ref().is_some_and(|v| v.field == RECORD_FIELD)
            })
            .collect::<Vec<_>>();
        stages.sort_by_key(|s| s.variant.as_ref().map(|v| v.value));
        let mut why = "no compatible forward Colour record".to_owned();
        for stage in stages {
            let Some(record) = stage.variant.as_ref().map(|v| v.value) else {
                continue;
            };
            if let Some(reason) = inputs.unresolved.get(&record) {
                why = reason.clone();
                continue;
            }
            let t = match templates
                .entry(record)
                .or_insert_with(|| template(db, record, stage, self.influence))
            {
                Ok(t) => t,
                Err(e) => {
                    why = e.clone();
                    continue;
                }
            };
            let bound = match filled(t, stage) {
                Ok(b) => b,
                Err(e) => {
                    why = e;
                    continue;
                }
            };
            if w.get::<Shader>(bound.shader).is_none() {
                why = "original shader provenance not read".into();
                continue;
            }
            let d = &t.derived.derivation;
            let mut notes = d.cuts.clone();
            notes.extend(d.cut_inputs.iter().map(|input| format!("cut input: {input}")));
            notes.push(format!(
                "{}: derived from lit forward Colour, record {}, permutation {:#x}, pixel cache key {:#x}, original {}, \
                 {} influence; base UV supplied by the game-owned VS graph when read",
                d.name,
                record,
                d.key.permutation,
                d.pixel_key,
                d.source_hash,
                influence_name(self.influence)
            ));
            let target = t
                .inputs
                .iter()
                .find(|i| i.label.name == "TEXCOORD" && i.label.index == 7)
                .map(|i| i.label.clone());
            let vertex_input = match target {
                None => None,
                Some(target) => match base_uv::derive(db, record, stage, material) {
                    Ok(uv) => {
                        let d = &uv.derivation;
                        let mut notes = d.cuts.clone();
                        notes.extend(d.cut_inputs.iter().map(|i| format!("cut input: {i}")));
                        notes.push(format!(
                            "{}: record {}, permutation {:#x}, native VS key {:#x}, original {}; evaluated once on raw \
                             vertex UVs before interpolation",
                            d.name, record, d.key.permutation, d.vertex_key, d.source_hash
                        ));
                        Some(DerivedInput {
                            target,
                            name: d.name.into(),
                            program: Arc::new(uv.program),
                            notes,
                            inputs: uv.inputs,
                            constants: uv.constants,
                        })
                    }
                    Err(e) => {
                        why = e;
                        continue;
                    }
                },
            };
            log.count(
                &format!("derived from record {record}, key {:#x}", d.key.permutation),
                1,
            );
            if vertex_input.is_some() {
                log.count("derived with the record's base UVs", 1);
            }
            if identity_base(material) == Some(false) {
                log.count("derived with a non-identity base UV transform", 1);
            }
            return Outcome::Derived(Box::new(DerivedStage {
                stage: bound,
                state: inputs.bindings.state,
                program: Arc::clone(&t.program),
                notes,
                vertex_input,
            }));
        }
        Outcome::Refused(why)
    }
}

impl Pass for DeriveShaunwhiteAlbedo {
    fn name(&self) -> &'static str {
        "derive-shaunwhite-albedo"
    }
    fn options(&self) -> Options {
        Options::from([("influence".into(), influence_name(self.influence).into())])
    }
    fn requires(&self) -> &[&'static str] {
        &[ShaderInputs::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[ShaderDerivation::NAME]
    }
    fn approximates(&self) -> &'static str {
        APPROXIMATES
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let db = w.resource::<ShaderLibrary>().map(|s| Arc::clone(&s.0));
        let materials: Vec<(Entity, ShaderInputs)> =
            w.iter::<ShaderInputs>().map(|(e, i)| (e, i.clone())).collect();
        let mut templates = BTreeMap::new();
        for (entity, inputs) in materials {
            log.count(
                match identity_base(&inputs.material) {
                    Some(true) => "materials with an identity base UV transform",
                    Some(false) => "materials with a non-identity base UV transform",
                    None => "materials with no diffuse slot",
                },
                1,
            );
            let result = match self.derive(w, db.as_deref(), &inputs, entity, &mut templates, log) {
                Outcome::Derived(derived) => {
                    log.approximate_on(entity, "albedo derived from a forward Colour record", 1);
                    Ok(*derived)
                }
                Outcome::NotTried(why) => {
                    log.count(&format!("no record tried: {why}"), 1);
                    Err(why)
                }
                Outcome::Refused(why) => {
                    log.count(&format!("every record refused: {why}"), 1);
                    Err(why)
                }
            };
            w.insert(
                entity,
                ShaderDerivation {
                    name: albedo::NAME.into(),
                    result,
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::SlotRef;
    use primase_core::shader::{ProgramFacts, ShaderBinding};

    fn inputs(slots: BTreeMap<String, SlotRef>) -> ShaderInputs {
        ShaderInputs {
            material: MaterialRef {
                id: 1,
                name: "unresolved fixture".into(),
                state: None,
                slots,
                unread: Vec::new(),
            },
            bindings: ShaderBinding {
                stages: Vec::new(),
                state: Default::default(),
            },
            unresolved: BTreeMap::new(),
        }
    }

    #[test]
    fn a_material_with_no_record_keeps_the_resolvers_reason_once() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, inputs(BTreeMap::new()));
        world.insert(
            entity,
            MaterialPrograms(vec![ProgramFacts::unavailable(
                "fixture",
                Stage::Pixel,
                None,
                "no supported static draw",
            )]),
        );
        let pass = DeriveShaunwhiteAlbedo {
            influence: Influence::Restored,
        };
        let mut log = PassLog::default();
        pass.run(&mut world, &mut log).unwrap();
        let derivation = world.get::<ShaderDerivation>(entity).unwrap();
        assert_eq!(derivation.name, albedo::NAME);
        assert_eq!(
            derivation.result.as_ref().err().map(String::as_str),
            Some("no supported static draw")
        );
        assert_eq!(log.counts["no record tried: no supported static draw"], 1);
        assert_eq!(log.counts["materials with no diffuse slot"], 1);
        assert_eq!(log.counts.len(), 2);
        assert!(log.approximations.is_empty());
    }

    #[test]
    fn base_uv_scope_checks_authored_fields_not_material_names() {
        let mut w = World::new();
        let e = w.spawn();
        let mut m = inputs(BTreeMap::new()).material;
        m.name = "unrelated name".into();
        assert_eq!(identity_base(&m), None);
        m.slots.insert(
            "diffuse".into(),
            SlotRef {
                texture: e,
                scale: [1.; 2],
                offset: [0.; 2],
                rotation: 0.,
                texcoord: 0,
                address: None,
            },
        );
        assert_eq!(identity_base(&m), Some(true));
        m.slots.get_mut("diffuse").unwrap().scale[0] = 2.;
        assert_eq!(identity_base(&m), Some(false));
        m.slots.get_mut("diffuse").unwrap().scale[0] = 1.;
        m.slots.get_mut("diffuse").unwrap().rotation = 90.;
        assert_eq!(identity_base(&m), Some(false));
    }
}
