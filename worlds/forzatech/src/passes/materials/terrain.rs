//! # `terr_mega` and `terr_distant`
//!
//! Evidence, over the 65 GBuffer permutations of `terr_mega` and the 2 of `terr_distant` in the Tokyo track's
//! `bin.zip` (render target 0 holds the albedo, 1 the roughness in W, 2 the normal): the two families read the same
//! three textures the same way (the first texture of a family reads from the index dword itself, which the compiler
//! folds into the shift).
//! - every texture is sampled at `BlendMapOffset + BlendMapScale * (x, z)`, `x` and `z` the surface point's
//!   position in the game's ground plane (the scene's origin shift added to the interpolated world position): no
//!   texture coordinate set of the mesh is read, except by the `Alpha` permutations of `terr_mega`;
//! - the albedo (target 0 XYZ) of the dry, snow-free permutation is `TerrainNormalTexture.rgb` (a BC7 sRGB texture,
//!   so colour; the name is the texture set's) times `TerrainColourTexture.a` squared, taken to gamma 1/2.2 and back
//!   by 2.2 with the snow and wetness mixes between, and times the scene's occlusion factor;
//! - the roughness (target 1 W) is `saturate(TerrainGlossTexture.r * 1.0752689)` (that is `r / 0.93`) where dry, which
//!   wetness and snow lerp with their models; the metalness (target 0 W) is not written: 0. The same arithmetic is in
//!   every permutation: the dependence sets of target 0 XYZ and W, target 1 W (the same textures, per channel) are
//!   those of `Default` in `Distant`, `Default_Winter`, `Distant_Winter`, `Alpha_NR` and `Noise` of `terr_mega`;
//! - the normal (target 2) is a world-space one in `TerrainColourTexture.rg`: `x = 2 r - 1`, `z = 2 g - 1` in the
//!   game's frame, `y = sqrt(saturate(1 - x^2 - z^2))`, lerped toward the vertex normal by a distance term of the
//!   scene (`scenet7[34]`), and the sand, wetness and snow terms.
//!
//! The rule: the three textures at the planar coordinates ([`PlanarUv`]: `TexCoord(3)` holds `(x, z)`, the slot's
//! transform the family's `BlendMapScale` and `BlendMapOffset`); the albedo `TerrainNormalTexture.rgb` times
//! `TerrainColourTexture.a` squared, baked into one base colour map (the squared alpha is a factor of the shader's
//! albedo, not an occlusion term of its own: it has no scene constant, so it is not carried by the occlusion slot,
//! which a consumer may draw as ambient occlusion or not at all; the bake follows the map with more texels, because the
//! road markings are in the colour map's alpha: over the Shibuya box's pair the alpha holds the zebra crossings, lane
//! lines and painted text at 2,048 texels (0.25 m) where the colour map has 1,024, and the colour is read between its
//! texels), the roughness `r / 0.93` and no metal, and the normal as a
//! tangent-space map over the planar coordinates (see [`Derived::WorldNormalXz`]: red `x`, green `-z`). A glTF
//! consumer rebuilds the tangent frame from those coordinates, so the normal agrees with the shader's world-space one
//! where the terrain faces up and tilts with the surface elsewhere (named). Left out by name: the skirt, deformable
//! tracks, the custom surfaces, snow, noise and parallax permutations' switches; wetness, snow, sand lighting and the
//! twinkle are run-time state.
//!
//! `terr_mega` draws the ground near the camera from a clipmap of rings (the per-cell splat maps and the megatexture's
//! layers) and blends the far-field colour above into its outermost ring; this lowering reads no near-field data, so the
//! far-field colour is drawn everywhere, named per `terr_mega` material and triangle (`terr_distant` has no ring).
use super::uber_roaddecal::{Level, decoded_levels, texel_of};
use super::{Binding, Derived, Lowering, PLANAR_TEXCOORD, Params, approximate, left_out, seen};
use crate::components::{ForzaMaterial, MaterialTextures, PlanarUv};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{ColorSpace, Entity, Name, TexRole, Texture, World};
use primase_foundations::color::linear_to_srgb8;
use primase_image::{TextureMetadata, map_rgba8_levels};

/// What a baked terrain albedo is: sRGB colour.
fn albedo_metadata() -> TextureMetadata {
    TextureMetadata {
        color_space: ColorSpace::Srgb,
        role: TexRole::Color,
        normal: None,
    }
}

/// The linear colour of `level` at the texture coordinates `(u, v)` (0 to 1, the texel centres at half steps): the four
/// nearest texels weighted by their distance, decoded from `space` first, the edge texels held beyond the edge.
fn bilinear_linear(level: &Level, space: ColorSpace, u: f64, v: f64) -> [f64; 3] {
    let (x, y) = (
        u * f64::from(level.width) - 0.5,
        v * f64::from(level.height) - 0.5,
    );
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f64, yi: f64| {
        let cx = (xi.max(0.0) as usize).min(level.width as usize - 1);
        let cy = (yi.max(0.0) as usize).min(level.height as usize - 1);
        let i = (cy * level.width as usize + cx) * 4;
        std::array::from_fn::<f64, 3, _>(|k| seen(space, level.texels.get(i + k).copied().unwrap_or(0)))
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1.0, y0),
        at(x0, y0 + 1.0),
        at(x0 + 1.0, y0 + 1.0),
    );
    std::array::from_fn(|k| {
        let top = a[k] * (1.0 - fx) + b[k] * fx;
        let bottom = c[k] * (1.0 - fx) + d[k] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

/// The key of a terrain albedo among the baked textures.
const TERRAIN_ALBEDO: u32 = 0x7e44_a1b0;

/// The roughness scale of the terrain shaders: `1 / 0.93`.
const ROUGHNESS_SCALE: f32 = 1.075_268_9;

/// The terrain rules' simplifications, by the switch that asks for what is left out.
const LEFT_OUT: &[(&str, &str)] = &[
    ("Alpha", "skirt alpha left out"),
    ("Custom1", "first custom surface left out"),
    ("Custom2", "second custom surface left out"),
    ("DeformableTracks", "deformable tracks left out"),
    ("DeformableVisual", "deformable visual left out"),
    ("Noise", "noise blend left out"),
    ("Parallax", "parallax left out"),
    ("Snow", "snow left out"),
];

impl Lowering {
    /// The terrain's albedo: `colour`'s RGB (sRGB) times the squared alpha of `shade`, as an sRGB map, made once per
    /// pair. `shade` is read at the nearest texel when its size differs (counted). `None` where a texture has no pixels.
    fn terrain_albedo(
        &mut self,
        w: &mut World,
        colour: Entity,
        shade: Entity,
        log: &mut PassLog,
    ) -> Result<Option<Entity>, String> {
        let key = (colour, vec![TERRAIN_ALBEDO], Some(shade));
        if let Some(&e) = self.baked.get(&key) {
            return Ok(Some(e));
        }
        let (Some(c), Some(s)) = (w.get::<Texture>(colour), w.get::<Texture>(shade)) else {
            log.count("terrain albedo maps left out: no pixels", 1);
            return Ok(None);
        };
        // the map with more texels is the one the bake follows, the other is read where it stands: the markings are in the
        // alpha, so a larger alpha map keeps them, and the colour is read between its texels
        let colour_first =
            u64::from(c.width) * u64::from(c.height) >= u64::from(s.width) * u64::from(s.height);
        let resampled = (s.width, s.height) != (c.width, c.height);
        let space = c.color_space;
        let texture = if colour_first {
            let shade_levels =
                decoded_levels(s).map_err(|e| format!("decoding a terrain colour source: {e}"))?;
            if shade_levels.is_empty() {
                log.count("terrain albedo maps left out: no level", 1);
                return Ok(None);
            }
            map_rgba8_levels(c, 0..c.levels.len(), albedo_metadata(), |texel| {
                let [r, g, b, _] = texel.rgba;
                let alpha = texel_of(
                    &shade_levels,
                    texel.level,
                    texel.x,
                    texel.y,
                    texel.width,
                    texel.height,
                );
                let a2 = (f64::from(alpha[3]) / 255.0).powi(2);
                let lit = |x: u8| linear_to_srgb8(seen(space, x) * a2);
                Ok([lit(r), lit(g), lit(b), 255])
            })
        } else {
            let colour_levels =
                decoded_levels(c).map_err(|e| format!("decoding a terrain albedo source: {e}"))?;
            if colour_levels.is_empty() {
                log.count("terrain albedo maps left out: no level", 1);
                return Ok(None);
            }
            map_rgba8_levels(s, 0..s.levels.len(), albedo_metadata(), |texel| {
                let a2 = (f64::from(texel.rgba[3]) / 255.0).powi(2);
                let level = colour_levels.get(texel.level).or(colour_levels.last());
                let rgb = level.map_or([0.0; 3], |l| {
                    bilinear_linear(
                        l,
                        space,
                        (f64::from(texel.x) + 0.5) / f64::from(texel.width),
                        (f64::from(texel.y) + 0.5) / f64::from(texel.height),
                    )
                });
                Ok([
                    linear_to_srgb8(rgb[0] * a2),
                    linear_to_srgb8(rgb[1] * a2),
                    linear_to_srgb8(rgb[2] * a2),
                    255,
                ])
            })
        }
        .map_err(|e| format!("baking a terrain albedo: {e}"))?;
        let name = w.get::<Name>(colour).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} (terrain albedo)")));
        w.insert(e, texture);
        log.count(
            "terrain albedo textures made of the colour times the squared alpha of a second map",
            1,
        );
        if resampled {
            log.count(
                if colour_first {
                    "terrain albedo textures baked at the colour map's size, the alpha map read at the nearest texel"
                } else {
                    "terrain albedo textures baked at the alpha map's size, the colour map read between its texels"
                },
                1,
            );
        }
        self.baked.insert(key, e);
        Ok(Some(e))
    }

    /// The `terr_mega` and `terr_distant` families' material.
    pub(super) fn terrain(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let scale = p.vec2("BlendMapScale");
        let offset = p.vec2("BlendMapOffset").unwrap_or([0.0; 2]);
        let slot = |texture: Entity| TextureRef {
            texcoord: PLANAR_TEXCOORD,
            transform: scale.map(|scale| UvTransform {
                offset,
                scale,
                rotation: 0.0,
            }),
            ..TextureRef::new(texture)
        };
        if scale.is_none() {
            approximate(log, e, "no map scale: coordinates unscaled", triangles);
        }
        let mut pbr = PbrMaterial::default();

        let shade = p.texture("TerrainColourTexture");
        match p.texture("TerrainNormalTexture") {
            Binding::Texture(t) => {
                // the albedo is the colour times the squared alpha of the colour map; without that map, the colour alone
                let baked = match shade {
                    Binding::Texture(s) => self.terrain_albedo(w, t, s, log)?,
                    Binding::Absent | Binding::None => None,
                };
                if let Some(baked) = baked {
                    pbr.base_color_texture = Some(slot(baked));
                } else {
                    pbr.base_color_texture = Some(slot(t));
                    if let Some(tex) = w.get_mut::<Texture>(t)
                        && tex.role == TexRole::Other
                        && tex.color_space == ColorSpace::Srgb
                    {
                        tex.role = TexRole::Color;
                    }
                    approximate(
                        log,
                        e,
                        "terrain albedo without the colour map's squared alpha factor",
                        triangles,
                    );
                }
            }
            Binding::Absent => approximate(log, e, "base colour texture in no archive: white", triangles),
            Binding::None => approximate(log, e, "no base colour texture: white", triangles),
        }

        match shade {
            Binding::Texture(t) => match self.derive(w, t, Derived::WorldNormalXz, log)? {
                Some(normal) => {
                    if let Some(map) = self.normal_slot(w, e, normal, triangles, log, slot)? {
                        pbr.normal_texture = Some(map);
                    }
                }
                None => approximate(log, e, "normal map not lowered", triangles),
            },
            Binding::Absent | Binding::None => {
                approximate(log, e, "no terrain colour map: no normal map", triangles)
            }
        }

        match p.texture("TerrainGlossTexture") {
            Binding::Texture(t) => {
                match self.derive(w, t, Derived::RedRoughness(ROUGHNESS_SCALE.to_bits()), log)? {
                    Some(rough) => {
                        pbr.metallic_roughness_texture = Some(slot(rough));
                        pbr.metallic = 0.0;
                        pbr.roughness = 1.0;
                    }
                    None => approximate(log, e, "roughness map not lowered: rough", triangles),
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no gloss map: rough", triangles),
        }
        if pbr.metallic_roughness_texture.is_none() {
            pbr.metallic = 0.0;
            pbr.roughness = 1.0;
        }

        for &(switch, what) in LEFT_OUT {
            left_out(log, &p, e, switch, what, triangles);
        }
        approximate(
            log,
            e,
            "world-space normal read as a tangent-space map over the planar coordinates",
            triangles,
        );
        if m.shader.as_ref().is_some_and(|s| s.family == "terr_mega") {
            // the shader blends the far-field colour into rings of a near-field clipmap (the per-cell splat maps and the
            // megatexture's layers) around the camera, which this lowering does not read
            approximate(
                log,
                e,
                "near-field ground (the clipmap of per-cell splat maps and megatexture layers) not read: the far-field colour everywhere",
                triangles,
            );
        }
        w.insert(
            e,
            PlanarUv {
                texcoord: PLANAR_TEXCOORD,
            },
        );
        Ok(pbr)
    }
}
