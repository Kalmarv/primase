//! # `environmentdeferreddecal`
//!
//! Evidence, over the 10 `DeferredDecalsLightScenario` pixel shaders of the family in the Tokyo track's `bin.zip` (the
//! pass that draws the family's meshes and boxes into the decal buffer: render target 0 is the colour and its alpha,
//! 1 the normal and its weight, 2 the roughness, the occlusion, a constant and the weight), each channel followed back
//! to its texture reads and parameter rows through the family's `CBMP` and `TXMP` maps (the texture indices sit in
//! parameter rows from register 9). The 8 mesh permutations (`_Standard_DR`, `_DR_ST`, `_DR_NAO`, `_DR_NAO_ST`, the
//! two `RCSM` ones, and `_Standard` and `_Standard_NAO`, which write no colour) and the 2 projected ones
//! (`_Projected_DR`, `_Projected_DR_NAO`):
//! - `a = DiffRoughIntensity * DiffAlphaTex.a * (VertexAlphaOnOff ? COLOR.a : 1)` is the decal's opacity; the pixel is
//!   discarded below 0.01. `DiffAlphaTex` is read at `DiffAlphaUVTiling * (uv0 + Uv0Offset)`;
//! - target 0 is `(DiffAlphaTex.rgb, a)`, a colour on an sRGB view, so linear; the `_ST` permutations read
//!   `SwatchTintTexture` and the vertex colour's red as the environment's swatch tint;
//! - target 2 is `(1 - RoughMetAOTex.r, 0, 0.5, a)` in the `_DR` permutations, read at `RMAOUVTiling * (uv0 +
//!   Uv0Offset)`; the `_NAO` ones (`NormalAO`) add the occlusion `1 - max(cb2[43].x, RoughMetAOTex.b)` in Y and the
//!   weight `max(a, NormalAOIntensity * DiffAlphaTex.a * vertex alpha)` in W, and write the normal to target 1: the
//!   standard decode of `NormalTex.rg` (read at `NormalUVTiling * (uv0 + Uv0Offset)`) brought to the world as
//!   `x T - y B + z N`, `B = (N x T) w`, with weight `NormalAOIntensity * DiffAlphaTex.a * vertex alpha`;
//! - the `RCSM` permutations march `ConeMapTexture` (relaxed cone-step mapping) for the uv;
//! - the projected permutations read the same maps in a unit box projected onto the depth buffer
//!   (`engines/forzatech/effects/decals`): no mesh carries them, and the box is not a `pbr.material`.
//!
//! The rule: a decal is a blended `pbr.material` in the decal layer (`core.draw_order`, queue 2500, no depth write,
//! the convention). `DiffAlphaTex` is the base colour (its alpha is the opacity) and `DiffRoughIntensity` the colour's
//! alpha factor; `RoughMetAOTex.r` is the roughness (G of a metallic-roughness map, no metal: the decal buffer's third
//! channel is the constant 0.5), and in the `_NAO` permutations `RoughMetAOTex.b` the occlusion and `NormalTex` the
//! normal map, when `NormalAOIntensity` is not 0 (the occlusion's weight is the larger of the two). A projected permutation is left unlowered.
//!
//! The decal buffer stores `1 - RoughMetAOTex.r` and no file says how it is composited with the surface beneath
//! (`engines/forzatech/effects/decals`): the roughness is taken as the texture's red, the opposite of the buffer's
//! value, which is a reading of the environment family's texture and not of the resolve. The blend, the vertex alpha,
//! swatch tint, cone-step mapping and the weight of the normal and occlusion by their own intensities are left out,
//! named.
use super::{Binding, DECAL_QUEUE, Derived, Lowering, Params, approximate, left_out};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{ColorSpace, DrawLayer, DrawOrder, Entity, TexFormat, TexRole, Texture, World};

impl Lowering {
    /// The `environmentdeferreddecal` family's material: `None` for a projected permutation.
    pub(super) fn environmentdeferreddecal(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        if p.on("Projected") {
            return Ok(None);
        }
        let offset = p.vec2("Uv0Offset").unwrap_or([0.0; 2]);
        // the decal's opacity is DiffRoughIntensity * DiffAlphaTex.a: the colour map's alpha, blended
        let mut pbr = PbrMaterial {
            alpha_mode: AlphaMode::Blend,
            base_color: [
                1.0,
                1.0,
                1.0,
                p.float("DiffRoughIntensity").unwrap_or(1.0).clamp(0.0, 1.0),
            ],
            ..PbrMaterial::default()
        };
        match p.texture("DiffAlphaTex") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(Self::slot(t, p.vec2("DiffAlphaUVTiling"), offset));
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
            }
            Binding::Absent => approximate(log, e, "colour texture in no archive: white", triangles),
            Binding::None => approximate(log, e, "no colour texture: white", triangles),
        }

        // target 2 holds 1 - RoughMetAOTex.r, no metal (a constant), and in the NAO permutations 1 - AO in Y
        pbr.metallic = 0.0;
        pbr.roughness = 1.0;
        let nao = p.on("NormalAO");
        match p.texture("RoughMetAOTex") {
            Binding::Texture(t) => {
                // a two-channel map has no blue to read the occlusion from
                let blue = w
                    .get::<Texture>(t)
                    .is_some_and(|x| !matches!(x.format, TexFormat::Bc5Unorm | TexFormat::Bc5Snorm));
                let kind = if nao && blue {
                    Derived::OcclusionRoughnessRed
                } else {
                    Derived::RedRoughness(1.0f32.to_bits())
                };
                if nao && !blue {
                    approximate(
                        log,
                        e,
                        "occlusion map has no blue channel: no occlusion",
                        triangles,
                    );
                }
                if let Some(rm) = self.derive(w, t, kind, log)? {
                    let slot = Self::slot(rm, p.vec2("RMAOUVTiling"), offset);
                    pbr.metallic_roughness_texture = Some(slot);
                    if kind == Derived::OcclusionRoughnessRed {
                        pbr.occlusion_texture = Some(slot);
                    }
                }
                approximate(
                    log,
                    e,
                    "roughness taken as RoughMetAOTex.r, the decal buffer holds 1 - r and its resolve is in no file",
                    triangles,
                );
            }
            Binding::Absent | Binding::None => approximate(log, e, "no roughness map: rough", triangles),
        }

        if nao {
            if p.float("NormalAOIntensity").unwrap_or(1.0) <= 0.0 {
                // the occlusion keeps its weight (the larger of the two): only the normal's is 0
                approximate(
                    log,
                    e,
                    "normal weight 0 (NormalAOIntensity): no normal map",
                    triangles,
                );
            } else {
                match p.texture("NormalTex") {
                    Binding::Texture(t) => {
                        let tiling = p.vec2("NormalUVTiling");
                        if let Some(slot) =
                            self.normal_slot(w, e, t, triangles, log, |n| Self::slot(n, tiling, offset))?
                        {
                            pbr.normal_texture = Some(slot);
                        }
                    }
                    Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
                }
            }
        }

        for (switch, what) in [
            ("VertexAlphaOnOff", "vertex alpha (COLOR.a) left out"),
            ("SwatchTint", "swatch tint left out"),
            ("RCSM", "relaxed cone-step mapping left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        approximate(
            log,
            e,
            "decal buffer blend taken as an alpha blend over the surface",
            triangles,
        );
        w.insert(e, DrawOrder::conventional(DrawLayer::Decal, DECAL_QUEUE));
        Ok(Some(pbr))
    }
}
