//! # `anim_flag`
//!
//! Evidence, over the 9 GBuffer permutations of the family in the Tokyo track's `bin.zip` (each render target channel
//! followed back through the `CBMP` and `TXMP` maps; the texture indices of this family's parameter buffer start at
//! dword 32): the albedo (target 0 XYZ) is `DiffuseATexture` read at `uv0 + DiffuseUVOffset` (the offset picks a flag in
//! an atlas) times a factor of `AOTexture.r` (read at `uv1`) and scene constants, taken through wetness; the metalness
//! (target 0 W) is `RoughnessMetalnessATexture.g` and the roughness (target 1 W) `RoughnessMetalnessATexture.r` (both
//! at `uv0`) with the ambient occlusion and wetness terms; the normal (target 2) is `NormalATexture` at `uv0` plus
//! `COLOR.r * COLOR.a * NormalAnimTexture` read at `(uv2 + 1 - time) * NormalAnimUVTiling`, a wind ripple of the
//! run-time clock; the `AlphaTest` permutations discard when `DiffuseATexture.a` is below the draw's reference
//! (`cb2[128].x`).
//!
//! The rule: `DiffuseATexture` the base colour through `DiffuseUVOffset` as its transform, masked at the middle
//! reference where the permutation tests alpha; `RoughnessMetalnessATexture` re-encoded as glTF's roughness-metalness
//! map; `NormalATexture` the normal map. Left out, named: the occlusion map (`uv1`, another coordinate set than the
//! others, which a material cannot be sampled through alongside), the wind ripple (a run-time clock and vertex colours), the
//! transmissive (translucent cloth) lighting of the `Transmissive` permutations, and wetness.
use super::{ALPHA_REFERENCE, Binding, Derived, Lowering, Params, alpha_tested, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

impl Lowering {
    /// The `anim_flag` family's material.
    pub(super) fn anim_flag(
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

        match p.texture("DiffuseATexture") {
            Binding::Texture(t) => {
                let offset = p.vec2("DiffuseUVOffset").unwrap_or([0.0; 2]);
                pbr.base_color_texture = Some(TextureRef {
                    transform: Some(UvTransform {
                        offset,
                        scale: [1.0; 2],
                        rotation: 0.0,
                    })
                    .filter(|t| *t != UvTransform::IDENTITY),
                    ..TextureRef::new(t)
                });
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

        match p.texture("RoughnessMetalnessATexture") {
            Binding::Texture(t) => {
                if let Some(rm) = self.derive(w, t, Derived::RoughnessMetalRg, log)? {
                    pbr.metallic_roughness_texture = Some(TextureRef::new(rm));
                    pbr.metallic = 1.0;
                    pbr.roughness = 1.0;
                }
            }
            Binding::Absent | Binding::None => {
                approximate(log, e, "no roughness-metalness map: dielectric, rough", triangles)
            }
        }

        match p.texture("NormalATexture") {
            Binding::Texture(t) => {
                if Self::normal_map(w, t, log) {
                    pbr.normal_texture = Some(TextureRef::new(t));
                } else {
                    approximate(log, e, "normal map not lowered", triangles);
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
        }

        // discard if DiffuseATexture.a < reference, the reference a per-draw constant
        if alpha_tested(m) {
            pbr.alpha_mode = AlphaMode::Mask {
                cutoff: ALPHA_REFERENCE,
            };
            approximate(log, e, "alpha-test reference taken as one half", triangles);
        }

        if matches!(p.texture("AOTexture"), Binding::Texture(_)) {
            approximate(
                log,
                e,
                "occlusion map (TEXCOORD1) left out: a material is sampled through one coordinate set",
                triangles,
            );
        }
        approximate(
            log,
            e,
            "wind ripple of the normal (run-time clock) and wetness left out",
            triangles,
        );
        left_out(
            log,
            &p,
            e,
            "Transmissive",
            "transmissive (translucent cloth) lighting left out",
            triangles,
        );
        Ok(pbr)
    }
}
