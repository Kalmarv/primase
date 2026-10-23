//! The Enhanced shader database in the world: one `core.shader` entity per vertex and pixel program (the container as
//! stored, its interface read from it) and one [`GtaEffect`] per effect naming its programs. The programs of the other
//! stages stay named in the effect with no entity: a `core.shader` has no stage for them yet.
//!
//! The database is read by [`crate::rage::sgd2`] from bytes; where the install keeps those bytes is the caller's.
use primase_core::shader::{Shader, ShaderFormat};
use primase_core::{Entity, Name, World};

use crate::components::{GtaEffect, GtaEffectProgram, GtaStage};
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
