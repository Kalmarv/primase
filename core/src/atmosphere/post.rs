//! A tabled tone curve followed by colour correction and a three-dimensional colour lookup.
use serde::Serialize;

/// Display processing after exposure and bloom. Values describe operations, independent of a source's settings files.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToneMap {
    pub curve: ReinhardCurve,
    /// Three rows applied to (tone-mapped R, G, B, 1); negative results are clamped to zero.
    pub colour_matrix: [[f32; 4]; 3],
    /// At screen coordinate (u,v), r = length((2uv-1) * xy), t = 1-clamp(r*z+w,0,1).
    /// Multiply RGB by (t*t*(3-2*t))^4 before the lookup.
    pub vignette: [f32; 4],
    pub lut: ColourLut,
}

/// A 1024-entry R16F curve: at texel i, u=(i+.5)/1024, z=(.2*u/(1-u)*white_balance)^contrast,
/// output=(z/(1+z))^(1/gamma). Positive values are quantized towards zero to binary16.
/// HDR is sampled through the table at HDR/(HDR+.2), linearly with edge clamping.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReinhardCurve {
    pub contrast: f32,
    pub white_balance: f32,
    pub gamma: f32,
}

/// A cubic RGB lookup, red fastest, then green, then blue. Before lookup, raise RGB to `input_power`;
/// sample with trilinear filtering at texel centres, clamping to the cube's edges. The values and filtered result are
/// sRGB-encoded display RGB. Decode after filtering when writing to a target that encodes sRGB on write.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ColourLut {
    pub side: u32,
    pub values: Vec<[f32; 3]>,
    pub input_power: f32,
}

impl ToneMap {
    pub(super) fn check(&self) -> Result<(), String> {
        for (name, value) in [
            ("contrast", self.curve.contrast),
            ("white balance", self.curve.white_balance),
            ("gamma", self.curve.gamma),
            ("LUT input power", self.lut.input_power),
        ] {
            if !(value.is_finite() && value > 0.) {
                return Err(format!("tone map {name} {value} must be finite and positive"));
            }
        }
        if !self
            .colour_matrix
            .iter()
            .flatten()
            .chain(self.vignette.iter())
            .all(|x| x.is_finite())
        {
            return Err("tone map colour matrix or vignette is not finite".into());
        }
        let count = u64::from(self.lut.side).checked_pow(3);
        if self.lut.side < 2 || count != Some(self.lut.values.len() as u64) {
            return Err("tone map LUT requires side >= 2 and side cubed RGB values".into());
        }
        if !self.lut.values.iter().flatten().all(|x| x.is_finite()) {
            return Err("tone map LUT contains a nonfinite value".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Atmosphere, World};

    #[test]
    fn display_processing_survives_world_serialization_and_rejects_invalid_lut_sizes() {
        let tone = ToneMap {
            curve: ReinhardCurve {
                contrast: 1.45,
                white_balance: 1.7,
                gamma: 1.,
            },
            colour_matrix: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            vignette: [0.; 4],
            lut: ColourLut {
                side: 2,
                values: vec![[0.; 3]; 8],
                input_power: 1. / 2.2,
            },
        };
        let mut world = World::new();
        world.insert_resource(Atmosphere {
            tone_map: Some(tone.clone()),
            ..Default::default()
        });
        assert!(world.verify_resources().is_empty());
        use crate::Resource;
        let json = world
            .resource::<Atmosphere>()
            .unwrap()
            .to_json()
            .unwrap()
            .unwrap();
        assert_eq!(json["tone_map"]["lut"]["side"], 2);
        let mut invalid = tone.clone();
        invalid.lut.values.pop();
        assert_eq!(
            invalid.check().unwrap_err(),
            "tone map LUT requires side >= 2 and side cubed RGB values"
        );
        let mut invalid = tone;
        invalid.curve.gamma = 0.;
        assert!(invalid.check().unwrap_err().contains("gamma"));
    }
}
