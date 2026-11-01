//! # `emissiveonly`
//!
//! Evidence, over the family's 1 GBuffer permutation in the Tokyo track's `bin.zip`: the albedo (target 0 XYZ) is the
//! constant `Color` shifted by a scene sky-tint factor (`cb7[3].z`, a run-time constant) and the occlusion constants;
//! the metalness is the literal 0, the roughness the literal 1, the normal the vertex normal, no texture is read; the
//! emissive term (target 1 XYZ) is `Color * EmissiveMultiplier * g(d)`, `g(d) = d >= 1 ? 1 / (d + 1e-5)^2 : (d +
//! 1e-5)^2` the camera distance's encoding; the only `discard` compares a constant of the draw with one.
//!
//! The rule: base colour `Color`, dielectric, rough, opaque; emission `Color` at `EmissiveMultiplier` (the part above
//! 1 in `emissive_strength`). Left out, named: the sky-tint and occlusion constants and the distance encoding `g(d)`,
//! whose later undoing is not determinable from the shader.
use super::{Lowering, Params, approximate};
use crate::components::ForzaMaterial;
use primase_core::pass::PassLog;
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, World};

impl Lowering {
    /// The `emissiveonly` family's material.
    pub(super) fn emissiveonly(
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
            ..PbrMaterial::default()
        };
        let Some(c) = p.vec4("Color") else {
            approximate(log, e, "no Color: white", triangles);
            return Ok(pbr);
        };
        let colour = [c[0].max(0.0), c[1].max(0.0), c[2].max(0.0)];
        pbr.base_color = [colour[0], colour[1], colour[2], 1.0];
        let multiplier = p.float("EmissiveMultiplier").unwrap_or(0.0).max(0.0);
        let radiance = colour.map(|x| x * multiplier);
        let strength = radiance.iter().copied().fold(0.0f32, f32::max);
        if strength > 0.0 {
            pbr.emissive = radiance.map(|x| x / strength.max(1.0));
            pbr.emissive_strength = strength.max(1.0);
            approximate(log, e, "emission without the distance encoding g(d)", triangles);
        }
        approximate(
            log,
            e,
            "scene sky tint and occlusion constants (run-time) left out",
            triangles,
        );
        Ok(pbr)
    }
}
