//! # `diff1`
//!
//! Evidence, over the family's 1 GBuffer permutation in the Tokyo track's `bin.zip`: the albedo (target 0 XYZ) is
//! `DiffuseA` read at `uv0 * UVTilingA`, shifted by a scene sky-tint factor (`cb7[3].z`) and the occlusion constants
//! (run-time); the metalness is the literal 0, the roughness the literal 1, the normal the vertex normal, and the only
//! `discard` compares a constant of the draw with one: the surface is opaque.
//!
//! The rule: `DiffuseA` the base colour through the tiling as its transform, dielectric, rough, opaque. Left out, named:
//! the sky tint and occlusion constants.
use super::{Binding, Lowering, Params, approximate};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::PbrMaterial;
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

impl Lowering {
    /// The `diff1` family's material.
    pub(super) fn diff1(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let mut pbr = PbrMaterial {
            metallic: 0.0,
            roughness: 1.0,
            ..PbrMaterial::default()
        };
        match p.texture("DiffuseA") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(Self::slot(t, p.vec2("UVTilingA"), [0.0; 2]));
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
        approximate(
            log,
            e,
            "scene sky tint and occlusion constants (run-time) left out",
            triangles,
        );
        Ok(pbr)
    }
}
