//! WhiteRoom SpotLight records. Offsets established in riders-republic/docs/lighting.md §2. Stored intensity
//! units and shadow booleans remain source data; a lowering must state its calibration.
#![deny(clippy::arithmetic_side_effects, clippy::indexing_slicing, clippy::cast_possible_truncation)]
pub use crate::components::stored::StoredSpotLight;
pub const SPOT_LIGHT: u32 = 0x8032_0FB8;

impl StoredSpotLight {
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        if body.len() != 328 { return Err(format!("SpotLight: {} bytes, expected 328", body.len())); }
        if body.get(34..38) != Some(&0xA797_67EDu32.to_le_bytes()) {
            return Err("SpotLight: missing LightColor class at 34".into());
        }
        let float = |at: usize| -> Result<f32, String> {
            let end=at.checked_add(4).ok_or("SpotLight: float offset overflow")?;
            let value = f32::from_le_bytes(body.get(at..end).and_then(|b| b.try_into().ok())
                .ok_or("SpotLight: missing float")?);
            if value.is_finite() && value >= 0.0 { Ok(value) } else { Err(format!("SpotLight: invalid value at {at}")) }
        };
        let light = Self { color: [float(38)?, float(42)?, float(46)?, float(50)?],
            intensity: float(54)?, luminance: float(58)?, inner: float(308)?, outer: float(312)?,
            range: float(320)?, near: float(324)? };
        if light.inner > light.outer || light.outer > std::f32::consts::PI || light.range == 0.0 {
            return Err("SpotLight: invalid cone ordering or range".into());
        }
        Ok(light)
    }
}

/// The light reference is embedded in LightInstance, not its Visual's object field. Its leading 03 01 is the
/// established managed reference form here; it must not be passed to the generic null-reference decoder.
pub fn instance_reference(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() != 311 || bytes.get(12..16) != Some(&0x4AEC_3476u32.to_le_bytes()) {
        return Err("LightInstance: invalid length or BoundingVolume class".into());
    }
    let reference = bytes.get(44..54).ok_or("LightInstance: missing light reference")?;
    if reference.get(..2) != Some(&[3, 1]) { return Err("LightInstance: expected managed light reference 03 01".into()); }
    Ok(u64::from_le_bytes(reference.get(2..10).and_then(|b|b.try_into().ok()).ok_or("LightInstance: missing id")?))
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects,clippy::indexing_slicing,clippy::cast_possible_truncation,reason="fixtures")]
mod tests {
    use super::*;
    #[test]
    fn managed_light_reference_is_not_mistaken_for_a_null_material_reference() {
        let mut instance = vec![0; 311];
        instance[12..16].copy_from_slice(&0x4AEC_3476u32.to_le_bytes());
        instance[44..46].copy_from_slice(&[3, 1]);
        instance[46..54].copy_from_slice(&0x1D5_EF76_A006u64.to_le_bytes());
        assert_eq!(instance_reference(&instance).unwrap(), 0x1D5_EF76_A006);
        instance[44] = 0;
        assert!(instance_reference(&instance).is_err());
    }
    #[test]
    fn spot_fields_remain_stored_values_and_invalid_cones_are_refused() {
        let mut body = vec![0; 328];
        body[34..38].copy_from_slice(&0xA797_67EDu32.to_le_bytes());
        for (offset, value) in [(38, 1.0f32), (42, 0.891), (46, 0.560), (50, 1.0),
            (54, 5.3e9), (58, 3.4e7), (308, 0.2), (312, 0.3), (320, 20.0), (324, 0.1)] {
            body[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let stored = StoredSpotLight::parse(&body).unwrap();
        assert_eq!(stored.intensity, 5.3e9);
        assert_eq!(stored.range, 20.0);
        body[308..312].copy_from_slice(&0.4f32.to_le_bytes());
        assert!(StoredSpotLight::parse(&body).is_err());
    }
}
