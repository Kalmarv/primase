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
//!   and domain stages are not bound), a compute pass or with no technique table (the post-processing and lighting
//!   effects) leaves its materials unbound, by name.
//! - **Render state.** The pass names a record of each of the effect's tables ([`GtaPass::rasterizer`], `depth_stencil`
//!   and `blend`): the cull mode, fill mode and winding; the depth test and write and the stencil test; and for every
//!   render target its blend (colour and alpha equations, in the vocabulary's factors: a blend of a factor it has none
//!   for, such as source alpha saturate, is left `None` and counted) and its write mask, `blend` being target 0's. An
//!   effect without a table states none of it. Not carried: the polygon offset (the rasterizer record's float at +8,
//!   non-zero on the two foam effects, which of the depth bias terms it is is not established); the alpha test is not a
//!   render state in GTA (the cutoff is a shader constant).
//! - **Constants.** Each constant buffer a program declares is the table record at its register and space; each
//!   constant of the record is bound at its byte offset (a constant past the end of the buffer the program declares is
//!   one it does not read, and is left out, counted). A constant the effect renames (its parameter records:
//!   [`GtaEffectParameters`]) is a material parameter, bound to the words the material stores under the new name
//!   (`Material`), or `Unknown` where it stores none; the engine's world, view and projection matrices have the roles
//!   [`engine_role`] names; every other engine constant is `Unknown` with its name.
//! - **Textures.** A texture the program declares is the table record at its register and space that has a texture tag
//!   (the sampler heap's and the constant buffers' records share register and space numbers with it). The material
//!   binds it (`Texture`) through the parameter it stores under the record's second name where the effect renames it,
//!   else under the record's own name (`DiffuseTex`); unset takes `unset-textures` (`white`: white, counted as an
//!   approximation; `leave`: `Missing`), and a texture of the engine's (an unrenamed one the material stores nothing
//!   for) is `Missing`.
//! - **Samplers.** Most programs sample through one bindless heap, indexed at run time by a word of the constant buffer
//!   `g_rage_dynamicsamplerindices` ([`sampler_use`] traces each sample to its word, which is one of the program's
//!   `gDynamicSamplerIndex_<name>` constants). A texture is read through the sampler its sample's word names: a static
//!   sampler (`..._sdiTrilinearWrap`) has the state its name states ([`sampler_state::named`]; the effect's sampler
//!   records hold it for most, counted), and the state is the texture's `sampler`. A word the material chooses
//!   (`..._DiffuseSampler`) leaves the state unstated: the material stores a sampler value for it, but the value is
//!   not an index into the effect's sampler records (primitives whose value points at a clamping record tile their
//!   texture across several UV units; `tests/sampler_params.rs` counts them), and the heap's contents are the
//!   engine's. A program that declares a sampler of its own samples through it: the texture binding names that
//!   resource and the state is unstated (the root signature's, not read). A texture sampled through several samplers
//!   of different states is left unstated.
//! - **Inputs and outputs.** The vertex stage's inputs are the mesh streams their semantics name (terrain weights are
//!   `COLOR1`); the pixel stage's are the vertex stage's. The outputs of a `PS_DeferredTextured*` program that writes
//!   the four G-buffer targets have the roles [`gbuffer`] states: target 0's xyz the albedo, target 1's xyz the world
//!   normal (where the program stores it as `n * 0.5 + 0.5`), target 2's x the specular intensity and y the specular
//!   exponent (where the program stores each as a square root, the exponent over 512), z the Fresnel term, target 3's
//!   z the emission (where the program writes one) and target 4 the motion vector; every other lane, and every
//!   output of another family or of a program that writes fewer targets, is `Unknown` by its index (the lanes'
//!   contents are in [`gbuffer`]'s table).
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{
    Blend, BlendEquation, BlendFactor, BlendOp, Compare, ConstantBinding, ConstantRole, Cull, D3dClass,
    DepthState, Fill, FrameParameter, InputBinding, InputRole, Interface, NormalEncoding, NormalSpace,
    ObjectMatrix, ObjectParameter, OutputBinding, OutputRole, RenderState, ResourceId, ResourceKind,
    SamplerState, ScalarEncoding, Shader, ShaderBinding, Stage, StageBinding, Stencil, StencilFace,
    StencilOp, SystemValue, TargetState, TextureBinding, TextureSource, ViewParameter,
};
use primase_core::{Component, Entity, Semantic, Texture, TextureFile, World};

use super::gbuffer::{self, GBufferWrites};
use super::sampler_state;
use super::sampler_use::{self, SamplerUses, Through};
use crate::components::{
    GtaEffect, GtaEffectParameters, GtaEffectTechniques, GtaMaterialEffect, GtaParam, GtaPass, GtaShader,
    GtaShaderTable,
};
use crate::rage::sgd2_effect::SamplerDesc;
use crate::rage::sgd2_table::{ProgramTable, TableConstant, TableResource};

/// The pixel program family whose targets the pass names roles for (`PS_DeferredTextured` and its alpha-clipped and
/// sub-sample variants: [`gbuffer`] has the layout and how it was checked).
const GBUFFER_FAMILY: &str = "PS_DeferredTextured";

/// The D3D12 comparison function as the vocabulary's.
pub(super) fn compare(f: u8) -> Option<Compare> {
    Some(match f {
        1 => Compare::Never,
        2 => Compare::Less,
        3 => Compare::Equal,
        4 => Compare::LessEqual,
        5 => Compare::Greater,
        6 => Compare::NotEqual,
        7 => Compare::GreaterEqual,
        8 => Compare::Always,
        _ => return None,
    })
}

/// The D3D12 stencil operation as the vocabulary's.
fn stencil_op(o: u8) -> Option<StencilOp> {
    Some(match o {
        1 => StencilOp::Keep,
        2 => StencilOp::Zero,
        3 => StencilOp::Replace,
        4 => StencilOp::IncrementClamp,
        5 => StencilOp::DecrementClamp,
        6 => StencilOp::Invert,
        7 => StencilOp::IncrementWrap,
        8 => StencilOp::DecrementWrap,
        _ => return None,
    })
}

/// The D3D12 blend factor as the vocabulary's; `None` for the ones it has none for (source alpha saturate, the blend
/// factor constants and the dual-source factors).
fn blend_factor(f: u8) -> Option<BlendFactor> {
    Some(match f {
        1 => BlendFactor::Zero,
        2 => BlendFactor::One,
        3 => BlendFactor::SrcColour,
        4 => BlendFactor::OneMinusSrcColour,
        5 => BlendFactor::SrcAlpha,
        6 => BlendFactor::OneMinusSrcAlpha,
        7 => BlendFactor::DstAlpha,
        8 => BlendFactor::OneMinusDstAlpha,
        9 => BlendFactor::DstColour,
        10 => BlendFactor::OneMinusDstColour,
        _ => return None,
    })
}

fn blend_op(o: u8) -> Option<BlendOp> {
    Some(match o {
        1 => BlendOp::Add,
        2 => BlendOp::Subtract,
        3 => BlendOp::ReverseSubtract,
        4 => BlendOp::Min,
        5 => BlendOp::Max,
        _ => return None,
    })
}

/// The render state a pass names, from its effect's tables, and what could not be carried (once each, in `notes`).
fn render_state(t: &GtaEffectTechniques, pass: &GtaPass, notes: &mut Vec<&'static str>) -> RenderState {
    let mut state = RenderState::default();
    if let Some(r) = t.rasterizer.get(usize::from(pass.rasterizer)) {
        state.cull = match r.cull {
            1 => Some(Cull::None),
            2 => Some(Cull::Front),
            3 => Some(Cull::Back),
            _ => None,
        };
        state.fill = match r.fill {
            2 => Some(Fill::Wireframe),
            3 => Some(Fill::Solid),
            _ => None,
        };
        state.front_counter_clockwise = Some(r.front_counter_clockwise == 1);
        if r.offset != 0.0 {
            notes.push("a polygon offset (the rasterizer record's float at +8) is not read (left None)");
        }
    }
    if let Some(d) = t.depth_stencil.get(usize::from(pass.depth_stencil)) {
        state.depth = if d.depth_enable == 0 {
            Some(DepthState {
                test: Compare::Always,
                write: false,
            })
        } else {
            compare(d.depth_func).map(|test| DepthState {
                test,
                write: d.depth_write_mask == 1,
            })
        };
        if d.stencil_enable == 1 {
            let face = |f: &crate::rage::sgd2_effect::StencilFace| {
                Some(StencilFace {
                    fail: stencil_op(f.fail_op)?,
                    depth_fail: stencil_op(f.depth_fail_op)?,
                    pass: stencil_op(f.pass_op)?,
                    compare: compare(f.func)?,
                })
            };
            state.stencil = face(&d.front).zip(face(&d.back)).map(|(front, back)| Stencil {
                reference: None,
                read_mask: d.stencil_read_mask,
                write_mask: d.stencil_write_mask,
                front,
                back,
            });
        }
    }
    if let Some(b) = t.blend.get(usize::from(pass.blend)) {
        for (i, target) in b.targets.iter().enumerate() {
            let equation = |s, d, o| {
                Some(BlendEquation {
                    source: blend_factor(s)?,
                    destination: blend_factor(d)?,
                    op: blend_op(o)?,
                })
            };
            let blend = if target.enable == 0 {
                None
            } else {
                let both = equation(target.src, target.dest, target.op).zip(equation(
                    target.src_alpha,
                    target.dest_alpha,
                    target.op_alpha,
                ));
                if both.is_none() {
                    notes.push("a blend factor or operation outside the vocabulary (left None)");
                }
                both.map(|(colour, alpha)| Blend { colour, alpha })
            };
            if let Some(slot) = state.targets.get_mut(i) {
                *slot = Some(TargetState {
                    blend,
                    write_mask: target.write_mask & 0xf,
                });
            }
        }
        state.blend = state.targets[0].and_then(|t| t.blend);
    }
    notes.sort_unstable();
    notes.dedup();
    state
}

/// The programs an effect's materials are bound to.
#[derive(Clone)]
struct Chosen {
    /// The pass's render state and what it could not carry.
    state: RenderState,
    notes: Vec<&'static str>,
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
    if pass.compute.is_some() {
        return Err(format!("technique {name} is a compute pass"));
    }
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
        "gWorldView" => ConstantRole::Object(ObjectParameter::WorldView),
        "gWorldViewProj" => ConstantRole::Object(ObjectParameter::WorldViewProjection),
        "gPrevWorld" => ConstantRole::Object(ObjectParameter::PreviousFrame(ObjectMatrix::World)),
        "gPrevWorldView" => ConstantRole::Object(ObjectParameter::PreviousFrame(ObjectMatrix::WorldView)),
        "gPrevWorldViewProj" => {
            ConstantRole::Object(ObjectParameter::PreviousFrame(ObjectMatrix::WorldViewProjection))
        }
        "gRelativeWorld" => ConstantRole::Object(ObjectParameter::CameraRelative(ObjectMatrix::World)),
        "gRelativeWorldView" => {
            ConstantRole::Object(ObjectParameter::CameraRelative(ObjectMatrix::WorldView))
        }
        "gRelativeWorldViewProj" => {
            ConstantRole::Object(ObjectParameter::CameraRelative(ObjectMatrix::WorldViewProjection))
        }
        "gRelativePrevWorld" => {
            ConstantRole::Object(ObjectParameter::CameraRelativePreviousFrame(ObjectMatrix::World))
        }
        "gRelativePrevWorldView" => ConstantRole::Object(ObjectParameter::CameraRelativePreviousFrame(
            ObjectMatrix::WorldView,
        )),
        "gRelativePrevWorldViewProj" => ConstantRole::Object(ObjectParameter::CameraRelativePreviousFrame(
            ObjectMatrix::WorldViewProjection,
        )),
        "gViewInverse" => ConstantRole::View(ViewParameter::InverseView),
        "gPrevViewProj" => ConstantRole::View(ViewParameter::PreviousViewProjection),
        "globalScreenSize" => ConstantRole::View(ViewParameter::ViewportSize),
        "gDirectionalLight" => ConstantRole::Frame(FrameParameter::LightDirection),
        "gDirectionalColour" => ConstantRole::Frame(FrameParameter::LightColour),
        "gDirectionalAmbientColour" => ConstantRole::Frame(FrameParameter::DirectionalAmbientColour),
        "globalAnimUV0" => ConstantRole::Object(ObjectParameter::TextureTransformRow(0)),
        "globalAnimUV1" => ConstantRole::Object(ObjectParameter::TextureTransformRow(1)),
        "tintPaletteSelector" => ConstantRole::Object(ObjectParameter::PaletteCoordinate),
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
    /// The effect's sampler records.
    samplers: &'a [SamplerDesc],
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

/// The record tags of the resources the programs bind as textures: 258 a 2D texture, 322 a 2D array, 514 and 770 the
/// other dimensions. A texture shares its register and space with other records (the sampler heap's, tag 1059, a
/// constant buffer's, 1072: the table numbers each kind of resource from 0 in each space), and the tag is what tells
/// them apart (`tests/texture_records.rs` holds every texture bind point of the install to exactly one of these).
const TEXTURE_TAGS: [u16; 4] = [258, 322, 514, 770];

/// The constant buffer record of `table` for a bind point, among those of its register and space: the size the program
/// declares rounded up to 16, or the root constants' record, which states none (size 0).
fn buffer_record(table: &ProgramTable, register: u32, space: u32, size: u32) -> Option<usize> {
    let at = |size: u32| {
        table.resources.iter().position(|r| {
            u32::from(r.register) == register
                && u32::from(r.space) == space
                && r.size == size
                && r.constants > 0
        })
    };
    at(size.div_ceil(16).saturating_mul(16)).or_else(|| at(0))
}

/// The texture record of `table` for a bind point: the one of the texture tags among those of its register and space.
fn texture_record(table: &ProgramTable, register: u32, space: u32) -> Option<usize> {
    table.resources.iter().position(|r| {
        u32::from(r.register) == register && u32::from(r.space) == space && TEXTURE_TAGS.contains(&r.tag)
    })
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
        sample_uses: &mut BTreeMap<Entity, Result<SamplerUses, String>>,
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
        let uses = sample_uses
            .entry(shader)
            .or_insert_with(|| sampler_use::uses(&program.bytecode));
        for r in &interface.resources {
            let Some(bind) = r.provenance.d3d else { continue };
            match (&r.kind, bind.class) {
                (ResourceKind::ConstantBuffer { layout }, D3dClass::ConstantBuffer) => {
                    let Some(i) = buffer_record(table, bind.register, bind.space, layout.size) else {
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
                    let sampling = sampling(
                        interface,
                        table,
                        inputs,
                        uses,
                        (bind.space, bind.register),
                        counts,
                    );
                    let source = match texture_record(table, bind.register, bind.space)
                        .and_then(|i| table.resources.get(i))
                    {
                        Some(t) => texture_source(world, inputs, t, sampling.state, counts),
                        None => {
                            counts.add("texture slots with no table record, left Missing");
                            TextureSource::Missing
                        }
                    };
                    textures.push(TextureBinding {
                        resource: r.id,
                        sampler: sampling.resource,
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

/// Why an engine constant the programs read has no role, where the programs show something about it (what the vertex
/// programs do with its lanes: `tests/engine_use.rs`).
pub fn engine_reason(name: &str) -> Option<&'static str> {
    Some(match name {
        "globalScalars2" => {
            "engine frame scalars: lane y divides the clip depth of a see-through fade (VS_Transform_SeeThrough) and lane z is subtracted from a squared-distance fade of the vertex colour (VS_TransformD); no game-free meaning established"
        }
        "globalScalars" => {
            "engine frame scalars: lanes y and z are passed on to the pixel stage (VS_decalFx); no game-free meaning established"
        }
        "gClothVertexStride" => {
            "lane z: where non-zero, the vertex position is read from a structured buffer at (z * vertex index) >> 2 instead of the mesh stream (the cloth programs); that buffer is not in the vocabulary"
        }
        "gUIHDRParams" => {
            "lanes x and y read by the same twelve cloth-capable vertex programs as gClothVertexStride; no game-free meaning established"
        }
        n if n.starts_with("gDynamicSamplerIndex_") => {
            "a word of the sampler-heap index buffer (see sampler_use): the engine writes the heap index of the sampler it names; the state of a static sampler is stated on the texture binding"
        }
        _ => return None,
    })
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
    // a constant the program names after one of the engine's matrices is the engine's, whether or not the effect's
    // parameter records list it under another name
    if let Some(role) = name.and_then(engine_role) {
        counts.add("engine constants with a role");
        return role;
    }
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
    // a constant the effect does not rename is the material's where the material stores a value under the program's own
    // name (the hash is of the lowercase name, so `bumpiness` is the material's `Bumpiness`)
    if let Some(GtaParam::Floats(v)) = inputs.param(c.name) {
        counts.add(&format!(
            "material parameters stored under the program's own name, bound to the material's stored words (constant buffer space {space})"
        ));
        let mut words = bits(v);
        words.truncate(usize::try_from(bytes / 4).unwrap_or(0));
        return ConstantRole::Material(words);
    }
    match name.and_then(engine_role) {
        Some(role) => {
            counts.add("engine constants with a role");
            role
        }
        None => {
            counts.add("engine constants without a role, Unknown");
            ConstantRole::Unknown {
                why: match name.and_then(engine_reason) {
                    Some(reason) => format!("engine constant {}: {reason}", name.unwrap_or_default()),
                    None => format!(
                        "engine constant {}: no role resolved",
                        name.unwrap_or("(unnamed)")
                    ),
                },
            }
        }
    }
}

/// What a texture is read through: the sampler resource the program declares for it, if it samples through one, and the
/// state where the data states one.
struct Sampling {
    resource: Option<ResourceId>,
    state: SamplerState,
}

/// The state a word of the sampler-index buffer stands for: a static sampler's, from its constant's name. `None` for the
/// material's samplers and for a word this does not name.
fn heap_state(
    table: &ProgramTable,
    inputs: &Inputs<'_>,
    buffer: (u32, u32),
    offset: u32,
    counts: &mut Counts,
) -> Option<SamplerState> {
    let name = table
        .resources
        .iter()
        .enumerate()
        .find(|(_, r)| u32::from(r.space) == buffer.0 && u32::from(r.register) == buffer.1 && r.constants > 0)
        .and_then(|(i, _)| {
            table
                .constants_of(i)
                .iter()
                .find(|c| u32::from(c.offset) == offset)
        })
        .and_then(|c| table.name_of(c.name));
    let Some(name) = name else {
        counts.add("sampler words with no constant in the program's table, sampler left unstated");
        return None;
    };
    if let Some(state) = sampler_state::named(name) {
        counts.add(if sampler_state::held_by(&state, inputs.samplers) {
            "static samplers whose state is read from the constant's name, which the effect's sampler records hold (sampler state read from the constant's name)"
        } else {
            "static samplers whose state is read from the constant's name, which the effect's sampler records do not hold (sampler state read from the constant's name)"
        });
        return Some(state);
    }
    match name.strip_prefix("gDynamicSamplerIndex_") {
        Some(material) => {
            let stored = inputs.param(primase_hash::joaat(material.as_bytes()));
            counts.add(if matches!(stored, Some(GtaParam::Sampler(_))) {
                "samplers the material chooses (it stores a sampler value, which names no state), left unstated"
            } else {
                "samplers the material chooses (it stores no sampler value), left unstated"
            });
        }
        None => counts.add("sampler words of a name this pass does not read, left unstated"),
    }
    None
}

/// The sampler a texture slot at a bind point is read through, from the samples of its program.
fn sampling(
    interface: &Interface,
    table: &ProgramTable,
    inputs: &Inputs<'_>,
    uses: &Result<SamplerUses, String>,
    texture: (u32, u32),
    counts: &mut Counts,
) -> Sampling {
    let none = Sampling {
        resource: None,
        state: SamplerState::default(),
    };
    let uses = match uses {
        Ok(u) => u,
        Err(why) => {
            counts.add(&format!(
                "textures of programs whose samples could not be traced ({why}), sampler left unstated"
            ));
            return none;
        }
    };
    let mut declared = BTreeSet::new();
    let mut states = Vec::new();
    let mut unstated = false;
    for u in uses.uses.iter().filter(|u| u.texture == texture) {
        match u.through {
            Through::Direct(at) => {
                let resource = interface.resources.iter().find(|r| {
                    matches!(r.kind, ResourceKind::Sampler { .. })
                        && r.provenance.d3d.is_some_and(|b| (b.space, b.register) == at)
                });
                match resource {
                    Some(r) => {
                        declared.insert(r.id);
                    }
                    None => unstated = true,
                }
            }
            Through::Heap { buffer, offset } => match heap_state(table, inputs, buffer, offset, counts) {
                Some(state) => {
                    if !states.contains(&state) {
                        states.push(state);
                    }
                }
                None => unstated = true,
            },
        }
    }
    match (declared.len(), states.as_slice(), unstated) {
        (0, [], false) => {
            counts.add("textures no sample reads, sampler left unstated");
            none
        }
        (0, [state], false) => Sampling {
            resource: None,
            state: *state,
        },
        (1, [], false) => {
            counts.add("textures read through a sampler the program declares (its state is the root signature's, not read)");
            Sampling {
                resource: declared.first().copied(),
                state: SamplerState::default(),
            }
        }
        _ => {
            if !unstated {
                counts
                    .add("textures read through several samplers of different states, sampler left unstated");
            }
            none
        }
    }
}

/// What a texture record's parameter samples.
fn texture_source(
    world: &World,
    inputs: &Inputs<'_>,
    t: &TableResource,
    sampler: SamplerState,
    counts: &mut Counts,
) -> TextureSource {
    // a texture the effect renames is stored under the new name; any other under the program's own
    let renamed = t.second_name != 0 && t.second_name != t.name;
    let key = if renamed { t.second_name } else { t.name };
    match inputs.param(key) {
        None if !renamed => {
            counts.add("texture slots that are the engine's (the material stores none under their name), left Missing");
            TextureSource::Missing
        }
        Some(GtaParam::Texture {
            texture: Some(texture),
            ..
        }) => {
            if world.has::<Texture>(*texture) || world.has::<TextureFile>(*texture) {
                counts.add("texture parameters bound to the material's texture");
                TextureSource::Texture {
                    texture: *texture,
                    sampler,
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
        "a texture parameter with no texture reads white (unset-textures=white); a static sampler has the state its constant's name states"
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
        // per program: the samplers its samples read through (lifting the program is the cost)
        let mut sample_uses: BTreeMap<Entity, Result<SamplerUses, String>> = BTreeMap::new();
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
                    let mut notes = Vec::new();
                    let state = render_state(techniques, pass, &mut notes);
                    Ok(Chosen {
                        state,
                        notes,
                        pixel: pixel.map(|(entity, _)| entity),
                        vertex,
                        writes,
                    })
                })
                .clone();
            let Chosen {
                state,
                notes,
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
                samplers: world
                    .get::<GtaEffectTechniques>(effect_entity)
                    .map_or(&[][..], |t| t.samplers.as_slice()),
                unset: self.unset,
            };
            let mut stages = Vec::new();
            let mut failed = None;
            for (stage, e) in [(Stage::Vertex, Some(vertex)), (Stage::Pixel, pixel)] {
                let Some(e) = e else { continue };
                match self.stage_binding(
                    world,
                    &inputs,
                    stage,
                    e,
                    true,
                    writes.as_ref(),
                    &mut sample_uses,
                    &mut counts,
                ) {
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
            for note in &notes {
                counts.add(&format!("render state: {note}"));
            }
            bound.push((material, ShaderBinding { stages, state }));
        }
        for (e, b) in bound {
            world.insert(e, b);
        }
        for (what, n) in counts.0 {
            if what.contains("(unset-textures=white)")
                || what.contains("(sampler state read from the constant's name)")
            {
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
            geometry: None,
            compute: None,
            rasterizer: 0,
            depth_stencil: 0,
            blend: 0,
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
            rasterizer: Vec::new(),
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

    fn target(enable: u8, src: u8, dest: u8) -> crate::rage::sgd2_effect::TargetBlend {
        crate::rage::sgd2_effect::TargetBlend {
            enable,
            src,
            dest,
            op: 1,
            src_alpha: 1,
            dest_alpha: 6,
            op_alpha: 1,
            write_mask: 7,
        }
    }

    /// A pass's render state is the records of its effect's tables it names, in the vocabulary's terms: the cull mode,
    /// fill and winding; the depth test and write and the stencil; every render target's blend and write mask. A blend
    /// factor the vocabulary lacks leaves that target's blend `None` (with a note), a disabled blend too (the mask kept),
    /// and an effect without tables states none.
    #[test]
    fn a_passes_state_is_the_records_it_names() {
        use crate::rage::sgd2_effect::{
            Blend as BlendRecord, DepthStencil, Rasterizer, StencilFace as FaceRecord,
        };
        let face = |func| FaceRecord {
            fail_op: 1,
            depth_fail_op: 2,
            pass_op: 3,
            func,
        };
        let depth = |enable, write, func, stencil| DepthStencil {
            depth_enable: enable,
            depth_write_mask: write,
            depth_func: func,
            stencil_enable: stencil,
            stencil_read_mask: 0xf0,
            stencil_write_mask: 0x0f,
            front: face(8),
            back: face(3),
            tail: [0; 10],
        };
        // render target 0 as given, the others opaque with the write mask 0
        let blend = |t: crate::rage::sgd2_effect::TargetBlend| {
            let mut targets = [crate::rage::sgd2_effect::TargetBlend {
                write_mask: 0,
                ..target(0, 2, 1)
            }; 8];
            targets[0] = t;
            BlendRecord {
                head: [0; 4],
                targets,
            }
        };
        let rasterizer = |cull, fill, ccw| Rasterizer {
            fill,
            cull,
            front_counter_clockwise: ccw,
            offset: 0.0,
            raw: [0; 16],
        };
        let mut t = techniques();
        t.rasterizer = vec![rasterizer(3, 3, 1), rasterizer(1, 2, 0)];
        t.depth_stencil = vec![depth(1, 0, 5, 0), depth(1, 1, 2, 1), depth(0, 0, 8, 0)];
        t.blend = vec![target(1, 5, 6), target(0, 2, 1), target(1, 11, 6)]
            .into_iter()
            .map(blend)
            .collect();
        let mut p = pass(None, None, false);
        let mut notes = Vec::new();
        // record 0 of each: back faces culled, solid, counter-clockwise fronts; depth tested GREATER without writing, no
        // stencil; target 0 source alpha over its inverse with the colour channels written, the others opaque, unwritten
        let state = render_state(&t, &p, &mut notes);
        assert_eq!(
            (state.cull, state.fill, state.front_counter_clockwise),
            (Some(Cull::Back), Some(Fill::Solid), Some(true))
        );
        assert_eq!(
            state.depth,
            Some(DepthState {
                test: Compare::Greater,
                write: false
            })
        );
        assert_eq!(state.stencil, None);
        let b = state.blend.unwrap();
        assert_eq!(
            (
                b.colour.source,
                b.colour.destination,
                b.alpha.source,
                b.alpha.destination
            ),
            (
                BlendFactor::SrcAlpha,
                BlendFactor::OneMinusSrcAlpha,
                BlendFactor::Zero,
                BlendFactor::OneMinusSrcAlpha
            )
        );
        assert_eq!(
            state.targets[0],
            Some(TargetState {
                blend: state.blend,
                write_mask: 7
            })
        );
        assert_eq!(
            state.targets[5],
            Some(TargetState {
                blend: None,
                write_mask: 0
            })
        );
        assert_eq!(state.targets.iter().flatten().count(), 8);
        assert!(notes.is_empty());
        assert_eq!(
            ShaderBinding {
                stages: Vec::new(),
                state
            }
            .state
            .verify(),
            Ok(())
        );
        // record 1: no culling, wireframe, clockwise fronts; depth LESS with writes and a stencil test; blending off
        p.rasterizer = 1;
        p.depth_stencil = 1;
        p.blend = 1;
        let state = render_state(&t, &p, &mut notes);
        assert_eq!(
            (state.cull, state.fill, state.front_counter_clockwise, state.blend),
            (Some(Cull::None), Some(Fill::Wireframe), Some(false), None)
        );
        assert_eq!(
            state.depth,
            Some(DepthState {
                test: Compare::Less,
                write: true
            })
        );
        let stencil = state.stencil.unwrap();
        assert_eq!((stencil.read_mask, stencil.write_mask), (0xf0, 0x0f));
        assert_eq!(
            stencil.front,
            StencilFace {
                fail: StencilOp::Keep,
                depth_fail: StencilOp::Zero,
                pass: StencilOp::Replace,
                compare: Compare::Always
            }
        );
        assert_eq!(stencil.back.compare, Compare::Equal);
        assert_eq!(
            state.targets[0],
            Some(TargetState {
                blend: None,
                write_mask: 7
            })
        );
        // record 2: depth off, a factor outside the vocabulary (its target left unblended, the mask kept)
        p.depth_stencil = 2;
        p.blend = 2;
        let state = render_state(&t, &p, &mut notes);
        assert_eq!(
            state.depth,
            Some(DepthState {
                test: Compare::Always,
                write: false
            })
        );
        assert_eq!(
            (state.blend, state.targets[0]),
            (
                None,
                Some(TargetState {
                    blend: None,
                    write_mask: 7
                })
            )
        );
        assert_eq!(notes.len(), 1);
        // an effect without tables states none
        assert_eq!(
            render_state(&techniques(), &pass(None, None, false), &mut Vec::new()),
            RenderState::default()
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
        assert_eq!(
            engine_role("gWorldView"),
            Some(ConstantRole::Object(ObjectParameter::WorldView))
        );
        assert_eq!(
            engine_role("gPrevWorldViewProj"),
            Some(ConstantRole::Object(ObjectParameter::PreviousFrame(
                ObjectMatrix::WorldViewProjection
            )))
        );
        assert_eq!(
            engine_role("gRelativeWorld"),
            Some(ConstantRole::Object(ObjectParameter::CameraRelative(
                ObjectMatrix::World
            )))
        );
        assert_eq!(
            engine_role("gRelativePrevWorldView"),
            Some(ConstantRole::Object(
                ObjectParameter::CameraRelativePreviousFrame(ObjectMatrix::WorldView)
            ))
        );
        assert_eq!(
            engine_role("gPrevViewProj"),
            Some(ConstantRole::View(ViewParameter::PreviousViewProjection))
        );
        assert_eq!(
            engine_role("gDirectionalLight"),
            Some(ConstantRole::Frame(FrameParameter::LightDirection))
        );
        assert_eq!(
            engine_role("globalScreenSize"),
            Some(ConstantRole::View(ViewParameter::ViewportSize))
        );
        assert_eq!(
            engine_role("globalAnimUV1"),
            Some(ConstantRole::Object(ObjectParameter::TextureTransformRow(1)))
        );
        assert_eq!(engine_role("globalScalars2"), None);
        assert!(engine_reason("globalScalars2").is_some());
        assert_eq!(engine_role("gSomethingElse"), None);
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
