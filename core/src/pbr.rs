//! The `pbr.*` dialect: a baked metal-rough material, glTF 2.0's material model. The common lowering target for the
//! glTF and USD writers and a destination's: a pass lowers a source's material (`layered_material`, ...) to a
//! [`PbrMaterial`] on the same material entity, which keeps its dialect components.
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

    /// A tiling and offset given with the texture origin bottom-left (`uv * scale + offset`, v up: one source's
    /// `_MainTex_ST`-style uniform), for a reader that stores v as 1 - v and flips the texture's rows.
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

/// How texels are filtered when a texture is magnified or minified (glTF's sampler filters).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Filter {
    /// The nearest texel: crisp texels.
    Nearest,
    /// Bilinear between the four nearest texels.
    Linear,
}

/// How mip levels are used when a texture is minified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum MipFilter {
    /// No mip levels: the base level only (glTF's `NEAREST` or `LINEAR` minification filter).
    None,
    /// The nearest mip level.
    Nearest,
    /// Between the two nearest mip levels (trilinear with a linear `min`).
    Linear,
}

/// A sampler's filters as the source states them: each `None` where it states none (the renderer's default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Filters {
    /// Magnification.
    pub mag: Option<Filter>,
    /// Minification within a level.
    pub min: Option<Filter>,
    /// Minification across mip levels.
    pub mip: Option<MipFilter>,
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
    /// The sampler's filters, as the source states them.
    pub filters: Filters,
}

impl TextureRef {
    /// Sampling `TexCoord(0)` with repeat wrapping, untransformed, no filter stated.
    pub fn new(texture: Entity) -> Self {
        TextureRef {
            texture,
            texcoord: 0,
            wrap: [Wrap::Repeat; 2],
            transform: None,
            filters: Filters::default(),
        }
    }
}

/// Whether a material is lit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum Shading {
    /// Lit by the scene's lights (glTF's metal-rough model).
    #[default]
    Lit,
    /// Not lit, exactly `KHR_materials_unlit`: the colour drawn is `base_color` x `base_color_texture` x the vertex
    /// colour (`COLOR_0`), whatever lights the scene. Its other slots stay as the source stated them (a writer may
    /// emit them as the fallback glTF prescribes); a renderer draws none of them.
    Unlit,
}

/// How the base colour's alpha is used.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize)]
pub enum AlphaMode {
    /// Alpha ignored.
    #[default]
    Opaque,
    /// Alpha tested: drawn where alpha >= `cutoff` (finite and >= 0, as glTF's `alphaCutoff`).
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
    /// Linear RGBA, each channel within 0-1 (verified), multiplied into `base_color_texture`.
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
    /// Scales the normal map's X and Y; finite.
    pub normal_scale: f32,
    /// Linear RGB, each channel within 0-1 (verified), multiplied into `emissive_texture`.
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
    /// The source's specular reflectance; `None`: it says nothing (glTF's default, the extension not written).
    pub specular: Option<Specular>,
    /// Lit or not. Whether what it draws casts shadows is the drawing entity's ([`crate::Shadows`]), not the
    /// material's.
    pub shading: Shading,
}

impl Component for PbrMaterial {
    const NAME: &'static str = "pbr.material";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// 2: `emissive_strength`; 3: `cast_shadows`; 4: `specular`; 5: `shading` and each texture's `filters`,
    /// `cast_shadows` gone (shadow casting is [`crate::Shadows`]', per drawn entity).
    const VERSION: u32 = 5;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Every range its fields document: the colours, factors and strengths, the mask cutoff and each texture's
    /// coordinate transform.
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        let unit = |x: f32| (0.0..=1.0).contains(&x);
        if !self.base_color.iter().all(|&c| unit(c)) {
            return Err(format!("base colour {:?} is not within 0-1", self.base_color));
        }
        for (name, x) in [
            ("metallic", self.metallic),
            ("roughness", self.roughness),
            ("occlusion strength", self.occlusion_strength),
        ] {
            if !unit(x) {
                return Err(format!("{name} {x} is not within 0-1"));
            }
        }
        if !self.normal_scale.is_finite() {
            return Err(format!("normal scale {} is not finite", self.normal_scale));
        }
        if let AlphaMode::Mask { cutoff } = self.alpha_mode
            && !(cutoff.is_finite() && cutoff >= 0.0)
        {
            return Err(format!("mask cutoff {cutoff} is not finite and >= 0"));
        }
        for (slot, r) in self.slots() {
            if let Some(t) = r.transform
                && !t
                    .offset
                    .iter()
                    .chain(&t.scale)
                    .chain([&t.rotation])
                    .all(|x| x.is_finite())
            {
                return Err(format!("{slot:?} texture transform {t:?} is not finite"));
            }
        }
        if !self.emissive.iter().all(|c| (0.0..=1.0).contains(c)) {
            return Err(format!(
                "emissive {:?} is not within 0-1 (brighter emission is carried by emissive_strength)",
                self.emissive
            ));
        }
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
    /// glTF's default material: white, fully metallic and rough, opaque, single-sided, lit.
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
            specular: None,
            shading: Shading::Lit,
        }
    }
}

/// A texture slot of a [`PbrMaterial`], in glTF's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextureSlot {
    BaseColor,
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
    /// The specular extension's strength (`specularTexture`).
    Specular,
    /// The specular extension's colour (`specularColorTexture`).
    SpecularColor,
}

impl TextureSlot {
    /// Its name in a report.
    pub fn name(self) -> &'static str {
        match self {
            TextureSlot::BaseColor => "base colour",
            TextureSlot::MetallicRoughness => "metallic-roughness",
            TextureSlot::Normal => "normal",
            TextureSlot::Occlusion => "occlusion",
            TextureSlot::Emissive => "emissive",
            TextureSlot::Specular => "specular",
            TextureSlot::SpecularColor => "specular colour",
        }
    }
}

/// Every texture slot of material `$m` with each slot's field borrowed as `$b` (nothing: by value; `&mut`: mutably),
/// in glTF's order: the one list [`PbrMaterial::slots`] and [`PbrMaterial::slots_mut`] both read.
macro_rules! texture_slots {
    ($m:expr, $($b:tt)*) => {{
        let m = $m;
        let (specular, color) = match $($b)* m.specular {
            Some(s) => (Some($($b)* s.texture), Some($($b)* s.color_texture)),
            None => (None, None),
        };
        [
            (TextureSlot::BaseColor, Some($($b)* m.base_color_texture)),
            (TextureSlot::MetallicRoughness, Some($($b)* m.metallic_roughness_texture)),
            (TextureSlot::Normal, Some($($b)* m.normal_texture)),
            (TextureSlot::Occlusion, Some($($b)* m.occlusion_texture)),
            (TextureSlot::Emissive, Some($($b)* m.emissive_texture)),
            (TextureSlot::Specular, specular),
            (TextureSlot::SpecularColor, color),
        ]
    }};
}

impl PbrMaterial {
    /// Every texture slot that is set, with its slot, in glTF's order: base colour, metallic-roughness, normal,
    /// occlusion, emissive, then the specular extension's strength and colour.
    pub fn slots(&self) -> impl Iterator<Item = (TextureSlot, TextureRef)> {
        texture_slots!(self,)
            .into_iter()
            .filter_map(|(slot, r)| r.flatten().map(|r| (slot, r)))
    }
    /// [`PbrMaterial::slots`], each mutably.
    pub fn slots_mut(&mut self) -> impl Iterator<Item = (TextureSlot, &mut TextureRef)> {
        texture_slots!(self, &mut)
            .into_iter()
            .filter_map(|(slot, r)| r.and_then(Option::as_mut).map(|r| (slot, r)))
    }
    /// Every texture slot that is set ([`PbrMaterial::slots`] without the slots).
    pub fn textures(&self) -> impl Iterator<Item = TextureRef> {
        self.slots().map(|(_, r)| r)
    }
    /// Every texture slot that is set, mutably ([`PbrMaterial::slots_mut`] without the slots).
    pub fn textures_mut(&mut self) -> impl Iterator<Item = &mut TextureRef> {
        self.slots_mut().map(|(_, r)| r)
    }
}

#[cfg(test)]
mod tests {
    use super::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
    use crate::{Component, World};

    /// Each documented range is verified: one field out of it at a time is refused, naming it.
    #[test]
    fn verify_covers_every_documented_range() {
        let mut w = World::new();
        let e = w.spawn();
        let texture = w.spawn();
        let ok = PbrMaterial::default();
        assert_eq!(ok.verify(e, &w), Ok(()));
        let refused = |m: PbrMaterial, what: &str| {
            let err = m.verify(e, &w).unwrap_err();
            assert!(err.contains(what), "{what}: {err}");
        };
        refused(
            PbrMaterial {
                base_color: [1.5, 0.0, 0.0, 1.0],
                ..ok.clone()
            },
            "base colour",
        );
        refused(
            PbrMaterial {
                metallic: -0.1,
                ..ok.clone()
            },
            "metallic",
        );
        refused(
            PbrMaterial {
                roughness: f32::NAN,
                ..ok.clone()
            },
            "roughness",
        );
        refused(
            PbrMaterial {
                occlusion_strength: 2.0,
                ..ok.clone()
            },
            "occlusion strength",
        );
        refused(
            PbrMaterial {
                normal_scale: f32::INFINITY,
                ..ok.clone()
            },
            "normal scale",
        );
        refused(
            PbrMaterial {
                alpha_mode: AlphaMode::Mask { cutoff: -0.5 },
                ..ok.clone()
            },
            "mask cutoff",
        );
        let mut transformed = TextureRef::new(texture);
        transformed.transform = Some(UvTransform {
            rotation: f32::NAN,
            ..UvTransform::IDENTITY
        });
        refused(
            PbrMaterial {
                normal_texture: Some(transformed),
                ..ok.clone()
            },
            "texture transform",
        );
        // a negative normal scale (a flipped map) and a cutoff above 1 are allowed
        assert_eq!(
            PbrMaterial {
                normal_scale: -1.0,
                ..ok.clone()
            }
            .verify(e, &w),
            Ok(())
        );
        assert_eq!(
            PbrMaterial {
                alpha_mode: AlphaMode::Mask { cutoff: 1.5 },
                ..ok
            }
            .verify(e, &w),
            Ok(())
        );
    }

    /// Every slot, the specular extension's two included, by value and mutably, in one order.
    #[test]
    fn every_slot_is_listed_by_value_and_mutably_in_gltf_order() {
        use super::{Entity, Specular, TextureRef, TextureSlot};
        let r = |i: u32| TextureRef::new(Entity::from_index(i));
        let mut m = PbrMaterial {
            base_color_texture: Some(r(1)),
            metallic_roughness_texture: Some(r(2)),
            normal_texture: Some(r(3)),
            occlusion_texture: Some(r(4)),
            emissive_texture: Some(r(5)),
            specular: Some(Specular {
                factor: 1.0,
                texture: Some(r(6)),
                color: [1.0; 3],
                color_texture: Some(r(7)),
            }),
            ..PbrMaterial::default()
        };
        let slots: Vec<TextureSlot> = m.slots().map(|(s, _)| s).collect();
        assert_eq!(
            slots,
            [
                TextureSlot::BaseColor,
                TextureSlot::MetallicRoughness,
                TextureSlot::Normal,
                TextureSlot::Occlusion,
                TextureSlot::Emissive,
                TextureSlot::Specular,
                TextureSlot::SpecularColor
            ]
        );
        assert_eq!(
            m.textures().map(|t| t.texture.index()).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7]
        );
        for t in m.textures_mut() {
            t.texcoord = 1;
        }
        assert!(m.textures().all(|t| t.texcoord == 1));
        // an unset slot is not listed
        m.specular = None;
        m.normal_texture = None;
        assert_eq!(m.textures_mut().count(), 4);
    }

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

    /// The emissive colour is within 0-1 (glTF's `emissiveFactor`): a channel above 1, below 0 or not finite fails
    /// verification, brighter emission being the strength's; the bounds themselves pass.
    #[test]
    fn emissive_is_within_0_to_1() {
        use crate::World;

        let failures = |emissive: [f32; 3]| {
            let mut w = World::new();
            let e = w.spawn();
            w.insert(
                e,
                PbrMaterial {
                    emissive,
                    emissive_strength: 4.0,
                    ..PbrMaterial::default()
                },
            );
            w.verify_components()
                .into_iter()
                .map(|(_, _, detail)| detail)
                .collect::<Vec<_>>()
        };
        for emissive in [
            [1.5, 0.0, 0.0],
            [0.0, -0.25, 0.0],
            [0.0, 0.0, f32::NAN],
            [f32::INFINITY, 1.0, 1.0],
        ] {
            let failures = failures(emissive);
            assert_eq!(failures.len(), 1, "{emissive:?}");
            assert!(failures[0].contains("emissive"), "{failures:?}");
        }
        assert!(failures([0.0; 3]).is_empty());
        assert!(failures([1.0, 0.5, 0.0]).is_empty());
    }
}
