//! The `pbr.*` dialect: a baked metal-rough material, glTF 2.0's material model. The common lowering target for the
//! glTF, USD and skate writers: a pass lowers a game's material (`skate.material`, `fromsoft.layered_material`,
//! `gta.*`) to a [`PbrMaterial`] on the same material entity, which keeps its dialect components.
//!
//! Meanings follow glTF 2.0 §3.9 exactly, so the glTF writer is a direct mapping: factors are linear, colour textures
//! sRGB, packed maps linear, and [`Default`] is glTF's default material.

use serde::Serialize;
use serde_json::Value;

use crate::json::JsonError;
use crate::world::{Component, Entity};

/// How UVs outside 0-1 sample a texture along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize)]
pub enum Wrap {
    #[default]
    Repeat,
    /// Clamp to the edge texel.
    Clamp,
    Mirror,
}

/// A transform of the texture coordinates a slot samples with, exactly glTF's `KHR_texture_transform` (in the world's
/// texture convention: origin top-left, v down): `uv' = offset + R(rotation) * (scale * uv)`, `R(a)` taking (u, v) to
/// (u cos a - v sin a, u sin a + v cos a). No reader sets a rotation yet.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct UvTransform {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    /// Radians.
    pub rotation: f32,
}

impl UvTransform {
    /// No change: offset 0, scale 1, rotation 0.
    pub const IDENTITY: UvTransform = UvTransform {
        offset: [0.0; 2],
        scale: [1.0; 2],
        rotation: 0.0,
    };

    /// The transform applied to one coordinate pair, in f32.
    pub fn apply(&self, [u, v]: [f32; 2]) -> [f32; 2] {
        let (u, v) = (u * self.scale[0], v * self.scale[1]);
        let (u, v) = if self.rotation == 0.0 {
            (u, v)
        } else {
            let (s, c) = self.rotation.sin_cos();
            (u * c - v * s, u * s + v * c)
        };
        [u + self.offset[0], v + self.offset[1]]
    }

    /// A tiling and offset given with the texture origin bottom-left (`uv * scale + offset`, v up: Unity's
    /// `_MainTex_ST`), for a reader that stores v as 1 - v and flips the texture's rows.
    ///
    /// The source samples its texture at v_s = v * scale_v + offset_v. In the world both the coordinate and the image
    /// are flipped, so a world coordinate w = 1 - v must sample at 1 - v_s = 1 - (1 - w) * scale_v - offset_v =
    /// w * scale_v + (1 - scale_v - offset_v): the scale is the same and the offset along v is 1 - scale_v - offset_v.
    /// u is unchanged.
    pub fn from_bottom_left(scale: [f32; 2], offset: [f32; 2]) -> UvTransform {
        UvTransform {
            offset: [offset[0], 1.0 - scale[1] - offset[1]],
            scale,
            rotation: 0.0,
        }
    }
}

/// A texture used by a material slot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TextureRef {
    /// The texture entity (`core.texture` or `core.texture_file`).
    pub texture: Entity,
    /// Which `TexCoord(n)` stream it samples.
    pub texcoord: u8,
    /// Wrap along u, then v.
    pub wrap: [Wrap; 2],
    /// Applied to the coordinates before sampling; `None` is the identity.
    pub transform: Option<UvTransform>,
}

impl TextureRef {
    /// Sampling `TexCoord(0)` with repeat wrapping, untransformed.
    pub fn new(texture: Entity) -> Self {
        TextureRef {
            texture,
            texcoord: 0,
            wrap: [Wrap::Repeat; 2],
            transform: None,
        }
    }
}

/// How the base colour's alpha is used.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize)]
pub enum AlphaMode {
    /// Alpha ignored.
    #[default]
    Opaque,
    /// Alpha tested: drawn where alpha >= `cutoff`.
    Mask { cutoff: f32 },
    /// Alpha blended over what is behind.
    Blend,
}

/// Specular reflectance, exactly glTF's `KHR_materials_specular`: the dielectric part's F0 is
/// `min(0.04 * color, 1) * factor` (glTF's default index of refraction, 1.5). Metals ignore it. What a game's own
/// specular model means in these terms is its lowering pass's named approximation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Specular {
    /// 0-1 (`specularFactor`), multiplied into `texture`'s A channel.
    pub factor: f32,
    /// Linear: A the specular strength (`specularTexture`).
    pub texture: Option<TextureRef>,
    /// Linear RGB, each >= 0 (`specularColorFactor`), multiplied into `color_texture`.
    pub color: [f32; 3],
    /// sRGB colour (`specularColorTexture`).
    pub color_texture: Option<TextureRef>,
}

impl Default for Specular {
    /// The extension's defaults: factor 1, colour white, no textures.
    fn default() -> Self {
        Specular {
            factor: 1.0,
            texture: None,
            color: [1.0; 3],
            color_texture: None,
        }
    }
}

/// A baked metal-rough material (component `pbr.material`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PbrMaterial {
    /// Linear RGBA, multiplied into `base_color_texture`.
    pub base_color: [f32; 4],
    /// sRGB colour, alpha in A.
    pub base_color_texture: Option<TextureRef>,
    /// 0-1, multiplied into the texture's B channel.
    pub metallic: f32,
    /// 0-1, multiplied into the texture's G channel.
    pub roughness: f32,
    /// Linear: G roughness, B metallic (R may hold occlusion: point `occlusion_texture` at the same entity).
    pub metallic_roughness_texture: Option<TextureRef>,
    /// Linear: R ambient occlusion.
    pub occlusion_texture: Option<TextureRef>,
    /// 0-1: how much of the occlusion applies.
    pub occlusion_strength: f32,
    /// Tangent-space normal map in glTF's convention (OpenGL green up, Z stored); a pass converts others first.
    pub normal_texture: Option<TextureRef>,
    /// Scales the normal map's X and Y.
    pub normal_scale: f32,
    /// Linear RGB, multiplied into `emissive_texture`.
    pub emissive: [f32; 3],
    /// sRGB colour.
    pub emissive_texture: Option<TextureRef>,
    /// Multiplies the emission (`KHR_materials_emissive_strength`): finite, >= 0, 1 by default. Emission brighter
    /// than 1 keeps `emissive` within 0-1 (normalised by its largest channel) and carries the rest here, so a source's
    /// radiance is kept rather than clamped.
    pub emissive_strength: f32,
    pub alpha_mode: AlphaMode,
    /// Back faces drawn too (and lit with the flipped normal).
    pub double_sided: bool,
    /// Surfaces drawn with it cast shadows; `None`: unknown (the renderer's default). A placement's
    /// [`crate::Shadows`] `cast` overrides it.
    pub cast_shadows: Option<bool>,
    /// The source's specular reflectance; `None`: it says nothing (glTF's default, the extension not written).
    pub specular: Option<Specular>,
}

impl Component for PbrMaterial {
    const NAME: &'static str = "pbr.material";
    /// 2: `emissive_strength`; 3: `cast_shadows`; 4: `specular`.
    const VERSION: u32 = 4;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        if !self.emissive_strength.is_finite() || self.emissive_strength < 0.0 {
            return Err(format!(
                "emissive strength {} is not finite and >= 0",
                self.emissive_strength
            ));
        }
        if let Some(s) = &self.specular {
            if !(s.factor.is_finite() && (0.0..=1.0).contains(&s.factor)) {
                return Err(format!("specular factor {} is not in 0-1", s.factor));
            }
            if s.color.iter().any(|c| !c.is_finite() || *c < 0.0) {
                return Err(format!("specular colour {:?} is not finite and >= 0", s.color));
            }
        }
        Ok(())
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures().map(|r| r.texture));
    }
}

impl Default for PbrMaterial {
    /// glTF's default material: white, fully metallic and rough, opaque, single-sided.
    fn default() -> Self {
        PbrMaterial {
            base_color: [1.0; 4],
            base_color_texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            normal_texture: None,
            normal_scale: 1.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            emissive_strength: 1.0,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            cast_shadows: None,
            specular: None,
        }
    }
}

impl PbrMaterial {
    /// Every texture slot that is set, in glTF's order: base colour, metallic-roughness, normal, occlusion, emissive,
    /// then the specular extension's strength and colour.
    pub fn textures(&self) -> impl Iterator<Item = TextureRef> {
        [
            self.base_color_texture,
            self.metallic_roughness_texture,
            self.normal_texture,
            self.occlusion_texture,
            self.emissive_texture,
            self.specular.and_then(|s| s.texture),
            self.specular.and_then(|s| s.color_texture),
        ]
        .into_iter()
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::{PbrMaterial, UvTransform};

    #[test]
    fn bottom_left_tiling_samples_the_same_texels() {
        // a source coordinate and its tiling, all dyadic so every step is exact
        let (scale, offset) = ([2.0f32, 0.5], [0.25f32, 0.125]);
        let t = UvTransform::from_bottom_left(scale, offset);
        assert_eq!(t.offset, [0.25, 0.375]);
        for (u, v) in [(0.0f32, 0.0f32), (0.5, 0.25), (0.75, 1.0)] {
            // the source samples its image at (u, v) * scale + offset; the world's image is flipped, so that texel is
            // at 1 - v_s in the world
            let source = [u * scale[0] + offset[0], v * scale[1] + offset[1]];
            let world = t.apply([u, 1.0 - v]);
            assert_eq!(world, [source[0], 1.0 - source[1]], "({u}, {v})");
        }
    }

    #[test]
    fn rotation_is_counter_clockwise_after_the_scale() {
        let t = UvTransform {
            offset: [0.0, 1.0],
            scale: [0.5, 0.5],
            rotation: std::f32::consts::FRAC_PI_2,
        };
        let [u, v] = t.apply([1.0, 0.0]);
        assert!(u.abs() < 1e-7 && (v - 1.5).abs() < 1e-7, "{u} {v}");
        assert_eq!(UvTransform::IDENTITY.apply([0.3, 0.7]), [0.3, 0.7]);
    }

    /// The rule the glTF writer relies on its callers to enforce: a negative, infinite or NaN strength fails
    /// verification, and the zero and default strengths pass.
    #[test]
    fn emissive_strength_is_finite_and_non_negative() {
        use crate::World;

        let failures = |strength: f32| {
            let mut w = World::new();
            let e = w.spawn();
            w.insert(
                e,
                PbrMaterial {
                    emissive_strength: strength,
                    ..PbrMaterial::default()
                },
            );
            w.verify_components()
                .into_iter()
                .map(|(name, _, detail)| (name.to_owned(), detail))
                .collect::<Vec<_>>()
        };
        for strength in [-1.0, -0.001, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let failures = failures(strength);
            assert_eq!(failures.len(), 1, "{strength}");
            assert_eq!(failures[0].0, "pbr.material", "{strength}");
            assert!(failures[0].1.contains("emissive strength"), "{:?}", failures[0]);
        }
        assert!(failures(0.0).is_empty());
        assert!(failures(1.0).is_empty());
    }
}
