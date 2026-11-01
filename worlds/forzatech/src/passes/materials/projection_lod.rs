//! # `projection_lod`
//!
//! Evidence, over the 24 GBuffer permutations: every map (`DiffuseATexture`, `RoughnessMetalnessATexture`,
//! `NormalATexture`, `EmissiveTexture`, `BakedVCTexture`) is read at `UVOutMinMax.xy + (UVOutMinMax.zw -
//! UVOutMinMax.xy) * TEXCOORD0`, the atlas cell of the building's far view; the albedo is `DiffuseATexture.rgb` times
//! `(1 - a (1 - v)) (1 - s) + s`, `a` the diffuse alpha, `v = BakedVCTexture.r`, `s = SwatchTintTexture.r` (the swatch
//! row `BakedVCTexture.g` picks, from the instance variation); with `RoughnessMetal` the roughness is
//! `RoughnessMetalnessATexture.r` and the metalness `.g` (otherwise the constant `Roughness` and 0); the HiQ
//! permutations read the normal from `NormalATexture.rg` as the environment does and the others take the vertex normal;
//! the alpha-test permutations discard when `DiffuseATexture.a` is below the draw's reference. The roughness-metalness
//! maps are BC5 SNORM (9,650 triangles) or BC7 sRGB (352): the shader reads `v` of the signed one, so a roughness of 0
//! to 1 is its positive half (19 of the 36 maps' 34,701,312 channels in the box are below -0.06), and the sRGB one as decoded.
//!
//! The rule: the cell is each slot's transform; `DiffuseATexture` the base colour, masked at the middle reference
//! where the permutation tests alpha; the roughness-metalness map re-encoded as glTF's (G roughness, B metallic, the
//! signed half-range read back); `NormalATexture` the normal map. The baked occlusion factor (two textures), swatch
//! tint, lightmap and wetness are left out, named.
//!
//! The emission (target 1 XYZ, on with `Emissive`) is `EmissiveTexture.rgb * EmissiveTextureScale * v * g(d)`, read at
//! the same cell, `v = sat((y - EmissiveVariationMin) / (EmissiveVariationMax - EmissiveVariationMin))` of the
//! instance variation's second value `y` and `g(d)` the distance encoding (see `emissiveonly`). The rule: the texture at
//! the cell as the emissive map, the scale as the emissive factor (what is above 1 in `emissive_strength`), when the
//! texture holds any light: most materials bind the black `ubershader_default_emissive`, and those emit nothing. `v` is
//! the placement's: the material carries its ramp ([`crate::components::EmissionVariation`]) and
//! `apply-forzatech-emission-variation` applies it. Left out, named: `g(d)` and the sky preset's emissive scale
//! (run-time).
use super::{ALPHA_REFERENCE, Binding, Derived, Lowering, Params, alpha_tested, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

impl Lowering {
    /// The `projection_lod` family's material: the atlas cell of a building's far view.
    pub(super) fn projection_lod(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        // every map is read at `UVOutMinMax.xy + (zw - xy) * uv0`
        let cell = p.vec4("UVOutMinMax").map(|c| UvTransform {
            offset: [c[0], c[1]],
            scale: [c[2] - c[0], c[3] - c[1]],
            rotation: 0.0,
        });
        let slot = |texture: Entity| TextureRef {
            transform: cell.filter(|t| *t != UvTransform::IDENTITY),
            ..TextureRef::new(texture)
        };
        if cell.is_none() {
            approximate(
                log,
                e,
                "no atlas cell (UVOutMinMax): the whole texture",
                triangles,
            );
        }
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffuseATexture") {
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

        // RoughnessMetal: roughness is RoughnessMetalnessATexture.r and metalness .g
        pbr.metallic = 0.0;
        pbr.roughness = p.float("Roughness").unwrap_or(1.0).clamp(0.0, 1.0);
        if p.on("RoughnessMetal") {
            match p.texture("RoughnessMetalnessATexture") {
                Binding::Texture(t) => {
                    if let Some(rm) = self.derive(w, t, Derived::RoughnessMetalRg, log)? {
                        pbr.metallic_roughness_texture = Some(slot(rm));
                        pbr.metallic = 1.0;
                        pbr.roughness = 1.0;
                    }
                }
                Binding::Absent | Binding::None => {
                    approximate(
                        log,
                        e,
                        "no roughness-metalness map: dielectric, the constant roughness",
                        triangles,
                    );
                }
            }
        }

        // the HiQ permutation reads NormalATexture's red and green; the others use the vertex normal
        match p.texture("NormalATexture") {
            Binding::Texture(t) => {
                if Self::normal_map(w, t, log) {
                    pbr.normal_texture = Some(slot(t));
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

        // the albedo's factor 1 - DiffuseA.a (1 - BakedVCTexture.r) is a baked occlusion from two textures
        approximate(
            log,
            e,
            "baked occlusion (BakedVCTexture masked by the diffuse alpha) left out",
            triangles,
        );
        if p.on("Emissive") {
            let scale = p.float("EmissiveTextureScale").unwrap_or(0.0).max(0.0);
            let bound = p.texture("EmissiveTexture");
            self.emission_map(w, e, &mut pbr, bound, scale, log);
            if let Some(texture) = pbr.emissive_texture.as_mut() {
                *texture = slot(texture.texture);
            }
            Self::vary_emission(w, e, &p, &pbr);
        }
        for (switch, what) in [
            ("Tint", "swatch tint left out"),
            ("Lightmap", "lightmap left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(Some(pbr))
    }
}
