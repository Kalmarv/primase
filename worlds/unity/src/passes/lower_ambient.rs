//! Unity's ambient lighting lowered to `core.environment`'s ambient (pass `lower-unity-ambient`); the stored facts stay
//! in `unity.render_settings`.
//!
//! RenderSettings' `m_AmbientMode` (Unity's `AmbientMode`):
//! - 1, trilight: [`AmbientLight::Gradient`] of the sky, equator and ground colours;
//! - 3, flat: [`AmbientLight::Flat`], the sky colour;
//! - 0, skybox, and 4, custom: Unity lights from an ambient probe made from the skybox or a custom cubemap, which the
//!   reader does not read, and the three colours are what the scene last stored for trilight. The program chooses
//!   ([`UnityAmbientProbe`]): no ambient (omitted, reported) or the stored colours as a gradient (an approximation,
//!   reported);
//! - any other value is not an ambient Unity defines: none, reported omitted.
//!
//! Colours: stored gamma-encoded, so linear by `srgb_to_linear` (as `lower-unity-lights` takes the lights' colours);
//! alpha is not light. Intensity: `m_AmbientIntensity` as stored. Unity's ambient is in the units of its lights'
//! intensity, relative to nothing physical, and is taken at the scale of a directional `core.light`'s (as
//! `lower-unity-lights` takes a directional light's intensity as lux): an approximation, reported. Unity's editor shows
//! the intensity multiplier for skybox ambient alone; that it scales trilight and flat colours too is not verified
//! against the engine, so a multiplier other than 1 in those modes is reported as an approximation.
//!
//! The pass keeps the environment's sun (`lower-unity-lights`') and replaces its ambient.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Ambient, AmbientLight, Environment, World};
use primase_image::srgb_to_linear_f32;

use crate::components::{UnityAmbient, UnityRenderSettings};

/// What [`LowerUnityAmbient`] makes of a skybox (0) or custom (4) ambient, whose probe the reader does not read. No
/// default: the program chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnityAmbientProbe {
    /// No ambient, reported omitted (`probe=omit`).
    Omit,
    /// The stored sky, equator and ground colours as a gradient, reported as an approximation (`probe=stored-gradient`).
    StoredGradient,
}

impl UnityAmbientProbe {
    fn text(self) -> &'static str {
        match self {
            UnityAmbientProbe::Omit => "omit",
            UnityAmbientProbe::StoredGradient => "stored-gradient",
        }
    }
}

/// `lower-unity-ambient{probe=omit|stored-gradient}`: see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerUnityAmbient {
    pub probe: UnityAmbientProbe,
}

impl LowerUnityAmbient {
    /// From pipeline options: `probe` is required.
    pub fn from_options(o: &Options) -> Result<LowerUnityAmbient, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "probe") {
            return Err(format!("unknown option {k}"));
        }
        let probe = match o.get("probe").map(String::as_str) {
            Some("omit") => UnityAmbientProbe::Omit,
            Some("stored-gradient") => UnityAmbientProbe::StoredGradient,
            Some(v) => return Err(format!("probe={v}: omit or stored-gradient expected")),
            None => {
                return Err(
                    "probe=omit|stored-gradient is required: what a skybox or custom ambient becomes".into(),
                );
            }
        };
        Ok(LowerUnityAmbient { probe })
    }
}

/// The stored colours as linear light, alpha left out.
fn linear(c: [f32; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]].map(srgb_to_linear_f32)
}

/// The ambient `a` lowers to under `probe`, each outcome reported; `None` when there is none.
pub fn lower_unity_ambient(a: &UnityAmbient, probe: UnityAmbientProbe, log: &mut PassLog) -> Option<Ambient> {
    let gradient = || AmbientLight::Gradient {
        sky: linear(a.sky),
        equator: linear(a.equator),
        ground: linear(a.ground),
    };
    let light = match (a.mode, probe) {
        (1, _) => gradient(),
        (3, _) => AmbientLight::Flat(linear(a.sky)),
        (0 | 4, UnityAmbientProbe::Omit) => {
            log.omit(
                &format!(
                    "ambient mode {} (skybox or custom): its probe is not read",
                    a.mode
                ),
                1,
            );
            return None;
        }
        (0 | 4, UnityAmbientProbe::StoredGradient) => {
            log.approximate(
                &format!(
                    "ambient mode {} (skybox or custom) approximated by its stored sky, equator and ground",
                    a.mode
                ),
                1,
            );
            gradient()
        }
        (m, _) => {
            log.omit(&format!("ambient mode {m}: not an ambient mode Unity defines"), 1);
            return None;
        }
    };
    if matches!(a.mode, 1 | 3) && a.intensity != 1.0 {
        log.approximate(
            "ambient intensity multiplier applied to trilight or flat colours (not verified against Unity)",
            1,
        );
    }
    log.approximate(
        "Unity ambient intensity taken at the scale of a directional light's (lux)",
        1,
    );
    log.count(
        match light {
            AmbientLight::Flat(_) => "flat ambient",
            AmbientLight::Gradient { .. } => "gradient ambient",
        },
        1,
    );
    Some(Ambient {
        intensity: a.intensity,
        light,
    })
}

impl Pass for LowerUnityAmbient {
    fn name(&self) -> &'static str {
        "lower-unity-ambient"
    }
    fn options(&self) -> Options {
        Options::from([("probe".to_owned(), self.probe.text().to_owned())])
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.render_settings"]
    }
    /// `core.environment`, made or with its ambient replaced.
    fn provides(&self) -> &[&'static str] {
        &["core.environment"]
    }
    fn approximates(&self) -> &'static str {
        "Unity ambient intensity taken as lux; a skybox or custom ambient omitted or approximated by its stored colours"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(stored) = w
            .resource::<UnityRenderSettings>()
            .and_then(|s| s.ambient.clone())
        else {
            log.count("scenes storing no ambient", 1);
            return Ok(());
        };
        let ambient = lower_unity_ambient(&stored, self.probe, log);
        let mut env = w.resource::<Environment>().cloned().unwrap_or_default();
        env.ambient = ambient;
        w.insert_resource(env);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::Resource;

    const SKY: [f32; 4] = [0.5647, 0.5972, 0.6795, 1.0];
    const EQUATOR: [f32; 4] = [0.7835, 0.7835, 0.7835, 1.0];
    const GROUND: [f32; 4] = [0.5457, 0.5457, 0.5457, 0.0];

    fn stored(mode: i64, intensity: f32) -> UnityAmbient {
        UnityAmbient {
            mode,
            sky: SKY,
            equator: EQUATOR,
            ground: GROUND,
            intensity,
        }
    }

    fn lowered(mode: i64, intensity: f32, probe: UnityAmbientProbe) -> (World, PassLog) {
        let mut w = World::new();
        w.insert_resource(UnityRenderSettings {
            sun: None,
            pipeline: None,
            ambient: Some(stored(mode, intensity)),
        });
        let mut log = PassLog::default();
        LowerUnityAmbient { probe }.run(&mut w, &mut log).unwrap();
        assert!(w.verify_resources().is_empty());
        (w, log)
    }

    fn ambient(w: &World) -> Option<Ambient> {
        w.resource::<Environment>().and_then(|e| e.ambient.clone())
    }

    #[test]
    fn trilight_and_flat_lower_to_linear_gradient_and_flat() {
        let lin = |c: [f32; 4]| [c[0], c[1], c[2]].map(srgb_to_linear_f32);
        for probe in [UnityAmbientProbe::Omit, UnityAmbientProbe::StoredGradient] {
            let (w, log) = lowered(1, 1.0, probe);
            assert_eq!(
                ambient(&w),
                Some(Ambient {
                    intensity: 1.0,
                    light: AmbientLight::Gradient {
                        sky: lin(SKY),
                        equator: lin(EQUATOR),
                        ground: lin(GROUND),
                    },
                })
            );
            assert_eq!(log.counts["gradient ambient"], 1);
            assert_eq!(
                log.approximations.keys().collect::<Vec<_>>(),
                ["Unity ambient intensity taken at the scale of a directional light's (lux)"]
            );
            let (w, log) = lowered(3, 1.0, probe);
            assert_eq!(
                ambient(&w),
                Some(Ambient {
                    intensity: 1.0,
                    light: AmbientLight::Flat(lin(SKY)),
                })
            );
            assert_eq!(log.counts["flat ambient"], 1);
        }
        // 0.5647 stored is 0.2788... linear, not the stored value
        let (w, _) = lowered(3, 1.0, UnityAmbientProbe::Omit);
        let Some(AmbientLight::Flat(c)) = ambient(&w).map(|a| a.light) else {
            panic!()
        };
        assert!((c[0] - 0.27889).abs() < 1e-4, "{c:?}");
        // a multiplier other than 1 is kept and reported
        let (w, log) = lowered(1, 2.0, UnityAmbientProbe::Omit);
        assert_eq!(ambient(&w).unwrap().intensity, 2.0);
        assert_eq!(
            log.approximations["ambient intensity multiplier applied to trilight or flat colours (not verified against Unity)"],
            1
        );
    }

    #[test]
    fn skybox_and_custom_follow_the_programs_choice_and_never_fall_through_silently() {
        for mode in [0, 4] {
            let (w, log) = lowered(mode, 1.0, UnityAmbientProbe::Omit);
            assert_eq!(ambient(&w), None);
            assert!(w.resource::<Environment>().is_some());
            assert_eq!(
                log.omitted[&format!("ambient mode {mode} (skybox or custom): its probe is not read")],
                1
            );
            assert!(log.approximations.is_empty());
            let (w, log) = lowered(mode, 1.0, UnityAmbientProbe::StoredGradient);
            assert!(matches!(
                ambient(&w).unwrap().light,
                AmbientLight::Gradient { .. }
            ));
            assert_eq!(
                log.approximations[&format!(
                    "ambient mode {mode} (skybox or custom) approximated by its stored sky, equator and ground"
                )],
                1
            );
        }
        let (w, log) = lowered(2, 1.0, UnityAmbientProbe::StoredGradient);
        assert_eq!(ambient(&w), None);
        assert_eq!(
            log.omitted["ambient mode 2: not an ambient mode Unity defines"],
            1
        );
    }

    #[test]
    fn the_sun_is_kept_and_a_scene_without_ambient_changes_nothing() {
        let mut w = World::new();
        let sun = w.spawn();
        w.insert(
            sun,
            primase_core::Light {
                kind: primase_core::LightKind::Directional,
                color: [1.0; 3],
                intensity: 1.0,
                range: None,
                cast_shadows: None,
            },
        );
        w.insert_resource(Environment {
            sun: Some(sun),
            ambient: None,
        });
        w.insert_resource(UnityRenderSettings {
            sun: None,
            pipeline: None,
            ambient: Some(stored(3, 1.0)),
        });
        let pass = LowerUnityAmbient {
            probe: UnityAmbientProbe::Omit,
        };
        pass.run(&mut w, &mut PassLog::default()).unwrap();
        let env = w.resource::<Environment>().unwrap();
        assert_eq!(env.sun, Some(sun));
        assert!(env.ambient.is_some());
        let mut bare = World::new();
        bare.insert_resource(UnityRenderSettings::default());
        let mut log = PassLog::default();
        pass.run(&mut bare, &mut log).unwrap();
        assert!(bare.resource::<Environment>().is_none());
        assert_eq!(log.counts["scenes storing no ambient"], 1);
        assert_eq!(Environment::NAME, "core.environment");
    }

    #[test]
    fn options_name_the_choice_and_have_no_default() {
        let o = |kv: &[(&str, &str)]| -> Options {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(LowerUnityAmbient::from_options(&o(&[])).is_err());
        assert!(LowerUnityAmbient::from_options(&o(&[("probe", "sky")])).is_err());
        assert!(LowerUnityAmbient::from_options(&o(&[("probe", "omit"), ("x", "1")])).is_err());
        for p in ["omit", "stored-gradient"] {
            let pass = LowerUnityAmbient::from_options(&o(&[("probe", p)])).unwrap();
            assert_eq!(pass.options(), o(&[("probe", p)]));
        }
    }
}
