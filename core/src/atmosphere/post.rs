//! A tabled tone curve, source-ordered colour correction and a three-dimensional colour lookup.
use serde::Serialize;

/// Display processing after exposure and bloom. Values describe operations, independent of a source's settings files.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToneMap {
    pub curve: ReinhardCurve,
    /// A source-stated transform before or after the curve. None skips this stage.
    pub colour_matrix: Option<ColourMatrix>,
    /// At screen coordinate (u,v), r = length((2uv-1) * xy), t = 1-clamp(r*z+w,0,1).
    /// Multiply RGB by (t*t*(3-2*t))^4 before the lookup. None skips this stage.
    pub vignette: Option<[f32; 4]>,
    /// None skips the lookup and leaves the curve/matrix/vignette result as linear RGB. A source must separately
    /// report any stage it cannot establish; None is not a claim that its renderer uses an identity lookup.
    pub lut: Option<ColourLut>,
}

/// Three rows applied to (R, G, B, 1), with negative results clamped to zero. Placement matters: a tone curve is
/// nonlinear, so a colour transform of HDR before the curve is a different operation from a transform after it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum ColourMatrix {
    /// Transform exposed HDR before forming the tone-table coordinates.
    BeforeCurve([[f32; 4]; 3]),
    /// Transform the tone-table result, before vignette and lookup.
    AfterCurve([[f32; 4]; 3]),
}

impl ColourMatrix {
    /// The transform's three rows, independent of its placement.
    pub fn rows(&self) -> &[[f32; 4]; 3] {
        match self {
            Self::BeforeCurve(rows) | Self::AfterCurve(rows) => rows,
        }
    }
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

impl ReinhardCurve {
    /// Evaluate the 1024 texel-centre samples as binary16 words, quantized towards zero.
    /// Uses the documented table arithmetic in binary32, including logarithm/exponent powers.
    pub fn table(&self) -> [u16; 1024] {
        std::array::from_fn(|i| {
            let u = (i as f32 + 0.5) / 1024.;
            let x = u * 0.2 / (1. - u) * self.white_balance;
            let z = (x.log2() * self.contrast).exp2();
            let mapped = z / (z + 1.);
            let value = (mapped.log2() * (1. / self.gamma)).exp2();
            let rounded = half::f16::from_f32(value);
            rounded.to_bits() - u16::from(rounded.to_f32() > value)
        })
    }
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
        ] {
            if !(value.is_finite() && value > 0.) {
                return Err(format!("tone map {name} {value} must be finite and positive"));
            }
        }
        if !self
            .colour_matrix
            .iter()
            .flat_map(|matrix| matrix.rows())
            .flatten()
            .chain(self.vignette.iter().flatten())
            .all(|x| x.is_finite())
        {
            return Err("tone map colour matrix or vignette is not finite".into());
        }
        let Some(lut) = &self.lut else { return Ok(()) };
        if !(lut.input_power.is_finite() && lut.input_power > 0.) {
            return Err("tone map LUT input power must be finite and positive".into());
        }
        let count = u64::from(lut.side).checked_pow(3);
        if lut.side < 2 || count != Some(lut.values.len() as u64) {
            return Err("tone map LUT requires side >= 2 and side cubed RGB values".into());
        }
        if !lut.values.iter().flatten().all(|x| x.is_finite()) {
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
            colour_matrix: Some(ColourMatrix::AfterCurve([
                [1., 0., 0., 0.],
                [0., 1., 0., 0.],
                [0., 0., 1., 0.],
            ])),
            vignette: Some([0.; 4]),
            lut: Some(ColourLut {
                side: 2,
                values: vec![[0.; 3]; 8],
                input_power: 1. / 2.2,
            }),
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
        assert!(json["tone_map"]["colour_matrix"]["AfterCurve"].is_array());
        let mut before = tone.clone();
        before.colour_matrix = Some(ColourMatrix::BeforeCurve(
            *tone.colour_matrix.as_ref().unwrap().rows(),
        ));
        assert!(before.check().is_ok());
        assert!(serde_json::to_value(&before).unwrap()["colour_matrix"]["BeforeCurve"].is_array());
        if let Some(ColourMatrix::BeforeCurve(rows)) = &mut before.colour_matrix {
            rows[0][0] = f32::NAN;
        }
        assert!(before.check().unwrap_err().contains("matrix"));
        let mut invalid = tone.clone();
        invalid.lut.as_mut().unwrap().values.pop();
        assert_eq!(
            invalid.check().unwrap_err(),
            "tone map LUT requires side >= 2 and side cubed RGB values"
        );
        let mut invalid = tone;
        invalid.curve.gamma = 0.;
        assert!(invalid.check().unwrap_err().contains("gamma"));
    }

    #[test]
    fn independent_absent_stages_are_preserved_without_identity_data() {
        let tone = ToneMap {
            curve: ReinhardCurve {
                contrast: 1.45,
                white_balance: 1.7,
                gamma: 1.,
            },
            colour_matrix: None,
            vignette: None,
            lut: None,
        };
        assert!(tone.check().is_ok());
        let json = serde_json::to_value(&tone).unwrap();
        for stage in ["colour_matrix", "vignette", "lut"] {
            assert!(json[stage].is_null(), "{stage}");
        }
        let mut invalid = tone;
        invalid.vignette = Some([f32::NAN; 4]);
        assert!(invalid.check().is_err());
    }
}
