//! Sekiro's binding rules for a layered material's G-buffer pixel program: what each constant, texture slot and output
//! of the program stands for, as a `core.shader_binding` stage. The one home of those rules: slot resolution (a
//! numeric slot named twice prefers the texture the MTD declares), the material's SAT parameters and texture tiling
//! as the constants' own words, the engine's per-view, per-frame and per-object values as roles (never values), and
//! the G-buffer layout of the outputs.
use crate::materials::{Binding, LayeredMaterial};
use crate::metaparam::Metaparam;
use crate::mtd::Value;
use crate::{Result, invalid};
use primase_core::Entity;
use primase_core::shader::{
    Blend, BlendEquation, BlendFactor, BlendOp, Compare, ConstantBinding, ConstantRole, DepthState,
    FrameParameter, FrameTexture, InputBinding, InputRole, LaneStream, NormalEncoding, NormalSpace,
    ObjectParameter, OutputBinding, OutputRole, RenderState, ResourceId, ResourceKind, SamplerState, Stage,
    StageBinding, TargetState, TextureBinding, TextureSource, ViewParameter,
};
use primase_shader::exec::ProgramShader;
use primase_shader::{D3dClass, Interface, Layout, PixelShader, Resource, SystemValue};
use std::collections::BTreeMap;

/// The stream that carries a layered mesh's second stored vertex colour (the element the FLVER indexes 1, which the
/// layered vertex programs read as `COLOR1`): `Color(0)` is the first colour of a glTF mesh and `Color(1)` to `Color(6)`
/// are the Byte4C members, so the second colour is kept apart from both. Float values as decoded, four lanes.
pub const SECOND_COLOUR: primase_core::Semantic = primase_core::Semantic::Color(9);

/// What becomes of a texture slot the material binds no texture to, or whose texture file the reader does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unbound {
    /// The slot reads a stated colour for its role (a mid-grey albedo, a flat normal, zero elsewhere), counted as an
    /// approximation.
    Preview,
    /// The slot is left `Missing`: nothing is invented.
    Leave,
}

/// The G-buffer outputs the layered programs write, by render target: the world normal, the gloss (x only), the
/// albedo, the reflectance and the emission.
const NORMAL_BIAS_BITS: u32 = 0x3f00_2008;

/// Whether the material names a texture kind exactly as a program resource: the fixed-function (`GXFlver_*`) programs
/// call their textures `g_DiffuseTexture`, `g_SpecularTexture`, `g_BumpmapTexture`, as the material's own kinds, not by
/// numbered slot.
pub(crate) fn named_by_material(material: &LayeredMaterial, name: &str) -> bool {
    material.bindings.contains_key(name) || material.definition.textures.iter().any(|t| t.kind == name)
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

/// A program texture's role and the material's binding for it: for a numbered slot (`Texture2D_<n>`) by the slot's number
/// (a declared slot retains its role even without a path: an unbound normal gets the flat normal, not the zero of an
/// unknown resource), for a fixed-function texture by the kind's own name.
pub(crate) fn role_and_binding<'a>(
    material: &'a LayeredMaterial,
    slots: &BTreeMap<u32, (&'a str, &'a Binding)>,
    resource: &Resource,
) -> Result<(&'a str, Option<&'a Binding>)> {
    Ok(match texture_slot(resource) {
        Ok(slot) => {
            let fallback_role = declared_role(material, slot)?;
            slots
                .get(&slot)
                .map_or((fallback_role, None), |(role, b)| (*role, Some(*b)))
        }
        Err(_) => {
            let name = resource.provenance.name.as_str();
            let kind = material
                .definition
                .textures
                .iter()
                .find(|t| t.kind == name)
                .map_or(name, |t| t.kind.as_str());
            (kind_role(kind), material.bindings.get(name))
        }
    })
}

/// The name of the engine's dither noise texture in the alpha-tested permutations (`<shader>_<switch>_Gbuf_[A].ppo`).
pub const ENGINE_DITHER_TEXTURE: &str = "g_DitherTexture";

/// The name of the scene depth texture the forward programs of fog, foam and water read (`t56`: the hardware depth in x,
/// linearised with `FC_DepthComputeParam`: view depth `-z / (x - depth)`): the frame's, bound as one.
pub const ENGINE_SCENE_DEPTH_TEXTURE: &str = "g_DepthTexture";

/// `<index>` and role of a texture kind named `…Texture2D_<index>_<Role>…`.
pub(crate) fn slot_name(kind: &str) -> Option<(u32, &str)> {
    let rest = kind.rsplit_once("Texture2D_")?.1;
    let (slot, role) = rest.split_once('_')?;
    Some((slot.parse().ok()?, role.split('_').next()?))
}

/// The role a slot's declared texture has in the MTD, `?` if it declares none; two declarations that disagree are an
/// error.
pub(crate) fn declared_role(material: &LayeredMaterial, slot: u32) -> Result<&str> {
    let mut role = None;
    for texture in &material.definition.textures {
        if let Some((index, found)) = slot_name(&texture.kind)
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
pub(crate) fn slots(material: &LayeredMaterial) -> Result<BTreeMap<u32, (&str, &Binding)>> {
    let mut slots = BTreeMap::new();
    for (kind, binding) in &material.bindings {
        if !binding.scale.iter().all(|v| v.is_finite()) {
            return Err(invalid("layer binding", 0, "nonfinite texture scale"));
        }
        if let Some((slot, role)) = slot_name(kind) {
            let declared = material.definition.textures.iter().any(|t| t.kind == *kind);
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

/// A resource's `Texture2D_<slot>` number.
pub(crate) fn texture_slot(resource: &Resource) -> Result<u32> {
    resource
        .provenance
        .name
        .rsplit_once("Texture2D_")
        .and_then(|(_, s)| s.parse::<u32>().ok())
        .ok_or_else(|| {
            invalid(
                "layer binding",
                0,
                format!("unrecognized texture binding {}", resource.provenance.name),
            )
        })
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

/// Bind the pixel program `shader` (entity `entity`) of a layered `material`. `metaparam` is the parameter definition of
/// its shader's bundle, which states the default of a parameter the material does not. `texture` finds the entity of a
/// texture by stem and colour space (`true`: sRGB, the albedo maps), or `None` when the reader holds no such file.
pub fn resolve(
    shader: &ProgramShader,
    entity: Entity,
    material: &LayeredMaterial,
    metaparam: Option<&Metaparam>,
    unbound: Unbound,
    texture: &mut dyn FnMut(&str, bool) -> Result<Option<Entity>>,
) -> Result<Resolution> {
    let interface = PixelShader::interface(shader);
    let leaf = material
        .definition
        .shader
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let family = leaf.rsplit_once('.').map_or(leaf, |x| x.0);
    let prefix = format!("{}_snp_", family.replace(['[', ']'], "_"));
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
    // a constant of the layout, at `word` words into it, `size` bytes (cut where its buffer ends), by name
    let mut constant = |name: &str, word: usize, size: u32, mut role: ConstantRole| -> Result<()> {
        let Some(&primase_shader::Field { buffer, offset, .. }) = layout.fields.get(name) else {
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
        // a texture that is not a material slot (no `Texture2D_<n>` in its name) is the engine's own: the dither noise the
        // alpha-tested permutations read for the dithered fade (a neutral grey: the fade is off), a depth or scene texture
        // of a program that reads one, the volumes and cube maps of a forward program's lighting (zero). The material binds
        // nothing to it; each is counted by name.
        if texture_slot(resource).is_err() && !named_by_material(material, &resource.provenance.name) {
            let name = &resource.provenance.name;
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
        let (role, binding) = role_and_binding(material, &slots, resource)?;
        let mut found = None;
        if let Some(b) = binding {
            found = texture(&b.texture, role == "AlbedoMap")?;
            constant(
                "FC_TextureTilingScale",
                register as usize * 4,
                16,
                words(&[b.scale[0], b.scale[1], 0., 0.]),
            )?;
        } else if unbound == Unbound::Preview {
            constant(
                "FC_TextureTilingScale",
                register as usize * 4,
                16,
                words(&[1., 1., 0., 0.]),
            )?;
        }
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
    let definition_param = |name: &str| material.definition.parameters.iter().find(|p| p.name == name);
    match definition_param("g_DiffuseMapColor").map(|p| &p.value) {
        Some(Value::Float(v)) if v.len() >= 3 => {
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
    let reference = |word: usize| match definition_param("g_AlphaRef").map(|p| &p.value) {
        Some(Value::Int(v)) => v.get(word).copied().filter(|&r| r > 0),
        _ => None,
    };
    constant(
        "FC_AlphaRef",
        0,
        8,
        words(&[
            reference(0).map_or(-100., |r| r as f32 / 255.),
            reference(1).map_or(0., |r| r as f32 / 255.),
        ]),
    )?;
    // the glow scale: read only in its w, into the emission target's alpha (all 34 programs that read it), which no bake
    // reads; the material's g_GlowScale where it states one (a four-float parameter), Unknown, by name, where it does not
    match definition_param("g_GlowScale").map(|p| &p.value) {
        Some(Value::Float(v)) if v.len() >= 4 => constant("FC_GlowScale", 0, 16, words(&v[..4]))?,
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
    // the material's own parameters: the program's `<shader>_snp_*` constants
    let names: Vec<&String> = layout
        .fields
        .keys()
        .filter(|n| n.starts_with(&prefix) && !n.ends_with("_PADDING"))
        .collect();
    for name in names {
        let field = layout.fields[name];
        let Some(param) = material.definition.parameters.iter().find(|p| p.name == *name) else {
            // the shader bundle's metaparam states a default for every parameter it declares: the value of one the
            // material does not state. A colour's default is its four lanes and its intensity (the fifth), folded as
            // a stated colour's is: the first three times the intensity
            let default = metaparam.and_then(|m| m.mtd_params.iter().find(|p| p.name == *name));
            if let Some(p) = default {
                let mut value = p.default;
                if p.value_type == 12 {
                    for v in &mut value[..3] {
                        *v *= p.default[4];
                    }
                }
                let lanes = if p.value_type == 12 { 4 } else { value.len() };
                let n = (field.words as usize).min(lanes);
                *policies
                    .entry(
                        "missing SAT parameter taken from the shader bundle's metaparam default".to_owned(),
                    )
                    .or_default() += 1;
                constant(name, 0, field.words * 4, words(&value[..n]))?;
            } else if let Some(p) = metaparam
                .and_then(|m| m.ca_params.iter().find(|p| p.name == *name))
                .filter(|_| field.words == 1)
            {
                // a bundle that declares CAParams (the other SAT shaders: their flags say 0x10) names each component of a
                // parameter the engine sets per draw by the program's own field name, with the value it has when none
                // is set
                *policies
                    .entry("missing SAT parameter taken from the shader bundle's CAParam default".to_owned())
                    .or_default() += 1;
                constant(name, 0, 4, words(&[p.default]))?;
            } else {
                constant(
                    name,
                    0,
                    field.words * 4,
                    ConstantRole::Unknown {
                        why: "missing SAT parameter left at frozen zero".into(),
                    },
                )?;
            }
            continue;
        };
        let mut values = match &param.value {
            Value::Bool(b) => vec![u8::from(*b) as f32],
            Value::Int(v) => v.iter().map(|&i| i as f32).collect(),
            Value::Float(v) => v.clone(),
        };
        if name.contains("_color_") {
            let intensity = material
                .definition
                .parameters
                .iter()
                .find(|p| p.name == format!("{name}-Intensity"))
                .and_then(|p| match &p.value {
                    Value::Float(v) => v.first().copied(),
                    _ => None,
                })
                .unwrap_or(1.);
            for v in values.iter_mut().take(3) {
                *v *= intensity;
            }
            if values.len() == 3 {
                values.push(1.);
            }
        }
        constant(name, 0, values.len() as u32 * 4, words(&values))?;
    }
    let outputs = output_bindings(interface, material.is_forward());
    let inputs = interface
        .inputs
        .iter()
        .map(|v| InputBinding {
            label: v.label.clone(),
            role: InputRole::Previous,
        })
        .collect();
    Ok(Resolution {
        stage: StageBinding {
            stage: Stage::Pixel,
            shader: entity,
            inputs,
            constants,
            textures: bindings,
            outputs,
        },
        policies,
    })
}

/// The G-buffer layout: render target 0 the world normal, 1 the gloss in x, 2 the albedo, 3 the reflectance and 4 the
/// emission, in the lanes the layered programs write them; only those the program declares. A forward program's render
/// target 0 is the finished colour (its target 1, drawn to a target the capture does not name, has no role).
fn output_bindings(interface: &Interface, forward: bool) -> Vec<OutputBinding> {
    let normal = OutputRole::Normal {
        encoding: NormalEncoding::XyzBiased {
            bias_bits: NORMAL_BIAS_BITS,
        },
        space: NormalSpace::World,
    };
    let mut out = Vec::new();
    for v in &interface.outputs {
        let Some(SystemValue::Target(n)) = v.system else {
            continue;
        };
        // the interface lists the whole signature: a target the program does not declare is not bound
        if !v.used {
            continue;
        }
        let (lanes, role) = match n {
            0 if forward => (0b1111, OutputRole::Colour),
            _ if forward => continue,
            0 => (0b0111, normal),
            1 => (0b0001, OutputRole::Gloss),
            2 => (0b0111, OutputRole::Albedo),
            3 => (0b0111, OutputRole::Reflectance),
            4 => (0b0111, OutputRole::Emission),
            _ => continue,
        };
        if u32::from(lanes) >> v.value.width != 0 {
            continue;
        }
        out.push(OutputBinding {
            label: v.label.clone(),
            lanes,
            role,
        });
    }
    out
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
///   object matrices, the skinned permutation's the joints' palette: `Object(BonePalette)`) and the texture coordinate scale and scroll (`VC_TexScroll_0` to `_3`: `Object(TextureScroll(n))`,
///   the engine's per-model value, observed as the identity on every layered draw of a running game: see
///   [`super::TEXTURE_SCROLL_POLICY`]); constants the table does not name are not bound;
/// - no output roles: its outputs reach the pixel stage by label.
pub fn resolve_vertex(interface: &Interface, entity: Entity) -> Result<StageBinding> {
    use primase_core::Semantic;
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
                    },
                    LaneStream {
                        semantic: Semantic::TexCoord(1),
                        first: 2,
                        count: 2,
                    },
                ]),
                ("COLOR", 1, _) => InputRole::Stream(SECOND_COLOUR),
                ("TEXCOORD", n @ 1..=6, _) => InputRole::Stream(Semantic::Color(n as u8)),
                // the skinned permutation's joint indices and weights
                ("BLENDINDICES", 0, _) => InputRole::Stream(Semantic::Joints(0)),
                ("BLENDWEIGHT", 0, _) => InputRole::Stream(Semantic::Weights(0)),
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
    let skinned = interface
        .inputs
        .iter()
        .any(|v| v.label.name.eq_ignore_ascii_case("BLENDINDICES"));
    let mut constants = Vec::new();
    for resource in &interface.resources {
        let ResourceKind::ConstantBuffer { layout } = &resource.kind else {
            continue;
        };
        // every constant of the buffer, the members of a structure (the instance record, an array of them) by their own
        // names at their own offsets in the first element, as the layout the evaluators use names them
        let mut flat: Vec<(String, u32, Option<u32>)> = Vec::new();
        for c in &layout.constants {
            match c.ty.members() {
                [] => flat.push((c.name.clone(), c.offset, c.ty.size())),
                members => flat.extend(
                    members
                        .iter()
                        .map(|m| (m.name.clone(), c.offset + m.offset, m.ty.size())),
                ),
            }
        }
        for (name, offset, size) in flat {
            let role = match name.as_str() {
                "FC_MatrixView" => ConstantRole::View(ViewParameter::View),
                // the instance record's own fields the vertex program reads
                "matricesData" => ConstantRole::Object(ObjectParameter::MatrixBase),
                "userClipPlane" => ConstantRole::Object(ObjectParameter::ClipPlane),
                "FC_MatrixProj" => ConstantRole::View(ViewParameter::Projection),
                "VC_MatrixViewProj" => ConstantRole::View(ViewParameter::ViewProjection),
                "FC_MatrixInvView" => ConstantRole::View(ViewParameter::InverseView),
                "FC_MatrixInvProj" => ConstantRole::View(ViewParameter::InverseProjection),
                "FC_MatrixInvViewProj" => ConstantRole::View(ViewParameter::InverseViewProjection),
                "SC_CameraPos" => ConstantRole::View(ViewParameter::CameraPosition),
                "FC_ScreenSize" => ConstantRole::View(ViewParameter::ViewportSize),
                // the object matrices: one per object, or the joints' palette in the skinned permutation (the program
                // that takes joint indices)
                "VC_aObjMatrix" if skinned => ConstantRole::Object(ObjectParameter::BonePalette),
                "VC_aObjMatrix" => ConstantRole::Object(ObjectParameter::World),
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
            });
        }
    }
    Ok(StageBinding {
        stage: Stage::Vertex,
        shader: entity,
        inputs,
        constants,
        textures: Vec::new(),
        outputs: Vec::new(),
    })
}
