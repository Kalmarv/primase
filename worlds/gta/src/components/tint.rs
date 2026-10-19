//! GTA's tints as the `gta-tint` pass leaves them: a material's tint palette rows ([`GtaTint`]) and the materials it
//! tinted ([`GtaTinted`]), which later lowerings read.
use std::collections::BTreeSet;

use primase_core::{Component, Entity, JsonError};
use serde::Serialize;

/// The tint row a tinted prototype is drawn with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GtaTint {
    pub row: u32,
}

impl Component for GtaTint {
    const NAME: &'static str = "gta.tint";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// The materials whose primitives `gta-tint` wrote the palette colour into (their COLOR_0 is a tint, not GTA's
/// ambient occlusion and palette index).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GtaTinted(pub BTreeSet<Entity>);

impl primase_core::Resource for GtaTinted {
    const NAME: &'static str = "gta.tinted";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
