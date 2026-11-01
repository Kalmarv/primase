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
//! swatch tint, modulate, deposits, triplanar, relaxed cone-step mapping, mega-skirt, retro-reflection and
//! per-instance random UV offset are left out, because each needs a vertex colour, the instance variation, the world
//! position or a run-time constant that no material holds.
//!
//! The emission (target 1 XYZ, `Emissive`, the `_Emissive` permutations) is `DiffAlphaTex_A.rgb * EmissiveTexture.rgb *
//! EmissiveTint.rgb * GIEmissiveScale * v * g(d)`, the emission map read at `Uv0Offset + uv0` (no tiling), `v` the
//! instance variation's factor when `LightEmisVariationOnOff` is on (see `projection_lod`), and `g(d)` the distance
//! encoding. The rule, for a material whose base colour map is not tiled: the two maps baked into one emission map at the
//! base colour map's texels, the tint times `GIEmissiveScale` the emissive factor (what is above 1 in
//! `emissive_strength`), when the emission map holds any light. Left out, named: a material whose base colour map is
//! tiled (the maps are on different coordinates), `EmissiveAltUV` (the map is on `TEXCOORD4`), the hue
//! variation (`EmissiveTempVariationOnOff`, from the world position), the flash (`EmissiveFlash`, time-driven, the
//! steady state is the emission), `g(d)` and the sky preset's scale (run-time). `v` is the placement's: the material
//! carries its ramp ([`crate::components::EmissionVariation`]) and `apply-forzatech-emission-variation` applies it.
use super::{ALPHA_REFERENCE, Binding, Derived, Lowering, Params, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures, SwatchTint};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

/// The `environment` rule's simplifications, by the switch that asks for what is left out.
const LEFT_OUT: &[(&str, &str)] = &[
    ("Modulate", "modulate texture left out"),
    ("Deposits", "deposits left out"),
    ("TriPlanarMapping", "triplanar mapping left out"),
    ("RCSM", "relaxed cone-step mapping left out"),
    ("MegaSkirt", "mega-skirt terrain blend left out"),
    ("RetroReflective", "retro-reflection left out"),
    ("RandomOffsetOnOff", "per-instance random UV offset left out"),
    ("DualUv", "second UV set of a dual-UV material left out"),
    ("WetnessMask", "wetness (run-time) left out: dry"),
];

impl Lowering {
    /// The `environment` family's material.
    pub(super) fn environment(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let mut pbr = self.environment_maps(w, e, m, log, LEFT_OUT)?;
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        if p.on("Emissive") {
            // the signage family's murals take this rule for their maps, not for their emission
            if m.shader.as_ref().is_some_and(|s| s.family == "environment") {
                self.environment_emission(w, e, &p, &mut pbr, log)?;
            } else {
                left_out(
                    log,
                    &p,
                    e,
                    "Emissive",
                    "emission left out",
                    self.triangles.get(&e).copied().unwrap_or(0),
                );
            }
        }
        if p.on("SwatchTint") {
            match p.texture("SwatchTintTexture") {
                Binding::Texture(palette) => {
                    w.insert(e, SwatchTint { palette });
                    log.count(
                        "swatch-tinted materials recorded for bake-forzatech-swatch-tint",
                        1,
                    );
                }
                Binding::Absent | Binding::None => approximate(
                    log,
                    e,
                    "swatch tint palette not found: untinted",
                    self.triangles.get(&e).copied().unwrap_or(0),
                ),
            }
        }
        Ok(pbr)
    }

    /// The emission of an `environment` material with `Emissive` on.
    fn environment_emission(
        &mut self,
        w: &mut World,
        e: Entity,
        p: &Params,
        pbr: &mut PbrMaterial,
        log: &mut PassLog,
    ) -> Result<(), String> {
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        if p.on("EmissiveAltUV") {
            approximate(
                log,
                e,
                "emission texture on TEXCOORD4 (EmissiveAltUV) left out: a material is sampled through one coordinate set",
                triangles,
            );
            return Ok(());
        }
        let tint = p.vec4("EmissiveTint").unwrap_or([1.0; 4]);
        let scale = p.float("GIEmissiveScale").unwrap_or(1.0).max(0.0);
        let radiance = [tint[0], tint[1], tint[2]].map(|c| c.max(0.0) * scale);
        let strength = radiance.iter().copied().fold(0.0f32, f32::max);
        let tiling = p.vec2("BaseColorAlpha_A_UVTiling").unwrap_or([1.0; 2]);
        let (Binding::Texture(emission), Binding::Texture(base)) =
            (p.texture("EmissiveTexture"), p.texture("DiffAlphaTex_A"))
        else {
            approximate(
                log,
                e,
                "emission left out: no emission map or no base colour map",
                triangles,
            );
            return Ok(());
        };
        if strength <= 0.0 || !self.emits_light(w, emission) {
            log.count(
                "environment emissive materials whose emission texture is black: no emission",
                1,
            );
            return Ok(());
        }
        if tiling != [1.0; 2] {
            approximate(
                log,
                e,
                "emission left out: the base colour map is tiled, so it and the emission map are on different coordinates",
                triangles,
            );
            return Ok(());
        }
        let Some(baked) = self.bake_sign(w, base, None, Some(emission), "environment", log)? else {
            approximate(log, e, "emission left out: a map has no pixels", triangles);
            return Ok(());
        };
        let offset = p.vec2("Uv0Offset").unwrap_or([0.0; 2]);
        pbr.emissive_texture = Some(Self::slot(baked, Some([1.0; 2]), offset));
        pbr.emissive = radiance.map(|c| c / strength.max(1.0));
        pbr.emissive_strength = strength.max(1.0);
        if p.on("LightEmisVariationOnOff") {
            Self::vary_emission(w, e, p, pbr);
        }
        for (switch, what) in [
            (
                "EmissiveTempVariationOnOff",
                "emission hue variation (from the world position) left out",
            ),
            ("EmissiveFlash", "emission flash left out: the steady state"),
        ] {
            left_out(log, p, e, switch, what, triangles);
        }
        approximate(
            log,
            e,
            "environment emission without the distance encoding and the sky preset's scale (run-time); the instance variation is applied per placement",
            triangles,
        );
        Ok(())
    }

    /// The three-map material the `environment` and `environment_plot` families share, whose `left_out` switches
    /// are each family's own.
    pub(super) fn environment_maps(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
        left_out_switches: &[(&str, &str)],
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
                let tiling = p.vec2("NormalHeight_A_UVTiling");
                if let Some(slot) =
                    self.normal_slot(w, e, t, triangles, log, |n| Self::slot(n, tiling, offset))?
                {
                    pbr.normal_texture = Some(slot);
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

        for &(switch, what) in left_out_switches {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(pbr)
    }
}
