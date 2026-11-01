//! # `environment`
//!
//! Evidence, over all 379 GBuffer permutations of the family in the Tokyo track's `bin.zip` (render target 0 holds the
//! albedo and, in W, the metalness; 1 the roughness in W and the lit colour in XYZ; 2 the normal):
//! - the metalness (target 0 W) is `RoughMetAoPorTex_A.g` in the 273 permutations with one texture set (199 alone, 74
//!   with the mega-skirt blend's textures, the `_MS` ones), and that texture and `RoughMetAoPorTex_B.g` in the 106 that
//!   blend two (`Blend2`, off in every material of the track's box);
//! - the roughness (target 1 W) with wetness off is `saturate(RoughMetAoPorTex_A.r)`; wetness (`WetnessMask`) lerps it
//!   with a wet model from run-time constants;
//! - occlusion is `RoughMetAoPorTex_A.b`, raised to a scene power and lerped by a scene strength into the albedo
//!   (target 0 XYZ);
//! - the albedo (target 0 XYZ) of the permutation without swatch tint, modulate, deposits or triplanar is
//!   `DiffAlphaTex_A.rgb` (read through an sRGB view, so linear) taken to gamma 1/2.2, darkened by wetness, taken
//!   back by 2.2: itself when dry, times the occlusion factor above. 378 of the 379 permutations read
//!   `DiffAlphaTex_A.r` into target 0;
//! - the normal (target 2) is `x = 2 r - 1, y = 2 g - 1, z = sqrt(saturate(1 - x^2 - y^2))` of `NormHeightTex_A`,
//!   brought to the world as `x T - y B + z N` with `B = (N x T) w` of the vertex tangent (`TANGENT.w` is decoded to
//!   its sign). The game's `B` points along +v, down the image (the engine wiki's check over 73,176 triangles of its
//!   meshes), the opposite of glTF's, so the `-y B` is glTF's `+y B`: green up, X red, Z rebuilt (`NormalConvention`
//!   says so, and no writer converts it). 378 of the 379 permutations read `NormHeightTex_A.r` and `.g` into target 2;
//! - the three maps are sampled at `tiling * (uv0 + Uv0Offset)` with each map's own tiling parameter
//!   (`BaseColorAlpha_A_UVTiling`, `NormalHeight_A_UVTiling`, `RoughMetAoPoros_A_UVTiling`), so each slot's transform
//!   is `scale = tiling, offset = tiling * Uv0Offset` on the first texture coordinate set (the shader's `TEXCOORD0`).
//!   A vertical scroll (`AnimTranslate`) is added to the V of the normal, albedo and RoughMetAo samples when a
//!   parameter is set; it is off in every material of the box.
//!
//! The `RoughMetAoPorTex_A` texture (R rough, G metal, B occlusion, A porosity) is re-encoded as glTF's packed map,
//! R occlusion, G roughness, B metallic, and bound to both the metallic-roughness and the occlusion slot, so each
//! sampler reads it once; a source stored sRGB (BC1 `_SRGB` in the track) is decoded first, as the shader's view does.
//!
//! The simplifications: the surface the permutation makes dry and unlit (wetness, the lightmap and light probes are
//! run-time state); the albedo's occlusion product is carried by the occlusion slot instead; and, named per material,
//! swatch tint, modulate, deposits, triplanar, relaxed cone-step mapping, mega-skirt, emission, retro-reflection and
//! per-instance random UV offset are left out, because each needs a vertex colour, the instance variation, the world
//! position or a run-time constant that no material holds.
use super::{ALPHA_REFERENCE, Binding, Derived, Lowering, Params, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

impl Lowering {
    /// The `environment` family's material.
    pub(super) fn environment(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let offset = p.vec2("Uv0Offset").unwrap_or([0.0; 2]);
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffAlphaTex_A") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(Self::slot(t, p.vec2("BaseColorAlpha_A_UVTiling"), offset));
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

        match p.texture("NormHeightTex_A") {
            Binding::Texture(t) => {
                if Self::normal_map(w, t, log) {
                    pbr.normal_texture = Some(Self::slot(t, p.vec2("NormalHeight_A_UVTiling"), offset));
                } else {
                    approximate(log, e, "normal map not lowered", triangles);
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
        }

        match p.texture("RoughMetAoPorTex_A") {
            Binding::Texture(t) => match self.derive(w, t, Derived::OcclusionRoughnessMetallic, log)? {
                Some(packed) => {
                    let slot = Self::slot(packed, p.vec2("RoughMetAoPoros_A_UVTiling"), offset);
                    pbr.metallic_roughness_texture = Some(slot);
                    pbr.occlusion_texture = Some(slot);
                    pbr.metallic = 1.0;
                    pbr.roughness = 1.0;
                }
                None => approximate(
                    log,
                    e,
                    "RoughMetAoPor map not lowered: dielectric, rough",
                    triangles,
                ),
            },
            Binding::Absent | Binding::None => {
                approximate(log, e, "no RoughMetAoPor map: dielectric, rough", triangles);
            }
        }
        if pbr.metallic_roughness_texture.is_none() {
            pbr.metallic = 0.0;
            pbr.roughness = 1.0;
        }

        if p.on("AlphaTest") {
            // the reference is a per-draw constant of the engine, not a parameter of the material
            pbr.alpha_mode = AlphaMode::Mask {
                cutoff: ALPHA_REFERENCE,
            };
            approximate(log, e, "alpha-test reference taken as one half", triangles);
        } else if p.on("AlphaBlend") {
            pbr.alpha_mode = AlphaMode::Blend;
        }

        for (switch, what) in [
            ("SwatchTint", "swatch tint left out"),
            ("Modulate", "modulate texture left out"),
            ("Deposits", "deposits left out"),
            ("TriPlanarMapping", "triplanar mapping left out"),
            ("RCSM", "relaxed cone-step mapping left out"),
            ("MegaSkirt", "mega-skirt terrain blend left out"),
            ("Emissive", "emission left out"),
            ("RetroReflective", "retro-reflection left out"),
            ("RandomOffsetOnOff", "per-instance random UV offset left out"),
            ("DualUv", "second UV set of a dual-UV material left out"),
            ("WetnessMask", "wetness (run-time) left out: dry"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(pbr)
    }
}
