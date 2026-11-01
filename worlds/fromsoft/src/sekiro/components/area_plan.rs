//! Lightweight authored inputs retained before an area's heavy payload load.
use fromsoft_formats::Game;
use primase_core::{JsonError, Resource};
use serde::Serialize;

/// Every drawn-part point and available GPARAM file, without choosing a region or file.
#[derive(Clone, Debug, Serialize)]
pub struct AreaCandidates {
    pub game: Game,
    pub area: String,
    pub drawn_points: Vec<[f32; 2]>,
    pub draw_params: Vec<String>,
}
impl Resource for AreaCandidates {
    const NAME: &'static str = "fromsoft.area_candidates";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A caller's named density-preview policy applied by the registered area selection pass.
#[derive(Clone, Debug, Serialize)]
pub struct AutoRegion {
    pub rect: [f32; 4],
    pub kept: usize,
    pub parts: usize,
}
impl AutoRegion {
    /// Retained wording of the former preview report; the decision now belongs to a pass.
    pub fn why(&self) -> String {
        format!(
            "the area holds {} drawn parts, more than a read keeps whole (1500): the densest \
             150 m square, chosen by select-fromsoft-area{{region=preview}}, holds the most of them \
             ({} parts) and is what is read; state region to read another (null reads the whole area)",
            self.parts, self.kept
        )
    }
    pub fn says(&self) -> String {
        format!(
            "select-fromsoft-area{{region=preview}}'s pick: the densest 150 m square, {} of {} parts",
            self.kept, self.parts
        )
    }
}

/// Typed load scope chosen by `select-fromsoft-area`, never inferred by the payload reader.
#[derive(Clone, Debug, Serialize)]
pub struct AreaSelection {
    pub region: Option<[f32; 4]>,
    pub automatic_region: Option<AutoRegion>,
    pub draw_param: Option<String>,
    pub draw_param_choice: Option<(String, String)>,
    pub no_environment: bool,
    pub scope: primase_core::read_scope::ReadScope,
}
impl Resource for AreaSelection {
    const NAME: &'static str = "fromsoft.area_selection";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
