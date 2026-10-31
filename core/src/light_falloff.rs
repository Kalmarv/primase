//! A source-stated distance used by a light's shader model, apart from its emitter radius and range.
use serde::Serialize;

use crate::{Component, Entity, Geometry, JsonError, World};

/// The normalization distance of a punctual light's source-defined falloff: a source program evaluates its
/// distance-dependent diffuse wrap and specular width using `distance / self.distance`. This states the input,
/// not an attenuation function; the source shader defines the function. It is neither the emitter's physical radius
/// nor the range where lighting stops. Attached to the same entity as [`crate::Light`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LightFalloff {
    /// A finite positive distance in the world's length unit, with a finite f32 reciprocal.
    pub distance: f32,
}

impl Component for LightFalloff {
    const NAME: &'static str = "core.light_falloff";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        self.inverse_distance().map(|_| ())
    }
}

impl LightFalloff {
    /// The source-program input; invalid distances are refused rather than clamped or replaced.
    pub fn inverse_distance(self) -> Result<f32, String> {
        if !(self.distance.is_finite() && self.distance > 0. && self.distance.recip().is_finite()) {
            return Err(format!(
                "light falloff distance {} is not positive with a finite reciprocal",
                self.distance
            ));
        }
        Ok(self.distance.recip())
    }
}
