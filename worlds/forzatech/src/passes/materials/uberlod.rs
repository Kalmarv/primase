//! # `uberlod`
//!
//! Evidence, over the two G-buffer permutations (`_Default`, `_Rock_NM`) of the Tokyo track, and the `_Default` one that
//! the box's 10 materials (8,568 triangles, all with `RoughnessMetal` and `Emissive` on, `NormalMap` and `Modulate` off)
//! use: every map is read at `TEXCOORD0` as it is. The albedo is `DiffuseATexture.rgb` mixed toward its luma by a factor
//! of the vertex normal's `y` and two scene constants, times a scene constant (run-time state); the roughness is
//! `RoughnessMetalnessATexture.r` and the metalness `.g` (the constant `Roughness` is read only by `_Rock_NM`); the
//! normal is the vertex normal; nothing tests alpha. The emission (target 1 XYZ) is `EmissiveTexture.rgb * EmissiveScale *
//! g(d)`, `g(d)` the distance encoding (see `emissiveonly`), with no instance variation and no lightmap.
//!
//! The rule: `DiffuseATexture` the base colour; the roughness-metalness map re-encoded as glTF's (the map is a BC1 sRGB
//! view, read as decoded); the emission texture as the emissive map and `EmissiveScale` as the factor, when the texture
//! holds any light. Left out, named: the albedo's scene-dependent desaturation and scale, `g(d)` and the sky preset's
//! emissive scale (run-time). A material with `NormalMap` on (the `_Rock_NM` permutation, not read here) is left unlowered by
//! name.
use super::{Binding, Derived, Lowering, Params, approximate};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

impl Lowering {
    /// The `uberlod` family's material: the far view of the heightfield's clusters.
    pub(super) fn uberlod(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        if p.on("NormalMap") {
            return Ok(None);
        }
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffuseATexture") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(TextureRef::new(t));
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

        pbr.metallic = p.float("Metalness").unwrap_or(0.0).clamp(0.0, 1.0);
        pbr.roughness = p.float("Roughness").unwrap_or(1.0).clamp(0.0, 1.0);
        if p.on("RoughnessMetal") {
            match p.texture("RoughnessMetalnessATexture") {
                Binding::Texture(t) => {
                    if let Some(rm) = self.derive(w, t, Derived::RoughnessMetalRg, log)? {
                        pbr.metallic_roughness_texture = Some(TextureRef::new(rm));
                        pbr.metallic = 1.0;
                        pbr.roughness = 1.0;
                    }
                }
                Binding::Absent | Binding::None => approximate(
                    log,
                    e,
                    "no roughness-metalness map: the constant roughness and metalness",
                    triangles,
                ),
            }
        }

        approximate(
            log,
            e,
            "albedo desaturation by the surface's slope and the scene's scale left out (run-time constants)",
            triangles,
        );
        if p.on("Emissive") {
            let scale = p.float("EmissiveScale").unwrap_or(0.0).max(0.0);
            let texture = p.texture("EmissiveTexture");
            self.emission_map(w, e, &mut pbr, texture, scale, log);
        }
        Ok(Some(pbr))
    }
}
