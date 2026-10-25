//! `resolve-unity-shaders{tier=N,keywords=A+B|scene|none,colour-space=linear|gamma}`: the programs of the pass each Unity
//! material draws with (`unity.programs`, [`crate::program::read_programs`]) as `core.shader` entities and each
//! material's `core.shader_binding`, by Unity's rules. The one home of those rules for the binding; `bind` still serves
//! the CPU evaluation until it is moved onto the binding.
//!
//! - **Variants.** The draw's keyword set is the material's keywords and `keywords` (the renderer's: the light, shadow,
//!   lightmap and probe keywords Unity sets per draw; `scene` stands for the ones the scene's own state gives each
//!   material, [`SceneKeywords`]); with hardware tier `tier`, each stage's variant is the one
//!   whose keywords are the draw's restricted to those the stage uses ([`UnityPrograms::draw_variants`]). A player
//!   stores a stage's variants only for the keyword sets a draw can have, so the draw also has the keywords every
//!   stored variant of a stage has in the tier (a light type the pass always runs under, a feature the pipeline
//!   always enables): counted per keyword, since they are the build's evidence, not the material's or the renderer's.
//!   A material with none for a stage's variants is counted and the stage left out (no fragment variant: the
//!   material is left unbound).
//! - **Constants.** Each constant-buffer field the variant's parameters name, at its byte offset: a float or int
//!   property of the material by the field's name; a colour property (`colour-space=linear`: converted from its
//!   gamma-encoded storage to linear, as Unity sets colours in a linear-colour-space player; `gamma`: as stored); a
//!   texture's `<name>_ST` tiling (scale x, y, offset x, y); Unity's own matrices and camera position as the roles of
//!   [`builtin`]; its vectors that pack several values (the screen size, the projection's flip and planes, the time) part
//!   by part ([`builtin_parts`]: each part's own role, the derived ones `Unknown`); anything else `Unknown` with the
//!   reason, struct (instancing) fields included. Only the bytes the program reads are bound
//!   ([`primase_shader::WordsRead`] of the lifted program: the words its outputs, discards and writes are made of; a
//!   program whose value graph cannot be followed has every byte bound, counted): a field or part it does not read is
//!   left out, counted,
//!   and the bytes between the fields it reads are `Unknown` ("the player stores no field for them"). A buffer the player
//!   gives no binding slot (the Universal pipeline's `UnityPerMaterial`, which holds the material's own properties, and
//!   `LightShadows`) is not placed: the DXBC names no buffer and its sizes are not Unity's (a buffer's size there is
//!   not its declared size), so the register is not told, and its read bytes are such gaps.
//! - **Textures.** A texture parameter binds the texture the material's property of that name holds; a property the
//!   material leaves unset takes the shader's declared default, valued from the editor oracle
//!   (`primase-oracles/unity/default-textures`: [`default_colour`], whose doc states the version and colour-space gap;
//!   a player holds no default texture of the engine), counted by name (an empty or unknown name is the engine's
//!   fallback grey); a default it does not value is `Missing`. A texture of Unity's own that the material has no
//!   property of is the frame's ([`frame_texture`]: the camera's depth and colour, the main light's shadow map or
//!   screen-space shadows, the occlusion, the nearest reflection probe, the light-probe volume), counted by name; the
//!   other engine textures the parameters name are `Missing` with the reason, and a slot the parameters do not name is
//!   `Missing` (a 2021.2 player stores a variant's textures in its parameter blob, which
//!   [`crate::program::parameter_blob`] reads: the pass's common parameters name none). The sampler state is Unity's
//!   packed value ([`crate::components::Parameters::samplers`]): not decoded.
//! - **Outputs.** The fragment stage's first target is the finished colour of a forward pass; the others `Unknown`
//!   (what the render pipeline binds past the first target, and what reads it, is the project's and not read).
//! - **State.** The pass's stored render state (`m_State`), each value the material's property where the pass binds
//!   it to one: every render target's blend (colour and alpha equations: factors and operation; `Blend One Zero` is no
//!   blending; the pass blends each target itself only when it says so, otherwise every target takes the first's, whose
//!   stored copies are defaults) and colour mask (the target's own: an unindexed `ColorMask` states target 0 only), the
//!   culling, the depth test (`Disabled` passes everything) and write, `Offset factor, units` as the slope-scaled and
//!   constant depth bias, and the stencil (the reference, the masks, and per face the operations and comparison: the
//!   common `Comp`, `Pass`, `Fail` and `ZFail` stand for a face that states none of its own; a pass whose faces always
//!   pass and keep has no stencil). A value the vocabulary has none for (`SrcAlphaSaturate`, a logical blend
//!   operation, a mask that is not a byte) leaves that part unstated and the pass is counted. Not stated: the fill, the
//!   winding, `ZClip`, alpha-to-coverage and the conservative flag.
use std::collections::BTreeMap;

use primase_core::pass::{Choice, Given, OptionSpec, PassSpec};
use primase_core::shader::{
    Blend, BlendEquation, BlendFactor, BlendOp, Compare, ConstantBinding, ConstantRole, Cull, D3dClass,
    DepthBias, DepthState, FrameParameter, FrameTexture, InputBinding, InputRole, Interface, ObjectParameter,
    OutputBinding, OutputRole, RenderState, SamplerState, Shader, ShaderBinding, ShaderFormat, Stage,
    StageBinding, Stencil, StencilFace, StencilOp, SystemValue, TargetState, TextureBinding, TextureSource,
    ViewParameter,
};
use primase_core::{Component, Entity, Name, Semantic, Texture, World};
use primase_shader::matrix::state_lines;
use primase_shader::{ProgramGraph, WordsRead};

use crate::components::{Parameters, Shape, UnityMaterial, UnityProgramVariant, UnityPrograms, UnityTexEnv};
use crate::passes::draw_keywords::{SceneKeywords, receivers};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveUnityShaders {
    tier: usize,
    keywords: Vec<String>,
    /// Whether the scene's state gives each material its renderer keywords too.
    scene: bool,
    linear: bool,
}

impl ResolveUnityShaders {
    pub fn new(tier: usize, keywords: Vec<String>, linear: bool) -> Self {
        ResolveUnityShaders {
            tier,
            keywords,
            scene: false,
            linear,
        }
    }

    /// Also give each material the keywords the scene's own state gives its draws ([`SceneKeywords`]).
    pub fn with_scene_keywords(mut self) -> Self {
        self.scene = true;
        self
    }

    /// From its declared options, through [`ResolveUnityShaders::new`]: `keywords` the renderer's joined by `+`,
    /// `scene` among them for the scene's own, or `none`.
    fn from_given(g: &Given) -> Result<Self, String> {
        let v = g.required("tier");
        let tier = v
            .parse::<usize>()
            .map_err(|_| format!("tier={v} is not a number"))?;
        let v = g.required("keywords");
        let (mut named, mut scene) = (Vec::new(), false);
        for k in v.split('+') {
            match k {
                "none" if v == "none" => {}
                "scene" => scene = true,
                _ => named.push(k.to_owned()),
            }
        }
        let pass = Self::new(
            tier,
            named,
            g.choice::<ColourSpace>("colour-space") == ColourSpace::Linear,
        );
        Ok(if scene { pass.with_scene_keywords() } else { pass })
    }
}

/// Option `colour-space`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColourSpace {
    Linear,
    Gamma,
}

impl Choice for ColourSpace {
    const VALUES: &'static [(&'static str, Self)] =
        &[("linear", ColourSpace::Linear), ("gamma", ColourSpace::Gamma)];
}

/// `resolve-unity-shaders`' declaration ([`primase_core::pass::PassRegistry::add`]): the variant and colour space
/// are the program's, every option required.
pub const RESOLVE_UNITY_SHADERS: PassSpec = PassSpec {
    name: "resolve-unity-shaders",
    options: &[
        OptionSpec::required("tier", "0", "the hardware tier the variants are chosen for"),
        OptionSpec::required(
            "keywords",
            "none",
            "the renderer's keywords joined by +, scene among them for the ones the scene's state gives, or none",
        ),
        OptionSpec::choice::<ColourSpace>(
            "colour-space",
            "whether the player sets colour properties linear (converted from storage) or as stored",
        ),
    ],
    make: |g| Ok(Box::new(ResolveUnityShaders::from_given(g)?)),
};

/// What Unity's built-in constants are, by field name, where one role names the whole field: the matrices and the camera
/// position the roles of the vocabulary name. (`unity_CameraToWorld` is the camera's world matrix, the inverse of
/// `unity_MatrixV` by Unity's own definition.)
pub fn builtin(name: &str) -> Option<ConstantRole> {
    Some(match name {
        "unity_ObjectToWorld" => ConstantRole::Object(ObjectParameter::World),
        "unity_WorldToObject" => ConstantRole::Object(ObjectParameter::InverseWorld),
        "unity_MatrixV" => ConstantRole::View(ViewParameter::View),
        "unity_MatrixInvV" | "unity_CameraToWorld" => ConstantRole::View(ViewParameter::InverseView),
        "unity_MatrixVP" => ConstantRole::View(ViewParameter::ViewProjection),
        "unity_MatrixInvVP" => ConstantRole::View(ViewParameter::InverseViewProjection),
        "glstate_matrix_projection" => ConstantRole::View(ViewParameter::Projection),
        "_WorldSpaceCameraPos" => ConstantRole::View(ViewParameter::CameraPosition),
        _ => return None,
    })
}

/// Unity's built-in vectors that pack several values in one register: the byte offset in the field, the bytes and the
/// role of each part. A part with no role in the vocabulary (a value derived from another, or one the vocabulary has no
/// meaning for) is `Unknown` with the reason.
pub fn builtin_parts(name: &str) -> Option<Vec<(u32, u32, ConstantRole)>> {
    let derived = |of: &str, how: &str| ConstantRole::Unknown {
        why: format!(
            "{name} {how}: derived from {of}, which has the role; the vocabulary names no derived value"
        ),
    };
    Some(match name {
        // `(width, height, 1 + 1/width, 1 + 1/height)` of the render target
        "_ScreenParams" => vec![
            (0, 8, ConstantRole::View(ViewParameter::ViewportSize)),
            (
                8,
                8,
                ConstantRole::Unknown {
                    why: "_ScreenParams.zw is (1 + 1/width, 1 + 1/height): the reciprocals of the viewport plus one, which the vocabulary has no role for".into(),
                },
            ),
        ],
        // `(flip, near, far, 1/far)`: the flip is +1, or -1 when the projection is flipped, so Unity's sign is core's
        "_ProjectionParams" => vec![
            (0, 4, ConstantRole::View(ViewParameter::ProjectionFlip)),
            (4, 4, ConstantRole::View(ViewParameter::NearPlane)),
            (8, 4, ConstantRole::View(ViewParameter::FarPlane)),
            (12, 4, ConstantRole::View(ViewParameter::InverseFarPlane)),
        ],
        // `(t/20, t, t*2, t*3)`
        "_Time" => vec![
            (0, 4, derived("_Time.y", "x is t / 20")),
            (4, 4, ConstantRole::Frame(FrameParameter::Time)),
            (8, 4, derived("_Time.y", "z is t * 2")),
            (12, 4, derived("_Time.y", "w is t * 3")),
        ],
        // `(t, sin t, cos t)` in the Universal pipeline's shaders
        "_TimeParameters" => vec![
            (0, 4, ConstantRole::Frame(FrameParameter::Time)),
            (4, 4, derived("_TimeParameters.x", "y is sin(t)")),
            (8, 4, derived("_TimeParameters.x", "z is cos(t)")),
        ],
        "_SinTime" => vec![(0, 16, derived("_Time.y", "is sin(t / 8), sin(t / 4), sin(t / 2), sin(t)"))],
        "_CosTime" => vec![(0, 16, derived("_Time.y", "is cos(t / 8), cos(t / 4), cos(t / 2), cos(t)"))],
        _ => return None,
    })
}

/// Why a built-in Unity constant has no role in the vocabulary, for the ones whose meaning is known.
fn builtin_reason(name: &str) -> Option<&'static str> {
    Some(match name {
        "_WorldSpaceLightPos0" | "_MainLightPosition" => {
            "the main light's position (w 1) or, for a directional light (w 0), the direction toward the light: the vocabulary's light direction is the direction the light travels, and a point or spot light has no role"
        }
        "_LightColor0" | "_MainLightColor" => {
            "the colour of the light the pass is drawn for, times its intensity: no role for a per-pass light colour"
        }
        "_LightShadowData" | "unity_ShadowFadeCenterAndType" | "_MainLightShadowParams" => {
            "the shadow strength and fade of the light the pass is drawn for: no role"
        }
        "unity_OrthoParams" => {
            "(orthographic width, height, 0, 1 if the camera is orthographic else 0): no role"
        }
        "_ZBufferParams" => {
            "the factors that turn the depth buffer's value into eye depth, derived from the near and far planes and the depth direction: no role"
        }
        "unity_WorldTransformParams" => {
            "w is -1 for an object with an odd number of negative scale axes (it flips the bitangent), else 1: a per-object value with no role"
        }
        "unity_LightmapST" | "unity_DynamicLightmapST" => {
            "the scale and offset of the renderer's lightmap coordinates: a per-renderer baked-light value with no role"
        }
        "_MainLightLayerMask" | "_MainLightWorldToShadow" => {
            "the main light's layer mask or its cascades' world-to-shadow matrices: a per-pass light value with no role"
        }
        n if n.starts_with("_AdditionalLights") => {
            "an array of the additional lights' values (position, colour, attenuation, spot direction, layer mask), one entry per light: no role"
        }
        "_FPParams0" | "_FPParams1" | "_FPParams2" | "urp_Tiles" | "urp_ZBins" => {
            "the Forward+ light culling's parameters, tile masks or depth bins: no role"
        }
        "_GlobalMipBias" => "the mip bias of dynamic resolution and its power of two: no role",
        "_ScaledScreenParams" => {
            "(width, height, 1 + 1/width, 1 + 1/height) of the scaled render target: no role"
        }
        n if n.starts_with("_GlossyEnvironmentCubeMap") => {
            "the decode parameters of the environment reflection cube: no role"
        }
        n if n.starts_with("unity_SH") => {
            "a spherical-harmonics coefficient of the ambient light from the light probes: no role"
        }
        n if n.starts_with("unity_SpecCube") || n.starts_with("urp_ReflProbes") => {
            "a reflection probe's value: no role"
        }
        n if n.starts_with("unity_ProbeVolume") || n == "unity_OcclusionMaskSelector" => {
            "a light-probe-volume value: no role"
        }
        _ => return None,
    })
}

/// A field's bytes under D3D packing: a vector its columns, a matrix four per row, each array element but the last
/// padded to whole registers.
fn field_bytes(shape: Shape, array_size: u32) -> u32 {
    let one = match shape {
        Shape::Vector { columns } => columns,
        Shape::Matrix { rows } => rows * 4,
    };
    4 * match array_size {
        0 => one,
        n => (n - 1) * one.div_ceil(4) * 4 + one,
    }
}

fn words(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The material's value for a constant-buffer field, if it has one.
fn material_value(m: &UnityMaterial, name: &str, linear: bool) -> Option<Vec<u32>> {
    if let Some(v) = m.float(name) {
        return Some(words(&[v]));
    }
    if let Some(c) = m.color(name) {
        let l = |x: f32| primase_image::srgb_to_linear(f64::from(x)) as f32;
        return Some(words(&if linear {
            [l(c[0]), l(c[1]), l(c[2]), c[3]]
        } else {
            c
        }));
    }
    if let Some((_, i)) = m.ints.iter().find(|(k, _)| k == name) {
        return Some(words(&[*i as f32]));
    }
    let base = name.strip_suffix("_ST")?;
    let (scale, offset) = m.textures.iter().find(|t| t.name == base)?.finite_tiling()?;
    Some(words(&[scale[0], scale[1], offset[0], offset[1]]))
}

/// A declared field or struct of a buffer and the bindings it stands for: its bytes at `offset`, as parts (the byte
/// offset in the field, the bytes, the role).
struct Declared {
    offset: u32,
    size: u32,
    name: String,
    parts: Vec<(u32, u32, ConstantRole)>,
}

/// The constants of the constant buffer `resource` (of `size` bytes, register `slot`) from the variant's parameters:
/// each named field and struct field, and `Unknown` over the bytes no field covers, where the program (`reads`) reads
/// them. A field or part of a field the program does not read, and bytes it does not read between the fields, are left
/// out, counted. A built-in vector that packs several values is bound part by part ([`builtin_parts`]). `reads` is
/// `None` for a program whose reads could not be taken: every byte is then bound.
#[allow(clippy::too_many_arguments)]
pub fn buffer_constants(
    parameters: &Parameters,
    slot: u32,
    resource: primase_core::shader::ResourceId,
    size: u32,
    m: &UnityMaterial,
    linear: bool,
    reads: Option<&WordsRead>,
    counts: &mut BTreeMap<String, u64>,
) -> Vec<ConstantBinding> {
    let mut bump = |what: &str| *counts.entry(what.to_owned()).or_default() += 1;
    let mut declared: Vec<Declared> = Vec::new();
    if let Some(cb) = parameters
        .constant_buffers
        .iter()
        .find(|cb| cb.slot == Some(slot))
    {
        for f in &cb.fields {
            let bytes = field_bytes(f.shape, f.array_size);
            let whole = |role| vec![(0, bytes, role)];
            let parts = if let Some(mut w) = material_value(m, &f.name, linear) {
                w.truncate(bytes as usize / 4);
                bump("constants set from the material's properties");
                whole(ConstantRole::Material(w))
            } else if let Some(role) = builtin(&f.name) {
                bump("constants that are Unity's built-in matrices and camera position");
                whole(role)
            } else if let Some(parts) = builtin_parts(&f.name) {
                bump("constants that are Unity's built-in vectors of several values, bound part by part");
                parts.into_iter().filter(|(at, n, _)| at + n <= bytes).collect()
            } else {
                bump(
                    "constants no role is resolved for (not a material property, not in the built-in table)",
                );
                whole(ConstantRole::Unknown {
                    why: match builtin_reason(&f.name) {
                        Some(reason) => format!("{}: {reason}", f.name),
                        None => format!(
                            "{}: not a material property of this name and not a built-in this pass maps",
                            f.name
                        ),
                    },
                })
            };
            declared.push(Declared {
                offset: f.offset,
                size: bytes,
                name: f.name.clone(),
                parts,
            });
        }
        for s in &cb.structs {
            bump("struct (instancing) constants, Unknown");
            let bytes = s.size.saturating_mul(s.array_size.max(1));
            declared.push(Declared {
                offset: s.offset,
                size: bytes,
                name: s.name.clone(),
                parts: vec![(
                    0,
                    bytes,
                    ConstantRole::Unknown {
                        why: format!("{}: a struct field (an instancing array): not laid out", s.name),
                    },
                )],
            });
        }
    }
    // only what fits the buffer the program declares
    declared.retain(|c| c.size > 0 && c.offset.checked_add(c.size).is_some_and(|end| end <= size));
    declared.sort_by_key(|c| (c.offset, c.size));
    // overlapping declarations (a field and a struct's member over the same bytes) keep the first
    let mut kept: Vec<Declared> = Vec::new();
    for c in declared {
        if kept.last().is_none_or(|l| c.offset >= l.offset + l.size) {
            kept.push(c);
        }
    }
    // whether the program reads the bytes (every byte, where its reads are not known)
    let read = |offset: u32, bytes: u32| reads.is_none_or(|r| r.reads_at(0, slot, offset, bytes));
    let mut out = Vec::new();
    let mut unread = 0u64;
    let mut at = 0;
    let mut gaps = Vec::new();
    for c in &kept {
        if c.offset > at {
            gaps.push((at, c.offset - at));
        }
        at = c.offset + c.size;
        for (rel, bytes, role) in &c.parts {
            if read(c.offset + rel, *bytes) {
                out.push(ConstantBinding {
                    resource,
                    offset: c.offset + rel,
                    size: *bytes,
                    name: Some(c.name.clone()),
                    role: role.clone(),
                    lines: None,
                });
            } else {
                unread += 1;
            }
        }
    }
    if at < size {
        gaps.push((at, size - at));
    }
    // the bytes between the fields, in runs of words the program reads
    for (offset, bytes) in gaps {
        let mut word = offset;
        while word < offset + bytes {
            if !read(word, 4) {
                unread += 1;
                while word < offset + bytes && !read(word, 4) {
                    word += 4;
                }
                continue;
            }
            let start = word;
            while word < offset + bytes && read(word, 4) {
                word += 4;
            }
            out.push(ConstantBinding {
                resource,
                offset: start,
                size: word.min(offset + bytes) - start,
                name: None,
                role: ConstantRole::Unknown {
                    why: "the program reads these bytes and the player stores no field for them".into(),
                },
                lines: None,
            });
        }
    }
    if unread > 0 {
        *counts
            .entry("constants and runs of bytes the program does not read, left out".into())
            .or_default() += unread;
    }
    out.sort_by_key(|c| c.offset);
    out
}

/// A shader's declared default texture (`white`...) as the colour a fragment samples from it, where Unity's name is
/// one this pass values: the editor oracle's (`primase-oracles/unity/default-textures`, Unity 6000.6.4f1 in a Gamma
/// colour space project; a player does not hold the engine's default textures). `white` is 1 in every channel, `black`
/// and `red` have alpha 0, `gray`, `grey`, `linearGray` and `linearGrey` are 127/255 in every channel (alpha too) and
/// `bump` is (127/255, 127/255, 1, 1). An empty name (`2D) = "" {}`) or one that is no default is the engine's fallback,
/// 128/255 in every channel (not `gray`'s 127/255: the oracle's `none` and `notadefault` properties). Those are the
/// editor's, not verified for the players' versions (SkateBIRD 2019.4, Sledding Game 6000.3), and for a Linear colour
/// space (`linear`) only the colours whose channels are all 0 or 1 (`white`, `black`, `red`) are valued: a grey sampled
/// through an sRGB decode is another number, which the oracle did not observe.
fn default_colour(name: &str, linear: bool) -> Option<[f32; 4]> {
    const GREY: f32 = 127.0 / 255.0;
    Some(match name {
        "white" => [1.0; 4],
        "black" => [0.0; 4],
        "red" => [1.0, 0.0, 0.0, 0.0],
        _ if linear => return None,
        "gray" | "grey" | "linearGray" | "linearGrey" => [GREY; 4],
        "bump" => [GREY, GREY, 1.0, 1.0],
        _ => [FALLBACK; 4],
    })
}

/// The engine's fallback for a default texture name that is empty or no default: 128/255 (the oracle's `none` and
/// `notadefault` properties).
const FALLBACK: f32 = 128.0 / 255.0;

/// Whether `name` is valued as the engine's fallback ([`default_colour`]): an empty name or one that is no default.
fn is_fallback(name: &str) -> bool {
    default_colour(name, false) == Some([FALLBACK; 4])
}

/// Unity's `BlendMode` as the vocabulary's factor; `None` for `SrcAlphaSaturate`.
fn blend_factor(mode: u8) -> Option<BlendFactor> {
    Some(match mode {
        0 => BlendFactor::Zero,
        1 => BlendFactor::One,
        2 => BlendFactor::DstColour,
        3 => BlendFactor::SrcColour,
        4 => BlendFactor::OneMinusDstColour,
        5 => BlendFactor::SrcAlpha,
        6 => BlendFactor::OneMinusSrcColour,
        7 => BlendFactor::DstAlpha,
        8 => BlendFactor::OneMinusDstAlpha,
        10 => BlendFactor::OneMinusSrcAlpha,
        _ => return None,
    })
}

/// Unity's `BlendOp` as the vocabulary's; `None` for the logical operations (5 and up).
fn blend_op(op: u8) -> Option<BlendOp> {
    Some(match op {
        0 => BlendOp::Add,
        1 => BlendOp::Subtract,
        2 => BlendOp::ReverseSubtract,
        3 => BlendOp::Min,
        4 => BlendOp::Max,
        _ => return None,
    })
}

/// Unity's `CompareFunction` as the vocabulary's; `None` for 0 (disabled) and what is not one.
fn compare(function: u8) -> Option<Compare> {
    Some(match function {
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

/// Unity's `StencilOp` as the vocabulary's.
fn stencil_op(op: u8) -> Option<StencilOp> {
    Some(match op {
        0 => StencilOp::Keep,
        1 => StencilOp::Zero,
        2 => StencilOp::Replace,
        3 => StencilOp::IncrementClamp,
        4 => StencilOp::DecrementClamp,
        5 => StencilOp::Invert,
        6 => StencilOp::IncrementWrap,
        7 => StencilOp::DecrementWrap,
        _ => return None,
    })
}

/// A state value that is a whole number in `0..=max`: the material's property where one is bound
/// ([`UnityMaterial::state_value`]).
fn whole(m: &UnityMaterial, v: &crate::shader::ShaderValue, max: u8) -> Option<u8> {
    let x = m.state_value(v);
    (x.fract() == 0.0 && (0.0..=f32::from(max)).contains(&x)).then_some(x as u8)
}

/// What a target's blend states: `Ok(None)` for no blending (`Blend One Zero`, both equations, `Add`), `Err(())` for a
/// value the vocabulary has none for (`SrcAlphaSaturate`, a logical operation), and `Ok(None)` too where a factor is
/// not a whole `BlendMode` (the property is unset: nothing is stated).
fn target_blend(m: &UnityMaterial, t: &crate::shader::ShaderTargetBlend) -> Result<Option<Blend>, ()> {
    let equation = |src, dst, op| -> Option<Result<BlendEquation, ()>> {
        let (src, dst, op) = (
            whole(m, src, crate::shader::blend_mode::LAST)?,
            whole(m, dst, crate::shader::blend_mode::LAST)?,
            whole(m, op, u8::MAX)?,
        );
        Some(match (blend_factor(src), blend_factor(dst), blend_op(op)) {
            (Some(source), Some(destination), Some(op)) => Ok(BlendEquation {
                source,
                destination,
                op,
            }),
            _ => Err(()),
        })
    };
    let (Some(colour), Some(alpha)) = (
        equation(&t.src, &t.dst, &t.op),
        equation(&t.src_alpha, &t.dst_alpha, &t.op_alpha),
    ) else {
        return Ok(None);
    };
    let (colour, alpha) = (colour?, alpha?);
    let none = BlendEquation {
        source: BlendFactor::One,
        destination: BlendFactor::Zero,
        op: BlendOp::Add,
    };
    Ok((colour != none || alpha != none).then_some(Blend { colour, alpha }))
}

/// A stencil face of the pass: its own operations where it states some (not Unity's defaults), else the common ones
/// (`Comp`, `Pass`, `Fail` and `ZFail` set both faces, `CompFront` and the like one). `None` where the face does not
/// test (`Comp Disabled`) or a value is not one the vocabulary has; a face that always passes and keeps is the
/// no-op face.
fn stencil_face(
    m: &UnityMaterial,
    common: &crate::shader::ShaderStencilOp,
    own: &crate::shader::ShaderStencilOp,
) -> Option<Option<StencilFace>> {
    let fields =
        |o: &crate::shader::ShaderStencilOp| [&o.pass, &o.fail, &o.z_fail, &o.comp].map(|v| whole(m, v, 8));
    let stated = |o: &crate::shader::ShaderStencilOp| {
        o.pass.property.is_some()
            || o.fail.property.is_some()
            || o.z_fail.property.is_some()
            || o.comp.property.is_some()
            || [o.pass.value, o.fail.value, o.z_fail.value]
                .iter()
                .any(|v| *v != 0.0)
            || o.comp.value != 8.0
    };
    let source = if stated(own) { own } else { common };
    let [pass, fail, z_fail, comp] = fields(source);
    let (pass, fail, z_fail, comp) = (pass?, fail?, z_fail?, comp?);
    if comp == 0 {
        return Some(None);
    }
    Some(Some(StencilFace {
        fail: stencil_op(fail)?,
        depth_fail: stencil_op(z_fail)?,
        pass: stencil_op(pass)?,
        compare: compare(comp)?,
    }))
}

/// The render state the material's draw pass states (see the module documentation), and whether it had a value the
/// vocabulary has none for (a blend factor, a blend or stencil operation, a mask or reference that is not a byte).
pub fn render_state(m: &UnityMaterial, programs: &UnityPrograms) -> (RenderState, bool) {
    let mut state = RenderState::default();
    let mut unmapped = false;
    let Some(pass) = m
        .shader_state
        .get(programs.subshader)
        .and_then(|s| s.passes.get(programs.pass))
    else {
        return (state, false);
    };
    let detail = &pass.state;
    // the blend of a target: its own where the pass blends each target separately, else the first target's (the
    // stored blends of targets 1 to 7 are then defaults the engine does not use)
    let mut blends = Vec::with_capacity(detail.targets.len());
    for (i, t) in detail.targets.iter().enumerate() {
        let source = if detail.separate_blend {
            t
        } else {
            detail.targets.first().unwrap_or(t)
        };
        let blend = target_blend(m, source).unwrap_or_else(|()| {
            unmapped = true;
            None
        });
        // the colour mask is the target's own (an unindexed `ColorMask` states target 0 only)
        let mask = whole(m, &t.mask, 15);
        if mask.is_none() {
            unmapped = true;
        }
        blends.push(blend);
        if let Some(slot) = state.targets.get_mut(i) {
            *slot = Some(TargetState {
                blend,
                write_mask: mask.unwrap_or(0xf),
            });
        }
    }
    state.blend = blends.first().copied().flatten();
    let cull = m.state_value(&pass.cull);
    state.cull = match cull as i64 {
        0 => Some(Cull::None),
        1 => Some(Cull::Front),
        2 => Some(Cull::Back),
        _ => None,
    };
    // the depth test (`Disabled` passes everything) and write
    state.depth = Some(DepthState {
        test: whole(m, &detail.z_test, 8)
            .and_then(|f| if f == 0 { Some(Compare::Always) } else { compare(f) })
            .unwrap_or_else(|| {
                unmapped = true;
                Compare::LessEqual
            }),
        write: m.state_value(&pass.zwrite) != 0.0,
    });
    // `Offset factor, units`: ShaderLab's factor scales the maximum depth slope and its units the smallest resolvable
    // depth difference, which is D3D12's slope-scaled and constant bias
    let (factor, units) = (
        m.state_value(&detail.offset_factor),
        m.state_value(&detail.offset_units),
    );
    if factor != 0.0 || units != 0.0 {
        state.depth_bias = Some(DepthBias {
            constant: units,
            slope_scaled: factor,
            clamp: 0.0,
        });
    }
    let front = stencil_face(m, &detail.stencil_op, &detail.stencil_op_front);
    let back = stencil_face(m, &detail.stencil_op, &detail.stencil_op_back);
    let masks = (
        whole(m, &detail.stencil_read_mask, 255),
        whole(m, &detail.stencil_write_mask, 255),
        whole(m, &detail.stencil_ref, 255),
    );
    match (front, back, masks) {
        (Some(front), Some(back), (Some(read_mask), Some(write_mask), Some(reference))) => {
            let noop = StencilFace {
                fail: StencilOp::Keep,
                depth_fail: StencilOp::Keep,
                pass: StencilOp::Keep,
                compare: Compare::Always,
            };
            let (front, back) = (front.unwrap_or(noop), back.unwrap_or(noop));
            // faces that always pass and keep are no stencil test
            if front != noop || back != noop {
                state.stencil = Some(Stencil {
                    read_mask,
                    write_mask,
                    reference: Some(reference),
                    front,
                    back,
                });
            }
        }
        _ => unmapped = true,
    }
    (state, unmapped)
}

fn semantic(name: &str, index: u32) -> Option<Semantic> {
    let n = u8::try_from(index).ok()?;
    match name.to_ascii_uppercase().as_str() {
        "POSITION" => Some(Semantic::Position),
        "NORMAL" => Some(Semantic::Normal),
        "TANGENT" => Some(Semantic::Tangent(0)),
        "COLOR" => Some(Semantic::Color(n)),
        "TEXCOORD" => Some(Semantic::TexCoord(n)),
        _ => None,
    }
}

impl primase_core::pass::Pass for ResolveUnityShaders {
    fn name(&self) -> &'static str {
        "resolve-unity-shaders"
    }
    fn options(&self) -> primase_core::pass::Options {
        primase_core::pass::Options::from([
            ("tier".to_owned(), self.tier.to_string()),
            (
                "keywords".to_owned(),
                match (self.scene, self.keywords.is_empty()) {
                    (false, true) => "none".to_owned(),
                    (true, true) => "scene".to_owned(),
                    (scene, false) => {
                        let mut named = self.keywords.clone();
                        if scene {
                            named.insert(0, "scene".to_owned());
                        }
                        named.join("+")
                    }
                },
            ),
            (
                "colour-space".to_owned(),
                if self.linear {
                    ColourSpace::Linear
                } else {
                    ColourSpace::Gamma
                }
                .text()
                .to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[UnityMaterial::NAME, UnityPrograms::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Shader::NAME, ShaderBinding::NAME, Name::NAME]
    }
    fn run(&self, world: &mut World, log: &mut primase_core::pass::PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, UnityMaterial, UnityPrograms)> = world
            .iter::<UnityMaterial>()
            .filter_map(|(e, m)| Some((e, m.clone(), world.get::<UnityPrograms>(e)?.clone())))
            .collect();
        let without: u64 = world
            .iter::<UnityMaterial>()
            .filter(|(e, _)| !world.has::<UnityPrograms>(*e))
            .count() as u64;
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        counts.insert(
            "materials without unity.programs (no draw pass, shader object or D3D11 code)".into(),
            without,
        );
        let mut shaders: BTreeMap<std::sync::Arc<[u8]>, Result<Entity, String>> = BTreeMap::new();
        // each program's value graph and the constants it reads, taken when it is lifted, or why the graph could not be
        let mut graph_of: BTreeMap<Entity, Result<ProgramGraph, String>> = BTreeMap::new();
        let scene = self.scene.then(|| SceneKeywords::read(world));
        if scene.as_ref().is_some_and(|s| !s.known()) {
            log.approximate(
                "keywords=scene on a scene whose render pipeline has no rules here (not the built-in pipeline, or a Universal one whose asset was not read): the scene gives no keywords",
                1,
            );
        }
        if scene.as_ref().is_some_and(SceneKeywords::known) {
            log.approximate(
                "draw keywords the scene's state cannot give (lightmaps, vertex lights, GPU instancing) taken as absent",
                1,
            );
        }
        let receiving = receivers(world);
        for (material, m, programs) in materials {
            let mut global = self.keywords.clone();
            if let Some(scene) = &scene {
                // a material drawn by renderers that both receive shadows and do not takes the commoner state
                let (yes, no) = receiving
                    .get(&(m.material.file.clone(), m.material.path_id))
                    .copied()
                    .unwrap_or((1, 0));
                if yes > 0 && no > 0 {
                    log.approximate(
                        "materials drawn by renderers that both receive shadows and do not: bound as the commoner state",
                        1,
                    );
                }
                for k in scene.for_draw(yes >= no) {
                    *counts
                        .entry(format!("draws given a keyword the scene's state gives: {k}"))
                        .or_default() += 1;
                    global.push(k.to_owned());
                }
            }
            let chosen = programs.draw_variants(&m.keywords, &global, self.tier);
            let Some(fragment) = chosen.fragment else {
                *counts
                    .entry("materials left unbound: no fragment variant for these keywords and tier".into())
                    .or_default() += 1;
                continue;
            };
            for keyword in &chosen.implied {
                *counts
                    .entry(format!(
                        "draws given a keyword the build implies, not the material or the renderer: {keyword} (every stored variant of a stage in the tier has it)"
                    ))
                    .or_default() += 1;
            }
            let mut bound = Vec::new();
            for (stage, variant, lists) in [
                (Stage::Vertex, chosen.vertex, !programs.vertex.is_empty()),
                (Stage::Pixel, Some(fragment), true),
            ] {
                let Some(variant) = variant else {
                    if lists {
                        *counts
                            .entry("stages left unbound: no variant for these keywords and tier".into())
                            .or_default() += 1;
                    }
                    continue;
                };
                if !shaders.contains_key(&variant.code) {
                    let made = match primase_dxbc::lift(&variant.code) {
                        Err(e) => Err(format!("a program the lifter refuses: {}", e.group())),
                        Ok(program) => {
                            let e = world.spawn();
                            let graph = program.graph();
                            if let Err(why) = &graph {
                                *counts
                                    .entry(format!(
                                        "programs whose constant reads could not be taken ({why}): every byte of \
                                         their buffers bound"
                                    ))
                                    .or_default() += 1;
                            }
                            graph_of.insert(e, graph);
                            world.insert(e, Name(format!("{}-{:?}", programs.shader, stage)));
                            world.insert(
                                e,
                                Shader {
                                    format: ShaderFormat::Dxbc,
                                    bytecode: variant.code.to_vec(),
                                    interface: program.interface,
                                },
                            );
                            Ok(e)
                        }
                    };
                    shaders.insert(variant.code.clone(), made);
                }
                let made = shaders[&variant.code].clone();
                match made {
                    Ok(shader) => {
                        let interface = world
                            .get::<Shader>(shader)
                            .map(|s| s.interface.clone())
                            .ok_or("a resolved shader without its shader")?;
                        bound.push(self.stage_binding(
                            &m,
                            variant,
                            &programs.texture_defaults,
                            stage,
                            shader,
                            &interface,
                            match graph_of.get(&shader) {
                                Some(Ok(graph)) => Ok(graph),
                                Some(Err(why)) => Err(why.as_str()),
                                None => Err("the program was not lifted"),
                            },
                            chosen.vertex.is_some(),
                            &mut counts,
                        ));
                    }
                    Err(why) if stage == Stage::Pixel => {
                        *counts
                            .entry(format!("materials left unbound: {why}"))
                            .or_default() += 1;
                        bound.clear();
                        break;
                    }
                    Err(why) => {
                        *counts
                            .entry(format!("vertex stages left unbound: {why}"))
                            .or_default() += 1;
                    }
                }
            }
            if bound.iter().all(|s: &StageBinding| s.stage != Stage::Pixel) {
                continue;
            }
            // textures: bound per stage binding by name, entities checked against the world
            for stage in &mut bound {
                for t in &mut stage.textures {
                    if let TextureSource::Texture { texture, .. } = &t.source
                        && !(world.has::<Texture>(*texture)
                            || world.has::<primase_core::TextureFile>(*texture))
                    {
                        t.source = TextureSource::Missing;
                        *counts
                            .entry("texture bindings of an entity with no texture, left Missing".into())
                            .or_default() += 1;
                    }
                }
            }
            let (state, unmapped) = render_state(&m, &programs);
            if unmapped {
                *counts
                    .entry("passes with a render-state value the vocabulary has none for (SrcAlphaSaturate, a logical operation, a mask that is not a byte): that part unstated".into())
                    .or_default() += 1;
            }
            world.insert(material, ShaderBinding { stages: bound, state });
            *counts.entry("materials bound".into()).or_default() += 1;
        }
        for (what, n) in counts {
            if n > 0 {
                log.count(&what, n);
            }
        }
        log.count(
            "shader programs",
            shaders.values().filter(|r| r.is_ok()).count() as u64,
        );
        Ok(())
    }
}

impl ResolveUnityShaders {
    #[allow(clippy::too_many_arguments)]
    fn stage_binding(
        &self,
        m: &UnityMaterial,
        variant: &UnityProgramVariant,
        defaults: &[(String, String)],
        stage: Stage,
        shader: Entity,
        interface: &Interface,
        graph: Result<&ProgramGraph, &str>,
        vertex_bound: bool,
        counts: &mut BTreeMap<String, u64>,
    ) -> StageBinding {
        let reads = graph.ok().map(|g| &g.reads);
        let inputs = interface
            .inputs
            .iter()
            .filter(|v| v.system.is_none())
            .map(|v| InputBinding {
                label: v.label.clone(),
                role: match stage {
                    Stage::Vertex => semantic(&v.label.name, v.label.index).map_or_else(
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
        let mut constants = Vec::new();
        let mut textures = Vec::new();
        let parameters = &variant.parameters;
        for r in &interface.resources {
            let Some(b) = r.provenance.d3d else { continue };
            match (&r.kind, b.class) {
                (primase_core::shader::ResourceKind::ConstantBuffer { layout }, D3dClass::ConstantBuffer) => {
                    constants.extend(buffer_constants(
                        parameters,
                        b.register,
                        r.id,
                        layout.size,
                        m,
                        self.linear,
                        reads,
                        counts,
                    ));
                }
                (primase_core::shader::ResourceKind::Texture { .. }, D3dClass::ShaderResource) => {
                    let named = parameters.textures.iter().find(|t| t.slot == b.register);
                    let source = match named {
                        None => {
                            *counts
                                .entry(
                                    "texture slots the parameters do not name (the engine's), left Missing"
                                        .into(),
                                )
                                .or_default() += 1;
                            TextureSource::Missing
                        }
                        Some(t) => {
                            resolve_texture(m, defaults, &variant.keywords, self.linear, &t.name, counts)
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
        let unshown = state_lines(graph, interface, &mut constants);
        if unshown > 0 {
            *counts
                .entry("matrices whose lines the program does not show (a consumer writing a non-symmetric matrix cannot bind them)".into())
                .or_default() += unshown as u64;
        }
        let outputs = if stage == Stage::Pixel {
            output_bindings(interface)
        } else {
            Vec::new()
        };
        StageBinding {
            stage,
            variant: None,
            shader,
            inputs,
            constants,
            textures,
            buffers: Vec::new(),
            constant_buffers: Vec::new(),
            outputs,
        }
    }
}

/// The roles of a pixel program's colour outputs: target 0 is the colour, any other target Unknown with why. A
/// lifted program states a target as the system value `SV_Target<n>`, a program described by semantic as the label's
/// index; any other system value is not a colour output.
fn output_bindings(interface: &Interface) -> Vec<OutputBinding> {
    interface
        .outputs
        .iter()
        .filter_map(|v| {
            let target = match v.system {
                None => v.label.index,
                Some(SystemValue::Target(n)) => n,
                Some(_) => return None,
            };
            Some(OutputBinding {
                label: v.label.clone(),
                lanes: ((1u16 << v.value.width) - 1) as u8,
                role: if target == 0 {
                    OutputRole::Colour
                } else {
                    OutputRole::unknown(format!(
                        "target {target} of a forward pass: what the render pipeline binds past the first target, and what reads it, is the project's and not read"
                    ))
                },
            })
        })
        .collect()
}

/// The image of the frame a texture parameter of Unity's own name is, by what it holds (`keywords` are the fragment
/// variant's: `_ShadowMapTexture` is the main light's screen-space shadow term under `SHADOWS_SCREEN`, else a light's
/// shadow depth map).
pub fn frame_texture(name: &str, keywords: &[String]) -> Option<FrameTexture> {
    Some(match name {
        "_CameraDepthTexture" => FrameTexture::SceneDepth,
        "_CameraOpaqueTexture" | "_GrabTexture" | "_GrabScreen0" => FrameTexture::SceneColour,
        "_MainLightShadowmapTexture" => FrameTexture::ShadowMap,
        "_ShadowMapTexture" if keywords.iter().any(|k| k == "SHADOWS_SCREEN") => {
            FrameTexture::ScreenSpaceShadows
        }
        "_ShadowMapTexture" => FrameTexture::ShadowMap,
        "_ScreenSpaceOcclusionTexture" => FrameTexture::AmbientOcclusion,
        "unity_SpecCube0" | "_GlossyEnvironmentCubeMap" => FrameTexture::ReflectionProbe,
        "unity_ProbeVolumeSH" => FrameTexture::LightProbeVolume,
        _ => return None,
    })
}

/// Why an engine texture of Unity's has no frame texture in the vocabulary, for the ones whose meaning is known.
fn engine_texture_reason(name: &str) -> Option<&'static str> {
    Some(match name {
        "_AdditionalLightsShadowmapTexture" => {
            "an atlas of the shadow maps of the additional lights: which lights it holds is not stated"
        }
        "unity_SpecCube1" => {
            "the second-nearest reflection probe, blended with the first by a weight in the engine's constants: no role for it"
        }
        "urp_ReflProbes_Atlas" => {
            "an atlas of the scene's reflection probes, picked per surface by the cluster: no role"
        }
        _ => return None,
    })
}

/// What a texture parameter named `name` samples: the property's texture, else the shader's default; the engine's
/// own textures ([`frame_texture`]) are the frame's.
fn resolve_texture(
    m: &UnityMaterial,
    defaults: &[(String, String)],
    keywords: &[String],
    linear: bool,
    name: &str,
    counts: &mut BTreeMap<String, u64>,
) -> TextureSource {
    let mut bump = |what: String| *counts.entry(what).or_default() += 1;
    let Some(UnityTexEnv {
        referenced, texture, ..
    }) = m.textures.iter().find(|t| t.name == name)
    else {
        if let Some(frame) = frame_texture(name, keywords) {
            bump(format!(
                "texture parameters that are the frame's {frame:?}: {name}"
            ));
            return TextureSource::Frame(frame);
        }
        bump(match engine_texture_reason(name) {
            Some(why) => {
                format!("texture parameters the material has no property of, left Missing: {name} ({why})")
            }
            None => format!("texture parameters the material has no property of, left Missing: {name}"),
        });
        return TextureSource::Missing;
    };
    match (referenced, texture) {
        (true, Some(texture)) => {
            bump("texture parameters bound to the property's texture".into());
            TextureSource::Texture {
                texture: *texture,
                sampler: SamplerState::default(),
            }
        }
        (true, None) => {
            bump("texture properties naming a texture the world does not hold, left Missing".into());
            TextureSource::Missing
        }
        (false, _) => match defaults.iter().find(|(p, _)| p == name) {
            Some((_, d)) => match default_colour(d, linear) {
                Some(value) => {
                    let named = if is_fallback(d) {
                        format!("{d:?}, no default name: the engine's fallback")
                    } else {
                        d.clone()
                    };
                    bump(format!(
                        "unset texture properties, bound the shader's default ({named})"
                    ));
                    TextureSource::Default {
                        value,
                        why: format!("unset; the shader's declared default is \"{d}\""),
                    }
                }
                None => {
                    bump(format!(
                        "unset texture properties whose shader default \"{d}\" is not valued, left Missing"
                    ));
                    TextureSource::Missing
                }
            },
            None => {
                bump("unset texture properties whose shader declares no default, left Missing".into());
                TextureSource::Missing
            }
        },
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
    use crate::components::{ConstantBuffer, Field, StructField, TextureParam, UnityObject};
    use crate::shader::fixture;
    use primase_core::shader::ResourceId;

    fn material() -> UnityMaterial {
        UnityMaterial {
            material: UnityObject {
                file: "level1".into(),
                path_id: 1,
            },
            shader: Some("S".into()),
            shader_state: Vec::new(),
            textures: vec![
                UnityTexEnv {
                    name: "_MainTex".into(),
                    referenced: true,
                    texture: None,
                    scale: [2.0, 3.0],
                    offset: [0.5, 0.25],
                },
                UnityTexEnv {
                    name: "_BumpMap".into(),
                    referenced: false,
                    texture: None,
                    scale: [1.0; 2],
                    offset: [0.0; 2],
                },
            ],
            floats: vec![("_Cutoff".into(), 0.25)],
            ints: vec![("_Mode".into(), 2)],
            colors: vec![("_BaseColor".into(), [0.5, 1.0, 0.0, 0.75])],
            keywords: Vec::new(),
            tags: Vec::new(),
            render_queue: -1,
        }
    }

    fn field(name: &str, offset: u32, shape: Shape) -> Field {
        Field {
            name: name.into(),
            offset,
            kind: 0,
            shape,
            array_size: 0,
        }
    }

    /// A buffer at register 1 of 192 bytes: a colour, a float, a matrix, a tiling, an unknown vector and a struct
    /// array of one 16-byte element, with bytes left over at 20 to 32, 112 to 128 and 160 to 192.
    fn parameters() -> Parameters {
        Parameters {
            constant_buffers: vec![ConstantBuffer {
                name: "UnityPerMaterial".into(),
                slot: Some(1),
                size: 192,
                fields: vec![
                    field("_BaseColor", 0, Shape::Vector { columns: 4 }),
                    field("_Cutoff", 16, Shape::Vector { columns: 1 }),
                    field("unity_ObjectToWorld", 32, Shape::Matrix { rows: 4 }),
                    field("_MainTex_ST", 96, Shape::Vector { columns: 4 }),
                    field("_Mystery", 128, Shape::Vector { columns: 4 }),
                ],
                structs: vec![StructField {
                    name: "Props".into(),
                    offset: 144,
                    array_size: 1,
                    size: 16,
                    members: Vec::new(),
                }],
            }],
            textures: vec![TextureParam {
                name: "_MainTex".into(),
                slot: 0,
                sampler: None,
                dim: 2,
            }],
            samplers: Vec::new(),
        }
    }

    #[test]
    fn fields_take_the_materials_properties_builtins_or_unknown_and_every_byte_has_a_role() {
        let mut counts = BTreeMap::new();
        let c = buffer_constants(
            &parameters(),
            1,
            ResourceId(7),
            192,
            &material(),
            true,
            None,
            &mut counts,
        );
        let at = |offset: u32| c.iter().find(|c| c.offset == offset).unwrap();
        // a colour, gamma-encoded as stored, set linear (alpha unchanged): 0.5 -> 0.21404114, 1 -> 1, 0 -> 0
        let ConstantRole::Material(w) = &at(0).role else {
            panic!("{:?}", at(0))
        };
        let v: Vec<f32> = w.iter().map(|x| f32::from_bits(*x)).collect();
        assert!(
            (v[0] - 0.214_041_14).abs() < 1e-6 && v[1] == 1.0 && v[2] == 0.0 && v[3] == 0.75,
            "{v:?}"
        );
        assert_eq!(at(16).role, ConstantRole::Material(vec![0.25f32.to_bits()]));
        assert_eq!(at(32).role, ConstantRole::Object(ObjectParameter::World));
        assert_eq!(at(32).size, 64);
        assert_eq!(at(96).role, ConstantRole::Material(words(&[2.0, 3.0, 0.5, 0.25])));
        assert!(
            matches!(at(128).role, ConstantRole::Unknown { .. })
                && at(128).name.as_deref() == Some("_Mystery")
        );
        assert_eq!((at(144).name.as_deref(), at(144).size), (Some("Props"), 16));
        // no byte twice, none left out
        let mut covered = [false; 192];
        for b in &c {
            for byte in b.offset..b.offset + b.size {
                assert!(!covered[byte as usize], "byte {byte} twice");
                covered[byte as usize] = true;
            }
        }
        assert!(covered.iter().all(|&x| x));
        // as stored, not converted, in a gamma player
        let g = buffer_constants(
            &parameters(),
            1,
            ResourceId(7),
            192,
            &material(),
            false,
            None,
            &mut BTreeMap::new(),
        );
        assert_eq!(
            g.iter().find(|c| c.offset == 0).unwrap().role,
            ConstantRole::Material(words(&[0.5, 1.0, 0.0, 0.75]))
        );
        // a buffer the parameters do not name is one Unknown
        let other = buffer_constants(
            &parameters(),
            5,
            ResourceId(8),
            32,
            &material(),
            true,
            None,
            &mut BTreeMap::new(),
        );
        assert_eq!(other.len(), 1);
        assert_eq!((other[0].offset, other[0].size), (0, 32));
    }

    #[test]
    fn the_builtin_table_names_the_matrices_and_the_camera() {
        assert_eq!(
            builtin("unity_MatrixVP"),
            Some(ConstantRole::View(ViewParameter::ViewProjection))
        );
        assert_eq!(
            builtin("_WorldSpaceCameraPos"),
            Some(ConstantRole::View(ViewParameter::CameraPosition))
        );
        assert_eq!(builtin("_Time"), None);
        // the camera's world matrix is the inverse of the view matrix
        assert_eq!(
            builtin("unity_CameraToWorld"),
            Some(ConstantRole::View(ViewParameter::InverseView))
        );
        assert_eq!(
            builtin("unity_MatrixInvVP"),
            Some(ConstantRole::View(ViewParameter::InverseViewProjection))
        );
    }

    #[test]
    fn the_packed_builtin_vectors_bind_each_value_by_its_lanes() {
        let role = |name: &str, at: u32| {
            builtin_parts(name)
                .unwrap()
                .into_iter()
                .find(|(a, _, _)| *a == at)
                .unwrap()
                .2
        };
        // `_ProjectionParams` is (flip, near, far, 1/far)
        assert_eq!(
            role("_ProjectionParams", 0),
            ConstantRole::View(ViewParameter::ProjectionFlip)
        );
        assert_eq!(
            role("_ProjectionParams", 4),
            ConstantRole::View(ViewParameter::NearPlane)
        );
        assert_eq!(
            role("_ProjectionParams", 8),
            ConstantRole::View(ViewParameter::FarPlane)
        );
        assert_eq!(
            role("_ProjectionParams", 12),
            ConstantRole::View(ViewParameter::InverseFarPlane)
        );
        // `_ScreenParams` is (width, height, 1 + 1/width, 1 + 1/height): the size is the role, the rest is not
        assert_eq!(
            role("_ScreenParams", 0),
            ConstantRole::View(ViewParameter::ViewportSize)
        );
        assert!(
            matches!(role("_ScreenParams", 8), ConstantRole::Unknown { why } if why.contains("1 + 1/width"))
        );
        // the time is `_Time.y` and `_TimeParameters.x`; the lanes derived from it are not roles
        assert_eq!(role("_Time", 4), ConstantRole::Frame(FrameParameter::Time));
        assert!(matches!(role("_Time", 0), ConstantRole::Unknown { .. }));
        assert_eq!(
            role("_TimeParameters", 0),
            ConstantRole::Frame(FrameParameter::Time)
        );
        assert!(builtin_parts("_WorldSpaceCameraPos").is_none());
        // a light's position points toward it: stated, not mapped onto the light's travel direction
        assert!(builtin_reason("_WorldSpaceLightPos0").is_some_and(|r| r.contains("toward the light")));
    }

    /// The constants of `modes.dxbc`, a pixel program of one buffer at register 0 (`f` row 0, `u` row 1, `i` row 2) that
    /// reads every lane of rows 0 and 1 and the x and y lanes of row 2.
    #[test]
    fn only_the_bytes_the_program_reads_are_bound() {
        let program = primase_dxbc::lift(include_bytes!(
            "../../../../toolkit/dxbc/tests/fixtures/modes.dxbc"
        ))
        .unwrap();
        let reads = program.constant_reads().unwrap();
        let parameters = Parameters {
            constant_buffers: vec![ConstantBuffer {
                name: "UnityPerCamera".into(),
                slot: Some(0),
                size: 64,
                fields: vec![
                    field("_ProjectionParams", 0, Shape::Vector { columns: 4 }),
                    field("_Time", 32, Shape::Vector { columns: 4 }),
                    field("_Never", 48, Shape::Vector { columns: 4 }),
                ],
                structs: Vec::new(),
            }],
            textures: Vec::new(),
            samplers: Vec::new(),
        };
        let mut counts = BTreeMap::new();
        let c = buffer_constants(
            &parameters,
            0,
            ResourceId(1),
            64,
            &material(),
            true,
            Some(&reads),
            &mut counts,
        );
        let at: Vec<(u32, u32, Option<&str>)> =
            c.iter().map(|c| (c.offset, c.size, c.name.as_deref())).collect();
        assert_eq!(
            at,
            [
                // all four values of the vector: each lane is read
                (0, 4, Some("_ProjectionParams")),
                (4, 4, Some("_ProjectionParams")),
                (8, 4, Some("_ProjectionParams")),
                (12, 4, Some("_ProjectionParams")),
                // the row no field covers, read whole
                (16, 16, None),
                // `_Time` x and y are read, z and w are not
                (32, 4, Some("_Time")),
                (36, 4, Some("_Time")),
            ]
        );
        assert_eq!(c[1].role, ConstantRole::View(ViewParameter::NearPlane));
        assert!(matches!(&c[4].role, ConstantRole::Unknown { why } if why.contains("reads these bytes")));
        assert_eq!(c[6].role, ConstantRole::Frame(FrameParameter::Time));
        // `_Time.z`, `_Time.w` and `_Never` are the three left out
        assert_eq!(
            counts["constants and runs of bytes the program does not read, left out"],
            3
        );
        // a program whose reads are not known keeps every byte
        let all = buffer_constants(
            &parameters,
            0,
            ResourceId(1),
            64,
            &material(),
            true,
            None,
            &mut BTreeMap::new(),
        );
        assert_eq!(all.iter().map(|c| c.size).sum::<u32>(), 64);
    }

    #[test]
    fn a_texture_parameter_samples_the_property_else_the_shaders_default() {
        let m = material();
        let defaults = vec![
            ("_BumpMap".to_owned(), "bump".to_owned()),
            ("_Other".to_owned(), "white".to_owned()),
        ];
        let mut counts = BTreeMap::new();
        // referenced, but the world holds no texture for it
        assert_eq!(
            resolve_texture(&m, &defaults, &[], true, "_MainTex", &mut counts),
            TextureSource::Missing
        );
        // unset, with a default this pass does not value
        assert_eq!(
            resolve_texture(&m, &defaults, &[], true, "_BumpMap", &mut counts),
            TextureSource::Missing
        );
        assert!(counts.keys().any(|k| k.contains("\"bump\" is not valued")));
        // a property the material does not have
        assert_eq!(
            resolve_texture(&m, &defaults, &[], true, "_Other", &mut counts),
            TextureSource::Missing
        );
        let mut unset = material();
        unset.textures.push(UnityTexEnv {
            name: "_Other".into(),
            referenced: false,
            texture: None,
            scale: [1.0; 2],
            offset: [0.0; 2],
        });
        assert!(matches!(
            resolve_texture(&unset, &defaults, &[], true, "_Other", &mut counts),
            TextureSource::Default { value, .. } if value == [1.0; 4]
        ));
    }

    #[test]
    fn a_shaders_default_texture_is_the_oracles_colour_and_only_the_certain_ones_in_a_linear_project() {
        let grey = 127.0 / 255.0;
        // gamma: every default the oracle observed
        assert_eq!(default_colour("white", false), Some([1.0; 4]));
        assert_eq!(default_colour("black", false), Some([0.0; 4]));
        assert_eq!(default_colour("red", false), Some([1.0, 0.0, 0.0, 0.0]));
        for name in ["gray", "grey", "linearGray", "linearGrey"] {
            assert_eq!(default_colour(name, false), Some([grey; 4]), "{name}");
        }
        assert_eq!(default_colour("bump", false), Some([grey, grey, 1.0, 1.0]));
        // linear: only the colours of 0 and 1 channels
        assert_eq!(default_colour("white", true), Some([1.0; 4]));
        assert_eq!(default_colour("black", true), Some([0.0; 4]));
        assert_eq!(default_colour("gray", true), None);
        assert_eq!(default_colour("bump", true), None);
        // an empty name or one that is no default: the engine's fallback, in a gamma project; unobserved in a linear one
        let fallback = 128.0 / 255.0;
        assert_eq!(default_colour("", false), Some([fallback; 4]));
        assert_eq!(default_colour("notadefault", false), Some([fallback; 4]));
        assert_eq!(default_colour("", true), None);
        assert_eq!(default_colour("notadefault", true), None);
        // an unset property takes the default, counted by its name
        let mut counts = BTreeMap::new();
        let defaults = vec![("_Other".to_owned(), "black".to_owned())];
        let mut unset = material();
        unset.textures.push(UnityTexEnv {
            name: "_Other".into(),
            referenced: false,
            texture: None,
            scale: [1.0; 2],
            offset: [0.0; 2],
        });
        assert!(matches!(
            resolve_texture(&unset, &defaults, &[], false, "_Other", &mut counts),
            TextureSource::Default { value, .. } if value == [0.0; 4]
        ));
        assert_eq!(
            counts["unset texture properties, bound the shader's default (black)"],
            1
        );
    }

    #[test]
    fn the_engines_own_textures_are_the_frames() {
        let screen = vec!["DIRECTIONAL".to_owned(), "SHADOWS_SCREEN".to_owned()];
        let none: Vec<String> = Vec::new();
        assert_eq!(
            frame_texture("_CameraDepthTexture", &none),
            Some(FrameTexture::SceneDepth)
        );
        // the same name holds a screen-space term under SHADOWS_SCREEN and a light's depth map otherwise
        assert_eq!(
            frame_texture("_ShadowMapTexture", &screen),
            Some(FrameTexture::ScreenSpaceShadows)
        );
        assert_eq!(
            frame_texture("_ShadowMapTexture", &none),
            Some(FrameTexture::ShadowMap)
        );
        assert_eq!(
            frame_texture("unity_SpecCube0", &none),
            Some(FrameTexture::ReflectionProbe)
        );
        assert_eq!(frame_texture("unity_SpecCube1", &none), None);
        assert!(engine_texture_reason("unity_SpecCube1").is_some());
        // a property the material has wins over the engine's name; one it has not is the frame's, counted
        let mut counts = BTreeMap::new();
        assert_eq!(
            resolve_texture(&material(), &[], &none, true, "_CameraDepthTexture", &mut counts),
            TextureSource::Frame(FrameTexture::SceneDepth)
        );
        assert_eq!(
            counts["texture parameters that are the frame's SceneDepth: _CameraDepthTexture"],
            1
        );
    }

    #[test]
    fn state_is_the_passes_blend_and_cull() {
        let mut m = material();
        let programs = |subshader, pass| UnityPrograms {
            shader: "S".into(),
            subshader,
            pass,
            texture_defaults: Vec::new(),
            variants: Vec::new(),
            vertex: Vec::new(),
            geometry: Vec::new(),
            hull: Vec::new(),
            domain: Vec::new(),
        };
        // alpha blending, culling back faces
        m.shader_state = vec![fixture::sub(&[], vec![fixture::own(None, 5.0, 10.0)])];
        let (s, unmapped) = render_state(&m, &programs(0, 0));
        let eq = BlendEquation {
            source: BlendFactor::SrcAlpha,
            destination: BlendFactor::OneMinusSrcAlpha,
            op: BlendOp::Add,
        };
        assert_eq!(
            (s.blend, s.cull, unmapped),
            (
                Some(Blend {
                    colour: eq,
                    alpha: eq
                }),
                Some(Cull::Back),
                false
            )
        );
        // One Zero is no blending; SrcAlphaSaturate is unstated and counted
        m.shader_state = vec![fixture::sub(
            &[],
            vec![fixture::own(None, 1.0, 0.0), fixture::own(None, 9.0, 0.0)],
        )];
        assert_eq!(render_state(&m, &programs(0, 0)).0.blend, None);
        let (s, unmapped) = render_state(&m, &programs(0, 1));
        assert_eq!((s.blend, unmapped), (None, true));
        assert_eq!(render_state(&m, &programs(3, 0)).0, RenderState::default());
    }

    /// The rest of the pass's stored state: the depth test and write, the offset, the stencil with its faces, each
    /// target's blend and mask, and the property a value is bound to.
    #[test]
    fn state_holds_the_depth_the_offset_the_stencil_and_every_target() {
        let mut m = material();
        let programs = UnityPrograms {
            shader: "S".into(),
            subshader: 0,
            pass: 0,
            texture_defaults: Vec::new(),
            variants: Vec::new(),
            vertex: Vec::new(),
            geometry: Vec::new(),
            hull: Vec::new(),
            domain: Vec::new(),
        };
        let v = |x| fixture::value(x, None);
        let mut pass = fixture::own(None, 5.0, 10.0);
        pass.zwrite = v(0.0);
        pass.state.z_test = v(8.0);
        pass.state.offset_factor = v(-1.0);
        pass.state.offset_units = v(-2.0);
        // target 0 masks the alpha away; the others keep the default
        pass.state.targets[0].mask = v(7.0);
        pass.state.stencil_ref = v(2.0);
        pass.state.stencil_read_mask = v(15.0);
        // Comp Equal, Pass Replace: both faces; CompBack Never: the back face's own
        pass.state.stencil_op.comp = v(3.0);
        pass.state.stencil_op.pass = v(2.0);
        pass.state.stencil_op_back.comp = v(1.0);
        m.shader_state = vec![fixture::sub(&[], vec![pass])];
        let (s, unmapped) = render_state(&m, &programs);
        assert!(!unmapped);
        let eq = BlendEquation {
            source: BlendFactor::SrcAlpha,
            destination: BlendFactor::OneMinusSrcAlpha,
            op: BlendOp::Add,
        };
        let blend = Blend {
            colour: eq,
            alpha: eq,
        };
        assert_eq!(
            s.depth,
            Some(DepthState {
                test: Compare::Always,
                write: false
            })
        );
        assert_eq!(
            s.depth_bias,
            Some(DepthBias {
                constant: -2.0,
                slope_scaled: -1.0,
                clamp: 0.0
            })
        );
        // every target takes target 0's blend (the pass does not blend separately); the masks are each its own
        assert_eq!(s.blend, Some(blend));
        assert!(
            s.targets
                .iter()
                .all(|t| t.is_some_and(|t| t.blend == Some(blend)))
        );
        assert_eq!(s.targets[0].map(|t| t.write_mask), Some(7));
        assert_eq!(s.targets[1].map(|t| t.write_mask), Some(15));
        let stencil = s.stencil.unwrap();
        assert_eq!(
            (stencil.read_mask, stencil.write_mask, stencil.reference),
            (15, 255, Some(2))
        );
        assert_eq!(
            stencil.front,
            StencilFace {
                fail: StencilOp::Keep,
                depth_fail: StencilOp::Keep,
                pass: StencilOp::Replace,
                compare: Compare::Equal
            }
        );
        assert_eq!(stencil.back.compare, Compare::Never);
        // a disabled comparison and a pass that always passes and keeps state no stencil
        let mut plain = fixture::own(None, 1.0, 0.0);
        plain.state.stencil_op.comp = v(0.0);
        m.shader_state = vec![fixture::sub(&[], vec![plain, fixture::own(None, 1.0, 0.0)])];
        assert_eq!(
            render_state(
                &m,
                &UnityPrograms {
                    pass: 0,
                    ..programs.clone()
                }
            )
            .0
            .stencil,
            None
        );
        assert_eq!(
            render_state(
                &m,
                &UnityPrograms {
                    pass: 1,
                    ..programs.clone()
                }
            )
            .0
            .stencil,
            None
        );
        // separate blends: each target its own
        let mut separate = fixture::own(None, 1.0, 0.0);
        separate.state.separate_blend = true;
        separate.state.targets[2].src = v(1.0);
        separate.state.targets[2].dst = v(1.0);
        separate.state.targets[2].src_alpha = v(1.0);
        separate.state.targets[2].dst_alpha = v(1.0);
        m.shader_state = vec![fixture::sub(&[], vec![separate])];
        let s = render_state(&m, &programs).0;
        assert_eq!(s.targets[0].and_then(|t| t.blend), None);
        assert_eq!(
            s.targets[2].and_then(|t| t.blend).map(|b| b.colour.destination),
            Some(BlendFactor::One)
        );
        // a logical blend operation and a stencil operation outside the range are unmapped and counted
        let mut odd = fixture::own(None, 1.0, 0.0);
        odd.state.targets[0].op = v(7.0);
        m.shader_state = vec![fixture::sub(&[], vec![odd])];
        assert!(render_state(&m, &programs).1);
    }

    #[test]
    fn options_are_required_and_checked() {
        let o = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<primase_core::pass::Options>()
        };
        let make = |pairs: &[(&str, &str)]| RESOLVE_UNITY_SHADERS.instance(&o(pairs));
        assert!(make(&[("tier", "1")]).is_err());
        assert!(make(&[("tier", "x"), ("keywords", "none"), ("colour-space", "linear")]).is_err());
        let p = make(&[("tier", "1"), ("keywords", "A+B"), ("colour-space", "gamma")]).unwrap();
        let typed = ResolveUnityShaders::new(1, vec!["A".into(), "B".into()], false);
        assert_eq!(p.options(), primase_core::pass::Pass::options(&typed));
        assert_eq!(p.options()["keywords"], "A+B");
        let scene = make(&[("tier", "1"), ("keywords", "B+scene"), ("colour-space", "linear")]).unwrap();
        assert_eq!(scene.options()["keywords"], "scene+B");
    }

    /// The D3D behaviour fixture of the DXBC lifter (our own HLSL, `ps_5_0`): five colour targets, which a lifted
    /// program states as the system values `SV_Target0..4` (not as semantics).
    const LIFTED: &[u8] = include_bytes!("../../../../toolkit/dxbc/tests/fixtures/modes.dxbc");

    /// A lifted pixel program's outputs are system values, and each has its role all the same: target 0 the colour,
    /// the others Unknown with why; a depth output is none.
    #[test]
    fn a_lifted_pixel_programs_targets_have_roles() {
        let program = primase_dxbc::lift(LIFTED).unwrap();
        let roles: Vec<OutputRole> = output_bindings(&program.interface)
            .into_iter()
            .map(|o| o.role)
            .collect();
        assert_eq!(roles[0], OutputRole::Colour);
        assert_eq!(roles.len(), 5);
        for (n, role) in roles.iter().enumerate().skip(1) {
            let OutputRole::Unknown { why } = role else {
                panic!("target {n} is {role:?}");
            };
            assert!(why.starts_with(&format!("target {n} of a forward pass")));
        }
        let mut with_depth = program.interface.clone();
        with_depth.outputs[0].system = Some(SystemValue::Depth);
        assert_eq!(output_bindings(&with_depth).len(), 4);
    }
}
