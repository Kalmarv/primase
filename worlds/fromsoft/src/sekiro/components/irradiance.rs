//! Sekiro's authored environment boxes and their coefficient-package identities.
use primase_core::{Component, Entity, Geometry, JsonError};
use serde::Serialize;
use serde_json::Value;

/// The complete source region and the four textures its name joins, without choosing an hour.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IrradianceRegion {
    pub area: String,
    pub package: u8,
    pub region: fromsoft_formats::msb::Region,
    pub coefficients: [Entity; 4],
    pub display_distance: f32,
    pub fade_distance: f32,
    pub priority: u8,
    pub diffuse_intensity: f32,
    pub specular_intensity: f32,
}
impl Component for IrradianceRegion {
    const NAME: &'static str = "fromsoft.irradiance_region";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.coefficients);
    }
}
