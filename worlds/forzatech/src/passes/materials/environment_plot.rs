//! # `environment_plot`
//!
//! Evidence, over all 19 GBuffer permutations of the family in the Tokyo track's `bin.zip` (render target 0 holds the
//! albedo and, in W, the metalness; 1 the roughness in W and the lit colour in XYZ; 2 the normal). The family is the
//! `environment` family's one-texture-set material on a terrain plot: its parameters are named as `environment`'s and
//! its textures are read the same way, and a plot adds the terrain's snow, slush and wetness, which are run-time state.
//! The texture index dwords of the family's material rows start 4 dwords below the end of its constants (`TEXBASE` 92
//! for the 384-byte constants), not at the end: with that base every texture read is sampled with the tiling parameter
//! of its own role (`DiffAlphaTex_A` with `BaseColorAlpha_A_UVTiling`, `NormHeightTex_A` with `NormalHeight_A_UVTiling`,
//! `RoughMetAoPorTex_A` with `RoughMetAoPoros_A_UVTiling`, `TerrainGlossTexture` with the world-space `BlendMapScale`).
//! - the metalness (target 0 W) is `RoughMetAoPorTex_A.g` in all 19 permutations;
//! - the roughness (target 1 W) is `saturate(RoughMetAoPorTex_A.r)` where the plot is snow-free and dry; snow, slush and
//!   wetness (`TerrainGlossTexture`, `Snow_WetMax`, scene constants) lerp it with their models;
//! - the albedo (target 0 XYZ) is `DiffAlphaTex_A.rgb` (read through an sRGB view) taken to gamma 1/2.2, mixed toward
//!   a snow term by a mask from the height `NormHeightTex_A.a` against `BlendHeightMapRange`, `TerrainGlossTexture`, the
//!   slope of the vertex normal and scene constants, taken back by 2.2, and times an occlusion factor: a scene strength
//!   lerps from 1 to the larger of two scene floors and a blend of `AoWetBlendContextTex.r` (the second texture
//!   coordinate set, `MacroAOWetBlendMaskTiling`) with `RoughMetAoPorTex_A.b` raised to a scene power. With no snow the
//!   albedo is `DiffAlphaTex_A.rgb` times that occlusion;
//! - the normal (target 2) is `NormHeightTex_A.rg` decoded as `environment`'s and lerped with the snow's;
//! - the alpha test (the `AlphaTest` permutations) discards `DiffAlphaTex_A.a` below the per-draw reference;
//! - the permutations with a `_M` read the `ModulateTexture` pair (second texture coordinate set, `ModulateUVTiling`)
//!   into brightness (`R`), saturation (`G`) and roughness (`B`) modulations around 0.5 scaled by `_BrightnessStrength`,
//!   `_SaturationStrength` and `_RoughnessStrength`; the `_B` ones blend a second set (`DiffAlphaTex_B`, ...) by
//!   `BlendHeightMapBRange`; the `_W` ones add `Snow_WetMax` to the winter state.
//!
//! The rule is `environment`'s on the A maps (the maps are packed and the normal decoded the same). The simplifications,
//! named per material: the dry, snow-free plot (`TerrainWetness`, on in every material of the track's box), the macro
//! occlusion texture, and the switches `Modulate`, `Blend2`, `Winter`, `RCSM`, the world-space UVs, emission and
//! the lightmap, because each needs a run-time constant, a second texture coordinate set that the lowering does not
//! carry, or a texture set this rule does not pick.
use super::{Binding, Lowering, Params, approximate};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, World};

/// The `environment_plot` rule's simplifications, by the switch that asks for what is left out.
const LEFT_OUT: &[(&str, &str)] = &[
    ("Modulate", "modulate texture left out"),
    ("Blend2", "second texture set left out"),
    ("Winter", "winter state left out"),
    ("RCSM", "relaxed cone-step mapping left out"),
    ("WorldSpaceUV_A", "world-space UVs of the first set left out"),
    ("WorldSpaceUV_B", "world-space UVs of the second set left out"),
    ("DualUv", "second UV set of a dual-UV material left out"),
    ("Emissive", "emission left out"),
    ("Lightmap", "lightmap left out"),
    (
        "TerrainWetness",
        "terrain snow and wetness (run-time) left out: dry, snow-free",
    ),
];

impl Lowering {
    /// The `environment_plot` family's material.
    pub(super) fn environment_plot(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let bound = w.get::<MaterialTextures>(e).cloned();
        let macro_ao = matches!(
            Params::new(m, bound.as_ref()).texture("AoWetBlendContextTex"),
            Binding::Texture(_)
        );
        let pbr = self.environment_maps(w, e, m, log, LEFT_OUT)?;
        if macro_ao {
            approximate(
                log,
                e,
                "macro occlusion (second texture coordinate set) left out",
                triangles,
            );
        }
        Ok(pbr)
    }
}
