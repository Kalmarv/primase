//! The Enhanced shader database in the world: one `core.shader` entity per vertex and pixel program (the container as
//! stored, its interface read from it) and one [`GtaEffect`] per effect naming its programs. The programs of the other
//! stages stay named in the effect with no entity: a `core.shader` has no stage for them yet.
//!
//! The database is read by [`crate::rage::sgd2`] from bytes; where the install keeps those bytes is the caller's
//! ([`crate::read::source::GtaSource::add_shader_database`] finds the file in the install's listing).
//!
//! A drawable's shader names its effect by a hash: [`GtaShader::name`] is the joaat of the effect's lowercase name
//! (every one of the 119 names of Vespucci's 31,821 materials is one of the database's 388 effects). The word after an
//! effect's name in the database is not that hash (it is the offset of something inside the record). [`join_materials`]
//! links each material to its effect.
use std::collections::BTreeMap;

use primase_core::shader::{Shader, ShaderFormat};
use primase_core::{Entity, Name, World};

use crate::components::{GtaEffect, GtaEffectProgram, GtaMaterialEffect, GtaShader, GtaStage};
use crate::rage::fxc::ProgramStage;
use crate::rage::sgd2::{self, Sgd2};

/// What [`add_shader_database`] made.
#[derive(Debug, Default)]
pub struct ShaderTable {
    /// The `gta.effect` entities, in the database's order.
    pub effects: Vec<Entity>,
    /// The vertex and pixel programs made into `core.shader` entities.
    pub shaders: usize,
    /// The vertex and pixel programs whose interface could not be read, by the reason: left without an entity.
    pub unread: Vec<(String, String)>,
}

/// What [`join_materials`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EffectJoin {
    /// Materials linked to an effect, and the distinct effects they use.
    pub joined: usize,
    pub effects_used: usize,
    /// Materials whose shader name no effect has, by the name's hash: the number of materials.
    pub unjoined: BTreeMap<u32, usize>,
    /// Shader names that two or more effects hash to, with the effects' names: their materials are left unjoined (the
    /// name does not say which).
    pub ambiguous: Vec<(u32, Vec<String>)>,
}

fn stage_of(stage: ProgramStage) -> GtaStage {
    match stage {
        ProgramStage::Vertex => GtaStage::Vertex,
        ProgramStage::Pixel => GtaStage::Pixel,
        ProgramStage::Geometry => GtaStage::Geometry,
        ProgramStage::Domain => GtaStage::Domain,
        ProgramStage::Hull => GtaStage::Hull,
        ProgramStage::Compute => GtaStage::Compute,
    }
}

/// Add the database's vertex and pixel programs and its effects to `w`.
pub fn add_shader_database(w: &mut World, db: &Sgd2<'_>) -> ShaderTable {
    let mut table = ShaderTable::default();
    // per group, the entity of each program (None for a stage with no `core.shader`, or an interface not read)
    let mut entities: Vec<Vec<Option<Entity>>> = Vec::new();
    for (group, stage) in sgd2::GROUP_STAGES.iter().enumerate() {
        let programs = db.groups.get(group).map_or(&[][..], Vec::as_slice);
        let mut made = Vec::with_capacity(programs.len());
        for program in programs {
            let entity = match stage {
                Some(ProgramStage::Vertex | ProgramStage::Pixel) => {
                    match primase_dxil::interface(program.container) {
                        Ok(interface) => {
                            let e = w.spawn();
                            w.insert(e, Name(program.name.to_owned()));
                            w.insert(
                                e,
                                Shader {
                                    format: ShaderFormat::Dxil,
                                    bytecode: program.container.to_vec(),
                                    interface,
                                },
                            );
                            table.shaders += 1;
                            Some(e)
                        }
                        Err(reason) => {
                            table.unread.push((program.name.to_owned(), reason.to_string()));
                            None
                        }
                    }
                }
                _ => None,
            };
            made.push(entity);
        }
        entities.push(made);
    }
    for effect in &db.effects {
        let mut named = Vec::new();
        for (list, group) in effect.programs.iter().zip(sgd2::EFFECT_LISTS) {
            let Some(stage) = sgd2::GROUP_STAGES.get(group).copied().flatten() else {
                continue;
            };
            for &index in list {
                let Some(program) = db.groups.get(group).and_then(|g| g.get(index as usize)) else {
                    continue;
                };
                named.push(GtaEffectProgram {
                    stage: stage_of(stage),
                    name: program.name.to_owned(),
                    shader: entities
                        .get(group)
                        .and_then(|g| g.get(index as usize))
                        .copied()
                        .flatten(),
                });
            }
        }
        let e = w.spawn();
        w.insert(e, Name(effect.name.to_owned()));
        w.insert(
            e,
            GtaEffect {
                name: effect.name.to_owned(),
                programs: named,
            },
        );
        table.effects.push(e);
    }
    table
}

/// Links every material of `w` (an entity with [`GtaShader`]) to the effect among `effects` (the entities
/// [`add_shader_database`] made) whose lowercase name hashes to the shader's name, with [`GtaMaterialEffect`].
pub fn join_materials(w: &mut World, effects: &[Entity]) -> EffectJoin {
    let mut join = EffectJoin::default();
    let mut by_hash: BTreeMap<u32, Vec<Entity>> = BTreeMap::new();
    for &e in effects {
        if let Some(effect) = w.get::<GtaEffect>(e) {
            by_hash
                .entry(primase_hash::joaat(effect.name.as_bytes()))
                .or_default()
                .push(e);
        }
    }
    let materials: Vec<(Entity, u32)> = w.iter::<GtaShader>().map(|(e, s)| (e, s.name)).collect();
    let mut used = std::collections::BTreeSet::new();
    for (material, name) in materials {
        match by_hash.get(&name).map(Vec::as_slice) {
            Some([effect]) => {
                w.insert(material, GtaMaterialEffect { effect: *effect });
                used.insert(*effect);
                join.joined += 1;
            }
            _ => *join.unjoined.entry(name).or_default() += 1,
        }
    }
    join.effects_used = used.len();
    for (hash, list) in &by_hash {
        if list.len() > 1 {
            let names = list
                .iter()
                .filter_map(|&e| w.get::<GtaEffect>(e).map(|f| f.name.clone()))
                .collect();
            join.ambiguous.push((*hash, names));
        }
    }
    join
}
