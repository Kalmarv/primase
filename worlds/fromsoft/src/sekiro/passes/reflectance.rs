//! Interpretation of Sekiro's encoded G-buffer3, separate from shader execution.
use crate::{layer_bake::OutputRgb, materials::Material};
use fromsoft_formats::mtd::Value;
use primase_core::pass::PassLog;
use primase_core::pbr::Specular;

// Literal in M[Multiple]_1_Gbuf.ppo at byte 11208, independently decoded from
// the original bytes. GXLightAcc_DDirLight1 squares GBuffer3 RGB before Schlick.
const DIELECTRIC_ENCODED: u32 = 0x3e31_5cac;

/// The traced dielectric's F0, the square of the stored value over glTF's dielectric F0 (`0.04`, the extension's
/// default index of refraction): the `specularFactor` that makes a metallic-0 material reflect exactly what the program's
/// G-buffer 3 states (0.03), not 0.04.
pub(super) const DIELECTRIC_F0_FACTOR: f32 =
    f32::from_bits(DIELECTRIC_ENCODED) * f32::from_bits(DIELECTRIC_ENCODED) / 0.04;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reflectance {
    Dielectric,
    Unsupported,
}

impl Reflectance {
    pub(super) fn classify(source: &Material, size: u32, pixels: Option<&[OutputRgb]>) -> Self {
        let Some(layered) = &source.layered else {
            return Self::Unsupported;
        };
        let leaf = layered.definition.shader.rsplit(['/', '\\']).next().unwrap_or("");
        let family = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
        let switch = layered
            .definition
            .parameters
            .iter()
            .find(|p| p.name == "GXFT_SATStaticSwitch")
            .and_then(|p| match &p.value {
                Value::Int(v) if v.len() == 1 => Some(v[0]),
                _ => None,
            });
        // Scope of the retained 102-program producer/consumer census. The one
        // variable-reflectance program stays unsupported even at a .03 sample.
        let known = matches!(
            family,
            "M[Multiple]" | "M[MultipleGround]" | "M[Multiple]_Emissive"
        ) && switch.is_some_and(|s| (0..=33).contains(&s))
            && !(family == "M[Multiple]" && switch == Some(18));
        let complete = pixels.is_some_and(|pixels| {
            !pixels.is_empty()
                && pixels.len() == size as usize * size as usize
                && pixels
                    .iter()
                    .all(|p| p.mask == 7 && p.bits == [DIELECTRIC_ENCODED; 3])
        });
        if known && complete {
            Self::Dielectric
        } else {
            Self::Unsupported
        }
    }

    /// The material's specular reflectance: the traced dielectric's F0 as `specularFactor` (the metallic workflow's
    /// 0.04 scaled to it); none where the program's reflectance is not traced (the extension's default, 0.04).
    pub(super) fn specular(self) -> Option<Specular> {
        match self {
            Self::Dielectric => Some(Specular {
                factor: DIELECTRIC_F0_FACTOR,
                ..Specular::default()
            }),
            Self::Unsupported => None,
        }
    }

    // Called once for each newly instantiated, referenced output material;
    // neither unused bake levels nor repeated primitive uses inflate this count.
    pub(super) fn record_material(self, log: &mut PassLog) {
        match self {
            Self::Dielectric => {
                log.count("baked output materials with traced dielectric reflectance", 1);
            }
            Self::Unsupported => {
                log.count("baked output materials with unsupported reflectance", 1);
                log.approximate(super::pbr::UNKNOWN_METALLIC_LOSS, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(family: &str, switch: i32) -> Material {
        crate::materials::resolve(
            &fromsoft_formats::flver::Material {
                string_bytes: 0,
                index: 0,
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                textures: vec![],
                gx: vec![],
            },
            Some(&fromsoft_formats::mtd::MaterialDefinition {
                shader: family.into(),
                description: String::new(),
                textures: vec![],
                block_lengths: vec![],
                parameters: vec![fromsoft_formats::mtd::Parameter {
                    name: "GXFT_SATStaticSwitch".into(),
                    value: Value::Int(vec![switch]),
                }],
            }),
            |_| false,
        )
    }
    #[test]
    fn only_full_exact_supported_outputs_are_dielectric() {
        let source = source("M[Multiple].fx", 1);
        let pixel = OutputRgb {
            bits: [0x3e31_5cac; 3],
            mask: 7,
        };
        assert_eq!(
            Reflectance::classify(&source, 1, Some(&[pixel])),
            Reflectance::Dielectric
        );
        for pixels in [
            None,
            Some(vec![]),
            Some(vec![pixel; 2]),
            Some(vec![OutputRgb { mask: 0, ..pixel }]),
            Some(vec![OutputRgb { mask: 3, ..pixel }]),
            Some(vec![OutputRgb {
                bits: [f32::NAN.to_bits(); 3],
                ..pixel
            }]),
            Some(vec![OutputRgb {
                bits: [f32::INFINITY.to_bits(); 3],
                ..pixel
            }]),
            Some(vec![OutputRgb {
                bits: [0x3e31_5cad; 3],
                ..pixel
            }]),
        ] {
            assert_eq!(
                Reflectance::classify(&source, 1, pixels.as_deref()),
                Reflectance::Unsupported
            );
        }
    }
    /// A traced dielectric reflects the program's F0 (0.03 within f32), not glTF's 0.04: the extension's
    /// `min(0.04 * colour, 1) * factor`; an untraced one states nothing.
    #[test]
    fn a_traced_dielectric_states_the_programs_f0_as_the_specular_factor() {
        let stored = f32::from_bits(DIELECTRIC_ENCODED);
        // an absent specular reads as the default (factor 1), which fails the checks below
        let specular = Reflectance::Dielectric.specular().unwrap_or_default();
        assert!((0.04 * specular.factor - stored * stored).abs() < 1e-9);
        assert!((0.04 * specular.factor - 0.03).abs() < 1e-5);
        assert_eq!(
            (specular.color, specular.texture, specular.color_texture),
            ([1.; 3], None, None)
        );
        assert!(specular.factor > 0. && specular.factor <= 1.);
        assert_eq!(Reflectance::Unsupported.specular(), None);
        let mut log = PassLog::default();
        Reflectance::Dielectric.record_material(&mut log);
        assert!(log.approximations.is_empty());
    }
    #[test]
    fn unsupported_programs_cannot_become_dielectric_by_coincidence() {
        let pixel = OutputRgb {
            bits: [0x3e31_5cac; 3],
            mask: 7,
        };
        for (family, switch) in [("M[Multiple]", 18), ("M[Multiple]", 34), ("Other", 1)] {
            assert_eq!(
                Reflectance::classify(&source(family, switch), 1, Some(&[pixel])),
                Reflectance::Unsupported
            );
        }
    }
}
