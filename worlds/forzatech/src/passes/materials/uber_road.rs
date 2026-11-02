//! # `uber_road`
//!
//! Evidence, over the 50 GBuffer permutations of the family in the Tokyo track's `bin.zip` (render target 0 holds the
//! albedo, 1 the roughness in W, 2 the normal; the permutations `blend1` to `blend4` are one to four layers, `M`
//! and `modnorm` the modulate maps, `TB` the terrain blend, `winter` and `S` the frost and snow).
//! - a layer `L` (`A` to `D`) is a diffuse map `UberRoadLDiffuseTexture` (colour in RGB, the roughness in A) and a
//!   normal map `UberRoadLNormalTexture` (`x = 2 r - 1`, `z = 2 g - 1`, the occlusion in B, a height in A), both
//!   sampled at one coordinate pair: the mesh's `TEXCOORD0` or `TEXCOORD1` times `UberRoadLUVTiling` when
//!   `UberRoadLUVSet` is 0 or 1, and when it is -1 the surface point's `(x, z)` in the game's ground plane times
//!   `UberRoadLWorldUVScale`;
//! - the layers combine from the last: `rgb = lerp(D, lerp(C, lerp(B, A, s1), s2), s3)`, the roughness, the occlusion
//!   and each normal's decoded direction (then normalized) the same way, with `s1 = smoothstep(saturate((s + (1 - hB) -
//!   w0 (1 + s)) / s))`, `s2` the same with `hC` and `w1`, `s3` with `hD` and `w2`; `s` is `BlendSmoothness`, `hL` the
//!   layer's normal map alpha, and `wk` the channel `BlendShuffle[k]` of `BlendTexture` (`TEXCOORD2`): the blend
//!   texture's value raises a layer over the ones before it where it passes the layer's height;
//! - the albedo is then blended with `TintTexture` (`TEXCOORD2`) as an overlay (`2 t d` below one half, `1 - 2 (1 - t)
//!   (1 - d)` above), taken to gamma 1/2.2, mixed with frost and snow by `UberRoadLFrost`, `UberRoadLSnow`,
//!   `FrostAmount`, the terrain gloss texture and scene constants, and back by 2.2, times the scene's occlusion factor (a
//!   scene constant: no material or settings file holds it, so the occlusion slots carry the raw blue, named);
//! - the metalness (target 0 W) is not written: 0.
//!
//! The rule: one layer is a `pbr.material` whose slots take the layer's coordinates; two to four are a
//! `core.layered` of one such material per layer, composed `Over` in layer order (slot 0 is `A`, and slot `k`'s weight
//! `1 - s_k`), each weight `saturate((w (1 + s) + h - 1) / s)` of the blend texture's channel and the layer's height
//! (the same ramp as the shader's, without its smoothstep: named). The layer's coordinates, `TexCoord(0)` or `(1)`
//! and the tiling, or `TexCoord(3)` filled with the planar position ([`PlanarUv`]) and the world scale, are on the
//! layer, whose textures take them. The roughness is the diffuse map's alpha (`AlphaRoughness`), the occlusion
//! the normal map's blue, no metal. Left out by name: the tint overlay, the modulate maps (`Modulate`,
//! `ModulateNormal`, `ModulateUV`), the terrain blend, frost and snow (run-time state, `Winter`), the deformable
//! tracks, parallax, noise and the custom surfaces. The weights are per pixel; a consumer that bakes them to the
//! vertices says so.
use super::{Binding, Derived, Lowered, Lowering, NormalMap, PLANAR_TEXCOORD, Params, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures, PlanarUv};
use primase_core::layered::{Channel, Composition, Source};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Entity, Layer, LayerWeights, LayeredMaterial, Name, Semantic, TexRole, Texture,
    WeightMapping, World,
};

/// The layers' letters, in blend order.
const LAYERS: [&str; 4] = ["A", "B", "C", "D"];

/// The texture coordinate set of the road meshes' blend and tint textures.
const BLEND_TEXCOORD: u8 = 2;

/// The `uber_road` rule's simplifications, by the switch that asks for what is left out.
const LEFT_OUT: &[(&str, &str)] = &[
    ("Modulate", "modulate map left out"),
    ("ModulateNormal", "modulate normal left out"),
    ("TerrainBlend", "terrain blend left out"),
    ("Winter", "frost and snow (run-time) left out: dry"),
    ("Snow", "snow left out"),
    ("DeformableTracks", "deformable tracks left out"),
    ("Parallax", "parallax left out"),
    ("Noise", "noise left out"),
    ("Custom1", "first custom surface left out"),
    ("Custom2", "second custom surface left out"),
];

/// How a layer maps its textures.
struct Mapping {
    texcoord: u8,
    transform: Option<UvTransform>,
    planar: bool,
}

impl Lowering {
    /// The `uber_road` family's material.
    pub(super) fn uber_road(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Lowered, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let count = if p.on("Blend4") {
            4
        } else if p.on("Blend3") {
            3
        } else if p.on("Blend2") {
            2
        } else {
            1
        };
        let name = w.get::<Name>(e).map(|n| n.0.clone());

        // the layers: a material, its mapping and its height, each None when the layer's maps are not there
        let mut layers = Vec::with_capacity(count);
        let mut planar = false;
        for letter in LAYERS.iter().take(count) {
            let made = self.road_layer(w, &p, letter, name.as_deref(), log)?;
            match &made {
                Some((_, mapping, _)) => planar |= mapping.planar,
                None => approximate(
                    log,
                    e,
                    &format!("layer {letter} without its diffuse or normal map: black"),
                    triangles,
                ),
            }
            layers.push(made);
        }
        let blend = match p.texture("BlendTexture") {
            Binding::Texture(t) => Some(t),
            _ => None,
        };
        if matches!(p.texture("TintTexture"), Binding::Texture(_)) {
            approximate(log, e, "tint overlay left out", triangles);
        }
        // the albedo times the scene's occlusion factor: carried by the occlusion slots, the raw map's blue
        if layers.iter().any(Option::is_some) {
            approximate(log, e, "occlusion strength of the scene left out", triangles);
        }
        for &(switch, what) in LEFT_OUT {
            left_out(log, &p, e, switch, what, triangles);
        }
        if planar {
            w.insert(
                e,
                PlanarUv {
                    texcoord: PLANAR_TEXCOORD,
                },
            );
        }

        // one layer is a material of its own, its slots taking the layer's coordinates
        let alone = |w: &World, layer: Option<&(Entity, Mapping, Source)>| {
            let Some((material, mapping, _)) = layer else {
                return PbrMaterial::default();
            };
            let mut pbr = w.get::<PbrMaterial>(*material).cloned().unwrap_or_default();
            for slot in [
                &mut pbr.base_color_texture,
                &mut pbr.metallic_roughness_texture,
                &mut pbr.occlusion_texture,
                &mut pbr.normal_texture,
            ]
            .into_iter()
            .flatten()
            {
                slot.texcoord = mapping.texcoord;
                slot.transform = mapping.transform;
            }
            pbr
        };
        let blend = match blend {
            Some(blend) if count > 1 => blend,
            other => {
                if other.is_none() && count > 1 {
                    approximate(log, e, "no blend map: the first layer alone", triangles);
                }
                return Ok(Lowered::Plain(Box::new(alone(
                    w,
                    layers.first().and_then(Option::as_ref),
                ))));
            }
        };
        let smoothness = p.float("BlendSmoothness").unwrap_or(0.0).max(0.001);
        let shuffle = p.swizzle("BlendShuffle").unwrap_or([0.0, 1.0, 2.0, 3.0]);
        let mut slots = Vec::with_capacity(count);
        let mut channels = Vec::with_capacity(count);
        for (i, layer) in layers.iter().enumerate() {
            let Some((material, mapping, height)) = layer else {
                slots.push(None);
                channels.push(Channel::Source(Source::Mask {
                    texture: TextureRef::new(blend),
                    channels: [0.0; 4],
                    bias: 0.0,
                }));
                continue;
            };
            slots.push(Some(Layer {
                material: *material,
                texcoord: Some(mapping.texcoord),
                transform: mapping.transform,
            }));
            channels.push(if i == 0 {
                // the first layer is the base: the blend's weight is 1 there
                Channel::Source(Source::Mask {
                    texture: TextureRef::new(blend),
                    channels: [0.0; 4],
                    bias: 1.0,
                })
            } else {
                // weight = saturate((w (1 + s) + h - 1) / s), w the blend map's channel BlendShuffle[i - 1]
                let at = shuffle[i - 1].clamp(0.0, 3.0) as usize;
                let mut pick = [0.0; 4];
                pick[at] = 1.0;
                let s = smoothness;
                Channel::Threshold {
                    v: Source::Mask {
                        texture: TextureRef {
                            texcoord: BLEND_TEXCOORD,
                            ..TextureRef::new(blend)
                        },
                        channels: pick,
                        bias: 0.0,
                    },
                    m: height.clone(),
                    k: [-1.0 / s, (1.0 + s) / s, 1.0 / s, 0.0],
                }
            });
        }
        let mut weights = LayerWeights::new(Semantic::TexCoord(BLEND_TEXCOORD), WeightMapping::Identity);
        weights.channels = Some(channels);
        weights.composition = Composition::Over;
        w.insert(
            e,
            LayeredMaterial {
                layers: slots,
                weights,
            },
        );
        approximate(
            log,
            e,
            "layer blend's smoothstep taken as a linear ramp",
            triangles,
        );
        log.count("uber_road layered materials made", 1);
        Ok(Lowered::Blend)
    }

    /// One road layer: its material, how it maps its textures, and its height as a mask.
    fn road_layer(
        &mut self,
        w: &mut World,
        p: &Params<'_>,
        letter: &str,
        name: Option<&str>,
        log: &mut PassLog,
    ) -> Result<Option<(Entity, Mapping, Source)>, String> {
        let (Binding::Texture(diffuse), Binding::Texture(normal)) = (
            p.texture(&format!("UberRoad{letter}DiffuseTexture")),
            p.texture(&format!("UberRoad{letter}NormalTexture")),
        ) else {
            return Ok(None);
        };
        let set = p.int(&format!("UberRoad{letter}UVSet")).unwrap_or(0);
        let mapping = if set < 0 {
            Mapping {
                texcoord: PLANAR_TEXCOORD,
                transform: p
                    .vec2(&format!("UberRoad{letter}WorldUVScale"))
                    .map(|scale| UvTransform {
                        offset: [0.0; 2],
                        scale,
                        rotation: 0.0,
                    }),
                planar: true,
            }
        } else {
            Mapping {
                texcoord: u8::try_from(set.min(1)).unwrap_or(0),
                transform: p
                    .vec2(&format!("UberRoad{letter}UVTiling"))
                    .map(|scale| UvTransform {
                        offset: [0.0; 2],
                        scale,
                        rotation: 0.0,
                    }),
                planar: false,
            }
        };
        let mut pbr = PbrMaterial {
            base_color_texture: Some(TextureRef::new(diffuse)),
            metallic: 0.0,
            roughness: 1.0,
            ..PbrMaterial::default()
        };
        if let Some(tex) = w.get_mut::<Texture>(diffuse)
            && tex.role == TexRole::Other
            && tex.color_space == ColorSpace::Srgb
        {
            tex.role = TexRole::Color;
        }
        if let Some(rough) = self.derive(w, diffuse, Derived::AlphaRoughness(1.0f32.to_bits()), log)? {
            pbr.metallic_roughness_texture = Some(TextureRef::new(rough));
        }
        if let Some(occlusion) = self.derive(w, normal, Derived::BlueOcclusion, log)? {
            pbr.occlusion_texture = Some(TextureRef::new(occlusion));
        }
        // a flat default map takes no slot; one with no pixels is counted by `normal_map`
        if let NormalMap::Map(map) = self.normal_map(w, normal, log)? {
            pbr.normal_texture = Some(TextureRef::new(map));
        }
        let height = match self.derive(w, normal, Derived::AlphaHeight, log)? {
            Some(h) => Source::Mask {
                texture: TextureRef {
                    texcoord: mapping.texcoord,
                    transform: mapping.transform,
                    ..TextureRef::new(h)
                },
                channels: [1.0, 0.0, 0.0, 0.0],
                bias: 0.0,
            },
            None => return Ok(None),
        };
        let material = w.spawn();
        if let Some(name) = name {
            w.insert(material, Name(format!("{name} layer {letter}")));
        }
        w.insert(material, pbr);
        Ok(Some((material, mapping, height)))
    }
}
