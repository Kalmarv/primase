//! Source material banks, without destination or shading policy.
use primase_core::{Component, Geometry, JsonError};
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
    /// Current fragment programs contain no sampler names.
    pub name: Option<String>,
    /// Empty is a source empty binding, not a generated default texture.
    pub texture: String,
    /// Descriptor +0, +4, +8, +12, +20. State semantics are not yet decoded.
    pub unknown_state: [u32; 5],
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
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
