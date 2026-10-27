//! Prepare material masks and losses without changing the source world.
use std::collections::BTreeMap;

use iridia_interface::Filter;
use primase_core::pass::PassLog;
use primase_core::{ColorSpace, Entity, Name, NormalConvention, TexRole, Texture, TextureFile, World};
use primase_core::{pbr::PbrMaterial, pbr::Shading, pbr::TextureRef, pbr::UvTransform, pbr::Wrap};

use crate::{Error, Result};

pub(super) struct Materials {
    pub masks: Vec<Texture>,
    /// Each mask's name: its source texture's and what was derived from it ([`mask_name`]).
    pub mask_names: Vec<String>,
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
    /// The folded reflectance ([`fold_reflectance`]): F0 (bits) and whether the mask source's A scales it per texel.
    fold: Option<(u64, bool)>,
}

/// A mask's name: `<source texture's name>_msk_<16 hex>`, the hex the first 64 bits of SHA-256 over the source's
/// pixels (size, block layout and every level's bytes) and what was derived from them (the factors, wrap and fold of
/// its [`MaskKey`]), each in a fixed encoding. It is the same wherever the mask is made, and equal names mean equal
/// masks (an index would differ between a whole map and a map written in parts).
fn mask_name(world: &World, key: &MaskKey, source: &Texture) -> String {
    use sha2::{Digest, Sha256};
    let name = world
        .get::<primase_core::Name>(key.source)
        .map_or("mask", |n| n.0.as_str());
    let mut h = Sha256::new();
    let (bw, bh) = source.format.block_dims();
    for x in [source.width, source.height, bw, bh, source.format.block_bytes()] {
        h.update(x.to_le_bytes());
    }
    h.update((source.levels.len() as u64).to_le_bytes());
    for level in &source.levels {
        h.update((level.len() as u64).to_le_bytes());
        h.update(level);
    }
    h.update([u8::from(key.mr), u8::from(key.ao)]);
    for x in [key.metallic, key.roughness, key.strength] {
        h.update(x.to_le_bytes());
    }
    h.update([match key.wrap {
        Wrap::Repeat => 0u8,
        Wrap::Clamp => 1,
        Wrap::Mirror => 2,
    }]);
    match key.fold {
        None => h.update([0]),
        Some((f0, per_texel)) => {
            h.update([1, u8::from(per_texel)]);
            h.update(f0.to_le_bytes());
        }
    }
    let d = h.finalize();
    let hex: String = d[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{name}_msk_{hex}")
}

/// Skate's reflectance at normal incidence for every dielectric surface: Studio's MSK holds metallic, smoothness,
/// occlusion and opacity, no reflectance or specular level, so a surface reflects as F0 0.04 whatever its source says.
pub const SKATE_F0: f64 = 0.04;

/// The roughness at which a GGX highlight at skate's [`SKATE_F0`] peaks as high as the source's highlight of
/// reflectance `f0` at `roughness` (perceptual): the peak goes as F0 / alpha^2 (alpha = roughness^2), so
/// alpha' = alpha sqrt(0.04 / f0), at most 1. A source at or above 0.04 keeps its roughness (never sharpened);
/// reflectance 0 is roughness 1. The PEAK is kept, not the energy: GGX's distribution is normalised, so the widened
/// lobe still reflects 0.04 in total, more than the source; a low-reflectance surface becomes a broad sheen (a lawn
/// at roughness 1 still reflects 4 % diffusely) rather than a mirror.
pub fn fold_reflectance(roughness: f64, f0: f64) -> f64 {
    if f0.is_nan() || f0 >= SKATE_F0 {
        return roughness;
    }
    if f0 <= 0.0 {
        return 1.0;
    }
    (roughness * roughness * (SKATE_F0 / f0).sqrt()).min(1.0).sqrt()
}

/// A dielectric material's reflectance at normal incidence by `KHR_materials_specular` without its textures:
/// 0.04 x the largest colour channel x factor (at most 1: the largest channel, an upper bound, folds no more than
/// any channel needs); `None` for a material without the extension or not exactly dielectric (metallic not exactly
/// 0: its F0 comes from its base colour, which skate keeps).
pub fn specular_f0(m: &PbrMaterial) -> Option<f64> {
    let s = m.specular?;
    (m.metallic == 0.0).then(|| {
        let colour = s.color.iter().fold(0f32, |a, &c| a.max(c));
        (SKATE_F0 * f64::from(colour) * f64::from(s.factor)).clamp(0.0, 1.0)
    })
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
        mask_names: Vec::new(),
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
        for r in [m.base_color_texture, m.normal_texture, m.emissive_texture]
            .into_iter()
            .flatten()
        {
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
        // skate lights every surface and filters every texture linearly: what it cannot draw is reported by name
        let label = || {
            world
                .get::<Name>(entity)
                .map_or_else(|| format!("{entity:?}"), |n| n.0.clone())
        };
        if m.shading == Shading::Unlit {
            plan.log.approximate(
                &format!(
                    "unlit material drawn lit (lower-reskate-unlit{{as=matte}} lowers it first): {}",
                    label()
                ),
                1,
            );
        }
        if m.textures()
            .any(|r| r.filters.mag == Some(Filter::Nearest) || r.filters.min == Some(Filter::Nearest))
        {
            plan.log.approximate(
                &format!(
                    "textures sampled nearest drawn filtered linearly (upscale-nearest keeps their look): {}",
                    label()
                ),
                1,
            );
        }
        let f0 = specular_f0(m);
        match (m.specular, f0) {
            (Some(_), None) => plan
                .log
                .approximate("PBR specular reflectance not representable in skate-scene", 1),
            (Some(s), Some(f0)) => {
                if f0 < SKATE_F0 {
                    plan.log.approximate(
                        "specular reflectance below skate's 0.04 folded into roughness (GGX highlight peak kept)",
                        1,
                    );
                }
                if s.color_texture.is_some() {
                    plan.log
                        .approximate("specular colour textures not folded (their factor only)", 1);
                }
            }
            (None, _) => {}
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
        // the strength texture folds per texel when it is the mask source (lower-gta-pbr derives one texture for both)
        let per_texel = m
            .specular
            .and_then(|s| s.texture)
            .is_some_and(|t| t.texture == slot.texture && m.metallic_roughness_texture.is_some());
        if f0.is_some() && m.specular.is_some_and(|s| s.texture.is_some()) && !per_texel {
            plan.log.approximate(
                "specular strength textures other than the mask source not folded (their factor only)",
                1,
            );
        }
        let key = MaskKey {
            source: slot.texture,
            mr: m.metallic_roughness_texture.is_some(),
            ao: m.occlusion_texture.is_some(),
            metallic: m.metallic.to_bits(),
            roughness: m.roughness.to_bits(),
            strength: m.occlusion_strength.to_bits(),
            wrap,
            fold: f0.filter(|f| *f < SKATE_F0).map(|f| (f.to_bits(), per_texel)),
        };
        let (index, quantized) = if let Some(&entry) = cache.get(&key) {
            entry
        } else {
            // any texel rounded: an OR over the texels, which no thread's order changes
            let quantized = std::sync::atomic::AtomicBool::new(false);
            let texture = primase_image::map_rgba8_levels_with(
                primase_core::parallel::Threads::of(world),
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
                    // smoothness: the unfolded expression unchanged (byte-identical masks), the folded one per texel
                    let smoothness = match f0.filter(|f| *f < SKATE_F0) {
                        None => 255.0 - f64::from(m.roughness) * f64::from(mr[1]),
                        Some(f0) => {
                            let strength = if per_texel {
                                f64::from(p.rgba[3]) / 255.0
                            } else {
                                1.0
                            };
                            let roughness = f64::from(m.roughness) * f64::from(mr[1]) / 255.0;
                            255.0 - fold_reflectance(roughness, f0 * strength) * 255.0
                        }
                    };
                    let values = [
                        f64::from(m.metallic) * f64::from(mr[2]),
                        smoothness,
                        255.0 - f64::from(m.occlusion_strength) * (255.0 - f64::from(ao)),
                    ];
                    let mut pixel = [255; 4];
                    for (out, value) in pixel.iter_mut().zip(values) {
                        let rounded = value.round();
                        if value != rounded {
                            quantized.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        *out = rounded as u8;
                    }
                    Ok(pixel)
                },
            )
            .map_err(|error| bad(entity, format!("mask source {:?}: {error}", slot.texture)))?;
            let quantized = quantized.into_inner();
            let index = plan.masks.len();
            plan.masks.push(texture);
            plan.mask_names.push(mask_name(world, &key, source));
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
