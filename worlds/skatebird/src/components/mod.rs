//! The `skatebird.*` dialect: what SkateBIRD's scripts state about an object, as stored. Each is on its own entity
//! with the object's world `core.transform`, `core.name` (its GameObject's), `core.source` and, when the object is
//! stored inactive or the script disabled, a `core.condition` on the Unity level read's axes.
use primase_core::{Component, JsonError};
use primase_unity::components::UnityObject;
use serde::Serialize;

/// A `ResetPosition` script: a point the player is put back at, and whether it is where the level starts
/// (`startingPosition`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdResetPosition {
    pub starting_position: bool,
    /// The script's MonoBehaviour.
    pub behaviour: UnityObject,
    /// Its GameObject's path in the scene.
    pub path: String,
}

impl Component for SkatebirdResetPosition {
    const NAME: &'static str = "skatebird.reset_position";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A `MissionGiver` script: where a mission is given, and its title as the script stores it (its first field; `None`
/// when empty).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdMissionGiver {
    pub title: Option<String>,
    /// The script's MonoBehaviour.
    pub behaviour: UnityObject,
    /// Its GameObject's path in the scene.
    pub path: String,
}

impl Component for SkatebirdMissionGiver {
    const NAME: &'static str = "skatebird.mission_giver";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
