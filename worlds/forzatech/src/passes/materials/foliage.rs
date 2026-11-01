//! # `uber_foliage`
//!
//! Evidence, over the 27 GBuffer permutations of the family (the dependency census of each render target channel;
//! the arithmetic of the bush and tree permutations read out beside it): the albedo is `DiffuseTexture.rgb` times a
//! factor of `RTAOTexture.b` that the scene's constants (`cb2[43]`, `[44]`, `[159]`) and the vertex normal's Y shape;
//! the roughness (target 1 W) is `NormalTexture.a * RoughnessScale` in the 11 high-quality, static and punch-through tree,
//! bush and grass permutations (the very-low-quality ones write none, nor a normal), and the metalness is 0;
//! the normal (target 2) of the bush permutations is the standard decode of `NormalTexture.rg` taken to the world on
//! the vertex tangent (fading to the up vector beyond 4,500 m), and the tree's a canopy normal of all four channels
//! and the tree's bounding box (`CanopyNormal*`), which no glTF slot holds; the pixel is discarded when
//! `saturate((DiffuseTexture.a - AlphaStepMin) / (AlphaStepMax - AlphaStepMin))` is below the draw's reference
//! (`cb2[128].x`), so the alpha is tested against `AlphaStepMin + reference * (AlphaStepMax - AlphaStepMin)`. Every
//! map is read at `TEXCOORD0` untiled.
//!
//! The rule: `DiffuseTexture` the base colour, its alpha masked at the middle reference; `NormalTexture.a * scale`
//! re-encoded as the roughness of a metallic-roughness map (no metal), which glTF reads at `TEXCOORD0` as the shader
//! does; the bush's `NormalTexture` the normal map; `RTAOTexture.b` re-encoded as the occlusion map (its scene
//! strength and power are left out, named); `DoubleSided` the material's. The billboard and impostor permutations
//! (atlases of views) are left unlowered; tint, world tint, wind animation, vertex occlusion and wetness are left out,
//! named. A BC5 `NormalTexture` has no alpha: rough.
use super::{ALPHA_REFERENCE, Binding, Derived, Lowering, Params, alpha_tested, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{ColorSpace, Entity, TexFormat, TexRole, Texture, World};

impl Lowering {
    /// The `uber_foliage` family's material: `None` for the billboard and impostor permutations, whose textures are
    /// atlases of views, not the leaf and bark maps.
    pub(super) fn uber_foliage(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        if p.on("Billboard") || p.on("Impostor") {
            return Ok(None);
        }
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffuseTexture") {
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

        // RT1.w is NormalTexture.a * RoughnessScale in the high-quality permutations; no metal
        pbr.metallic = 0.0;
        pbr.roughness = 1.0;
        let scale = p.float("RoughnessScale").unwrap_or(1.0).max(0.0);
        match p.texture("NormalTexture") {
            Binding::Texture(t) => {
                let alpha = w
                    .get::<Texture>(t)
                    .is_some_and(|x| !matches!(x.format, TexFormat::Bc5Unorm | TexFormat::Bc5Snorm));
                if alpha {
                    if let Some(rough) = self.derive(w, t, Derived::AlphaRoughness(scale.to_bits()), log)? {
                        pbr.metallic_roughness_texture = Some(TextureRef::new(rough));
                    }
                } else {
                    approximate(log, e, "roughness map has no alpha channel: rough", triangles);
                }
                // the bush's normal is the map's red and green, brought to the world on the vertex tangent; the
                // tree's is a canopy normal of all four channels and the bounding box, which has no glTF form
                if p.on("Bush") {
                    if Self::normal_map(w, t, log) {
                        pbr.normal_texture = Some(TextureRef::new(t));
                    } else {
                        approximate(log, e, "normal map not lowered", triangles);
                    }
                } else {
                    approximate(log, e, "canopy normal left out", triangles);
                }
            }
            Binding::Absent | Binding::None => {
                approximate(log, e, "no normal and roughness map: rough", triangles)
            }
        }

        // the albedo is darkened by RTAOTexture.b through scene constants: taken whole as the occlusion
        match p.texture("RTAOTexture") {
            Binding::Texture(t) => {
                if let Some(occlusion) = self.derive(w, t, Derived::BlueOcclusion, log)? {
                    pbr.occlusion_texture = Some(TextureRef::new(occlusion));
                }
                approximate(log, e, "occlusion strength of the scene left out", triangles);
            }
            Binding::Absent | Binding::None => approximate(log, e, "no occlusion map", triangles),
        }

        // discard if sat((a - AlphaStepMin) / (AlphaStepMax - AlphaStepMin)) < reference, the reference a per-draw
        // constant: alpha at the point of the range the middle reference picks
        if alpha_tested(m) {
            let min = p.float("AlphaStepMin").unwrap_or(0.0);
            let max = p.float("AlphaStepMax").unwrap_or(1.0);
            pbr.alpha_mode = AlphaMode::Mask {
                cutoff: (min + ALPHA_REFERENCE * (max - min)).clamp(0.0, 1.0),
            };
            approximate(
                log,
                e,
                "alpha-test reference taken as one half of the step range",
                triangles,
            );
        }
        pbr.double_sided = p.on("DoubleSided");

        for (switch, what) in [
            ("Tint", "tint (colour lookup) left out"),
            ("UseWorldTint", "world tint left out"),
            ("ApplyWetness", "wetness (run-time) left out: dry"),
            ("Animated", "wind animation left out"),
            ("Vertex_AO", "vertex occlusion left out"),
            ("DistantColourBlend", "distant colour blend left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(Some(pbr))
    }
}
