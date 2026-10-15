//! Lowering `skate.material` to `pbr.material` (glTF 2.0's metal-rough model), keeping the source: the skate
//! material stays, so a skate-scene writer still has it and a glTF writer carries it in extras.

use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, TextureRef, Wrap as PbrWrap};
use primase_core::{
    ColorSpace, Entity, NormalConvention, NormalGreen, NormalX, NormalZ, TexRole, Texture, World,
};

use crate::components::{AlphaMode, MaterialClass, SkateMaterial, SkateTexture, Wrap};

/// Register this crate's passes.
pub fn register(reg: &mut PassRegistry) {
    reg.register("lower-skate-pbr", |o: &Options| {
        if let Some(k) = o.keys().next() {
            return Err(format!("unknown option {k}"));
        }
        Ok(Box::new(LowerSkatePbr))
    });
}

/// `skate.material` -> `pbr.material` with skate-scene v1's class defaults (see [`lower_material`]). Also sets what
/// the scene says about each loaded `core.texture` its materials use: role (colour, normal, mask), colour space
/// (`color_space`, default sRGB; normal and mask textures linear) and a normal map's convention (`green`, `z`), so a
/// normal-conversion pass and the glTF writer see them.
#[derive(Debug, Default)]
pub struct LowerSkatePbr;

impl Pass for LowerSkatePbr {
    fn name(&self) -> &'static str {
        "lower-skate-pbr"
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.material"]
    }
    fn provides(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn approximates(&self) -> &'static str {
        "Studio MSK mask textures left out; wrap clip written as clamp; normal maps still files in a non-glTF \
         convention left out"
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
        let lowered: Vec<(Entity, PbrMaterial)> = w
            .iter::<SkateMaterial>()
            .map(|(e, m)| (e, lower_material(m, w, log)))
            .collect();
        for (e, p) in lowered {
            w.insert(e, p);
        }
        Ok(())
    }
}

/// sRGB-encoded byte -> linear (IEC 61966-2-1 piecewise curve), in f64, rounded once to f32.
fn srgb_to_linear(c: u8) -> f32 {
    let c = f64::from(c) / 255.0;
    (if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }) as f32
}

/// One material, with skate-scene v1's defaults: alpha `mask` for cutout/leaves, `blend` for
/// decal/glass/water/alpha; cutoff 0.5 (leaves 0.33); two-sided for cutout/leaves; metallic 0; roughness 0.85
/// (glass/water 0.15). `base` -> base colour texture; without it `base_color` (sRGB bytes, default 128 grey) -> the
/// factor; `tint` (sRGB bytes, multiplied in linear light) -> the factor, which glTF multiplies into the texture in
/// linear light too; `normal` -> normal texture, unless it is still only a file in a convention other than glTF's
/// (green up, z stored: loaded textures are converted by a normal pass); `wrap` -> the base texture's sampler.
/// Counted in `log`: materials, masks not lowered, clip as clamp, normals left out.
pub fn lower_material(m: &SkateMaterial, w: &World, log: &mut PassLog) -> PbrMaterial {
    use MaterialClass as C;
    log.count("materials", 1);
    let alpha = m.alpha.unwrap_or(match m.class {
        C::Cutout | C::Leaves => AlphaMode::Mask,
        C::Decal | C::Glass | C::Water | C::Alpha => AlphaMode::Blend,
        _ => AlphaMode::Opaque,
    });
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
    if m.mask.is_some() {
        log.approximate("Studio MSK mask texture left out", 1);
    }
    let glassy = matches!(m.class, C::Glass | C::Water);
    PbrMaterial {
        base_color,
        base_color_texture,
        metallic: m.metallic.unwrap_or(0.0) as f32,
        roughness: m.roughness.unwrap_or(if glassy { 0.15 } else { 0.85 }) as f32,
        normal_texture,
        alpha_mode: match alpha {
            AlphaMode::Mask => PbrAlpha::Mask { cutoff },
            AlphaMode::Blend => PbrAlpha::Blend,
            AlphaMode::Opaque => PbrAlpha::Opaque,
        },
        double_sided: m.two_sided.unwrap_or(matches!(m.class, C::Cutout | C::Leaves)),
        ..PbrMaterial::default()
    }
}
