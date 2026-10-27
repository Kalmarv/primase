//! `resolve-anvil-shaders{unset-textures=white|leave}`: the G-buffer programs of Anvil Materials as
//! `core.shader` entities and each Material's `core.shader_binding`, by the engine's rules (Riders Republic
//! docs/material-spec.md §2, §6.2, §11, §12.2, §15.4). The one home of those rules for the binding; the existing
//! `bake-anvil-material` still carries its own recipes until it is moved onto the binding.
//!
//! - **Program.** The Material's template permutation ([`AnvilShaders`], read with `Placer::with_shaders`) for the
//!   G-buffer pass of a plain draw: the pixel key [`gbuffer_key`] composes from the Material's flags (a
//!   Material that sets a flag the template's feature word gates is left unbound, naming it), with the discard variant
//!   (`0x4000`, [`alpha_tested_key`]) where the Material tests alpha and the template has it (inferred, counted: the
//!   program then discards by its own alpha test, `cb2[18].y` the Material's reference), and a vertex key for each
//!   vertex layout number (bits 17 to 21 of the key: the mesh's, not the Material's) of the meshes drawn with the
//!   Material, read from the data: the `kind` of every `anvil.mesh` that lists the Material among its ranges' Materials, or of
//!   every lowered mesh's primitive that draws it (its Material, or one of its variants'; `anvil.layout`), so the pass
//!   binds the same stages before `lower-anvil-mesh` and after it.
//!   A Material drawn with several layouts has a vertex stage for each, each a [`VariantKey`] (`anvil.layout.kind` = the
//!   layout) a consumer matches to the mesh it draws, refusing a mesh whose layout has no stage; a Material no mesh is
//!   drawn with has the pixel stage only (counted). Each program's interface is lifted from its DXBC (the containers are
//!   stripped of their resource definitions, so its resources are its declarations').
//! - **Constants.** `cb3` is the Material's property vector: each of the template's bindings of the Material's
//!   properties at the vec4 it states (a scalar its `.x`, a vector its four, a UV transform the two rows
//!   `(su cos t, -su sin t, tu, 0)` and `(sv sin t, sv cos t, tv, 0)` where the speeds are 0, as the words of the
//!   Material's property, else the template's default for the binding). A property neither sets, and a UV transform
//!   that moves (the time term is not established), is `Unknown` with the reason. Static rotation uses the same
//!   `S R uv + translation` rows as the material lowering (material-spec §12.2).
//!   The rows of `cb3` no binding covers are `Unknown`. The other constant buffers (`cb0` per frame, `cb1` per view,
//!   `cb2` per draw) are the engine's: the program is lifted and only the lanes it reads are bound
//!   ([`primase_shader::WordsRead`]: the words its outputs, discards and writes are made of; a program reads a
//!   few rows of buffers of some hundred; the rest are counted as left out, a buffer indexed by a computed value is one
//!   `Unknown` constant). The rows the notes establish have roles where the vocabulary has one: `cb1[37]`
//!   (`1/W, 1/H, W, H`) the `InverseViewportSize` in its lanes x, y and the `ViewportSize` in z, w, `cb1[39].x` the `Exposure`, `cb2[8..12]` and `cb2[12..16]` the object's `WorldViewProjection` and its `PreviousFrame` one,
//!   `cb2[18].y` the Material's alpha-test
//!   reference (`AlphaTestValue / 255` where `AlphaTestEnabled`, else 0); the rest are `Unknown` with the notes' reason
//!   where they state one.
//! - **Textures.** The template's texture binding of shader slot `i` is `t(75 + i)`: the texture the Material's
//!   binding resolves to (record, else TextureSet slot, else template default), as a `core.texture` entity per
//!   texture and colour space (sRGB where its TextureMap's field 25 says so). A binding resolved to none is
//!   `unset-textures`: `white` gives it white (counted as an approximation: the engine's own substitute is not read),
//!   `leave` binds it `Missing`. The engine's textures (`t < 75`, the exposure `t23`, the terrain's) are `Missing`.
//!   The sampler state is not read.
//! - **Outputs** (§3, §15.4), the G-buffer's render targets: `o0.rgb` is the albedo, `o0.w` the roughness (opaque
//!   Materials: for a blended one it is the alpha); `o1.xyz` the world normal, `OctahedralDiamond` (the lighting pass
//!   decodes the 9-bit diamond octahedral from the three lanes, its sign of z in lane 2's bit 0 and the shading class
//!   in the lane's upper five bits: `NormalEncoding::decode_octahedral_diamond`, material-spec §15.4; the class byte is
//!   not part of the normal); `o1.w` the metalness (shading class 1) or the occlusion
//!   (classes 3 to 5) by the class its template writes ([`shading_class`], the notes' table of the templates it covers;
//!   a template it does not cover leaves `o1.w` `Unknown`); `o2` and `o3` by the class the lighting pass reads them
//!   with (§15.4): the velocity `MotionVector` in `o2.xy`; class 1 the `Emission` in `o2.z` and in `o3` the `Occlusion`
//!   (x), the `SpecularOcclusion` (z) and the `Translucency` (w); classes 3 to 5 the `SpecularOcclusion` in `o2.z` and the
//!   `Translucency` in `o2.w`; every other lane, and every target of a template the table does not cover, `Unknown`
//!   with what the notes show of it.
//! - **State.** `TwoSided` is no culling; `ZTestDisabled` an always-passing depth test, with the write the Material's
//!   `ZWriteDisabledOpaque` (blend mode 0) or `ZWriteDisabledAlpha` states; `AlphaTestEnabled` an alpha test, a pixel
//!   kept where `alpha >= AlphaTestValue / 255` (the engine's division). What the Material does not state stays `None`.
//!   The template states none: a MaterialTemplate holds its programs, their keys and the parameters they read, and a
//!   pipeline-state database that is null (`PSODB`, docs/registry-named-fields.md §12: the `CompiledMaterialTemplate`
//!   field list read from the running image, null in the template examined; the shader code permutations carry a
//!   `LowestShaderConfig` and a hash, no blend, rasterizer or depth-stencil record). The engine composes the state per
//!   pass from the Material's flags (docs/shaders.md §6.2: "applied by one method that switches on six pass kinds"),
//!   which is the part mapped here. Not stated, so `None`: the blend (the notes infer source alpha over inverse source
//!   alpha for the deferred decals and do not establish it), the fill and the winding. A census of the 432 template
//!   bodies of the six forges (outside the code frames) holds this: between the last code frame and the first
//!   permutation record there are 0 bytes in 432 of 432 (no pipeline-state object), the 59 bytes after the last record
//!   are the same length in 432 of 432 (the two colours and the category), and the 135 distinct registry classes found
//!   in the heads are all material-graph nodes, lighting and shade-constant sources and the compiled-template
//!   structure; none is a pipeline-state class (`SerializedPSODB`, `PSOEntry*`, `PSOBlob`: 0 of 432), a blend,
//!   rasterizer or depth-stencil one (`OperatorBlendFloats` and its kin blend values in the graph). The graph's own
//!   bytes are not parsed whole, so this is a census of classes and gaps, not an account of every byte.
//!
//! Inputs of the pixel stage are the vertex stage's outputs by label (`Previous`) where it is bound, else `Unknown`;
//! the vertex stage's are mesh streams by their semantic name.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::shader::{
    AlphaTest, Compare, ConstantBinding, ConstantRole, Cull, DepthState, FrameParameter, InputBinding,
    InputRole, Interface, NormalEncoding, NormalSpace, ObjectMatrix, ObjectParameter, Occluded,
    OutputBinding, OutputRole, RenderState, ResourceId, ResourceKind, SamplerState, ScalarEncoding, Shader,
    ShaderBinding, ShaderFormat, Stage, StageBinding, SystemValue, TextureBinding, TextureDim, TextureSource,
    VariantKey, Varying, ViewParameter,
};
use primase_core::shader::{MaterialPrograms, ProgramFacts};
use primase_core::{ColorSpace, Component, Entity, MaterialVariants, Mesh, Name, Semantic, Texture, World};
use primase_shader::matrix::state_lines;
use primase_shader::{ProgramGraph, WordsRead};

use crate::components::{
    AnvilLayout, AnvilMaterial, AnvilMesh, AnvilShaders, MaterialBinding, MaterialFlags, MaterialValue,
};

/// What a texture binding with no texture becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsetTexture {
    /// The slot reads white, counted as an approximation.
    White,
    /// The slot is `Missing`.
    Leave,
}

impl Choice for UnsetTexture {
    const VALUES: &'static [(&'static str, Self)] =
        &[("white", UnsetTexture::White), ("leave", UnsetTexture::Leave)];
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolveAnvilShaders {
    unset: UnsetTexture,
}

impl ResolveAnvilShaders {
    pub fn new(unset: UnsetTexture) -> Self {
        ResolveAnvilShaders { unset }
    }
}

/// `resolve-anvil-shaders`' declaration ([`primase_core::pass::PassRegistry::add`]): what a texture binding with no
/// texture reads is the program's. (The vertex layout is no option: each mesh's own decides, `anvil.layout.kind`.)
pub const RESOLVE_ANVIL_SHADERS: PassSpec = PassSpec {
    name: "resolve-anvil-shaders",
    options: &[OptionSpec::choice::<UnsetTexture>(
        "unset-textures",
        "a texture binding with no texture reads white (counted), or is left missing",
    )],
    make: |g| Ok(Box::new(ResolveAnvilShaders::new(g.choice("unset-textures")))),
};

/// The G-buffer pass's number, in the key's bits 22 to 29.
const GBUFFER: u32 = 1;

/// The pixel ShaderKey the engine's key composer gives the G-buffer pass of a plain draw of a Material with the flags
/// `f` of a template with the features `tables` (docs/shaders.md §6.2, read from the engine): the pass in bits 22 to
/// 29, then each Material flag sets its bit only where the template's feature word allows it:
/// - `0x40`: `TwoSided` and `FlipNormalOnBackFace` both set, and feature 12;
/// - `0x2`: `AlphaDisplayMode` 2, and feature 13;
/// - `0x4`: `LayerMaterial`, and feature 10; `0x20` also with a blend mode other than 0 (only with `0x4`);
/// - `0x10`: boolean 34 (`42452A12`), and features 3 and 4 (`T & 0x18 == 0x18`).
///
/// The draw's bits (vertex pulling, reduced material levels, the mesh's per-instance features, the eight variant
/// subsets) are those of a plain draw: none. A template whose feature bytes are not 0 or 1 is refused. (The
/// `bake-anvil-material` pass has its own [`super::gbuffer_pixel_key`], which refuses the gated flags instead.)
pub fn gbuffer_key(f: &MaterialFlags, tables: &AnvilShaders) -> Result<u32, String> {
    if f.blend_mode > 0xF || f.alpha_display_mode > 3 {
        return Err("a blend or alpha display mode wider than the engine's field".into());
    }
    let mut set = [false; 35];
    for (s, &b) in set.iter_mut().zip(&f.booleans) {
        *s = match b {
            0 => false,
            1 => true,
            _ => return Err("a boolean stored as neither 0 nor 1".into()),
        };
    }
    let feature = |n: usize| {
        tables
            .feature(n)
            .ok_or_else(|| format!("the template's shader-state node has no byte for feature {n}"))
    };
    let mut key = GBUFFER << 22;
    if set[MaterialFlags::TWO_SIDED] && set[MaterialFlags::FLIP_NORMAL_ON_BACK_FACE] && feature(12)? {
        key |= 0x40;
    }
    if f.alpha_display_mode == 2 && feature(13)? {
        key |= 0x2;
    }
    if set[MaterialFlags::LAYER_MATERIAL] && feature(10)? {
        key |= 0x4;
        if f.blend_mode != 0 {
            key |= 0x20;
        }
    }
    if set[MaterialFlags::BOOLEAN_42452A12] && feature(3)? && feature(4)? {
        key |= 0x10;
    }
    Ok(key)
}

/// The variant bit of the permutations that discard (`0x4000`; docs/shaders.md §6.2): a screen-position dither for the LOD
/// cross-fade and, in a template that has them, the alpha test (`alpha - cb2[18].y < 0`).
const DISCARD_VARIANT: u32 = 0x4000;

/// The pixel key a Material that tests alpha is drawn with: `key` with the discard variant, where the Material
/// states an alpha test (`AlphaTestEnabled`, an `AlphaTestValue` above 0, blend mode 0: the rule `lower-anvil-material`
/// follows) and the template has that permutation. **Inferred, not read**: the engine's key composer does not set bits 14
/// to 16 (they are chosen at draw time), and the permutations with the bit are the only ones of an alpha-tested template
/// that discard by alpha (MAD_CHR_Hair_Anisotropic: no other of its 528 pixel keys has a discard), so a Material that
/// tests alpha can only be cut by them. `None` where it does not apply.
pub fn alpha_tested_key(f: &MaterialFlags, tables: &AnvilShaders, key: u32) -> Option<u32> {
    let tests = f.blend_mode == 0
        && f.alpha_test_value > 0
        && f.booleans.get(MaterialFlags::ALPHA_TEST_ENABLED) == Some(&1);
    let variant = key | DISCARD_VARIANT;
    (tests && tables.pixel(variant).is_some()).then_some(variant)
}

/// The vertex key of the pair a pixel key `pixel` belongs to, for vertex layout `layout`: the Material-flag bits (1 to
/// 5, and 0) occur in the pixel stage's keys only, the layout number (bits 17 to 21) in the vertex stage's only.
pub fn gbuffer_vertex_key(pixel: u32, layout: u32) -> u32 {
    (pixel & !0x3F) | (layout << 17)
}

/// The shading class byte a template's G-buffer programs write (docs/material-spec.md §14, §15.4, shaders notes):
/// 1 for layout A (metalness in `o1.w`), 3 to 5 for layout B (occlusion in `o1.w`). Only the templates the notes
/// name; `None` for any other.
pub fn shading_class(template: &str) -> Option<u8> {
    match template {
        "MAD_Tex1_TerrainBlend" | "MAD_Outskirt" => Some(3),
        "MAD_Clutters_Grass" | "SHD_Nat_Veg_Grass_AO" => Some(4),
        "MAD_Tex1_Leaves" | "MAD_Bark" | "MAD_Tex1_Rocks" | "MAD_FakeForest" => Some(5),
        "MAD_Fake"
        | "MAD_Fake_Sediment"
        | "MAD_Tex1"
        | "MAD_Tex1_Wind"
        | "MAD_Decals1"
        | "MAD_Tex1_Emissive"
        | "MAD_Tex1_UGE"
        | "MAD_Cliffs_TriPlanar"
        | "MAD_OutBorder"
        | "MAD_Raw_TerrainBlend"
        | "MAD_Tex1_UniqueNormal"
        | "MAD_Tex1_UniqueNormalGrad_Triplanar"
        | "MAD_Tex1_UniqueNormal_DiffuseBlend"
        | "MAD_Tex1_Graffiti_Monster"
        | "MAD_Tex1_Graffiti_Mode"
        // Saved ps_400000 disassemblies: bfi inserts the sign into bit 0 of literal 8, then the
        // two normal high bits add only 2 and 4 before utof and the o1.xyz UNORM store. This is
        // class 1 regardless of material constants (material-spec §27 and the drift evidence table).
        | "MAD_Tex1_SkinningSupported"
        | "MAD_Tex1_TeamColoured"
        | "MAD_Tex2"
        | "MAD_Tex2_TintBFromShaderConstants"
        | "MAD_Tex2_Graffiti_Mode"
        | "MAD_Grounds"
        | "MAD_Road"
        | "MAD_Hero_Ice"
        | "MAD_IceBrick"
        | "MAD_Tex1_UniqueNormalGradient"
        | "MAD_Tex1_Screen"
        | "MAD_Screens"
        | "MAD_Screens_Simple"
        | "MAD_SHD_UNLIT_UndergroundBox"
        | "MAD_Tex1_FakeRiver"
        // Same pack in the saved cut-out permutations ps_404004 / ps_404000 respectively.
        | "MAD_Decals1_Graffiti"
        | "MAD_Tex1_Graffiti_Mode_DecalFX" => Some(1),
        t if t.starts_with("MAD_CHR_") => Some(1),
        _ => None,
    }
}

/// The fixed-function state a Material's flags state (see the module documentation).
pub fn render_state(f: &MaterialFlags) -> RenderState {
    let set = |j: usize| f.booleans.get(j).copied() == Some(1);
    let mut state = RenderState::default();
    if set(MaterialFlags::TWO_SIDED) {
        state.cull = Some(Cull::None);
    }
    if set(MaterialFlags::Z_TEST_DISABLED) {
        let write_disabled = if f.blend_mode == 0 {
            set(MaterialFlags::Z_WRITE_DISABLED_OPAQUE)
        } else {
            set(MaterialFlags::Z_WRITE_DISABLED_ALPHA)
        };
        state.depth = Some(DepthState {
            test: Compare::Always,
            write: !write_disabled,
        });
    }
    if set(MaterialFlags::ALPHA_TEST_ENABLED) {
        state.alpha_test = Some(AlphaTest {
            compare: Compare::GreaterEqual,
            reference: f32::from(f.alpha_test_value) / 255.0,
        });
    }
    state
}

/// The words of a float.
fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// What the Material's `cb3` bindings give, as constants of the buffer `cb3` (of `size` bytes), and what it counted:
/// each template binding's property (the Material's value, else the template's default for the binding) at its row,
/// `Unknown` where it has none or is a UV transform whose rows are not established. The rows of `cb3` no binding
/// covers are `Unknown` too, so every byte of the buffer has a role.
pub fn material_constants(
    m: &AnvilMaterial,
    cb3: ResourceId,
    size: u32,
    counts: &mut BTreeMap<String, u64>,
) -> Vec<ConstantBinding> {
    let value = |key: u32, binding: MaterialBinding| {
        m.values
            .iter()
            .find(|(k, v)| *k == key && !matches!(v, MaterialValue::Texture { .. }))
            .map(|(_, v)| v)
            .or_else(|| m.defaults.iter().find(|(b, _)| *b == binding).map(|(_, v)| v))
    };
    let mut out: Vec<ConstantBinding> = Vec::new();
    let mut push = |offset: u32, bytes: u32, role: ConstantRole, key: u32| {
        if offset.checked_add(bytes).is_some_and(|end| end <= size) {
            out.push(ConstantBinding {
                resource: cb3,
                offset,
                size: bytes,
                name: Some(format!("{key:08X}")),
                role,
                lines: None,
            });
        }
    };
    let unknown = |why: &str, counts: &mut BTreeMap<String, u64>| {
        *counts.entry(why.to_owned()).or_default() += 1;
        ConstantRole::Unknown { why: why.to_owned() }
    };
    for &(key, binding) in &m.bindings {
        match binding {
            MaterialBinding::Scalar { vec4 } => {
                let role = match value(key, binding) {
                    Some(MaterialValue::Float(x)) => ConstantRole::Material(bits(&[*x])),
                    Some(MaterialValue::Int(i)) => ConstantRole::Material(vec![*i as u32]),
                    _ => unknown(
                        "a bound scalar property neither the Material nor its template sets",
                        counts,
                    ),
                };
                push(vec4.saturating_mul(16), 4, role, key);
            }
            MaterialBinding::Vector { vec4 } => {
                let role = match value(key, binding) {
                    Some(MaterialValue::Float4(v)) => ConstantRole::Material(bits(v)),
                    _ => unknown(
                        "a bound vector property neither the Material nor its template sets",
                        counts,
                    ),
                };
                push(vec4.saturating_mul(16), 16, role, key);
            }
            MaterialBinding::UvTransform { vec4 } => {
                let role = match value(key, binding) {
                    Some(MaterialValue::Floats(f)) => match f.as_slice() {
                        f if f.len() == 8 => match super::material::static_uv_rows(f) {
                            Ok(rows) => ConstantRole::Material(bits(rows.as_flattened())),
                            Err(why) => unknown(
                                &super::material::uv_motion_notice(f).unwrap_or_else(|| why.to_owned()),
                                counts,
                            ),
                        },
                        _ => unknown("a UV transform property that is not eight floats", counts),
                    },
                    _ => unknown(
                        "a bound UV transform neither the Material nor its template sets",
                        counts,
                    ),
                };
                push(vec4.saturating_mul(16), 32, role, key);
            }
            MaterialBinding::Texture { .. } | MaterialBinding::Other => {}
        }
    }
    // every byte of the buffer has a role: the rows no binding covers are the engine's or the template's, unresolved
    out.sort_by_key(|c| c.offset);
    let mut gaps = Vec::new();
    let mut at = 0u32;
    for c in &out {
        if c.offset > at {
            gaps.push((at, c.offset - at));
        }
        at = at.max(c.offset.saturating_add(c.size));
    }
    if at < size {
        gaps.push((at, size - at));
    }
    for (offset, bytes) in gaps {
        out.push(ConstantBinding {
            resource: cb3,
            offset,
            size: bytes,
            name: None,
            role: ConstantRole::Unknown {
                why: "no template binding at these rows of cb3 (the engine's or unresolved)".into(),
            },
            lines: None,
        });
    }
    out.sort_by_key(|c| c.offset);
    out
}

/// The register of a resource of class `class` in `interface`, with its id.
/// A reason a row of the engine's buffers has no role, where the notes (docs/material-spec.md §3, §12.3, §17.1, §23)
/// say what the programs do with it: not a role in the vocabulary, or not established.
fn engine_reason(register: u32, row: u32, lane: u32) -> Option<&'static str> {
    Some(match (register, row, lane) {
        (2, 16, 0 | 1) => {
            "the LOD cross-fade: x and y are multiplied into a hash of the pixel's screen position, a fragment is discarded where it is below 0 (shaders.md §6.2, key bit 0x4000)"
        }
        (2, 18, 3) => {
            "the instance index the program adds to its own to find the instance's record in a buffer"
        }
        (2, 22, 1) => {
            "the offset of the instance's data in the per-instance buffer (three float4 per instance)"
        }
        (2, 0..=2, _) => {
            "the instance's transform as three rows of four (the translation in w), read with the position and normal by the vertex programs: not bound to a role, the rows' space (the stream's decoded coordinates or the world's) is not established"
        }
        (1, 41, 1) => "the bias of every biased texture sample",
        (1, 53, _) => "scene depth to view depth",
        (1, 77 | 78, _) => {
            "read as the sun's colour or illuminance and its Lambert term (an inference, material-spec §17.1)"
        }
        (1, 83, _) => "the water scale of the water pass: its colours are divided by it (material-spec §16)",
        (0, 4, 0..=2) => "a per-frame light vector",
        (0, 47 | 48, _) => "the terrain's virtual texture constants (material-spec §14.2, §23)",
        _ => return None,
    })
}

/// The matrices of the engine's buffers that have a role: the first row, and the role, of the four rows (64 bytes) that
/// contain `row`. The vertex programs write `SV_Position` as the dot products of the position (the stream's
/// coordinates divided by 32768, with w = 1) with `cb2[8..12]`, and the interpolant their pixel programs turn into the
/// velocity (`uv - (xy / w * (0.5, -0.5) + 0.5)`, `tests/engine_use.rs`) the same with `cb2[12..16]`: the object's clip
/// matrix and the previous frame's, as the rows `x`, `y`, `z`, `w` of a column-vector matrix.
fn engine_matrix(register: u32, row: u32) -> Option<(u32, ObjectParameter)> {
    Some(match (register, row) {
        (2, 8..=11) => (8, ObjectParameter::WorldViewProjection),
        (2, 12..=15) => (
            12,
            ObjectParameter::PreviousFrame(ObjectMatrix::WorldViewProjection),
        ),
        _ => return None,
    })
}

/// The tiles of the engine's buffers that have a role: (first lane, lanes, the role) of each, in a row. A program that
/// reads any lane of a tile has the tile bound whole, the role naming all of its values.
fn engine_tile(register: u32, row: u32, alpha_test_reference: f32) -> Vec<(u32, u32, ConstantRole)> {
    match (register, row) {
        // `cb1[37]` is `(1/W, 1/H, W, H)` in every copy of the per-view buffer found in memory, and the G-buffer pixel
        // shaders scale a pixel position by its xy for the velocity output (material-spec §17.1)
        (1, 37) => vec![
            (0, 2, ConstantRole::View(ViewParameter::InverseViewportSize)),
            (2, 2, ConstantRole::View(ViewParameter::ViewportSize)),
        ],
        // the multiplier the lighting passes scale their output by (`cb1[39].x`, 1 in every copy)
        (1, 39) => vec![(0, 1, ConstantRole::Frame(FrameParameter::Exposure))],
        // the alpha-test reference of the G-buffer pass: the Material's `AlphaTestValue / 255` where `AlphaTestEnabled`
        // is on, else 0 (shaders.md §6.2): a fragment is discarded where its alpha minus this is below 0
        (2, 18) => vec![(1, 1, ConstantRole::Material(bits(&[alpha_test_reference])))],
        _ => Vec::new(),
    }
}

/// The engine's constant buffers a program reads, as constants: `cb0` per frame, `cb1` per view, `cb2` per draw. The
/// notes know a handful of their rows ([`engine_tile`] holds the ones the vocabulary has a role for, [`engine_reason`]
/// the reasons for others); a program reads a few rows of each, so only the lanes it reads are bound, the rest counted
/// as left out, and a buffer the program indexes by a value it computes is one `Unknown` constant (any row may be read).
fn engine_constants(
    register: u32,
    resource: ResourceId,
    size: u32,
    reads: Result<&WordsRead, &str>,
    m: &AnvilMaterial,
    counts: &mut BTreeMap<String, u64>,
) -> Vec<ConstantBinding> {
    let mut bump = |what: &str, n: u64| *counts.entry(what.to_owned()).or_default() += n;
    let whole = |why: String| {
        vec![ConstantBinding {
            resource,
            offset: 0,
            size,
            name: None,
            role: ConstantRole::Unknown { why },
            lines: None,
        }]
    };
    let reads = match reads {
        Ok(reads) => reads,
        Err(why) => {
            bump(
                &format!(
                    "engine constant buffers of a program whose reads could not be taken ({why}), bound whole and Unknown"
                ),
                1,
            );
            return whole(format!(
                "cb{register} is the engine's per-frame, per-view or per-draw constants: no role resolved (the program's reads were not taken: {why})"
            ));
        }
    };
    let buffer = reads.buffer_at(0, register);
    let (rows, dynamic) = buffer.map_or((Vec::new(), false), |b| (reads.rows(b), reads.indexed(b)));
    if dynamic {
        bump(
            "engine constant buffers a program indexes by a computed value, bound whole and Unknown",
            1,
        );
        return whole(format!(
            "cb{register} is indexed by a value the program computes, so any row may be read: no role resolved"
        ));
    }
    let reference = if m.flags.booleans.get(MaterialFlags::ALPHA_TEST_ENABLED).copied() == Some(1) {
        f32::from(m.flags.alpha_test_value) / 255.0
    } else {
        0.0
    };
    let mut out = Vec::new();
    let mut read_rows = 0u32;
    // a matrix a program reads any row of is bound whole, once
    let mut matrices = BTreeSet::new();
    for row in rows
        .into_iter()
        .filter(|r| r.checked_mul(16).is_some_and(|at| at < size))
    {
        read_rows += 1;
        if let Some((first, parameter)) = engine_matrix(register, row) {
            if matrices.insert(first) && (first + 4) * 16 <= size {
                bump("engine constants with a role", 1);
                out.push(ConstantBinding {
                    resource,
                    offset: first * 16,
                    size: 64,
                    name: None,
                    role: ConstantRole::Object(parameter),
                    lines: None,
                });
            }
            continue;
        }
        let lanes = buffer.map_or_else(Vec::new, |b| reads.lanes(b, row));
        let mut covered = [false; 4];
        for (first, count, role) in engine_tile(register, row, reference) {
            if !(first..first + count).any(|lane| lanes.contains(&lane)) {
                continue;
            }
            bump("engine constants with a role", 1);
            for lane in first..first + count {
                covered[lane as usize] = true;
            }
            out.push(ConstantBinding {
                resource,
                offset: row * 16 + first * 4,
                size: count * 4,
                name: None,
                role,
                lines: None,
            });
        }
        // the other lanes it reads, in runs of one reason
        let mut lane = 0u32;
        while lane < 4 {
            if !lanes.contains(&lane) || covered[lane as usize] {
                lane += 1;
                continue;
            }
            let reason = engine_reason(register, row, lane);
            let start = lane;
            while lane < 4
                && lanes.contains(&lane)
                && !covered[lane as usize]
                && engine_reason(register, row, lane) == reason
            {
                lane += 1;
            }
            bump("engine constants without a role, Unknown", 1);
            out.push(ConstantBinding {
                resource,
                offset: row * 16 + start * 4,
                size: (lane - start) * 4,
                name: None,
                role: ConstantRole::Unknown {
                    why: match reason {
                        Some(r) => format!("engine constant cb{register}[{row}]: {r}"),
                        None => format!(
                            "engine constant cb{register}[{row}] (per-frame, per-view or per-draw): no role resolved"
                        ),
                    },
                },
                lines: None,
            });
        }
    }
    bump(
        "rows of the engine's constant buffers the program does not read, left out",
        u64::from((size / 16).saturating_sub(read_rows)),
    );
    out
}

fn by_register(
    interface: &Interface,
    class: primase_core::shader::D3dClass,
) -> Vec<(u32, &primase_core::shader::Resource)> {
    interface
        .resources
        .iter()
        .filter_map(|r| {
            let b = r.provenance.d3d?;
            (b.class == class).then_some((b.register, r))
        })
        .collect()
}

/// A stage's mesh-stream semantic by its signature name.
fn stream(name: &str, index: u32) -> Option<Semantic> {
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

/// What resolving a Material needs besides itself, from the pass: texture entities by (texture, sRGB), spawned on
/// first use.
struct Textures {
    spawned: BTreeMap<(Entity, bool), Entity>,
}

impl Textures {
    /// The entity to bind for the TextureMap entity `e`: itself where its colour space is the sRGB-ness its
    /// TextureMap states, else a copy in that colour space. `None` where `e` holds no `core.texture`.
    fn entity(&mut self, world: &mut World, e: Entity) -> Option<Entity> {
        let srgb = super::material::srgb(world, e);
        let wanted = if srgb {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        };
        if let Some(have) = world.get::<primase_core::TextureLayers>(e) {
            if have.color_space == wanted {
                return Some(e);
            }
            if let Some(&copy) = self.spawned.get(&(e, srgb)) {
                return Some(copy);
            }
            let mut copy = have.clone();
            copy.color_space = wanted;
            let name = world.get::<Name>(e).map_or_else(String::new, |n| n.0.clone());
            let entity = world.spawn();
            world.insert(entity, Name(format!("{name}@{wanted:?}")));
            world.insert(entity, copy);
            self.spawned.insert((e, srgb), entity);
            return Some(entity);
        }
        let have = world.get::<Texture>(e)?;
        if have.color_space == wanted {
            return Some(e);
        }
        if let Some(&c) = self.spawned.get(&(e, srgb)) {
            return Some(c);
        }
        let mut copy = have.clone();
        copy.color_space = wanted;
        let name = world.get::<Name>(e).map_or_else(String::new, |n| n.0.clone());
        let c = world.spawn();
        world.insert(c, Name(if srgb { format!("{name}@srgb") } else { name }));
        world.insert(c, copy);
        self.spawned.insert((e, srgb), c);
        Some(c)
    }
}

impl Pass for ResolveAnvilShaders {
    fn name(&self) -> &'static str {
        "resolve-anvil-shaders"
    }
    fn options(&self) -> Options {
        Options::from([("unset-textures".to_owned(), self.unset.text().to_owned())])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMaterial::NAME, AnvilShaders::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Shader::NAME,
            ShaderBinding::NAME,
            MaterialPrograms::NAME,
            ProgramFacts::NAME,
            Texture::NAME,
            primase_core::TextureLayers::NAME,
            Name::NAME,
        ]
    }
    fn approximates(&self) -> &'static str {
        "a texture binding with no texture reads white (unset-textures=white)"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, AnvilMaterial)> = world
            .iter::<AnvilMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut programs: BTreeMap<(Entity, bool, u32), Result<Entity, String>> = BTreeMap::new();
        // each program's value graph and constant reads, or why they could not be taken (the lift's or the graph's error)
        let mut graphs: BTreeMap<Entity, Result<ProgramGraph, String>> = BTreeMap::new();
        // the vertex layout number (`anvil.layout.kind`) of every mesh drawn with each Material
        let mut layouts_of: BTreeMap<Entity, std::collections::BTreeSet<u8>> = BTreeMap::new();
        for (_, mesh) in world.iter::<AnvilMesh>() {
            for material in mesh.material_entities.iter().flatten() {
                layouts_of.entry(*material).or_default().insert(mesh.kind);
            }
        }
        // a mesh `lower-anvil-mesh` has lowered: each primitive's layout, for the Material it draws with and the ones its
        // variants draw it with
        for (e, layout) in world.iter::<AnvilLayout>() {
            let Some(mesh) = world.get::<Mesh>(e) else {
                continue;
            };
            let variants = world.get::<MaterialVariants>(e);
            for (i, (primitive, &kind)) in mesh.primitives().iter().zip(&layout.kind).enumerate() {
                let drawn = primitive.material.into_iter().chain(
                    variants
                        .and_then(|v| v.0.get(i))
                        .into_iter()
                        .flat_map(|v| v.values().copied()),
                );
                for material in drawn {
                    layouts_of.entry(material).or_default().insert(kind);
                }
            }
        }
        let mut textures = Textures {
            spawned: BTreeMap::new(),
        };
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        let bump = |what: &str, n: u64, counts: &mut BTreeMap<String, u64>| {
            *counts.entry(what.to_owned()).or_default() += n;
        };
        'materials: for (material, m) in materials {
            let Some(tables) = m.shaders else {
                world.insert(
                    material,
                    MaterialPrograms(vec![ProgramFacts::unavailable(
                        m.template_name.as_deref().unwrap_or(""),
                        Stage::Pixel,
                        None,
                        "the reader does not hold the template's shader permutations",
                    )]),
                );
                bump(
                    "materials without their template's shader permutations read",
                    1,
                    &mut counts,
                );
                continue;
            };
            let template = m.template_name.clone().unwrap_or_default();
            let pixel_key = match world.get::<AnvilShaders>(tables).map_or_else(
                || Err("the template's shader tables are gone".to_owned()),
                |t| gbuffer_key(&m.flags, t).map(|k| (k, alpha_tested_key(&m.flags, t, k))),
            ) {
                Ok((k, None)) => k,
                Ok((_, Some(k))) => {
                    bump(
                        "materials bound to their template's alpha-tested permutation (key bit 0x4000, which the engine draws an AlphaTestEnabled Material with: inferred, not read)",
                        1,
                        &mut counts,
                    );
                    k
                }
                Err(why) => {
                    world.insert(
                        material,
                        MaterialPrograms(vec![ProgramFacts::unavailable(
                            &template,
                            Stage::Pixel,
                            None,
                            &why,
                        )]),
                    );
                    bump(&format!("materials left unbound: {why}"), 1, &mut counts);
                    continue;
                }
            };
            // the vertex layouts of the meshes drawn with this material (bits 17 to 21 of the vertex key, from the data)
            let layouts = layouts_of.get(&material).cloned().unwrap_or_default();
            if layouts.is_empty() {
                bump(
                    "materials no mesh is drawn with: bound with the pixel stage only",
                    1,
                    &mut counts,
                );
            }
            let pixel = match program(
                world,
                &mut programs,
                &mut graphs,
                &template,
                tables,
                false,
                pixel_key,
            ) {
                Ok(e) => e,
                Err(why) => {
                    let hash = world
                        .get::<AnvilShaders>(tables)
                        .and_then(|tables| tables.pixel(pixel_key))
                        .map(primase_shader::source_hash);
                    world.insert(
                        material,
                        MaterialPrograms(vec![ProgramFacts::unavailable(
                            format!("{template}-{pixel_key:06x}.ps"),
                            Stage::Pixel,
                            hash,
                            &why,
                        )]),
                    );
                    bump(&format!("materials left unbound: {why}"), 1, &mut counts);
                    continue 'materials;
                }
            };
            let mut stages: Vec<(Stage, Option<VariantKey>, Entity)> = Vec::new();
            let mut facts = Vec::new();
            for kind in layouts {
                if kind >= 32 {
                    bump(
                        &format!(
                            "meshes whose vertex layout number {kind} is past the key's five bits: no vertex stage bound"
                        ),
                        1,
                        &mut counts,
                    );
                    continue;
                }
                // the vertex stage of the pair; a template with no vertex permutation for the discard variant draws the
                // plain one (a vertex program does not discard)
                let key = gbuffer_vertex_key(pixel_key, u32::from(kind));
                let has = |k: u32| {
                    world
                        .get::<AnvilShaders>(tables)
                        .is_some_and(|t| t.vertex(k).is_some())
                };
                let key = if has(key) {
                    key
                } else {
                    let plain = gbuffer_vertex_key(pixel_key & !DISCARD_VARIANT, u32::from(kind));
                    if plain != key {
                        bump(
                            "vertex layouts of discard-variant materials drawn by the plain variant's vertex program (the template stores no vertex permutation for the discard variant: inferred, not read)",
                            1,
                            &mut counts,
                        );
                    }
                    plain
                };
                match program(world, &mut programs, &mut graphs, &template, tables, true, key) {
                    Ok(e) => stages.push((
                        Stage::Vertex,
                        Some(VariantKey {
                            field: MESH_LAYOUT_FIELD.to_owned(),
                            value: u32::from(kind),
                            parts: Vec::new(),
                        }),
                        e,
                    )),
                    Err(why) => {
                        let hash = world
                            .get::<AnvilShaders>(tables)
                            .and_then(|tables| tables.vertex(key))
                            .map(primase_shader::source_hash);
                        facts.push(ProgramFacts::unavailable(
                            format!("{template}-{key:06x}.vs"),
                            Stage::Vertex,
                            hash,
                            &why,
                        ));
                        bump(
                            &format!("vertex layouts whose vertex stage is left unbound: {why}"),
                            1,
                            &mut counts,
                        );
                    }
                }
            }
            let vertex_bound = !stages.is_empty();
            stages.push((Stage::Pixel, None, pixel));
            let mut bound = Vec::new();
            for (stage, variant, shader) in stages {
                let interface = world
                    .get::<Shader>(shader)
                    .map(|s| s.interface.clone())
                    .ok_or("a resolved shader entity without its shader")?;
                // program() cached these from the same lift that produced the interface.
                let graph = graphs
                    .get(&shader)
                    .ok_or("a resolved shader entity without its graph result")?;
                let binding = self.stage_binding(
                    world,
                    &mut textures,
                    &m,
                    &template,
                    stage,
                    variant,
                    shader,
                    &interface,
                    vertex_bound,
                    graph.as_ref().map_err(String::as_str),
                    &mut counts,
                );
                facts.push(
                    world
                        .get::<ProgramFacts>(shader)
                        .ok_or("a resolved shader entity without its facts")?
                        .clone()
                        .bound(shader, &binding.outputs),
                );
                bound.push(binding);
            }
            bound.sort_by_key(|s| s.stage);
            world.insert(material, MaterialPrograms(facts));
            world.insert(
                material,
                ShaderBinding {
                    stages: bound,
                    state: render_state(&m.flags),
                },
            );
            bump("materials bound", 1, &mut counts);
        }
        for (what, n) in counts {
            if what.starts_with("texture bindings with no texture, bound white") {
                log.approximate(&what, n);
            } else {
                log.count(&what, n);
            }
        }
        log.count(
            "shader programs",
            programs.values().filter(|r| r.is_ok()).count() as u64,
        );
        Ok(())
    }
}

/// The mesh-side field a vertex stage's variant key names: the vertex layout number of the mesh (bits 17 to 21 of its
/// vertex key), kept per primitive of a lowered mesh ([`AnvilLayout`]).
pub const MESH_LAYOUT_FIELD: &str = "anvil.layout.kind";

/// The `core.shader` entity of the template's `key` permutation of one stage, made once and kept in `programs`.
fn program(
    world: &mut World,
    programs: &mut BTreeMap<(Entity, bool, u32), Result<Entity, String>>,
    graphs: &mut BTreeMap<Entity, Result<ProgramGraph, String>>,
    template: &str,
    tables: Entity,
    is_vertex: bool,
    key: u32,
) -> Result<Entity, String> {
    if let Some(found) = programs.get(&(tables, is_vertex, key)) {
        return found.clone();
    }
    let stage = if is_vertex { "vertex" } else { "pixel" };
    let bytes = world
        .get::<AnvilShaders>(tables)
        .and_then(|t| if is_vertex { t.vertex(key) } else { t.pixel(key) }.map(<[u8]>::to_vec));
    let made = match bytes {
        None => Err(format!(
            "the template has no {stage} permutation for the G-buffer key"
        )),
        Some(bytes) => match primase_dxbc::lift(&bytes) {
            Err(e) => Err(format!("a {stage} program the lifter refuses: {e}")),
            Ok(program) => {
                let e = world.spawn();
                let name = format!("{template}-{key:06x}.{}", if is_vertex { "vs" } else { "ps" });
                world.insert(e, program.facts(&name));
                graphs.insert(
                    e,
                    program.graph().map_err(|why| format!("the value graph: {why}")),
                );
                world.insert(e, Name(name));
                world.insert(
                    e,
                    Shader {
                        format: ShaderFormat::Dxbc,
                        bytecode: bytes,
                        interface: program.interface,
                    },
                );
                Ok(e)
            }
        },
    };
    programs.insert((tables, is_vertex, key), made.clone());
    made
}

impl ResolveAnvilShaders {
    #[allow(clippy::too_many_arguments)]
    fn stage_binding(
        &self,
        world: &mut World,
        textures: &mut Textures,
        m: &AnvilMaterial,
        template: &str,
        stage: Stage,
        variant: Option<VariantKey>,
        shader: Entity,
        interface: &Interface,
        vertex_bound: bool,
        graph: Result<&ProgramGraph, &str>,
        counts: &mut BTreeMap<String, u64>,
    ) -> StageBinding {
        let reads = graph.map(|graph| &graph.reads);
        let bump = |what: &str, counts: &mut BTreeMap<String, u64>| {
            *counts.entry(what.to_owned()).or_default() += 1;
        };
        // inputs
        let inputs = interface
            .inputs
            .iter()
            .filter(|v| v.system.is_none())
            .map(|v| InputBinding {
                label: v.label.clone(),
                role: match stage {
                    Stage::Vertex => match stream(&v.label.name, v.label.index) {
                        Some(s) => InputRole::Stream(s),
                        None => InputRole::Unknown {
                            why: format!(
                                "vertex input {} is not a mesh semantic this pass maps",
                                v.label.name
                            ),
                        },
                    },
                    Stage::Pixel if vertex_bound => InputRole::Previous,
                    Stage::Pixel => InputRole::Unknown {
                        why: "the vertex stage is not bound (no mesh is drawn with the material)".into(),
                    },
                    _ => InputRole::Unknown {
                        why: "a stage this pass does not bind".into(),
                    },
                },
            })
            .collect();
        // constants: cb3 the Material's, the rest the engine's
        let mut constants = Vec::new();
        for (register, r) in by_register(interface, primase_core::shader::D3dClass::ConstantBuffer) {
            let ResourceKind::ConstantBuffer { layout } = &r.kind else {
                continue;
            };
            if register == 3 {
                constants.extend(material_constants(m, r.id, layout.size, counts));
            } else if layout.size > 0 {
                constants.extend(engine_constants(register, r.id, layout.size, reads, m, counts));
            }
        }
        // textures
        let mut bindings = Vec::new();
        for (register, r) in by_register(interface, primase_core::shader::D3dClass::ShaderResource) {
            let ResourceKind::Texture { dim, .. } = r.kind else {
                continue;
            };
            let slot = register.checked_sub(75);
            let bound = slot.and_then(|s| m.textures.iter().find(|t| t.shader_slot == s));
            let source = match (bound, dim) {
                (None, _) => {
                    bump("texture slots that are the engine's, left Missing", counts);
                    TextureSource::Missing
                }
                (Some(_), d) if d != TextureDim::D2 => {
                    bump(
                        "texture bindings of a dimension other than 2D, left Missing",
                        counts,
                    );
                    TextureSource::Missing
                }
                (Some(b), _) => match b.texture.and_then(|e| textures.entity(world, e)) {
                    Some(texture) => {
                        bump("texture bindings bound", counts);
                        TextureSource::Texture {
                            texture,
                            sampler: SamplerState::default(),
                        }
                    }
                    None => match self.unset {
                        UnsetTexture::White => {
                            bump(
                                "texture bindings with no texture, bound white (unset-textures=white)",
                                counts,
                            );
                            TextureSource::Default {
                                value: [1.0; 4],
                                why: "no texture bound: white (unset-textures=white)".into(),
                            }
                        }
                        UnsetTexture::Leave => {
                            bump("texture bindings with no texture, left Missing", counts);
                            TextureSource::Missing
                        }
                    },
                },
            };
            bindings.push(TextureBinding {
                resource: r.id,
                sampler: None,
                source,
            });
        }
        // outputs: the G-buffer's targets
        let mut outputs = Vec::new();
        if stage == Stage::Pixel {
            let class = shading_class(template);
            outputs = output_bindings(interface, class, m.flags.blend_mode == 0);
            if class.is_none() {
                bump(
                    "pixel stages of a template whose shading class is not in the notes' table (o1.w Unknown)",
                    counts,
                );
            }
        }
        let unshown = state_lines(graph, interface, &mut constants);
        if unshown > 0 {
            *counts
                .entry("matrices whose lines the program does not show (a consumer writing a non-symmetric matrix cannot bind them)".into())
                .or_default() += unshown as u64;
        }
        StageBinding {
            stage,
            variant,
            shader,
            inputs,
            constants,
            textures: bindings,
            buffers: Vec::new(),
            constant_buffers: Vec::new(),
            outputs,
        }
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
    use primase_core::frame::SourceFrame;

    #[test]
    fn program_texture_binding_without_texturemap_keeps_the_declared_transfer() {
        use primase_core::{TexFormat, TexRole};
        let mut world = World::new();
        let e = world.spawn();
        world.insert(
            e,
            Texture {
                width: 1,
                height: 1,
                format: TexFormat::Rgba8Unorm,
                levels: vec![vec![128, 64, 32, 255]],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        let mut bindings = Textures {
            spawned: BTreeMap::new(),
        };
        assert_eq!(bindings.entity(&mut world, e), Some(e));
        assert_eq!(world.get::<Texture>(e).unwrap().color_space, ColorSpace::Srgb);
        assert!(
            bindings.spawned.is_empty(),
            "no linear duplicate of an already stated sRGB texture"
        );
    }

    pub(super) fn material(
        bindings: Vec<(u32, MaterialBinding)>,
        values: Vec<(u32, MaterialValue)>,
        defaults: Vec<(MaterialBinding, MaterialValue)>,
    ) -> AnvilMaterial {
        AnvilMaterial {
            index_id: 1,
            template: None,
            template_name: Some("MAD_Tex1".into()),
            bindings,
            values,
            texture_set: Vec::new(),
            textures: Vec::new(),
            defaults,
            frame: SourceFrame::WORLD,
            flags: MaterialFlags {
                blend_mode: 0,
                alpha_display_mode: 0,
                alpha_test_value: 0,
                booleans: [0; 35],
            },
            shaders: None,
        }
    }

    /// A scalar at row 1, a vector at row 2, a UV transform at rows 4 and 5 of a 6-row buffer: the Material sets the
    /// scalar, the template defaults the vector, nothing sets the transform. Every byte of the buffer has a role.
    #[test]
    fn cb3_rows_follow_the_templates_bindings_and_leave_nothing_unroled() {
        let m = material(
            vec![
                (0xA, MaterialBinding::Scalar { vec4: 1 }),
                (0xB, MaterialBinding::Vector { vec4: 2 }),
                (0xC, MaterialBinding::UvTransform { vec4: 4 }),
            ],
            vec![(0xA, MaterialValue::Float(0.25))],
            vec![(
                MaterialBinding::Vector { vec4: 2 },
                MaterialValue::Float4([1.0, 2.0, 3.0, 4.0]),
            )],
        );
        let mut counts = BTreeMap::new();
        let c = material_constants(&m, ResourceId(0), 6 * 16, &mut counts);
        let at = |offset: u32| c.iter().find(|c| c.offset == offset).unwrap();
        assert_eq!(at(16).role, ConstantRole::Material(vec![0.25f32.to_bits()]));
        assert_eq!((at(16).size, at(16).name.as_deref()), (4, Some("0000000A")));
        assert_eq!(
            at(32).role,
            ConstantRole::Material(bits(&[1.0, 2.0, 3.0, 4.0])),
            "the template's default where the Material sets none"
        );
        assert!(
            matches!(at(64).role, ConstantRole::Unknown { .. }),
            "a transform nothing sets"
        );
        assert_eq!(at(64).size, 32);
        // the bytes between and around (row 0, the rest of row 1, row 3) are Unknown gaps: the whole buffer is covered
        let mut covered = [false; 96];
        for b in &c {
            for byte in b.offset..b.offset + b.size {
                assert!(!covered[byte as usize], "byte {byte} twice");
                covered[byte as usize] = true;
            }
        }
        assert!(covered.iter().all(|&x| x));
        assert_eq!(
            counts["a bound UV transform neither the Material nor its template sets"],
            1
        );
    }

    /// Static translation/rotation use S R; a moving or malformed property retains its named refusal.
    #[test]
    fn uv_transform_rows_are_exact_only_where_the_notes_show_them() {
        let transform = |floats: Vec<f32>| {
            let m = material(
                vec![(1, MaterialBinding::UvTransform { vec4: 0 })],
                vec![(1, MaterialValue::Floats(floats))],
                Vec::new(),
            );
            material_constants(&m, ResourceId(0), 48, &mut BTreeMap::new())[0]
                .role
                .clone()
        };
        // su 2, sv 3, tu 0.5, tv -1: rows (2, 0, 0.5, 0) and (0, 3, -1, 0)
        assert_eq!(
            transform(vec![2.0, 3.0, 0.5, -1.0, 0.0, 0.0, 0.0, 0.0]),
            ConstantRole::Material(bits(&[2.0, 0.0, 0.5, 0.0, 0.0, 3.0, -1.0, 0.0]))
        );
        let (s, c) = 0.3f32.sin_cos();
        assert_eq!(
            transform(vec![2.0, 3.0, 0.5, -1.0, 0.0, 0.0, 0.3, 0.0]),
            ConstantRole::Material(bits(&[2.0 * c, -2.0 * s, 0.5, 0.0, 3.0 * s, 3.0 * c, -1.0, 0.0]))
        );
        for moving in [
            vec![1.0, 1.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0],
            vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.3, 0.1],
            vec![1.0, 1.0, 0.0],
        ] {
            assert!(matches!(transform(moving), ConstantRole::Unknown { .. }));
        }
        let ConstantRole::Unknown { why } = transform(vec![1.0, 1.0, 0.0, 0.0, 0.25, -0.5, 0.0, 0.125])
        else {
            panic!("a moving property must retain its rate-bearing refusal");
        };
        assert!(
            why.contains("TranslationSpeedU=0.25") && why.contains("RotationSpeed=0.125"),
            "{why}"
        );
    }

    #[test]
    fn state_is_what_the_flags_state() {
        let mut f = material(Vec::new(), Vec::new(), Vec::new()).flags;
        assert_eq!(render_state(&f), RenderState::default());
        f.booleans[MaterialFlags::TWO_SIDED] = 1;
        f.booleans[MaterialFlags::Z_TEST_DISABLED] = 1;
        f.booleans[MaterialFlags::Z_WRITE_DISABLED_OPAQUE] = 1;
        f.booleans[MaterialFlags::ALPHA_TEST_ENABLED] = 1;
        f.alpha_test_value = 51;
        let s = render_state(&f);
        assert_eq!(s.cull, Some(Cull::None));
        assert_eq!(
            s.depth,
            Some(DepthState {
                test: Compare::Always,
                write: false
            })
        );
        assert_eq!(
            s.alpha_test,
            Some(AlphaTest {
                compare: Compare::GreaterEqual,
                reference: 0.2
            })
        );
        // a blended Material's depth write follows the alpha flag
        f.blend_mode = 2;
        assert!(render_state(&f).depth.unwrap().write);
        f.booleans[MaterialFlags::Z_WRITE_DISABLED_ALPHA] = 1;
        assert!(!render_state(&f).depth.unwrap().write);
    }

    #[test]
    fn the_shading_class_is_the_notes_table_only() {
        assert_eq!(shading_class("MAD_Tex1_Rocks"), Some(5));
        assert_eq!(shading_class("MAD_Tex1_TerrainBlend"), Some(3));
        assert_eq!(shading_class("MAD_Clutters_Grass"), Some(4));
        assert_eq!(shading_class("MAD_Tex1"), Some(1));
        assert_eq!(shading_class("MAD_CHR_5Zones_Details"), Some(1));
        // The saved Road program packs literal 8 plus the normal's three low bits: class 1.
        assert_eq!(shading_class("MAD_Road"), Some(1));
        assert_eq!(shading_class("MAD_Road_Unestablished"), None);
    }

    #[test]
    fn options_are_required_and_checked() {
        let o = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<Options>()
        };
        let make = |pairs: &[(&str, &str)]| RESOLVE_ANVIL_SHADERS.instance(&o(pairs));
        assert!(make(&[]).is_err());
        assert!(make(&[("unset-textures", "white")]).is_ok());
        // the vertex layout is no option: each mesh's own decides
        let gone = make(&[("unset-textures", "white"), ("vertex-layout", "2")]);
        assert!(gone.err().unwrap().contains("unknown option vertex-layout"));
        let p = make(&[("unset-textures", "leave")]).unwrap();
        assert_eq!(
            p.options(),
            ResolveAnvilShaders::new(UnsetTexture::Leave).options()
        );
        assert!(!p.options().contains_key("vertex-layout"));
    }
}

/// The render target a pixel output writes: a lifted program states it as the system value `SV_Target<n>`, a
/// program described by semantic as the label's index; any other system value is not a colour output.
pub(super) fn target_of(v: &Varying) -> Option<u32> {
    match v.system {
        None => Some(v.label.index),
        Some(SystemValue::Target(n)) => Some(n),
        Some(_) => None,
    }
}

/// The G-buffer roles of a pixel program's targets: target 0 is albedo (rgb) and roughness (a, opaque materials
/// only), target 1 the unnamed rgb and the metallic or occlusion lane of the shading class; any other lane is Unknown
/// with what the notes (material-spec §3, §15.4) show of it.
pub(super) fn output_bindings(interface: &Interface, class: Option<u8>, opaque: bool) -> Vec<OutputBinding> {
    let mut outputs = Vec::new();
    for (v, target) in interface.outputs.iter().filter_map(|v| Some((v, target_of(v)?))) {
        let full: u8 = (1u16 << v.value.width).saturating_sub(1) as u8;
        let mut role = |lanes: u8, role: OutputRole| {
            let lanes = lanes & full;
            if lanes != 0 {
                outputs.push(OutputBinding {
                    label: v.label.clone(),
                    lanes,
                    role,
                });
            }
        };
        match target {
            0 => {
                role(0b0111, OutputRole::Albedo);
                role(
                    0b1000,
                    if opaque {
                        OutputRole::Roughness
                    } else {
                        OutputRole::unknown(
                            "target 0's w of a blended Material: its alpha (material-spec §3), and the blend that would read it is not stated",
                        )
                    },
                );
            }
            1 => {
                role(
                    0b0111,
                    OutputRole::Normal {
                        encoding: NormalEncoding::OctahedralDiamond,
                        space: NormalSpace::World,
                    },
                );
                role(
                    0b1000,
                    match class {
                        Some(1) => OutputRole::Metallic,
                        Some(3..=5) => OutputRole::Occlusion {
                            of: Occluded::AllLight,
                            encoding: ScalarEncoding::LINEAR,
                        },
                        _ => OutputRole::unknown(
                            "target 1's w of a template whose shading class is not in the notes' table: the metalness (class 1) or the occlusion (3 to 5) by the class",
                        ),
                    },
                );
            }
            // the lanes the deferred lighting pass reads, by the shading class in `o1.z` (material-spec §15.4): the
            // velocity in xy for every class; for layout A (class 1) `o2.z` the emissive scalar (divided by the exposure
            // texture), and `o3` the occlusion in x, the specular occlusion in z and the translucency in w; for layout B
            // (classes 3 to 5) `o2.z` the specular occlusion and `o2.w` the translucency, with no `o3`. `o2.w` of
            // layout A (the albedo alpha of the blended permutations) and `o3.y` (a 6-bit per-pixel value the lighting
            // pass reads, meaning not established) are `Unknown`.
            2 if class.is_some() => {
                role(0b0011, OutputRole::MotionVector);
                match class {
                    Some(1) => {
                        role(
                            0b0100,
                            OutputRole::Emission {
                                encoding: ScalarEncoding::LINEAR,
                            },
                        );
                        role(
                            0b1000,
                            OutputRole::unknown(
                                "target 2's w in layout A (class 1): the albedo alpha of the blended permutations, not among the lanes the lighting pass reads for the class (material-spec §15.4)",
                            ),
                        );
                    }
                    _ => {
                        role(0b0100, OutputRole::SpecularOcclusion);
                        role(
                            0b1000,
                            if opaque {
                                OutputRole::Translucency
                            } else {
                                OutputRole::unknown(
                                    "blended layout-B target 2.w: coverage replaces the opaque translucency (Clutters_Grass writes opacity); no transmission inferred",
                                )
                            },
                        );
                    }
                }
            }
            3 if class == Some(1) => {
                role(
                    0b0001,
                    OutputRole::Occlusion {
                        of: Occluded::AllLight,
                        encoding: ScalarEncoding::LINEAR,
                    },
                );
                role(
                    0b0010,
                    OutputRole::unknown(
                        "target 3's y in layout A (class 1): a 6-bit per-pixel value the lighting pass reads, its meaning not established (material-spec §15.4)",
                    ),
                );
                role(0b0100, OutputRole::SpecularOcclusion);
                role(0b1000, OutputRole::Translucency);
            }
            2 | 3 if class.is_none() => role(
                0b1111,
                OutputRole::unknown(format!(
                    "target {target} of a template whose shading class is not in the notes' table: the lighting pass reads its lanes by the class"
                )),
            ),
            3 => role(
                0b1111,
                OutputRole::unknown(
                    "target 3 in layout B (classes 3 to 5): read by nothing (material-spec §15.4)",
                ),
            ),
            n => role(
                0b1111,
                OutputRole::unknown(format!(
                    "target {n}, past the four the lighting pass reads (material-spec §15.4)"
                )),
            ),
        }
    }
    outputs
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "hand-worked fixtures")]
mod key_tests {
    use super::tests::material;
    use super::*;

    /// A template whose feature node has the given features set (byte 0, the object's, then one byte per feature).
    fn template(features: &[usize]) -> AnvilShaders {
        let mut node = vec![1u8; 1];
        node.resize(54, 0);
        for &n in features {
            node[n + 1] = 1;
        }
        AnvilShaders {
            pixel: Vec::new(),
            vertex: Vec::new(),
            programs: Vec::new(),
            feature_node: node,
        }
    }

    /// A Material that tests alpha (AlphaTestEnabled, a value above 0, blend mode 0) is drawn with its template's discard
    /// variant (`0x4000`) where the template has it, and with the plain key otherwise; one that tests no alpha never.
    #[test]
    fn a_material_that_tests_alpha_takes_the_discard_variant_where_there_is_one() {
        let plain = 1 << 22;
        let mut t = template(&[]);
        t.programs = vec![Vec::new(); 2];
        t.pixel = vec![(plain, 0)];
        let mut tests = flags(&[MaterialFlags::ALPHA_TEST_ENABLED], 0, 0);
        tests.alpha_test_value = 32;
        assert_eq!(
            alpha_tested_key(&tests, &t, plain),
            None,
            "the template has no such permutation"
        );
        t.pixel = vec![(plain, 0), (plain | 0x4000, 1)];
        assert_eq!(alpha_tested_key(&tests, &t, plain), Some(plain | 0x4000));
        assert_eq!(
            alpha_tested_key(&tests, &t, plain | 0x40),
            None,
            "the pair of another key is not there"
        );
        let mut no_value = tests;
        no_value.alpha_test_value = 0;
        let off = flags(&[], 0, 0);
        let mut blended = tests;
        blended.blend_mode = 4;
        for f in [no_value, off, blended] {
            assert_eq!(alpha_tested_key(&f, &t, plain), None, "{f:?}");
        }
    }

    fn flags(set: &[usize], blend: u32, alpha_display: u32) -> MaterialFlags {
        let mut booleans = [0u8; 35];
        for &j in set {
            booleans[j] = 1;
        }
        MaterialFlags {
            blend_mode: blend,
            alpha_display_mode: alpha_display,
            alpha_test_value: 0,
            booleans,
        }
    }

    /// Each gated bit, with the gate open and shut (docs/shaders.md §6.2's table, worked by hand).
    #[test]
    fn a_flag_sets_its_key_bit_only_where_the_templates_feature_allows_it() {
        let plain = 1 << 22;
        let both_sides = [MaterialFlags::TWO_SIDED, MaterialFlags::FLIP_NORMAL_ON_BACK_FACE];
        // TwoSided alone never; with FlipNormalOnBackFace, 0x40 only through feature 12
        assert_eq!(
            gbuffer_key(&flags(&[MaterialFlags::TWO_SIDED], 0, 0), &template(&[12])),
            Ok(plain)
        );
        assert_eq!(gbuffer_key(&flags(&both_sides, 0, 0), &template(&[])), Ok(plain));
        assert_eq!(
            gbuffer_key(&flags(&both_sides, 0, 0), &template(&[12])),
            Ok(plain | 0x40)
        );
        // AlphaDisplayMode 2: 0x2 through feature 13
        assert_eq!(gbuffer_key(&flags(&[], 0, 2), &template(&[])), Ok(plain));
        assert_eq!(gbuffer_key(&flags(&[], 0, 2), &template(&[13])), Ok(plain | 0x2));
        assert_eq!(gbuffer_key(&flags(&[], 0, 1), &template(&[13])), Ok(plain));
        // LayerMaterial: 0x4 through feature 10, with 0x20 beside it for a blend mode
        let layer = [MaterialFlags::LAYER_MATERIAL];
        assert_eq!(gbuffer_key(&flags(&layer, 0, 0), &template(&[])), Ok(plain));
        assert_eq!(
            gbuffer_key(&flags(&layer, 0, 0), &template(&[10])),
            Ok(plain | 0x4)
        );
        assert_eq!(
            gbuffer_key(&flags(&layer, 3, 0), &template(&[10])),
            Ok(plain | 0x24)
        );
        assert_eq!(gbuffer_key(&flags(&layer, 3, 0), &template(&[])), Ok(plain));
        // boolean 42452A12: 0x10 only with features 3 and 4 both
        let b = [MaterialFlags::BOOLEAN_42452A12];
        assert_eq!(gbuffer_key(&flags(&b, 0, 0), &template(&[3])), Ok(plain));
        assert_eq!(
            gbuffer_key(&flags(&b, 0, 0), &template(&[3, 4])),
            Ok(plain | 0x10)
        );
        // everything at once
        let all = [
            MaterialFlags::TWO_SIDED,
            MaterialFlags::FLIP_NORMAL_ON_BACK_FACE,
            MaterialFlags::LAYER_MATERIAL,
            MaterialFlags::BOOLEAN_42452A12,
        ];
        assert_eq!(
            gbuffer_key(&flags(&all, 1, 2), &template(&[3, 4, 10, 12, 13])),
            Ok(plain | 0x40 | 0x2 | 0x24 | 0x10)
        );
    }

    #[test]
    fn a_stored_value_the_engine_does_not_read_is_refused() {
        assert!(gbuffer_key(&flags(&[], 16, 0), &template(&[])).is_err());
        assert!(gbuffer_key(&flags(&[], 0, 4), &template(&[])).is_err());
        let mut f = flags(&[], 0, 0);
        f.booleans[7] = 2;
        assert!(gbuffer_key(&f, &template(&[])).is_err());
        // a feature the node has no byte for, where a flag asks for it
        let mut t = template(&[]);
        t.feature_node.truncate(10);
        let both = flags(
            &[MaterialFlags::TWO_SIDED, MaterialFlags::FLIP_NORMAL_ON_BACK_FACE],
            0,
            0,
        );
        assert!(gbuffer_key(&both, &t).unwrap_err().contains("feature 12"));
        // and one it does not ask for is not read
        assert_eq!(gbuffer_key(&flags(&[], 0, 0), &t), Ok(1 << 22));
        // a feature byte of 2 or 5 is a set feature (feature 15's byte is 2 on 82 templates), as 1 is
        let mut t = template(&[]);
        t.feature_node[13] = 5;
        assert_eq!(gbuffer_key(&both, &t), Ok((1 << 22) | 0x40));
    }

    /// The vertex stage's key keeps the shared bits (pass, 0x40, the feature flags) and takes the layout number, and
    /// drops the Material-flag bits only the pixel stage's keys have.
    #[test]
    fn the_vertex_key_drops_the_pixel_stages_flag_bits_and_takes_the_layout() {
        assert_eq!(gbuffer_vertex_key(0x40_0000, 0), 0x40_0000);
        assert_eq!(
            gbuffer_vertex_key(0x40_0000 | 0x40 | 0x24 | 0x2, 3),
            0x40_0000 | 0x40 | (3 << 17)
        );
        assert_eq!(
            gbuffer_vertex_key(0x40_0000 | 0x10 | 0x1, 11),
            0x40_0000 | (11 << 17)
        );
    }

    /// The D3D behaviour fixture of the DXBC lifter (our own HLSL, `ps_5_0`): five colour targets, which a lifted
    /// program states as the system values `SV_Target0..4` (not as semantics), as every program this pass binds.
    const LIFTED: &[u8] = include_bytes!("../../../../toolkit/dxbc/tests/fixtures/modes.dxbc");

    #[test]
    fn cached_program_records_facts_and_graph_from_its_single_fixture_lift() {
        let mut world = World::new();
        let tables = world.spawn();
        let mut template = template(&[]);
        template.pixel = vec![(0, 0)];
        template.programs = vec![LIFTED.to_vec()];
        world.insert(tables, template);
        let mut programs = BTreeMap::new();
        let mut graphs = BTreeMap::new();
        let entity = program(
            &mut world,
            &mut programs,
            &mut graphs,
            "fixture",
            tables,
            false,
            0,
        )
        .unwrap();
        assert_eq!(
            program(
                &mut world,
                &mut programs,
                &mut graphs,
                "fixture",
                tables,
                false,
                0
            )
            .unwrap(),
            entity
        );
        assert_eq!(graphs.len(), 1);
        assert!(graphs[&entity].is_ok());
        let shader = world.get::<Shader>(entity).unwrap();
        let outputs = output_bindings(&shader.interface, Some(1), true);
        let facts = world
            .get::<ProgramFacts>(entity)
            .unwrap()
            .clone()
            .bound(entity, &outputs);
        assert_eq!(facts.hash, Some(primase_shader::source_hash(LIFTED)));
        assert_eq!(facts.stage, Stage::Pixel);
        assert!(facts.instruction_count.unwrap().count > 0);
        assert_eq!(
            facts.instruction_count.unwrap().unit,
            primase_core::shader::InstructionUnit::DxbcInstructions
        );
        assert!(
            facts
                .inputs
                .iter()
                .all(|input| input.used && input.provenance.d3d.is_some())
        );
        assert_eq!(
            facts.surface,
            primase_core::shader::Surface::read(outputs.iter().map(|output| (output.lanes, &output.role)))
        );
    }

    /// An output role with any why as `UNKNOWN`'s, where it has one: the tests below hold which lanes are Unknown.
    fn unknown_as_one(role: OutputRole) -> OutputRole {
        match role {
            OutputRole::Unknown { why } => {
                assert!(!why.trim().is_empty());
                OutputRole::unknown("…")
            }
            role => role,
        }
    }

    /// A lifted pixel program's outputs are system values, and the roles come all the same: target 0 is albedo and
    /// roughness, target 1 the unnamed rgb and the lane its shading class names, targets 2 and 3 the lanes the lighting
    /// pass reads for the class, the rest Unknown with why.
    #[test]
    fn a_lifted_pixel_programs_targets_have_the_gbuffer_roles() {
        let program = primase_dxbc::lift(LIFTED).unwrap();
        assert!(
            program
                .interface
                .outputs
                .iter()
                .all(|v| matches!(v.system, Some(SystemValue::Target(_))))
        );
        let roles: Vec<(u32, u8, OutputRole)> = output_bindings(&program.interface, Some(1), true)
            .into_iter()
            .map(|o| {
                let target = program
                    .interface
                    .outputs
                    .iter()
                    .position(|v| v.label == o.label)
                    .unwrap();
                (u32::try_from(target).unwrap(), o.lanes, unknown_as_one(o.role))
            })
            .collect();
        assert_eq!(
            roles,
            [
                (0, 0b0111, OutputRole::Albedo),
                (0, 0b1000, OutputRole::Roughness),
                (
                    1,
                    0b0111,
                    OutputRole::Normal {
                        encoding: NormalEncoding::OctahedralDiamond,
                        space: NormalSpace::World
                    }
                ),
                (1, 0b1000, OutputRole::Metallic),
                (2, 0b0011, OutputRole::MotionVector),
                (
                    2,
                    0b0100,
                    OutputRole::Emission {
                        encoding: ScalarEncoding::LINEAR
                    }
                ),
                (2, 0b1000, OutputRole::unknown("…")),
                (
                    3,
                    0b0001,
                    OutputRole::Occlusion {
                        of: Occluded::AllLight,
                        encoding: ScalarEncoding::LINEAR
                    }
                ),
                (3, 0b0010, OutputRole::unknown("…")),
                (3, 0b0100, OutputRole::SpecularOcclusion),
                (3, 0b1000, OutputRole::Translucency),
                (4, 0b1111, OutputRole::unknown("…")),
            ]
        );
        // a depth output is not a colour target
        let mut with_depth = program.interface.clone();
        with_depth.outputs[0].system = Some(SystemValue::Depth);
        assert_eq!(output_bindings(&with_depth, Some(1), true).len(), 10);
    }

    /// The lanes of targets 2 and 3 follow the shading class the lighting pass decodes them with (material-spec §15.4):
    /// layout B has the specular occlusion in `o2.z` and the translucency in `o2.w` and its `o3` is read by nothing, and a
    /// template with no class in the notes' table has them all Unknown.
    #[test]
    fn the_lanes_of_targets_2_and_3_follow_the_shading_class() {
        let program = primase_dxbc::lift(LIFTED).unwrap();
        let of = |class: Option<u8>, target: u32| -> Vec<(u8, OutputRole)> {
            output_bindings(&program.interface, class, true)
                .into_iter()
                .filter(|o| {
                    program.interface.outputs.iter().position(|v| v.label == o.label)
                        == Some(usize::try_from(target).unwrap())
                })
                .map(|o| (o.lanes, unknown_as_one(o.role)))
                .collect()
        };
        assert_eq!(
            of(Some(5), 2),
            [
                (0b0011, OutputRole::MotionVector),
                (0b0100, OutputRole::SpecularOcclusion),
                (0b1000, OutputRole::Translucency),
            ]
        );
        assert_eq!(of(Some(5), 3), [(0b1111, OutputRole::unknown("…"))]);
        assert_eq!(of(None, 2), [(0b1111, OutputRole::unknown("…"))]);
        assert_eq!(of(None, 3), [(0b1111, OutputRole::unknown("…"))]);
    }

    /// The G-buffer read as a surface (`primase_core::shader::surface`), the one reading a viewer's composite takes, by
    /// shading class: target 0 the albedo and, opaque, the roughness in w; target 1 the diamond normal and, in w, the
    /// metalness for class 1 or the occlusion of every light for classes 3 to 5; class 1's emission is target 2's z,
    /// one lane that scales the albedo. A blended material has no roughness, and the classes 3 to 5 no metalness and
    /// no emission.
    #[test]
    fn the_gbuffer_is_read_as_a_surface_by_shading_class() {
        use primase_core::shader::surface::{Base, Emission, Place, Roughness, Surface, Term};
        let program = primase_dxbc::lift(LIFTED).unwrap();
        let read = |class: Option<u8>, opaque: bool| -> (Surface, Vec<u32>) {
            let outputs = output_bindings(&program.interface, class, opaque);
            let targets = outputs
                .iter()
                .map(|o| {
                    let n = program
                        .interface
                        .outputs
                        .iter()
                        .position(|v| v.label == o.label)
                        .unwrap();
                    u32::try_from(n).unwrap()
                })
                .collect();
            (Surface::read(outputs.iter().map(|o| (o.lanes, &o.role))), targets)
        };
        let at = |targets: &[u32], place: Place| (targets[place.output], place.lanes);
        let linear = ScalarEncoding::LINEAR;

        let (metal, targets) = read(Some(1), true);
        assert!(metal.refused.is_empty(), "{:?}", metal.refused);
        let Some(Base::Albedo(albedo)) = metal.base else {
            panic!("an albedo: {:?}", metal.base)
        };
        assert_eq!(at(&targets, albedo), (0, 0b0111));
        let Some(Roughness::Roughness(roughness)) = metal.roughness else {
            panic!("a roughness: {:?}", metal.roughness)
        };
        assert_eq!(
            (at(&targets, roughness.place), roughness.encoding),
            ((0, 0b1000), linear)
        );
        let normal = metal.normal.unwrap();
        assert_eq!(
            (at(&targets, normal.place), normal.encoding),
            ((1, 0b0111), NormalEncoding::OctahedralDiamond)
        );
        let metallic: Term = metal.metallic.unwrap();
        assert_eq!(at(&targets, metallic.place), (1, 0b1000));
        let Some(Emission::OfAlbedo(emission)) = metal.emission else {
            panic!("one lane of emission: {:?}", metal.emission)
        };
        assert_eq!(at(&targets, emission.place), (2, 0b0100));
        assert_eq!(at(&targets, metal.occlusion.unwrap().place), (3, 0b0001));
        assert_eq!((metal.reflectance, metal.specular), (None, None));
        // the diamond normal decodes what its encoder stores, the emission scales the albedo
        let lanes =
            NormalEncoding::encode_octahedral_diamond([0.0, 0.0, 1.0], 5).map(|b| f32::from(b) / 255.0);
        let up = normal.decode([lanes[0], lanes[1], lanes[2], 0.4]).unwrap();
        assert!(up[2] > 0.999, "{up:?}");
        assert_eq!(
            metal
                .emission
                .unwrap()
                .decode([0.0, 0.0, 2.0, 0.0], [0.25, 0.5, 1.0]),
            [0.5, 1.0, 2.0]
        );

        let (rock, targets) = read(Some(5), true);
        assert_eq!(at(&targets, rock.occlusion.unwrap().place), (1, 0b1000));
        assert_eq!((rock.metallic, rock.emission), (None, None));
        assert!(rock.roughness.is_some() && rock.normal.is_some() && rock.base.is_some());

        let (blended, _) = read(Some(5), false);
        assert_eq!(blended.roughness, None);
        let roles = output_bindings(&program.interface, Some(4), false);
        assert!(
            !roles.iter().any(|o| o.role == OutputRole::Translucency),
            "blended foliage coverage must not be lit as opaque transmission"
        );
    }

    /// Only the lanes a program reads of the engine's buffers are bound, each with the role the notes establish or an
    /// Unknown with a reason; the rows it does not read are counted as left out. Held to the lanes the lifted program
    /// takes, on the lifter's D3D fixture.
    #[test]
    fn the_engine_buffers_bind_the_lanes_the_program_reads() {
        let program = primase_dxbc::lift(LIFTED).unwrap();
        let reads = program.constant_reads().unwrap();
        let m = material(Vec::new(), Vec::new(), Vec::new());
        let mut found_buffer = false;
        for r in &program.interface.resources {
            let (ResourceKind::ConstantBuffer { layout }, Some(bind)) = (&r.kind, r.provenance.d3d) else {
                continue;
            };
            if bind.register == 3 || layout.size == 0 {
                continue;
            }
            found_buffer = true;
            let mut counts = BTreeMap::new();
            let bound = engine_constants(bind.register, r.id, layout.size, Ok(&reads), &m, &mut counts);
            let buffer = reads.buffer_at(0, bind.register).unwrap();
            let (rows, dynamic) = (reads.rows(buffer), reads.indexed(buffer));
            if dynamic {
                assert_eq!(bound.len(), 1);
                assert_eq!(bound[0].size, layout.size);
                continue;
            }
            // every lane read is inside a binding, and a binding of an Unknown run holds only lanes that were read
            for row in &rows {
                for lane in reads.lanes(buffer, *row) {
                    let at = row * 16 + lane * 4;
                    assert!(
                        bound.iter().any(|c| c.offset <= at && at < c.offset + c.size),
                        "cb{} row {row} lane {lane} is read and not bound",
                        bind.register
                    );
                }
            }
            for c in bound
                .iter()
                .filter(|c| matches!(c.role, ConstantRole::Unknown { .. }))
            {
                for at in (c.offset..c.offset + c.size).step_by(4) {
                    assert!(
                        reads.reads(buffer, at, 4),
                        "cb{} byte {at} is bound Unknown and not read",
                        bind.register
                    );
                }
            }
            let left_out = counts
                .get("rows of the engine's constant buffers the program does not read, left out")
                .copied()
                .unwrap_or(0);
            assert_eq!(left_out, u64::from(layout.size / 16) - rows.len() as u64);
        }
        assert!(found_buffer, "the fixture declares no engine constant buffer");
    }

    /// The rows the notes establish have roles, and a tile is bound whole when a program reads any lane of it.
    #[test]
    fn the_established_rows_have_roles() {
        let mut m = material(Vec::new(), Vec::new(), Vec::new());
        m.flags.alpha_test_value = 128;
        m.flags.booleans[MaterialFlags::ALPHA_TEST_ENABLED] = 1;
        let mut counts = BTreeMap::new();
        // a program that reads cb2[18].y gets the Material's reference, the 128 / 255 of a Material with the test on
        let reads = primase_dxbc::lift(LIFTED).unwrap().constant_reads().unwrap();
        let _ = engine_constants(2, ResourceId(0), 18 * 16 + 16, Ok(&reads), &m, &mut counts);
        let tile = |reference: f32| engine_tile(2, 18, reference).remove(0);
        assert_eq!(
            tile(128.0 / 255.0).2,
            ConstantRole::Material(vec![(128.0f32 / 255.0).to_bits()])
        );
        assert_eq!(
            engine_tile(1, 37, 0.0),
            vec![
                (0, 2, ConstantRole::View(ViewParameter::InverseViewportSize)),
                (2, 2, ConstantRole::View(ViewParameter::ViewportSize)),
            ]
        );
        assert_eq!(
            engine_tile(1, 39, 0.0).remove(0),
            (0, 1, ConstantRole::Frame(FrameParameter::Exposure))
        );
        assert!(engine_tile(0, 4, 0.0).is_empty());
        // the clip matrix and the previous frame's are four rows each, whichever row a program reads
        for row in 8..12 {
            assert_eq!(
                engine_matrix(2, row),
                Some((8, ObjectParameter::WorldViewProjection))
            );
        }
        for row in 12..16 {
            assert_eq!(
                engine_matrix(2, row),
                Some((
                    12,
                    ObjectParameter::PreviousFrame(ObjectMatrix::WorldViewProjection)
                ))
            );
        }
        assert_eq!(engine_matrix(2, 7), None);
        assert_eq!(engine_matrix(1, 8), None);
        assert!(engine_reason(2, 16, 0).is_some());
        assert!(engine_reason(2, 16, 2).is_none());
    }
}
