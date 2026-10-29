//! Source effect bytes and level-table settings, kept independently of reader I/O.
use primase_core::{Component, Geometry};

/// The level renderer's source effect and the scale used when reading its geometry.
#[derive(Clone, Debug)]
pub struct StuntboostEffect {
    pub bytes: Option<Vec<u8>>,
    pub missing: Option<String>,
    pub metres_per_unit: f64,
    pub shading: std::collections::BTreeMap<String, String>,
}
impl Component for StuntboostEffect {
    const NAME: &'static str = "stuntboost.effect";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(serde_json::json!({
            "byte_count": self.bytes.as_ref().map(Vec::len),
            "missing": self.missing,
            "metres_per_unit": self.metres_per_unit,
            "level_shading": self.shading,
        })))
    }
    fn payload_bytes(&self) -> u64 {
        self.bytes.as_ref().map_or(0, |b| b.len() as u64)
    }
}
