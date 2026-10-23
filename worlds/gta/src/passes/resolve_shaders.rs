//! `resolve-gta-shaders{pixel=NAME,skinned=yes|no,unset-textures=white|leave}`: a drawable's material bound to its
//! effect's programs, as `core.shader_binding` on the material's entity. The programs are the `core.shader` entities
//! of the shader database ([`crate::read::shaders`]); what they read comes from the database's own tables
//! ([`crate::rage::sgd2_table`]) and the material's stored parameters.
//!
//! - **Programs.** An effect lists every permutation of its programs; the draw picks one by pass and vertex format.
//!   `pixel` names the pixel program family by its name without the permutation hash (`PS_DeferredTextured`: the
//!   G-buffer pass; `PS_DeferredTextured_AlphaClipC`: its alpha-clipped variant); each of the effect's programs of
//!   that name has a hash suffix shared with its vertex program (`PS_DeferredTextured_0174add1_Wrapped` and
//!   `VS_TransformD_0174add1_Wrapped`), and `skinned` chooses between the vertex programs that read bone weights
//!   (`VS_TransformSkinD_f146f347_Wrapped`) and those that do not. The tessellated vertex programs (`..._PNTri_...`,
//!   which feed hull and domain programs) are not chosen. An effect with no such pair, or with several, leaves its
//!   materials unbound, by name; the draw's choice among permutations is the program's to state, not this pass's.
//! - **Constants.** Each constant buffer a program declares is the table record at its register and space; each
//!   constant of the record is bound at its byte offset (a constant past the end of the buffer the program declares is
//!   one it does not read, and is left out, counted). A constant the effect renames (its parameter records:
//!   [`GtaEffectParameters`]) is a material parameter, bound to the words the material stores under the new name
//!   (`Material`), or `Unknown` where it stores none; the engine's world, view and projection matrices have the roles
//!   [`engine_role`] names; every other engine constant is `Unknown` with its name.
//! - **Textures.** A texture the program declares is the table record at its register and space; the material
//!   parameter of the record's second name binds it (`Texture`), unset takes `unset-textures` (`white`: white,
//!   counted as an approximation; `leave`: `Missing`), and a texture with no second name (the engine's) is `Missing`.
//!   Samplers are bindless in the programs (an unbounded array indexed at run time): none is bound.
//! - **Inputs and outputs.** The vertex stage's inputs are the mesh streams their semantics name (terrain weights are
//!   `COLOR1`); the pixel stage's are the vertex stage's. Every output is `Unknown` by its index: the pass states no
//!   G-buffer layout it has not read.
use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{
    ConstantBinding, ConstantRole, D3dClass, InputBinding, InputRole, ObjectParameter, OutputBinding,
    OutputRole, RenderState, ResourceKind, SamplerState, Shader, ShaderBinding, Stage, StageBinding,
    TextureBinding, TextureSource, ViewParameter,
};
use primase_core::{Component, Entity, Semantic, Texture, TextureFile, World};

use crate::components::{
    GtaEffect, GtaEffectParameters, GtaEffectProgram, GtaMaterialEffect, GtaParam, GtaShader, GtaShaderTable,
    GtaStage,
};
use crate::rage::sgd2_table::{ProgramTable, TableConstant, TableResource};

/// What a texture parameter with no texture becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsetTexture {
    White,
    Leave,
}

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveGtaShaders {
    pixel: String,
    skinned: bool,
    unset: UnsetTexture,
}

impl ResolveGtaShaders {
    pub fn new(pixel: &str, skinned: bool, unset: UnsetTexture) -> Self {
        ResolveGtaShaders {
            pixel: pixel.to_owned(),
            skinned,
            unset,
        }
    }

    /// From its options (all three required), through [`ResolveGtaShaders::new`].
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let (mut pixel, mut skinned, mut unset) = (None, None, None);
        for (k, v) in o {
            match k.as_str() {
                "pixel" => {
                    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                        return Err(format!("pixel={v:?} is not a program name without its hash"));
                    }
                    pixel = Some(v.clone());
                }
                "skinned" => {
                    skinned = Some(match v.as_str() {
                        "yes" => true,
                        "no" => false,
                        _ => return Err(format!("skinned={v} is not yes or no")),
                    });
                }
                "unset-textures" => {
                    unset = Some(match v.as_str() {
                        "white" => UnsetTexture::White,
                        "leave" => UnsetTexture::Leave,
                        _ => return Err(format!("unset-textures={v} is not white or leave")),
                    });
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Self {
            pixel: pixel
                .ok_or("pixel is required: the pixel program family drawn with (PS_DeferredTextured...)")?,
            skinned: skinned.ok_or("skinned is required: whether the vertex program reads bone weights")?,
            unset: unset
                .ok_or("unset-textures is required: what a texture parameter with no texture reads")?,
        })
    }
}

/// The permutation hash and the name without it of a program name `<stem>_<8 hex digits>_Wrapped`.
pub fn split_program_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_suffix("_Wrapped")?;
    let (stem, hash) = rest.rsplit_once('_')?;
    (hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some((stem, hash))
}

/// The pixel program of `pixel` family and its vertex program, among an effect's programs: the one pair whose names
/// share a permutation hash, the vertex program reading bone weights as `skinned` says and not tessellated.
pub fn select_programs<'a>(
    programs: &'a [GtaEffectProgram],
    pixel: &str,
    skinned: bool,
) -> Result<(&'a GtaEffectProgram, &'a GtaEffectProgram), String> {
    let mut pairs = Vec::new();
    for p in programs
        .iter()
        .filter(|p| p.stage == GtaStage::Pixel && p.shader.is_some())
    {
        let Some((stem, hash)) = split_program_name(&p.name) else {
            continue;
        };
        if stem != pixel {
            continue;
        }
        for v in programs
            .iter()
            .filter(|v| v.stage == GtaStage::Vertex && v.shader.is_some())
        {
            let Some((vstem, vhash)) = split_program_name(&v.name) else {
                continue;
            };
            if vhash == hash && !vstem.contains("PNTri") && vstem.contains("Skin") == skinned {
                pairs.push((p, v));
            }
        }
    }
    match pairs.as_slice() {
        [] => Err(format!(
            "the effect has no {pixel} program with a{} untessellated vertex program of its hash",
            if skinned { " skinned" } else { "n unskinned" }
        )),
        [one] => Ok(*one),
        many => Err(format!(
            "the effect has {} {pixel} permutations that fit",
            many.len()
        )),
    }
}

/// The role of an engine constant, by the program's name for it: the object and view matrices the vocabulary names.
/// Every other engine constant has no role here. (`gWorld` and `gWorldViewProj` are the object's matrices,
/// `gViewInverse` the camera's.)
pub fn engine_role(name: &str) -> Option<ConstantRole> {
    Some(match name {
        "gWorld" => ConstantRole::Object(ObjectParameter::World),
        "gWorldViewProj" => ConstantRole::Object(ObjectParameter::WorldViewProjection),
        "gViewInverse" => ConstantRole::View(ViewParameter::InverseView),
        _ => return None,
    })
}

/// The bytes a constant's value takes, by its type byte (9 one float, 10 two, 11 three, 12 four, 14 a matrix, 8 four
/// words; the flags' high 16 bits an array's element count; 0, 1, 5 one 32-bit word, 6 two): `None` for a type this
/// pass does not know. `tests/program_tables.rs` holds the sizes to the offsets of the constants that follow each.
pub fn value_bytes(flags: u32) -> Option<u32> {
    let count = (flags >> 16).max(1);
    Some(match flags & 0xff {
        0 | 1 | 5 | 9 => 4,
        6 | 10 => 8,
        11 => 12,
        8 => 16,
        12 => 16u32.checked_mul(count)?,
        14 => 64u32.checked_mul(count)?,
        _ => return None,
    })
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The semantic of a vertex input by its signature name and index.
fn stream(name: &str, index: u32) -> Option<Semantic> {
    let n = u8::try_from(index).ok()?;
    match name.to_ascii_uppercase().as_str() {
        "POSITION" => Some(Semantic::Position),
        "NORMAL" => Some(Semantic::Normal),
        "TANGENT" => Some(Semantic::Tangent),
        "COLOR" => Some(Semantic::Color(n)),
        "TEXCOORD" => Some(Semantic::TexCoord(n)),
        "BLENDWEIGHT" | "BLENDWEIGHTS" => Some(Semantic::Weights(n)),
        "BLENDINDICES" => Some(Semantic::Joints(n)),
        _ => None,
    }
}

/// What a material and its effect give one program's binding.
struct Inputs<'a> {
    material: &'a GtaShader,
    /// The effect's parameter records: the program's name hash to the key the material stores the parameter under.
    renamed: BTreeMap<u32, u32>,
    unset: UnsetTexture,
}

impl Inputs<'_> {
    fn param(&self, key: u32) -> Option<&GtaParam> {
        self.material
            .params
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, p)| p)
    }
}

/// The resource record of `table` for a bind point, among those of its register and space: the constant buffer record
/// has the size the program declares rounded up to 16 (or states none: the root constants' record has size 0); any
/// other has the first with a current name, else the first.
fn record(table: &ProgramTable, register: u32, space: u32, buffer_size: Option<u32>) -> Option<usize> {
    let at: Vec<usize> = table
        .resources
        .iter()
        .enumerate()
        .filter(|(_, r)| u32::from(r.register) == register && u32::from(r.space) == space)
        .map(|(i, _)| i)
        .collect();
    match buffer_size {
        Some(size) => {
            let rounded = size.div_ceil(16).saturating_mul(16);
            let sized = |size: u32| {
                at.iter()
                    .copied()
                    .find(|&i| table.resources.get(i).is_some_and(|r| r.size == size))
            };
            sized(rounded).or_else(|| sized(0))
        }
        None => at
            .iter()
            .copied()
            .find(|&i| table.resources.get(i).is_some_and(|r| r.second_name != 0))
            .or_else(|| at.first().copied()),
    }
}

/// Counts of what one run resolved, by reason.
#[derive(Default)]
struct Counts(BTreeMap<String, u64>);

impl Counts {
    fn add(&mut self, what: &str) {
        *self.0.entry(what.to_owned()).or_default() += 1;
    }
}

impl ResolveGtaShaders {
    #[allow(clippy::too_many_arguments)]
    fn stage_binding(
        &self,
        world: &World,
        inputs: &Inputs<'_>,
        stage: Stage,
        shader: Entity,
        vertex_bound: bool,
        counts: &mut Counts,
    ) -> Result<StageBinding, String> {
        let program = world
            .get::<Shader>(shader)
            .ok_or("a program entity without its shader")?;
        let table = &world
            .get::<GtaShaderTable>(shader)
            .ok_or("a program entity without its binding table")?
            .0;
        let interface = &program.interface;
        let mut constants = Vec::new();
        let mut textures = Vec::new();
        for r in &interface.resources {
            let Some(bind) = r.provenance.d3d else { continue };
            match (&r.kind, bind.class) {
                (ResourceKind::ConstantBuffer { layout }, D3dClass::ConstantBuffer) => {
                    let Some(i) = record(table, bind.register, bind.space, Some(layout.size)) else {
                        counts.add("constant buffers with no table record of their size");
                        continue;
                    };
                    for c in table.constants_of(i) {
                        let Some(bytes) = value_bytes(c.flags) else {
                            counts.add(&format!(
                                "constants of type byte {} (flags {:#x}), which this pass does not size, left out",
                                c.kind(),
                                c.flags >> 8
                            ));
                            continue;
                        };
                        if u32::from(c.offset).saturating_add(bytes) > layout.size {
                            counts.add("constants past the end of the buffer the program declares (not read), left out");
                            continue;
                        }
                        let name = table.name_of(c.name).map(str::to_owned);
                        let role = constant_role(inputs, c, bytes, name.as_deref(), bind.space, counts);
                        constants.push(ConstantBinding {
                            resource: r.id,
                            offset: u32::from(c.offset),
                            size: bytes,
                            name,
                            role,
                        });
                    }
                }
                (ResourceKind::Texture { .. }, D3dClass::ShaderResource) => {
                    let source = match record(table, bind.register, bind.space, None)
                        .and_then(|i| table.resources.get(i))
                    {
                        Some(t) => texture_source(world, inputs, t, counts),
                        None => {
                            counts.add("texture slots with no table record, left Missing");
                            TextureSource::Missing
                        }
                    };
                    textures.push(TextureBinding {
                        resource: r.id,
                        sampler: None,
                        source,
                    });
                }
                _ => {}
            }
        }
        constants.sort_by_key(|c| (c.resource.0, c.offset));
        let in_roles = interface
            .inputs
            .iter()
            .filter(|v| v.system.is_none())
            .map(|v| InputBinding {
                label: v.label.clone(),
                role: match stage {
                    Stage::Vertex => stream(&v.label.name, v.label.index).map_or_else(
                        || InputRole::Unknown {
                            why: format!(
                                "vertex input {} is not a mesh semantic this pass maps",
                                v.label.name
                            ),
                        },
                        InputRole::Stream,
                    ),
                    Stage::Pixel if vertex_bound => InputRole::Previous,
                    _ => InputRole::Unknown {
                        why: "the vertex stage is not bound".into(),
                    },
                },
            })
            .collect();
        let mut outputs = Vec::new();
        if stage == Stage::Pixel {
            for v in interface.outputs.iter().filter(|v| v.system.is_none()) {
                let full: u8 = ((1u16 << v.value.width) - 1) as u8;
                outputs.push(OutputBinding {
                    label: v.label.clone(),
                    lanes: full,
                    role: OutputRole::Unknown(v.label.index),
                });
            }
        }
        Ok(StageBinding {
            stage,
            shader,
            inputs: in_roles,
            constants,
            textures,
            outputs,
        })
    }
}

/// The role of one constant of a program's buffer.
fn constant_role(
    inputs: &Inputs<'_>,
    c: &TableConstant,
    bytes: u32,
    name: Option<&str>,
    space: u32,
    counts: &mut Counts,
) -> ConstantRole {
    if let Some(&key) = inputs.renamed.get(&c.name) {
        return match inputs.param(key) {
            Some(GtaParam::Floats(v)) => {
                counts.add(&format!(
                    "material parameters bound to the material's stored words (constant buffer space {space})"
                ));
                let mut words = bits(v);
                words.truncate(usize::try_from(bytes / 4).unwrap_or(0));
                ConstantRole::Material(words)
            }
            Some(_) => {
                counts.add("material parameters the material stores as another kind of value, Unknown");
                ConstantRole::Unknown {
                    why: "the material stores this parameter as a texture, sampler or other value".into(),
                }
            }
            None => {
                counts.add(&format!(
                    "renamed constants the material stores no value for (constant buffer space {space}), Unknown"
                ));
                ConstantRole::Unknown {
                    why: format!("the material stores no value under {key:08x}"),
                }
            }
        };
    }
    match name.and_then(engine_role) {
        Some(role) => {
            counts.add("engine constants with a role");
            role
        }
        None => {
            counts.add("engine constants without a role, Unknown");
            ConstantRole::Unknown {
                why: format!(
                    "engine constant {}: no role resolved",
                    name.unwrap_or("(unnamed)")
                ),
            }
        }
    }
}

/// What a texture record's parameter samples.
fn texture_source(
    world: &World,
    inputs: &Inputs<'_>,
    t: &TableResource,
    counts: &mut Counts,
) -> TextureSource {
    if t.second_name == 0 || t.second_name == t.name {
        counts.add("texture slots that are the engine's, left Missing");
        return TextureSource::Missing;
    }
    match inputs.param(t.second_name) {
        Some(GtaParam::Texture {
            texture: Some(texture),
            ..
        }) => {
            if world.has::<Texture>(*texture) || world.has::<TextureFile>(*texture) {
                counts.add("texture parameters bound to the material's texture");
                TextureSource::Texture {
                    texture: *texture,
                    sampler: SamplerState::default(),
                }
            } else {
                counts.add("texture parameters whose texture is not loaded, left Missing");
                TextureSource::Missing
            }
        }
        Some(GtaParam::Texture { texture: None, .. }) | None => match inputs.unset {
            UnsetTexture::White => {
                counts.add("texture parameters with no texture, bound white (unset-textures=white)");
                TextureSource::Default {
                    value: [1.0; 4],
                    why: "no texture: white (unset-textures=white)".into(),
                }
            }
            UnsetTexture::Leave => {
                counts.add("texture parameters with no texture, left Missing");
                TextureSource::Missing
            }
        },
        Some(_) => {
            counts.add("texture parameters stored as another kind of value, left Missing");
            TextureSource::Missing
        }
    }
}

impl Pass for ResolveGtaShaders {
    fn name(&self) -> &'static str {
        "resolve-gta-shaders"
    }
    fn options(&self) -> Options {
        Options::from([
            ("pixel".to_owned(), self.pixel.clone()),
            (
                "skinned".to_owned(),
                if self.skinned { "yes" } else { "no" }.to_owned(),
            ),
            (
                "unset-textures".to_owned(),
                match self.unset {
                    UnsetTexture::White => "white",
                    UnsetTexture::Leave => "leave",
                }
                .to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[
            GtaShader::NAME,
            GtaMaterialEffect::NAME,
            GtaEffectParameters::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[ShaderBinding::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a texture parameter with no texture reads white (unset-textures=white)"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, GtaShader, Entity)> = world
            .iter::<GtaMaterialEffect>()
            .filter_map(|(e, l)| Some((e, world.get::<GtaShader>(e)?.clone(), l.effect)))
            .collect();
        let mut counts = Counts::default();
        // per effect: the programs chosen, or why not
        let mut chosen: BTreeMap<Entity, Result<(Entity, Entity), String>> = BTreeMap::new();
        let mut bound: Vec<(Entity, ShaderBinding)> = Vec::new();
        for (material, shader, effect_entity) in materials {
            let picked = chosen
                .entry(effect_entity)
                .or_insert_with(|| {
                    let effect = world
                        .get::<GtaEffect>(effect_entity)
                        .ok_or_else(|| "a material's effect entity without its effect".to_owned())?;
                    let (p, v) = select_programs(&effect.programs, &self.pixel, self.skinned)
                        .map_err(|why| format!("{} ({})", why, effect.name))?;
                    Ok((
                        p.shader.ok_or("a pixel program without an entity")?,
                        v.shader.ok_or("a vertex program without an entity")?,
                    ))
                })
                .clone();
            let (pixel, vertex) = match picked {
                Ok(pair) => pair,
                Err(why) => {
                    counts.add(&format!("materials left unbound: {why}"));
                    continue;
                }
            };
            let renamed: BTreeMap<u32, u32> = world
                .get::<GtaEffectParameters>(effect_entity)
                .map(|p| p.0.iter().map(|r| (r.name, r.second_name)).collect())
                .unwrap_or_default();
            let inputs = Inputs {
                material: &shader,
                renamed,
                unset: self.unset,
            };
            let mut stages = Vec::new();
            let mut failed = None;
            for (stage, e) in [(Stage::Vertex, vertex), (Stage::Pixel, pixel)] {
                match self.stage_binding(world, &inputs, stage, e, true, &mut counts) {
                    Ok(s) => stages.push(s),
                    Err(why) => {
                        failed = Some(why);
                        break;
                    }
                }
            }
            if let Some(why) = failed {
                counts.add(&format!("materials left unbound: {why}"));
                continue;
            }
            counts.add("materials bound");
            bound.push((
                material,
                ShaderBinding {
                    stages,
                    state: RenderState::default(),
                },
            ));
        }
        for (e, b) in bound {
            world.insert(e, b);
        }
        for (what, n) in counts.0 {
            if what.contains("(unset-textures=white)") {
                log.approximate(&what, n);
            } else {
                log.count(&what, n);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "hand-worked fixtures"
)]
mod tests {
    use super::*;

    fn program(stage: GtaStage, name: &str) -> GtaEffectProgram {
        GtaEffectProgram {
            stage,
            name: name.to_owned(),
            shader: Some(World::new().spawn()),
        }
    }

    /// An effect with a plain unskinned pair, a plain skinned pair, an alpha-clipped unskinned pair, a tessellated
    /// vertex program of the plain hash, and a forward pass.
    fn effect() -> Vec<GtaEffectProgram> {
        use GtaStage::{Pixel as P, Vertex as V};
        vec![
            program(V, "VS_Transform_7903a05e_Wrapped"),
            program(P, "PS_Textured_Zero_7903a05e_Wrapped"),
            program(V, "VS_TransformD_0174add1_Wrapped"),
            program(V, "VS_northVertexInputBump_PNTri_0174add1_Wrapped"),
            program(P, "PS_DeferredTextured_0174add1_Wrapped"),
            program(V, "VS_TransformSkinD_f146f347_Wrapped"),
            program(P, "PS_DeferredTextured_f146f347_Wrapped"),
            program(V, "VS_TransformD_672703e2_Wrapped"),
            program(P, "PS_DeferredTextured_AlphaClip2_672703e2_Wrapped"),
        ]
    }

    #[test]
    fn program_names_split_into_family_and_hash() {
        assert_eq!(
            split_program_name("PS_DeferredTextured_AlphaClip2_672703e2_Wrapped"),
            Some(("PS_DeferredTextured_AlphaClip2", "672703e2"))
        );
        assert_eq!(split_program_name("PS_DeferredTextured_Wrapped"), None);
        assert_eq!(split_program_name("PS_X_0174add1"), None);
        assert_eq!(split_program_name("PS_X_0174add_Wrapped"), None);
    }

    /// The pixel family, skinned or not, picks the one pair that shares a hash; the tessellated vertex program of the
    /// same hash is not chosen; a family or a skin the effect lacks, and two fits, are named.
    #[test]
    fn a_pair_is_chosen_by_family_hash_and_skin() {
        let programs = effect();
        let (p, v) = select_programs(&programs, "PS_DeferredTextured", false).unwrap();
        assert_eq!(
            (p.name.as_str(), v.name.as_str()),
            (
                "PS_DeferredTextured_0174add1_Wrapped",
                "VS_TransformD_0174add1_Wrapped"
            )
        );
        let (p, v) = select_programs(&programs, "PS_DeferredTextured", true).unwrap();
        assert_eq!(
            (p.name.as_str(), v.name.as_str()),
            (
                "PS_DeferredTextured_f146f347_Wrapped",
                "VS_TransformSkinD_f146f347_Wrapped"
            )
        );
        let (p, v) = select_programs(&programs, "PS_DeferredTextured_AlphaClip2", false).unwrap();
        assert_eq!(
            (p.name.as_str(), v.name.as_str()),
            (
                "PS_DeferredTextured_AlphaClip2_672703e2_Wrapped",
                "VS_TransformD_672703e2_Wrapped"
            )
        );
        assert!(
            select_programs(&programs, "PS_DeferredTextured_AlphaClip2", true)
                .unwrap_err()
                .contains("skinned")
        );
        assert!(select_programs(&programs, "PS_DeferredVehicleTextured", false).is_err());
        // two permutations of the family that fit
        let mut two = effect();
        two.push(program(GtaStage::Vertex, "VS_TransformD_aaaaaaaa_Wrapped"));
        two.push(program(GtaStage::Pixel, "PS_DeferredTextured_aaaaaaaa_Wrapped"));
        assert!(
            select_programs(&two, "PS_DeferredTextured", false)
                .unwrap_err()
                .contains("2 PS_DeferredTextured permutations")
        );
    }

    #[test]
    fn a_constants_size_follows_its_type_byte() {
        assert_eq!(
            [9u32, 10, 11, 12, 14, 0, 5].map(|k| value_bytes(k).unwrap()),
            [4, 8, 12, 16, 64, 4, 4]
        );
        assert_eq!(value_bytes(12 | (6 << 16)), Some(96));
        assert_eq!(value_bytes(14 | (2 << 16)), Some(128));
        assert_eq!(value_bytes(3), None);
    }

    #[test]
    fn engine_matrices_have_roles_and_other_engine_constants_none() {
        assert_eq!(
            engine_role("gWorld"),
            Some(ConstantRole::Object(ObjectParameter::World))
        );
        assert_eq!(
            engine_role("gViewInverse"),
            Some(ConstantRole::View(ViewParameter::InverseView))
        );
        assert_eq!(engine_role("gDirectionalLight"), None);
    }

    #[test]
    fn options_are_required_and_checked() {
        let o = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<Options>()
        };
        assert!(ResolveGtaShaders::from_options(&o(&[])).is_err());
        assert!(
            ResolveGtaShaders::from_options(&o(&[
                ("pixel", "PS Deferred"),
                ("skinned", "no"),
                ("unset-textures", "white")
            ]))
            .is_err()
        );
        let p = ResolveGtaShaders::from_options(&o(&[
            ("pixel", "PS_DeferredTextured"),
            ("skinned", "yes"),
            ("unset-textures", "leave"),
        ]))
        .unwrap();
        assert_eq!(
            p,
            ResolveGtaShaders::new("PS_DeferredTextured", true, UnsetTexture::Leave)
        );
        assert_eq!(p.options()["skinned"], "yes");
    }
}
