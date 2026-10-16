//! Prepare material masks and losses without changing the source world.
use std::collections::BTreeMap;

use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{ColorSpace, Entity, NormalConvention, TexRole, Texture, TextureFile, World};

use crate::{Error, Result};

pub(super) struct Materials {
    pub masks: Vec<Texture>,
    pub bindings: BTreeMap<Entity, usize>,
    pub wraps: BTreeMap<Entity, Wrap>,
    pub log: PassLog,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MaskKey {
    source: Entity,
    mr: bool,
    ao: bool,
    metallic: u32,
    roughness: u32,
    strength: u32,
    wrap: Wrap,
}

fn bad(entity: Entity, message: impl std::fmt::Display) -> Error {
    Error::Scene(format!("raise-skate: material {entity:?}: {message}"))
}

fn unit(entity: Entity, value: f32, label: &str) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(bad(entity, format!("{label} must be finite and in 0..1")));
    }
    Ok(())
}

fn sampling(entity: Entity, slot: TextureRef) -> Result<Wrap> {
    if slot.texcoord != 0 || slot.transform.is_some_and(|t| t != UvTransform::IDENTITY) {
        return Err(bad(
            entity,
            "mask conversion requires UV0 with no texture transform; bake coordinates first",
        ));
    }
    if slot.wrap[0] != slot.wrap[1] {
        return Err(bad(entity, "mask conversion requires the same wrap on both axes"));
    }
    Ok(slot.wrap[0])
}

pub(super) fn prepare(world: &World, materials: &[(Entity, PbrMaterial)]) -> Result<Materials> {
    let mut plan = Materials {
        masks: Vec::new(),
        bindings: BTreeMap::new(),
        wraps: BTreeMap::new(),
        log: PassLog::default(),
    };
    let mut cache = BTreeMap::<MaskKey, (usize, bool)>::new();
    for (entity, m) in materials {
        let entity = *entity;
        unit(entity, m.metallic, "metallic factor")?;
        unit(entity, m.roughness, "roughness factor")?;
        unit(entity, m.occlusion_strength, "occlusion strength")?;
        for value in m.base_color {
            unit(entity, value, "base colour factor")?;
        }
        if let primase_core::pbr::AlphaMode::Mask { cutoff } = m.alpha_mode {
            unit(entity, cutoff, "alpha cutoff")?;
        }
        if !m.normal_scale.is_finite() || (m.normal_texture.is_some() && m.normal_scale != 1.0) {
            return Err(bad(
                entity,
                "normal scale must be baked into normal pixels before conversion",
            ));
        }
        if !m.emissive_strength.is_finite()
            || m.emissive_strength < 0.0
            || m.emissive.iter().any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(bad(entity, "emission must be finite and nonnegative"));
        }
        for r in m.textures() {
            if !world.has::<Texture>(r.texture) && !world.has::<TextureFile>(r.texture) {
                return Err(bad(
                    entity,
                    format!("texture {:?} has no pixels or file", r.texture),
                ));
            }
        }
        for r in [m.base_color_texture, m.normal_texture].into_iter().flatten() {
            if r.transform.is_some() {
                return Err(bad(
                    entity,
                    "a texture slot still has a transform: bake it first (bake-texture-transforms)",
                ));
            }
            let t = world
                .get::<Texture>(r.texture)
                .ok_or_else(|| bad(entity, format!("texture {:?} has no pixels", r.texture)))?;
            if t.role == TexRole::Normal && t.normal != Some(NormalConvention::GLTF) {
                return Err(bad(
                    entity,
                    "normal map not in glTF's convention: run normalize-normals first",
                ));
            }
        }
        if m.emissive_texture.is_some() {
            plan.log
                .approximate("PBR emission texture binding not representable in skate-scene", 1);
        }
        if m.emissive_strength > 0.0 && m.emissive.iter().any(|v| *v > 0.0) {
            plan.log
                .approximate("PBR emissive radiance not representable in skate-scene", 1);
        }
        let Some(slot) = m.metallic_roughness_texture.or(m.occlusion_texture) else {
            continue;
        };
        let wrap = sampling(entity, slot)?;
        for r in [
            m.base_color_texture,
            m.normal_texture,
            m.metallic_roughness_texture,
            m.occlusion_texture,
        ]
        .into_iter()
        .flatten()
        {
            if sampling(entity, r)? != wrap {
                return Err(bad(entity, "base, normal and mask slots require one shared wrap"));
            }
        }
        if m.metallic_roughness_texture
            .zip(m.occlusion_texture)
            .is_some_and(|(mr, ao)| mr.texture != ao.texture)
        {
            return Err(bad(
                entity,
                "distinct metallic-roughness and occlusion images require an explicit packing policy",
            ));
        }
        let source = world
            .get::<Texture>(slot.texture)
            .ok_or_else(|| bad(entity, format!("mask source {:?} has no pixels", slot.texture)))?;
        if source.color_space != ColorSpace::Linear || source.levels.is_empty() {
            return Err(bad(
                entity,
                "mask source requires linear pixels and at least one mip",
            ));
        }
        let key = MaskKey {
            source: slot.texture,
            mr: m.metallic_roughness_texture.is_some(),
            ao: m.occlusion_texture.is_some(),
            metallic: m.metallic.to_bits(),
            roughness: m.roughness.to_bits(),
            strength: m.occlusion_strength.to_bits(),
            wrap,
        };
        let (index, quantized) = if let Some(&entry) = cache.get(&key) {
            entry
        } else {
            let mut quantized = false;
            let texture = primase_image::map_rgba8_levels(
                source,
                0..source.levels.len(),
                primase_image::TextureMetadata {
                    color_space: ColorSpace::Linear,
                    role: TexRole::Mask,
                    normal: None,
                },
                |p| {
                    let mr = if key.mr { p.rgba } else { [255; 4] };
                    let ao = if key.ao { p.rgba[0] } else { 255 };
                    let values = [
                        f64::from(m.metallic) * f64::from(mr[2]),
                        255.0 - f64::from(m.roughness) * f64::from(mr[1]),
                        255.0 - f64::from(m.occlusion_strength) * (255.0 - f64::from(ao)),
                    ];
                    let mut pixel = [255; 4];
                    for (out, value) in pixel.iter_mut().zip(values) {
                        let rounded = value.round();
                        quantized |= value != rounded;
                        *out = rounded as u8;
                    }
                    Ok(pixel)
                },
            )
            .map_err(|error| bad(entity, format!("mask source {:?}: {error}", slot.texture)))?;
            let index = plan.masks.len();
            plan.masks.push(texture);
            cache.insert(key, (index, quantized));
            (index, quantized)
        };
        plan.bindings.insert(entity, index);
        plan.wraps.insert(entity, wrap);
        plan.log.count("PBR materials raised with an MSK binding", 1);
        if quantized {
            plan.log.approximate("PBR mask factors quantized to UNORM8", 1);
        }
    }
    if !plan.masks.is_empty() {
        plan.log.count("derived MSK textures", plan.masks.len() as u64);
    }
    Ok(plan)
}
