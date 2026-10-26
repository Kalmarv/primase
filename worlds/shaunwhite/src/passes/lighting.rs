//! Explicit interpretation of the source's relative light units, colour encoding and spot/sun directions. The
//! game's falloff, cookies, flares, activation states and character-only lighting are not the punctual core model.
use crate::components::{LightShape, LightSource, SkySettings};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Environment, Light, LightKind, Transform, World};

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

#[derive(Clone, Copy, Debug)]
pub struct LowerShaunwhiteLighting {
    pub point_scale: f32,
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
            "candela per stored local-light intensity unit (uncalibrated)",
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
            point_scale: n("point-scale")?,
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
            ("point-scale".into(), self.point_scale.to_string()),
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
        "chosen relative intensity scales, RGB encoding and spot/sun directions; inverse-square falloff replaces source near/far falloff; all resolved activation states retained"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if [self.point_scale, self.sun_scale]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.)
        {
            return Err("light scales must be finite and nonnegative".into());
        }
        let skies: Vec<_> = w.iter::<SkySettings>().map(|(e, s)| (e, s.clone())).collect();
        if skies.len() > 1 {
            return Err("multiple SkyLight settings: select the intended sky before lowering".into());
        }
        let all: Vec<(Entity, LightSource)> = w.iter::<LightSource>().map(|(e, l)| (e, l.clone())).collect();
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
                LightShape::Point { far, .. } => (LightKind::Point, Some(far), self.point_scale),
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
                        self.point_scale,
                    )
                }
                LightShape::Directional => (LightKind::Directional, None, self.sun_scale),
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
            let convert = |v: f32| {
                if self.colour == LightColour::Linear {
                    v
                } else if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
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
        if let Some((sun, _)) = skies.first() {
            if environment.sun.is_some_and(|e| e != *sun) {
                return Err("world already names a different sun; select one explicitly".into());
            }
            environment.sun = prepared.iter().any(|(e, _, _, _)| e == sun).then_some(*sun);
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
            log.omit(
                "sky ambient, unlabelled settings and global fog (meaning/units not established)",
                skies.len() as u64,
            );
        }
        log.approximate(
            "uncalibrated intensity scales, colour encoding and light axes from explicit options",
            1,
        );
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
