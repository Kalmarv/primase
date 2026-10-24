//! Source records whose data must not disappear between parsing and inspection.
use primase_core::{Component, Geometry, JsonError};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct AnvilLookPreset { pub index_id: u64, pub stored: crate::read::look::StoredLook }
impl Component for AnvilLookPreset {
    const NAME: &'static str = "anvil.look_preset";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        let value = match &self.stored {
            crate::read::look::StoredLook::Ambiance { season, forecast, hour, angle, flag } =>
                json!({"source_id": self.index_id, "season": season, "forecast": forecast.as_ref().map(|r| r.id),
                    "hour": hour, "angle_degrees_unresolved_role": angle, "flag_unresolved": flag,
                    "applied_to_environment": false}),
            crate::read::look::StoredLook::Forecast { values } =>
                json!({"source_id": self.index_id, "forecast_lanes": values, "lane_roles": "unresolved; name correlations are not a lowering contract",
                    "applied_to_environment": false}),
        };
        Ok(Some(value))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnvilGiDyn {
    pub index_id: u64,
    pub frame: primase_core::frame::SourceFrame,
    pub stored: crate::read::gidyn::StoredGiDyn,
}
impl Component for AnvilGiDyn {
    const NAME: &'static str = "anvil.gi_dyn";
    const GEOMETRY: Geometry = Geometry::None;
    fn payload_bytes(&self) -> u64 { self.stored.streams.iter().map(|s| s.len() as u64).sum() }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        let s = &self.stored;
        Ok(Some(json!({"source_id": self.index_id, "world_id": s.world_id, "cell_id": s.cell_id,
            "position_offset": s.position, "samples": s.samples, "probes": s.probes, "indoor": s.indoor,
            "voxel_bounds": s.voxel_bounds, "subvolume_word": s.subvolume_word,
            "encoded_stream_bytes": s.streams.each_ref().map(Vec::len), "decoded_bytes": s.decoded_bytes,
            "lighting": "probe positions decoded; irradiance and GPU GIDynData conversion unresolved"})))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceComponent {
    pub type_id: u32,
    pub id: u64,
    pub status: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnvilSourceComponents {
    pub owner: u64,
    pub components: Vec<SourceComponent>,
}
/// Every occurrence references the one source record. Selecting a later instance must not lose its source data,
/// nor keep an earlier, out-of-region occurrence just because that occurrence held the bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilSourceOwner { pub source: primase_core::Entity }
impl Component for AnvilSourceOwner {
    const NAME: &'static str = "anvil.source_owner";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<primase_core::Entity>) { out.push(self.source); }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({"source_entity": self.source.index(), "data": "anvil.source_components"})))
    }
}
impl Component for AnvilSourceComponents {
    const NAME: &'static str = "anvil.source_components";
    const GEOMETRY: Geometry = Geometry::None;
    fn payload_bytes(&self) -> u64 { self.components.iter().map(|c| c.bytes.len() as u64).sum() }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({"owner": self.owner, "components": self.components.iter().map(|c| {
            let name = crate::read::component_types::COMPONENT_TYPES.iter()
                .find(|(kind, _)| *kind == c.type_id).map(|(_, name)| *name);
            json!({"type_id": format!("{:#010x}", c.type_id), "type": name, "id": c.id,
                "status": c.status, "bytes": c.bytes})
        }).collect::<Vec<_>>() })))
    }
}
