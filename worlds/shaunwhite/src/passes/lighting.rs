//! Explicit interpretation of the source's relative light units, colour encoding and spot/sun directions. The
//! game's falloff, cookies, flares, activation states and character-only lighting are not the punctual core model.
//!
//! The light unit is the game's own, shared by the sun and the local lights: its deferred lighting multiplies each
//! light's colour and intensity by N.L (and for a local light a linear `(far - d) / (far - near)` window) with no
//! other factor in the source (`eklighting.hlsl`, `ComputeOmniLightAttenuation`; the sun's constants are the same
//! colour x intensity). So the level's local lights and its sun compare in one scale, and `point-scale=sun:LUX` takes
//! that scale from the level's own sun: the sun's luminance (its colour's, in the chosen encoding) x its stored
//! intensity is taken as LUX lux (the noon sun, 100,000), which makes one stored unit `LUX / (intensity x luminance)`
//! lux, and a local light's candela its stored intensity x that, its illuminance at 1 m where its window is about 1.
//! A bare number stays an uncalibrated candela per unit, and the sun keeps its own `sun-scale`.
//!
//! The sky's `AmbientColor` is lowered as `core.environment`'s flat ambient, in the sun's unit (one stored unit is the
//! lux of a stored sun intensity of 1). The game's static world is lit by baked lightmaps and per-vertex lighting
//! whose decode is `value x scale + bias`, and the bias of every one of the six levels' bakes is that same
//! `AmbientColor` (the composite spherical-harmonic path is floored at it: `max(SH, g_AmbientColor)`): it is the light
//! a surface the bake leaves dark still has. The bake itself, which adds the baked sun, shadows, occlusion and local
//! light pools to it, is not carried, and a surface whose baked value is near 0 (a tower's shadowed faces, a tunnel
//! the bake leaves dark) has the ambient alone, as in the game. The colour's alpha (the uninfluenced level) is not used.
use crate::components::{LightSource, SkySettings};
use anvil_formats::shaunwhite::light::LightShape;
use primase_core::frame::SourceFrame;
use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Ambient, AmbientLight, Component, Entity, Environment, Light, LightKind, Transform, World,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotAxis {
    X,
    NegativeX,
    Y,
    NegativeY,
    Z,
    NegativeZ,
}
impl Choice for SpotAxis {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("x", Self::X),
        ("-x", Self::NegativeX),
        ("y", Self::Y),
        ("-y", Self::NegativeY),
        ("z", Self::Z),
        ("-z", Self::NegativeZ),
    ];
}
impl SpotAxis {
    fn vector(self) -> [f32; 3] {
        match self {
            Self::X => [1., 0., 0.],
            Self::NegativeX => [-1., 0., 0.],
            Self::Y => [0., 1., 0.],
            Self::NegativeY => [0., -1., 0.],
            Self::Z => [0., 0., 1.],
            Self::NegativeZ => [0., 0., -1.],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunVector {
    ToSun,
    FromSun,
}
impl Choice for SunVector {
    const VALUES: &'static [(&'static str, Self)] = &[("to-sun", Self::ToSun), ("from-sun", Self::FromSun)];
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightColour {
    Linear,
    Srgb,
}
impl Choice for LightColour {
    const VALUES: &'static [(&'static str, Self)] = &[("linear", Self::Linear), ("srgb", Self::Srgb)];
}

/// What one stored local-light intensity unit is, as `point-scale` states it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointScale {
    /// Candela per unit, a number of the caller's (the sun keeps its own `sun-scale`).
    Candela(f32),
    /// The level's sun taken as this many lux: the unit is `lux / (sun intensity x sun luminance)` for the local
    /// lights and the sun alike (`sun-scale` is not used).
    SunLux(f32),
}
impl PointScale {
    fn parse(text: &str) -> Result<PointScale, String> {
        let number = |v: &str| -> Result<f32, String> {
            let n: f32 = v
                .parse()
                .map_err(|_| format!("point-scale must be a number or sun:LUX, not {text}"))?;
            if n.is_finite() && n >= 0. {
                Ok(n)
            } else {
                Err("point-scale must be finite and nonnegative".into())
            }
        };
        match text.strip_prefix("sun:") {
            Some(lux) => match number(lux)? {
                n if n > 0. => Ok(PointScale::SunLux(n)),
                _ => Err("point-scale=sun:LUX needs LUX above 0".into()),
            },
            None => number(text).map(PointScale::Candela),
        }
    }
    fn text(self) -> String {
        match self {
            PointScale::Candela(c) => c.to_string(),
            PointScale::SunLux(l) => format!("sun:{l}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LowerShaunwhiteLighting {
    pub point_scale: PointScale,
    pub sun_scale: f32,
    pub spot_axis: SpotAxis,
    pub sun_vector: SunVector,
    pub colour: LightColour,
}
pub const LOWER_SHAUNWHITE_LIGHTING: PassSpec = PassSpec {
    name: "lower-shaunwhite-lighting",
    options: &[
        OptionSpec::required(
            "point-scale",
            "100",
            "candela per stored local-light intensity unit (uncalibrated), or sun:LUX: the level's sun taken as LUX lux \
             (100000 at noon) fixes the unit for the local lights and the sun alike",
        ),
        OptionSpec::required(
            "sun-scale",
            "1",
            "lux per stored directional intensity unit (uncalibrated)",
        ),
        OptionSpec::choice::<SpotAxis>(
            "spot-axis",
            "assumed source-local spot forward axis; sign/axis not runtime verified",
        ),
        OptionSpec::choice::<SunVector>(
            "sun-vector",
            "whether the stored world vector points toward or away from the sun",
        ),
        OptionSpec::choice::<LightColour>(
            "colour",
            "interpret stored RGB as linear light or sRGB (encoding unverified)",
        ),
    ],
    make: |g| {
        let n = |key| -> Result<f32, String> {
            let v: f32 = g
                .required(key)
                .parse()
                .map_err(|_| format!("{key} must be a number"))?;
            if v.is_finite() && v >= 0. {
                Ok(v)
            } else {
                Err(format!("{key} must be finite and nonnegative"))
            }
        };
        Ok(Box::new(LowerShaunwhiteLighting {
            point_scale: PointScale::parse(g.required("point-scale"))?,
            sun_scale: n("sun-scale")?,
            spot_axis: g.choice("spot-axis"),
            sun_vector: g.choice("sun-vector"),
            colour: g.choice("colour"),
        }))
    },
};

impl Pass for LowerShaunwhiteLighting {
    fn name(&self) -> &'static str {
        "lower-shaunwhite-lighting"
    }
    fn options(&self) -> Options {
        Options::from([
            ("point-scale".into(), self.point_scale.text()),
            ("sun-scale".into(), self.sun_scale.to_string()),
            ("spot-axis".into(), self.spot_axis.text().into()),
            ("sun-vector".into(), self.sun_vector.text().into()),
            ("colour".into(), self.colour.text().into()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[LightSource::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Light::NAME, "core.environment"]
    }
    fn removes(&self) -> &[&'static str] {
        &[LightSource::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Transform::NAME, "core.environment"]
    }
    fn approximates(&self) -> &'static str {
        "chosen relative intensity scales, RGB encoding and spot/sun directions; inverse-square falloff replaces source near/far falloff; all resolved activation states retained; the sky's ambient as the bias of baked lighting the game adds, without the bake"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if !self.sun_scale.is_finite() || self.sun_scale < 0. {
            return Err("light scales must be finite and nonnegative".into());
        }
        let skies: Vec<_> = w.iter::<SkySettings>().map(|(e, s)| (e, s.clone())).collect();
        if skies.len() > 1 {
            return Err("multiple SkyLight settings: select the intended sky before lowering".into());
        }
        let all: Vec<(Entity, LightSource)> = w.iter::<LightSource>().map(|(e, l)| (e, l.clone())).collect();
        let convert = |v: f32| {
            if self.colour == LightColour::Linear {
                v
            } else if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (point_scale, sun_scale) = match self.point_scale {
            PointScale::Candela(c) if c.is_finite() && c >= 0. => (c, self.sun_scale),
            PointScale::Candela(_) => return Err("light scales must be finite and nonnegative".into()),
            PointScale::SunLux(lux) => {
                let sun = skies
                    .first()
                    .and_then(|(e, _)| all.iter().find(|(l, _)| l == e))
                    .map(|(_, l)| l)
                    .ok_or("point-scale=sun:LUX needs the level's SkyLight and its sun")?;
                let [r, g, b, _] = sun.colour.map(convert);
                let luminance = 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
                let unit = f64::from(lux) / (f64::from(sun.intensity) * luminance);
                if !unit.is_finite() || unit <= 0. {
                    return Err(
                        "the sun's intensity and luminance must be above 0 for point-scale=sun:LUX".into(),
                    );
                }
                // the unit as lux per stored unit, in thousands
                log.count(
                    "light unit from the level's sun (thousand lux per stored unit)",
                    (unit / 1000.).round() as u64,
                );
                (unit as f32, unit as f32)
            }
        };
        let mut prepared = Vec::new();
        for (e, l) in all {
            if w.has::<Light>(e) {
                return Err(format!("{e:?}: already has core.light"));
            }
            if !l.flags[3] {
                log.omit("lights affecting characters only", 1);
                continue;
            }
            let mut t = *w
                .get::<Transform>(e)
                .ok_or_else(|| format!("{e:?}: light without a transform"))?;
            let kind = match l.shape {
                LightShape::Point { far, .. } => (LightKind::Point, Some(far), point_scale),
                LightShape::Spot {
                    inner, outer, far, ..
                } => {
                    let a = SourceFrame::ENU.direction(self.spot_axis.vector());
                    let dir = primase_core::Linear::of(&t).point(a);
                    t = Transform::facing(t.cols[3], dir).ok_or("degenerate spot axis")?;
                    (
                        LightKind::Spot {
                            inner_cone: inner,
                            outer_cone: outer,
                        },
                        Some(far),
                        point_scale,
                    )
                }
                LightShape::Directional => (LightKind::Directional, None, sun_scale),
            };
            if let Some((_, sky)) = skies.iter().find(|(sun, _)| *sun == e) {
                let d = SourceFrame::ENU.direction(sky.direction);
                let d = if self.sun_vector == SunVector::ToSun {
                    d.map(|v| -v)
                } else {
                    d
                };
                t = Transform::facing(t.cols[3], d).ok_or("degenerate sun vector")?;
            }
            let [r, g, b, _] = l.colour;
            let light = Light {
                kind: kind.0,
                color: [r, g, b].map(convert),
                intensity: l.intensity * kind.2,
                range: kind.1,
                cast_shadows: Some(l.flags[1]),
                radius: None,
            };
            if !light.intensity.is_finite() || light.color.iter().any(|v| !v.is_finite()) {
                return Err("converted light overflows".into());
            }
            prepared.push((e, light, t, l));
        }
        let mut environment = w.resource::<Environment>().cloned().unwrap_or_default();
        if let Some((sun, sky)) = skies.first() {
            if environment.sun.is_some_and(|e| e != *sun) {
                return Err("world already names a different sun; select one explicitly".into());
            }
            if environment.ambient.is_some() {
                return Err("world already states an ambient; remove it before lowering the sky's".into());
            }
            environment.sun = prepared.iter().any(|(e, _, _, _)| e == sun).then_some(*sun);
            let [r, g, b, _] = sky.ambient_colour;
            environment.ambient = Some(Ambient {
                intensity: sun_scale,
                light: AmbientLight::Flat([r, g, b].map(convert)),
            });
        }
        for (e, light, t, source) in prepared {
            w.insert(e, light);
            w.insert(e, t);
            w.remove::<LightSource>(e);
            log.count("punctual lights lowered", 1);
            if source.flare.enabled {
                log.omit("light flares", 1);
            }
            if matches!(source.shape,LightShape::Spot {cookie,..} if cookie!=0) {
                log.omit("spot cookies", 1);
            }
        }
        if !skies.is_empty() {
            w.insert_resource(environment);
            log.count("sky ambient colours lowered as core ambient", skies.len() as u64);
            log.omit(
                "unlabelled sky settings and global fog (meaning/units not established)",
                skies.len() as u64,
            );
            log.omit(
                "baked lighting (the lightmaps, per-vertex lighting and composite spherical harmonics the game lights its static world with): only its ambient bias is carried",
                skies.len() as u64,
            );
            log.approximate(
                "sky ambient as one flat light in the sun's unit: the game adds it as the bias of its baked lightmaps and vertex lighting and floors its composite spherical harmonics at it, and its alpha (the uninfluenced level) is not used",
                skies.len() as u64,
            );
        }
        if matches!(self.point_scale, PointScale::SunLux(_)) {
            log.approximate(
                "light unit from the stated sun lux (the game's light unit has no stated value), colour encoding and \
                 light axes from explicit options",
                1,
            );
        } else {
            log.approximate(
                "uncalibrated intensity scales, colour encoding and light axes from explicit options",
                1,
            );
        }
        log.approximate(
            "source near/far falloff replaced by core inverse-square falloff with far cutoff",
            1,
        );
        log.approximate(
            "day/night and scripted activation not selected; resolved light entities retained",
            1,
        );
        Ok(())
    }
}
