//! GTA's collision as the reader keeps it: map bounds ([`GtaBounds`]), the bound a prototype's asset embeds
//! ([`GtaEmbeddedBounds`]) and the install's collision material names ([`GtaMaterials`]), as stored; the passes
//! (`crate::passes::collision`) make them `core.collision`.
use primase_core::{Component, JsonError};

use rage_engine_formats::bounds::Bound;

/// One ybn's bound as stored (GTA's frame, metres, world space for map bounds), its file and its detail.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaBounds {
    pub path: String,
    /// The file name's prefix before `@` (`hi`, `ma`: the game's finer copies), lowercase; `None` for plain bounds.
    pub detail: Option<String>,
    pub bound: Bound,
}

impl Component for GtaBounds {
    const NAME: &'static str = "gta.bounds";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: `detail` added.
    const VERSION: u32 = 2;
    /// Bulk data (a bound's every vertex and polygon): not shown.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(None)
    }
}

/// The collision bound a prototype's asset embeds, as stored (GTA's frame, metres, the asset's own space): a
/// fragment's first physics LOD's, or a drawable's own; and the asset's file.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaEmbeddedBounds {
    pub path: String,
    pub bound: Bound,
}

impl Component for GtaEmbeddedBounds {
    const NAME: &'static str = "gta.embedded_bounds";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// Bulk data (a bound's every vertex and polygon): not shown.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(None)
    }
}

/// Entity flag bit 2: the placement does not collide by its asset's embedded bound.
pub const EMBEDDED_COLLISION_DISABLED: u32 = 1 << 2;

/// The install's collision material names (`materials.dat`), by material index.
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize)]
pub struct GtaMaterials(pub Vec<String>);

impl primase_core::Resource for GtaMaterials {
    const NAME: &'static str = "gta.materials";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
