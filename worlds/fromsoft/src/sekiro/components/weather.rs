//! Named observed weather inputs and the state a selection pass resolves.
use primase_core::{Resource, World};
use serde::{Deserialize, Serialize};

/// An independently observed current weather and fire-ash state. No hour selects these.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub name: String,
    pub source: String,
    pub weather: i32,
    pub fire_ash: bool,
    /// Independently observed WeatherBase weight. Absent means unknown, never dry.
    #[serde(default)]
    pub wetness: Option<Wetness>,
}

/// Named effective wetting state, not a value computed from a weather row or an hour.
/// The native WeatherBase getter returns the accumulated wetting weight (or its explicit override).
/// Reading this state does not yet apply WeatherBase to the environment or surface materials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wetness {
    pub name: String,
    pub source: String,
    pub value: f32,
}

impl Wetness {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() || self.source.trim().is_empty() {
            return Err("wetness requires a nonempty name and independent source".into());
        }
        if !self.value.is_finite() || !(0.0..=1.0).contains(&self.value) {
            return Err("wetness requires a finite observed WeatherBase weight from 0 to 1".into());
        }
        Ok(())
    }
}

impl Resource for Wetness {
    const NAME: &'static str = "fromsoft.wetness";

    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }

    fn verify(&self, _world: &World) -> std::result::Result<(), String> {
        self.validate()
    }
}

/// The installed row and sky axis selected by a snapshot. Layer composition remains unresolved.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub input: Snapshot,
    pub sky: String,
    pub weather_outdoor: String,
}

impl Snapshot {
    /// Validate provenance before opening game data.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() || self.source.trim().is_empty() || self.weather < 0 {
            return Err("weather requires a nonempty name and independent source and a nonnegative WeatherParam row id".into());
        }
        if let Some(wetness) = &self.wetness {
            wetness.validate()?;
        }
        Ok(())
    }
}
