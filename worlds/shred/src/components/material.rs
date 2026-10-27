//! Source material banks, without destination or shading policy.
use super::Stored;
use primase_core::{Component, Entity, Geometry, JsonError, Linear, world::Linearity};
use serde::Serialize;

/// A bank material, retained as authored. No PBR conversion or render-state guesses.
pub type ShredMaterial = Stored<rme_formats::material::ShredMaterial>;

/// What a material's pixel program does, from its microcode and not its name (`rme_formats::pixel_program`).
pub type PixelProgram = Stored<rme_formats::pixel_program::PixelProgram>;

/// The banks that hold a material id and the one chosen for it, with why.
pub type MaterialResolution = Stored<rme_formats::library::MaterialResolution>;

impl Component for PixelProgram {
    const NAME: &'static str = "shred.pixel_program";
    const GEOMETRY: Geometry = Geometry::None;
    const VERSION: u32 = 1;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(&self.0).map(Some)
    }
}

impl Component for ShredMaterial {
    const NAME: &'static str = "shred.material";
    const GEOMETRY: Geometry = Geometry::None;
    const VERSION: u32 = 2;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(&self.0).map(Some)
    }
}

/// Source quartet namespace. It disambiguates ID zero before material banks load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialNamespace(pub Vec<u8>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SamplerTexture {
    pub slot: u32,
    /// None: empty binding or a path absent from the supplied source archives;
    /// the source path remains in ShredMaterial.samplers[slot].texture.
    pub texture: Option<Entity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialTextures(pub Vec<SamplerTexture>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TextureResidency {
    Streamed,
    Prefetch,
    Other,
}

/// Metadata for one packaged header/payload. Every copy remains represented;
/// `faces` hold decoded core Texture components after the reader loads them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextureCopy {
    pub archive: String,
    pub section: Option<Vec<u8>>,
    pub header: Vec<u8>,
    pub residency: TextureResidency,
    pub first_mip: u8,
    pub cube_tail: bool,
    pub faces: Vec<Entity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceTexture {
    pub path: String,
    pub copies: Vec<TextureCopy>,
    /// An index into copies. The caller explicitly supplies the preference.
    pub preferred: Option<usize>,
}

/// A packaged texture whose layout cannot yet be decoded, kept without loss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedTexture {
    pub bytes: Vec<u8>,
    pub reason: String,
}

macro_rules! material_component {
    ($ty:ty, $name:literal) => {
        impl Component for $ty {
            const NAME: &'static str = $name;
            const GEOMETRY: Geometry = Geometry::None;
            fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
                Linearity::Invariant
            }
            fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
                primase_core::json::to_value(self).map(Some)
            }
        }
    };
}
material_component!(MaterialNamespace, "shred.material_namespace");
impl Component for MaterialResolution {
    const NAME: &'static str = "shred.material_resolution";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(&self.0).map(Some)
    }
}

impl Component for MaterialTextures {
    const NAME: &'static str = "shred.material_textures";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.0.iter().filter_map(|s| s.texture));
    }
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

impl Component for SourceTexture {
    const NAME: &'static str = "shred.source_texture";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.copies.iter().flat_map(|c| c.faces.iter().copied()));
    }
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

impl Component for EncodedTexture {
    const NAME: &'static str = "shred.encoded_texture";
    const GEOMETRY: Geometry = Geometry::None;
    fn payload_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(
            serde_json::json!({"bytes":self.bytes.len(),"reason":self.reason}),
        ))
    }
}
