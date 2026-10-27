//! What a source says about the air between the camera and the scene, and about how the camera sees it
//! (`core.atmosphere`, a world resource beside `core.environment`): fog and exposure, and a source's volumetric fog in the
//! source's own units (shown, not drawn). Sky, scattering and tone curves are not here until their meaning and units are
//! established for a source.

use serde::Serialize;
use serde_json::Value;

use crate::{JsonError, Resource, World};

/// Fog and exposure as the source authored them; each `None` when the source states none (or a lowering has not
/// established its meaning). A scene-wide fact, kept by every part of a split world as `core.environment`'s ambient is.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Atmosphere {
    pub fog: Option<Fog>,
    pub exposure: Option<Exposure>,
    /// A froxel-volume fog as the source stores it, in the source's own units: shown, never drawn (see [`VolumetricFog`]).
    pub volumetric: Option<VolumetricFog>,
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

impl Atmosphere {
    /// Finite values in range: fog colour and density not negative, a linear fog from a start at least 0 to a later end,
    /// exposure any finite number of stops.
    fn check(&self) -> Result<(), String> {
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
        self.volumetric.as_ref().map_or(Ok(()), VolumetricFog::check)
    }
}

impl Resource for Atmosphere {
    const NAME: &'static str = "core.atmosphere";
    const VERSION: u32 = 2;
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
                "volumetric": null
            }))
        );
        assert_eq!(
            Atmosphere::default().to_json().unwrap(),
            Some(serde_json::json!({"fog": null, "exposure": null, "volumetric": null}))
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
                },
                "fog colour [0.1, -0.1, 0.1] is not finite and at least 0",
            ),
            (
                Atmosphere {
                    fog: None,
                    exposure: Some(Exposure { stops: f32::INFINITY }),
                    volumetric: None,
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
