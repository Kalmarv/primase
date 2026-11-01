//! # `uber_branch`
//!
//! Evidence, over the 7 GBuffer permutations: the albedo is `DiffuseA.rgb` read at `uv0 * DiffuseAUVTiling`, times a
//! factor of `NormalA.a` (the `NormalAlpha_AO` permutation's occlusion), the vertex colour's alpha and scene constants;
//! the roughness is `DiffuseA.a`, the metalness 0; the normal is the standard decode of `NormalA.rg` read at
//! `uv0 * NormalAUVTiling`, `z` rebuilt from the unscaled `x, y`, then `x, y` times `NormalAIntensity`, brought to the
//! world as the environment's is (green up, which is glTF's `normalScale`); the only discard compares a constant, not a
//! texture, so the surface is opaque. The `_MS` permutations blend the terrain in below a height (mega-skirt): left
//! out, named. A material that blends a second set (`Blend2`) is left unlowered: the track's `bin.zip` holds seven
//! GBuffer permutations of the family (`_BranchHiQ_NormalAlpha_AO` with and without `_MS`, `_Snow_MS`, `_Tint`,
//! `_Tint_MS`, `_MicroNormal_..._MS`, and `_DXRSimpleHit_Base`) and none reads `DiffuseB` or `NormalB` (their
//! constant-buffer offsets are 0), so no shader in the install draws such a material and there is no rule to derive.
//! All 6 materials of the Shibuya box (8,213 triangles) have `Blend2` off.
use super::{Binding, Derived, Lowering, Params, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::PbrMaterial;
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

impl Lowering {
    /// The `uber_branch` family's material: `None` for a permutation that blends a second set of maps.
    pub(super) fn uber_branch(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        if p.on("Blend2") {
            return Ok(None);
        }
        let mut pbr = PbrMaterial::default();
        let diffuse_tiling = p.vec2("DiffuseAUVTiling");
        let normal_tiling = p.vec2("NormalAUVTiling");

        // roughness is DiffuseA.a, so DiffuseA is the colour map and the roughness map's source at once
        pbr.metallic = 0.0;
        pbr.roughness = 1.0;
        match p.texture("DiffuseA") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(Self::slot(t, diffuse_tiling, [0.0; 2]));
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
                if let Some(rough) = self.derive(w, t, Derived::AlphaRoughness(1.0f32.to_bits()), log)? {
                    pbr.metallic_roughness_texture = Some(Self::slot(rough, diffuse_tiling, [0.0; 2]));
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

        match p.texture("NormalA") {
            Binding::Texture(t) => {
                if let Some(slot) = self.normal_slot(w, e, t, triangles, log, |n| {
                    Self::slot(n, normal_tiling, [0.0; 2])
                })? {
                    pbr.normal_texture = Some(slot);
                    // z is rebuilt from the unscaled x and y, then x and y are scaled: glTF's scale
                    pbr.normal_scale = p.float("NormalAIntensity").unwrap_or(1.0);
                }
                // NormalAlpha_AO: the map's alpha is the ambient occlusion
                if p.on("NormalAlpha_AO")
                    && let Some(occlusion) = self.derive(w, t, Derived::AlphaOcclusion, log)?
                {
                    pbr.occlusion_texture = Some(Self::slot(occlusion, normal_tiling, [0.0; 2]));
                    approximate(log, e, "occlusion strength of the scene left out", triangles);
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
        }

        for (switch, what) in [
            ("MegaSkirt", "mega-skirt terrain blend left out"),
            ("Tint", "tint left out"),
            ("Moss", "moss left out"),
            ("MicroNormal", "micro normal left out"),
            ("Animated", "wind animation left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        approximate(
            log,
            e,
            "vertex occlusion and wetness (run-time) left out",
            triangles,
        );
        Ok(Some(pbr))
    }
}
