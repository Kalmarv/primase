//! `resolve-gta-shaders{technique=NAME,unset-textures=white|leave}`: a drawable's material bound to the programs of its
//! effect's technique, as `core.shader_binding` on the material's entity. The programs are the `core.shader` entities
//! of the shader database ([`crate::read::shaders`]); what they read comes from the database's own tables
//! ([`crate::rage::sgd2_table`]) and the material's stored parameters.
//!
//! - **Programs.** An effect names its techniques ([`GtaEffectTechniques`], read from the effect's record by
//!   [`crate::rage::sgd2_effect`]): `deferred_draw` is its G-buffer pass for an unskinned mesh, `deferred_drawskinned`
//!   for a skinned one, `deferredalphaclip_draw` the alpha-clipped variant, `lightweight0_draw` a forward pass,
//!   `wdcascade_draw` the cascaded shadow pass, `cube_draw` the cube-map pass, and each lists the programs it runs
//!   (the vertex and the pixel program, or the vertex program alone of a depth-only pass). Which technique a draw uses
//!   (its pass, its vertex format, its alpha mode) is the draw's choice, not the material's, so `technique` is a
//!   required option. An effect without that technique, with several passes in it, with a tessellated pass (the hull
//!   and domain stages are not bound) or with no technique table (the post-processing and lighting effects) leaves its
//!   materials unbound, by name.
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
//!   `COLOR1`); the pixel stage's are the vertex stage's. The outputs of a `PS_DeferredTextured*` program that writes
//!   the four G-buffer targets have the roles [`gbuffer`] states: target 0's xyz the albedo, target 1's xyz the world
//!   normal (where the program stores it as `n * 0.5 + 0.5`), target 2's x the specular intensity and y the specular
//!   exponent (where the program stores each as a square root, the exponent over 512), z the Fresnel term, target 3's
//!   z the emission (where the program writes one) and target 4 the motion vector; every other lane, and every
//!   output of another family or of a program that writes fewer targets, is `Unknown` by its index (the lanes'
//!   contents are in [`gbuffer`]'s table).
use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{
    ConstantBinding, ConstantRole, D3dClass, InputBinding, InputRole, NormalEncoding, NormalSpace,
    ObjectParameter, OutputBinding, OutputRole, RenderState, ResourceKind, SamplerState, ScalarEncoding,
    Shader, ShaderBinding, Stage, StageBinding, SystemValue, TextureBinding, TextureSource, ViewParameter,
};
use primase_core::{Component, Entity, Semantic, Texture, TextureFile, World};

use super::gbuffer::{self, GBufferWrites};
use crate::components::{
    GtaEffect, GtaEffectParameters, GtaEffectTechniques, GtaMaterialEffect, GtaParam, GtaPass, GtaShader,
    GtaShaderTable,
};
use crate::rage::sgd2_table::{ProgramTable, TableConstant, TableResource};

/// The pixel program family whose targets the pass names roles for (`PS_DeferredTextured` and its alpha-clipped and
/// sub-sample variants: [`gbuffer`] has the layout and how it was checked).
const GBUFFER_FAMILY: &str = "PS_DeferredTextured";

/// The programs an effect's materials are bound to.
#[derive(Clone)]
struct Chosen {
    /// None for a depth-only pass.
    pixel: Option<Entity>,
    vertex: Entity,
    /// The pixel program's G-buffer stores, where its family has a layout.
    writes: Option<Result<GBufferWrites, String>>,
}

/// What a texture parameter with no texture becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsetTexture {
    White,
    Leave,
}

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveGtaShaders {
    technique: String,
    unset: UnsetTexture,
}

impl ResolveGtaShaders {
    pub fn new(technique: &str, unset: UnsetTexture) -> Self {
        ResolveGtaShaders {
            technique: technique.to_owned(),
            unset,
        }
    }

    /// From its options (both required), through [`ResolveGtaShaders::new`].
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let (mut technique, mut unset) = (None, None);
        for (k, v) in o {
            match k.as_str() {
                "technique" => {
                    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                        return Err(format!("technique={v:?} is not a technique name"));
                    }
                    technique = Some(v.clone());
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
            technique: technique.ok_or(
                "technique is required: the technique drawn with (deferred_draw, deferred_drawskinned, lightweight0_draw...)",
            )?,
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

/// The pass of the technique `name` of an effect: its one pass, if it has one, with a vertex program and no
/// tessellation.
pub fn select_pass<'a>(techniques: &'a GtaEffectTechniques, name: &str) -> Result<&'a GtaPass, String> {
    let technique = techniques
        .techniques
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| format!("the effect has no technique {name}"))?;
    let [pass] = technique.passes.as_slice() else {
        return Err(format!("technique {name} has {} passes", technique.passes.len()));
    };
    if pass.hull.is_some() || pass.domain.is_some() {
        return Err(format!(
            "technique {name} is tessellated (the hull and domain stages are not bound)"
        ));
    }
    if pass.vertex.is_none() {
        return Err(format!("technique {name} has no vertex program"));
    }
    Ok(pass)
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
        writes: Option<&Result<GBufferWrites, String>>,
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
        let outputs = if stage == Stage::Pixel {
            output_bindings(interface, writes, counts)
        } else {
            Vec::new()
        };
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

/// The outputs of a pixel program with their roles: the G-buffer's where the program is of the family and writes its
/// four targets, `Unknown` by index otherwise.
fn output_bindings(
    interface: &primase_core::shader::Interface,
    writes: Option<&Result<GBufferWrites, String>>,
    counts: &mut Counts,
) -> Vec<OutputBinding> {
    // a DXIL program's colour outputs are system values (`SV_Target<n>`)
    let targets: Vec<_> = interface
        .outputs
        .iter()
        .filter(|v| matches!(v.system, None | Some(SystemValue::Target(_))))
        .collect();
    let four = targets.len() >= 4 && targets.iter().take(4).all(|v| v.value.width == 4);
    let layout = match writes {
        None => None,
        Some(Err(why)) => {
            counts.add(&format!(
                "pixel stages whose output stores could not be read (outputs Unknown by index): {why}"
            ));
            None
        }
        Some(Ok(w)) if four => {
            counts.add("pixel stages with the G-buffer layout of the family");
            Some(w)
        }
        Some(Ok(_)) => {
            counts.add("pixel stages of the family writing fewer than four targets of four lanes (outputs Unknown by index)");
            None
        }
    };
    let mut outputs = Vec::new();
    for v in targets {
        let full: u8 = ((1u16 << v.value.width) - 1) as u8;
        let mut lanes = |lanes: u8, role: OutputRole| {
            let lanes = lanes & full;
            if lanes != 0 {
                outputs.push(OutputBinding {
                    label: v.label.clone(),
                    lanes,
                    role,
                });
            }
        };
        match (layout, v.label.index) {
            (Some(_), 0) => {
                lanes(0b0111, OutputRole::Albedo);
                lanes(0b1000, OutputRole::Unknown(0));
            }
            (Some(w), 1) if w.normal_biased => {
                lanes(
                    0b0111,
                    OutputRole::Normal {
                        encoding: NormalEncoding::XyzUnorm,
                        space: NormalSpace::World,
                    },
                );
                lanes(0b1000, OutputRole::Unknown(1));
            }
            (Some(w), 2) => {
                lanes(
                    0b0001,
                    if w.specular_intensity_encoded {
                        OutputRole::SpecularIntensity {
                            encoding: ScalarEncoding::square_root_of(1.0),
                        }
                    } else {
                        OutputRole::Unknown(2)
                    },
                );
                lanes(
                    0b0010,
                    if w.specular_exponent_encoded {
                        OutputRole::SpecularExponent {
                            encoding: ScalarEncoding::square_root_of(512.0),
                        }
                    } else {
                        OutputRole::Unknown(2)
                    },
                );
                lanes(
                    0b0100,
                    if w.fresnel_written {
                        OutputRole::Fresnel
                    } else {
                        OutputRole::Unknown(2)
                    },
                );
                lanes(0b1000, OutputRole::Unknown(2));
            }
            (Some(w), 4) if w.motion_written => lanes(0b0011, OutputRole::MotionVector),
            (Some(w), 3) if w.emission_written => {
                lanes(0b0100, OutputRole::Emission);
                lanes(0b1011, OutputRole::Unknown(3));
            }
            (_, n) => lanes(0b1111, OutputRole::Unknown(n)),
        }
    }
    if let Some(w) = layout {
        if w.normal_zero {
            counts.add("G-buffer pixel stages that write no normal (target 1 xyz the constant 0, Unknown)");
        } else if w.normal_floored {
            counts.add("G-buffer pixel stages whose target 1 is floor(128 n + 128) / 256 with the fractional bits in w (Unknown)");
        } else if !w.normal_biased {
            counts.add("G-buffer pixel stages whose target 1 is stored another way (Unknown)");
        }
        if !w.specular_intensity_encoded {
            counts.add("G-buffer pixel stages whose target 2 x is not stored as a square root (Unknown)");
        }
        if !w.specular_exponent_encoded {
            counts.add("G-buffer pixel stages whose target 2 y is not stored as a square root (Unknown)");
        }
        if w.emission_written {
            counts.add("G-buffer pixel stages that write an emission");
        }
    }
    outputs
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
            ("technique".to_owned(), self.technique.clone()),
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
            GtaEffectTechniques::NAME,
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
        let mut chosen: BTreeMap<Entity, Result<Chosen, String>> = BTreeMap::new();
        let mut bound: Vec<(Entity, ShaderBinding)> = Vec::new();
        for (material, shader, effect_entity) in materials {
            let picked = chosen
                .entry(effect_entity)
                .or_insert_with(|| {
                    let effect = world
                        .get::<GtaEffect>(effect_entity)
                        .ok_or_else(|| "a material's effect entity without its effect".to_owned())?;
                    let techniques = world
                        .get::<GtaEffectTechniques>(effect_entity)
                        .ok_or_else(|| format!("the effect has no technique table ({})", effect.name))?;
                    let pass = select_pass(techniques, &self.technique)
                        .map_err(|why| format!("{why} ({})", effect.name))?;
                    let vertex =
                        pass.vertex.as_ref().and_then(|p| p.shader).ok_or_else(|| {
                            format!("the vertex program has no interface ({})", effect.name)
                        })?;
                    let pixel = match &pass.pixel {
                        None => None,
                        Some(p) => Some((
                            p.shader.ok_or_else(|| {
                                format!("the pixel program {} has no interface ({})", p.name, effect.name)
                            })?,
                            split_program_name(&p.name)
                                .is_some_and(|(stem, _)| stem.starts_with(GBUFFER_FAMILY)),
                        )),
                    };
                    // the G-buffer's roles are read from the program's own stores
                    let writes = pixel.and_then(|(entity, gbuffer_family)| {
                        gbuffer_family.then(|| {
                            world
                                .get::<Shader>(entity)
                                .ok_or_else(|| "a pixel program without its shader".to_owned())
                                .and_then(|s| gbuffer::writes(&s.bytecode))
                        })
                    });
                    Ok(Chosen {
                        pixel: pixel.map(|(entity, _)| entity),
                        vertex,
                        writes,
                    })
                })
                .clone();
            let Chosen {
                pixel,
                vertex,
                writes,
            } = match picked {
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
            for (stage, e) in [(Stage::Vertex, Some(vertex)), (Stage::Pixel, pixel)] {
                let Some(e) = e else { continue };
                match self.stage_binding(world, &inputs, stage, e, true, writes.as_ref(), &mut counts) {
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
    use crate::components::{GtaEffectProgram, GtaStage, GtaTechnique};

    fn program(stage: GtaStage, name: &str) -> Option<GtaEffectProgram> {
        Some(GtaEffectProgram {
            stage,
            name: name.to_owned(),
            shader: Some(World::new().spawn()),
        })
    }

    fn pass(vertex: Option<GtaEffectProgram>, pixel: Option<GtaEffectProgram>, tessellated: bool) -> GtaPass {
        GtaPass {
            vertex,
            pixel,
            hull: tessellated
                .then(|| program(GtaStage::Hull, "HS_x_0174add1_Wrapped"))
                .flatten(),
            domain: tessellated
                .then(|| program(GtaStage::Domain, "DS_x_0174add1_Wrapped"))
                .flatten(),
            record: vec![0; 24],
        }
    }

    fn technique(name: &str, passes: Vec<GtaPass>) -> GtaTechnique {
        GtaTechnique {
            name: name.to_owned(),
            passes,
        }
    }

    /// An effect with a G-buffer pass, its tessellated variant, a technique of two passes, a depth-only pass and a
    /// forward pass.
    fn techniques() -> GtaEffectTechniques {
        use GtaStage::{Pixel as P, Vertex as V};
        let gbuffer = || {
            pass(
                program(V, "VS_TransformD_0174add1_Wrapped"),
                program(P, "PS_DeferredTextured_0174add1_Wrapped"),
                false,
            )
        };
        GtaEffectTechniques {
            techniques: vec![
                technique("deferred_draw", vec![gbuffer()]),
                technique(
                    "deferred_drawtessellated",
                    vec![pass(
                        program(V, "VS_northVertexInputBump_PNTri_0174add1_Wrapped"),
                        program(P, "PS_DeferredTextured_0174add1_Wrapped"),
                        true,
                    )],
                ),
                technique("furry", vec![gbuffer(), gbuffer()]),
                technique(
                    "wdcascade_draw",
                    vec![pass(
                        program(V, "VS_CascadeShadows_draw_273b268a_Wrapped"),
                        None,
                        false,
                    )],
                ),
                technique(
                    "lightweight0_draw",
                    vec![pass(
                        program(V, "VS_Transform_7903a05e_Wrapped"),
                        program(P, "PS_Textured_Zero_7903a05e_Wrapped"),
                        false,
                    )],
                ),
                technique(
                    "broken",
                    vec![pass(None, program(P, "PS_x_7903a05e_Wrapped"), false)],
                ),
            ],
            depth_stencil: Vec::new(),
            blend: Vec::new(),
            samplers: Vec::new(),
        }
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

    /// A technique is found by its exact name and gives its one pass: the vertex program and the pixel program (none in
    /// a depth-only pass); a technique the effect lacks, one of several passes, a tessellated one and one with no
    /// vertex program are refused by name.
    #[test]
    fn a_pass_is_chosen_by_the_techniques_name() {
        let t = techniques();
        let names = |name: &str| {
            select_pass(&t, name).map(|p| {
                (
                    p.vertex.as_ref().map(|v| v.name.clone()),
                    p.pixel.as_ref().map(|v| v.name.clone()),
                )
            })
        };
        assert_eq!(
            names("deferred_draw").unwrap(),
            (
                Some("VS_TransformD_0174add1_Wrapped".to_owned()),
                Some("PS_DeferredTextured_0174add1_Wrapped".to_owned())
            )
        );
        assert_eq!(
            names("wdcascade_draw").unwrap(),
            (Some("VS_CascadeShadows_draw_273b268a_Wrapped".to_owned()), None)
        );
        assert_eq!(
            names("lightweight0_draw").unwrap().1,
            Some("PS_Textured_Zero_7903a05e_Wrapped".to_owned())
        );
        assert!(
            names("deferred_drawskinned")
                .unwrap_err()
                .contains("no technique deferred_drawskinned")
        );
        assert!(names("furry").unwrap_err().contains("has 2 passes"));
        assert!(
            names("deferred_drawtessellated")
                .unwrap_err()
                .contains("tessellated")
        );
        assert!(names("broken").unwrap_err().contains("no vertex program"));
        // the name is exact
        assert!(names("Deferred_draw").is_err());
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
        assert!(ResolveGtaShaders::from_options(&o(&[("unset-textures", "white")])).is_err());
        assert!(
            ResolveGtaShaders::from_options(&o(&[
                ("technique", "deferred draw"),
                ("unset-textures", "white")
            ]))
            .is_err()
        );
        // the options of the previous version name programs, not a technique
        assert!(
            ResolveGtaShaders::from_options(&o(&[
                ("pixel", "PS_DeferredTextured"),
                ("vertex", "VS_TransformD"),
                ("unset-textures", "white")
            ]))
            .is_err()
        );
        let p = ResolveGtaShaders::from_options(&o(&[
            ("technique", "deferred_drawskinned"),
            ("unset-textures", "leave"),
        ]))
        .unwrap();
        assert_eq!(
            p,
            ResolveGtaShaders::new("deferred_drawskinned", UnsetTexture::Leave)
        );
        assert_eq!(p.options()["technique"], "deferred_drawskinned");
    }
}
