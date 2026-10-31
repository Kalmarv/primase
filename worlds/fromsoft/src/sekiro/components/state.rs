//! Authored state candidates retained by the reader, and the explicit pass result.
use super::{source::DrawParam, weather, world_state};
use fromsoft_formats::{Game, emevd::Script, gparam_params::WeatherParam};
use primase_core::{Resource, json::JsonError};
use serde::Serialize;
use std::collections::BTreeMap;

/// Data families to read, without choosing a weather, story state or flag value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateReadScope {
    /// Every installed WeatherParam row, for an Elden Ring weather selection.
    pub weather: bool,
    /// The common event program and all candidate area GPARAMs, for Sekiro.
    pub common: bool,
}
impl StateReadScope {
    pub fn any(self) -> bool {
        self.weather || self.common
    }
}

/// Parsed common program and its identity. The script remains available to the pass;
/// JSON inspection reports its identity rather than duplicating the format encoding.
#[derive(Clone, Debug, Serialize)]
pub struct CommonState {
    #[serde(skip)]
    pub script: Script,
    pub script_sha256: String,
    pub script_bytes: usize,
    pub map: [u8; 2],
}

/// A dome's authored entity groups joined to its supported sky-state membership.
#[derive(Clone, Debug, Serialize)]
pub struct DomeMembership {
    pub groups: Vec<i32>,
    pub mask: u64,
}

/// All candidates in the requested state scope. No current weather or flags are stored.
#[derive(Clone, Debug, Serialize)]
pub struct StateInputs {
    pub game: Game,
    pub area: String,
    pub weather: Option<WeatherParam>,
    pub common: Option<CommonState>,
    /// None if no sky plan was read; Some(empty) retains an empty, present plan.
    pub sky_values: Option<Vec<String>>,
    pub domes: Vec<DomeMembership>,
    /// Whether the program requested draw-param data, even if its base file was unresolved.
    pub draw_param_requested: bool,
    /// Every available area file, including named read failures. No event has picked one.
    pub draw_params: BTreeMap<String, Result<DrawParam, String>>,
    /// Sum of decoded GPARAM file sizes, measured while reading the candidate set.
    pub draw_param_bytes: usize,
    /// Sum of serialized retained DrawParam values, a reproducible payload-size measure.
    pub draw_param_json_bytes: usize,
}
impl StateInputs {
    pub fn new(game: Game, area: String) -> Self {
        Self {
            game,
            area,
            weather: None,
            common: None,
            sky_values: None,
            domes: Vec::new(),
            draw_param_requested: false,
            draw_params: BTreeMap::new(),
            draw_param_bytes: 0,
            draw_param_json_bytes: 0,
        }
    }
}
impl Resource for StateInputs {
    const NAME: &'static str = "fromsoft.state_inputs";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// Selection and provenance produced by `resolve-fromsoft-state`, not by reading.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ResolvedState {
    pub weather: Option<weather::Report>,
    pub world_state: Option<world_state::Report>,
    pub sky_selection: Option<String>,
}
impl Resource for ResolvedState {
    const NAME: &'static str = "fromsoft.resolved_state";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
