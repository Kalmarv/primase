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
//! strength and power are left out, named); `DoubleSided` the material's. The billboard permutations (a flat card:
//! `DiffuseTexture` and `RTAOTexture` at `TEXCOORD0`, the same alpha step, no roughness and no normal written) take the
//! same base colour, occlusion and mask, rough and double-sided; the impostor permutations
//! (`Impostor`) read `DiffuseTexture`, `NormalTexture`, `RTAOTexture` and `MaskTexture` at the coordinates of views in
//! atlases of hemi-octahedral views (`FramesXY_Colour` square colour frames, 4 in every material of the box; the
//! views are of directions `(x, (1 - |x| - |z|)^p, z)` for `u`, `v` of the frame's position in -1 to 1, so the
//! frames on the border are views from the horizon), chosen from the view direction and blended, which no static
//! material holds: they are drawn as one fixed horizon colour frame (the corner of the atlas, `u` and `v` 1, a view
//! along an axis of the model) as the base colour on the slot's sub-rectangle, its alpha the coverage, a double-sided
//! card, named; the object-space normal frames, the occlusion and roughness atlases (a sub-rectangle on those slots
//! is not one the raise to skate takes), the view blend and the depth march are left out, named. Tint, world tint,
//! wind animation, vertex occlusion and wetness are left out, named. A BC5 `NormalTexture` has no alpha: rough.
use super::{ALPHA_REFERENCE, Binding, Derived, Lowering, Params, alpha_tested, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{ColorSpace, Entity, TexFormat, TexRole, Texture, World};

impl Lowering {
    /// The `uber_foliage` family's material.
    pub(super) fn uber_foliage(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let impostor = p.on("Impostor");
        // a card: the billboard's and the impostor's alike, with the maps of a flat sprite and no roughness or normal
        let billboard = p.on("Billboard") || impostor;
        // the colour frame the card shows: the atlas's corner frame, `u` and `v` 1, a horizon view
        let frame = if impostor {
            match p.int("FramesXY_Colour") {
                Some(n) if n >= 1 => {
                    let n = n as f32;
                    approximate(
                        log,
                        e,
                        "impostor drawn as one fixed horizon frame (no view selection, blend or depth march)",
                        triangles,
                    );
                    Some(UvTransform {
                        offset: [(n - 1.0) / n, (n - 1.0) / n],
                        scale: [1.0 / n, 1.0 / n],
                        rotation: 0.0,
                    })
                }
                _ => {
                    approximate(
                        log,
                        e,
                        "impostor without a frame count: the whole atlas",
                        triangles,
                    );
                    None
                }
            }
        } else {
            None
        };
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffuseTexture") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(TextureRef {
                    transform: frame,
                    // the frame's edges are not its neighbours' colour
                    wrap: if frame.is_some() {
                        [Wrap::Clamp; 2]
                    } else {
                        TextureRef::new(t).wrap
                    },
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

        // RT1.w is NormalTexture.a * RoughnessScale in the high-quality permutations; no metal
        pbr.metallic = 0.0;
        pbr.roughness = 1.0;
        let scale = p.float("RoughnessScale").unwrap_or(1.0).max(0.0);
        match p.texture("NormalTexture") {
            // the impostor's normal is an object-space frame of an atlas, its roughness another
            _ if impostor => approximate(
                log,
                e,
                "impostor normal and roughness frames left out: rough, the vertex normal",
                triangles,
            ),
            // the billboard permutations write no roughness and no normal
            _ if billboard => approximate(
                log,
                e,
                "billboard writes no roughness or normal: rough, the vertex normal",
                triangles,
            ),
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

        // the albedo is darkened by RTAOTexture.b through scene constants: taken whole as the occlusion; the
        // impostor's is an atlas of frames, whose sub-rectangle an occlusion slot cannot take
        match p.texture("RTAOTexture") {
            _ if impostor => approximate(log, e, "impostor occlusion frames left out", triangles),
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
        if alpha_tested(m) || impostor {
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
        // a billboard is a flat card seen from either side
        pbr.double_sided = p.on("DoubleSided") || billboard;

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
        Ok(pbr)
    }
}
