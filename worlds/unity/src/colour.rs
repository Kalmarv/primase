//! What a Unity player's programs read of a stored colour, as Unity sets it when it draws.
//!
//! A Material stores each colour property as the inspector shows it (gamma-encoded). A player in the linear colour
//! space converts it to linear before its programs read it ([`gamma_to_linear`], Unity's `GammaToLinearSpace`), except
//! a property its shader declares `[HDR]`, whose stored value the programs read as it is. A player in the gamma colour
//! space converts nothing. Measured in a Linear project of the Unity 6000.6.4f1 editor
//! (`primase-oracles/unity/colour-space`): a probe shader's plain colour property reads `GammaToLinearSpace` of the
//! value set (`Mathf.GammaToLinearSpace` and `Color.linear` give the same), its `[HDR]` one the value set; the players
//! read are 2019.4.20f1 and 6000.3 builds, not measured.

use primase_core::pass::Choice;

/// The colour space a player's programs work in (its PlayerSettings' `m_ActiveColorSpace`,
/// [`unity_formats::player::Player::colour_space`]); the option `colour-space` of the passes that read colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColourSpace {
    Linear,
    Gamma,
}

impl Choice for ColourSpace {
    const VALUES: &'static [(&'static str, Self)] =
        &[("linear", ColourSpace::Linear), ("gamma", ColourSpace::Gamma)];
}

impl From<unity_formats::player::ColourSpace> for ColourSpace {
    fn from(c: unity_formats::player::ColourSpace) -> Self {
        match c {
            unity_formats::player::ColourSpace::Linear => ColourSpace::Linear,
            unity_formats::player::ColourSpace::Gamma => ColourSpace::Gamma,
        }
    }
}

/// Unity's `GammaToLinearSpace` of one stored channel: the sRGB curve below 1 and `pow(v, 2.2)` from 1 up
/// (`GammaToLinearSpaceExact`, `CGIncludes/UnityCG.cginc`:95 of the 6000.6.4f1 editor; the oracle's measured values
/// match it on both sides of 1). In f64, rounded once to f32.
pub fn gamma_to_linear(v: f32) -> f32 {
    let v = f64::from(v);
    let linear = if v <= 0.04045 {
        v / 12.92
    } else if v < 1.0 {
        ((v + 0.055) / 1.055).powf(2.4)
    } else {
        v.powf(2.2)
    };
    linear as f32
}

/// The colour a player in `space` hands its programs for stored RGBA `stored` of a property declared `[HDR]` (`hdr`)
/// or not: in the linear space a plain property's RGB through [`gamma_to_linear`], an HDR one's as stored; in the gamma
/// space as stored. Alpha is never converted.
pub fn as_programs_read(stored: [f32; 4], hdr: bool, space: ColourSpace) -> [f32; 4] {
    match space {
        ColourSpace::Linear if !hdr => [
            gamma_to_linear(stored[0]),
            gamma_to_linear(stored[1]),
            gamma_to_linear(stored[2]),
            stored[3],
        ],
        _ => stored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The oracle's readings (`primase-oracles/unity/colour-space/baselines/colour_space_6000.6.4f1.json`): stored value,
    /// what the probe shader's plain colour property read (`Mathf.GammaToLinearSpace` and `Color.linear` agree), in a
    /// Linear project. The values past 1 are PEAK's stored HDR channels (M_Rope, M_Lava, M_Rock_Crystal).
    const MEASURED: [(f32, f32); 11] = [
        (0.02, 0.001_547_987_6),
        (0.5, 0.214_041_14),
        (0.866_145_3, 0.722_074_03),
        (0.999, 0.997_726_86),
        (1.0, 1.0),
        (1.001, 1.002_201_4),
        (1.827_499_2, 3.767_782_5),
        (2.0, 4.594_794),
        (3.031_433, 11.471_643),
        (4.083_006, 22.087_982),
        (11.984_314, 236.020_13),
    ];

    /// Unity's curve on both sides of 1, to the oracle's float (within an f32 rounding of the power).
    #[test]
    fn gamma_to_linear_is_unity_s_on_both_sides_of_one() {
        for (stored, read) in MEASURED {
            let got = gamma_to_linear(stored);
            assert!(
                (got - read).abs() <= read * 2e-7,
                "{stored}: {got} for Unity's {read}"
            );
        }
        // the sRGB curve past 1 is not Unity's: PEAK's stored 11.984314 would be 344.86 by it
        let srgb = primase_foundations::color::srgb_to_linear(11.984_314) as f32;
        assert!((srgb - 344.856).abs() < 0.01 && (gamma_to_linear(11.984_314) - srgb).abs() > 100.0);
    }

    /// The oracle's `[HDR]` property read every value as set, alpha too; a gamma player converts nothing.
    #[test]
    fn an_hdr_colour_and_a_gamma_player_s_colour_are_read_as_stored() {
        let stored = [11.984_314, 0.0, 0.866_145_3, 0.988_235_3];
        assert_eq!(as_programs_read(stored, true, ColourSpace::Linear), stored);
        assert_eq!(as_programs_read(stored, false, ColourSpace::Gamma), stored);
        let plain = as_programs_read(stored, false, ColourSpace::Linear);
        assert_eq!(
            plain,
            [
                gamma_to_linear(11.984_314),
                0.0,
                gamma_to_linear(0.866_145_3),
                0.988_235_3
            ]
        );
    }
}
