use super::Stored;
use primase_core::{JsonError, Resource};
use rme_formats::level_params;

/// The parameters one level's `vfx_level_load_<level>` script sets (fog, shadow, bloom, back light, LUT, ...), as the
/// script's calls in order. Nothing here says what a call's arguments mean or what unit they are in; that is the
/// engine function's, and only the proven ones are lowered. Not a core `Environment` or `Atmosphere`.
pub type LevelParams = Stored<level_params::LevelParams>;

impl Resource for LevelParams {
    const NAME: &'static str = "shred.level_params";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(&self.0).map(Some)
    }
}
