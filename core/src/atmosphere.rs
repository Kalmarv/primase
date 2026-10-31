//! What a source says about the air between the camera and the scene, and about how the camera sees it
//! (`core.atmosphere`, a world resource beside `core.environment`): fog, exposure, the camera's auto exposure, and a
//! source's volumetric fog in the source's own units (shown, not drawn). A tabled tone curve and colour lookup describe the established display processing.

mod post;
pub use post::{ColourLut, ReinhardCurve, ToneMap};

use serde::Serialize;
use serde_json::Value;

use crate::{JsonError, Resource, World};

/// Fog and exposure as the source authored them; each `None` when the source states none (or a lowering has not
/// established its meaning). A scene-wide fact, kept by every part of a split world as `core.environment`'s ambient is.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Atmosphere {
    /// The display processing after exposure and bloom, when fully established by a source.
    pub tone_map: Option<ToneMap>,
    pub fog: Option<Fog>,
    pub exposure: Option<Exposure>,
    /// A froxel-volume fog as the source stores it, in the source's own units: shown, never drawn (see [`VolumetricFog`]).
    pub volumetric: Option<VolumetricFog>,
    /// The camera's exposure as the source's renderer sets it from the frame it sees ([`AutoExposure`]), in place of
    /// `exposure`, which a source stating this leaves `None`.
    pub auto_exposure: Option<AutoExposure>,
}

/// A source's volumetric (froxel) fog, every field as the source stores it and named as the source names it. The units
/// are the source's: the densities are not established as per metre (the source states a `game_scale`), the colours are
/// not established as linear light at core's scale, and `directional_light_bank` names a light bank of the source, not a
/// core light. Nothing here is converted, so a consumer must not draw it as [`Fog`]: a renderer draws no fog from it
/// until a source has established its unit, and a lowering that states it says so in its notices. A field the source
/// file does not store is `None` (a later file of a map stores only what it changes).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct VolumetricFog {
    pub enabled: Option<bool>,
    pub game_scale: Option<f32>,
    pub near_depth: Option<f32>,
    pub far_depth: Option<f32>,
    pub near_offset: Option<f32>,
    pub fixed_density: Option<f32>,
    pub scattering_g: Option<f32>,
    pub local_light_scale: Option<f32>,
    pub local_light_dist_scale: Option<f32>,
    pub height_fog_distribution: Option<f32>,
    pub height_fog_offset: Option<f32>,
    pub height_fog_density_scale: Option<f32>,
    /// Four floats as stored; the fourth is 0 in every census file of the source we measured.
    pub sky_colour: Option<[f32; 4]>,
    pub sky_start_dist: Option<f32>,
    pub directional_light_bank: Option<u32>,
    pub directional_direct_colour: Option<[f32; 4]>,
    pub directional_opposite_colour: Option<[f32; 4]>,
    pub directional_start_dist: Option<f32>,
}

impl VolumetricFog {
    /// The fields, by the source's name, as stored, for a listing (a colour as its four floats).
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        let f = |v: Option<f32>| v.map(|x| x.to_string());
        let c = |v: Option<[f32; 4]>| v.map(|x| format!("{x:?}"));
        [
            ("Enable", self.enabled.map(|x| x.to_string())),
            ("GameScale", f(self.game_scale)),
            ("NearDepth", f(self.near_depth)),
            ("FarDepth", f(self.far_depth)),
            ("NearOffset", f(self.near_offset)),
            ("Global FixedDensity", f(self.fixed_density)),
            ("ScatteringG", f(self.scattering_g)),
            ("Global LocalLightScale", f(self.local_light_scale)),
            ("Global LocalLightDistScale", f(self.local_light_dist_scale)),
            ("Global HeightFogDistribution", f(self.height_fog_distribution)),
            ("Global HeightFogOffset", f(self.height_fog_offset)),
            ("Global HeightFogDensity Scale", f(self.height_fog_density_scale)),
            ("SkyLight SkyColor", c(self.sky_colour)),
            ("SkyLight StartDist", f(self.sky_start_dist)),
            (
                "DirLight LightBankId",
                self.directional_light_bank.map(|x| x.to_string()),
            ),
            ("DirLight DirectColor", c(self.directional_direct_colour)),
            ("DirLight OppositeColor", c(self.directional_opposite_colour)),
            ("DirLight StartDist", f(self.directional_start_dist)),
        ]
        .into_iter()
        .filter_map(|(name, value)| Some((name, value?)))
        .collect()
    }

    fn check(&self) -> Result<(), String> {
        let scalars = [
            ("GameScale", self.game_scale),
            ("NearDepth", self.near_depth),
            ("FarDepth", self.far_depth),
            ("NearOffset", self.near_offset),
            ("Global FixedDensity", self.fixed_density),
            ("ScatteringG", self.scattering_g),
            ("Global LocalLightScale", self.local_light_scale),
            ("Global LocalLightDistScale", self.local_light_dist_scale),
            ("Global HeightFogDistribution", self.height_fog_distribution),
            ("Global HeightFogOffset", self.height_fog_offset),
            ("Global HeightFogDensity Scale", self.height_fog_density_scale),
            ("SkyLight StartDist", self.sky_start_dist),
            ("DirLight StartDist", self.directional_start_dist),
        ];
        if let Some((name, v)) = scalars
            .into_iter()
            .find_map(|(n, v)| v.filter(|x| !x.is_finite()).map(|x| (n, x)))
        {
            return Err(format!("volumetric fog {name} {v} is not finite"));
        }
        let colours = [
            ("SkyLight SkyColor", self.sky_colour),
            ("DirLight DirectColor", self.directional_direct_colour),
            ("DirLight OppositeColor", self.directional_opposite_colour),
        ];
        if let Some((name, v)) = colours
            .into_iter()
            .find_map(|(n, v)| v.filter(|c| !c.iter().all(|x| x.is_finite())).map(|c| (n, c)))
        {
            return Err(format!("volumetric fog {name} {v:?} is not finite"));
        }
        Ok(())
    }
}

/// Distance fog: the light of a surface at distance `d` metres from the camera is mixed with `colour` by the fraction
/// the [`FogFalloff`] gives along the camera ray. This does not describe scattering towards the sun.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fog {
    /// Linear RGB of the fog where it is fully opaque, at the scale of `core.light` colours times intensity of 1 lux
    /// (a source whose fog colour is relative to its exposure states it here as stored and reports the approximation).
    pub colour: [f32; 3],
    pub falloff: FogFalloff,
}

/// How much of a surface's light the fog replaces, by its distance `d` from the camera in metres.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum FogFalloff {
    /// `1 - exp(-density * d)`; `density` per metre.
    Exponential { density: f32 },
    /// 0 up to `start`, 1 from `end`, linear between; metres, `start < end`.
    Linear { start: f32, end: f32 },
    /// Uniform density inside a horizontal slab, zero outside it. `bottom` and `top` are absolute world Y heights
    /// in metres (`bottom < top`), in core's Y-up frame. Replace by `1 - exp(-density * inside)` where `inside` is
    /// the length in metres of the camera-to-surface segment inside that slab; `density` is per metre. This is
    /// extinction and a constant fog colour, not a height-dependent density gradient or anisotropic scattering.
    ExponentialHeightSlab { density: f32, bottom: f32, top: f32 },
}

/// The camera's exposure: scene light is scaled by `2^stops` before any tone curve. 0 leaves it as it is. This is a
/// relative setting, not a measured camera: a source whose exposure is another quantity (a multiplier, an EV at ISO 100)
/// converts it and reports the approximation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Exposure {
    pub stops: f32,
}

/// The camera's automatic exposure: the frame's light is metered ([`ExposureMeter`]), compared with middle grey, and the
/// exposure moved from `stops` against the difference, by at most `max` stops down and `-min` stops up, approaching
/// those bounds smoothly ([`AutoExposure::correction`]). The scene is shown at `2^(stops - correction)` times its light
/// before any tone curve ([`AutoExposure::scale`]).
///
/// Light is metered in the world's light unit: where `core.directional_light_unit` states one, a frame lit in lux is
/// metered at lux divided by its `lux_per_unit` (the source's own radiance unit, which `middle_grey` is in); where it
/// states none, in lux. A renderer whose frame differs from the source's (another sky, other lights) meters another
/// value, and so settles at another exposure, for that stated reason.
///
/// The metered value adapts over time ([`AutoExposure::adapt`]); a still frame is shown at the adaptation's fixed point,
/// where the adapted value equals the frame's metered one.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AutoExposure {
    /// The exposure, in stops, of a frame metered at middle grey.
    pub stops: f32,
    /// The metered value a frame is shown at `stops` for: a log2 luminance in the world's light unit.
    pub middle_grey: f32,
    /// The most the exposure is lowered for a frame brighter than middle grey, in stops; at least 0.
    pub max: f32,
    /// The most it is raised for a darker frame, as a negative number of stops (the source's minimum); at most 0.
    pub min: f32,
    /// How the frame's light is metered.
    pub meter: ExposureMeter,
    /// How fast the adapted value rises towards a brighter frame's metered value: per `dt` seconds it moves the
    /// fraction `1 - 10^(-speed * dt)` of the way. At least 0.
    pub brighten_speed: f32,
    /// How fast it falls towards a darker frame's, the same way. At least 0.
    pub darken_speed: f32,
    /// The adaptation is held: the exposure is `stops`, whatever the frame.
    pub frozen: bool,
}

/// How a frame's light is metered for [`AutoExposure`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum ExposureMeter {
    /// The centre-weighted mean of log2 luminance. The frame is halved (each pixel of the half-size frame a bilinear
    /// sample at its centre: the mean of four), then cut into a 64 x 64 grid of cells over the frame; each cell takes
    /// 16 bilinear samples of the half-size frame ([`ExposureMeter::SAMPLE_OFFSETS`] from its centre in the frame's 0..1
    /// coordinates, each held under `1 - half a texel`, and to the frame's edge), each sample's luminance
    /// ([`ExposureMeter::LUMINANCE`], 0 where not finite) taken as `log2(Y + 1e-5)`. A cell's value is the mean of its 16;
    /// the frame's is the mean of its cells' values weighted by [`ExposureMeter::weight`] at their centres.
    CentreWeightedLog2 {
        /// How sharply the weight falls from the frame's centre; at least 0 (0 weighs every cell alike).
        centre_weight: f32,
    },
}

impl ExposureMeter {
    /// A cell's 4 x 4 samples' offsets from its centre, in the frame's 0..1 coordinates, x fastest.
    pub const SAMPLE_OFFSETS: [[f32; 2]; 16] = {
        let mut out = [[0.0; 2]; 16];
        let mut i = 0;
        while i < 16 {
            out[i] = [((i % 4) as f32 - 1.5) / 256.0, ((i / 4) as f32 - 1.5) / 256.0];
            i += 1;
        }
        out
    };
    /// What is added to a sample's luminance before its log is taken.
    pub const LUMINANCE_FLOOR: f32 = 1e-5;
    /// The luminance of linear red, green and blue.
    pub const LUMINANCE: [f32; 3] = [0.299, 0.587, 0.114];

    /// The weight of a cell centred at `(u, v)` (0..1 across the frame) of a frame `aspect` wide per unit high:
    /// `1 - (1 - 2^-r)^3`, `r` the length of `((2u - 1) c, (2v - 1) c / aspect)` for the centre weight `c`.
    pub fn weight(&self, u: f32, v: f32, aspect: f32) -> f32 {
        match *self {
            ExposureMeter::CentreWeightedLog2 { centre_weight } => {
                let r = ((2.0 * u - 1.0) * centre_weight).hypot((2.0 * v - 1.0) * centre_weight / aspect);
                1.0 - (1.0 - (-r).exp2()).powi(3)
            }
        }
    }
}

impl AutoExposure {
    /// The stops the exposure is lowered by for a frame metered `x` above middle grey (negative: raised):
    /// `max (1 - e^(-x / max))` for `x > 0`, `min (1 - e^(-x / min))` for `x < 0`, 0 at 0, where that bound is 0, or
    /// where the adaptation is held. It follows `x` with slope 1 near 0 and approaches `max` above and `min` below.
    pub fn correction(&self, x: f32) -> f32 {
        let bound = if x > 0.0 { self.max } else { self.min };
        if self.frozen || x == 0.0 || bound == 0.0 {
            return 0.0;
        }
        bound * (1.0 - (-x / bound).exp())
    }

    /// The factor the scene's light is shown at for a frame whose adapted metered value is `adapted`.
    pub fn scale(&self, adapted: f32) -> f32 {
        (self.stops - self.correction(adapted - self.middle_grey)).exp2()
    }

    /// The adapted value after `dt` seconds at a frame metered `target`, from `adapted`. `adapted == target` is the
    /// fixed point: the value a still frame settles at.
    pub fn adapt(&self, adapted: f32, target: f32, dt: f32) -> f32 {
        let speed = if target > adapted {
            self.brighten_speed
        } else {
            self.darken_speed
        };
        adapted + (target - adapted) * (1.0 - 10f32.powf(-speed * dt))
    }

    fn check(&self) -> Result<(), String> {
        let ExposureMeter::CentreWeightedLog2 { centre_weight } = self.meter;
        for (name, v, ok) in [
            ("stops", self.stops, true),
            ("middle grey", self.middle_grey, true),
            ("max", self.max, self.max >= 0.0),
            ("min", self.min, self.min <= 0.0),
            ("centre weight", centre_weight, centre_weight >= 0.0),
            ("brighten speed", self.brighten_speed, self.brighten_speed >= 0.0),
            ("darken speed", self.darken_speed, self.darken_speed >= 0.0),
        ] {
            if !(v.is_finite() && ok) {
                return Err(format!("auto exposure {name} {v} is out of range"));
            }
        }
        Ok(())
    }
}

impl Atmosphere {
    /// Finite values in range: fog colour and density not negative, a linear fog from a start at least 0 to a later end,
    /// exposure any finite number of stops, an auto exposure's fields as [`AutoExposure`] states them, and not both an
    /// exposure and an auto exposure.
    fn check(&self) -> Result<(), String> {
        if let Some(tone_map) = &self.tone_map {
            tone_map.check()?;
        }
        if let Some(fog) = &self.fog {
            if let Some(c) = (!fog.colour.iter().all(|x| x.is_finite() && *x >= 0.)).then_some(fog.colour) {
                return Err(format!("fog colour {c:?} is not finite and at least 0"));
            }
            match fog.falloff {
                FogFalloff::Exponential { density } if !(density.is_finite() && density >= 0.) => {
                    return Err(format!("fog density {density} is not finite and at least 0"));
                }
                FogFalloff::Linear { start, end }
                    if !(start.is_finite() && end.is_finite() && start >= 0. && start < end) =>
                {
                    return Err(format!(
                        "linear fog from {start} to {end} is not finite, from at least 0 to a later end"
                    ));
                }
                FogFalloff::ExponentialHeightSlab { density, bottom, top }
                    if !(density.is_finite()
                        && density >= 0.
                        && bottom.is_finite()
                        && top.is_finite()
                        && bottom < top) =>
                {
                    return Err(format!(
                        "fog slab density {density}, bottom {bottom}, top {top} requires finite nonnegative density and bottom < top"
                    ));
                }
                _ => {}
            }
        }
        if let Some(e) = self.exposure.as_ref().filter(|e| !e.stops.is_finite()) {
            return Err(format!("exposure {} stops is not finite", e.stops));
        }
        if let Some(auto) = &self.auto_exposure {
            if self.exposure.is_some() {
                return Err("both an exposure and an auto exposure".to_owned());
            }
            auto.check()?;
        }
        self.volumetric.as_ref().map_or(Ok(()), VolumetricFog::check)
    }
}

impl Resource for Atmosphere {
    const NAME: &'static str = "core.atmosphere";
    const VERSION: u32 = 5;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _w: &World) -> Result<(), String> {
        self.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn fog(falloff: FogFalloff) -> Atmosphere {
        Atmosphere {
            fog: Some(Fog {
                colour: [0.5, 0.6, 0.7],
                falloff,
            }),
            exposure: Some(Exposure { stops: -1.5 }),
            volumetric: None,
            auto_exposure: None,
            tone_map: None,
        }
    }

    /// erK1's Tone Map at clock 13.06 (the live block the game's sampler filled), centre weight and speeds as m10_00_0000
    /// stores them.
    fn noon() -> AutoExposure {
        AutoExposure {
            stops: 3.4042,
            middle_grey: -6.4,
            max: 0.4,
            min: -0.8933,
            meter: ExposureMeter::CentreWeightedLog2 { centre_weight: 7.0 },
            brighten_speed: 0.2,
            darken_speed: 0.2,
            frozen: false,
        }
    }

    #[test]
    fn a_valid_atmosphere_is_kept_by_every_part_and_shown_as_json() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert_resource(fog(FogFalloff::Linear {
            start: 10.,
            end: 400.,
        }));
        assert!(w.verify_resources().is_empty());
        let part = w.subset(&BTreeSet::from([e]));
        assert_eq!(part.resource::<Atmosphere>(), w.resource::<Atmosphere>());
        assert_eq!(
            w.resource::<Atmosphere>().unwrap().to_json().unwrap(),
            Some(serde_json::json!({
                "fog": {"colour": [0.5, 0.6000000238418579, 0.699999988079071],
                        "falloff": {"Linear": {"start": 10.0, "end": 400.0}}},
                "exposure": {"stops": -1.5},
                "volumetric": null,
                "auto_exposure": null, "tone_map": null
            }))
        );
        assert_eq!(
            Atmosphere::default().to_json().unwrap(),
            Some(
                serde_json::json!({"fog": null, "exposure": null, "volumetric": null, "auto_exposure": null, "tone_map": null})
            )
        );
    }

    #[test]
    fn a_volumetric_fog_keeps_the_sources_fields_unconverted_and_lists_what_it_states() {
        let volumetric = VolumetricFog {
            game_scale: Some(12.),
            fixed_density: Some(0.004),
            sky_colour: Some([0.062745, 0.070588, 0.078431, 0.]),
            directional_light_bank: Some(111010000),
            ..Default::default()
        };
        let a = Atmosphere {
            volumetric: Some(volumetric.clone()),
            ..Default::default()
        };
        let mut w = World::new();
        let e = w.spawn();
        w.insert_resource(a);
        assert!(w.verify_resources().is_empty());
        assert_eq!(
            w.subset(&BTreeSet::from([e])).resource::<Atmosphere>(),
            w.resource::<Atmosphere>()
        );
        assert_eq!(
            volumetric.fields(),
            [
                ("GameScale", "12".to_owned()),
                ("Global FixedDensity", "0.004".to_owned()),
                (
                    "SkyLight SkyColor",
                    "[0.062745, 0.070588, 0.078431, 0.0]".to_owned()
                ),
                ("DirLight LightBankId", "111010000".to_owned()),
            ]
        );
        let json = w.resource::<Atmosphere>().unwrap().to_json().unwrap().unwrap();
        assert_eq!(json["volumetric"]["game_scale"], serde_json::json!(12.0));
        assert!(json["volumetric"]["far_depth"].is_null());
        for bad in [
            VolumetricFog {
                fixed_density: Some(f32::NAN),
                ..Default::default()
            },
            VolumetricFog {
                sky_colour: Some([0., f32::INFINITY, 0., 0.]),
                ..Default::default()
            },
        ] {
            w.insert_resource(Atmosphere {
                volumetric: Some(bad),
                ..Default::default()
            });
            assert_eq!(w.verify_resources().len(), 1);
        }
    }

    #[test]
    fn values_out_of_range_are_refused_by_the_verifier() {
        let mut w = World::new();
        for (a, bad) in [
            (
                fog(FogFalloff::Exponential { density: -0.1 }),
                "fog density -0.1 is not finite and at least 0",
            ),
            (
                fog(FogFalloff::Exponential { density: f32::NAN }),
                "fog density NaN is not finite and at least 0",
            ),
            (
                fog(FogFalloff::Linear { start: 5., end: 5. }),
                "linear fog from 5 to 5 is not finite, from at least 0 to a later end",
            ),
            (
                fog(FogFalloff::Linear { start: -1., end: 5. }),
                "linear fog from -1 to 5 is not finite, from at least 0 to a later end",
            ),
            (
                Atmosphere {
                    fog: Some(Fog {
                        colour: [0.1, -0.1, 0.1],
                        falloff: FogFalloff::Exponential { density: 0.01 },
                    }),
                    exposure: None,
                    volumetric: None,
                    auto_exposure: None,
                    tone_map: None,
                },
                "fog colour [0.1, -0.1, 0.1] is not finite and at least 0",
            ),
            (
                Atmosphere {
                    fog: None,
                    exposure: Some(Exposure { stops: f32::INFINITY }),
                    volumetric: None,
                    auto_exposure: None,
                    tone_map: None,
                },
                "exposure inf stops is not finite",
            ),
        ] {
            w.insert_resource(a);
            assert_eq!(w.verify_resources(), [("core.atmosphere", bad.to_owned())]);
        }
        w.insert_resource(Atmosphere::default());
        assert!(w.verify_resources().is_empty());
    }

    /// The game's own readback is the oracle: erK1 frame 42768 showed its scene at `2^3.6826` (cbToneMap
    /// g_ToneMapSceneLumScale 12.8402), from the adapted value written four frames before (its readback ring), 0.3336
    /// below middle grey, at the Tone Map above.
    #[test]
    fn the_exposure_of_a_frame_of_the_game_is_its_stops_less_the_soft_clamped_metering() {
        let auto = noon();
        let shown = auto.scale(auto.middle_grey - 0.3336).log2();
        assert!((shown - 3.6826).abs() < 2e-4, "{shown}");
        // darker than middle grey raises it, towards -min stops and never past them (in f32 it reaches them)
        assert!(auto.correction(-0.3336) < 0.0);
        let near = auto.correction(-3.0);
        assert!(near > auto.min && near < auto.min + 0.05, "{near}");
        let far = auto.correction(-50.0);
        assert!(far >= auto.min && far - auto.min < 1e-6, "{far}");
        // brighter lowers it towards max the same way
        let near = auto.correction(3.0);
        assert!(near < auto.max && near > auto.max - 0.05, "{near}");
        let far = auto.correction(50.0);
        assert!(far <= auto.max && auto.max - far < 1e-6, "{far}");
        // slope 1 through middle grey, from both sides
        for x in [1e-3f32, -1e-3] {
            assert!((auto.correction(x) / x - 1.0).abs() < 3e-3, "{x}");
        }
        assert_eq!(auto.correction(0.0), 0.0);
        // held: the stops alone
        let frozen = AutoExposure {
            frozen: true,
            ..noon()
        };
        assert_eq!(frozen.scale(-20.0), 3.4042f32.exp2());
        // a bound of 0: no correction that way
        let flat = AutoExposure { max: 0.0, ..noon() };
        assert_eq!(flat.correction(2.0), 0.0);
    }

    #[test]
    fn the_adapted_value_moves_by_its_speed_and_rests_at_the_frames_metering() {
        let auto = AutoExposure {
            brighten_speed: 0.2,
            darken_speed: 0.5,
            ..noon()
        };
        // a still frame's metering is the fixed point
        assert_eq!(auto.adapt(-6.0, -6.0, 1.0 / 30.0), -6.0);
        // per second at speed 0.2 the fraction 1 - 10^-0.2 of the way up, at 0.5 the fraction 1 - 10^-0.5 down
        let up = auto.adapt(-8.0, -6.0, 1.0);
        assert!((up - (-8.0 + 2.0 * (1.0 - 0.630_957_3))).abs() < 1e-5, "{up}");
        let down = auto.adapt(-6.0, -8.0, 1.0);
        assert!((down - (-6.0 - 2.0 * (1.0 - 0.316_227_8))).abs() < 1e-5, "{down}");
        // iterated it approaches the frame's metering and does not pass it
        let mut a = -10.0f32;
        for _ in 0..2000 {
            let next = auto.adapt(a, -6.0, 1.0 / 30.0);
            assert!(next >= a && next <= -6.0);
            a = next;
        }
        assert!((a + 6.0).abs() < 1e-4, "{a}");
    }

    #[test]
    fn the_meter_weighs_the_centre_most_and_samples_a_four_by_four_grid_per_cell() {
        let meter = ExposureMeter::CentreWeightedLog2 { centre_weight: 7.0 };
        assert_eq!(meter.weight(0.5, 0.5, 16.0 / 9.0), 1.0);
        // the game's 1280 x 720 constants: (7, 3.9375) across the frame's two axes, so a cell a quarter of the way
        // across weighs 1 - (1 - 2^-3.5)^3
        let quarter = meter.weight(0.25, 0.5, 1280.0 / 720.0);
        assert!(
            (quarter - (1.0 - (1.0 - 2f32.powf(-3.5)).powi(3))).abs() < 1e-6,
            "{quarter}"
        );
        let vertical = meter.weight(0.5, 0.25, 1280.0 / 720.0);
        assert!(
            (vertical - (1.0 - (1.0 - 2f32.powf(-3.9375 / 2.0)).powi(3))).abs() < 1e-6,
            "{vertical}"
        );
        assert!(meter.weight(0.0, 0.0, 1.0) < 0.01);
        let flat = ExposureMeter::CentreWeightedLog2 { centre_weight: 0.0 };
        assert_eq!(flat.weight(0.0, 1.0, 1.0), 1.0);
        // the game's cbToneMapSampleOffset of the 64 x 64 pass (frame 7773): +-0.005859, +-0.001953, x fastest
        let o = ExposureMeter::SAMPLE_OFFSETS;
        assert_eq!(o[0], [-1.5 / 256.0, -1.5 / 256.0]);
        assert_eq!(o[1], [-0.5 / 256.0, -1.5 / 256.0]);
        assert_eq!(o[15], [1.5 / 256.0, 1.5 / 256.0]);
        assert!((o[0][0] + 0.005859).abs() < 1e-6 && (o[1][0] + 0.001953).abs() < 1e-6);
    }

    #[test]
    fn an_auto_exposure_out_of_range_or_beside_an_exposure_is_refused() {
        let mut w = World::new();
        for (auto, exposure, bad) in [
            (
                AutoExposure { max: -0.1, ..noon() },
                None,
                "auto exposure max -0.1 is out of range",
            ),
            (
                AutoExposure { min: 0.5, ..noon() },
                None,
                "auto exposure min 0.5 is out of range",
            ),
            (
                AutoExposure {
                    stops: f32::NAN,
                    ..noon()
                },
                None,
                "auto exposure stops NaN is out of range",
            ),
            (
                AutoExposure {
                    darken_speed: -1.0,
                    ..noon()
                },
                None,
                "auto exposure darken speed -1 is out of range",
            ),
            (
                AutoExposure {
                    meter: ExposureMeter::CentreWeightedLog2 { centre_weight: -1.0 },
                    ..noon()
                },
                None,
                "auto exposure centre weight -1 is out of range",
            ),
            (
                noon(),
                Some(Exposure { stops: 1.0 }),
                "both an exposure and an auto exposure",
            ),
        ] {
            w.insert_resource(Atmosphere {
                exposure,
                auto_exposure: Some(auto),
                ..Default::default()
            });
            assert_eq!(w.verify_resources(), [("core.atmosphere", bad.to_owned())]);
        }
        w.insert_resource(Atmosphere {
            auto_exposure: Some(noon()),
            ..Default::default()
        });
        assert!(w.verify_resources().is_empty());
        let json = w.resource::<Atmosphere>().unwrap().to_json().unwrap().unwrap();
        assert_eq!(
            json["auto_exposure"]["meter"]["CentreWeightedLog2"]["centre_weight"],
            serde_json::json!(7.0)
        );
    }

    #[test]
    fn height_slab_uses_absolute_heights_including_below_the_origin() {
        assert!(
            fog(FogFalloff::ExponentialHeightSlab {
                density: 0.01,
                bottom: -40.,
                top: 200.
            })
            .check()
            .is_ok()
        );
        for (density, bottom, top) in [(0.01, 2., 2.), (-0.01, 0., 2.), (0.01, f32::NAN, 2.)] {
            assert!(
                fog(FogFalloff::ExponentialHeightSlab { density, bottom, top })
                    .check()
                    .is_err()
            );
        }
    }
}
