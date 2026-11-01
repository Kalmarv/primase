//! # `window_linecastroom`
//!
//! Evidence, over the 51 GBuffer permutations of the family in the Tokyo track's `bin.zip` (each render target channel
//! followed back through the `CBMP` and `TXMP` maps; the texture indices of this family's parameter buffer start at
//! dword 124): the albedo (target 0 XYZ) is
//! `lerp(t, FrameDiffuseandMask.rgb, FrameDiffuseandMask.a) * (1 - (1 - Fittings.a)(1 - FrameDiffuseandMask.a))` with
//! `t` the fittings' colour (`FittingsDiffAlphaTex.r`, `uv1` in its atlas) tinted by `WindowTintColour`, so where neither
//! the frame nor a fitting covers the pane the albedo is 0; the frame atlas is read at `uv2` (the `_FT` permutations;
//! `FrameUseAtlasOnOff` picks the cell `(FrameAtlasIndex + uv2) / FrameAtlasCount`); the metalness (target 0 W) is the
//! constant `Metalness`; the roughness (target 1 W) is `max(FrameGlassRoughnessTexture(uv2).r, lerp(1 - COLOR.a,
//! GlassRoughnessTexture(uv0 * GlassRoughnessUVTiling).r, COLOR.a))`; the normal (target 2) is the standard decode of
//! `FrameGlassNormalTexture(uv2)`. The room behind the pane (`BoxDiffRoughTex`, `BoxEmissiveTex`, up to three
//! furniture layers with parallax) is an interior mapping done in the pixel shader from the world position, and
//! reaches the lit colour (target 1 XYZ), not the albedo. The only `discard` compares a constant of the draw with one.
//!
//! The rule: the frame is the surface, sampled on `TEXCOORD2` alone (a material is sampled through one coordinate set, and the
//! frame is the one that carries the glass normal and roughness): the base colour is `FrameDiffuseandMask.rgb` times
//! its mask, baked opaque (black where the mask is 0, as the shader's albedo is); the roughness the frame glass
//! roughness texture's red (the map's blue is 1, so that the constant `Metalness` is the metallic factor); the normal
//! `FrameGlassNormalTexture`. Left out, named per material and triangle: the room and its emission, the fittings (`uv1`),
//! the glass roughness texture tiled on `uv0`, the fittings' tint, and wetness and lighting.
use super::{Binding, Derived, Lowering, Params, approximate, left_out, seen};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{ColorSpace, Entity, Name, TexRole, Texture, World};
use primase_image::{TextureMetadata, map_rgba8_levels};

/// The coordinate set of the window frame.
const FRAME_SET: u8 = 2;

impl Lowering {
    /// `frame` as the shader's albedo sees it: `rgb * a`, opaque, sRGB. Made once per source.
    fn window_frame_colour(
        &mut self,
        w: &mut World,
        frame: Entity,
        log: &mut PassLog,
    ) -> Result<Option<Entity>, String> {
        if let Some(&e) = self.baked.get(&(frame, vec![0], None)) {
            return Ok(Some(e));
        }
        let Some(t) = w.get::<Texture>(frame) else {
            log.count("window frame maps left out: no pixels", 1);
            return Ok(None);
        };
        let space = t.color_space;
        let texture = map_rgba8_levels(
            t,
            0..t.levels.len(),
            TextureMetadata {
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
            |texel| {
                let [r, g, b, a] = texel.rgba;
                let mask = f64::from(a) / 255.0;
                let weighted = |x: u8| {
                    let linear = seen(space, x) * mask;
                    (primase_foundations::color::linear_to_srgb(linear) * 255.0).round() as u8
                };
                Ok([weighted(r), weighted(g), weighted(b), 255])
            },
        )
        .map_err(|e| format!("masking a window frame: {e}"))?;
        let name = w.get::<Name>(frame).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} (frame colour)")));
        w.insert(e, texture);
        log.count("window frame textures baked as colour maps", 1);
        self.baked.insert((frame, vec![0], None), e);
        Ok(Some(e))
    }

    /// The `window_linecastroom` family's material.
    pub(super) fn window_linecastroom(
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
            base_color: [0.0, 0.0, 0.0, 1.0],
            metallic: p.float("Metalness").unwrap_or(0.0).clamp(0.0, 1.0),
            roughness: 1.0,
            ..PbrMaterial::default()
        };

        // the frame atlas cell, `(index + uv2) / count`, on the frame's coordinate set
        let cell = if p.on("FrameUseAtlasOnOff") {
            match (p.vec2("FrameAtlasIndex"), p.vec2("FrameAtlasCount")) {
                (Some(i), Some(n))
                    if n.iter().all(|x| x.trunc() >= 1.0) && i.iter().all(|x| x.is_finite()) =>
                {
                    let (nx, ny) = (n[0].trunc(), n[1].trunc());
                    Some(UvTransform {
                        offset: [i[0].trunc() / nx, i[1].trunc() / ny],
                        scale: [1.0 / nx, 1.0 / ny],
                        rotation: 0.0,
                    })
                }
                _ => {
                    approximate(log, e, "no frame atlas cell: the whole atlas", triangles);
                    None
                }
            }
        } else {
            None
        };
        let slot = |texture: Entity| TextureRef {
            texcoord: FRAME_SET,
            wrap: [Wrap::Clamp; 2],
            transform: cell.filter(|t| *t != UvTransform::IDENTITY),
            ..TextureRef::new(texture)
        };

        if !self.has_set(e, FRAME_SET) {
            approximate(
                log,
                e,
                "a primitive without the frame's coordinates (TEXCOORD2): no frame textures",
                triangles,
            );
        } else if p.on("FrameTex") {
            match p.texture("FrameDiffuseandMask") {
                Binding::Texture(t) => {
                    if let Some(colour) = self.window_frame_colour(w, t, log)? {
                        pbr.base_color_texture = Some(slot(colour));
                    }
                }
                Binding::Absent | Binding::None => approximate(log, e, "no frame texture: black", triangles),
            }
            if let Binding::Texture(t) = p.texture("FrameGlassRoughnessTexture")
                && let Some(rough) = self.derive(w, t, Derived::RedRoughnessMetal, log)?
            {
                // the map's G holds the roughness and its B is 1: `Metalness` stays the metallic factor
                pbr.metallic_roughness_texture = Some(slot(rough));
                pbr.roughness = 1.0;
            }
            if let Binding::Texture(t) = p.texture("FrameGlassNormalTexture") {
                if Self::normal_map(w, t, log) {
                    pbr.normal_texture = Some(slot(t));
                } else {
                    approximate(log, e, "normal map not lowered", triangles);
                }
            }
        } else {
            approximate(log, e, "no frame (FrameTex off): a black pane", triangles);
        }

        approximate(
            log,
            e,
            "room behind the pane (interior mapping, furniture, emission) left out: black",
            triangles,
        );
        left_out(
            log,
            &p,
            e,
            "Fittings",
            "fittings (TEXCOORD1) and their tint left out",
            triangles,
        );
        approximate(
            log,
            e,
            "glass roughness texture (TEXCOORD0), wetness and lighting left out",
            triangles,
        );
        Ok(pbr)
    }
}
