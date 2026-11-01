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
//! transform the family's `BlendMapScale` and `BlendMapOffset`); the albedo as it is, the occlusion
//! `TerrainColourTexture.a` squared in its own slot, the roughness `r / 0.93` and no metal, and the normal as a
//! tangent-space map over the planar coordinates (see [`Derived::WorldNormalXz`]: red `x`, green `-z`). A glTF
//! consumer rebuilds the tangent frame from those coordinates, so the normal agrees with the shader's world-space one
//! where the terrain faces up and tilts with the surface elsewhere (named). Left out by name: the skirt, deformable
//! tracks, the custom surfaces, snow, noise and parallax permutations' switches; wetness, snow, sand lighting and the
//! twinkle are run-time state.
use super::{Binding, Derived, Lowering, PLANAR_TEXCOORD, Params, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures, PlanarUv};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

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

        match p.texture("TerrainNormalTexture") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(slot(t));
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
            }
            Binding::Absent => approximate(log, e, "base colour texture in no archive: white", triangles),
            Binding::None => approximate(log, e, "no base colour texture: white", triangles),
        }

        match p.texture("TerrainColourTexture") {
            Binding::Texture(t) => {
                match self.derive(w, t, Derived::WorldNormalXz, log)? {
                    Some(normal) => {
                        if let Some(map) = self.normal_slot(w, e, normal, triangles, log, slot)? {
                            pbr.normal_texture = Some(map);
                        }
                    }
                    None => approximate(log, e, "normal map not lowered", triangles),
                }
                match self.derive(w, t, Derived::SquaredAlphaOcclusion, log)? {
                    Some(occlusion) => pbr.occlusion_texture = Some(slot(occlusion)),
                    None => approximate(log, e, "occlusion map not lowered", triangles),
                }
            }
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
        w.insert(
            e,
            PlanarUv {
                texcoord: PLANAR_TEXCOORD,
            },
        );
        Ok(pbr)
    }
}
