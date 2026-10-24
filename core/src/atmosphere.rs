//! What a source says about the air between the camera and the scene, and about how the camera sees it
//! (`core.atmosphere`, a world resource beside `core.environment`): fog and exposure. Sky, scattering and tone curves are
//! not here until their meaning and units are established for a source.

use serde::Serialize;
use serde_json::Value;

use crate::{JsonError, Resource, World};

/// Fog and exposure as the source authored them; each `None` when the source states none (or a lowering has not
/// established its meaning). A scene-wide fact, kept by every part of a split world as `core.environment`'s ambient is.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Atmosphere {
    pub fog: Option<Fog>,
    pub exposure: Option<Exposure>,
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
                    if !(density.is_finite() && density >= 0. && bottom.is_finite() && top.is_finite() && bottom < top) =>
                {
                    return Err(format!("fog slab density {density}, bottom {bottom}, top {top} requires finite nonnegative density and bottom < top"));
                }
                _ => {}
            }
        }
        match &self.exposure {
            Some(e) if !e.stops.is_finite() => Err(format!("exposure {} stops is not finite", e.stops)),
            _ => Ok(()),
        }
    }
}

impl Resource for Atmosphere {
    const NAME: &'static str = "core.atmosphere";
    const VERSION: u32 = 1;
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
        }
    }

    #[test]
    fn a_valid_atmosphere_is_kept_by_every_part_and_shown_as_json() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert_resource(fog(FogFalloff::Linear { start: 10., end: 400. }));
        assert!(w.verify_resources().is_empty());
        let part = w.subset(&BTreeSet::from([e]));
        assert_eq!(part.resource::<Atmosphere>(), w.resource::<Atmosphere>());
        assert_eq!(
            w.resource::<Atmosphere>().unwrap().to_json().unwrap(),
            Some(serde_json::json!({
                "fog": {"colour": [0.5, 0.6000000238418579, 0.699999988079071],
                        "falloff": {"Linear": {"start": 10.0, "end": 400.0}}},
                "exposure": {"stops": -1.5}
            }))
        );
        assert_eq!(Atmosphere::default().to_json().unwrap(), Some(serde_json::json!({"fog": null, "exposure": null})));
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
                },
                "fog colour [0.1, -0.1, 0.1] is not finite and at least 0",
            ),
            (
                Atmosphere {
                    fog: None,
                    exposure: Some(Exposure { stops: f32::INFINITY }),
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
        assert!(fog(FogFalloff::ExponentialHeightSlab { density: 0.01, bottom: -40., top: 200. }).check().is_ok());
        for (density, bottom, top) in [(0.01, 2., 2.), (-0.01, 0., 2.), (0.01, f32::NAN, 2.)] {
            assert!(fog(FogFalloff::ExponentialHeightSlab { density, bottom, top }).check().is_err());
        }
    }
}
