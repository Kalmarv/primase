//! LayeredSky's authored scattering keyframes. The two resource profiles are from the registry-based read in
//! riders-republic/docs/sky.md §2; unknown profiles fail rather than using guessed offsets.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
pub use crate::components::stored::{SkyKeyframe, StoredSky};
pub const LAYERED_SKY: u32 = 0x24D3_2CF8;
pub const WORLD_AMBIANCE: u32 = 0x21C2_D472;
const KEYFRAME: u32 = 0x5CDF_6DE9;
const FRAME_BYTES: usize = 143_422;

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

fn word(body: &[u8], at: usize) -> Result<u32, String> {
    let end = at.checked_add(4).ok_or("sky: word offset overflow")?;
    body.get(at..end)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| format!("sky: missing word at {at}"))
}
impl StoredSky {
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        let (start, count): (usize, usize) = match body.len() {
            3_442_710 => (582, 24),
            143_994 => (572, 1),
            length => return Err(format!("LayeredSky: unsupported {length}-byte profile")),
        };
        if usize::try_from(word(
            body,
            start.checked_sub(4).ok_or("sky: frame count offset")?,
        )?)
        .map_err(|_| "sky: frame count overflow")?
            != count
        {
            return Err("LayeredSky: keyframe count differs from the established profile".into());
        }
        let mut keyframes = Vec::with_capacity(count);
        for index in 0..count {
            let begin = index
                .checked_mul(FRAME_BYTES)
                .and_then(|n| start.checked_add(n))
                .ok_or("sky: keyframe offset overflow")?;
            let end = begin
                .checked_add(FRAME_BYTES)
                .ok_or("sky: keyframe end overflow")?;
            let frame = body.get(begin..end).ok_or("LayeredSky: short keyframe")?;
            if word(frame, 8)? != KEYFRAME {
                return Err(format!("LayeredSky: keyframe {index} has another class"));
            }
            // ManagedObject's byte follows the object header, then TimeOfDay, CRC, five dimensions and a boolean.
            let hour = f32::from_bits(word(frame, 13)?);
            if !hour.is_finite() || !(0.0..24.0).contains(&hour) {
                return Err(format!("LayeredSky: invalid keyframe hour {hour}"));
            }
            let dimensions: Vec<_> = [21, 25, 29, 33, 37]
                .map(|at| word(frame, at))
                .into_iter()
                .collect::<Result<_, _>>()?;
            if dimensions != [256, 64, 32, 32, 16] {
                return Err(format!("LayeredSky: unknown keyframe dimensions {dimensions:?}"));
            }
            if frame.get(41) != Some(&1) {
                return Err("LayeredSky: scattering data is not generated".into());
            }
            let mut at = 42usize;
            let mut buffer = |elements: usize, stride: usize| -> Result<Vec<u8>, String> {
                if usize::try_from(word(frame, at)?).map_err(|_| "sky: buffer count overflow")? != elements {
                    return Err(format!("LayeredSky: buffer count at {at}"));
                }
                at = at.checked_add(4).ok_or("sky: buffer offset overflow")?;
                let end = elements
                    .checked_mul(stride)
                    .and_then(|n| at.checked_add(n))
                    .ok_or("sky: buffer end overflow")?;
                let bytes = frame
                    .get(at..end)
                    .ok_or("LayeredSky: buffer runs past its keyframe")?
                    .to_vec();
                at = end;
                Ok(bytes)
            };
            let key = SkyKeyframe {
                hour,
                crc: word(frame, 17)?,
                inscatter: buffer(65_536, 2)?,
                optical_depth: buffer(1_024, 2)?,
                irradiance: buffer(768, 4)?,
                sun_depth: buffer(768, 4)?,
                sun_irradiance: buffer(1_024, 4)?,
            };
            if at != frame.len() {
                return Err("LayeredSky: trailing keyframe bytes".into());
            }
            keyframes.push(key);
        }
        let mie_at = if start == 582 {
            [350, 354, 358]
        } else {
            [340, 344, 348]
        };
        Ok(Self {
            sun_position_mode: word(body, 12)?,
            sun_azimuth: f32::from_bits(word(body, 16)?),
            sun_tilt: f32::from_bits(word(body, 20)?),
            sun_elevation: f32::from_bits(word(body, 24)?),
            sun_intensity: f32::from_bits(word(body, 64)?),
            mie: mie_at
                .map(|at| word(body, at).map(f32::from_bits))
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| "LayeredSky: Mie length")?,
            sun_color: [48, 52, 56, 60]
                .map(|at| word(body, at).map(f32::from_bits))
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| "LayeredSky: color length")?,
            keyframes,
        })
    }

    /// Static temporal interpolation between authored scattering tables, including the 23:00→00:00 interval.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "finite floating point interpolation of authored tables, not byte addressing"
    )]
    pub fn radiance(
        &self,
        hour: f32,
        direction: [f32; 3],
        sun: [f32; 3],
        mie: [f32; 3],
    ) -> Result<[f32; 3], String> {
        if !hour.is_finite() || self.keyframes.is_empty() {
            return Err("sky: no finite hour/generated keyframes".into());
        }
        let hour = hour.rem_euclid(24.0);
        let mut keys: Vec<_> = self.keyframes.iter().collect();
        keys.sort_by(|a, b| a.hour.total_cmp(&b.hour));
        let last = keys.len().checked_sub(1).ok_or("sky: no keyframes")?;
        let i = keys.iter().rposition(|k| k.hour <= hour).unwrap_or(last);
        let j = if i == last {
            0
        } else {
            i.checked_add(1).ok_or("sky: key index overflow")?
        };
        let left = keys.get(i).ok_or("sky: missing left key")?;
        let right = keys.get(j).ok_or("sky: missing right key")?;
        let a = left.radiance(direction, sun, mie)?;
        if i == j {
            return Ok(a);
        }
        let start = left.hour;
        let end = right.hour + if j == 0 { 24.0 } else { 0.0 };
        let at = hour + if hour < start { 24.0 } else { 0.0 };
        let f = (at - start) / (end - start);
        let b = right.radiance(direction, sun, mie)?;
        let mut out = [0.0; 3];
        for ((v, a), b) in out.iter_mut().zip(a).zip(b) {
            *v = a + (b - a) * f;
        }
        Ok(out)
    }
}

/// IEEE-754 half to f32. No clamping to LDR: the scattering tables carry scene radiance.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "IEEE-754 half conversion uses bounded exponent and floating point arithmetic"
)]
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (bits >> 10) & 31;
    let mantissa = bits & 1023;
    match exponent {
        0 => sign * f32::from(mantissa) * 2.0f32.powi(-24),
        31 => {
            if mantissa == 0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + f32::from(mantissa) / 1024.0) * 2.0f32.powi(i32::from(exponent) - 15),
    }
}

impl SkyKeyframe {
    /// Static clear-sky slice of the stored single-scattering model (sky.md §6). Caller supplies the sun and Mie
    /// values; the weather/time controllers, clouds, moon, Milky Way, exposure and stars are separate engine inputs.
    #[allow(
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        reason = "numeric 32x32x16 sampling stencil: coordinates clamped to fixed arrays; byte accesses use checked ranges below"
    )]
    pub fn radiance(&self, direction: [f32; 3], sun: [f32; 3], mie: [f32; 3]) -> Result<[f32; 3], String> {
        let normalize = |v: [f32; 3]| -> Result<[f32; 3], String> {
            let length = v.iter().map(|v| v * v).sum::<f32>().sqrt();
            if !length.is_finite() || length <= 0.0 {
                return Err("sky: invalid direction".into());
            }
            Ok(v.map(|v| v / length))
        };
        let direction = normalize(direction)?;
        let sun = normalize(sun)?;
        if mie.iter().any(|v| !v.is_finite())
            || mie[0].abs() >= 1.0
            || mie[1].abs() >= 1.0
            || !(0.0..=1.0).contains(&mie[2])
        {
            return Err("sky: invalid Mie phase settings".into());
        }
        let cosine = direction
            .iter()
            .zip(sun)
            .map(|(d, s)| d * s)
            .sum::<f32>()
            .clamp(-1.0, 1.0);
        let radius = 6360.19f32;
        let distance = -radius * direction[2]
            + (radius * radius * (direction[2] * direction[2] - 1.0) + (radius + 60.0) * (radius + 60.0))
                .max(0.0)
                .sqrt();
        let low = (3.65587f32 * -0.34202).atan();
        let high = 3.65587f32.atan();
        let uvw = [
            0.015625 + ((3.65587 * sun[2].max(-0.34202)).atan() - low) / (high - low) * 0.96875,
            0.001047 * distance + 0.015625,
            (cosine + 1.0) * 0.46875 + 0.03125,
        ];
        let sizes = [32usize, 32, 16];
        let coordinates = std::array::from_fn::<_, 3, _>(|axis| {
            let value = (uvw[axis] * sizes[axis] as f32 - 0.5).clamp(0.0, (sizes[axis] - 1) as f32);
            (value.floor() as usize, value - value.floor())
        });
        let mut encoded = [0.0f32; 4];
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    let offsets = [x, y, z];
                    let weight: f32 = (0..3)
                        .map(|a| {
                            if offsets[a] == 0 {
                                1.0 - coordinates[a].1
                            } else {
                                coordinates[a].1
                            }
                        })
                        .product();
                    let p =
                        std::array::from_fn::<_, 3, _>(|a| (coordinates[a].0 + offsets[a]).min(sizes[a] - 1));
                    let at = ((p[2] * 32 + p[1]) * 32 + p[0]) * 8;
                    for (lane, value) in encoded.iter_mut().enumerate() {
                        let pair = self
                            .inscatter
                            .get(at + lane * 2..at + lane * 2 + 2)
                            .and_then(|b| b.try_into().ok())
                            .ok_or("sky: missing scattering texel")?;
                        *value += half(u16::from_le_bytes(pair)) * weight;
                    }
                }
            }
        }
        let decoded = encoded.map(|value| value.powi(4));
        let pi = std::f32::consts::PI;
        let rayleigh = 3.0 / (16.0 * pi) * (1.0 + cosine * cosine);
        let phase = |g: f32| {
            3.0 / (8.0 * pi) * (1.0 - g * g) * (1.0 + cosine * cosine)
                / ((2.0 + g * g) * (1.0 + g * g - 2.0 * g * cosine).powf(1.5))
        };
        let mie_phase = phase(mie[0]) * (1.0 - mie[2]) + phase(mie[1]) * mie[2];
        let ratio = [1.0, 0.4296, 0.1752];
        let result = std::array::from_fn(|lane| {
            decoded[lane] * rayleigh
                + decoded[lane] * decoded[3] / decoded[0].max(0.0001) * ratio[lane] * mie_phase
        });
        if result.iter().any(|v| !v.is_finite()) {
            return Err("sky: non-finite radiance".into());
        }
        Ok(result)
    }
}

/// WorldAmbiance's managed references at registry-established offsets. No byte scan for coincidental ClassIDs.
pub fn ambiance(body: &[u8]) -> Result<(u64, u64), String> {
    if body.len() != 5277 {
        return Err(format!("WorldAmbiance: unsupported {}-byte profile", body.len()));
    }
    let reference = |at: usize| -> Result<u64, String> {
        let end = at
            .checked_add(10)
            .ok_or("WorldAmbiance: reference offset overflow")?;
        let bytes: [u8; 10] = body
            .get(at..end)
            .and_then(|b| b.try_into().ok())
            .ok_or("WorldAmbiance: missing reference")?;
        let [tag, mode, a, b, c, d, e, f, g, h] = bytes;
        if !matches!(tag, 1 | 3) || mode != 1 {
            return Err(format!("WorldAmbiance: unsupported reference at {at}"));
        }
        Ok(u64::from_le_bytes([a, b, c, d, e, f, g, h]))
    };
    Ok((reference(3)?, reference(4108)?))
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
    fn lighting_scale_is_linear_and_only_resolved_positive_values_become_stops() {
        for (scale, stops) in [(0.25, -2.0), (0.5, -1.0), (1.0, 0.0), (2.0, 1.0), (4.0, 2.0)] {
            assert_eq!(exposure_from_lighting_scale(scale).unwrap().stops, stops);
        }
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(exposure_from_lighting_scale(scale).is_err());
        }
    }
    #[test]
    fn half_values_preserve_hdr_and_sign_and_bad_radiance_inputs_are_refused() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0x4000), 2.0);
        assert_eq!(half(0xbc00), -1.0);
        let key = SkyKeyframe {
            hour: 12.0,
            crc: 0,
            inscatter: vec![0; 131_072],
            optical_depth: vec![],
            irradiance: vec![],
            sun_depth: vec![],
            sun_irradiance: vec![],
        };
        assert_eq!(
            key.radiance([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.5, 0.99, 0.1])
                .unwrap(),
            [0.0; 3]
        );
        assert!(key.radiance([0.0; 3], [1.0, 0.0, 0.0], [0.5, 0.99, 0.1]).is_err());
        assert!(
            key.radiance([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [1.0, 0.99, 0.1])
                .is_err()
        );
    }
    #[test]
    fn the_small_sky_profile_accounts_for_every_buffer_and_rejects_a_wrong_count() {
        let mut body = vec![0; 143_994];
        body[568..572].copy_from_slice(&1u32.to_le_bytes());
        let frame = &mut body[572..];
        frame[8..12].copy_from_slice(&KEYFRAME.to_le_bytes());
        frame[12] = 1;
        frame[13..17].copy_from_slice(&12f32.to_le_bytes());
        for (index, size) in [256u32, 64, 32, 32, 16].into_iter().enumerate() {
            frame[21 + index * 4..25 + index * 4].copy_from_slice(&size.to_le_bytes());
        }
        frame[41] = 1;
        let mut at = 42;
        for (count, stride) in [(65_536u32, 2), (1_024, 2), (768, 4), (768, 4), (1_024, 4)] {
            frame[at..at + 4].copy_from_slice(&count.to_le_bytes());
            at += 4 + count as usize * stride;
        }
        assert_eq!(at, FRAME_BYTES);
        assert_eq!(StoredSky::parse(&body).unwrap().keyframes[0].hour, 12.0);
        body[572 + 42] = 1;
        assert!(StoredSky::parse(&body).is_err());
    }
}
