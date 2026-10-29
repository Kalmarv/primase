//! The unit the world's lights were lowered from (`core.light_unit`, a world resource).

use serde::Serialize;
use serde_json::Value;

use crate::{JsonError, Resource, World};

/// What one unit of the source's own value of a point or spot light is in candela: the calibration the lowering that
/// made the world's point and spot `core.light`s chose (a directional light's intensity, in lux, is not in it). Such a
/// light's intensity is its source value times `candela_per_unit`, so `colour * intensity / candela_per_unit` is the
/// value the source stored, the one a program of the source reads its lights in ([`crate::shader::LightValue::Colour`]). The lowering that makes the lights writes it; a world whose lights no
/// lowering made (a scene read in candela) has none, and a light table's colours have no value there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LightUnit {
    /// Candela per unit of the source's light value; finite and above 0.
    pub candela_per_unit: f32,
}

impl Resource for LightUnit {
    const NAME: &'static str = "core.light_unit";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// A finite factor above 0: a light of the source's value 1 has some intensity.
    fn verify(&self, _w: &World) -> Result<(), String> {
        if !(self.candela_per_unit.is_finite() && self.candela_per_unit > 0.0) {
            return Err(format!(
                "candela per unit {} is not finite and above 0",
                self.candela_per_unit
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A factor of 0, a negative one or a non-finite one is refused; any finite one above 0 is kept.
    #[test]
    fn the_unit_is_a_finite_factor_above_zero() {
        let w = World::new();
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                LightUnit {
                    candela_per_unit: bad
                }
                .verify(&w)
                .is_err(),
                "{bad}"
            );
        }
        for good in [1.0, 0.001, 25.0] {
            assert_eq!(
                LightUnit {
                    candela_per_unit: good
                }
                .verify(&w),
                Ok(())
            );
        }
    }
}
