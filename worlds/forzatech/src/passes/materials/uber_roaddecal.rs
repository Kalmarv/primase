//! # `uber_roaddecal` and `uber_roaddecalretro`
//!
//! Evidence, over the 30 GBuffer permutations of `uber_roaddecal` (`decal`, `winter`, `metal`, `T`, `AM`, `Modulate`,
//! `POM` in their combinations) and the 5 of `uber_roaddecalretro` (`decal`, `HA`, `M`, `T`, `T_M`) in the Tokyo
//! track's `bin.zip`, each render target channel followed back to its texture reads and parameter rows through the
//! family's `CBMP` and `TXMP` maps (the texture index of row 0 is folded by the compiler into `(X >> 2) & 0x3fff`):
//! - the albedo (target 0 XYZ), dry, is `DecalDiffuseATexture.rgb` (an sRGB view, so linear) read at
//!   `uv0 * RoadUVMultiplierA`; with `PerObjectTint0OnOff` it is `overlay(D, ObjectTintColor^(1/2.2))` per channel,
//!   `overlay(a, b) = a > 0.5 ? 1 - 2 (1 - a)(1 - b) : 2 a b`; toward `FadeColor` it goes by a smoothstep of the
//!   screen-space derivative of `uv0.x` between `FadeStart` and `FadeEnd`. The `T` permutations overlay it with the
//!   vertex colour's blue, the modulate ones (`Modulate`, `M`) scale it by `1 + (ModulateTexture.a - 1) k`,
//!   `k = sat(DecalNormalA.a + 1 - HeightInfluence) COLOR.r`, with `ModulateTexture` read at `ModulateUVTiling *`
//!   one of the texture coordinate sets (`ModulateUV`); `Snow` and `Frost` change nothing while `SnowAmount` and
//!   `FrostAmount` are 0, as in every material of the track's box;
//! - the roughness (target 1 W), dry, is `DecalDiffuseATexture.a` (modulate: times `1 + (M.b - 1) k`); the diffuse
//!   alpha is not an opacity. The metalness (target 0 W) is `Mask_MetalnessATexture.r` in the 12 `metal`
//!   permutations (read through its sRGB view) and 0 in every other one, and in the retro permutations;
//! - the normal (target 2) is the standard decode of `DecalNormalA.rg` at `uv0 * RoadUVMultiplierA` brought to the world
//!   as `x T - y B + z N`, `B = (N x T) w`, the environment's frame (green up, Z rebuilt);
//! - the only discard of the `AM` permutations and of every retro one tests `DecalMaskATexture.r` (read at the raw
//!   `uv0`) times the vertex colour's green where `AlphaFadeOnOff`, or `WorldSpaceAlphaTex` at the world position
//!   where `WorldSpaceAlphaOnOff`; `HA` (the retro `HeightAlpha`) reads `sat((mask + fade - 1) / MaxHeightAlpha)`
//!   instead, and the retro permutations also multiply by a smoothstep of the camera distance (scene constants).
//!   The test is dithered: with `s` that value the pixel is kept where `s sat(5 (s - 0.2)) - 1 + n >= 0`, `n` the
//!   interleaved gradient noise of the pixel (`frac(52.98 frac(0.0671 x + 0.00584 y))`), or, with the scene's dither
//!   off, where `s` reaches the draw's reference `cb2[128].x`. A permutation without `AM` has no mask test (its two
//!   discards are `1 - cb2[128].x < 0`, all or none of a draw): it is opaque. The retro permutations' target 1 XYZ also carries a retro-reflective term (`RetroReflectivity`,
//!   `RetroReflectivityFalloff`, `Hardness`);
//! - wetness (`WetScale`, `Porosity`, `HeightPuddlingOnOff`, the scene's rain) is run-time state: the maps above are
//!   the dry surface.
//!
//! The rule: a decal is a `pbr.material` in the decal layer (`core.draw_order`, queue 2500, the convention). The base
//! colour is a map whose RGB is `DecalDiffuseATexture` (overlaid with the constant `ObjectTintColor` where
//! `PerObjectTint0OnOff`, which is baked once per texture and tint) and whose alpha is the mask `DecalMaskATexture.r`
//! (`sat(mask / MaxHeightAlpha)` for `HA`), alpha-tested at one half: `s sat(5 (s - 0.2))` is `s` from 0.4 up, so one
//! half is the median of the dither. A permutation without `AM` is opaque and keeps the diffuse map as it is. The
//! roughness is the diffuse alpha, packed into G of a metallic-roughness map with `Mask_MetalnessATexture.r` in B where
//! `Metalness`; the normal map is `DecalNormalA`. Every map is read at `uv0 * RoadUVMultiplierA`, which is 1 in every
//! material of the box: the mask's raw `uv0` is the same read. Another value is a named approximation.
//!
//! `Enabled` (a `CBMP` entry, row 14) is read by none of the pixel shaders: a material with it off is drawn, named.
//!
//! Left out, named per material and triangle: the vertex-colour terms (`T`, modulate, `AlphaFadeOnOff`), because the
//! meshes carry no vertex colour (it is not read), the world-space alpha mask, parallax occlusion, the far fade to
//! `FadeColor`, the retro-reflective term and the retro distance fade, wetness, and the dither (taken as a plain test).
use super::{ALPHA_REFERENCE, Binding, DECAL_QUEUE, Derived, Lowering, Params, approximate, left_out, seen};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{ColorSpace, DrawLayer, DrawOrder, Entity, Name, TexFormat, TexRole, Texture, World};
use primase_foundations::color::linear_to_srgb8;
use primase_image::{TextureMetadata, map_rgba8_levels};

/// A map of the road decals made of two source textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum DecalMap {
    /// RGB the diffuse, overlaid with the tint's bits where present; A the mask (`sat(mask / range)` for the range's
    /// bits, `mask` for none) or 255 without a mask.
    Colour {
        diffuse: Entity,
        mask: Option<Entity>,
        tint: Option<[u32; 3]>,
        range: Option<u32>,
    },
    /// G the diffuse alpha (the roughness), B the metalness map's red.
    RoughnessMetal { diffuse: Entity, metal: Entity },
}

/// The overlay of the shader: `a > 0.5 ? 1 - 2 (1 - a)(1 - b) : 2 a b`.
fn overlay(a: f64, b: f64) -> f64 {
    if a > 0.5 {
        1.0 - 2.0 * (1.0 - a) * (1.0 - b)
    } else {
        2.0 * a * b
    }
}

/// One decoded level of a map: its size and its RGBA8 texels, row by row.
pub(super) struct Level {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) texels: Vec<u8>,
}

/// The levels of `t` decoded to RGBA8 as stored (no colour transfer).
pub(super) fn decoded_levels(t: &Texture) -> Result<Vec<Level>, String> {
    let mut levels: Vec<Level> = Vec::new();
    let identity = TextureMetadata {
        color_space: t.color_space,
        role: t.role,
        normal: None,
    };
    map_rgba8_levels(t, 0..t.levels.len(), identity, |texel| {
        if levels.len() <= texel.level {
            levels.push(Level {
                width: texel.width,
                height: texel.height,
                texels: Vec::with_capacity(texel.width as usize * texel.height as usize * 4),
            });
        }
        levels[texel.level].texels.extend_from_slice(&texel.rgba);
        Ok(texel.rgba)
    })
    .map_err(|e| e.to_string())?;
    Ok(levels)
}

/// The texel at (`x`, `y`) of a map of `width` by `height`, read nearest from the stored level of `levels` that stands
/// for `level` (the last one when the map stores fewer).
pub(super) fn texel_of(levels: &[Level], level: usize, x: u32, y: u32, width: u32, height: u32) -> [u8; 4] {
    let Some(l) = levels.get(level).or(levels.last()) else {
        return [0; 4];
    };
    let sx = (u64::from(x) * u64::from(l.width) / u64::from(width.max(1))) as usize;
    let sy = (u64::from(y) * u64::from(l.height) / u64::from(height.max(1))) as usize;
    let at = (sy.min((l.height as usize).saturating_sub(1)) * l.width as usize
        + sx.min((l.width as usize).saturating_sub(1)))
        * 4;
    l.texels
        .get(at..at + 4)
        .map_or([0; 4], |p| [p[0], p[1], p[2], p[3]])
}

impl Lowering {
    /// The map `key` names, made once.
    fn decal_map(
        &mut self,
        w: &mut World,
        key: DecalMap,
        log: &mut PassLog,
    ) -> Result<Option<Entity>, String> {
        if let Some(&e) = self.decal_maps.get(&key) {
            return Ok(Some(e));
        }
        let (source, other, what) = match key {
            DecalMap::Colour { diffuse, mask, .. } => (diffuse, mask, "decal colour and mask"),
            DecalMap::RoughnessMetal { diffuse, metal } => {
                (diffuse, Some(metal), "decal roughness and metalness")
            }
        };
        let Some(t) = w.get::<Texture>(source) else {
            log.count(&format!("{what} maps left out: no pixels"), 1);
            return Ok(None);
        };
        let other_texture = other.and_then(|o| w.get::<Texture>(o));
        if other.is_some() && other_texture.is_none() {
            log.count(&format!("{what} maps left out: no pixels"), 1);
            return Ok(None);
        }
        let space = t.color_space;
        let other_space = other_texture.map_or(ColorSpace::Linear, |o| o.color_space);
        let mut other_levels = Vec::new();
        if let Some(o) = other_texture {
            other_levels = decoded_levels(o).map_err(|e| format!("decoding a {what} source: {e}"))?;
            if other_levels.is_empty() {
                log.count(&format!("{what} maps left out: no level"), 1);
                return Ok(None);
            }
        }
        let resampled = other_texture.is_some_and(|o| (o.width, o.height) != (t.width, t.height));
        let metadata = match key {
            DecalMap::Colour { .. } => TextureMetadata {
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
            DecalMap::RoughnessMetal { .. } => TextureMetadata {
                color_space: ColorSpace::Linear,
                role: TexRole::Mask,
                normal: None,
            },
        };
        let texture = map_rgba8_levels(t, 0..t.levels.len(), metadata, |texel| {
            let [r, g, b, a] = texel.rgba;
            let o = if other_levels.is_empty() {
                [0; 4]
            } else {
                texel_of(
                    &other_levels,
                    texel.level,
                    texel.x,
                    texel.y,
                    texel.width,
                    texel.height,
                )
            };
            let channel = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
            Ok(match key {
                DecalMap::Colour {
                    mask, tint, range, ..
                } => {
                    let rgb = match tint {
                        // baked once: the overlay of the linear albedo with the tint, back to sRGB
                        Some(bits) => {
                            let c = [r, g, b].map(|x| seen(space, x));
                            [0, 1, 2].map(|i| {
                                let t = f64::from(f32::from_bits(bits[i])).powf(1.0 / 2.2);
                                linear_to_srgb8(overlay(c[i], t))
                            })
                        }
                        // the albedo is already the sRGB bytes of itself; a linear source is encoded
                        None if space == ColorSpace::Srgb => [r, g, b],
                        None => [r, g, b].map(|x| linear_to_srgb8(f64::from(x) / 255.0)),
                    };
                    let alpha = if mask.is_some() {
                        let m = seen(other_space, o[0]);
                        channel(range.map_or(m, |bits| m / f64::from(f32::from_bits(bits))))
                    } else {
                        255
                    };
                    [rgb[0], rgb[1], rgb[2], alpha]
                }
                // the roughness is the diffuse alpha (never sRGB-decoded), the metalness the second map's red
                DecalMap::RoughnessMetal { .. } => [255, a, channel(seen(other_space, o[0])), 255],
            })
        })
        .map_err(|e| format!("making a {what} map: {e}"))?;
        let name = w.get::<Name>(source).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} ({what})")));
        w.insert(e, texture);
        log.count(&format!("{what} textures made of two sources"), 1);
        if resampled {
            log.count(
                &format!("{what} textures whose second source was resampled to the first's size"),
                1,
            );
        }
        self.decal_maps.insert(key, e);
        Ok(Some(e))
    }

    /// The `uber_roaddecal` family's material.
    pub(super) fn uber_roaddecal(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        self.road_decal(w, e, m, log, false)
    }

    /// The `uber_roaddecalretro` family's material: the same decal, always alpha tested, retro-reflective.
    pub(super) fn uber_roaddecalretro(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        self.road_decal(w, e, m, log, true)
    }

    fn road_decal(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
        retro: bool,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let tiling = p.vec2("RoadUVMultiplierA");
        if tiling.is_some_and(|t| t != [1.0, 1.0]) {
            approximate(
                log,
                e,
                "mask read at the raw uv0, baked with the maps' RoadUVMultiplierA tiling",
                triangles,
            );
        }
        let mut pbr = PbrMaterial::default();
        // every permutation of the retro family tests the mask; the other family's `AM` ones do
        let tested = retro || p.on("AlphaMask");
        let height_alpha = retro && p.on("HeightAlpha");
        let tint = p
            .on("PerObjectTint0OnOff")
            .then(|| p.vec4("ObjectTintColor"))
            .flatten()
            .map(|c| [c[0].to_bits(), c[1].to_bits(), c[2].to_bits()])
            .filter(|bits| bits.map(f32::from_bits) != [1.0, 1.0, 1.0]);

        match p.texture("DecalDiffuseATexture") {
            Binding::Texture(d) => {
                let mask = match tested.then(|| p.texture("DecalMaskATexture")) {
                    Some(Binding::Texture(t)) => Some(t),
                    _ => None,
                };
                if tested && mask.is_none() {
                    approximate(log, e, "no mask map: the decal is opaque", triangles);
                }
                let range = height_alpha.then(|| p.float("MaxHeightAlpha").unwrap_or(1.0).to_bits());
                let colour = if tint.is_none() && mask.is_none() {
                    Some(d)
                } else {
                    self.decal_map(
                        w,
                        DecalMap::Colour {
                            diffuse: d,
                            mask,
                            tint,
                            range,
                        },
                        log,
                    )?
                };
                match colour {
                    Some(colour) => {
                        pbr.base_color_texture = Some(Self::slot(colour, tiling, [0.0; 2]));
                        if let Some(tex) = w.get_mut::<Texture>(colour)
                            && tex.role == TexRole::Other
                            && tex.color_space == ColorSpace::Srgb
                        {
                            tex.role = TexRole::Color;
                        }
                        if mask.is_some() {
                            pbr.alpha_mode = AlphaMode::Mask {
                                cutoff: ALPHA_REFERENCE,
                            };
                            approximate(
                                log,
                                e,
                                "dithered alpha test taken as a plain test at one half",
                                triangles,
                            );
                        }
                    }
                    None => approximate(log, e, "base colour map not lowered: white", triangles),
                }
                // roughness is DecalDiffuseATexture.a; the metal permutations add Mask_MetalnessATexture.r
                pbr.metallic = 0.0;
                pbr.roughness = 1.0;
                let metal_wanted = !retro && p.on("Metalness");
                let metal = match metal_wanted.then(|| p.texture("Mask_MetalnessATexture")) {
                    Some(Binding::Texture(t)) => Some(t),
                    _ => None,
                };
                if metal_wanted && metal.is_none() {
                    approximate(log, e, "no metalness map: dielectric", triangles);
                }
                let packed = match metal {
                    Some(metal_map) => {
                        pbr.metallic = 1.0;
                        self.decal_map(
                            w,
                            DecalMap::RoughnessMetal {
                                diffuse: d,
                                metal: metal_map,
                            },
                            log,
                        )?
                    }
                    None => self.derive(w, d, Derived::AlphaRoughness(1.0f32.to_bits()), log)?,
                };
                match packed {
                    Some(rm) => pbr.metallic_roughness_texture = Some(Self::slot(rm, tiling, [0.0; 2])),
                    None => approximate(log, e, "roughness map not lowered: rough", triangles),
                }
            }
            Binding::Absent => approximate(
                log,
                e,
                "base colour texture in no archive: white, rough",
                triangles,
            ),
            Binding::None => approximate(log, e, "no base colour texture: white, rough", triangles),
        }

        match p.texture("DecalNormalA") {
            Binding::Texture(t) => {
                // a one-channel map has no green to read
                let one_channel = w
                    .get::<Texture>(t)
                    .is_some_and(|x| matches!(x.format, TexFormat::Bc4Unorm | TexFormat::Bc4Snorm));
                if one_channel {
                    approximate(log, e, "normal map has one channel: not lowered", triangles);
                } else if let Some(slot) =
                    self.normal_slot(w, e, t, triangles, log, |n| Self::slot(n, tiling, [0.0; 2]))?
                {
                    pbr.normal_texture = Some(slot);
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
        }

        for (switch, what) in [
            ("Tint", "tint by the vertex colour's blue left out"),
            ("Modulate", "modulate (vertex colour's red) left out"),
            ("AlphaFadeOnOff", "vertex colour fade (green) left out"),
            ("WorldSpaceAlphaOnOff", "world-space alpha mask left out"),
            ("ParallaxOcclusionMapping", "parallax occlusion mapping left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        if p.off("Enabled") {
            approximate(
                log,
                e,
                "decal switched off (Enabled) drawn: no pixel shader reads it",
                triangles,
            );
        }
        approximate(
            log,
            e,
            "far fade to FadeColor (uv footprint) and wetness (run-time) left out: dry, unfaded",
            triangles,
        );
        if retro {
            approximate(
                log,
                e,
                "retro-reflection and the distance fade of the alpha (scene constants) left out",
                triangles,
            );
        }
        w.insert(e, DrawOrder::conventional(DrawLayer::Decal, DECAL_QUEUE));
        Ok(Some(pbr))
    }
}
