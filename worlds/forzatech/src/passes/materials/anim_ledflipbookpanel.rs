//! # `anim_ledflipbookpanel`
//!
//! Evidence, over the family's 1 GBuffer permutation in the Tokyo track's `bin.zip` (texture indices of the parameter
//! buffer from dword 12): the albedo (target 0 XYZ) is `FlipbookTexture * Brightness * MaskTexture.r`, scene
//! constants aside, the flipbook read at `(frac(uv0.x) / XFrames + (frame mod XFrames) / XFrames, frac(uv0.y) /
//! YFrames + trunc(frame / XFrames) / YFrames)` for the frame the clock and `FramesPerSecond` pick, the mask at
//! `uv1 * MaskUVTiling`; the roughness (target 1 W) is `lerp(RoughnessBoard, RoughnessLED, MaskTexture.r)`; the metalness
//! and the normal are the literal 0 and the vertex normal; the emissive term (target 1 XYZ) reads the same two textures.
//!
//! The rule: the first frame of `FlipbookTexture` (the cell at the atlas's origin, clamped) as the base colour at
//! `Brightness` and as the emission, dielectric, opaque. Left out, named: the animation (frames after the first), the
//! LED mask (`uv1`, another coordinate set than the flipbook's: the panel is lit without its pixel gaps), the roughness
//! blend (the board's roughness is used), and the emission's light-switch schedule and distance encoding.
use super::{Binding, Lowering, Params, approximate};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{ColorSpace, Entity, TexRole, Texture, World};

impl Lowering {
    /// The `anim_ledflipbookpanel` family's material.
    pub(super) fn anim_ledflipbookpanel(
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
            roughness: p.float("RoughnessBoard").unwrap_or(1.0).clamp(0.0, 1.0),
            ..PbrMaterial::default()
        };
        let brightness = p.float("Brightness").unwrap_or(1.0).max(0.0);
        match p.texture("FlipbookTexture") {
            Binding::Texture(t) => {
                let frames = match (p.float("XFrames"), p.float("YFrames")) {
                    (Some(x), Some(y)) if x >= 1.0 && y >= 1.0 => [x.floor(), y.floor()],
                    _ => {
                        approximate(
                            log,
                            e,
                            "no frame grid (XFrames, YFrames): the whole texture",
                            triangles,
                        );
                        [1.0, 1.0]
                    }
                };
                let slot = TextureRef {
                    wrap: [Wrap::Clamp; 2],
                    transform: Some(UvTransform {
                        offset: [0.0; 2],
                        scale: [1.0 / frames[0], 1.0 / frames[1]],
                        rotation: 0.0,
                    })
                    .filter(|t| *t != UvTransform::IDENTITY),
                    ..TextureRef::new(t)
                };
                pbr.base_color_texture = Some(slot);
                pbr.base_color = [brightness, brightness, brightness, 1.0];
                pbr.emissive_texture = Some(slot);
                pbr.emissive = [1.0; 3];
                pbr.emissive_strength = brightness.max(f32::MIN_POSITIVE);
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
            }
            Binding::Absent => approximate(log, e, "flipbook texture in no archive: white", triangles),
            Binding::None => approximate(log, e, "no flipbook texture: white", triangles),
        }
        approximate(
            log,
            e,
            "animation after the first frame, the LED mask (TEXCOORD1) and the roughness blend left out",
            triangles,
        );
        approximate(
            log,
            e,
            "emission without the light-switch schedule and the distance encoding",
            triangles,
        );
        Ok(pbr)
    }
}
