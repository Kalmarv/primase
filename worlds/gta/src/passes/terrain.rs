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
//! (see [`crate::passes::lower`]) factor the intensity, colour (1 - 0.98) / 0.04, roughness [`crate::passes::lower::roughness`] of
//! the power.
//!
//! The lookup-blended effects (`*_cm*`, `*_2tex_blend*`) take green and blue per pixel from `lookupTexture` at
//! TEXCOORD1 instead, mixed with vertex colour 1's by vertex colour 0's alpha (`*_cm*`: the lookup alone); the
//! `*_lod` ones take vertex colour 1 alone. Those are the layered material's per-pixel channels, exactly
//! ([`layer_weights`]): `rgb_product_4` of r = COLOR_1's red, and g, b = the lookup's green and blue (a mask at
//! TEXCOORD1), each `lerp(lookup, COLOR_1, COLOR_0's alpha)` for `*_2tex_blend*`. GTA's shaders read the lookup
//! filtered; the reader keeps no sampler state, so the mask is the lookup as every GTA texture is referenced
//! (`TextureRef::new`: repeat, its filters unstated, untransformed). A consumer that blends by vertex weights only has
//! a program run `bake-layer-channels` (primase-image) after this pass. A lookup-blended effect whose lookup texture
//! was not found is weighed by vertex colour 1 alone, counted. A terrain primitive without vertex colour 1 gets one of (0, 0, 0, 255)
//! (layer 0 alone, as the shader's saturate of a missing input gives). Tinted terrain (`*_tnt`) is layered without its
//! tint, counted.
use std::collections::BTreeMap;

use primase_core::layered::{Channel, Source};
use primase_core::pass::{Pass, PassLog, PassSpec, pass_text};
use primase_core::pbr::{AlphaMode, PbrMaterial, Specular, TextureRef};
use primase_core::{
    ColorSpace, Entity, Layer, LayerWeights, LayeredMaterial, Semantic, Stream, TexRole, Texture,
    WeightMapping, World,
};

use crate::components::GtaShader;
use crate::passes::lower::{DEFAULT_FRESNEL, GREY, classes, for_each_mesh, roughness, scalar, texture};
use crate::passes::shader_class::Class;

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

/// The layer weights of a terrain effect weighed by `mode`, with its lookup texture `lookup` (`None`: not found):
/// `rgb_product_4` of vertex colour 1, or with a lookup per-pixel channels (r, g, b): r COLOR_1's red, g and b the
/// lookup's green and blue at TEXCOORD1 (`Weights::Lookup`), or those mixed with COLOR_1's by COLOR_0's alpha,
/// `lerp(lookup, COLOR_1, alpha)` (`Weights::LookupBlend`).
pub fn layer_weights(mode: Weights, lookup: Option<Entity>) -> LayerWeights {
    let mut weights = LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4);
    let Some(t) = lookup.filter(|_| mode != Weights::VertexColour) else {
        return weights;
    };
    let colour1 = |channel| Source::Vertex {
        semantic: Semantic::Color(1),
        channel,
    };
    let lookup = |channel: usize| {
        let mut channels = [0.0; 4];
        channels[channel] = 1.0;
        Source::Mask {
            texture: TextureRef {
                texcoord: 1,
                ..TextureRef::new(t)
            },
            channels,
            bias: 0.0,
        }
    };
    let looked = |channel: u8| match mode {
        Weights::LookupBlend => Channel::Lerp {
            a: lookup(usize::from(channel)),
            b: colour1(channel),
            t: Source::Vertex {
                semantic: Semantic::Color(0),
                channel: 3,
            },
        },
        _ => Channel::Source(lookup(usize::from(channel))),
    };
    weights.channels = Some(vec![Channel::Source(colour1(0)), looked(1), looked(2)]);
    weights
}

/// `gta-terrain-layers` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GtaTerrainLayers;

/// `gta-terrain-layers`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by the game's own rule.
pub const GTA_TERRAIN_LAYERS: PassSpec = PassSpec {
    name: "gta-terrain-layers",
    options: &[],
    make: |_| Ok(Box::new(GtaTerrainLayers)),
};

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
        "terrain tint not applied; the specular-map terrains' per-pixel height term left out; a lookup-blended \
         terrain without its lookup weighed by vertex colour 1; the Blinn-Phong lobe as GGX, dry"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let classes = classes(w)?;
        let mut weights: BTreeMap<Entity, LayerWeights> = BTreeMap::new();
        let mut layered = Vec::new();
        let mut grey = Vec::new();
        for (&m, c) in &classes {
            if c.class != Class::Terrain || c.skip.is_some() {
                continue;
            }
            let s = w.get::<GtaShader>(m).expect("shader");
            let effect = crate::passes::shader_names::effect(s.name).expect("named effect");
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
            let lookup = texture(s, "lookupTexture").flatten();
            if mode != Weights::VertexColour && lookup.is_none() {
                log.approximate(
                    "lookup-blended terrain whose lookup texture was not found: weighed by vertex colour 1",
                    1,
                );
            }
            weights.insert(m, layer_weights(mode, lookup));
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
            let weights = weights[&m].clone();
            if weights.channels.is_some() {
                log.count("terrain materials weighed per pixel by their lookup", 1);
            }
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
            w.insert(m, LayeredMaterial { layers, weights });
            log.count("terrain materials layered", 1);
        }
        // every terrain primitive with the vertex colour 1 its weights read: (0, 0, 0, 255) where it has none
        for_each_mesh(w, |mesh| {
            for mut p in mesh.primitives_mut() {
                let Some(weights) = p.material.and_then(|m| weights.get(&m)) else {
                    continue;
                };
                match p.streams.get(&Semantic::Color(1)) {
                    Some(Stream::U8x4(_)) => {}
                    Some(_) => return Err("COLOR_1 not a u8 x 4 stream".into()),
                    None => {
                        let n = p.vertex_count();
                        p.streams_mut()
                            .insert(Semantic::Color(1), Stream::U8x4(vec![[0, 0, 0, 255]; n]));
                        log.count("terrain primitives without vertex colour 1", 1);
                    }
                }
                if weights.channels.is_some() {
                    if !p.streams.contains_key(&Semantic::TexCoord(1)) {
                        log.count(
                            "lookup-blended terrain primitives without TEXCOORD1 (the lookup's)",
                            1,
                        );
                    }
                    if !p.streams.contains_key(&Semantic::Color(0)) {
                        log.count(
                            "lookup-blended terrain primitives without vertex colour 0 (the blend's alpha)",
                            1,
                        );
                    }
                }
                log.count("terrain primitives layered", 1);
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

    /// The channels as the shaders weigh them: `*_lod` and a missing lookup COLOR_1 alone; `*_cm*` COLOR_1's red with
    /// the lookup's green and blue at TEXCOORD1; `*_2tex_blend*` those mixed with COLOR_1's by COLOR_0's alpha.
    #[test]
    fn the_lookup_weighs_green_and_blue_per_pixel() {
        let mut w = World::new();
        let t = w.spawn();
        let plain = LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4);
        assert_eq!(layer_weights(Weights::VertexColour, Some(t)), plain);
        assert_eq!(layer_weights(Weights::Lookup, None), plain);
        let colour1 = |channel| Source::Vertex {
            semantic: Semantic::Color(1),
            channel,
        };
        let mask = |channels| Source::Mask {
            texture: TextureRef {
                texcoord: 1,
                ..TextureRef::new(t)
            },
            channels,
            bias: 0.0,
        };
        let (green, blue) = (mask([0.0, 1.0, 0.0, 0.0]), mask([0.0, 0.0, 1.0, 0.0]));
        let lookup = layer_weights(Weights::Lookup, Some(t));
        assert_eq!(
            (lookup.attribute, lookup.mapping),
            (plain.attribute, plain.mapping)
        );
        assert_eq!(
            lookup.channels.unwrap(),
            [
                Channel::Source(colour1(0)),
                Channel::Source(green.clone()),
                Channel::Source(blue.clone())
            ]
        );
        let alpha0 = Source::Vertex {
            semantic: Semantic::Color(0),
            channel: 3,
        };
        assert_eq!(
            layer_weights(Weights::LookupBlend, Some(t)).channels.unwrap(),
            [
                Channel::Source(colour1(0)),
                Channel::Lerp {
                    a: green,
                    b: colour1(1),
                    t: alpha0.clone()
                },
                Channel::Lerp {
                    a: blue,
                    b: colour1(2),
                    t: alpha0
                }
            ]
        );
    }
}
