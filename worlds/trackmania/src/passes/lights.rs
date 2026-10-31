//! `lower-trackmania-lights{lux-per-unit}`: every placement's lights (`trackmania.lights` on its prototype, the light
//! trees of its solid) as placed `core.light` entities. Each light is declared as its placement's content
//! ([`PassLog::place_from`]), so a conversion keeps it with the part that owns the placement.
//!
//! Per light: an entity with `core.transform`, `core.light`, `core.name` (the light tree's) and `core.source`.
//! - Where: the placement's transform times the light tree's location. A TrackMania light shines along its tree's
//!   local +Z: in the Stadium blocks every spot's glow sprite (`StadiumWarpSpotsGlow`, 82 of 82 in A01-Race and
//!   Turbulence) faces +Z, as do the lamps' lenses (`*Spots` materials). The transform faces that way
//!   ([`Transform::facing`]: its local -Z along it, the core convention).
//! - Kind: a ball light is a point; a spot is a spot with its stored angles halved (the game stores the cone's full
//!   apex angles in degrees, inferred: 100 and 120 can only be full angles; the outer half-angle at most 89.5°).
//!   Frustum and directional lights have no core form here: left out, counted.
//! - Colour: the stored colour (0-1, the game's own gamma-space value, inferred: every one read is a byte over 255) in
//!   linear light.
//! - Intensity: the game's ball light falls off as `intensity / (1 + attenuation2 d^2)` (its stored linear term is 0
//!   on every light read); its far field is an inverse square of `intensity / attenuation2`, so the candela are that
//!   times the required `lux-per-unit`, the caller's calibration of the game's unit (no mood stores a sun intensity
//!   to measure it by). A light with no quadratic term is left out, counted.
//! - Range: the ball's radius times the placement's scale; emitter radius: its emitting radius when above 0.
//!
//! What the light's flags select (they differ between lights) is not read.
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Instance, Light, LightKind, Linear, Name, Source, Transform, World};

use crate::components::{TrackmaniaLight, TrackmaniaLightKind, TrackmaniaLights};

/// The largest spot half-angle made, in degrees.
const MAX_HALF_ANGLE: f32 = 89.5;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerTrackmaniaLights {
    /// Lux per unit of the game's light intensity (option `lux-per-unit`, required, finite and above 0).
    pub lux_per_unit: f32,
}

impl LowerTrackmaniaLights {
    fn from_given(g: &Given) -> Result<LowerTrackmaniaLights, String> {
        let v = g.required("lux-per-unit");
        let lux_per_unit = v
            .parse::<f32>()
            .ok()
            .filter(|c| c.is_finite() && *c > 0.0)
            .ok_or_else(|| format!("lux-per-unit: {v} is not a finite number above 0"))?;
        Ok(LowerTrackmaniaLights { lux_per_unit })
    }

    /// The core light of `l` at a placement of scale `scale`, or why it has none.
    fn light(&self, l: &TrackmaniaLight, scale: f32) -> Result<Light, &'static str> {
        let kind = match l.kind {
            TrackmaniaLightKind::Ball => LightKind::Point,
            TrackmaniaLightKind::Spot => {
                let s = l
                    .spot
                    .as_ref()
                    .ok_or("spot lights without their angles left out")?;
                let outer = (s.outer.max(s.inner) / 2.0)
                    .clamp(0.0, MAX_HALF_ANGLE)
                    .to_radians();
                let inner = (s.inner.min(s.outer) / 2.0)
                    .clamp(0.0, MAX_HALF_ANGLE)
                    .to_radians();
                LightKind::Spot {
                    inner_cone: inner.min(outer),
                    outer_cone: outer,
                }
            }
            TrackmaniaLightKind::Frustum => return Err("frustum lights left out (no core form)"),
            TrackmaniaLightKind::Directional => return Err("directional lights left out"),
        };
        let common = l
            .common
            .as_ref()
            .ok_or("lights without their colour and intensity left out")?;
        let ball = l
            .ball
            .as_ref()
            .ok_or("lights without their attenuation left out")?;
        // NaN refused with the rest
        if ball.attenuation2.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err("lights with no quadratic attenuation left out");
        }
        let light = Light {
            kind,
            color: common
                .color
                .map(|c| primase_foundations::color::srgb_to_linear_f32(c.clamp(0.0, 1.0))),
            intensity: common.intensity.max(0.0) / ball.attenuation2 * self.lux_per_unit,
            range: Some((ball.radius * scale).max(0.5)),
            cast_shadows: None,
            radius: (ball.emitting_radius > 0.0).then_some(ball.emitting_radius * scale),
            specular: None,
        };
        let finite = light.intensity.is_finite()
            && light.color.iter().all(|c| c.is_finite())
            && light.range.is_none_or(f32::is_finite)
            && light.radius.is_none_or(f32::is_finite);
        if finite {
            Ok(light)
        } else {
            Err("lights with values that are not finite left out")
        }
    }
}

/// `lower-trackmania-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): the calibration is the caller's.
pub const LOWER_TRACKMANIA_LIGHTS: PassSpec = PassSpec {
    name: "lower-trackmania-lights",
    options: &[OptionSpec::required(
        "lux-per-unit",
        "100000",
        "lux per unit of the game's light intensity, finite and above 0: the caller's calibration",
    )],
    make: |g| Ok(Box::new(LowerTrackmaniaLights::from_given(g)?)),
};

impl Pass for LowerTrackmaniaLights {
    fn name(&self) -> &'static str {
        "lower-trackmania-lights"
    }
    fn options(&self) -> Options {
        Options::from([("lux-per-unit".to_owned(), self.lux_per_unit.to_string())])
    }
    fn requires(&self) -> &[&'static str] {
        &[TrackmaniaLights::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Light::NAME, Transform::NAME, Name::NAME, Source::NAME]
    }
    fn approximates(&self) -> &'static str {
        "the game's light intensity as candela at the caller's lux-per-unit, its 1 / (1 + a d^2) falloff as an \
         inverse square with its radius as the range"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut made: Vec<(Entity, Transform, Light, String, String)> = Vec::new();
        for (e, i) in w.iter::<Instance>() {
            let Some(lights) = w.get::<TrackmaniaLights>(i.prototype) else {
                continue;
            };
            let t = w.get::<Transform>(e).ok_or("a placement without a transform")?;
            let linear = Linear::of(t);
            let scale = (0..3)
                .map(|c| t.cols[c].iter().map(|v| v * v).sum::<f32>().sqrt())
                .fold(0f32, f32::max);
            let source = w.get::<Source>(e).map(|s| s.0.clone()).unwrap_or_default();
            for (k, l) in lights.0.iter().enumerate() {
                let light = match self.light(l, scale) {
                    Ok(light) => light,
                    Err(why) => {
                        log.omit(why, 1);
                        continue;
                    }
                };
                let at = t.apply(l.location.cols[3]);
                // the game's light shines along its tree's +Z
                let along = linear.point(l.location.cols[2]);
                let Some(transform) = Transform::facing(at, along) else {
                    log.omit("lights with no direction left out", 1);
                    continue;
                };
                log.approximate("light intensity as candela at the caller's lux-per-unit", 1);
                let name = l.tree.clone().unwrap_or_else(|| format!("light {k}"));
                made.push((e, transform, light, name, format!("{source}/light {k}")));
            }
        }
        log.count("lights placed", made.len() as u64);
        for (placement, t, l, n, s) in made {
            let e = w.spawn();
            // the light is its placement's content: a conversion keeps it with the part that owns the placement
            log.place_from(e, placement);
            w.insert(e, t);
            w.insert(e, l);
            w.insert(e, Name(n));
            w.insert(e, Source(s));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use primase_core::pass::{Pass, PassLog};
    use primase_core::{Instance, Light, LightKind, Transform, World};

    use super::LowerTrackmaniaLights;
    use crate::components::{
        TrackmaniaBallProperties, TrackmaniaLight, TrackmaniaLightKind, TrackmaniaLightProperties,
        TrackmaniaLights, TrackmaniaSpotProperties,
    };

    fn spot(location: Transform) -> TrackmaniaLight {
        TrackmaniaLight {
            tree: Some("Startspot01".into()),
            location,
            kind: TrackmaniaLightKind::Spot,
            common: Some(TrackmaniaLightProperties {
                color: [1.0, 0.5, 0.0],
                flags: 121,
                intensity: 0.5,
                diffuse_intensity: 1.0,
                specular_intensity: 1.0,
                specular_power: 1.0,
                shadow_intensity: 0.4,
                flare_intensity: 0.3,
                shadow_rgb: [1.0; 3],
            }),
            ball: Some(TrackmaniaBallProperties {
                flags: 15,
                radius: 32.0,
                radius_specular: 32.0,
                radius_shadow: 32.0,
                radius_flare: 320.0,
                emitting_radius: 0.0,
                attenuation1: 0.0,
                attenuation2: 0.01,
                ambient_rgb: [0.0; 3],
            }),
            spot: Some(TrackmaniaSpotProperties {
                inner: 100.0,
                outer: 120.0,
                falloff_exponent: 1.0,
            }),
        }
    }

    /// A spot 10 m up its block, its tree turned so its +Z points down, in a block placed 32 m along +X: one light at
    /// (32, 10, 0) shining down (its transform's -Z down), its cone the stored full angles halved, its candela the
    /// far field of `intensity / (1 + a d^2)` at the calibration, its range the radius.
    #[test]
    fn a_spot_is_placed_shining_along_its_trees_z() {
        let mut w = World::new();
        let proto = w.spawn();
        // +Z down: x stays, y -> +z, z -> -y
        let down = Transform {
            cols: [
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, -1.0, 0.0],
                [0.0, 10.0, 0.0],
            ],
        };
        w.insert(proto, TrackmaniaLights(vec![spot(down)]));
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(
            placement,
            Transform {
                cols: [
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [32.0, 0.0, 0.0],
                ],
            },
        );
        let pass = LowerTrackmaniaLights { lux_per_unit: 1000.0 };
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        let lights: Vec<_> = w.iter::<Light>().map(|(e, l)| (e, l.clone())).collect();
        assert_eq!(lights.len(), 1);
        let (e, l) = &lights[0];
        let t = w.get::<Transform>(*e).unwrap();
        assert_eq!(t.cols[3], [32.0, 10.0, 0.0]);
        let minus_z = t.cols[2].map(|v| -v);
        assert!((minus_z[1] + 1.0).abs() < 1e-6, "{minus_z:?}");
        match l.kind {
            LightKind::Spot {
                inner_cone,
                outer_cone,
            } => {
                assert!((inner_cone - 50f32.to_radians()).abs() < 1e-6);
                assert!((outer_cone - 60f32.to_radians()).abs() < 1e-6);
            }
            other => panic!("{other:?}"),
        }
        assert!((l.intensity - 0.5 / 0.01 * 1000.0).abs() < 1e-2);
        assert_eq!(l.range, Some(32.0));
        assert_eq!(l.color[0], 1.0);
        assert!(l.color[1] > 0.2 && l.color[1] < 0.22, "{:?}", l.color);
        assert_eq!(log.counts.get("lights placed"), Some(&1));
    }

    /// A light the core has no form for, or with no quadratic falloff, is left out and counted, not guessed.
    #[test]
    fn lights_with_no_core_form_are_left_out_counted() {
        let mut w = World::new();
        let proto = w.spawn();
        let mut frustum = spot(Transform::IDENTITY);
        frustum.kind = TrackmaniaLightKind::Frustum;
        let mut flat = spot(Transform::IDENTITY);
        flat.ball.as_mut().unwrap().attenuation2 = 0.0;
        w.insert(proto, TrackmaniaLights(vec![frustum, flat]));
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(placement, Transform::IDENTITY);
        let mut log = PassLog::default();
        LowerTrackmaniaLights { lux_per_unit: 1.0 }
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(w.iter::<Light>().count(), 0);
        assert_eq!(
            log.omitted.get("frustum lights left out (no core form)"),
            Some(&1)
        );
        assert_eq!(
            log.omitted.get("lights with no quadratic attenuation left out"),
            Some(&1)
        );
    }
}
