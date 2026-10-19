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
//! Under another pipeline (built-in) the same conversion is applied and counted as an approximation: built-in
//! attenuates point and spot lights by its own falloff, not the inverse square, and in a linear-colour project
//! without linear light intensity it applies the intensity in gamma space; neither is modelled.
//!
//! Area lights (rectangle, disc) are baked-only in URP and have no punctual form: not lowered, counted.
//!
//! The sun, `core.environment`'s: the light the Universal Render Pipeline makes its main light, as its
//! `UniversalRenderPipeline.GetBrightestDirectionalLightIndex` chooses it (Unity-Technologies/Graphics,
//! `Packages/com.unity.render-pipelines.universal/Runtime/UniversalRenderPipeline.cs`, the same in 6000.3): among the
//! visible directional lights, the one RenderSettings names (`m_Sun`), else the one with the greatest `intensity`
//! (above 0; the first in its visible-light order on a tie). Here:
//! - the named light (`unity.render_settings`), once lowered to a directional light, whatever the pipeline;
//! - else, for a player whose pipeline is URP ([`RenderPipeline::Universal`]), the shining directional light with the
//!   strictly greatest intensity above 0. A tie gives none (URP's visible-light order is not a scene fact), as do no
//!   intensity above 0 and a fully baked winner (whether URP's culling keeps it is not modelled), each counted. URP
//!   does this only when its asset's main light mode is per-pixel; the asset is a script object without a type tree,
//!   not decoded, so that is assumed (counted as an approximation);
//! - else (another pipeline, or none known) none, counted.
//!
//! Lights with a state condition (`core.condition`: off as stored) are not visible as stored, so neither named nor
//! candidates; `resolve-state` before this pass leaves the same choice.

use primase_core::pass::{Pass, PassLog};
use primase_core::state::Condition;
use primase_core::{Entity, Light, LightKind, World};
use primase_image::srgb_to_linear;

use crate::Result;
use crate::components::{UnityLight, UnityRenderSettings};
use crate::player::RenderPipeline;

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
    let urp = w
        .resource::<UnityRenderSettings>()
        .is_some_and(|s| s.pipeline == Some(RenderPipeline::Universal));
    log.approximate(
        if urp {
            "URP intensity taken as candela (lux for directional lights)"
        } else {
            "intensity of a pipeline other than URP taken as URP's (built-in's attenuation and gamma-space intensity \
             not modelled)"
        },
        1,
    );
    if let Some(sun) = sun(w, &mut log) {
        w.insert_resource(primase_core::Environment { sun: Some(sun) });
    }
    Ok(log)
}

/// The sun (see the module documentation), each outcome counted.
fn sun(w: &World, log: &mut PassLog) -> Option<Entity> {
    let settings = w.resource::<UnityRenderSettings>().cloned().unwrap_or_default();
    let directional = |e: Entity| {
        w.get::<Light>(e)
            .is_some_and(|l| l.kind == LightKind::Directional)
            && !w.has::<Condition>(e)
    };
    match settings.sun {
        Some(sun) if directional(sun) => {
            log.count("sun named by RenderSettings", 1);
            return Some(sun);
        }
        Some(sun) if w.has::<Condition>(sun) => log.count("RenderSettings sun off as stored", 1),
        Some(_) => log.count("RenderSettings sun not a directional light", 1),
        None => {}
    }
    if settings.pipeline != Some(RenderPipeline::Universal) {
        log.count("no sun: none named and the pipeline is not URP", 1);
        return None;
    }
    let lights: Vec<(Entity, &UnityLight)> =
        w.iter::<UnityLight>().filter(|&(e, _)| directional(e)).collect();
    let best = lights.iter().map(|(_, l)| l.intensity).fold(0.0f32, f32::max);
    let top: Vec<&(Entity, &UnityLight)> = lights
        .iter()
        .filter(|(_, l)| best > 0.0 && l.intensity == best)
        .collect();
    match top[..] {
        [] => log.count("no sun: no directional light brighter than 0 (URP)", 1),
        [&(_, l)] if l.baked => log.count("no sun: URP's brightest directional light is fully baked", 1),
        [&(e, _)] => {
            log.count("sun by URP's main-light rule (brightest directional light)", 1);
            log.approximate(
                "URP main light mode taken as per-pixel (pipeline asset not decoded)",
                1,
            );
            return Some(e);
        }
        _ => log.count("no sun: directional lights tie for brightest (URP)", 1),
    }
    None
}

/// [`lower_unity_lights`] as a pass.
#[derive(Debug, Clone, Copy)]
pub struct LowerUnityLights;

impl Pass for LowerUnityLights {
    fn name(&self) -> &'static str {
        "lower-unity-lights"
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.light"]
    }
    /// `core.light`s, and the `core.environment` resource when RenderSettings names a directional sun.
    fn provides(&self) -> &[&'static str] {
        &["core.light", "core.environment"]
    }
    /// It lowers Unity lights and keeps them.
    fn legalizes(&self) -> &[&'static str] {
        &["unity.light"]
    }
    fn approximates(&self) -> &'static str {
        "URP intensity taken as candela or lux; colour temperature not applied; area lights left out"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = lower_unity_lights(w).map_err(|e| e.to_string())?;
        log.absorb(r);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::UnityObject;

    fn light(w: &mut World, kind: i64) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            UnityLight {
                light: UnityObject {
                    file: "level1".into(),
                    path_id: i64::from(e.index()) + 1,
                },
                kind,
                color: [1.0; 4],
                intensity: 2.0,
                range: 10.0,
                spot_angle: 30.0,
                inner_spot_angle: 0.0,
                shadows: 2,
                color_temperature: None,
                bounce_intensity: 1.0,
                light_unit: None,
                lux_at_distance: None,
                area_size: [1.0; 2],
                render_mode: 0,
                baked: false,
                culling_mask: u32::MAX,
                cookie: None,
            },
        );
        w.insert(e, primase_core::Transform::IDENTITY);
        e
    }

    fn settings(sun: Option<Entity>, pipeline: Option<RenderPipeline>) -> UnityRenderSettings {
        UnityRenderSettings {
            sun,
            pipeline,
            ambient: None,
        }
    }

    fn environment_sun(w: &mut World) -> (Option<Entity>, PassLog) {
        let log = lower_unity_lights(w).unwrap();
        assert!(w.verify_resources().is_empty());
        (w.resource::<primase_core::Environment>().and_then(|e| e.sun), log)
    }

    fn set_intensity(w: &mut World, e: Entity, intensity: f32) {
        let mut l = w.get::<UnityLight>(e).unwrap().clone();
        l.intensity = intensity;
        w.insert(e, l);
    }

    #[test]
    fn the_named_directional_sun_becomes_the_environment_sun() {
        let mut w = World::new();
        let sun = light(&mut w, 1);
        let brighter = light(&mut w, 1);
        set_intensity(&mut w, brighter, 3.0);
        let lamp = light(&mut w, 2);
        // no settings read: no environment
        assert_eq!(environment_sun(&mut w.clone()).0, None);
        // named, whatever the pipeline and however bright the others
        for pipeline in [
            None,
            Some(RenderPipeline::BuiltIn),
            Some(RenderPipeline::Universal),
        ] {
            let mut named = w.clone();
            named.insert_resource(settings(Some(sun), pipeline));
            let (found, log) = environment_sun(&mut named);
            assert_eq!(found, Some(sun));
            assert_eq!(log.counts["sun named by RenderSettings"], 1);
        }
        // a named light that is not directional is not a sun; URP then takes its brightest directional light
        w.insert_resource(settings(Some(lamp), Some(RenderPipeline::BuiltIn)));
        let (found, log) = environment_sun(&mut w.clone());
        assert_eq!(found, None);
        assert_eq!(log.counts["RenderSettings sun not a directional light"], 1);
        w.insert_resource(settings(Some(lamp), Some(RenderPipeline::Universal)));
        assert_eq!(environment_sun(&mut w).0, Some(brighter));
    }

    #[test]
    fn urp_without_a_named_sun_takes_the_strictly_brightest_directional_light() {
        let mut w = World::new();
        let a = light(&mut w, 1);
        let b = light(&mut w, 1);
        let lamp = light(&mut w, 2);
        set_intensity(&mut w, lamp, 100.0); // not directional: never the sun
        set_intensity(&mut w, b, 2.5);
        w.insert_resource(settings(None, Some(RenderPipeline::Universal)));
        let (found, log) = environment_sun(&mut w.clone());
        assert_eq!(found, Some(b));
        assert_eq!(
            log.counts["sun by URP's main-light rule (brightest directional light)"],
            1
        );
        assert_eq!(
            log.approximations["URP main light mode taken as per-pixel (pipeline asset not decoded)"],
            1
        );
        // other pipelines, or none known: no fallback
        for pipeline in [
            None,
            Some(RenderPipeline::BuiltIn),
            Some(RenderPipeline::Other(vec![
                "built-in".into(),
                crate::player::URP_ASSET.into(),
            ])),
        ] {
            let mut other = w.clone();
            other.insert_resource(settings(None, pipeline));
            let (found, log) = environment_sun(&mut other);
            assert_eq!(found, None);
            assert_eq!(log.counts["no sun: none named and the pipeline is not URP"], 1);
        }
        // a tie for brightest
        let mut tie = w.clone();
        set_intensity(&mut tie, a, 2.5);
        let (found, log) = environment_sun(&mut tie);
        assert_eq!(found, None);
        assert_eq!(
            log.counts["no sun: directional lights tie for brightest (URP)"],
            1
        );
        // nothing brighter than 0
        let mut dark = w.clone();
        set_intensity(&mut dark, a, 0.0);
        set_intensity(&mut dark, b, -1.0);
        let (found, log) = environment_sun(&mut dark);
        assert_eq!(found, None);
        assert_eq!(
            log.counts["no sun: no directional light brighter than 0 (URP)"],
            1
        );
        // a brighter light that is off as stored is not a candidate, nor a named sun
        let mut hidden = w.clone();
        set_intensity(&mut hidden, a, 9.0);
        hidden.insert(a, Condition(Vec::new()));
        assert_eq!(environment_sun(&mut hidden).0, Some(b));
        hidden.insert_resource(settings(Some(a), Some(RenderPipeline::Universal)));
        let (found, log) = environment_sun(&mut hidden);
        assert_eq!(found, Some(b));
        assert_eq!(log.counts["RenderSettings sun off as stored"], 1);
        // a fully baked winner
        let mut baked = w.clone();
        let mut l = baked.get::<UnityLight>(b).unwrap().clone();
        l.baked = true;
        baked.insert(b, l);
        let (found, log) = environment_sun(&mut baked);
        assert_eq!(found, None);
        assert_eq!(
            log.counts["no sun: URP's brightest directional light is fully baked"],
            1
        );
    }
}
