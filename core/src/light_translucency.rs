//! Source-stated parameters of a light's transmission/back-light contribution.
use crate::{Component, Entity, Geometry, JsonError, World};
use serde::Serialize;

/// A light shader's transmission contribution: an amplitude and a multiplier of its angular exponent.
/// The source shader defines the angular function and reference exponent. These parameters neither select a
/// particular lighting model nor change opaque material transmission. Attached to the same entity as [`crate::Light`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LightTranslucency {
    pub strength: f32,
    pub exponent_scale: f32,
}
impl Component for LightTranslucency {
    const NAME: &'static str = "core.light_translucency";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if !self.strength.is_finite()
            || self.strength < 0.
            || !self.exponent_scale.is_finite()
            || self.exponent_scale <= 0.
        {
            return Err(
                "light translucency requires finite nonnegative strength and positive exponent scale".into(),
            );
        }
        Ok(())
    }
}
