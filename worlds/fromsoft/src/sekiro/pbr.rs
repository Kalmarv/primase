//! Static source-material lowering. Unknown shader constants remain unknown and
//! are counted; this is not a reconstruction of dynamic wetness or light banks.
//! Source metadata stays attached. Existing baked PBR base/normal bindings win.
use crate::{
    Result, invalid,
    materials::{Binding, Material},
    mtd::Value,
};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::{ColorSpace, Entity, Name, TexFormat, TexRole, Texture, World};
use primase_texcodec::TexDecoder;
use std::collections::{BTreeMap, BTreeSet};

/// Intrinsic perceptual roughness from the linear second G-buffer output's X.
/// GXLightAcc_DDirLight1 takes sqrt(gloss), applies 1 - 0.95*x, then squares
/// that value to obtain GGX alpha. Its light-dependent gloss multiplier is
/// deliberately omitted: one glTF material cannot vary roughness per light.
pub fn roughness(gloss: f32) -> Result<f32> {
    if !gloss.is_finite() || !(0.0..=1.0).contains(&gloss) {
        return Err(invalid("Sekiro PBR", 0, "gloss outside finite 0..1"));
    }
    Ok(1.0 - 0.95 * gloss.sqrt())
}

/// Pack a baked linear gloss plane as glTF ORM. R/B are neutral multipliers;
/// this function does not claim to have measured AO or metallic content.
pub fn orm_from_gloss(width: u32, height: u32, gloss: &[f32]) -> Result<Texture> {
    let n = (width as usize)
        .checked_mul(height as usize)
        .filter(|&n| n > 0)
        .ok_or_else(|| invalid("Sekiro PBR", 0, "invalid gloss dimensions"))?;
    if n != gloss.len() {
        return Err(invalid("Sekiro PBR", 0, "gloss plane length mismatch"));
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(
        n.checked_mul(4)
            .ok_or_else(|| invalid("Sekiro PBR", 0, "gloss size overflow"))?,
    )
    .map_err(|e| invalid("Sekiro PBR", 0, e))?;
    for &g in gloss {
        rgba.extend_from_slice(&[255, (roughness(g)? * 255.).round() as u8, 255, 255]);
    }
    Ok(Texture {
        width,
        height,
        format: TexFormat::Rgba8Unorm,
        levels: vec![rgba],
        color_space: ColorSpace::Linear,
        role: TexRole::Mask,
        normal: None,
    })
}

fn simple(shader: &str) -> bool {
    matches!(
        shader,
        "MetallicTex" | "MetallicValue" | "MetallicValue_MeshDecal"
    )
}
fn binding<'a>(m: &'a Material, role: &str) -> Option<&'a Binding> {
    let d = m.definition.as_ref()?;
    d.textures
        .iter()
        .filter(|t| t.kind.contains(role))
        .find_map(|t| m.bindings.get(&t.kind))
}

/// Names required by the evidenced static policies, for one batched DDS read.
/// Layered base/normal textures must already be baked and bound by assembly.
pub fn required_textures(world: &World) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for (_, m) in world.iter::<Material>() {
        if m.skip.is_some() || m.layered.is_some() {
            continue;
        }
        names.extend(m.albedo.iter().cloned());
        names.extend(m.normal.iter().cloned());
        if simple(&m.shader)
            && let Some(b) = binding(m, "MetallicMap")
        {
            names.insert(b.texture.clone());
        }
    }
    Ok(names)
}

fn read(payloads: &BTreeMap<String, Vec<u8>>, name: &str) -> Result<Texture> {
    let bytes = payloads
        .get(name)
        .ok_or_else(|| invalid("Sekiro PBR", 0, format!("missing DDS {name}")))?;
    primase_dds::read(bytes).map_err(|e| invalid("Sekiro PBR DDS", 0, e))
}
fn pixels(t: &Texture) -> Result<Vec<u8>> {
    let level = t
        .levels
        .first()
        .ok_or_else(|| invalid("Sekiro PBR", 0, "missing base mip"))?;
    primase_texcodec::Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, level)
        .map_err(|e| invalid("Sekiro PBR pixels", 0, e))
}
fn spawn(world: &mut World, name: String, t: Texture) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(name));
    world.insert(e, t);
    e
}
fn texture(
    world: &mut World,
    payloads: &BTreeMap<String, Vec<u8>>,
    cache: &mut BTreeMap<(String, bool), Entity>,
    name: &str,
    normal: bool,
) -> Result<Entity> {
    let key = (name.to_owned(), normal);
    if let Some(e) = cache.get(&key) {
        return Ok(*e);
    }
    let mut t = read(payloads, name)?;
    t.role = if normal { TexRole::Normal } else { TexRole::Color };
    t.color_space = if normal {
        ColorSpace::Linear
    } else {
        ColorSpace::Srgb
    };
    t.normal = normal.then_some(super::normals::SOURCE);
    let e = spawn(
        world,
        format!("{name}@{}", if normal { "normal" } else { "color" }),
        t,
    );
    cache.insert(key, e);
    Ok(e)
}
fn transform(m: &Material, b: &Binding) -> Result<UvTransform> {
    if b.uv_number != 1
        || !m.uv_scale.iter().all(|v| v.is_finite() && *v > 0.)
        || !b.scale.iter().all(|v| v.is_finite())
    {
        return Err(invalid("Sekiro PBR", 0, "unsupported UV set or invalid scale"));
    }
    Ok(UvTransform {
        scale: std::array::from_fn(|i| b.scale[i] / m.uv_scale[i]),
        ..UvTransform::IDENTITY
    })
}

/// Lower static source materials to core PBR without serializing through a
/// consumer dialect. A missing required DDS is an error. Baked variants must
/// carry PBR base/normal bindings; these are never replaced by source layers.
/// Approximation units are material entities unless a counter says pixels.
pub fn lower(world: &mut World, payloads: &BTreeMap<String, Vec<u8>>) -> Result<PassLog> {
    let sources: Vec<_> = world.iter::<Material>().map(|(e, m)| (e, m.clone())).collect();
    let mut log = PassLog::default();
    let mut cache = BTreeMap::new();
    for (e, m) in sources {
        if m.skip.is_some() {
            continue;
        }
        let baked = m.layered.is_some();
        let existing = world.get::<PbrMaterial>(e).cloned();
        if baked && existing.is_none() {
            return Err(invalid(
                "Sekiro PBR",
                0,
                "layered material requires explicit baked PBR bindings",
            ));
        }
        let mut p = existing.unwrap_or_default();
        if !baked {
            if p.base_color_texture.is_none()
                && let Some(name) = &m.albedo
            {
                p.base_color_texture = Some(TextureRef::new(texture(
                    world, payloads, &mut cache, name, false,
                )?));
            }
            if p.normal_texture.is_none()
                && let Some(name) = &m.normal
            {
                let mut r = TextureRef::new(texture(world, payloads, &mut cache, name, true)?);
                if let Some(b) = binding(&m, "NormalMap") {
                    r.transform = Some(transform(&m, b)?);
                }
                p.normal_texture = Some(r);
            }
            p.alpha_mode = match m.class.as_str() {
                "leaves" | "cutout" => AlphaMode::Mask { cutoff: m.cutoff },
                "alpha" | "decal" | "water" => AlphaMode::Blend,
                _ => AlphaMode::Opaque,
            };
            p.double_sided = m.two_sided;
        }
        if baked {
            log.approximate("layered PBR channels require shader bake outputs", 1);
        } else if simple(&m.shader) {
            if let Some(name) = &m.normal {
                let normal = read(payloads, name)?;
                let rgba = pixels(&normal)?;
                let metal = binding(&m, "MetallicMap");
                let mut metal_sampler = None;
                if let Some(b) = metal {
                    let t = read(payloads, &b.texture)?;
                    metal_sampler = Some(crate::layer_bake::BaseMipBilinearWrap::new(
                        t.width,
                        t.height,
                        &pixels(&t)?,
                        false,
                    )?);
                }
                let normal_binding = binding(&m, "NormalMap")
                    .ok_or_else(|| invalid("Sekiro PBR", 0, "normal binding absent"))?;
                let nt = transform(&m, normal_binding)?;
                let mt = metal.map(|b| transform(&m, b)).transpose()?;
                if nt.scale.contains(&0.) {
                    return Err(invalid("Sekiro PBR", 0, "zero normal scale"));
                }
                let mut orm = Vec::with_capacity(rgba.len());
                for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
                    let uv = [
                        (i as u32 % normal.width) as f32 + 0.5,
                        (i as u32 / normal.width) as f32 + 0.5,
                    ];
                    let uv = [uv[0] / normal.width as f32, uv[1] / normal.height as f32];
                    let metallic = if let (Some(s), Some(mt)) = (&metal_sampler, mt) {
                        s.sample(std::array::from_fn(|j| uv[j] * mt.scale[j] / nt.scale[j]))?[0]
                    } else {
                        p.metallic
                    };
                    orm.extend_from_slice(&[
                        255,
                        (roughness(px[2] as f32 / 255.)? * 255.).round() as u8,
                        (metallic * 255.).round() as u8,
                        255,
                    ]);
                }
                let t = Texture {
                    width: normal.width,
                    height: normal.height,
                    format: TexFormat::Rgba8Unorm,
                    levels: vec![orm],
                    color_space: ColorSpace::Linear,
                    role: TexRole::Mask,
                    normal: None,
                };
                let te = spawn(world, format!("{}_orm_{}", name, e.index()), t);
                let mut r = TextureRef::new(te);
                r.transform = Some(nt);
                p.metallic_roughness_texture = Some(r);
                p.roughness = 1.;
                p.metallic = 1.;
                log.count("roughness textures", 1);
                log.approximate("per-light gloss correction omitted", 1);
                log.approximate("roughness quantized to UNORM8", 1);
                if metal.is_some() {
                    log.count("metallic textures", 1);
                    log.approximate("dielectric F0 0.03 represented by glTF 0.04", 1);
                    log.approximate("metal resampled at normal base-mip texel centers", 1);
                } else {
                    log.approximate("runtime metallic constant unknown; retained target default", 1);
                }
            } else {
                log.approximate("missing normal gloss; retained target roughness", 1);
            }
        } else {
            log.approximate("unclassified shader PBR; retained target factors", 1);
        }
        if m.definition.is_none() {
            log.approximate("missing material definition", 1);
        }
        if m.is_emissive || m.shader.contains("Emissive") || m.emissive.is_some() {
            log.approximate("source emission without proven radiance; retained dialect", 1);
        }
        // A static parameter may exist only in a subset of shader permutations;
        // retaining all definitions lets later policies establish its consumer.
        if m.definition.as_ref().is_some_and(|d| {
            d.parameters
                .iter()
                .any(|v| matches!(&v.value,Value::Float(x) if x.iter().any(|f|!f.is_finite())))
        }) {
            return Err(invalid("Sekiro PBR", 0, "nonfinite source parameter"));
        }
        world.insert(e, p);
        log.count("PBR materials", 1);
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gloss_matches_deferred_ggx_alpha() {
        assert_eq!(roughness(0.).unwrap(), 1.);
        assert!((roughness(1.).unwrap() - 0.05).abs() < 1e-6);
        assert!((roughness(0.25).unwrap() - 0.525).abs() < 1e-6);
        for bad in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            assert!(roughness(bad).is_err());
        }
    }
    #[test]
    fn gloss_pack_has_orm_channels_and_rejects_invalid_planes() {
        let t = orm_from_gloss(3, 1, &[0., 0.25, 1.]).unwrap();
        assert_eq!(
            t.levels[0],
            [255, 255, 255, 255, 255, 134, 255, 255, 255, 13, 255, 255]
        );
        assert_eq!(t.role, TexRole::Mask);
        assert_eq!(t.color_space, ColorSpace::Linear);
        assert!(orm_from_gloss(2, 2, &[0.; 3]).is_err());
        assert!(orm_from_gloss(0, 1, &[]).is_err());
        assert!(orm_from_gloss(1, 1, &[f32::NAN]).is_err());
    }
}
