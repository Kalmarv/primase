//! Interpretation of Sekiro's encoded G-buffer3, separate from shader execution.
use crate::{layer_bake::OutputRgb, materials::Material, mtd::Value};
use primase_core::pass::PassLog;

// Literal in M[Multiple]_1_Gbuf.ppo at byte 11208, independently decoded from
// the original bytes. GXLightAcc_DDirLight1 squares GBuffer3 RGB before Schlick.
const DIELECTRIC_ENCODED: u32 = 0x3e31_5cac;

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

    // Called once for each newly instantiated, referenced output material;
    // neither unused bake levels nor repeated primitive uses inflate this count.
    pub(super) fn record_material(self, log: &mut PassLog) {
        match self {
            Self::Dielectric => {
                log.count("baked output materials with traced dielectric reflectance", 1);
                log.approximate("source dielectric F0 0.03 mapped to target F0 0.04", 1);
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
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                textures: vec![],
                gx: vec![],
            },
            Some(&crate::mtd::MaterialDefinition {
                shader: family.into(),
                description: String::new(),
                textures: vec![],
                block_lengths: vec![],
                parameters: vec![crate::mtd::Parameter {
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
