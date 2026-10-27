//! The Enhanced shader database in the world: one `core.shader` entity per vertex, pixel, hull, domain and compute program (the container as
//! stored, its interface read from it) and one [`GtaEffect`] per effect naming its programs. A geometry program stays
//! named in the effect with no entity: a `core.shader` has no facts for it yet.
//!
//! The database is read by [`crate::rage::sgd2`] from bytes; where the install keeps those bytes is the caller's
//! ([`crate::read::source::GtaSource::add_shader_database`] finds the file in the install's listing).
//!
//! A drawable's shader names its effect by a hash: [`GtaShader::name`] is the joaat of the effect's lowercase name
//! (every one of the 119 names of Vespucci's 31,821 materials is one of the database's 388 effects). The word after an
//! effect's name in the database is not that hash (it is the offset of something inside the record). [`join_materials`]
//! links each material to its effect.
use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::ShaderFormat;
use primase_core::shader::Shader;
use primase_core::{Entity, Name, World};

use crate::components::{
    GtaEffect, GtaEffectParameters, GtaEffectProgram, GtaEffectTechniques, GtaMaterialEffect, GtaPass,
    GtaShader, GtaShaderTable, GtaStage, GtaTechnique,
};
use crate::rage::fxc::ProgramStage;
use crate::rage::sgd2::{self, Sgd2};
use crate::rage::sgd2_effect;

/// What [`add_shader_database`] made.
#[derive(Debug, Default)]
pub struct ShaderTable {
    /// The `gta.effect` entities, in the database's order.
    pub effects: Vec<Entity>,
    /// The programs made into `core.shader` entities.
    pub shaders: usize,
    /// Of them, those whose binding table was read (`gta.shader_table`); the others are in `unread_tables`, by the
    /// reason.
    pub tables: usize,
    pub unread_tables: Vec<(String, String)>,
    /// Parameter records found in the effects' records (`gta.effect_parameters`).
    pub parameters: usize,
    /// The effects whose techniques were read (`gta.effect_techniques`), and the passes in them.
    pub techniques: usize,
    pub passes: usize,
    /// The effects with no technique table (the post-processing and lighting effects).
    pub without_techniques: usize,
    /// The effects whose technique table was found and does not fit the effect, by the reason.
    pub unread_techniques: Vec<(String, String)>,
    /// The programs whose interface could not be read, by the reason: left without an entity.
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
    add_shader_effects(w, db, None)
}

/// The effects of the database `w`'s materials name ([`GtaShader::name`]), with only the programs those effects list,
/// added to `w`: the same entities and components as [`add_shader_database`] makes for them, for a world that holds a
/// part of a map and not the whole database's 11,000 programs.
pub fn add_used_shader_effects(w: &mut World, db: &Sgd2<'_>) -> ShaderTable {
    let used: BTreeSet<u32> = w.iter::<GtaShader>().map(|(_, s)| s.name).collect();
    add_shader_effects(w, db, Some(&used))
}

/// The database's effects whose name hashes ([`GtaShader::name`]) are in `only` (all of them for `None`) and the
/// programs they list, added to `w`.
pub fn add_shader_effects(w: &mut World, db: &Sgd2<'_>, only: Option<&BTreeSet<u32>>) -> ShaderTable {
    let mut table = ShaderTable::default();
    let wanted = |effect: &sgd2::Effect<'_>| {
        only.is_none_or(|hashes| hashes.contains(&primase_foundations::hash::joaat(effect.name.as_bytes())))
    };
    // the programs the wanted effects list, by group
    let mut needed: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); sgd2::GROUP_STAGES.len()];
    for effect in db.effects.iter().filter(|e| wanted(e)) {
        for (list, group) in effect.programs.iter().zip(sgd2::EFFECT_LISTS) {
            if let Some(set) = needed.get_mut(group) {
                set.extend(list.iter().map(|&i| i as usize));
            }
        }
    }
    // per group, the entity of each program (None for a stage with no `core.shader`, or an interface not read, or a
    // program no wanted effect lists)
    let mut entities: Vec<Vec<Option<Entity>>> = Vec::new();
    for (group, stage) in sgd2::GROUP_STAGES.iter().enumerate() {
        let programs = db.groups.get(group).map_or(&[][..], Vec::as_slice);
        let mut made = Vec::with_capacity(programs.len());
        for (index, program) in programs.iter().enumerate() {
            let listed = only.is_none() || needed.get(group).is_some_and(|s| s.contains(&index));
            let entity = match stage {
                _ if !listed => None,
                Some(
                    ProgramStage::Vertex
                    | ProgramStage::Pixel
                    | ProgramStage::Compute
                    | ProgramStage::Domain
                    | ProgramStage::Hull,
                ) => match iridia_dxil::interface(program.container) {
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
                        match crate::rage::sgd2_table::parse(program.tables) {
                            Ok(t) => {
                                w.insert(e, GtaShaderTable(t));
                                table.tables += 1;
                            }
                            Err(reason) => table
                                .unread_tables
                                .push((program.name.to_owned(), reason.to_string())),
                        }
                        Some(e)
                    }
                    Err(reason) => {
                        table.unread.push((program.name.to_owned(), reason.to_string()));
                        None
                    }
                },
                _ => None,
            };
            made.push(entity);
        }
        entities.push(made);
    }
    for effect in db.effects.iter().filter(|e| wanted(e)) {
        let mut named = Vec::new();
        // the programs of each of the effect's lists, in the list's order
        let mut listed: Vec<Vec<GtaEffectProgram>> = Vec::new();
        for (list, group) in effect.programs.iter().zip(sgd2::EFFECT_LISTS) {
            listed.push(Vec::new());
            let Some(stage) = sgd2::GROUP_STAGES.get(group).copied().flatten() else {
                continue;
            };
            for &index in list {
                let Some(program) = db.groups.get(group).and_then(|g| g.get(index as usize)) else {
                    continue;
                };
                let entry = GtaEffectProgram {
                    stage: stage_of(stage),
                    name: program.name.to_owned(),
                    shader: entities
                        .get(group)
                        .and_then(|g| g.get(index as usize))
                        .copied()
                        .flatten(),
                };
                if let Some(l) = listed.last_mut() {
                    l.push(entry.clone());
                }
                named.push(entry);
            }
        }
        // what its programs' tables read: each constant's name hash with its type bytes
        let mut known: BTreeMap<u32, std::collections::BTreeSet<u8>> = BTreeMap::new();
        for p in &named {
            if let Some(GtaShaderTable(t)) = p.shader.and_then(|s| w.get::<GtaShaderTable>(s)) {
                for c in &t.constants {
                    known.entry(c.name).or_default().insert(c.kind());
                }
            }
        }
        let parameters = crate::rage::sgd2_table::effect_parameters(effect.rest, &known);
        table.parameters += parameters.len();
        let e = w.spawn();
        w.insert(e, Name(effect.name.to_owned()));
        w.insert(
            e,
            GtaEffect {
                name: effect.name.to_owned(),
                programs: named,
            },
        );
        w.insert(e, GtaEffectParameters(parameters));
        match crate::rage::sgd2_effect::parse(effect.rest) {
            Ok(Some(t)) => match techniques(&t, &listed) {
                Ok(t) => {
                    table.techniques += 1;
                    table.passes += t.techniques.iter().map(|t| t.passes.len()).sum::<usize>();
                    w.insert(e, t);
                }
                Err(reason) => table.unread_techniques.push((effect.name.to_owned(), reason)),
            },
            Ok(None) => table.without_techniques += 1,
            Err(reason) => table
                .unread_techniques
                .push((effect.name.to_owned(), reason.to_string())),
        }
        table.effects.push(e);
    }
    table
}

/// An effect's techniques with each pass's programs named: positions are 1-based in the effect's lists of a stage
/// (vertex, pixel, geometry, domain, hull, compute).
fn techniques(
    t: &sgd2_effect::EffectTechniques,
    listed: &[Vec<GtaEffectProgram>],
) -> Result<GtaEffectTechniques, String> {
    let at = |list: usize, position: u8| -> Result<Option<GtaEffectProgram>, String> {
        let Some(index) = usize::from(position).checked_sub(1) else {
            return Ok(None);
        };
        listed
            .get(list)
            .and_then(|l| l.get(index))
            .cloned()
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "a pass names program {position} of a list of {}",
                    listed.get(list).map_or(0, Vec::len)
                )
            })
    };
    let mut out = Vec::with_capacity(t.techniques.len());
    for technique in &t.techniques {
        let mut passes = Vec::with_capacity(technique.passes.len());
        for p in &technique.passes {
            passes.push(GtaPass {
                vertex: at(0, p.vertex)?,
                pixel: at(1, p.pixel)?,
                geometry: at(2, p.geometry)?,
                domain: at(3, p.domain)?,
                hull: at(4, p.hull)?,
                compute: at(5, p.compute)?,
                rasterizer: p.rasterizer,
                depth_stencil: p.depth_stencil,
                blend: p.blend,
                record: p.raw.clone(),
            });
        }
        out.push(GtaTechnique {
            name: technique.name.clone(),
            passes,
        });
    }
    Ok(GtaEffectTechniques {
        techniques: out,
        rasterizer: t.rasterizer.clone(),
        depth_stencil: t.depth_stencil.clone(),
        blend: t.blend.clone(),
        samplers: t.samplers.clone(),
    })
}

/// Links every material of `w` (an entity with [`GtaShader`]) to the effect among `effects` (the entities
/// [`add_shader_database`] made) whose lowercase name hashes to the shader's name, with [`GtaMaterialEffect`].
pub fn join_materials(w: &mut World, effects: &[Entity]) -> EffectJoin {
    let mut join = EffectJoin::default();
    let mut by_hash: BTreeMap<u32, Vec<Entity>> = BTreeMap::new();
    for &e in effects {
        if let Some(effect) = w.get::<GtaEffect>(e) {
            by_hash
                .entry(primase_foundations::hash::joaat(effect.name.as_bytes()))
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
