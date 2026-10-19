//! `lower-fromsoft-lights`: BTL light records (`fromsoft.light`) as core point and spot lights. BTL's colour transfer
//! function, the unit of its powers and its cone's falloff have not been established against the game's lighting
//! shader, so each interpretation is a named, counted assumption and the power's calibration is the program's.
use super::settings::{Factor, Fraction, Metres};
use super::source::{LightKind, LightSource};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Light, Transform, World};

/// Options: `candela-per-power` (required: candela per unit of the stored diffuse power; no default, it is a
/// calibration), `spot-blend` (required: the inner cone as `1 - blend` of the outer; BTL's cone falloff is not
/// established, so the blend is the program's assumption) and `min-range` (metres; a light's range is its stored
/// radius, at least this; default 0, the radius as stored).
///
/// Colour is the diffuse bytes read as sRGB; intensity the diffuse power times `candela-per-power`; a spot's outer
/// cone is the stored angle, its full cone clamped to 1-179 degrees; shadows as stored. The light keeps its
/// placement (a spot shines along its local -Z) and its `fromsoft.light` record (raw powers, radius, specular
/// values and tangent). Directional lights are left unlowered, counted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerFromSoftLights {
    /// Candela per unit of the stored diffuse power.
    candela_per_power: Factor,
    /// The least range a light gets.
    min_range: Metres,
    /// The inner cone as `1 - blend` of the outer.
    spot_blend: Fraction,
}

impl LowerFromSoftLights {
    /// The lights at the program's calibration and spot blend, ranges as stored (`min-range` 0).
    pub fn new(candela_per_power: Factor, spot_blend: Fraction) -> Self {
        LowerFromSoftLights {
            candela_per_power,
            min_range: Metres::new(0.).expect("zero"),
            spot_blend,
        }
    }
    /// The least range a light gets.
    pub fn with_min_range(mut self, min_range: Metres) -> Self {
        self.min_range = min_range;
        self
    }
    pub fn candela_per_power(&self) -> Factor {
        self.candela_per_power
    }
    pub fn min_range(&self) -> Metres {
        self.min_range
    }
    pub fn spot_blend(&self) -> Fraction {
        self.spot_blend
    }

    /// From its options, through [`LowerFromSoftLights::new`] and its setter.
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let candela = o
            .get("candela-per-power")
            .ok_or("candela-per-power is required: the program calibrates BTL power")?;
        let blend = o.get("spot-blend").ok_or(
            "spot-blend is required: BTL's cone falloff is not established, the blend is the program's",
        )?;
        let mut pass = Self::new(
            Factor::parse("candela-per-power", candela)?,
            Fraction::parse("spot-blend", blend)?,
        );
        for (k, v) in o {
            pass = match k.as_str() {
                "candela-per-power" | "spot-blend" => pass,
                "min-range" => pass.with_min_range(Metres::parse(k, v)?),
                _ => return Err(format!("unknown option {k}")),
            };
        }
        Ok(pass)
    }
}

impl Pass for LowerFromSoftLights {
    fn name(&self) -> &'static str {
        "lower-fromsoft-lights"
    }
    fn options(&self) -> Options {
        Options::from([
            ("candela-per-power".to_owned(), self.candela_per_power.to_string()),
            ("min-range".to_owned(), self.min_range.to_string()),
            ("spot-blend".to_owned(), self.spot_blend.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[LightSource::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Light::NAME]
    }
    fn approximates(&self) -> &'static str {
        "BTL colour bytes as sRGB; power by the program's calibration; spot inner cone from a fixed blend; specular values not lowered"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut lowered = Vec::new();
        for (e, source) in world.iter::<LightSource>() {
            if !world.has::<Transform>(e) {
                return Err(format!("{e:?}: a light record without a transform"));
            }
            let kind = match source.kind {
                LightKind::Directional => {
                    log.count("directional lights not lowered", 1);
                    continue;
                }
                LightKind::Point => primase_core::LightKind::Point,
                LightKind::Spot => {
                    let full = 2. * source.cone_angle;
                    if !full.is_finite() {
                        return Err(format!("{e:?}: non-finite cone angle"));
                    }
                    if !(1. ..=179.).contains(&full) {
                        log.approximate("spot cones clamped to 1-179 degrees", 1);
                    }
                    let outer = full.clamp(1., 179.).to_radians() / 2.;
                    log.approximate("spot inner cone from the fixed blend", 1);
                    primase_core::LightKind::Spot {
                        inner_cone: outer * (1. - self.spot_blend.get()),
                        outer_cone: outer,
                    }
                }
            };
            let intensity = f64::from(source.diffuse_power) * self.candela_per_power.get();
            if !intensity.is_finite() || intensity < 0. || !source.radius.is_finite() {
                return Err(format!("{e:?}: invalid light power or radius"));
            }
            if source.radius < self.min_range.get() {
                log.count("ranges raised to the minimum", 1);
            }
            log.approximate("colour bytes read as sRGB", 1);
            log.approximate("intensity by the calibrated power", 1);
            log.approximate("specular colour and power not lowered", 1);
            lowered.push((
                e,
                Light {
                    kind,
                    color: source
                        .diffuse
                        .map(|c| primase_image::srgb_to_linear(f64::from(c) / 255.) as f32),
                    intensity: intensity as f32,
                    range: Some(source.radius.max(self.min_range.get())),
                    cast_shadows: Some(source.cast_shadows),
                },
            ));
        }
        for (e, light) in lowered {
            world.insert(e, light);
            log.count("lights lowered", 1);
        }
        Ok(())
    }
}
