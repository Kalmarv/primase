//! The sky's lighting scale in core's terms.

/// Convert the *evaluated lighting multiplier* cb1[39].x into core's log2 stops. PS_SunSkyModel (ShaderDataDX11
/// 1815) and DeferredAmbientOnly (448) multiply radiance by this value directly: it is a positive linear scale,
/// not EV and not the grading controller's property also named Exposure. This function requires an actual
/// resolved frame value; it must not be fed MinEV/MaxEV, t23, or the grading Exposure without their own mapping.
pub fn exposure_from_lighting_scale(scale: f32) -> Result<primase_core::Exposure, String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(format!(
            "cb1[39].x lighting exposure scale {scale} must be finite and positive"
        ));
    }
    Ok(primase_core::Exposure { stops: scale.log2() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_scale_is_linear_and_only_resolved_positive_values_become_stops() {
        for (scale, stops) in [(0.25, -2.0), (0.5, -1.0), (1.0, 0.0), (2.0, 1.0), (4.0, 2.0)] {
            assert_eq!(exposure_from_lighting_scale(scale).unwrap().stops, stops);
        }
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(exposure_from_lighting_scale(scale).is_err());
        }
    }
}
