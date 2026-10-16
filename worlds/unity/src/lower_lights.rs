//! Unity lights lowered to `core.light` (pass `lower-unity-lights`), on the same entities.
//!
//! The named conversion, URP to `KHR_lights_punctual`:
//! - colour: URP shades with the colour property in linear space (the stored value is gamma-encoded), so
//!   `color = srgb_to_linear(stored rgb)`; a colour temperature, when the light uses one, is not applied (counted);
//! - intensity: URP lights a surface with `colour x intensity / d^2` for point and spot lights (within its range) and
//!   `colour x intensity` for a directional one, which is what candela and lux mean to a renderer that takes
//!   `KHR_lights_punctual` at face value: `intensity = URP intensity`, unchanged (URP's light unit field is not used by
//!   URP's shading);
//! - range: URP's range, where its attenuation reaches zero; a directional light has none;
//! - a spot's cone: URP's spot angles are full cone angles in degrees, so `outer = spot_angle / 2` and
//!   `inner = inner_spot_angle / 2`, in radians (outer clamped to pi / 2, inner to outer);
//! - shadows: `m_Shadows.m_Type` none (0) does not cast, hard (1) and soft (2) do; softness is a renderer setting;
//! - direction: a Unity light shines along its local +Z, which the frame conversion makes the world's local -Z, as
//!   `core.light` wants: the transform is used as it is.
//!
//! Area lights (rectangle, disc) are baked-only in URP and have no punctual form: not lowered, counted.

use primase_core::pass::{Pass, PassLog};
use primase_core::{Entity, Light, LightKind, World};
use primase_image::srgb_to_linear;

use crate::Result;
use crate::components::UnityLight;

/// Lower every `unity.light` (see the module documentation).
pub fn lower_unity_lights(w: &mut World) -> Result<PassLog> {
    let mut log = PassLog::default();
    let lights: Vec<(Entity, UnityLight)> = w.iter::<UnityLight>().map(|(e, l)| (e, l.clone())).collect();
    for (e, l) in lights {
        let half = |deg: f32| (f64::from(deg) / 2.0).to_radians() as f32;
        let kind = match l.kind {
            0 => {
                let outer = half(l.spot_angle).clamp(0.0, std::f32::consts::FRAC_PI_2);
                LightKind::Spot {
                    inner_cone: half(l.inner_spot_angle).clamp(0.0, outer),
                    outer_cone: outer,
                }
            }
            1 => LightKind::Directional,
            2 => LightKind::Point,
            _ => {
                log.count("area lights not lowered (baked-only in URP)", 1);
                continue;
            }
        };
        if l.color_temperature.is_some() {
            log.approximate("light colour temperature not applied", 1);
        }
        let range = (kind != LightKind::Directional && l.range > 0.0).then_some(l.range);
        w.insert(
            e,
            Light {
                kind,
                color: [0, 1, 2].map(|k| srgb_to_linear(f64::from(l.color[k])) as f32),
                intensity: l.intensity.max(0.0),
                range,
                cast_shadows: match l.shadows {
                    0 => Some(false),
                    1 | 2 => Some(true),
                    _ => {
                        log.count("lights with an unknown shadow type, cast unknown", 1);
                        None
                    }
                },
            },
        );
        log.count(
            match kind {
                LightKind::Point => "point lights",
                LightKind::Spot { .. } => "spot lights",
                LightKind::Directional => "directional lights",
            },
            1,
        );
    }
    log.approximate("URP intensity taken as candela (lux for directional lights)", 1);
    Ok(log)
}

/// [`lower_unity_lights`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct LowerUnityLights;

impl Pass for LowerUnityLights {
    fn name(&self) -> &'static str {
        "lower-unity-lights"
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.light"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.light"]
    }
    fn approximates(&self) -> &'static str {
        "URP intensity taken as candela or lux; colour temperature not applied; area lights left out"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = lower_unity_lights(w).map_err(|e| e.to_string())?;
        for (k, n) in r.counts {
            log.count(&k, n);
        }
        for (k, n) in r.approximations {
            log.approximate(&k, n);
        }
        Ok(())
    }
}
