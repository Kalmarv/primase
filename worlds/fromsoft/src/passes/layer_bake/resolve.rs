//! Sekiro's binding rules for a layered material's G-buffer pixel program: what each constant, texture slot and output
//! of the program stands for, as a `core.shader_binding` stage. The one home of those rules: slot resolution (a
//! numeric slot named twice prefers the texture the MTD declares), the material's SAT parameters and texture tiling
//! as the constants' own words, the engine's per-view, per-frame and per-object values as roles (never values), and
//! the G-buffer layout of the outputs.
use crate::materials::{Binding, LayeredMaterial};
use crate::mtd::Value;
use crate::{Result, invalid};
use primase_core::Entity;
use primase_core::shader::{
    ConstantBinding, ConstantRole, FrameParameter, InputBinding, InputRole, NormalEncoding, NormalSpace,
    ObjectParameter, OutputBinding, OutputRole, ResourceId, ResourceKind, SamplerState, Stage, StageBinding,
    TextureBinding, TextureSource, ViewParameter,
};
use primase_dxbc::Shader;
use primase_shader::{D3dClass, Interface, Layout, Resource, SystemValue};
use std::collections::BTreeMap;

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

/// Bind the pixel program `shader` (entity `entity`) of a layered `material`. `texture` finds the entity of a texture
/// by stem and colour space (`true`: sRGB, the albedo maps), or `None` when the reader holds no such file.
pub fn resolve(
    shader: &Shader,
    entity: Entity,
    material: &LayeredMaterial,
    unbound: Unbound,
    texture: &mut dyn FnMut(&str, bool) -> Result<Option<Entity>>,
) -> Result<Resolution> {
    let interface = &shader.interface;
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
        if resource.count != 1 || dim != primase_core::shader::TextureDim::D2 || arrayed || multisampled {
            return Err(invalid(
                "layer binding",
                0,
                "only individual Texture2D resources are supported",
            ));
        }
        let register = register(resource)?;
        let slot = texture_slot(resource)?;
        // A declared slot retains its semantic role even without a path. In particular an unbound normal gets the
        // flat normal, not the zero of an unknown resource.
        let fallback_role = declared_role(material, slot)?;
        let (role, binding) = slots
            .get(&slot)
            .map_or((fallback_role, None), |(role, b)| (*role, Some(*b)));
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
                sampler: SamplerState::default(),
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
    for &slot in shader.program.textures() {
        if !textures_of(interface).any(|r| register(r).is_ok_and(|reg| reg == slot)) {
            return Err(invalid(
                "layer binding",
                0,
                format!("t{slot} has no named resource"),
            ));
        }
    }
    // the engine's values: roles, whatever a consumer freezes them to
    constant(
        "FC_DifMapMulCol",
        0,
        16,
        ConstantRole::Frame(FrameParameter::AlbedoTint),
    )?;
    constant("mWorld", 0, 48, ConstantRole::Object(ObjectParameter::World))?;
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
            constant(
                name,
                0,
                field.words * 4,
                ConstantRole::Unknown {
                    why: "missing SAT parameter left at frozen zero".into(),
                },
            )?;
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
    let outputs = output_bindings(interface);
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
/// emission, in the lanes the layered programs write them; only those the program declares.
fn output_bindings(interface: &Interface) -> Vec<OutputBinding> {
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
        let (lanes, role) = match n {
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
