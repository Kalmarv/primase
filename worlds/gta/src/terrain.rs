//! `gta-terrain-layers`: GTA's four-layer terrain shaders (`terrain_cb_*`) as `core.layered` materials.
//!
//! The terrain pixel shaders weigh their four layers per pixel by c = saturate(vertex colour 1) with
//! w0 = (1-r)(1-g)(1-b), w1 = (1-r)(1-g)b, w2 = (1-r)g(1-b), w3 = (1-r)gb, and sum the layers' diffuse and normals with
//! those weights: core's `rgb_product_4` on COLOR_1. A terrain material gets a `core.layered` whose slots are layer
//! materials (`DiffuseTexture_layer0..3`; a layer whose texture the lookup did not find is a null slot), each a
//! `pbr.material` with the terrain material's own roughness and specular, metallic 0, opaque. The terrain material
//! gets no `pbr.material` of its own; one with no layer texture found gets a grey one instead of a blend.
//!
//! Specular, as the terrain pixel shaders write it (no layer has its own): intensity `SpecularColor`^2 (dry), power
//! `Specular`, Fresnel 0.98; `*_spm` effects take `specularIntensityMultSpecMap` and `specularFalloffMultSpecMap`
//! scaled per pixel by the layers' height maps, which is not lowered (counted). In `KHR_materials_specular` terms
//! (see [`crate::lower`]) factor the intensity, colour (1 - 0.98) / 0.04, roughness [`crate::lower::roughness`] of
//! the power.
//!
//! The lookup-blended effects (`*_cm*`, `*_2tex_blend*`) take green and blue per pixel from `lookupTexture` at
//! TEXCOORD1 instead, mixed with vertex colour 1's by vertex colour 0's alpha (`*_cm*`: the lookup alone); the
//! `*_lod` ones take vertex colour 1 alone. Core's weights are per vertex, so for those COLOR_1's green and blue become
//! the lookup sampled at each vertex (point, wrapped, the top level): an approximation, counted. A terrain primitive
//! without vertex colour 1 gets one of (0, 0, 0, 255) where no lookup applies (layer 0 alone, as the shader's
//! saturate of a missing input gives). Tinted terrain (`*_tnt`) is layered without its tint, counted.
use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog, pass_text};
use primase_core::pbr::{AlphaMode, PbrMaterial, Specular, TextureRef};
use primase_core::{
    ColorSpace, Entity, Layer, LayerWeights, LayeredMaterial, Semantic, Stream, TexRole, Texture,
    WeightMapping, World,
};
use primase_texcodec::{Decoder, TexDecoder};

use crate::components::GtaShader;
use crate::lower::{DEFAULT_FRESNEL, GREY, classes, for_each_mesh, roughness, scalar, texture};
use crate::shader_class::Class;

/// How a terrain effect gets green and blue of its weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weights {
    /// Vertex colour 1 alone.
    VertexColour,
    /// The lookup texture alone (`*_cm*`).
    Lookup,
    /// The lookup texture mixed with vertex colour 1 by vertex colour 0's alpha (`*_2tex_blend*`).
    LookupBlend,
}

/// How the terrain effect `effect` weighs its layers.
pub fn weights_of(effect: &str) -> Weights {
    if effect.ends_with("_lod") {
        Weights::VertexColour
    } else if effect.contains("_cm") {
        Weights::Lookup
    } else if effect.contains("_2tex_blend") {
        Weights::LookupBlend
    } else {
        Weights::VertexColour
    }
}

/// One vertex's weight channels: vertex colour 1 (`None`: absent), the lookup's green and blue at the vertex (`None`:
/// no lookup), vertex colour 0's alpha, as the effect's `weights` combine them. Bytes, alpha 255 without vertex
/// colour 1.
pub fn channels(weights: Weights, colour1: Option<[u8; 4]>, lookup: Option<[u8; 2]>, alpha0: u8) -> [u8; 4] {
    let c1 = colour1.unwrap_or([0, 0, 0, 255]);
    let Some([lg, lb]) = lookup.filter(|_| weights != Weights::VertexColour) else {
        return c1;
    };
    let f = if weights == Weights::Lookup {
        0.0
    } else {
        f32::from(alpha0) / 255.0
    };
    let mix = |l: u8, v: u8| (f32::from(l) * (1.0 - f) + f32::from(v) * f).round() as u8;
    [c1[0], mix(lg, c1[1]), mix(lb, c1[2]), c1[3]]
}

/// A decoded lookup texture.
struct Lookup {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Lookup {
    /// Green and blue at `uv`, point sampled with wrapping.
    fn sample(&self, [u, v]: [f32; 2]) -> [u8; 2] {
        let texel =
            |t: f32, n: u32| (((t - t.floor()) * n as f32).floor() as i64).clamp(0, i64::from(n) - 1) as u32;
        let o = ((texel(v, self.height) * self.width + texel(u, self.width)) * 4) as usize;
        [self.rgba[o + 1], self.rgba[o + 2]]
    }
}

/// `gta-terrain-layers` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GtaTerrainLayers;

impl Pass for GtaTerrainLayers {
    fn name(&self) -> &'static str {
        "gta-terrain-layers"
    }
    // no `requires`: it runs on every loaded part and leaves one without its input (a part holding only a water
    // surface, a cell whose shaders all draw nothing) as it is
    fn provides(&self) -> &[&'static str] {
        &["core.layered", "pbr.material", "core.provenance"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "gta.sources", "core.texture"]
    }
    fn approximates(&self) -> &'static str {
        "lookup-blended terrain weighed per vertex (the lookup sampled at each vertex); terrain tint not applied; \
         the specular-map terrains' per-pixel height term left out; the Blinn-Phong lobe as GGX, dry"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let classes = classes(w)?;
        let mut weights: BTreeMap<Entity, (Weights, Option<Lookup>)> = BTreeMap::new();
        let mut layered = Vec::new();
        let mut grey = Vec::new();
        for (&m, c) in &classes {
            if c.class != Class::Terrain || c.skip.is_some() {
                continue;
            }
            let s = w.get::<GtaShader>(m).expect("shader");
            let effect = crate::shader_names::effect(s.name).expect("named effect");
            if c.tint.is_some() {
                log.approximate("tinted terrain layered without its tint", 1);
            }
            let (intensity, power) = match scalar(s, "SpecularColor") {
                Some(i) => (Some(i * i), scalar(s, "Specular")),
                None => {
                    let i = scalar(s, "specularIntensityMultSpecMap");
                    if i.is_some() {
                        log.approximate("specular-map terrain without its per-pixel height term", 1);
                    }
                    (i, scalar(s, "specularFalloffMultSpecMap"))
                }
            };
            let rough = power.map_or(1.0, roughness);
            let specular = match intensity {
                Some(i) => Some(Specular {
                    factor: i.clamp(0.0, 1.0),
                    texture: None,
                    color: [(1.0 - DEFAULT_FRESNEL) / 0.04; 3],
                    color_texture: None,
                }),
                None => {
                    log.count(
                        "terrain materials without a specular intensity: specular unstated",
                        1,
                    );
                    None
                }
            };
            let slots: Vec<Option<Entity>> = (0..4)
                .map(|i| texture(s, &format!("DiffuseTexture_layer{i}")).flatten())
                .collect();
            let mode = weights_of(effect);
            let lookup = match (mode, texture(s, "lookupTexture").flatten()) {
                (Weights::VertexColour, _) | (_, None) => None,
                (_, Some(t)) => {
                    let t = w.get::<Texture>(t).ok_or("lookup is not a loaded texture")?;
                    let rgba = Decoder::default()
                        .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
                        .map_err(|e| format!("lookup: {e}"))?;
                    Some(Lookup {
                        width: t.width,
                        height: t.height,
                        rgba,
                    })
                }
            };
            weights.insert(m, (mode, lookup));
            if slots.iter().all(Option::is_none) {
                grey.push((m, specular));
            } else {
                layered.push((m, slots, rough, specular));
            }
        }
        let srgb = |b: u8| primase_image::srgb_to_linear(f64::from(b) / 255.0) as f32;
        for (m, specular) in grey {
            log.count("terrain materials with no layer texture: grey", 1);
            w.insert(
                m,
                PbrMaterial {
                    base_color: [srgb(GREY), srgb(GREY), srgb(GREY), 1.0],
                    metallic: 0.0,
                    specular,
                    ..PbrMaterial::default()
                },
            );
        }
        for (m, slots, rough, specular) in layered {
            let mut layers = Vec::with_capacity(4);
            for slot in slots {
                let Some(t) = slot else {
                    log.count("terrain layer slots without a texture found: null", 1);
                    layers.push(None);
                    continue;
                };
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && (tex.role, tex.color_space) != (TexRole::Color, ColorSpace::Srgb)
                {
                    tex.role = TexRole::Color;
                    tex.color_space = ColorSpace::Srgb;
                    log.count("layer textures marked sRGB colour", 1);
                }
                let e = w.spawn();
                w.insert(
                    e,
                    PbrMaterial {
                        base_color_texture: Some(TextureRef::new(t)),
                        metallic: 0.0,
                        roughness: rough,
                        specular,
                        alpha_mode: AlphaMode::Opaque,
                        ..PbrMaterial::default()
                    },
                );
                // a layer material is made from the terrain material and the layer's texture
                if let Some(p) = primase_core::provenance::derive(w, &[m, t], &pass_text(self)) {
                    w.insert(e, p);
                }
                layers.push(Some(Layer {
                    material: e,
                    texcoord: None,
                    transform: None,
                }));
                log.count("layer materials", 1);
            }
            w.insert(
                m,
                LayeredMaterial {
                    layers,
                    weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
                },
            );
            log.count("terrain materials layered", 1);
        }
        // every terrain primitive's weights in COLOR_1
        for_each_mesh(w, |mesh| {
            for mut p in mesh.primitives_mut() {
                let Some((mode, lookup)) = p.material.and_then(|m| weights.get(&m)) else {
                    continue;
                };
                let n = p.vertex_count();
                let colour1 = match p.streams.get(&Semantic::Color(1)) {
                    Some(Stream::U8x4(v)) => Some(v.clone()),
                    Some(_) => return Err("COLOR_1 not a u8 x 4 stream".into()),
                    None => None,
                };
                let alpha0: Vec<u8> = match p.streams.get(&Semantic::Color(0)) {
                    Some(Stream::U8x4(v)) => v.iter().map(|c| c[3]).collect(),
                    _ => vec![255; n],
                };
                let uv1 = match (lookup, p.streams.get(&Semantic::TexCoord(1))) {
                    (Some(_), Some(Stream::F32x2(v))) => Some(v.clone()),
                    (Some(_), _) => {
                        log.count(
                            "lookup-blended terrain primitives without TEXCOORD1: vertex colour weights",
                            1,
                        );
                        None
                    }
                    _ => None,
                };
                let looked: Option<Vec<[u8; 2]>> = lookup
                    .as_ref()
                    .zip(uv1)
                    .map(|(l, uv)| uv.iter().map(|&t| l.sample(t)).collect());
                if looked.is_some() {
                    log.approximate("lookup-blended terrain primitives weighed per vertex", 1);
                }
                if colour1.is_none() {
                    log.count("terrain primitives without vertex colour 1", 1);
                }
                let out = (0..n)
                    .map(|i| {
                        channels(
                            *mode,
                            colour1.as_ref().map(|c| c[i]),
                            looked.as_ref().map(|l| l[i]),
                            alpha0[i],
                        )
                    })
                    .collect();
                p.streams_mut().insert(Semantic::Color(1), Stream::U8x4(out));
                log.count("terrain primitives weighed by COLOR_1", 1);
            }
            Ok(None)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_follow_the_effect() {
        assert_eq!(weights_of("terrain_cb_w_4lyr_pxm"), Weights::VertexColour);
        assert_eq!(weights_of("terrain_cb_w_4lyr_cm_tnt"), Weights::Lookup);
        assert_eq!(
            weights_of("terrain_cb_w_4lyr_2tex_blend_pxm"),
            Weights::LookupBlend
        );
        assert_eq!(
            weights_of("terrain_cb_w_4lyr_2tex_blend_lod"),
            Weights::VertexColour
        );
    }

    #[test]
    fn channels_mix_the_lookup_by_vertex_alpha() {
        let c1 = Some([10, 20, 30, 40]);
        assert_eq!(
            channels(Weights::VertexColour, c1, Some([200, 100]), 0),
            [10, 20, 30, 40]
        );
        assert_eq!(
            channels(Weights::Lookup, c1, Some([200, 100]), 255),
            [10, 200, 100, 40]
        );
        assert_eq!(
            channels(Weights::LookupBlend, c1, Some([200, 100]), 255),
            [10, 20, 30, 40]
        );
        assert_eq!(
            channels(Weights::LookupBlend, c1, Some([200, 100]), 0),
            [10, 200, 100, 40]
        );
        assert_eq!(channels(Weights::LookupBlend, None, None, 0), [0, 0, 0, 255]);
    }
}
