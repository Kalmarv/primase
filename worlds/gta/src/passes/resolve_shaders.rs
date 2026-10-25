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
//!   required option. Where the technique asked for is a G-buffer pass (`deferred_<S>`) and the effect has none, the
//!   material takes the pass the engine draws it with ([`fallback_technique`], from the effect's own technique names and
//!   the material's stored render bucket): the alpha-clip G-buffer pass `deferredalphaclip_<S>` for a cutout material,
//!   the forward pass `<S>` for any other, counted. An effect without that technique, with several passes in it, with a tessellated pass (the hull
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
//!   constant of the record is bound at its byte offset where the program reads it
//!   ([`primase_shader::WordsRead`] of the program lifted again: the words its value graph reaches, however it loads
//!   them; a buffer indexed by a value that is not a literal is read at every word). A constant the program does not read, or
//!   one past the end of the buffer it declares, is left out, counted. A constant the effect renames (its parameter records:
//!   [`GtaEffectParameters`]) is a material parameter, bound to the words the material stores under the new name
//!   (`Material`), or `Unknown` where it stores none; the engine's world, view and projection matrices have the roles
//!   [`engine_role`] names, and the screen size's two pairs ([`engine_parts`], each bound where read); every other engine
//!   constant is `Unknown` with its name.
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
//!   x and y the occlusion of the natural and of the artificial ambient (`Occlusion`, a fourth power over 4: the
//!   ambient lighting pass scales by the square of twice the stored square) and z the emission (read times 16), and
//!   target 4 the motion vector; every other lane, and every
//!   output of another family or of a program that writes fewer targets, is `Unknown` with why (what [`gbuffer`]'s
//!   table shows of the lane, or that the program is outside the layout), but for the one target of a forward-lit
//!   `PS_Textured_Zero|Four|Eight` program, the finished `Colour`.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::shader::{
    Blend, BlendEquation, BlendFactor, BlendOp, Compare, ConstantBinding, ConstantRole, Cull, D3dClass,
    DepthState, Fill, FrameParameter, InputBinding, InputRole, Interface, NormalEncoding, NormalSpace,
    ObjectMatrix, ObjectParameter, Occluded, OutputBinding, OutputRole, RenderState, ResourceId,
    ResourceKind, SamplerState, ScalarEncoding, Shader, ShaderBinding, Stage, StageBinding, Stencil,
    StencilFace, StencilOp, SystemValue, TargetState, TextureBinding, TextureSource, ViewParameter,
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
use primase_shader::matrix::state_lines;
use primase_shader::{ProgramGraph, WordsRead};
use primase_core::shader::{MaterialPrograms, ProgramFacts};

/// The pixel program family whose targets the pass names roles for (`PS_DeferredTextured` and its alpha-clipped and
/// sub-sample variants: [`gbuffer`] has the layout and how it was checked).
const GBUFFER_FAMILY: &str = "PS_DeferredTextured";

/// How the G-buffer's target 3 x and y are stored, by what the ambient lighting pass does with them: it takes
/// `2 * stored^2` and squares it, the factor it scales the ambient by, so the factor is `4 * stored^4`.
const AMBIENT_OCCLUSION: ScalarEncoding = ScalarEncoding {
    power: 4,
    root: 1,
    scale_bits: 0x4080_0000,
};

/// The forward-lit pixel programs of the shared `Textured` family: `Zero`, `Four` and `Eight` name the number of dynamic
/// lights the program lights with, and each writes the one colour target (`tests/technique_survey.rs`: the `draw`
/// technique of every effect runs `PS_Textured_Eight`, as its `lightweight8_draw` does).
const FORWARD_PROGRAMS: [&str; 3] = ["PS_Textured_Zero", "PS_Textured_Four", "PS_Textured_Eight"];

/// The render bucket of the cutout (alpha-tested) materials ([`crate::passes::lower::draw_order`]).
const CUTOUT_BUCKET: u8 = 3;

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
/// The constant words a program reads ([`WordsRead`]), its DXIL container lifted again, or why it cannot be lifted or
/// followed.
pub fn constant_reads_of(bytecode: &[u8]) -> Result<WordsRead, String> {
    graph_of(bytecode).map(|graph| graph.reads)
}

/// A program's value graph and the constant words it reads ([`ProgramGraph`]), its DXIL container lifted again, or why
/// it cannot be lifted or followed.
fn graph_of(bytecode: &[u8]) -> Result<ProgramGraph, String> {
    primase_dxil::lift(bytecode).map_err(|e| e.to_string())?.graph()
}

/// One lift per program for all the resolver's queries, including inspection. Individual query failures do not
/// become lift failures: the bytecode can be lifted even when a graph or sampler expression cannot be followed.
struct ProgramAnalysis {
    facts: ProgramFacts,
    graph: Result<ProgramGraph,String>,
    samples: Result<SamplerUses,String>,
    writes: Result<GBufferWrites,String>,
}
fn analyse(program: &Shader, name: &str) -> ProgramAnalysis {
    match primase_dxil::lift(&program.bytecode) {
        Ok(lifted) => ProgramAnalysis {facts:lifted.facts(name),graph:lifted.graph(),
            samples:sampler_use::uses_program(&lifted),writes:gbuffer::writes_program(&lifted)},
        Err(error) => {
            let reason = error.to_string();
            ProgramAnalysis {facts:ProgramFacts::unavailable(name,program.interface.stage,
                Some(primase_shader::source_hash(&program.bytecode)),&reason),
                graph:Err(reason.clone()),samples:Err(reason.clone()),writes:Err(reason)}
        }
    }
}

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
    /// The pixel program is one of the forward-lit programs, whose one target is the finished colour.
    forward: bool,
    /// The technique the pass was selected from, where it is not the one asked for: what the engine draws a material with
    /// whose effect has no such pass, and why that one.
    fallback: Option<&'static str>,
}

/// What a texture parameter with no texture becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsetTexture {
    White,
    Leave,
}

impl Choice for UnsetTexture {
    const VALUES: &'static [(&'static str, Self)] =
        &[("white", UnsetTexture::White), ("leave", UnsetTexture::Leave)];
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

    /// From its declared options: `technique` a technique name.
    fn from_given(g: &Given) -> Result<Self, String> {
        let technique = g.required("technique");
        if technique.is_empty() || !technique.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(format!("technique={technique:?} is not a technique name"));
        }
        Ok(Self::new(technique, g.choice("unset-textures")))
    }
}

/// `resolve-gta-shaders`' declaration ([`primase_core::pass::PassRegistry::add`]): which technique of an effect a draw
/// uses (pass, vertex format, alpha mode) is the draw's choice, no default.
pub const RESOLVE_GTA_SHADERS: PassSpec = PassSpec {
    name: "resolve-gta-shaders",
    options: &[
        OptionSpec::required(
            "technique",
            "deferred_draw",
            "the technique drawn with (deferred_draw, deferred_drawskinned, lightweight0_draw...)",
        ),
        OptionSpec::choice::<UnsetTexture>(
            "unset-textures",
            "a texture parameter with no texture reads white, or is left missing",
        ),
    ],
    make: |g| Ok(Box::new(ResolveGtaShaders::from_given(g)?)),
};

/// The permutation hash and the name without it of a program name `<stem>_<8 hex digits>_Wrapped`.
pub fn split_program_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_suffix("_Wrapped")?;
    let (stem, hash) = rest.rsplit_once('_')?;
    (hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some((stem, hash))
}

/// The technique to draw a material with where its effect has no G-buffer pass `deferred_<S>` (the one asked for), and
/// the reason, from the effect's own technique names and the material's stored render bucket. The effects without a
/// `deferred_draw` are the ones the engine does not draw in the G-buffer pass; of the 146 of the shader database, all
/// that have a cutout-bucket material have a `deferredalphaclip_<S>` pass, which writes the same five targets as
/// `deferred_<S>` with the same state, and every other has the forward pass `<S>` (`draw`, `drawskinned`), whose pixel
/// program lights with up to eight lights (`tests/technique_survey.rs`). A cutout material takes the alpha-clip pass or
/// none; any other takes the forward pass if the effect has it. Only a request for a G-buffer pass has a fallback.
pub fn fallback_technique(
    techniques: &GtaEffectTechniques,
    requested: &str,
    bucket: u8,
) -> Option<(String, &'static str)> {
    let suffix = requested.strip_prefix("deferred_")?;
    let has = |name: &str| techniques.techniques.iter().any(|t| t.name == name);
    if bucket == CUTOUT_BUCKET {
        let name = format!("deferredalphaclip_{suffix}");
        return has(&name).then_some((name, "the alpha-clip G-buffer pass of the cutout bucket"));
    }
    has(suffix).then(|| (suffix.to_owned(), "the forward pass"))
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

/// The engine constants whose register packs more than one value: the byte offset, the bytes and the role of each
/// part. `globalScreenSize` is `(x, y, z, w)` with x, y scaling a coordinate into pixels (the render target's size)
/// and z, w the uv-space steps of whole texels (its reciprocals); that z, w are `1 / x` and `1 / y` is the natural
/// reading of their use (`tests/engine_lanes.rs`: no program shown multiplies them together).
pub fn engine_parts(name: &str) -> Option<[(u32, u32, ConstantRole); 2]> {
    Some(match name {
        "globalScreenSize" => [
            (0, 8, ConstantRole::View(ViewParameter::ViewportSize)),
            (8, 8, ConstantRole::View(ViewParameter::InverseViewportSize)),
        ],
        _ => return None,
    })
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
        "gDirectionalLight" => ConstantRole::Frame(FrameParameter::LightDirection),
        "gDirectionalColour" => ConstantRole::Frame(FrameParameter::LightColour),
        // xyz are added to the lit colour's channels by the lighting programs, w scales the vertex colour
        // (tests/engine_lanes.rs)
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
        "TANGENT" => Some(Semantic::Tangent(0)),
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
        self.add_by(what, 1);
    }
    fn add_by(&mut self, what: &str, n: u64) {
        if n > 0 {
            *self.0.entry(what.to_owned()).or_default() += n;
        }
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
        forward: bool,
        analyses: &mut BTreeMap<Entity, ProgramAnalysis>,
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
        let analysis = analyses.entry(shader).or_insert_with(|| analyse(program,
            world.get::<primase_core::Name>(shader).map_or("",|name|name.0.as_str())));
        let uses = &analysis.samples;
        let graph = &analysis.graph;
        let reads = graph.as_ref().map(|graph| &graph.reads);
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
                        // a program whose reads cannot be found keeps every constant its buffers declare
                        if let Ok(reads) = reads
                            && !reads.reads_at(bind.space, bind.register, u32::from(c.offset), bytes)
                        {
                            counts.add("constants the program does not read, left out");
                            continue;
                        }
                        let name = table.name_of(c.name).map(str::to_owned);
                        if let Some(parts) = name.as_deref().and_then(engine_parts) {
                            for (at, size, role) in parts {
                                let offset = u32::from(c.offset) + at;
                                if let Ok(reads) = reads
                                    && !reads.reads_at(bind.space, bind.register, offset, size)
                                {
                                    counts.add("constants the program does not read, left out");
                                    continue;
                                }
                                counts.add("engine constants with a role");
                                constants.push(ConstantBinding {
                                    resource: r.id,
                                    offset,
                                    size,
                                    name: name.clone(),
                                    role,
                                    lines: None,
                                });
                            }
                            continue;
                        }
                        let role = constant_role(inputs, c, bytes, name.as_deref(), bind.space, counts);
                        constants.push(ConstantBinding {
                            resource: r.id,
                            offset: u32::from(c.offset),
                            size: bytes,
                            name,
                            role,
                            lines: None,
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
        let unshown = state_lines(graph.as_ref().map_err(String::as_str), interface, &mut constants);
        counts.add_by(
            "matrices whose lines the program does not show (a consumer writing a non-symmetric matrix cannot bind them)",
            unshown as u64,
        );
        let outputs = if stage == Stage::Pixel {
            output_bindings(interface, writes, forward, counts)
        } else {
            Vec::new()
        };
        Ok(StageBinding {
            stage,
            variant: None,
            shader,
            inputs: in_roles,
            constants,
            textures,
            buffers: Vec::new(),
            constant_buffers: Vec::new(),
            outputs,
        })
    }
}

/// The outputs of a pixel program with their roles: the G-buffer's where the program is of the family and writes its
/// four targets (the layout's lanes as `passes::gbuffer` reads them), `Unknown` with why otherwise.
fn output_bindings(
    interface: &primase_core::shader::Interface,
    writes: Option<&Result<GBufferWrites, String>>,
    forward: bool,
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
                "pixel stages whose output stores could not be read (outputs Unknown with why): {why}"
            ));
            None
        }
        Some(Ok(w)) if four => {
            counts.add("pixel stages with the G-buffer layout of the family");
            Some(w)
        }
        Some(Ok(_)) => {
            counts.add("pixel stages of the family writing fewer than four targets of four lanes (outputs Unknown with why)");
            None
        }
    };
    let one_colour = targets.len() == 1 && targets.iter().all(|v| v.label.index == 0 && v.value.width == 4);
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
                lanes(
                    0b1000,
                    OutputRole::unknown(
                        "G-buffer target 0's w: a byte of the draw's state, from the engine's constants",
                    ),
                );
            }
            (Some(w), 1) if w.normal_biased => {
                lanes(
                    0b0111,
                    OutputRole::Normal {
                        encoding: NormalEncoding::XyzUnorm,
                        space: NormalSpace::World,
                    },
                );
                lanes(
                    0b1000,
                    OutputRole::unknown(
                        "G-buffer target 1's w: the normal's sub-step bits for the lighting passes (tests/gbuffer_normal_w.rs)",
                    ),
                );
            }
            (Some(w), 2) => {
                lanes(
                    0b0001,
                    if w.specular_intensity_encoded {
                        OutputRole::SpecularIntensity {
                            encoding: ScalarEncoding::square_root_of(1.0),
                        }
                    } else {
                        OutputRole::unknown(
                            "G-buffer target 2's x, stored as neither a square root nor 0: not the family's specular intensity",
                        )
                    },
                );
                lanes(
                    0b0010,
                    if w.specular_exponent_encoded {
                        OutputRole::SpecularExponent {
                            encoding: ScalarEncoding::square_root_of(512.0),
                        }
                    } else {
                        OutputRole::unknown(
                            "G-buffer target 2's y, stored as neither a square root nor 0: not the family's specular falloff",
                        )
                    },
                );
                lanes(
                    0b0100,
                    if w.fresnel_written {
                        OutputRole::Fresnel
                    } else {
                        OutputRole::unknown("G-buffer target 2's z, not written: no Fresnel term")
                    },
                );
                lanes(
                    0b1000,
                    OutputRole::unknown("G-buffer target 2's w: the family's layout states nothing of it"),
                );
            }
            (Some(w), 4) if w.motion_written => lanes(0b0011, OutputRole::MotionVector),
            // target 3 as the ambient lighting pass reads it (`PS_lightTile_ambient`, `tests/gbuffer_encodings.rs`): x and y
            // the occlusion of the natural and of the artificial ambient, each squared after the pass has
            // squared the stored value and doubled it (a fourth power over 4), z the emission, read times 16; w a flag
            // that makes the pass multiply both occlusions by `deferredLightTexture2.x`: no role
            (Some(_), 3) => {
                lanes(
                    0b0001,
                    OutputRole::Occlusion {
                        of: Occluded::NaturalAmbient,
                        encoding: AMBIENT_OCCLUSION,
                    },
                );
                lanes(
                    0b0010,
                    OutputRole::Occlusion {
                        of: Occluded::ArtificialAmbient,
                        encoding: AMBIENT_OCCLUSION,
                    },
                );
                lanes(
                    0b0100,
                    OutputRole::Emission {
                        encoding: ScalarEncoding::linear_over(16.0),
                    },
                );
                lanes(
                    0b1000,
                    OutputRole::unknown(
                        "G-buffer target 3's w: a flag, where above one half the ambient pass multiplies both occlusions by deferredLightTexture2.x",
                    ),
                );
            }
            (None, 0) if forward && one_colour => lanes(0b1111, OutputRole::Colour),
            (Some(w), 1) => lanes(
                0b1111,
                OutputRole::unknown(if w.normal_floored {
                    "G-buffer target 1, the world normal stored as floor(n * 128 + 128) / 256 with its fractional bits in w: no normal encoding states it"
                } else if w.normal_zero {
                    "G-buffer target 1, the constant 0 in x, y and z: the pass writes no normal"
                } else {
                    "G-buffer target 1, stored as none of the family's normal encodings"
                }),
            ),
            (Some(_), 4) => lanes(
                0b1111,
                OutputRole::unknown("G-buffer target 4, whose x and y are not written: no motion vector"),
            ),
            (Some(_), n) => lanes(
                0b1111,
                OutputRole::unknown(format!(
                    "target {n} of a G-buffer program, past the five targets of the family's layout"
                )),
            ),
            (None, n) => lanes(
                0b1111,
                OutputRole::unknown(format!(
                    "target {n} of a program outside the G-buffer family's layout and not a single-target forward colour"
                )),
            ),
        }
    }
    if forward && one_colour {
        counts.add("forward pixel stages (PS_Textured_Zero, Four, Eight): target 0 is the finished colour");
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
            "engine frame scalars: in the vertex programs lane y divides the clip depth of a see-through fade (VS_Transform_SeeThrough) and lane z is subtracted from a squared-distance fade of the vertex colour (VS_TransformD); in the pixel programs lane z is multiplied as 1 - z into a wetness term and stored as the top bit of the state byte of target 0 w (floor(128 z)); no game-free meaning established"
        }
        "globalScalars" => {
            "engine frame scalars: lanes y and z are passed on to the pixel stage (VS_decalFx); in the pixel programs lane x multiplies the alpha (the diffuse alpha times the vertex alpha, before the alpha test), and lanes z and y multiply the vertex colour's x and y into the ambient terms of targets 0 w and 3; no game-free meaning established"
        }
        "gStippleAlpha" => {
            "a word whose low two bits are bits 6 and 7 of the state byte the G-buffer programs write to target 0 w ((word << 6) & 192); no game-free meaning established"
        }
        "gTemporalAntiGhosting" => {
            "a flag: the state byte the G-buffer programs write to target 0 w is 23 where the word is non-zero and 7 where it is zero (bit 4); no game-free meaning established"
        }
        "gDynamicBakesAndWetness" => {
            "lanes x and y are the constant and the up-facing coefficient of a scale, y * (0.5 + 0.5 n.z) + x, that multiplies globalScalars z and the vertex colour into the state byte of target 0 w (the ped programs); lane z scales a wetness term (it multiplies the up-facing term that darkens the albedo and raises the specular, the material's wetnessMultiplier applying after, in the parallax programs) and the blend between a dirt texture's channels (the vehicle programs); no game-free meaning established"
        }
        "gLightArtificialIntAmbient0" => {
            "lanes x, y and z are a colour the lighting passes blend with five neighbouring ambient colours (natural, interior and exterior, two each); lane w is added to the vertex colour's x scaled by globalScalars z and stored in target 3 x by the G-buffer programs; no game-free meaning established"
        }
        "gTransparencyAASamples" => {
            "a scalar the hair and foliage alpha-clip programs test: where positive they take alpha 1 and skip the diffuse texture's alpha; no game-free meaning established"
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
            ("unset-textures".to_owned(), self.unset.text().to_owned()),
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
        &[ShaderBinding::NAME, MaterialPrograms::NAME]
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
        let mut chosen: BTreeMap<(Entity, String), Result<Chosen, String>> = BTreeMap::new();
        let mut bound: Vec<(Entity, ShaderBinding)> = Vec::new();
        let mut analyses: BTreeMap<Entity,ProgramAnalysis> = BTreeMap::new();
        for (material, shader, effect_entity) in materials {
            // the technique asked for, or where the effect has none of that name the one its material is drawn with
            let (technique, fallback) = match world.get::<GtaEffectTechniques>(effect_entity) {
                Some(t) if !t.techniques.iter().any(|t| t.name == self.technique) => {
                    match fallback_technique(t, &self.technique, shader.render_bucket) {
                        Some((name, why)) => (name, Some(why)),
                        None => (self.technique.clone(), None),
                    }
                }
                _ => (self.technique.clone(), None),
            };
            let picked = chosen
                .entry((effect_entity, technique.clone()))
                .or_insert_with(|| {
                    let effect = world
                        .get::<GtaEffect>(effect_entity)
                        .ok_or_else(|| "a material's effect entity without its effect".to_owned())?;
                    let techniques = world
                        .get::<GtaEffectTechniques>(effect_entity)
                        .ok_or_else(|| format!("the effect has no technique table ({})", effect.name))?;
                    let pass = select_pass(techniques, &technique)
                        .map_err(|why| format!("{why} ({})", effect.name))?;
                    let vertex =
                        pass.vertex.as_ref().and_then(|p| p.shader).ok_or_else(|| {
                            format!("the vertex program has no interface ({})", effect.name)
                        })?;
                    let pixel = match &pass.pixel {
                        None => None,
                        Some(p) => {
                            let stem = split_program_name(&p.name).map(|(stem, _)| stem);
                            Some((
                                p.shader.ok_or_else(|| {
                                    format!("the pixel program {} has no interface ({})", p.name, effect.name)
                                })?,
                                stem.is_some_and(|s| s.starts_with(GBUFFER_FAMILY)),
                                stem.is_some_and(|s| FORWARD_PROGRAMS.contains(&s)),
                            ))
                        }
                    };
                    let forward = pixel.is_some_and(|(_, _, forward)| forward);
                    // the G-buffer's roles are read from the program's own stores
                    let writes = pixel.and_then(|(entity, gbuffer_family, _)| {
                        gbuffer_family.then(|| {
                            world
                                .get::<Shader>(entity)
                                .ok_or_else(|| "a pixel program without its shader".to_owned())
                                .and_then(|s| analyses.entry(entity).or_insert_with(||analyse(s,
                                    world.get::<primase_core::Name>(entity).map_or("",|name|name.0.as_str()))).writes.clone())
                        })
                    });
                    let mut notes = Vec::new();
                    let state = render_state(techniques, pass, &mut notes);
                    Ok(Chosen {
                        state,
                        notes,
                        pixel: pixel.map(|(entity, _, _)| entity),
                        vertex,
                        writes,
                        forward,
                        fallback,
                    })
                })
                .clone();
            let Chosen {
                state,
                notes,
                pixel,
                vertex,
                writes,
                forward,
                fallback,
            } = match picked {
                Ok(pair) => pair,
                Err(why) => {
                    world.insert(material,MaterialPrograms(vec![ProgramFacts::unavailable(
                        format!("effect {:08x} · {technique}",shader.name),Stage::Pixel,None,&why)]));
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
                    forward,
                    &mut analyses,
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
                world.insert(material,MaterialPrograms(vec![ProgramFacts::unavailable(
                    format!("effect {:08x} · {technique}",shader.name),Stage::Pixel,None,&why)]));
                counts.add(&format!("materials left unbound: {why}"));
                continue;
            }
            counts.add("materials bound");
            if let Some(why) = fallback {
                counts.add(&format!(
                    "materials bound to {technique}, {why}: the effect has no technique {}",
                    self.technique
                ));
            }
            for note in &notes {
                counts.add(&format!("render state: {note}"));
            }
            let facts = stages.iter().filter_map(|stage|analyses.get(&stage.shader)
                .map(|analysis|analysis.facts.clone().bound(stage.shader,&stage.outputs))).collect();
            world.insert(material,MaterialPrograms(facts));
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

    #[test]
    fn the_dxil_analysis_keeps_fixture_facts_and_failure_reasons() {
        // No proprietary GTA bytecode is committed; use the existing owned DXIL draw fixture through the actual
        // GTA shared analysis path, not a mocked instruction count.
        let bytes = include_bytes!("../../../../toolkit/dxil/tests/fixtures/draw_texture.dxil");
        let shader = Shader {format:primase_core::shader::ShaderFormat::Dxil,bytecode:bytes.to_vec(),
            interface:primase_dxil::interface(bytes).unwrap()};
        let analysed = analyse(&shader,"owned draw fixture");
        assert!(analysed.graph.is_ok()); assert!(analysed.samples.is_ok());
        assert_eq!(analysed.facts.hash,Some(primase_shader::source_hash(bytes)));
        assert_eq!(analysed.facts.instruction_count.unwrap().unit,primase_core::shader::InstructionUnit::DxilInstructions);
        assert!(analysed.facts.instruction_count.unwrap().count > 0);
        assert!(analysed.facts.inputs.iter().all(|input|input.used && input.provenance.d3d.is_some()));
        let refused = analyse(&Shader {bytecode:vec![0],..shader},"broken");
        assert!(refused.facts.not_lifted.is_some()); assert!(refused.facts.instruction_count.is_none());
        assert!(refused.graph.is_err()); assert!(refused.samples.is_err());
    }

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

    /// An effect that lacks `deferred_<S>` is drawn by its alpha-clip G-buffer pass where the material is in the cutout
    /// bucket and the effect has one, by its forward pass `<S>` where the material is in any other bucket and the effect
    /// has one, and by none otherwise; a technique that is not a G-buffer pass has no fallback.
    #[test]
    fn a_material_whose_effect_has_no_g_buffer_pass_takes_the_engines_other_pass() {
        let mut t = techniques();
        t.techniques.retain(|t| t.name != "deferred_draw");
        let pass = |t: &GtaEffectTechniques, name: &str, bucket: u8| {
            fallback_technique(t, name, bucket).map(|(n, _)| n)
        };
        // the forward pass of the effect's name, for the buckets that are not the cutout's
        t.techniques.push(technique("draw", Vec::new()));
        assert_eq!(pass(&t, "deferred_draw", 1).as_deref(), Some("draw"));
        assert_eq!(pass(&t, "deferred_draw", 6).as_deref(), Some("draw"));
        // a cutout material takes the alpha-clip pass or none
        assert_eq!(pass(&t, "deferred_draw", CUTOUT_BUCKET), None);
        t.techniques.push(technique("deferredalphaclip_draw", Vec::new()));
        assert_eq!(
            pass(&t, "deferred_draw", CUTOUT_BUCKET).as_deref(),
            Some("deferredalphaclip_draw")
        );
        assert_eq!(pass(&t, "deferred_draw", 0).as_deref(), Some("draw"));
        // the skinned pass has its own suffix; a technique that is not a G-buffer pass has no fallback
        assert_eq!(pass(&t, "deferred_drawskinned", 1), None);
        assert_eq!(pass(&t, "lightweight0_draw", 1), None);
        assert_eq!(pass(&t, "wdcascade_draw", 1), None);
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
        assert_eq!(engine_role("globalScreenSize"), None);
        assert_eq!(
            engine_parts("globalScreenSize"),
            Some([
                (0, 8, ConstantRole::View(ViewParameter::ViewportSize)),
                (8, 8, ConstantRole::View(ViewParameter::InverseViewportSize)),
            ])
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
        assert!(RESOLVE_GTA_SHADERS.instance(&o(&[])).is_err());
        assert!(
            RESOLVE_GTA_SHADERS
                .instance(&o(&[("unset-textures", "white")]))
                .is_err()
        );
        assert!(
            RESOLVE_GTA_SHADERS
                .instance(&o(&[("technique", "deferred draw"), ("unset-textures", "white")]))
                .is_err()
        );
        // the options of the previous version name programs, not a technique
        assert!(
            RESOLVE_GTA_SHADERS
                .instance(&o(&[
                    ("pixel", "PS_DeferredTextured"),
                    ("vertex", "VS_TransformD"),
                    ("unset-textures", "white")
                ]))
                .is_err()
        );
        let p = RESOLVE_GTA_SHADERS
            .instance(&o(&[
                ("technique", "deferred_drawskinned"),
                ("unset-textures", "leave"),
            ]))
            .unwrap();
        assert_eq!(
            p.options(),
            ResolveGtaShaders::new("deferred_drawskinned", UnsetTexture::Leave).options()
        );
        assert_eq!(p.options()["technique"], "deferred_drawskinned");
    }

    /// A pixel program's interface with `count` colour targets of four float lanes (`SV_Target0` on).
    fn targets(count: u32) -> Interface {
        use primase_core::shader::{Label, ScalarType, StageFacts, Value, Varying, VaryingProvenance};
        Interface {
            stage: Stage::Pixel,
            inputs: Vec::new(),
            outputs: (0..count)
                .map(|n| Varying {
                    label: Label {
                        name: "SV_Target".into(),
                        index: n,
                    },
                    system: Some(SystemValue::Target(n)),
                    value: Value {
                        scalar: ScalarType::Float,
                        width: 4,
                    },
                    interp: None,
                    used: true,
                    provenance: VaryingProvenance::default(),
                })
                .collect(),
            resources: Vec::new(),
            facts: StageFacts::None,
        }
    }

    /// The family's G-buffer read as a surface (`primase_core::shader::surface`), the one reading a viewer's composite
    /// takes: the albedo in target 0, the world normal `n * 0.5 + 0.5` in target 1, the roughness from target 2's
    /// specular falloff (`sqrt(n / 512)`, Blinn-Phong) and the specular strength from its x, target 3's z the emission
    /// over 16 as a scale of the albedo and its x the natural ambient's occlusion, which scales the ambient light alone;
    /// no metalness and no reflectance. A forward program's one target is its finished colour, the surface's base.
    #[test]
    fn the_gbuffer_is_read_as_a_surface() {
        use primase_core::shader::surface::{Base, Emission, Place, Roughness, Surface};
        let writes: Result<GBufferWrites, String> = Ok(GBufferWrites {
            normal_biased: true,
            normal_zero: false,
            normal_floored: false,
            specular_intensity_encoded: true,
            specular_exponent_encoded: true,
            fresnel_written: true,
            emission_written: true,
            motion_written: true,
        });
        let interface = targets(5);
        let outputs = output_bindings(&interface, Some(&writes), false, &mut Counts::default());
        let s = Surface::read(outputs.iter().map(|o| (o.lanes, &o.role)));
        assert!(s.refused.is_empty(), "{:?}", s.refused);
        // each place as the target (the label's index) and lanes
        let at = |place: Place| (outputs[place.output].label.index, place.lanes);
        let Some(Base::Albedo(albedo)) = s.base else {
            panic!("an albedo: {:?}", s.base)
        };
        assert_eq!(at(albedo), (0, 0b0111));
        let normal = s.normal.unwrap();
        assert_eq!(
            (at(normal.place), normal.encoding, normal.space),
            ((1, 0b0111), NormalEncoding::XyzUnorm, NormalSpace::World)
        );
        let Some(Roughness::Exponent(exponent)) = s.roughness else {
            panic!("a specular exponent: {:?}", s.roughness)
        };
        assert_eq!(
            (at(exponent.place), exponent.encoding),
            ((2, 0b0010), ScalarEncoding::square_root_of(512.0))
        );
        let specular = s.specular.unwrap();
        assert_eq!(
            (at(specular.place), specular.encoding),
            ((2, 0b0001), ScalarEncoding::square_root_of(1.0))
        );
        let Some(Emission::OfAlbedo(emission)) = s.emission else {
            panic!("one lane of emission: {:?}", s.emission)
        };
        assert_eq!(
            (at(emission.place), emission.encoding),
            ((3, 0b0100), ScalarEncoding::linear_over(16.0))
        );
        let ambient = s.ambient_occlusion.unwrap();
        assert_eq!((at(ambient.place), ambient.encoding), ((3, 0b0001), AMBIENT_OCCLUSION));
        assert_eq!((s.metallic, s.reflectance, s.occlusion), (None, None, None));
        // decoded: a falloff of 510 stored as sqrt(510 / 512) is roughness 1/4, an emission of 0.5 is 8 times the albedo
        let stored = (510.0f32 / 512.0).sqrt();
        let roughness = s.roughness.unwrap().decode([0.0, stored, 0.0, 0.0]);
        assert!((roughness - 0.25).abs() < 1e-4, "{roughness}");
        assert_eq!(
            s.emission.unwrap().decode([0.0, 0.0, 0.5, 0.0], [0.25, 0.5, 0.125]),
            [2.0, 4.0, 1.0]
        );

        let forward = output_bindings(&targets(1), None, true, &mut Counts::default());
        let s = Surface::read(forward.iter().map(|o| (o.lanes, &o.role)));
        assert_eq!(
            s.base,
            Some(Base::Colour(Place {
                output: 0,
                lanes: 0b1111
            }))
        );
        assert_eq!((s.normal, s.roughness), (None, None));
    }
}
