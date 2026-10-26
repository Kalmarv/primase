//! Source material banks, without destination or shading policy.
use primase_core::{Component, Entity, Geometry, JsonError, Linear, world::Linearity};
use serde::Serialize;

/// IDs are local to a bank: ID zero occurs in all three banks.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct MaterialKey {
    pub bank: String,
    pub id: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaterialParameter {
    pub hash: u32,
    /// Only named by an exact sdbm join against this material's fragment program.
    pub name: Option<String>,
    /// Original descriptor word. The high bits' semantics remain unknown.
    pub source_encoding: u32,
    /// Four stored f32 values, including unused lanes, in source order.
    pub value: [f32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialSampler {
    /// Descriptor ordinal; no claim that this is a hardware texture unit.
    pub slot: u32,
    pub hash: u32,
    /// Exact sdbm join against an engine string; not Cg reflection or inferred use.
    pub name: Option<String>,
    pub name_source: Option<SamplerNameSource>,
    /// Empty is a source empty binding, not a generated default texture.
    pub texture: String,
    /// Descriptor +0, +4, +8, +12, +20. State semantics are not yet decoded.
    pub unknown_state: [u32; 5],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SamplerNameSource {
    ExecutableStringSdbm,
}

/// A bank material, retained as authored. No PBR conversion or render-state guesses.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShredMaterial {
    pub key: MaterialKey,
    /// Header +0x1c, +0x28, +0x2c, +0x3c, respectively.
    pub unknown_header: [u32; 4],
    pub vertex_shader: String,
    pub fragment_shader: String,
    pub parameters: Vec<MaterialParameter>,
    pub samplers: Vec<MaterialSampler>,
}

impl Component for ShredMaterial {
    const NAME: &'static str = "shred.material";
    const GEOMETRY: Geometry = Geometry::None;
    const VERSION: u32 = 2;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// Source quartet namespace. It disambiguates ID zero before material banks load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialNamespace(pub Vec<u8>);

/// Why a bank was selected. Namespace inference is supported by every nonzero
/// material reference on the disc, but is not a native-loader observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum BankEvidence {
    UniqueId,
    InferredQuartetNamespace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialResolution {
    pub candidates: Vec<MaterialKey>,
    pub selected: Option<MaterialKey>,
    pub evidence: Option<BankEvidence>,
}

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
material_component!(MaterialResolution, "shred.material_resolution");

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
