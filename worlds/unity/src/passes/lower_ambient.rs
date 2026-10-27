//! Unity's ambient lighting lowered to `core.environment`'s ambient (pass `lower-unity-ambient`); the stored facts stay
//! in `unity.render_settings`.
//!
//! RenderSettings' `m_AmbientMode` (Unity's `AmbientMode`):
//! - 1, trilight: [`AmbientLight::Gradient`] of the sky, equator and ground colours;
//! - 3, flat: [`AmbientLight::Flat`], the sky colour;
//! - 0, skybox, and 4, custom: Unity lights from its ambient probe (`m_AmbientProbe`), generated in the editor from
//!   the skybox or a custom cubemap; the three colours stored beside it are whatever the scene last held for trilight
//!   (Unity's defaults when never set). The program chooses ([`UnityAmbientProbe`]): no ambient (omitted, reported),
//!   the stored colours as a gradient (an approximation, reported, and reported again when they are Unity's defaults),
//!   or the probe's average ([`probe_average`]: flat, an approximation, reported; none when the scene stores no probe or
//!   an all-zero one, which the editor never generated);
//! - any other value is not an ambient Unity defines: none, reported omitted.
//!
//! In trilight and flat modes the probe is not used: the colours are what Unity lights by, and a stored probe need not
//! match them (SkateBIRD 2019.4's trilight levels store one probe for different colours).
//!
//! Colours: stored gamma-encoded, so linear by `srgb_to_linear` (as `lower-unity-lights` takes the lights' colours);
//! alpha is not light. Intensity: `m_AmbientIntensity` as stored, times `candela-per-unit`, the same required
//! calibration `lower-unity-lights` states (finite and above 0): Unity's ambient is in the units of its lights'
//! intensity, relative to nothing physical, so ambient and sun keep Unity's proportion only when one number scales both,
//! and the ambient is taken at the scale of a directional `core.light`'s: an approximation, reported. An ambient
//! intensity that is negative or not finite (or not finite at the calibration) is no ambient `core.environment` can
//! hold: the ambient is left out and counted. Unity's editor shows
//! the intensity multiplier for skybox ambient alone; that it scales trilight and flat colours too is not verified
//! against the engine, so a multiplier other than 1 in those modes is reported as an approximation. A skybox probe is
//! taken to hold the multiplier already (intensity 1 here; a stored multiplier other than 1 is reported).
//!
//! The pass keeps the environment's sun (`lower-unity-lights`') and replaces its ambient.

use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};

use super::lower_lights::parse_candela_per_unit;
use primase_core::{Ambient, AmbientLight, Environment, World};
use primase_foundations::color::srgb_to_linear_f32;

use crate::components::UnityRenderSettings;
use unity_formats::scene::UnityAmbient;

/// What [`LowerUnityAmbient`] makes of a skybox (0) or custom (4) ambient. No default: the program chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnityAmbientProbe {
    /// No ambient, reported omitted (`probe=omit`).
    Omit,
    /// The stored sky, equator and ground colours as a gradient, reported as an approximation (`probe=stored-gradient`).
    StoredGradient,
    /// The stored ambient probe's average over all directions as a flat ambient ([`probe_average`]), reported as an
    /// approximation (`probe=probe-average`).
    ProbeAverage,
}

impl Choice for UnityAmbientProbe {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("omit", UnityAmbientProbe::Omit),
        ("stored-gradient", UnityAmbientProbe::StoredGradient),
        ("probe-average", UnityAmbientProbe::ProbeAverage),
    ];
}

/// `lower-unity-ambient{probe=omit|stored-gradient|probe-average,candela-per-unit}`: see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerUnityAmbient {
    pub probe: UnityAmbientProbe,
    /// Candela (lux) per unit of Unity's intensity: the calibration `lower-unity-lights` is given, finite and above 0.
    pub candela_per_unit: f32,
}

/// `lower-unity-ambient`'s declaration ([`primase_core::pass::PassRegistry::add`]): what an ambient whose probe is
/// not read becomes, and the calibration of Unity's intensity, are the program's, no default.
pub const LOWER_UNITY_AMBIENT: PassSpec = PassSpec {
    name: "lower-unity-ambient",
    options: &[
        OptionSpec::choice::<UnityAmbientProbe>(
            "probe",
            "what a skybox or custom ambient becomes: omitted, the stored gradient, or the stored probe's average",
        ),
        OptionSpec::required(
            "candela-per-unit",
            "1",
            "candela (lux) per unit of Unity's light intensity, finite and above 0: the calibration the lights are given",
        ),
    ],
    make: |g| {
        Ok(Box::new(LowerUnityAmbient {
            probe: g.choice("probe"),
            candela_per_unit: parse_candela_per_unit(g.required("candela-per-unit"))?,
        }))
    },
};

/// The stored colours as linear light, alpha left out.
fn linear(c: [f32; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]].map(srgb_to_linear_f32)
}

/// Unity's default ambient colours (sky, equator, ground), as a scene that never set them stores them.
const UNITY_DEFAULT_COLOURS: [[f32; 3]; 3] = [
    [0.212, 0.227, 0.259],
    [0.114, 0.125, 0.133],
    [0.047, 0.043, 0.035],
];

/// What one coefficient of Unity's packing contributes to the probe's average over directions, per unit: the band-0
/// coefficient times this is the average.
///
/// Unity's SphericalHarmonicsL2 holds, per channel, the radiance's real SH coefficients convolved with the clamped
/// cosine (divided by pi: what a white diffuse surface gives back) and multiplied by each basis function's
/// normalization constant (`kNormalizationConstants`, undone by `UndoCosineRescaling` in Unity's Graphics repository,
/// `com.unity.render-pipelines.high-definition/Runtime/Lighting/SphericalHarmonics.cs`: band 0's is
/// `c0 = 1 / (2 sqrt(pi))`). Band 0's basis function is the constant `c0`, so its term evaluates to
/// `c0 x (U0 / c0) = U0` in every direction, and bands 1 and 2 average to zero over the sphere: the average of the
/// probe's lighting is `U0` itself, a factor of exactly 1. (So does Unity read it: `m_IndirectSpecularColor`, its
/// average ambient, equals `U0` in the skybox scenes read.)
pub const PROBE_AVERAGE_PER_L0: f32 = 1.0;

/// The probe's average over all directions (see [`PROBE_AVERAGE_PER_L0`]): each channel's band-0 coefficient
/// (indices 0, 9, 18).
pub fn probe_average(probe: &[f32; 27]) -> [f32; 3] {
    [probe[0], probe[9], probe[18]].map(|u| u * PROBE_AVERAGE_PER_L0)
}

/// The ambient `a` lowers to under `probe` at `candela_per_unit`, each outcome reported; `None` when there is none.
pub fn lower_unity_ambient(
    a: &UnityAmbient,
    probe: UnityAmbientProbe,
    candela_per_unit: f32,
    log: &mut PassLog,
) -> Option<Ambient> {
    if !a.intensity.is_finite() || a.intensity < 0.0 {
        log.omit("ambient intensity negative or not finite, left out", 1);
        return None;
    }
    let gradient = || AmbientLight::Gradient {
        sky: linear(a.sky),
        equator: linear(a.equator),
        ground: linear(a.ground),
    };
    let mut intensity = a.intensity;
    let light = match (a.mode, probe) {
        (1, _) => gradient(),
        (3, _) => AmbientLight::Flat(linear(a.sky)),
        (0 | 4, UnityAmbientProbe::Omit) => {
            log.omit(
                &format!(
                    "ambient mode {} (skybox or custom), left out by the program's choice",
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
            let stored = [a.sky, a.equator, a.ground].map(|c| [c[0], c[1], c[2]]);
            if stored == UNITY_DEFAULT_COLOURS {
                log.approximate(
                    "the stored sky, equator and ground are Unity's defaults, never set for the scene",
                    1,
                );
            }
            gradient()
        }
        (0 | 4, UnityAmbientProbe::ProbeAverage) => {
            let Some(p) = a.probe.filter(|p| p.iter().any(|&c| c != 0.0)) else {
                log.omit(
                    &format!(
                        "ambient mode {} (skybox or custom): the scene stores no ambient probe, or an all-zero one \
                         (never generated)",
                        a.mode
                    ),
                    1,
                );
                return None;
            };
            log.approximate(
                "ambient probe reduced to its average over directions (its band-0 coefficients, factor 1 in Unity's \
                 packing); bands 1 and 2 dropped",
                1,
            );
            if a.intensity != 1.0 {
                log.approximate(
                    "ambient intensity multiplier taken as already in the stored probe (not verified against Unity)",
                    1,
                );
            }
            intensity = 1.0;
            AmbientLight::Flat(probe_average(&p))
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
        "Unity ambient intensity at the caller's candela-per-unit, the scale of a directional light's (lux)",
        1,
    );
    log.count(
        match light {
            AmbientLight::Flat(_) => "flat ambient",
            AmbientLight::Gradient { .. } => "gradient ambient",
        },
        1,
    );
    let intensity = intensity * candela_per_unit;
    if !intensity.is_finite() {
        log.omit("ambient intensity not finite at the calibration, left out", 1);
        return None;
    }
    Some(Ambient { intensity, light })
}

impl Pass for LowerUnityAmbient {
    fn name(&self) -> &'static str {
        "lower-unity-ambient"
    }
    fn options(&self) -> Options {
        Options::from([
            ("probe".to_owned(), self.probe.text().to_owned()),
            ("candela-per-unit".to_owned(), self.candela_per_unit.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.render_settings"]
    }
    /// `core.environment`, made or with its ambient replaced.
    fn provides(&self) -> &[&'static str] {
        &["core.environment"]
    }
    fn approximates(&self) -> &'static str {
        "Unity ambient intensity as lux at the caller's candela-per-unit; a skybox or custom ambient omitted or approximated by its stored colours"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(stored) = w
            .resource::<UnityRenderSettings>()
            .and_then(|s| s.ambient.clone())
        else {
            log.count("scenes storing no ambient", 1);
            return Ok(());
        };
        let ambient = lower_unity_ambient(&stored, self.probe, self.candela_per_unit, log);
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

    /// Sledding Game's `Main Mountain Scene` (Unity 6000.3, skybox ambient): its stored `m_AmbientProbe`, whose
    /// `m_IndirectSpecularColor` (Unity's own average) is 0.17922863, 0.22442051, 0.30615595.
    const SLEDDING_PROBE: [f32; 27] = [
        0.17922863,
        -0.005708974,
        -0.0033339635,
        -0.013613597,
        -0.0107105,
        -0.0026229343,
        0.0063115368,
        0.004681349,
        0.027807336,
        0.22442052,
        0.04147503,
        -0.0052438304,
        -0.021416659,
        -0.017134635,
        -0.004195459,
        0.008511362,
        0.006835743,
        0.03851894,
        0.30615595,
        0.12832482,
        -0.009049028,
        -0.036960714,
        -0.030826086,
        -0.007547209,
        0.009171469,
        0.009939574,
        0.046476558,
    ];

    fn stored(mode: i64, intensity: f32) -> UnityAmbient {
        UnityAmbient {
            mode,
            sky: SKY,
            equator: EQUATOR,
            ground: GROUND,
            intensity,
            probe: Some(SLEDDING_PROBE),
        }
    }

    fn lowered(mode: i64, intensity: f32, probe: UnityAmbientProbe) -> (World, PassLog) {
        let mut w = World::new();
        w.insert_resource(UnityRenderSettings {
            sun: None,
            pipeline: None,
            ambient: Some(stored(mode, intensity)),
            ..Default::default()
        });
        let mut log = PassLog::default();
        LowerUnityAmbient {
            probe,
            candela_per_unit: 1.0,
        }
        .run(&mut w, &mut log)
        .unwrap();
        assert!(w.verify_resources().is_empty());
        (w, log)
    }

    fn ambient(w: &World) -> Option<Ambient> {
        w.resource::<Environment>().and_then(|e| e.ambient.clone())
    }

    #[test]
    fn trilight_and_flat_lower_to_linear_gradient_and_flat() {
        let lin = |c: [f32; 4]| [c[0], c[1], c[2]].map(srgb_to_linear_f32);
        // the probe is not used in these modes, whatever the choice
        for probe in [
            UnityAmbientProbe::Omit,
            UnityAmbientProbe::StoredGradient,
            UnityAmbientProbe::ProbeAverage,
        ] {
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
                [
                    "Unity ambient intensity at the caller's candela-per-unit, the scale of a directional light's (lux)"
                ]
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
                log.omitted
                    [&format!("ambient mode {mode} (skybox or custom), left out by the program's choice")],
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
        let (w, log) = lowered(2, 1.0, UnityAmbientProbe::ProbeAverage);
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
                radius: None,
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
            ..Default::default()
        });
        let pass = LowerUnityAmbient {
            probe: UnityAmbientProbe::Omit,
            candela_per_unit: 1.0,
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
    fn options_name_the_choice_and_the_calibration_and_have_no_default() {
        let o = |kv: &[(&str, &str)]| -> Options {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(LOWER_UNITY_AMBIENT.instance(&o(&[])).is_err());
        // each option alone is not enough
        assert!(LOWER_UNITY_AMBIENT.instance(&o(&[("probe", "omit")])).is_err());
        assert!(
            LOWER_UNITY_AMBIENT
                .instance(&o(&[("candela-per-unit", "1")]))
                .is_err()
        );
        assert!(
            LOWER_UNITY_AMBIENT
                .instance(&o(&[("probe", "sky"), ("candela-per-unit", "1")]))
                .is_err()
        );
        assert!(
            LOWER_UNITY_AMBIENT
                .instance(&o(&[("probe", "omit"), ("candela-per-unit", "1"), ("x", "1")]))
                .is_err()
        );
        for bad in ["0", "-1", "nan", "inf", "x", ""] {
            let err = LOWER_UNITY_AMBIENT
                .instance(&o(&[("probe", "omit"), ("candela-per-unit", bad)]))
                .err()
                .unwrap();
            assert!(
                err.to_string().contains("not a finite number above 0"),
                "{bad}: {err}"
            );
        }
        for p in ["omit", "stored-gradient", "probe-average"] {
            let both = o(&[("probe", p), ("candela-per-unit", "1771")]);
            let pass = LOWER_UNITY_AMBIENT.instance(&both).unwrap();
            assert_eq!(pass.options(), both);
        }
    }

    #[test]
    fn the_calibration_scales_the_ambient_and_a_bad_intensity_leaves_it_out() {
        let calibrated = |intensity: f32, c: f32| {
            let mut w = World::new();
            w.insert_resource(UnityRenderSettings {
                sun: None,
                pipeline: None,
                ambient: Some(stored(3, intensity)),
                ..Default::default()
            });
            let mut log = PassLog::default();
            LowerUnityAmbient {
                probe: UnityAmbientProbe::Omit,
                candela_per_unit: c,
            }
            .run(&mut w, &mut log)
            .unwrap();
            (ambient(&w), log)
        };
        let (a, _) = calibrated(2.0, 1771.0);
        assert_eq!(a.unwrap().intensity, 2.0 * 1771.0);
        for (bad, c, why) in [
            (-1.0, 1.0, "ambient intensity negative or not finite, left out"),
            (
                f32::NAN,
                1.0,
                "ambient intensity negative or not finite, left out",
            ),
            (
                f32::MAX,
                2.0,
                "ambient intensity not finite at the calibration, left out",
            ),
        ] {
            let (a, log) = calibrated(bad, c);
            assert!(a.is_none(), "{bad}");
            assert_eq!(log.omitted[why], 1, "{:?}", log.omitted);
        }
    }

    /// A probe's lighting in a direction `n` (x, y, z as Unity's SH basis takes them), evaluated independently of
    /// [`probe_average`]: Unity's packed coefficients back to canonical ones (`UndoCosineRescaling`'s constants, Unity's
    /// Graphics repository), then the real SH basis ("Stupid Spherical Harmonics Tricks" order and signs).
    fn evaluate(probe: &[f32; 27], n: [f64; 3]) -> [f64; 3] {
        let pi = std::f64::consts::PI;
        let (c0, c1, c2) = (0.5 / pi.sqrt(), (3.0 / pi).sqrt() / 3.0, (15.0 / pi).sqrt() / 8.0);
        let (c3, c4) = ((5.0 / pi).sqrt() / 16.0, (15.0 / pi).sqrt() / 16.0);
        let norm = [c0, -c1, c1, -c1, c2, -c2, c3, -c2, c4];
        let [x, y, z] = n;
        let (k1, k2) = (0.5 * (3.0 / pi).sqrt(), 0.5 * (15.0 / pi).sqrt());
        let (k3, k4) = (0.25 * (5.0 / pi).sqrt(), 0.25 * (15.0 / pi).sqrt());
        let basis = [
            c0,
            -k1 * y,
            k1 * z,
            -k1 * x,
            k2 * x * y,
            -k2 * y * z,
            k3 * (3.0 * z * z - 1.0),
            -k2 * x * z,
            k4 * (x * x - y * y),
        ];
        std::array::from_fn(|ch| {
            (0..9)
                .map(|i| f64::from(probe[ch * 9 + i]) / norm[i] * basis[i])
                .sum()
        })
    }

    #[test]
    fn a_probe_lowers_to_its_average_over_directions() {
        // a uniform ambient of colour C is C in each channel's band-0 coefficient (Unity's AddAmbientLight), and lights
        // every direction with C
        let mut uniform = [0.0f32; 27];
        (uniform[0], uniform[9], uniform[18]) = (0.25, 0.5, 0.75);
        for n in [[0.0, 1.0, 0.0], [0.6, 0.0, -0.8], [0.0, 0.0, -1.0]] {
            let e = evaluate(&uniform, n);
            assert!((e[0] - 0.25).abs() < 1e-6 && (e[2] - 0.75).abs() < 1e-6, "{e:?}");
        }
        assert_eq!(probe_average(&uniform), [0.25, 0.5, 0.75]);
        // the stored probe of a real skybox scene: its average over many directions (a Fibonacci sphere) is its band-0
        // coefficients, which Unity's own average (m_IndirectSpecularColor) states too
        let count = 20_000;
        let mut sum = [0.0f64; 3];
        for i in 0..count {
            let y = 1.0 - 2.0 * (f64::from(i) + 0.5) / f64::from(count);
            let r = (1.0 - y * y).sqrt();
            let phi = f64::from(i) * std::f64::consts::PI * (3.0 - 5f64.sqrt());
            let e = evaluate(&SLEDDING_PROBE, [r * phi.cos(), y, r * phi.sin()]);
            for (s, v) in sum.iter_mut().zip(e) {
                *s += v / f64::from(count);
            }
        }
        let average = probe_average(&SLEDDING_PROBE);
        assert_eq!(average, [0.17922863, 0.22442052, 0.30615595]);
        for (s, a) in sum.iter().zip(average) {
            assert!((s - f64::from(a)).abs() < 1e-5, "{sum:?} vs {average:?}");
        }
        // lowered: flat, intensity 1, reported
        for mode in [0, 4] {
            let (w, log) = lowered(mode, 1.0, UnityAmbientProbe::ProbeAverage);
            assert_eq!(
                ambient(&w),
                Some(Ambient {
                    intensity: 1.0,
                    light: AmbientLight::Flat(average),
                })
            );
            assert_eq!(
                log.approximations["ambient probe reduced to its average over directions (its band-0 coefficients, \
                                    factor 1 in Unity's packing); bands 1 and 2 dropped"],
                1
            );
            assert_eq!(log.counts["flat ambient"], 1);
        }
        // a multiplier other than 1 is taken as already in the probe, reported
        let (w, log) = lowered(0, 2.0, UnityAmbientProbe::ProbeAverage);
        assert_eq!(ambient(&w).unwrap().intensity, 1.0);
        assert_eq!(
            log.approximations["ambient intensity multiplier taken as already in the stored probe (not verified against Unity)"],
            1
        );
    }

    #[test]
    fn no_probe_or_an_all_zero_one_is_no_ambient_and_default_colours_are_reported() {
        for probe in [None, Some([0.0f32; 27])] {
            let mut w = World::new();
            w.insert_resource(UnityRenderSettings {
                sun: None,
                pipeline: None,
                ambient: Some(UnityAmbient {
                    probe,
                    ..stored(0, 1.0)
                }),
                ..Default::default()
            });
            let mut log = PassLog::default();
            LowerUnityAmbient {
                probe: UnityAmbientProbe::ProbeAverage,
                candela_per_unit: 1.0,
            }
            .run(&mut w, &mut log)
            .unwrap();
            assert_eq!(ambient(&w), None);
            assert_eq!(
                log.omitted["ambient mode 0 (skybox or custom): the scene stores no ambient probe, or an all-zero one \
                             (never generated)"],
                1
            );
        }
        let mut w = World::new();
        w.insert_resource(UnityRenderSettings {
            sun: None,
            pipeline: None,
            ambient: Some(UnityAmbient {
                mode: 0,
                sky: [0.212, 0.227, 0.259, 1.0],
                equator: [0.114, 0.125, 0.133, 1.0],
                ground: [0.047, 0.043, 0.035, 1.0],
                intensity: 1.0,
                probe: None,
            }),
            ..Default::default()
        });
        let mut log = PassLog::default();
        LowerUnityAmbient {
            probe: UnityAmbientProbe::StoredGradient,
            candela_per_unit: 1.0,
        }
        .run(&mut w, &mut log)
        .unwrap();
        assert!(ambient(&w).is_some());
        assert_eq!(
            log.approximations["the stored sky, equator and ground are Unity's defaults, never set for the scene"],
            1
        );
        // authored colours are not reported as defaults
        let (_, log) = lowered(0, 1.0, UnityAmbientProbe::StoredGradient);
        assert!(!log.approximations.keys().any(|k| k.contains("defaults")));
    }
}
