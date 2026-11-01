//! # `pylon_cables`
//!
//! Evidence, over the 2 GBuffer permutations of the family in the Tokyo track's `bin.zip` (`_Standard` and `_LineInner`,
//! each render target channel followed back through the `CBMP` and `TXMP` maps): the albedo (target 0 XYZ) is the
//! constant `Color` times a scene factor (`cb2[43]`, `[44]`, `[159]`, the ambient-occlusion constants, 1 when they are
//! neutral); the metalness (target 0 W) is the literal 0 and the roughness (target 1 W) the literal 1, so
//! `Roughness` reaches only the lit colour (target 1 XYZ); the normal (target 2) of `_Standard` is the vertex normal
//! (`_LineInner` writes none, a constant of the game); no texture reaches the albedo, and `Lightmap` (the only
//! texture, on `uv1`) enters only the emission, behind `LightmapActive`.
//!
//! The pixel is discarded when a dither of `ClipInTrackLS` (a screen-space texture read, `1 - t103 < cb2[128].x`) fails
//! and, in `_LineInner`, when `sqrt(saturate(LineThickness / (distance * pixel scale)))` is below the draw's reference:
//! the cable is a line of constant screen width that thins out with distance.
//!
//! The rule: `pbr.material` of base colour `Color`, dielectric, rough, opaque, double-sided (a cable is a thin
//! ribbon seen from either side). Left out, named: the lightmap emission (`LightmapActive`, off in every material of
//! the box), the dither, `LineThickness`'s distance thinning, and the scene's occlusion constants.
use super::{Lowering, Params, approximate, left_out};
use crate::components::ForzaMaterial;
use primase_core::pass::PassLog;
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, World};

impl Lowering {
    /// The `pylon_cables` family's material.
    pub(super) fn pylon_cables(
        &mut self,
        _w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let p = Params::new(m, None);
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let mut pbr = PbrMaterial {
            metallic: 0.0,
            roughness: 1.0,
            double_sided: true,
            ..PbrMaterial::default()
        };
        match p.vec4("Color") {
            Some(c) => pbr.base_color = [c[0].max(0.0), c[1].max(0.0), c[2].max(0.0), 1.0],
            None => approximate(log, e, "no Color: white", triangles),
        }
        left_out(
            log,
            &p,
            e,
            "LightmapActive",
            "lightmap emission left out",
            triangles,
        );
        if p.on("LineInner") {
            approximate(
                log,
                e,
                "the line's screen-space width and its thinning with distance left out",
                triangles,
            );
        }
        approximate(
            log,
            e,
            "dither fade and scene occlusion constants (run-time) left out",
            triangles,
        );
        Ok(pbr)
    }
}
