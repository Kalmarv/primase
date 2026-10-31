//! Caller-supplied observations, retained with their file identity. No weather or flag value is guessed.
#![allow(
    clippy::disallowed_methods,
    reason = "reader I/O: reads the explicit observation file the caller names"
)]
use fromsoft_formats::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct WeatherSnapshot {
    pub input: crate::sekiro::weather::Snapshot,
    pub file_sha256: String,
}
impl WeatherSnapshot {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let input:crate::sekiro::weather::Snapshot=serde_json::from_slice(&bytes).map_err(|error|crate::invalid("weather observation",0,format!("weather expects {{name, source, weather, fire_ash, wetness?: {{name, source, value}}}}; weather is a WeatherParam row id and wetness is an observed weight 0..1: {error}")))?;
        input
            .validate()
            .map_err(|error| crate::invalid("weather observation", 0, error))?;
        Ok(Self {
            input,
            file_sha256: Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WorldFlags {
    pub input: crate::sekiro::world_state::FlagSet,
    pub file_sha256: String,
}
impl WorldFlags {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let input:crate::sekiro::world_state::FlagSet=serde_json::from_slice(&bytes)
            .map_err(|error|crate::invalid("world-state observation",0,format!("world_state expects {{name, source, flags}}; flags are a map of event ids to booleans: {error}")))?;
        if input.name.trim().is_empty() || input.source.trim().is_empty() {
            return Err(crate::invalid(
                "world-state observation",
                0,
                "world_state needs a nonempty snapshot name and independent source",
            ));
        }
        Ok(Self {
            input,
            file_sha256: Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        })
    }
}
