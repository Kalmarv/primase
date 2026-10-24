//! Sekiro's binding rules for a layered material's G-buffer pixel program: what each constant, texture slot and output
//! of the program stands for, as a `core.shader_binding` stage. The one home of those rules: slot resolution (a
//! numeric slot named twice prefers the texture the MTD declares), the material's SAT parameters and texture tiling
//! as the constants' own words, the engine's per-view, per-frame and per-object values as roles (never values), and
//! the G-buffer layout of the outputs.
use crate::materials::{Binding, LayeredMaterial, MaterialView, ParameterValue};
use crate::metaparam::Metaparam;
use crate::{Result, invalid};
use primase_core::Entity;
use primase_core::shader::{
    Blend, BlendEquation, BlendFactor, BlendOp, Compare, ConstantBinding, ConstantBufferBinding,
    ConstantRole, DepthState, FrameParameter, FrameTexture, InputBinding, InputRole, LaneStream, MatrixLines,
    ObjectParameter, OutputBinding, OutputRole, RenderState, ResourceId, ResourceKind, SamplerState, Shader,
    Stage, StageBinding, TargetState, TextureBinding, TextureSource, ViewParameter,
};
use primase_shader::matrix::{Unshown, state_lines};
use primase_shader::{D3dClass, Interface, Program, Resource, ScalarType, SystemValue};
use primase_shader_exec::ProgramShader;
use primase_shader_exec::{Layout, PixelShader};
use std::collections::{BTreeMap, BTreeSet};

/// The stream that carries a layered mesh's second stored vertex colour (the element the FLVER indexes 1, which the
/// layered vertex programs read as `COLOR1`): `Color(0)` is the first colour of a glTF mesh and `Color(1)` to `Color(6)`
/// are the Byte4C members, so the second colour is kept apart from both. Float values as decoded, four lanes.
pub const SECOND_COLOUR: primase_core::Semantic = primase_core::Semantic::Color(9);

/// The Limgrave capture's eye group matches three MATBINs with identical defaults, not a uniquely identified
/// character. Its 17 draws with available cb3 blobs all have colour 100/255 and scalar 0.024705887. Those values do
/// not establish a material encoding or a universal runtime override; retain the definition and count the scope.
const EYE_COLOUR_DEFAULT_POLICY: &str = "eye colour lanes: the MATBIN default; the one capture shows 100/255 on every lane (perhaps character creation), not established";
const EYE_SCALAR_DEFAULT_POLICY: &str = "eye scalar input: the MATBIN default; the one capture shows 0.024705887 instead of 0 (perhaps character creation), not established";

/// What becomes of a texture slot the material binds no texture to, or whose texture file the reader does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unbound {
    /// The slot reads a stated colour for its role (a mid-grey albedo, a flat normal, zero elsewhere), counted as an
    /// approximation.
    Preview,
    /// The slot is left `Missing`: nothing is invented.
    Leave,
}

impl primase_core::pass::Choice for Unbound {
    const VALUES: &'static [(&'static str, Self)] =
        &[("preview", Unbound::Preview), ("leave", Unbound::Leave)];
}

/// What a program's texture resource is to its material: the one rule by which the resolve, the bake's executor and the
/// repeat ([`super::sampled_period`]) tell a material's texture from the engine's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProgramTexture<'r> {
    /// A texture the material binds or declares.
    Material(MaterialTexture<'r>),
    /// The engine's own texture, neither numbered nor named by the material, which binds nothing to it: the dither noise
    /// the alpha-tested permutations read for the dithered fade, a depth or scene texture of a program that reads one, the
    /// volumes and cube maps of a forward program's lighting.
    Engine(&'r str),
}

/// How a program names a material's texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterialTexture<'r> {
    /// A numbered slot, `Texture2D_<n>`.
    Slot(u32),
    /// A texture kind the material names exactly: the fixed-function (`GXFlver_*`) programs call their textures
    /// `g_DiffuseTexture`, `g_SpecularTexture`, `g_BumpmapTexture`, as the material's own kinds, not by numbered slot.
    Kind(&'r str),
}

/// What the texture `resource` is to `material` ([`ProgramTexture`]).
pub(crate) fn program_texture<'r>(material: &MaterialView<'_>, resource: &'r Resource) -> ProgramTexture<'r> {
    let name = resource.provenance.name.as_str();
    match texture_slot(resource) {
        Some(slot) => ProgramTexture::Material(MaterialTexture::Slot(slot)),
        None if material.bindings.contains_key(name) || material.definition.declares(name) => {
            ProgramTexture::Material(MaterialTexture::Kind(name))
        }
        None => ProgramTexture::Engine(name),
    }
}

/// The role a texture kind named without a slot number has, from its name: a diffuse texture is the albedo, a bump map the
/// normal map, anything else carries its name's own role.
fn kind_role(name: &str) -> &'static str {
    if name.contains("Diffuse") {
        "AlbedoMap"
    } else if name.contains("Bump") {
        "NormalMap"
    } else {
        "OtherMap"
    }
}

/// A material texture's role and the material's binding for it: for a numbered slot by the slot's number (a declared slot
/// retains its role even without a path: an unbound normal gets the flat normal, not the zero of an unknown resource),
/// for a fixed-function texture by the kind's own name.
pub(crate) fn role_and_binding<'a>(
    material: &MaterialView<'a>,
    slots: &BTreeMap<u32, (&'a str, &'a Binding)>,
    texture: MaterialTexture<'_>,
) -> Result<(&'a str, Option<&'a Binding>)> {
    let binding = material_binding(material, slots, texture);
    Ok(match texture {
        MaterialTexture::Slot(slot) => {
            let fallback_role = declared_role(material, slot)?;
            (slots.get(&slot).map_or(fallback_role, |(role, _)| *role), binding)
        }
        MaterialTexture::Kind(name) => (kind_role(name), binding),
    })
}

/// The material's binding for a material texture: a numbered slot's by [`slots`], a fixed-function texture's by the kind's
/// own name.
pub(crate) fn material_binding<'a>(
    material: &MaterialView<'a>,
    slots: &BTreeMap<u32, (&'a str, &'a Binding)>,
    texture: MaterialTexture<'_>,
) -> Option<&'a Binding> {
    match texture {
        MaterialTexture::Slot(slot) => slots.get(&slot).map(|(_, b)| *b),
        MaterialTexture::Kind(name) => material.bindings.get(name),
    }
}

/// The count of a program whose material fields (`_snp_`) none of the material's parameters reach, by name or through
/// `SAT_MtdParam`: those fields keep the program's zeros.
pub const UNBOUND_MATERIAL_FIELDS: &str =
    "programs with material fields bound to none of the material's parameters: left at frozen zero";

/// The name of the engine's dither noise texture in the alpha-tested permutations (`<shader>_<switch>_Gbuf_[A].ppo`).
pub const ENGINE_DITHER_TEXTURE: &str = "g_DitherTexture";

/// The name of the scene depth texture the forward programs of fog, foam and water read (`t56`: the hardware depth in x,
/// linearised with `FC_DepthComputeParam`: view depth `-z / (x - depth)`): the frame's, bound as one.
pub const ENGINE_SCENE_DEPTH_TEXTURE: &str = "g_DepthTexture";

/// `<index>` and role of a texture kind named `…Texture2D_<index>_<Role>…`: the first named segment after the index, past
/// a blend shader's `GSBlendMap` prefix (`…Texture2D_2_GSBlendMap_AlbedoMap_1` is an albedo map, as both games' blend
/// shaders name their slots; a slot named only `GSBlendMap` is the blend map) and past the empty segments of a kind such
/// as `…Texture2D_2__DistortionDepth`. A kind naming no role (`…Texture2D_3_`) is still its slot, with an empty role.
///
/// The evidence is the files' formats: in Elden Ring's m14_00_00_00 the 96 `GSBlendMap_AlbedoMap` bindings all point at
/// DX10 `*_SRGB` textures and the 57 DX10 `GSBlendMap_NormalMap` ones at linear textures, as the plain albedo and normal
/// maps do; in Sekiro's m11_01 no binding changes role. No runtime capture confirms it yet (the Sekiro D3D11 capture
/// records render-target and depth formats only, not the views a program samples).
///
/// The index is [`crate::metaparam::texture_number`]'s, the one parse of a binding key's slot the reader's unbound rule
/// ([`crate::metaparam::unbound_slots`]) also uses, so a key binds the same slot on both sides.
pub(crate) fn slot_name(kind: &str) -> Option<(u32, &str)> {
    let slot = crate::metaparam::texture_number(kind)?;
    let rest = kind.split_once("Texture2D_")?.1;
    let roles = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let mut segments = roles.split('_').filter(|s| !s.is_empty());
    let role = match segments.next() {
        Some(blend @ "GSBlendMap") => segments.next().unwrap_or(blend),
        Some(role) => role,
        None => "",
    };
    Some((slot, role))
}

/// The role a slot's declared texture has in the MTD, `?` if it declares none; two declarations that disagree are an
/// error.
pub(crate) fn declared_role<'a>(material: &MaterialView<'a>, slot: u32) -> Result<&'a str> {
    let mut role = None;
    for kind in material.definition.texture_kinds() {
        if let Some((index, found)) = slot_name(kind)
            && index == slot
        {
            if role.is_some_and(|r| r != found) {
                return Err(invalid("layer binding", 0, "conflicting declared texture roles"));
            }
            role = Some(found);
        }
    }
    Ok(role.unwrap_or("?"))
}

/// The material's bindings by numeric slot, each with its role. A slot named by two texture kinds takes the one the
/// MTD declares (the other bindings stay in the source material).
pub(crate) fn slots<'a>(material: &MaterialView<'a>) -> Result<BTreeMap<u32, (&'a str, &'a Binding)>> {
    let mut slots = BTreeMap::new();
    for (kind, binding) in material.bindings {
        if !binding.scale.iter().all(|v| v.is_finite()) {
            return Err(invalid("layer binding", 0, "nonfinite texture scale"));
        }
        if let Some((slot, role)) = slot_name(kind) {
            let declared = material.definition.declares(kind);
            if !slots.contains_key(&slot) || declared {
                slots.insert(slot, (role, binding));
            }
        }
    }
    Ok(slots)
}

/// The texture resources of a program's interface.
pub(crate) fn textures_of(interface: &Interface) -> impl Iterator<Item = &Resource> {
    interface
        .resources
        .iter()
        .filter(|r| matches!(r.kind, ResourceKind::Texture { .. }))
}

/// A program resource's `Texture2D_<n>` number, by the one parse of it ([`crate::metaparam::texture_number`]).
fn texture_slot(resource: &Resource) -> Option<u32> {
    crate::metaparam::texture_number(&resource.provenance.name)
}

/// A resource's D3D register.
pub(crate) fn register(resource: &Resource) -> Result<u32> {
    resource
        .provenance
        .d3d
        .map(|b| b.register)
        .ok_or_else(|| invalid("layer binding", 0, "a resource without a D3D register"))
}

/// The value a texture slot with no texture reads, for its role.
fn preview_colour(role: &str) -> [f32; 4] {
    match role {
        "AlbedoMap" => [0.5, 0.5, 0.5, 1.],
        "NormalMap" => [0.5, 0.5, 1., 1.],
        _ => [0.; 4],
    }
}

/// What resolving a layered material's program gave: the pixel stage's binding, and what it counted.
#[derive(Debug)]
pub struct Resolution {
    pub stage: StageBinding,
    /// Counted approximations (a preview colour for a slot with no texture), by name.
    pub policies: BTreeMap<String, u64>,
}

/// The fixed-function state Sekiro draws a material with, as far as a capture of the running game settles it (one session of
/// 222,552 draws of 180 pixel programs in a castle interior, with each draw's blend, depth-stencil and rasterizer state read
/// back from the D3D11 context and matched to the MTDs by program and material buffer):
///
/// - depth test `GreaterEqual` in every pass (the engine's depth is reversed); depth write on, except where the material states
///   `g_GX_bDisableDepthWrite` (every blended draw matched to a material that states it wrote none, and every one matched to
///   one that does not wrote depth);
/// - blend `SrcAlpha`/`InvSrcAlpha` (add) on colour and `One`/`Zero` on alpha for `g_BlendMode` 2, render target 0, all
///   channels written (the forward passes of the materials with that mode: cloud and ground fog, the moon, GXFlver `_al`); the
///   G-buffer pass has no blending.
///
/// Not stated here, each for a reason the capture gives: the alpha test is the program's own (`discard` against
/// `FC_AlphaRef`: kept where alpha is at least the reference, bound as a constant), so no fixed-function test is stated; the
/// cull follows the face set (`fromsoft.face_cull`: the same material drew with both), not the material; the winding
/// convention is the lowering's (it repairs windings from the stored normals); the stencil is the engine's per pass (a
/// reference per model class, depth-equal passes after a prepass), the depth bias was set on some alpha-tested programs'
/// draws (3 slope-scaled) with no matched field that predicts it, and blend modes 3, 4 and 5 were matched to no draw.
pub fn render_state(material: &LayeredMaterial) -> RenderState {
    let mut state = RenderState {
        depth: Some(DepthState {
            test: Compare::GreaterEqual,
            write: !material.depth_write_disabled(),
        }),
        ..RenderState::default()
    };
    if material.blend_mode() == 2 {
        let blend = Blend {
            colour: BlendEquation {
                source: BlendFactor::SrcAlpha,
                destination: BlendFactor::OneMinusSrcAlpha,
                op: BlendOp::Add,
            },
            alpha: BlendEquation {
                source: BlendFactor::One,
                destination: BlendFactor::Zero,
                op: BlendOp::Add,
            },
        };
        state.blend = Some(blend);
        state.targets[0] = Some(TargetState {
            blend: Some(blend),
            write_mask: 0xf,
        });
    }
    state
}

/// Bind the pixel program `shader` (entity `entity`) of `material` (an MTD's or a material binary's, [`MaterialView`]),
/// a program of `game` (its G-buffer layout: [`crate::Game::gbuffer`]). `metaparam` is the parameter definition of its
/// shader's bundle, which states the default of a parameter the material does not, where each parameter sits in an
/// array-addressed program, and each texture's uv group. `texture` finds the entity of a texture by stem and colour
/// space (`true`: sRGB, the albedo maps), or `None` when the reader holds no such file.
///
/// A program's own layout says how it reads the material's parameters: Sekiro's name each one as a field
/// (`<family>_snp_*`, the name the material and the metaparam give the parameter), Elden Ring's read them from one array,
/// `SAT_MtdParam`, where the metaparam places each at its start word in the buffer (absolute: the array starts at word
/// 36 of the 156-byte `cbMtdParam` and the first parameter's start is 36). A program with material fields that binds
/// none of the material's parameters by either is counted, never left at zero unsaid.
pub fn resolve(
    shader: &ProgramShader,
    entity: Entity,
    material: &MaterialView<'_>,
    game: crate::Game,
    metaparam: Option<&Metaparam>,
    unbound: Unbound,
    texture: &mut dyn FnMut(&str, bool) -> Result<Option<Entity>>,
) -> Result<Resolution> {
    let interface = PixelShader::interface(shader);
    let layout = Layout::of(shader).map_err(|e| invalid("layer binding", e.at, e.detail))?;
    let buffer_of = |slot: u32| -> Option<(ResourceId, u32)> {
        interface.resources.iter().find_map(|r| match &r.kind {
            ResourceKind::ConstantBuffer { layout }
                if r.provenance
                    .d3d
                    .is_some_and(|b| b.class == D3dClass::ConstantBuffer && b.register == slot) =>
            {
                Some((r.id, layout.size))
            }
            _ => None,
        })
    };
    let mut policies = BTreeMap::new();
    let mut constants = Vec::new();
    // the program's value graph, read once: what states a matrix's lines and an alpha test's reference
    let graph = shader.program().graph();
    // The producer establishes the dither role; the generic query sees only graph resource ids and words.
    let discard_compares = |name: &str| {
        let field = layout.fields.get(name)?;
        let graph = graph.as_ref().ok()?;
        let bind_point = buffer_of(field.buffer)
            .and_then(|(id, _)| interface.resource(id))
            .and_then(|r| r.provenance.d3d)?;
        let buffer = graph.reads.buffer_at(bind_point.space, bind_point.register)?;
        let dither = textures_of(interface).find_map(|r| {
            if !matches!(program_texture(material, r), ProgramTexture::Engine(name) if name == ENGINE_DITHER_TEXTURE) {
                return None;
            }
            let point = r.provenance.d3d?;
            let texture = graph.interface.resources.iter().find(|g| g.provenance.d3d == Some(point))?.id;
            Some(primase_shader::output_uses::AlphaDither { texture, scale_word: field.offset + 1 })
        });
        primase_shader::output_uses::discard_compares(graph, buffer, field.offset, dither)
    };
    // a constant of the layout, at `word` words into it, `size` bytes (cut where its buffer ends), by name
    let mut constant = |name: &str, word: usize, size: u32, mut role: ConstantRole| -> Result<()> {
        let Some(&primase_shader_exec::Field { buffer, offset, .. }) = layout.fields.get(name) else {
            return Ok(());
        };
        let Some((resource, buffer_size)) = buffer_of(buffer) else {
            return Ok(());
        };
        let start = (offset as usize + word) as u64 * 4;
        let fit = u64::from(buffer_size).saturating_sub(start).min(u64::from(size)) as u32;
        if fit == 0 {
            return Ok(());
        }
        if let ConstantRole::Material(words) = &mut role {
            words.truncate(fit as usize / 4);
        }
        constants.push(ConstantBinding {
            resource,
            offset: start as u32,
            size: fit,
            name: Some(name.to_owned()),
            role,
            lines: None,
        });
        Ok(())
    };
    let words = |values: &[f32]| ConstantRole::Material(values.iter().map(|v| v.to_bits()).collect());
    let slots = slots(material)?;
    let mut bindings = Vec::new();
    for resource in textures_of(interface) {
        let ResourceKind::Texture {
            dim,
            arrayed,
            multisampled,
            ..
        } = resource.kind
        else {
            unreachable!("textures_of yields textures")
        };
        let individual_2d =
            resource.count == 1 && dim == primase_core::shader::TextureDim::D2 && !arrayed && !multisampled;
        let register = register(resource)?;
        // the engine's own texture: the dither noise a neutral grey (the fade is off), the scene depth the frame's, any
        // other zero; each is counted by name
        let named = match program_texture(material, resource) {
            ProgramTexture::Material(named) => named,
            ProgramTexture::Engine(name) => {
                if name == ENGINE_SCENE_DEPTH_TEXTURE {
                    bindings.push(TextureBinding {
                        resource: resource.id,
                        sampler: None,
                        source: TextureSource::Frame(FrameTexture::SceneDepth),
                    });
                    continue;
                }
                let (value, what) = if name == ENGINE_DITHER_TEXTURE {
                    ([0.5; 4], "read only by the dithered alpha fade, which is off")
                } else {
                    ([0.; 4], "not a material slot, left at zero")
                };
                let why = format!("engine texture {name}: {what}");
                *policies.entry(why.clone()).or_default() += 1;
                bindings.push(TextureBinding {
                    resource: resource.id,
                    sampler: None,
                    source: TextureSource::Default { value, why },
                });
                continue;
            }
        };
        if !individual_2d {
            return Err(invalid(
                "layer binding",
                0,
                format!(
                    "{}: a material texture slot that is not an individual Texture2D",
                    resource.provenance.name
                ),
            ));
        }
        let (role, binding) = role_and_binding(material, &slots, named)?;
        let found = match binding {
            Some(b) => texture(&b.texture, role == "AlbedoMap")?,
            None if game == crate::Game::EldenRing => {
                let found = match default_texture(material, metaparam, resource) {
                    Some(d) => {
                        let found = texture(&d, role == "AlbedoMap")?;
                        // the engine binds this default; a read that did not keep it is a different fact from a slot
                        // with no default, counted before the unbound policy below applies
                        if found.is_none() {
                            *policies
                                .entry(format!("metaparam default named but not loaded: {d}"))
                                .or_default() += 1;
                        }
                        found
                    }
                    None => None,
                };
                if found.is_some() {
                    *policies
                        .entry(
                            "material slots bound to the metaparam's default texture (the material binds nothing there)"
                                .to_owned(),
                        )
                        .or_default() += 1;
                }
                found
            }
            None => None,
        };
        // the slot's tiling, in its row of FC_TextureTilingScale, as both games' captures show it ([`tiling`])
        constant(
            "FC_TextureTilingScale",
            register as usize * 4,
            16,
            words(&tiling(material, metaparam, resource, binding)),
        )?;
        let source = match (found, unbound) {
            (Some(texture), _) => TextureSource::Texture {
                texture,
                // the addressing the FLVER stores for this binding; the source states no filters
                sampler: SamplerState {
                    wrap: [
                        binding.and_then(|b| b.address[0]),
                        binding.and_then(|b| b.address[1]),
                        None,
                    ],
                    ..SamplerState::default()
                },
            },
            (None, Unbound::Preview) => {
                let why = format!("preview default for missing {role} binding");
                *policies.entry(why.clone()).or_default() += 1;
                TextureSource::Default {
                    value: preview_colour(role),
                    why,
                }
            }
            (None, Unbound::Leave) => TextureSource::Missing,
        };
        bindings.push(TextureBinding {
            resource: resource.id,
            sampler: None,
            source,
        });
    }
    for &slot in PixelShader::textures(shader) {
        // a texture register the program reads names a resource: a texture, or the structured buffer a forward program's
        // lights are read from
        if !interface.resources.iter().any(|r| {
            !matches!(
                r.kind,
                ResourceKind::ConstantBuffer { .. } | ResourceKind::Sampler { .. }
            ) && register(r).is_ok_and(|reg| reg == slot)
        }) {
            return Err(invalid(
                "layer binding",
                0,
                format!("t{slot} has no named resource"),
            ));
        }
    }
    // the material buffer's engine-named fields, from the MTD's engine-named parameters: the colour the diffuse map is
    // multiplied by is `g_DiffuseMapColor`'s rgb (and 0), the alpha test's reference `g_AlphaRef`'s first word over 255
    // when it has one and -100 (never discards) when it has none, its second word over 255 (the second is not seen in
    // Sekiro's data). The captured contents of this buffer vary per material in exactly these fields
    // ((1, 0.9216, 0.9216, 0) beside (1, 1, 1, 0); 0.502 = 128 / 255 and -100 in FC_AlphaRef.x). Its z and w are a flag
    // the engine derives (0 or 1 in the capture) whose source is not established.
    let definition_param = |name: &str| material.definition.parameter(name);
    match definition_param("g_DiffuseMapColor") {
        Some(ParameterValue::Float(v)) if v.len() >= 3 => {
            constant("FC_DifMapMulCol", 0, 16, words(&[v[0], v[1], v[2], 0.]))?
        }
        _ => constant(
            "FC_DifMapMulCol",
            0,
            16,
            ConstantRole::Unknown {
                why: "the material states no g_DiffuseMapColor, the colour its diffuse map is multiplied by"
                    .into(),
            },
        )?,
    }
    let reference = |word: usize| match definition_param("g_AlphaRef") {
        Some(ParameterValue::Int(v)) => v.get(word).copied().and_then(crate::materials::alpha_reference),
        _ => None,
    };
    // its x the alpha test's reference where the program's discard compares a value it writes with it (the material's,
    // or the engine's -100 where it states none); otherwise both words the material's
    let (x, y) = (reference(0).unwrap_or(-100.), reference(1).unwrap_or(0.));
    if let Some(compared) = discard_compares("FC_AlphaRef")
        && y.is_finite()
    {
        // The dither texture's bake answer above is exactly 0.5. For the proved centred-product expression the
        // perturbation is therefore zero for this finite scale; freezing the reference changes no retained alpha.
        constant(
            "FC_AlphaRef",
            0,
            4,
            ConstantRole::AlphaReference { value: x, compared },
        )?;
        constant("FC_AlphaRef", 1, 4, words(&[y]))?;
    } else {
        constant("FC_AlphaRef", 0, 8, words(&[x, y]))?;
    }
    // the glow scale: read only in its w, into the emission target's alpha (all 34 programs that read it), which no bake
    // reads; the material's g_GlowScale where it states one (a four-float parameter), Unknown, by name, where it does not
    match definition_param("g_GlowScale") {
        Some(ParameterValue::Float(v)) if v.len() >= 4 => constant("FC_GlowScale", 0, 16, words(&v[..4]))?,
        _ => constant(
            "FC_GlowScale",
            0,
            16,
            ConstantRole::Unknown {
                why: "the material states no g_GlowScale: read only into the emission target's alpha".into(),
            },
        )?,
    }
    constant(
        "FC_AlphaRef",
        2,
        8,
        ConstantRole::Unknown {
            why:
                "the alpha test's flag the engine derives (zw of FC_AlphaRef): its source is not established"
                    .into(),
        },
    )?;
    // the engine's values: roles, whatever a consumer freezes them to
    constant("mWorld", 0, 48, ConstantRole::Object(ObjectParameter::World))?;
    // the instance record's environment data word: it reaches only G-buffer alpha lanes ([`super::INSTANCE_DATA_POLICY`])
    constant(
        "envData",
        0,
        4,
        ConstantRole::Object(ObjectParameter::EnvironmentData),
    )?;
    constant(
        "aColor",
        0,
        16,
        ConstantRole::Object(ObjectParameter::InstanceColour),
    )?;
    constant(
        "FC_ColorSlotNo",
        0,
        4,
        ConstantRole::Frame(FrameParameter::ColourSlot),
    )?;
    // the dither and camera-fade fields an alpha-tested program reads from the per-draw buffers (cbMatDynParam and
    // cbModelParam, all 102 `_[A]` programs): the engine's per-draw state, never the material's (the MTD's
    // g_GX_DisableCameraFade only allows the fade). Each is a role; what a consumer freezes them to is its own
    constant(
        "FC_isDitherAlpha",
        0,
        4,
        ConstantRole::Object(ObjectParameter::DitherAlphaEnabled),
    )?;
    constant(
        "FC_isEnableCameraDitherFade",
        0,
        4,
        ConstantRole::Object(ObjectParameter::CameraFadeEnabled),
    )?;
    constant(
        "FC_DitherEnable",
        0,
        4,
        ConstantRole::Object(ObjectParameter::DitherEnabled),
    )?;
    constant(
        "FC_CameraFadeEnd",
        0,
        4,
        ConstantRole::Object(ObjectParameter::CameraFadeEnd),
    )?;
    constant(
        "FC_CameraFadeInvRange",
        0,
        4,
        ConstantRole::Object(ObjectParameter::CameraFadeInverseRange),
    )?;
    constant(
        "FC_MatrixInvViewProj",
        0,
        64,
        ConstantRole::View(ViewParameter::InverseViewProjection),
    )?;
    constant(
        "SC_RenderCameraPos",
        0,
        16,
        ConstantRole::View(ViewParameter::CameraPosition),
    )?;
    constant("FC_GlobalTime", 0, 4, ConstantRole::Frame(FrameParameter::Time))?;
    constant(
        "FC_TimeOfDay",
        0,
        4,
        ConstantRole::Frame(FrameParameter::TimeOfDay),
    )?;
    constant(
        "SC_PreExposure",
        0,
        4,
        ConstantRole::Frame(FrameParameter::Exposure),
    )?;
    // the program's `SAT_MtdParam` (Elden Ring's): its buffer, the buffer's size and the words its members occupy. Each
    // member is a component of a parameter the program reads (`<parameter>_<component>`, at the parameter's start word);
    // a parameter it does not read is not a member
    let sat = interface.resources.iter().find_map(|r| match &r.kind {
        ResourceKind::ConstantBuffer { layout } => layout
            .constants
            .iter()
            .find(|c| c.name == "SAT_MtdParam")
            .map(|c| {
                let read: BTreeSet<u32> =
                    c.ty.members()
                        .iter()
                        .filter_map(|m| {
                            let at = c.offset.checked_add(m.offset)?;
                            Some(at / 4..at.checked_add(m.ty.size()?)?.div_ceil(4))
                        })
                        .flatten()
                        .collect();
                (r.id, layout.size, read)
            }),
        _ => None,
    });
    // the material's own parameters, of a program that names each as a field (Sekiro's): its `_snp_` fields, when one
    // of them is a parameter the material states or the bundle's metaparam declares. The three name a parameter alike,
    // not after the material's shader: a `_cloth` shader's material and program both name its parameters with the
    // base family's prefix (`P_BD_M_9040_tops1_Maffler_cloth.mtd`, shader `Character_AMSN_[DetailBlend]_cloth`, states
    // `Character_AMSN__DetailBlend__snp_0_color_0`, the field its bundle's `_cloth_1_Gbuf.ppo` reads at byte 384).
    // Sekiro's programs carry a `SAT_MtdParam` array beside them: the array is read only by a program none of whose
    // fields is a stated parameter (Elden Ring's: its members are `<shader>_snp_<switch>_*` where the material binary
    // and the metaparam say `<shader>__snp_*`)
    let declared = |name: &str| {
        material.definition.parameter(name).is_some()
            || metaparam.is_some_and(|m| {
                m.mtd_params.iter().any(|p| p.name == name) || m.ca_params.iter().any(|p| p.name == name)
            })
    };
    let fields: Vec<&String> = layout
        .fields
        .keys()
        .filter(|n| n.contains("_snp_") && !n.ends_with("_PADDING"))
        .collect();
    let names: Vec<&String> = if fields.iter().any(|n| declared(n)) {
        fields.clone()
    } else {
        Vec::new()
    };
    let named = !names.is_empty();
    let sat = sat.filter(|_| !named);
    let mut sat_bound = 0;
    for name in names {
        let field = layout.fields[name];
        let declared = metaparam.and_then(|m| m.mtd_params.iter().find(|p| p.name == *name));
        let Some(param) = material.definition.parameter(name) else {
            // the shader bundle's metaparam states a default for every parameter it declares: the value of one the
            // material does not state, a colour's with the default's own intensity
            if let Some(p) = declared {
                let value = if is_colour(name, declared) {
                    let intensity = intensity(
                        material,
                        name,
                        &p.default,
                        declared,
                        ParameterPath::Field,
                        &mut policies,
                    );
                    folded_colour(game, declared, &p.default, intensity)
                } else {
                    Ok(p.default.to_vec())
                };
                *policies
                    .entry(ParameterPath::Field.counted("taken from the shader bundle's metaparam default"))
                    .or_default() += 1;
                let role = match value {
                    Ok(value) => words(&value[..(field.words as usize).min(value.len())]),
                    Err(why) => ConstantRole::Unknown { why: why.into() },
                };
                constant(name, 0, field.words * 4, role)?;
            } else if let Some(p) = metaparam
                .and_then(|m| m.ca_params.iter().find(|p| p.name == *name))
                .filter(|_| field.words == 1)
            {
                // a bundle that declares CAParams (the other SAT shaders: their flags say 0x10) names each component of a
                // parameter the engine sets per draw by the program's own field name, with the value it has when none
                // is set
                *policies
                    .entry(ParameterPath::Field.counted("taken from the shader bundle's CAParam default"))
                    .or_default() += 1;
                constant(name, 0, 4, words(&[p.default]))?;
            } else {
                constant(
                    name,
                    0,
                    field.words * 4,
                    ConstantRole::Unknown {
                        why: ParameterPath::Field.counted("left at frozen zero"),
                    },
                )?;
            }
            continue;
        };
        let mut values = floats(&param);
        if is_colour(name, declared) {
            let intensity = intensity(
                material,
                name,
                &values,
                declared,
                ParameterPath::Field,
                &mut policies,
            );
            match folded_colour(game, declared, &values, intensity) {
                Ok(colour) => values = colour,
                Err(why) => {
                    constant(
                        name,
                        0,
                        field.words * 4,
                        ConstantRole::Unknown { why: why.into() },
                    )?;
                    continue;
                }
            }
        }
        constant(name, 0, values.len() as u32 * 4, words(&values))?;
    }
    // a program that reads its parameters from `SAT_MtdParam`: each the metaparam declares as a shader input and the
    // program reads (its start word a member's), at that word, the material's value (else the metaparam's default,
    // counted), as many lanes as the program reads from there, written as the engine writes it ([`sat_words`]), a
    // colour's with its intensity ([`intensity`])
    if let (Some((resource, buffer_size, read)), Some(metaparam)) = (&sat, metaparam) {
        let (resource, buffer_size) = (*resource, *buffer_size);
        for p in metaparam.mtd_params.iter().filter(|p| !p.not_shader_input) {
            if !read.contains(&p.start) {
                *policies
                    .entry("SAT parameters the program does not read (not in its SAT_MtdParam)".to_owned())
                    .or_default() += 1;
                continue;
            }
            let lanes = match p.value_type {
                0 | 1 => 1,
                2 => 2,
                3 => 3,
                4 | 12 | 13 => 4,
                other => {
                    return Err(invalid(
                        "layer binding",
                        0,
                        format!("{}: parameter type {other} not read", p.name),
                    ));
                }
            };
            let mut value = match material.definition.parameter(&p.name) {
                Some(v) => floats(&v),
                None => {
                    *policies
                        .entry(ParameterPath::Sat.counted("taken from the shader bundle's metaparam default"))
                        .or_default() += 1;
                    p.default.to_vec()
                }
            };
            let colour = is_colour(&p.name, Some(p));
            let intensity = if colour {
                intensity(
                    material,
                    &p.name,
                    &value,
                    Some(p),
                    ParameterPath::Sat,
                    &mut policies,
                )
            } else {
                1.
            };
            if colour {
                value.resize(4, 1.);
            }
            let value = sat_words(game, p, &value, intensity);
            let lanes = (0..lanes).take_while(|k| read.contains(&(p.start + k))).count();
            let lanes = value.as_ref().map_or(lanes, |value| lanes.min(value.len()));
            let start = u64::from(p.start) * 4;
            let fit = u64::from(buffer_size).saturating_sub(start).min(lanes as u64 * 4) as u32;
            if fit == 0 {
                return Err(invalid(
                    "layer binding",
                    0,
                    format!("{}: start word {} beyond its buffer", p.name, p.start),
                ));
            }
            let role = match value {
                Ok(mut value) => {
                    value.truncate(fit as usize / 4);
                    if game == crate::Game::EldenRing {
                        let policy = match p.name.as_str() {
                            "P_ChrCustomize__Eye__snp_0_float3_2" => Some(EYE_COLOUR_DEFAULT_POLICY),
                            "P_ChrCustomize__Eye__snp_0_float_0" => Some(EYE_SCALAR_DEFAULT_POLICY),
                            _ => None,
                        };
                        if let Some(policy) = policy {
                            *policies.entry(policy.to_owned()).or_default() += value.len() as u64;
                        }
                    }
                    ConstantRole::Material(value)
                }
                Err(why) => ConstantRole::Unknown { why: why.into() },
            };
            constants.push(ConstantBinding {
                resource,
                offset: start as u32,
                size: fit,
                name: Some(p.name.clone()),
                role,
                lines: None,
            });
            sat_bound += 1;
        }
    }
    if !named && sat_bound == 0 && !fields.is_empty() {
        *policies.entry(UNBOUND_MATERIAL_FIELDS.to_owned()).or_default() += 1;
    }
    let outputs = output_bindings(interface, material.is_forward(), game);
    let inputs = interface
        .inputs
        .iter()
        .map(|v| InputBinding {
            label: v.label.clone(),
            role: InputRole::Previous,
        })
        .collect();
    // a matrix's lines as the program applies it, or why it does not show them
    state_lines(graph.as_ref().map_err(String::as_str), interface, &mut constants);
    Ok(Resolution {
        stage: StageBinding {
            stage: Stage::Pixel,
            variant: None,
            shader: entity,
            inputs,
            constants,
            textures: bindings,
            buffers: Vec::new(),
            constant_buffers: Vec::new(),
            outputs,
        },
        policies,
    })
}

/// The outputs' roles: of a G-buffer program the game's G-buffer layout ([`crate::Game::gbuffer`]), the lanes of each
/// target the program declares; a forward program's render target 0 is the finished colour, and its others (target 1,
/// drawn to a target the capture does not name) Unknown with that.
fn output_bindings(interface: &Interface, forward: bool, game: crate::Game) -> Vec<OutputBinding> {
    let layout = game.gbuffer();
    let mut out = Vec::new();
    for v in &interface.outputs {
        let Some(SystemValue::Target(n)) = v.system else {
            continue;
        };
        // the interface lists the whole signature: a target the program does not declare is not bound
        if !v.used {
            continue;
        }
        let lanes: Vec<(u8, OutputRole)> = if forward {
            vec![(
                0b1111,
                if n == 0 {
                    OutputRole::Colour
                } else {
                    OutputRole::unknown(format!(
                        "a forward program's target {n}, drawn to a target the capture does not name"
                    ))
                },
            )]
        } else {
            layout
                .iter()
                .filter(|(target, ..)| *target == n)
                .map(|(_, lanes, role)| (*lanes, role.clone()))
                .collect()
        };
        for (lanes, role) in lanes {
            if u32::from(lanes) >> v.value.width != 0 {
                continue;
            }
            out.push(OutputBinding {
                label: v.label.clone(),
                lanes,
                role,
            });
        }
    }
    out
}

/// The resolve's two ways a program reads a material's parameters, named in what it counts: a field per parameter
/// (Sekiro's programs), or the `SAT_MtdParam` array (Elden Ring's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParameterPath {
    Field,
    Sat,
}

impl ParameterPath {
    fn name(self) -> &'static str {
        match self {
            ParameterPath::Field => "material field",
            ParameterPath::Sat => "SAT parameter",
        }
    }

    /// The count of a parameter the material does not state, which this path writes as `what` says.
    fn counted(self, what: &str) -> String {
        format!("missing {} {what}", self.name())
    }
}

/// Whether the parameter `name` is a colour, four lanes and an intensity: by the metaparam's type where it declares the
/// parameter ([`crate::metaparam::MtdParam::is_colour`]), else by its name (`_color_`). The two agree on every colour of
/// Elden Ring's metaparams (3,028) and material binaries (46,000).
fn is_colour(name: &str, declared: Option<&crate::metaparam::MtdParam>) -> bool {
    declared.map_or_else(|| name.contains("_color_"), crate::metaparam::MtdParam::is_colour)
}

/// A colour's intensity, both paths' one rule: where the material states it, its fifth lane where `value` has five (Elden
/// Ring's material binaries state each of their 46,000 colours so; a metaparam default always has five), else its
/// `<name>-Intensity` parameter (Sekiro's MTDs state colours in four lanes, 7,884 of their 7,892 with that parameter).
/// One the material does not state is the metaparam default's fifth lane, else 1, each counted under `path`.
fn intensity(
    material: &MaterialView<'_>,
    name: &str,
    value: &[f32],
    declared: Option<&crate::metaparam::MtdParam>,
    path: ParameterPath,
    policies: &mut BTreeMap<String, u64>,
) -> f32 {
    if let Some(&stated) = value.get(4) {
        return stated;
    }
    if let Some(ParameterValue::Float(v)) = material.definition.parameter(&format!("{name}-Intensity"))
        && let Some(&stated) = v.first()
    {
        return stated;
    }
    let (intensity, taken) = match declared {
        Some(p) => (p.default[4], "the metaparam default's"),
        None => (1., "1, as no metaparam declares it"),
    };
    *policies
        .entry(format!(
            "{} colour stated without its intensity: {taken}",
            path.name()
        ))
        .or_default() += 1;
    intensity
}

const SEKIRO_COLOUR_DOMAIN_UNKNOWN: &str =
    "Sekiro sRGB colour outside the observed finite nonnegative RGB/intensity domain";

/// The named-field and SAT paths share this colour conversion. Alpha is unchanged (1 for a three-lane colour).
///
/// Sekiro 1.06 cb11-capture (capture SHA256 3cda409b7013ac8dce24577ae9506dd31ef18e1229961f62e8fab78fa246225c):
/// query `sekiro-constants-v2/lanes.json`, filter `class == colour_rgb`, count distinct (PS, cb3, field, lane): 84.
/// All have intensity 1. Six observations (three head colour lanes in two buffers) distinguish pow(2.2) from plain;
/// 63 match both and 15 remain unattributed. Candidates were selected by PS and cb1 tiling, never cb3 fit. RDEF
/// member offsets and Values::reached_from of outputs/discards select live words. The metaparam marks this colour
/// sRGB. The earlier 273 ambiguous nonunit contexts do not establish an ordering.
///
/// The saved Wolf programs supply reflection missing from that census. PS hashes e7df7bdee77e672f and
/// 7c1a30fd7bf346ec read cb1 rows 0..6 and cb3 RGB words 96..98, 104..106, 108..110 (Values::reached_from of
/// outputs/discards). Selecting candidates only by those live cb1 rows gives 11 contexts / 121 recorded draws
/// containing only the Court MTD pairs, whose values agree. Five distinct cb3 blobs independently show the rule:
/// e7df7bdee77e672f: 66fa849424a9b2c0, 434665e308ad2cc5;
/// 7c1a30fd7bf346ec: eb02e7ebe89f7cd5, 473fa6a5c4de33f0, eac709d04a2ed85c.
/// A sixth, e7df7bdee77e672f / c30913ef12c05dd1, has identical colour words but is not independently attributed.
/// `Character_AN_Blend_AMSN__Fresnel__snp_0_color_0` at byte 384: (1,1,0.86274517), intensity 10,
/// captured (10,10,7.22672558); `color_2` at byte 416: (0.78039223,0.64705884,0.56862748), intensity 0.5,
/// captured (0.289773375,0.191887334,0.144408002); `color_3` at byte 432: (1,1,1), intensity 0.6,
/// captured (0.600000024,0.600000024,0.600000024). These separate pow(c)*i from both c*i and pow(c*i).
/// These are CPU-uploaded material words: the conversion belongs to the declared sRGB parameter type, not to
/// a particular program or material name. Both named and SAT paths use this rule for finite nonnegative values;
/// other unresolved roles remain unknown. This is not a census of all materials.
fn folded_colour(
    game: crate::Game,
    declared: Option<&crate::metaparam::MtdParam>,
    value: &[f32],
    intensity: f32,
) -> std::result::Result<Vec<f32>, &'static str> {
    let srgb = declared.is_some_and(|p| p.srgb);
    if game == crate::Game::Sekiro
        && srgb
        && (!intensity.is_finite()
            || intensity < 0.
            || value.iter().take(3).any(|v| !v.is_finite() || *v < 0.))
    {
        return Err(SEKIRO_COLOUR_DOMAIN_UNKNOWN);
    }
    let mut lanes = value.to_vec();
    lanes.resize(4, 1.);
    for v in &mut lanes[..3] {
        if srgb {
            *v = v.powf(2.2);
        }
        *v *= intensity;
    }
    Ok(lanes)
}

/// A parameter's value as the floats a constant holds: a boolean 0 or 1, integers as floats.
fn floats(value: &ParameterValue) -> Vec<f32> {
    match value {
        ParameterValue::Bool(b) => vec![f32::from(u8::from(*b))],
        ParameterValue::Int(v) => v.iter().map(|&i| i as f32).collect(),
        ParameterValue::Float(v) => v.clone(),
    }
}

/// The Byte4C member and the lane of it that each of the eight bake inputs (four layer weights, the tint's red, green and
/// blue, the detail) is read from: MultipleGround reads its second weight from member 2's y, Multiple from its z.
pub fn layer_channels(ground: bool) -> [(u8, usize); 8] {
    [
        (1, 0),
        (2, if ground { 1 } else { 2 }),
        (4, 0),
        (5, 1),
        (3, 0),
        (3, 1),
        (3, 2),
        (6, 2),
    ]
}

/// The texture coordinate set a `VC_TexScroll_<n>` constant (the scale and scroll of set `n`, four to a model) is for.
fn texture_scroll_set(name: &str) -> Option<u8> {
    name.strip_prefix("VC_TexScroll_")?
        .parse()
        .ok()
        .filter(|n| *n < 4)
}

/// Bind a layered shader's vertex program (`<shader>_Gbuf.vpo`, the G-buffer pass of an unskinned draw, or `<shader>_Gbuf_Skin.vpo`
/// of a skinned one; the `@[cl]` permutations differ only in having no clip-distance outputs) from its
/// interface (`primase_dxbc::reflection`) as `core.shader_binding` states it:
/// - its inputs as streams of the lowered geometry: `POSITION0`, `NORMAL0` and `TANGENT0` as the position, normal and
///   tangent (and of the skinned permutation `BLENDINDICES0` and `BLENDWEIGHT0` as `Joints(0)` and `Weights(0)`); the
///   packed `TEXCOORD0` as lanes (`xy` the first UV set, `zw` the second: `TexCoord(1)`); `COLOR1` as the
///   second vertex colour ([`SECOND_COLOUR`]); `TEXCOORD1` to `TEXCOORD6` as the FLVER Byte4C members the layered
///   programs read as layer weights, tint and detail (the geometry lowering's `Color(n)` streams); the pipeline's
///   instance index as itself; anything else `Unknown` with the reason. A mesh that stores no second UV set or colour
///   has no such stream, and what the engine then feeds is not known;
/// - the engine's scene and object constants it names as roles (a named table of the shader family's own constant names
///   to the vocabulary: the view, projection and their inverses, the camera position and the screen size, and the
///   object matrices, the skinned permutation's the joints' palette: `Object(BonePalette)`, and the cloth-cancel matrices
///   some skinned programs apply before it: `Object(ClothCancelPalette)`) and the texture coordinate scale and scroll (`VC_TexScroll_0` to `_3`: `Object(TextureScroll(n))`,
///   the engine's per-model value, observed as the identity on every layered draw of a running game: see
///   [`super::TEXTURE_SCROLL_POLICY`]); constants the table does not name are not bound;
/// - no output roles: its outputs reach the pixel stage by label.
pub fn resolve_vertex(shader: &Shader, entity: Entity) -> Result<VertexBinding> {
    use super::vertex_probe::{
        CameraForm, camera_form, flat_constants, inputs_read, palette_indexed_by_vertex,
    };
    use primase_core::Semantic;
    let interface = &shader.interface;
    // the program, lifted once: what it shows of itself (its value graph and a run) is read from it, and an error stays as
    // the reason of what needs it
    let program = crate::passes::program_reads::lift(shader.format, &shader.bytecode);
    let read = program.as_ref().map_err(Clone::clone).and_then(inputs_read);
    let inputs = interface
        .inputs
        .iter()
        .map(|v| {
            let role = match (
                v.label.name.to_ascii_uppercase().as_str(),
                v.label.index,
                v.system,
            ) {
                (_, _, Some(SystemValue::InstanceIndex)) => InputRole::System,
                ("POSITION", 0, _) => InputRole::Stream(Semantic::Position),
                ("NORMAL", 0, _) => InputRole::Stream(Semantic::Normal),
                ("TANGENT", 0, _) => InputRole::Stream(Semantic::Tangent),
                // a program that reads one coordinate set takes its two lanes alone
                ("TEXCOORD", 0, _) if v.value.width <= 2 => InputRole::Stream(Semantic::TexCoord(0)),
                // the packed pair of texture coordinate sets: uv0 in `xy`, uv1 in `zw`
                ("TEXCOORD", 0, _) => InputRole::Lanes(vec![
                    LaneStream {
                        semantic: Semantic::TexCoord(0),
                        first: 0,
                        count: 2,
                        component: 0,
                    },
                    LaneStream {
                        semantic: Semantic::TexCoord(1),
                        first: 2,
                        count: 2,
                        component: 0,
                    },
                ]),
                ("COLOR", 1, _) => InputRole::Stream(SECOND_COLOUR),
                // the Byte4C members of the layered programs, four floats each; a coordinate pair of integers, or an effect
                // program's one or two floats, is another input
                ("TEXCOORD", n @ 1..=6, _) if v.value.scalar == ScalarType::Float && v.value.width == 4 => {
                    InputRole::Stream(Semantic::Color(n as u8))
                }
                // the skinned permutation's joint indices and weights
                ("BLENDINDICES", 0, _) => InputRole::Stream(Semantic::Joints(0)),
                ("BLENDWEIGHT", 0, _) => InputRole::Stream(Semantic::Weights(0)),
                // the cloth permutations take the tangent's w, the bitangent sign, as the byte of an input of its own: the
                // program decodes it, `(byte - 127) / 127`, into the w of the tangent it passes on
                ("BINORMAL", 0, _) if v.value.width == 1 && v.value.scalar == ScalarType::Uint => {
                    InputRole::Lanes(vec![LaneStream {
                        semantic: Semantic::Tangent,
                        first: 0,
                        count: 1,
                        component: 3,
                    }])
                }
                // an input the program declares and the value graph never reads needs no stream
                _ if read.as_ref().is_ok_and(|read| {
                    !read.contains(&(v.label.name.to_ascii_uppercase(), v.label.index))
                }) =>
                {
                    InputRole::Unknown {
                        why: "declared by the program and never read".into(),
                    }
                }
                // the lowered geometry keeps one tangent set and two coordinate sets (the packed pair of TEXCOORD0)
                ("TANGENT", 1.., _) => InputRole::Unknown {
                    why: "a further tangent set (bytes decoded like TANGENT0's); the lowered geometry keeps one".into(),
                },
                ("TEXCOORD", 1.., _) => InputRole::Unknown {
                    why: format!(
                        "a further coordinate set ({:?} x{}); the lowered geometry keeps two",
                        v.value.scalar, v.value.width
                    ),
                },
                _ => InputRole::Unknown {
                    why: "no stream of the lowered geometry carries it".into(),
                },
            };
            InputBinding {
                label: v.label.clone(),
                role,
            }
        })
        .collect();
    // what the program shows of itself: its value graph (which buffers a vertex stream indexes) and a run (whether it takes
    // the camera away from the world position); an error stays as the reason of the constants that need it
    let camera = program
        .as_ref()
        .map_err(|e| format!("the program does not lift: {e}"))
        .and_then(camera_form);
    let mut undecided = None;
    let mut constants = Vec::new();
    for resource in &interface.resources {
        let ResourceKind::ConstantBuffer { layout } = &resource.kind else {
            continue;
        };
        // every constant of the buffer, the members of a structure (the instance record, an array of them) by their own
        // names at their own offsets in the first element, as the layout the evaluators use names them
        let flat = flat_constants(layout);
        // a palette is a buffer the program indexes by a vertex stream (a joint), not by the instance alone
        let palette = || -> std::result::Result<bool, String> {
            let program = program.as_ref().map_err(Clone::clone)?;
            let own = program
                .interface
                .resources
                .iter()
                .find(|r| {
                    r.provenance.d3d == resource.provenance.d3d
                        && matches!(r.kind, ResourceKind::ConstantBuffer { .. })
                })
                .ok_or("the lifted program has not this constant buffer")?;
            palette_indexed_by_vertex(program, own.id)
        };
        for (name, offset, ty) in flat {
            let size = ty.size();
            // the screen size is the target's size in pixels, then its reciprocals: two pairs, a role each (the order
            // the vocabulary's old four-value size had; no program of the corpus was checked for it)
            if name == "FC_ScreenSize" && size == Some(16) {
                for (at, role) in [
                    (0, ConstantRole::View(ViewParameter::ViewportSize)),
                    (8, ConstantRole::View(ViewParameter::InverseViewportSize)),
                ] {
                    constants.push(ConstantBinding {
                        resource: resource.id,
                        offset: offset + at,
                        size: 8,
                        name: Some(name.clone()),
                        role,
                        lines: None,
                    });
                }
                continue;
            }
            let role = match name.as_str() {
                "FC_MatrixView" => ConstantRole::View(ViewParameter::View),
                // the instance record's own fields the vertex program reads
                "matricesData" => ConstantRole::Object(ObjectParameter::MatrixBase),
                "userClipPlane" => ConstantRole::Object(ObjectParameter::ClipPlane),
                // the cloth switch of the programs that have a cloth permutation
                "VC_isEnableCloth" => ConstantRole::Object(ObjectParameter::ClothSimulation),
                "FC_MatrixProj" => ConstantRole::View(ViewParameter::Projection),
                // the matrix of a world with the camera at its origin, or of the world itself, as the program's run shows
                "VC_MatrixViewProj" => match &camera {
                    Ok(CameraForm::Relative) => {
                        ConstantRole::View(ViewParameter::CameraRelativeViewProjection)
                    }
                    Ok(CameraForm::Plain) => ConstantRole::View(ViewParameter::ViewProjection),
                    // a run that does not decide is not a finding: 15,483 of the 15,707 matrix-times-world programs of
                    // Elden Ring's corpus are camera-relative (224 plain), so the matrix is taken as camera-relative,
                    // as a policy the caller counts with the reason
                    Err(why) => {
                        undecided = Some(normalised(why));
                        ConstantRole::View(ViewParameter::CameraRelativeViewProjection)
                    }
                },
                "FC_MatrixInvView" => ConstantRole::View(ViewParameter::InverseView),
                "FC_MatrixInvProj" => ConstantRole::View(ViewParameter::InverseProjection),
                "FC_MatrixInvViewProj" => ConstantRole::View(ViewParameter::InverseViewProjection),
                "SC_CameraPos" => ConstantRole::View(ViewParameter::CameraPosition),
                // the object matrices: one per object, or the joints' palette when a vertex stream (a joint index)
                // indexes them, as the program's value graph shows
                "VC_aObjMatrix" => match palette() {
                    Ok(true) => ConstantRole::Object(ObjectParameter::BonePalette),
                    Ok(false) => ConstantRole::Object(ObjectParameter::World),
                    Err(why) => ConstantRole::Unknown { why },
                },
                // the second palette the skinned programs of Elden Ring apply first, by its constant's own name
                "VC_aClothCancelObjMatrix" => match palette() {
                    Ok(true) => ConstantRole::Object(ObjectParameter::ClothCancelPalette),
                    Ok(false) => ConstantRole::Unknown {
                        why: "no vertex stream indexes it".into(),
                    },
                    Err(why) => ConstantRole::Unknown { why },
                },
                // `_n` for texture coordinate set n: the program multiplies the set by `xy` (a scale) and adds `zw` (a scroll
                // offset)
                name => match texture_scroll_set(name) {
                    Some(set) => ConstantRole::Object(ObjectParameter::TextureScroll(set)),
                    None => continue,
                },
            };
            let size = size.ok_or_else(|| invalid("layer binding", 0, format!("{name} overflows")))?;
            constants.push(ConstantBinding {
                resource: resource.id,
                offset,
                size,
                name: Some(name),
                role,
                lines: None,
            });
        }
    }
    // a matrix's lines as the program applies it, or why it does not show them
    let graph = program.as_ref().map_err(Clone::clone).and_then(Program::graph);
    state_lines(graph.as_ref().map_err(String::as_str), interface, &mut constants);
    let palette_rows = palette_rows_by_rule(&mut constants);
    // the buffers of the joints' palettes are bound larger than the program declares
    let mut constant_buffers: Vec<ConstantBufferBinding> = Vec::new();
    if let Some(bound) = palette_bound_size(shader.format) {
        for c in &constants {
            if matches!(
                c.role,
                ConstantRole::Object(ObjectParameter::BonePalette | ObjectParameter::ClothCancelPalette)
            ) && !constant_buffers.iter().any(|b| b.resource == c.resource)
            {
                constant_buffers.push(ConstantBufferBinding {
                    resource: c.resource,
                    bound_size: Some(bound),
                });
            }
        }
    }
    let palette_bound = !constant_buffers.is_empty();
    Ok(VertexBinding {
        stage: StageBinding {
            stage: Stage::Vertex,
            variant: None,
            shader: entity,
            inputs,
            constants,
            textures: Vec::new(),
            buffers: Vec::new(),
            constant_buffers,
            outputs: Vec::new(),
        },
        undecided,
        palette_rows,
        palette_bound,
    })
}

/// How the engine holds its joints' palette, `VC_aObjMatrix`: by rows (three lines of four per joint, each a row of the
/// joint's matrix). Every vertex program that applies the palette reads it so, and none by columns: 17,455 of Elden
/// Ring's (the one other reads only the joints' translations) and 42 of 42 of Sekiro's ([`state_lines`] on each).
pub const PALETTE_ROWS_RULE: &str = "joints' palette read but not applied as a matrix (the program takes only the joints' translations): its lines are rows, as every program that applies VC_aObjMatrix reads them (Elden Ring 17,455 of 17,455, Sekiro 42 of 42)";

/// States the lines of a joints' palette the program reads but does not apply ([`Unshown::NotApplied`]) by
/// [`PALETTE_ROWS_RULE`]: the program shows nothing of them, and a consumer that writes the joints' matrices must still
/// place their translations. A palette the program does not read, or applies both ways, is left as it shows.
/// Returns whether a palette was so stated.
fn palette_rows_by_rule(constants: &mut [ConstantBinding]) -> bool {
    let mut ruled = false;
    for c in constants
        .iter_mut()
        .filter(|c| c.role == ConstantRole::Object(ObjectParameter::BonePalette))
    {
        if c.lines == Some(MatrixLines::Unshown(Unshown::NotApplied)) {
            c.lines = Some(MatrixLines::Rows);
            ruled = true;
        }
    }
    ruled
}

/// The joints a byte of `BLENDINDICES` addresses, the most a palette of Elden Ring's skinned programs holds.
pub const PALETTE_JOINTS: u32 = 256;

/// The policy a consumer counts when a stage states a joint palette's bound ([`PALETTE_JOINTS`] matrices of three 16-byte
/// rows, 12,288 bytes) for a DXIL program: the size is not read from the model.
///
/// Elden Ring's skinned programs declare the palette (`VC_aObjMatrix`) and the cloth-cancel palette
/// (`VC_aClothCancelObjMatrix`) as two matrices (six rows), and D3D12 bounds a constant buffer read by the view the draw
/// binds, not by the declaration. The capture `ercap3` of the running game (colour
/// `scratch/elden-ring/parity/gap/colour-ercap3-records.json`, `readbacks[].cbvs[]`, `extent`
/// `descriptor_SizeInBytes`) binds register 5 with 12,288 bytes in the 20 readbacks of the skinned program
/// `b51c251219ab7269` (draws 4297, 4301, 4306, 4313 and 5316 to 5331: 244, 244, ..., 253 of the 256 matrices non-zero, the
/// last at index 255) and with 3,072 bytes (64 matrices, 61 non-zero, the last at index 62) in the one readback of the
/// skinned program `94c0b95ba49885bb` (draw 1020). The engine binds a different size for different draws, probably what
/// the model needs, and what decides it is not established here. So the bound stated is the largest observed, which is the
/// most a byte index addresses; a consumer that fills fewer matrices leaves the rest zero, as the engine's unwritten rows
/// read. Sekiro's programs (DXBC) declare the same six rows, and no capture of its palette's size exists
/// (`cache/sekiro/cb11-capture` reads only the texture scroll buffer, VS slot 11), so Sekiro states no bound.
pub const PALETTE_BOUND_POLICY: &str = "joint palette bound stated at 256 matrices (12,288 bytes), the largest observed in a capture of Elden Ring (the engine binds 64 in one skinned draw and 256 in 20 others; the model's own size is not read)";

/// The bound stated for the palettes of a program of `format`, or none.
fn palette_bound_size(format: primase_core::shader::ShaderFormat) -> Option<u32> {
    (format == primase_core::shader::ShaderFormat::Dxil).then_some(PALETTE_JOINTS * 3 * 16)
}

/// A vertex program's binding ([`resolve_vertex`]) and what the caller must count.
#[derive(Clone, Debug)]
pub struct VertexBinding {
    pub stage: StageBinding,
    /// Why the probe left the view-projection matrix undecided, its numbers as `N`, when it did: the matrix is then taken as
    /// camera-relative by policy, which the caller reports with this reason.
    pub undecided: Option<String>,
    /// Whether the joints' palette's lines are stated by [`PALETTE_ROWS_RULE`], which the caller counts.
    pub palette_rows: bool,
    /// Whether the stage states a bound for its palettes ([`PALETTE_BOUND_POLICY`]), which the caller counts.
    pub palette_bound: bool,
}

/// A reason with its numbers as `N` (offsets and counts differ program to program; the reason does not).
fn normalised(reason: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in reason.chars() {
        if c.is_ascii_digit() || (digits && c == '.') {
            if !digits {
                out.push('N');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

/// The uv group of the metaparam's record for program texture `resource`, where it names one: the record of the same
/// `Texture2D_N` ([`crate::metaparam::texture_number`] of its name), both games' rule. A record's
/// [`crate::metaparam::Texture::slot`] is its register, not that number: Sekiro's `Character_AMSN_[DetailBlend]`
/// metaparam records `Texture2D_7_AlbedoMap` (the base albedo, register 1) in group_1 and `Texture2D_2_AlbedoMap` (a
/// detail fabric, register 7) in group_3, and finding the record by its slot swapped their tilings.
fn uv_group<'a>(metaparam: Option<&'a Metaparam>, resource: &Resource) -> Option<&'a str> {
    let n = texture_slot(resource)?;
    metaparam?
        .textures
        .iter()
        .find(|t| crate::metaparam::texture_number(&t.name) == Some(n) && !t.uv_group.is_empty())
        .map(|t| t.uv_group.as_str())
}

/// The row of `FC_TextureTilingScale` the engine writes for the material texture `resource` (bound to `binding`, where
/// the material binds it), both games' rule as their captures show it, each axis scaled about the texture's centre
/// ([`centred`]):
///
/// - the scale the FLVER's texture of the slot stores, where it is not 1;
/// - else the scale the definition stores for the slot, where it is not zero
///   ([`crate::materials::DefinitionRef::stored_scale`]: the floats after an MTD texture's path, after a material
///   binary sampler's key);
/// - else the uv group's `<group>_CommonUV-UVParam` of the metaparam texture that shares the resource's `Texture2D_N`
///   ([`uv_group`]) as the material states it, else as the metaparam defaults it;
/// - else 1.
///
/// Elden Ring: the 578 rows the programs of 160 G-buffer draw groups read, the groups tied to their material binaries by
/// shader and bound textures: the sampler's scale 153 rows, the uv group's 425; every character's row matched, and nine
/// rows of one map material binary, AEG099_324_ID001, did not (19, 55 and 24 captured where it states 10, 35 and 10):
/// the draw is likely of another material with the same textures, not established. Its FLVERs store no scale but 1
/// (`sat_field_census`), so the first step changes none of its rows.
///
/// Sekiro: the 483 `cbTextureTilingScale` contents of 58 pixel programs captured in an interior of Ashina (cb11-capture),
/// each tied to the shader bundles holding its program by the program's hash, and to every MTD of those shaders with every
/// set of texture scales the install's FLVERs store with it: of the 440 tied, the rule reproduces 301, 101 of them with
/// a row other than 1 (every row of the Wolf's scarf, top and trousers: `P_BD_M_9040_tops1_Maffler`'s stored 18 by 6 at
/// `Texture2D_1` and its uv group's 51 by 17 at `Texture2D_2`, which stores zero), where the uv group's scale first,
/// else the FLVER's, uncentred, reproduced 221 (12). Every row of a scale other than 1 is centred. The stored scale before
/// the uv group is decided by 4 contents and contradicted by none; the FLVER's before the stored one by 3 (the FLVER's
/// times the rest fits 3 fewer); the metaparam's default decides no content. Not reproduced: 130 contents by any order
/// (M[Multiple] 27, Fur_NTC 25, M[MultipleGround] 19, AddNormal 13, Character_AMSN 10, Character_AMSN_[DetailBlend] 10,
/// 26 of eleven other families), and 9 with every row 1 that only the order before fits
/// (Character_AN_Blend_AMSN_[Fresnel] 4, M[LandScape]_Mountain 4, Character_AMSN_[AO_SSS]_[Cs] 1): every MTD of their
/// programs stores a scale other than 1 at a register captured 1, not established.
fn tiling(
    material: &MaterialView<'_>,
    metaparam: Option<&Metaparam>,
    resource: &Resource,
    binding: Option<&Binding>,
) -> [f32; 4] {
    let n = texture_slot(resource);
    let flver = binding.map(|b| b.scale).filter(|s| *s != [1.; 2]);
    let stored = || material.definition.stored_scale(n?);
    let group = || {
        let name = format!("{}_CommonUV-UVParam", uv_group(metaparam, resource)?);
        match material.definition.parameter(&name) {
            Some(ParameterValue::Float(v)) if v.len() >= 2 => Some([v[0], v[1]]),
            _ => metaparam?
                .mtd_params
                .iter()
                .find(|p| p.name == name)
                .map(|p| [p.default[0], p.default[1]]),
        }
    };
    centred(flver.or_else(stored).or_else(group).unwrap_or([1.; 2]))
}

/// The texture Elden Ring binds when the material leaves `resource` unbound: the shader metaparam's default for
/// the same `Texture2D_N`, by its stem, where [`crate::metaparam::unbound_slots`] (the reader's rule too) takes the slot
/// as unbound. The reader retains the authored texture payload, so its stored texels and the slot's colour transfer
/// determine the value. The Stormveil capture (ercap3) ties the empty DetailBlend slots' uploaded mip hashes to these
/// system textures. An explicit binding still takes precedence if its payload is missing.
fn default_texture(
    material: &MaterialView<'_>,
    metaparam: Option<&Metaparam>,
    resource: &Resource,
) -> Option<String> {
    let n = texture_slot(resource)?;
    crate::metaparam::unbound_slots(material.bindings.keys(), metaparam?)
        .find(|(slot, _)| *slot == n)
        .map(|(_, stem)| stem)
}

/// A tiling scale as both games write it: each axis scaled about the texture's centre, `(u, v, (1 - u) / 2,
/// (1 - v) / 2)` (the program samples `uv * scale + offset`).
fn centred([u, v]: [f32; 2]) -> [f32; 4] {
    [u, v, (1. - u) / 2., (1. - v) / 2.]
}

/// The words the engine writes into `SAT_MtdParam` for the parameter `p` of `value` (a colour's four lanes, its
/// `intensity` apart; `intensity` is 1 for any other type), as a capture of Elden Ring running shows it: the 260
/// G-buffer draw groups of a session in Limgrave whose programs read SAT parameters, 97 of them tied to their material
/// binaries by shader and bound textures, every lane the programs read compared with the material's stated value:
///
/// - a colour (type 12 or 13) whose metaparam sets its srgb flag: red, green and blue raised to 2.2, then times the
///   intensity (124 lanes; the intensities seen, 1 to 20, tell this from the power of the product), alpha as stated;
/// - a colour whose flag is clear (the M[AMSN_V] ice and cheap map shaders' among the 197 of them): times the intensity
///   only (8 lanes, none raised);
/// - a bool (type 0): the integer's bit pattern, 1 and not 1.0 (3 lanes; 4 more were 0);
/// - every other value as stated (float, float2 and float3: 267 lanes that are not 0 or 1).
///
/// Four lanes of `P[ChrCustomize][Eye]` differ from what the material states (100 / 255 for a stated 0.8): the
/// character's own colours, set per draw, is likely and not established.
///
/// Sekiro's named fields and this path use the same scoped encoding ([`folded_colour`]).
fn sat_words(
    game: crate::Game,
    p: &crate::metaparam::MtdParam,
    value: &[f32],
    intensity: f32,
) -> std::result::Result<Vec<u32>, &'static str> {
    if p.is_colour() {
        return folded_colour(game, Some(p), value, intensity)
            .map(|lanes| lanes.into_iter().map(f32::to_bits).collect());
    }
    if game == crate::Game::EldenRing && p.value_type == 0 {
        return Ok(value.iter().map(|&v| (v as i32) as u32).collect());
    }
    Ok(value.iter().map(|v| v.to_bits()).collect())
}

#[cfg(test)]
mod tests {
    use super::{centred, sat_words, slot_name, texture_slot};
    use crate::Game;
    use crate::metaparam::MtdParam;

    fn param(value_type: u8, srgb: bool) -> MtdParam {
        MtdParam {
            name: "C_DetailBlend__snp_0_color_1".into(),
            start: 36,
            sequence_index: 0,
            value_type,
            not_shader_input: false,
            srgb,
            colour: matches!(value_type, 12 | 13),
            key: 0,
            default: [1.; 5],
        }
    }

    /// A program's texture is numbered as a material key is: the digits after its first `Texture2D_`, whatever follows
    /// them.
    #[test]
    fn a_program_texture_is_numbered_as_a_material_key_is() {
        use primase_core::shader::{
            Resource, ResourceId, ResourceKind, ResourceProvenance, SampleType, TextureDim,
        };
        let resource = |name: &str| Resource {
            id: ResourceId(0),
            kind: ResourceKind::Texture {
                dim: TextureDim::D2,
                arrayed: false,
                multisampled: false,
                sample: SampleType::Float,
            },
            count: 1,
            provenance: ResourceProvenance {
                name: name.into(),
                ..Default::default()
            },
        };
        for (name, number) in [
            ("C_DetailBlend_snp_0_Texture2D_7", Some(7)),
            ("C_DetailBlend_snp_0_Texture2D_7_A", Some(7)),
            ("g_Texture2D_x", None),
            ("g_DitherTexture", None),
        ] {
            assert_eq!(texture_slot(&resource(name)), number, "{name}");
        }
    }

    /// Captured rows of Elden Ring's FC_TextureTilingScale: each axis scaled about the texture's centre.
    #[test]
    fn tiling_rows_are_centred_as_the_capture_shows() {
        // C[c3251]_BD's leather detail (its sampler's 10), C[c3251]_Shield's bark (24 by 48), C[c3251]_HD's
        // leather (its uv group's 20), a slot of no scale
        assert_eq!(centred([10., 10.]), [10., 10., -4.5, -4.5]);
        assert_eq!(centred([24., 48.]), [24., 48., -11.5, -23.5]);
        assert_eq!(centred([20., 20.]), [20., 20., -9.5, -9.5]);
        assert_eq!(centred([1., 1.]), [1., 1., 0., 0.]);
    }

    /// Captured words of Elden Ring's SAT parameters from the material binaries' values: an srgb colour raised to 2.2
    /// before its intensity (`C[c3251]_BD_Cloth`'s colour 4 at intensity 1, `P[BD_M_1360]_fabric`'s colour 1 at 3.5),
    /// a colour without the flag times its intensity only, a bool as the integer's bits.
    #[test]
    fn sat_parameters_are_written_as_the_capture_shows() {
        let close = |words: std::result::Result<Vec<u32>, &str>, want: &[f32]| {
            let words = words.unwrap();
            assert_eq!(words.len(), want.len());
            for (w, &e) in words.iter().zip(want) {
                let got = f32::from_bits(*w);
                assert!((got - e).abs() <= 2e-6 * e.abs().max(1.), "{got} for {e}");
            }
        };
        close(
            sat_words(
                Game::EldenRing,
                &param(13, true),
                &[0.824_385, 0.713_067, 0.581_919, 1.],
                1.,
            ),
            &[0.653_862_1, 0.475_211_26, 0.303_876_85, 1.],
        );
        let fabric = sat_words(
            Game::EldenRing,
            &param(13, true),
            &[1., 0.967_288, 0.952_941, 1.],
            3.5,
        );
        close(Ok(fabric.unwrap()[1..3].to_vec()), &[3.253_050_8, 3.147_844_6]);
        close(
            sat_words(Game::EldenRing, &param(13, false), &[0.5, 0.25, 1., 0.75], 3.),
            &[1.5, 0.75, 3., 0.75],
        );
        assert_eq!(
            sat_words(Game::EldenRing, &param(0, false), &[1.], 1.),
            Ok(vec![1])
        );
        assert_eq!(
            sat_words(Game::EldenRing, &param(1, false), &[0.35], 1.),
            Ok(vec![0.35f32.to_bits()])
        );
        // Sekiro's captured head colour, plus an arbitrary alpha to ensure only RGB is encoded.
        close(
            sat_words(
                Game::Sekiro,
                &param(12, true),
                &[0.776_470_66, 0.721_568_64, 0.721_568_64, 0.75],
                1.,
            ),
            &[0.573_159, 0.487_765_22, 0.487_765_22, 0.75],
        );
        // Nonunit CPU-uploaded coat words. Alpha is arbitrary here to check it is preserved.
        close(
            sat_words(Game::Sekiro, &param(12, true), &[1., 1., 0.862_745_17, 0.75], 10.),
            &[10., 10., 7.226_725_6, 0.75],
        );
        close(
            sat_words(
                Game::Sekiro,
                &param(12, true),
                &[0.780_392_2, 0.647_058_84, 0.568_627_5, 0.75],
                0.5,
            ),
            &[0.289_773_37, 0.191_887_33, 0.144_408, 0.75],
        );
        close(
            sat_words(Game::Sekiro, &param(12, true), &[1., 1., 1., 0.75], 0.6),
            &[0.6, 0.6, 0.6, 0.75],
        );
        for intensity in [-1., f32::INFINITY, f32::NAN] {
            assert_eq!(
                sat_words(Game::Sekiro, &param(12, true), &[1.; 4], intensity),
                Err(super::SEKIRO_COLOUR_DOMAIN_UNKNOWN),
            );
        }
        for value in [-1., f32::INFINITY, f32::NAN] {
            assert_eq!(
                sat_words(Game::Sekiro, &param(12, true), &[value, 1., 1., 1.], 1.),
                Err(super::SEKIRO_COLOUR_DOMAIN_UNKNOWN),
            );
        }
        close(
            sat_words(Game::Sekiro, &param(12, false), &[0.5, 0.25, 1., 0.75], 2.),
            &[1., 0.5, 2., 0.75],
        );
        assert_eq!(
            sat_words(Game::Sekiro, &param(0, false), &[1.], 1.),
            Ok(vec![1f32.to_bits()])
        );
    }

    /// Roles past a blend shader's `GSBlendMap` prefix and past empty segments; a slot named only `GSBlendMap` keeps it,
    /// and a slot naming no role is still a slot.
    #[test]
    fn slot_roles() {
        for (kind, slot, role) in [
            ("g_DetailBlend_Texture2D_2_GSBlendMap_AlbedoMap_1", 2, "AlbedoMap"),
            ("g_DetailBlend_Texture2D_3_GSBlendMap", 3, "GSBlendMap"),
            ("g_Water_Texture2D_2__DistortionDepth", 2, "DistortionDepth"),
            ("g_DetailBlend_Texture2D_0_AlbedoMap_0", 0, "AlbedoMap"),
            ("g_Detail_Texture2D_3_", 3, ""),
            // a key naming no role, without the trailing separator: still its slot, as the reader's rule takes it
            ("g_Detail_Texture2D_3", 3, ""),
        ] {
            assert_eq!(slot_name(kind), Some((slot, role)), "{kind}");
        }
        assert_eq!(slot_name("g_Texture2D_x_AlbedoMap"), None);
        assert_eq!(slot_name("AlbedoMap"), None);
    }
}
