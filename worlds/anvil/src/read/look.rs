//! Event look presets (lighting.md §4). Decode stored values; the second preset scalar and forecast lane roles
//! are not proven sun/wind/fog inputs. Do not apply them to core.environment by a name-based guess.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
pub use crate::components::stored::StoredLook;
use crate::read::reference;
pub const AMBIANCE_PRESET: u32 = 0xE8EC_CA01;
pub const FORECAST_PRESET: u32 = 0x79B0_E782;

impl StoredLook {
    pub fn parse(type_id: u32, body: &[u8]) -> Result<Self, String> {
        if body.first() != Some(&1) {
            return Err("look preset: unknown managed header".into());
        }
        let float = |at: usize| -> Result<f32, String> {
            let end = at.checked_add(4).ok_or("look preset: scalar offset overflow")?;
            let value = f32::from_le_bytes(
                body.get(at..end)
                    .and_then(|b| b.try_into().ok())
                    .ok_or("look preset: truncated scalar")?,
            );
            if !value.is_finite() {
                return Err(format!("look preset: nonfinite scalar at {at}"));
            }
            Ok(value)
        };
        match (type_id, body.len()) {
            (AMBIANCE_PRESET, 29) if body.get(1) == Some(&0) => {
                let season = u64::from_le_bytes(
                    body.get(2..10)
                        .and_then(|b| b.try_into().ok())
                        .ok_or("look preset: season reference")?,
                );
                let forecast = reference::parse(
                    body.get(10..20)
                        .and_then(|b| b.try_into().ok())
                        .ok_or("look preset: forecast reference")?,
                )
                .map_err(|e| e.to_string())?;
                let hour = float(20)?;
                let angle = float(24)?;
                if !(0.0..24.0).contains(&hour) {
                    return Err(format!("look preset hour {hour} outside [0,24)"));
                }
                if !(0.0..=360.0).contains(&angle) {
                    return Err(format!("look preset angle {angle} outside [0,360]"));
                }
                Ok(Self::Ambiance {
                    season,
                    forecast,
                    hour,
                    angle,
                    flag: *body.get(28).ok_or("look preset: missing flag")?,
                })
            }
            (FORECAST_PRESET, 49) => {
                let mut values = [0.; 12];
                for (index, value) in values.iter_mut().enumerate() {
                    let at = index
                        .checked_mul(4)
                        .and_then(|n| n.checked_add(1))
                        .ok_or("look preset: lane offset overflow")?;
                    *value = float(at)?;
                }
                Ok(Self::Forecast { values })
            }
            _ => Err(format!(
                "look preset: unsupported type {type_id:#010x}, {}-byte profile",
                body.len()
            )),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures"
)]
mod tests {
    use super::*;
    #[test]
    fn references_hour_and_unknown_angle_are_kept_separate_and_lengths_are_exact() {
        let mut body = vec![1, 0];
        body.extend(17u64.to_le_bytes());
        body.extend([3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        body.extend(7f32.to_le_bytes());
        body.extend(135f32.to_le_bytes());
        body.push(0);
        assert!(matches!(
            StoredLook::parse(AMBIANCE_PRESET, &body),
            Ok(StoredLook::Ambiance {
                season: 17,
                forecast: None,
                hour: 7.0,
                angle: 135.0,
                flag: 0
            })
        ));
        body.push(0);
        assert!(StoredLook::parse(AMBIANCE_PRESET, &body).is_err());
        let mut forecast = vec![1];
        forecast.extend([0; 48]);
        assert!(
            matches!(StoredLook::parse(FORECAST_PRESET, &forecast), Ok(StoredLook::Forecast { values }) if values == [0.; 12])
        );
    }
}
