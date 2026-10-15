//! The `pbr.*` dialect: a baked metal-rough material, glTF 2.0's material model. The common lowering target for the
//! glTF, USD and skate writers: a pass lowers a game's material (`skate.material`, `fromsoft.layered_material`,
//! `gta.*`) to a [`PbrMaterial`] on the same material entity, which keeps its dialect components.
//!
//! Meanings follow glTF 2.0 §3.9 exactly, so the glTF writer is a direct mapping: factors are linear, colour textures
//! sRGB, packed maps linear, and [`Default`] is glTF's default material.

use serde::Serialize;
use serde_json::Value;

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

/// A texture used by a material slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TextureRef {
    /// The texture entity (`core.texture` or `core.texture_file`).
    pub texture: Entity,
    /// Which `TexCoord(n)` stream it samples.
    pub texcoord: u8,
    /// Wrap along u, then v.
    pub wrap: [Wrap; 2],
}

impl TextureRef {
    /// Sampling `TexCoord(0)` with repeat wrapping.
    pub fn new(texture: Entity) -> Self {
        TextureRef {
            texture,
            texcoord: 0,
            wrap: [Wrap::Repeat; 2],
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
    pub alpha_mode: AlphaMode,
    /// Back faces drawn too (and lit with the flipped normal).
    pub double_sided: bool,
}

impl Component for PbrMaterial {
    const NAME: &'static str = "pbr.material";
    fn to_json(&self) -> Option<Value> {
        serde_json::to_value(self).ok()
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
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

impl PbrMaterial {
    /// Every texture slot that is set, in glTF's order: base colour, metallic-roughness, normal, occlusion, emissive.
    pub fn textures(&self) -> impl Iterator<Item = TextureRef> {
        [
            self.base_color_texture,
            self.metallic_roughness_texture,
            self.normal_texture,
            self.occlusion_texture,
            self.emissive_texture,
        ]
        .into_iter()
        .flatten()
    }
}
