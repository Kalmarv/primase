//! Lowering `skate.material` to `pbr.material` (glTF 2.0's metal-rough model) and `skate.light` to `core.light`,
//! keeping the source: the skate components stay, so a skate-scene writer still has them and a glTF writer carries
//! the material in extras.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, TextureRef, Wrap as PbrWrap};
use primase_core::{
    ColorSpace, Entity, Light, LightKind, Name, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat,
    TexRole, Texture, Transform, World,
};

use crate::components::{AlphaMode, MaterialClass, SkateLight, SkateMaterial, SkateTexture, Wrap};

/// Register the dialect's lowerings.
pub(crate) fn register(reg: &mut PassRegistry) {
    reg.register("lower-layered-pbr", |o: &Options| {
        if let Some(k) = o.keys().next() {
            return Err(format!("unknown option {k}"));
        }
        Ok(Box::new(super::LowerLayeredPbr))
    });
    reg.register("lower-skate-pbr", |o: &Options| {
        if let Some(k) = o.keys().next() {
            return Err(format!("unknown option {k}"));
        }
        Ok(Box::new(LowerSkatePbr))
    });
    reg.register("lower-skate-lights", |o: &Options| {
        if let Some(k) = o.keys().next() {
            return Err(format!("unknown option {k}"));
        }
        Ok(Box::new(LowerSkateLights))
    });
}

/// `skate.material` -> `pbr.material` with skate-scene v1's class defaults (see [`lower_material`]). Also sets what
/// the scene says about each loaded `core.texture` its materials use: role (colour, normal, mask), colour space
/// (`color_space`, default sRGB; normal and mask textures linear) and a normal map's convention (`green`, `z`), so a
/// normal-conversion pass and the glTF writer see them.
///
/// A loaded Studio MSK mask (R metallic, G smoothness, B ambient occlusion, A opacity; linear) becomes a derived
/// `core.texture` in glTF's packed layout, named `<mask>@orm`, linear, role mask: R occlusion = MSK B, G roughness =
/// 255 - MSK G, B metallic = MSK R, A 255, every mip level (each output byte depends on one input byte, so the
/// mapping commutes with the mips). It is bound as both `metallic_roughness_texture` and `occlusion_texture`, with
/// metallic and roughness factors 1 so the mask's values pass through. Roughness is `1 - smoothness`: Skate is a
/// Frostbite title, whose smoothness is one minus perceptual roughness (Lagarde and de Rousiers, "Moving Frostbite
/// to Physically Based Rendering", 2014: `alpha = (1 - smoothness)^2`), and glTF's roughness is that perceptual
/// roughness (`alpha = roughness^2`). The MSK layout is the Studio mask's as written by the ReSkate kit
/// (`kit/docs/SCENE_INTERCHANGE.md`, materials `mask`) and confirmed in game (an MSK compiled by Studio rendered
/// with G as gloss). The skate material keeps its MSK, so skate-scene writers are unaffected.
#[derive(Debug, Default)]
pub struct LowerSkatePbr;

impl Pass for LowerSkatePbr {
    fn name(&self) -> &'static str {
        "lower-skate-pbr"
    }
    /// A part without skate materials (a tile of only a light) has nothing to lower.
    fn part_may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.material"]
    }
    fn provides(&self) -> &[&'static str] {
        // an MSK mask's ORM texture is a new entity, named after the mask
        &["pbr.material", "core.texture", "core.name"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    /// It lowers skate materials and keeps them.
    fn legalizes(&self) -> &[&'static str] {
        &["skate.material"]
    }
    fn approximates(&self) -> &'static str {
        "Studio MSK masks not loaded as RGBA8 or with inconsistent mip sizes left out; a material's scalar metallic and roughness ignored where its \
         MSK is bound; wrap clip written as clamp; normal maps still files in a non-glTF convention left out; vertex-alpha \
         cutout materials blended without their cutoff"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut roles: Vec<(Entity, TexRole, ColorSpace, Option<NormalConvention>)> = Vec::new();
        for (_, m) in w.iter::<SkateMaterial>() {
            for (t, role) in [
                (m.base, TexRole::Color),
                (m.normal, TexRole::Normal),
                (m.mask, TexRole::Mask),
            ] {
                let Some(t) = t else { continue };
                let st = w.get::<SkateTexture>(t);
                let linear = role != TexRole::Color || st.and_then(|s| s.linear).unwrap_or(false);
                let normal = (role == TexRole::Normal).then(|| NormalConvention {
                    green: st.and_then(|s| s.green).unwrap_or(NormalGreen::Up),
                    z: st.and_then(|s| s.z).unwrap_or(NormalZ::Stored),
                    x: NormalX::Red,
                    basis: primase_core::NormalBasis::TangentBitangent,
                });
                roles.push((
                    t,
                    role,
                    if linear {
                        ColorSpace::Linear
                    } else {
                        ColorSpace::Srgb
                    },
                    normal,
                ));
            }
        }
        for (t, role, cs, normal) in roles {
            if let Some(tex) = w.get_mut::<Texture>(t) {
                if (tex.role, tex.color_space, tex.normal) != (role, cs, normal) {
                    log.count("texture metadata set", 1);
                }
                tex.role = role;
                tex.color_space = cs;
                tex.normal = normal;
            }
        }
        let masks: BTreeSet<Entity> = w.iter::<SkateMaterial>().filter_map(|(_, m)| m.mask).collect();
        let mut orm = BTreeMap::new();
        for mask in masks {
            match w.get::<Texture>(mask).map(msk_to_orm) {
                Some(Ok(texture)) => {
                    let name = w
                        .get::<Name>(mask)
                        .map_or_else(|| format!("texture_{}", mask.index()), |n| n.0.clone());
                    let e = w.spawn();
                    w.insert(e, texture);
                    w.insert(e, Name(format!("{name}@orm")));
                    orm.insert(mask, e);
                    log.count("MSK masks lowered to ORM textures", 1);
                }
                None | Some(Err(MskError::NotRgba8)) => {
                    log.approximate("Studio MSK mask not loaded as RGBA8 left out", 1);
                }
                Some(Err(MskError::InconsistentLevels)) => {
                    log.approximate("Studio MSK mask with inconsistent mip sizes left out", 1);
                }
                Some(Err(MskError::Mapping(error))) => return Err(error.to_string()),
            }
        }
        let lowered: Vec<(Entity, PbrMaterial)> = w
            .iter::<SkateMaterial>()
            .filter(|(_, m)| m.class != MaterialClass::Layered)
            .map(|(e, m)| {
                (
                    e,
                    lower_material(m, m.mask.and_then(|t| orm.get(&t).copied()), w, log),
                )
            })
            .collect();
        for (e, p) in lowered {
            w.insert(e, p);
        }
        Ok(())
    }
}

/// `skate.light` -> `core.light`, the inverse of [`crate::passes::raise_skate`]'s lights: intensity in candela from the
/// Studio watts by the same named policy (`P = 4 pi^2 I`, [`crate::passes::studio_watts`]); a spot's outer cone half its full
/// `spot_size` and the inner `outer x (1 - spot_blend)`; range 0 unlimited. The entity gets a `core.transform` at
/// `position` whose local -Z is `direction` and whose +Y is as near world +Y as possible (+Z when the light points
/// straight up or down): a point light and a round spot cone look the same under any roll, so `tangent` is not
/// used. Time-of-day mask, radius, shadows, priority and group stay in the dialect. Lights that already have a
/// `core.light` are left as they are; one it cannot lower (a non-finite or zero direction) gets none, counted as
/// omitted (a conversion's unsupported notice).
#[derive(Debug, Default)]
pub struct LowerSkateLights;

impl Pass for LowerSkateLights {
    fn name(&self) -> &'static str {
        "lower-skate-lights"
    }
    /// A part without skate lights has nothing to lower.
    fn part_may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.light"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.light", "core.transform"]
    }
    /// It lowers skate lights and keeps them.
    fn legalizes(&self) -> &[&'static str] {
        &["skate.light"]
    }
    fn approximates(&self) -> &'static str {
        "light energy as candela (Studio watts / 4 pi^2); time of day, radius, priority and group not lowered"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let lights: Vec<(Entity, SkateLight)> = w
            .iter::<SkateLight>()
            .filter(|(e, _)| !w.has::<Light>(*e))
            .map(|(e, l)| (e, l.clone()))
            .collect();
        for (e, l) in lights {
            // a light it cannot lower (a non-finite or zero direction) is left out of core, as unsupported
            match lower_light(&l) {
                Ok((light, transform)) => {
                    w.insert(e, light);
                    w.insert(e, transform);
                    log.count("lights", 1);
                }
                Err(why) => log.omit(&format!("lights not lowered: {why}"), 1),
            }
        }
        Ok(())
    }
}

/// One light; an error for a non-finite or zero direction.
pub fn lower_light(l: &SkateLight) -> Result<(Light, Transform), String> {
    let kind = if l.spot {
        let outer = l.spot_size / 2.0;
        LightKind::Spot {
            inner_cone: outer * (1.0 - l.spot_blend.clamp(0.0, 1.0)),
            outer_cone: outer,
        }
    } else {
        LightKind::Point
    };
    // the inverse of raise-skate's default calibration (crate::passes::SKATE_LIGHT_SCALE)
    let intensity = (f64::from(l.energy)
        / (4.0 * std::f64::consts::PI * std::f64::consts::PI)
        / crate::passes::SKATE_LIGHT_SCALE) as f32;
    let light = Light {
        kind,
        color: l.color,
        intensity,
        range: (l.range > 0.0).then_some(l.range),
        cast_shadows: Some(l.cast_shadow),
    };
    let transform =
        Transform::facing(l.position, l.direction).ok_or("a light direction that is zero or not finite")?;
    Ok((light, transform))
}

/// sRGB-encoded byte -> linear (IEC 61966-2-1 piecewise curve), in f64, rounded once to f32.
fn srgb_to_linear(c: u8) -> f32 {
    primase_image::srgb_to_linear(f64::from(c) / 255.0) as f32
}

/// One material, with skate-scene v1's defaults: alpha `mask` for cutout/leaves, `blend` for
/// decal/glass/water/alpha, and `blend` for any `vertex_alpha` material (its parts fade by COLOR_0's alpha); cutoff 0.5 (leaves 0.33); two-sided for cutout/leaves; metallic 0; roughness 0.85
/// (glass/water 0.15). `base` -> base colour texture; without it `base_color` (sRGB bytes, default 128 grey) -> the
/// factor; `tint` (sRGB bytes, multiplied in linear light) -> the factor, which glTF multiplies into the texture in
/// linear light too; `normal` -> normal texture, unless it is still only a file in a convention other than glTF's
/// (green up, z stored: loaded textures are converted by a normal pass); `wrap` -> the base texture's sampler.
/// `orm`: the material's MSK lowered to glTF's packed occlusion-roughness-metallic texture (see [`LowerSkatePbr`]),
/// bound as metallic-roughness and occlusion with metallic and roughness factors 1; without it the scalar metallic
/// and roughness. Counted in `log`: materials, scalars ignored for an MSK, clip as clamp, normals left out.
pub fn lower_material(m: &SkateMaterial, orm: Option<Entity>, w: &World, log: &mut PassLog) -> PbrMaterial {
    use MaterialClass as C;
    log.count("materials", 1);
    let mut alpha = m.alpha.unwrap_or(match m.class {
        C::Cutout | C::Leaves => AlphaMode::Mask,
        C::Decal | C::Glass | C::Water | C::Alpha => AlphaMode::Blend,
        _ => AlphaMode::Opaque,
    });
    // glTF multiplies COLOR_0's alpha into the base colour's; only blending shows the fade.
    if m.vertex_alpha == Some(true) && alpha != AlphaMode::Blend {
        if alpha == AlphaMode::Mask {
            log.approximate("vertex-alpha cutout materials blended, cutoff dropped", 1);
        } else {
            log.count("vertex-alpha materials blended", 1);
        }
        alpha = AlphaMode::Blend;
    }
    let cutoff = m.cutoff.unwrap_or(if m.class == C::Leaves { 0.33 } else { 0.5 }) as f32;
    let wrap = match m.wrap.unwrap_or(Wrap::Repeat) {
        Wrap::Repeat => PbrWrap::Repeat,
        Wrap::Extend => PbrWrap::Clamp,
        Wrap::Clip => {
            log.approximate("wrap clip written as clamp", 1);
            PbrWrap::Clamp
        }
        Wrap::Mirror => PbrWrap::Mirror,
    };
    let mut base_color = [1.0f32; 4];
    let base_color_texture = match m.base {
        Some(t) => Some(TextureRef {
            texture: t,
            texcoord: 0,
            wrap: [wrap; 2],
            transform: None,
        }),
        None => {
            let c = m.base_color.unwrap_or([128, 128, 128, 255]);
            for i in 0..3 {
                base_color[i] = srgb_to_linear(c[i]);
            }
            base_color[3] = (f64::from(c[3]) / 255.0) as f32;
            None
        }
    };
    if let Some(t) = m.tint {
        for i in 0..3 {
            base_color[i] *= srgb_to_linear(t[i]);
        }
    }
    let normal_texture = m.normal.and_then(|t| {
        let gltf_convention = w.get::<SkateTexture>(t).is_none_or(|x| {
            x.green.unwrap_or(NormalGreen::Up) == NormalGreen::Up
                && x.z.unwrap_or(NormalZ::Stored) == NormalZ::Stored
        });
        if gltf_convention || w.has::<Texture>(t) {
            Some(TextureRef::new(t))
        } else {
            log.approximate("normal map still a file in a non-glTF convention left out", 1);
            None
        }
    });
    let glassy = matches!(m.class, C::Glass | C::Water);
    // With an ORM bound, metallicFactor and roughnessFactor are 1 so the mask's values pass through unscaled.
    if orm.is_some() && (m.metallic.is_some() || m.roughness.is_some()) {
        log.approximate(
            "scalar metallic/roughness ignored where an MSK is bound (factors 1)",
            1,
        );
    }
    let orm_ref = orm.map(|t| TextureRef {
        texture: t,
        texcoord: 0,
        wrap: [wrap; 2],
        transform: None,
    });
    PbrMaterial {
        base_color,
        base_color_texture,
        metallic: if orm.is_some() {
            1.0
        } else {
            m.metallic.unwrap_or(0.0) as f32
        },
        roughness: if orm.is_some() {
            1.0
        } else {
            m.roughness.unwrap_or(if glassy { 0.15 } else { 0.85 }) as f32
        },
        metallic_roughness_texture: orm_ref,
        occlusion_texture: orm_ref,
        occlusion_strength: 1.0,
        normal_texture,
        alpha_mode: match alpha {
            AlphaMode::Mask => PbrAlpha::Mask { cutoff },
            AlphaMode::Blend => PbrAlpha::Blend,
            AlphaMode::Opaque => PbrAlpha::Opaque,
        },
        double_sided: m.two_sided.unwrap_or(matches!(m.class, C::Cutout | C::Leaves)),
        cast_shadows: m.cast_shadows,
        ..PbrMaterial::default()
    }
}

/// Why a Studio mask could not be mapped to packed ORM.
#[derive(Debug, thiserror::Error)]
pub enum MskError {
    #[error("Studio mask is not loaded as RGBA8")]
    NotRgba8,
    #[error("Studio mask has inconsistent mip dimensions or sizes")]
    InconsistentLevels,
    /// Operational failures are errors, not approximations about the input format.
    #[error(transparent)]
    Mapping(primase_image::Error),
}

/// A Studio MSK (RGBA8: R metallic, G smoothness, B occlusion, A opacity) as glTF's packed texture: R occlusion,
/// G roughness = 255 - smoothness, B metallic, A 255; every level. Errors distinguish
/// unsupported storage from inconsistent mip sizes; no partial output is returned.
pub fn msk_to_orm(msk: &Texture) -> Result<Texture, MskError> {
    if msk.format != TexFormat::Rgba8Unorm {
        return Err(MskError::NotRgba8);
    }
    primase_image::map_rgba8_levels(
        msk,
        0..msk.levels.len(),
        primase_image::TextureMetadata {
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        },
        |texel| {
            let p = texel.rgba;
            Ok([p[2], 255 - p[1], p[0], 255])
        },
    )
    .map_err(|error| match error {
        primase_image::Error::Invalid(_) => MskError::InconsistentLevels,
        error => MskError::Mapping(error),
    })
}
