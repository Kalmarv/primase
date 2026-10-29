//! `lower-fromsoft-lights`: BTL light records (`fromsoft.light`) and the point and spot lights of effect files
//! (`fromsoft.effect_light`) as core point and spot lights. Both kinds reach the game's lighting shader as one record
//! (`PositionAndRadius`, `DiffColor`, ...): its diffuse colour is `(byte / 255)^2 * power`, a BTL light's bytes and
//! power, an effect light's stored colour (the byte's value over 255) and brightness (alpha times the stored diffuse
//! multiplier). So the two share one unit, `candela-per-power`, and one colour transfer, the square. The evidence is the
//! lights' records read back from a running game against the same lights' stored values
//! (`tests/engine_light_records.rs`).
use super::settings::{Factor, Fraction, Metres};
use crate::sekiro::source::{EffectLight, EffectShape, EffectValue, LightKind, LightSource};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Light, LightSpecular, LightUnit, Resource, Transform, World};

/// The game's transfer from a stored colour component in 0 to 1 (a BTL byte over 255, an effect colour's red, green or
/// blue) to the linear value its lighting shader multiplies by the power: the square. Read back from a running game: the
/// diffuse colour of every light record, BTL or effect, is `component^2 * power` to float rounding (relative error
/// 1.4e-7 over 10 BTL light readings), not the sRGB curve.
fn engine_linear(component: f64) -> f32 {
    (component * component) as f32
}

/// A BTL spot light's half-angles in radians, `[horizontal, vertical]`, as the game's loader builds them from the two
/// stored full angles in degrees: the vertical axis takes `cone_angle`, the horizontal axis `cone_width` (record
/// offset 184), each taking the other's where it is not above the loader's epsilon (1.1920929e-7 rad); `None` where
/// neither is, a spot the game leaves switched off. The game's lighting program scales a light-space axis by
/// `cot(half angle)` and shades with `max(0, 1 - x^2 - y^2)` of the scaled coordinates, so the stored value is the full
/// angle of the cone's edge along that axis: read back from a running game, the five spot records of two maps hold
/// scale rows of exactly `cot(angle / 2)` for the stored angle of each axis (`tests/engine_light_records.rs`).
pub fn spot_half_angles(cone_angle: f32, cone_width: f32) -> Option<[f64; 2]> {
    const EPSILON: f32 = 1.192_092_9e-7;
    const RADIANS: f32 = 0.017_453_292;
    let mut vertical = cone_angle * RADIANS;
    let mut horizontal = cone_width * RADIANS;
    if horizontal < EPSILON {
        horizontal = vertical;
    }
    if vertical < EPSILON {
        vertical = horizontal;
    }
    (horizontal >= EPSILON && vertical >= EPSILON && horizontal.is_finite() && vertical.is_finite())
        .then(|| [f64::from(horizontal) / 2., f64::from(vertical) / 2.])
}

/// Which value of an effect light's colour and multiplier properties is lowered (`brightness=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brightness {
    /// Where the property ends: the last keyframe of a function that does not loop, a constant's value (a looping
    /// property has no end: its first keyframe is used, counted).
    Settled,
    /// The first keyframe (time 0).
    Initial,
}
impl Choice for Brightness {
    const VALUES: &'static [(&'static str, Self)] =
        &[("settled", Brightness::Settled), ("initial", Brightness::Initial)];
}

/// Whether effect lights of regions that start disabled, and lights of a config the effect's first state does not
/// choose, are lowered (`include-disabled=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disabled {
    /// Left out, counted by reason.
    Skip,
    /// Lowered like the others.
    Include,
}
impl Choice for Disabled {
    const VALUES: &'static [(&'static str, Self)] = &[("off", Disabled::Skip), ("on", Disabled::Include)];
}

/// Options: `candela-per-power` (required: candela per unit of the stored diffuse power, above 0; no default, it is a
/// calibration, and the world's `core.light_unit` states it), `spot-blend` (required: the inner cone as `1 - blend` of the outer; the game's spot has no inner cone,
/// its falloff is `1 - (tan a / tan edge)^2` of the angle `a` from the axis, so the blend is the program's choice
/// of where a core spot's falloff starts) and `min-range` (metres; a light's range is its stored
/// radius, at least this; default 0, the radius as stored).
///
/// Colour is the stored component squared, the game's own transfer (a BTL light's diffuse bytes over 255, an effect
/// light's stored red, green and blue; every component of every effect light of both installs is a whole number of
/// 255ths, the representation of a byte); intensity the diffuse power times `candela-per-power`; a BTL light's
/// specular (`core.light`'s `specular`) its specular bytes squared and its specular power times `candela-per-power`, as
/// the game's light records hold the two apart (Elden Ring's Stormveil and Sekiro's Ashina bank lights, read back from the
/// running games, hold exactly these); a spot's outer
/// cone is the stored full angle (the vertical axis's; the horizontal axis's is `cone_width`, the circle of the same
/// area replacing the game's pyramid), its full cone clamped to 1-179 degrees; shadows as stored. The light keeps its
/// placement (a spot shines along its local -Z) and its `fromsoft.light` record (raw powers, radius, specular
/// values and tangent). Directional lights are left unlowered, counted.
///
/// Effect lights (`fromsoft.effect_light`, read with `Options::effect_lights`) are lowered like BTL lights, in the same
/// unit: the game's lighting shader takes a BTL light's `bytes^2 * power` and an effect light's `colour^2 * alpha *
/// diffuse multiplier` as the same field of one record, so an effect light's brightness (alpha times the stored
/// multiplier, at the value `brightness=` picks: `settled`, the default, or `initial`, the first keyframe) is a BTL
/// power and takes `candela-per-power` and nothing else. A spot's multiplier is used as stored: the game's code
/// reads the property into the light's intensity with no tenth (the fxr library's tenth, `SPOT_MULTIPLIER_UNIT` of an
/// earlier revision, is its own convention); that is read from the executable's code, not yet checked on a live spot
/// light, and counted as an approximation. A light dark at that value is not lowered, counted. A point's range is its
/// radius (its settled value), a spot's its far plane; a spot's cone is the wider of the half-angles `atan(radius / far)`
/// of its elliptic base and its axis the effect's local +Z, both counted approximations (the game's code builds the
/// frustum from `2 * atan(radius / 2 / far)` per axis, also not yet checked on a live spot), with `spot-blend` for the
/// inner cone. Lights of a region that starts disabled and lights of a config the effect's first state does not choose
/// are left out, counted by reason, unless `include-disabled=on`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerFromSoftLights {
    /// Candela per unit of the stored diffuse power.
    candela_per_power: Factor,
    /// The least range a light gets.
    min_range: Metres,
    /// The inner cone as `1 - blend` of the outer.
    spot_blend: Fraction,
    /// Which keyframe of an effect light's colour and multiplier is lowered.
    brightness: Brightness,
    /// Whether disabled and unchosen effect lights are lowered.
    include_disabled: Disabled,
}

impl LowerFromSoftLights {
    /// The lights at the program's calibration and spot blend, ranges as stored (`min-range` 0), effect lights at
    /// their settled brightness, disabled ones left out.
    pub fn new(candela_per_power: Factor, spot_blend: Fraction) -> Self {
        LowerFromSoftLights {
            candela_per_power,
            min_range: Metres::new(0.).expect("zero"),
            spot_blend,
            brightness: Brightness::Settled,
            include_disabled: Disabled::Skip,
        }
    }
    /// The least range a light gets.
    pub fn with_min_range(mut self, min_range: Metres) -> Self {
        self.min_range = min_range;
        self
    }
    /// Which keyframe of an effect light's colour and multiplier is lowered.
    pub fn with_brightness(mut self, brightness: Brightness) -> Self {
        self.brightness = brightness;
        self
    }
    /// Whether disabled and unchosen effect lights are lowered.
    pub fn with_include_disabled(mut self, include_disabled: Disabled) -> Self {
        self.include_disabled = include_disabled;
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
    pub fn brightness(&self) -> Brightness {
        self.brightness
    }
    pub fn include_disabled(&self) -> Disabled {
        self.include_disabled
    }

    /// From its declared options, through [`LowerFromSoftLights::new`] and its setters.
    fn from_given(g: &Given) -> Result<Self, String> {
        let mut pass = Self::new(
            Factor::parse("candela-per-power", g.required("candela-per-power"))?,
            Fraction::parse("spot-blend", g.required("spot-blend"))?,
        );
        if let Some(v) = g.optional("min-range") {
            pass = pass.with_min_range(Metres::parse("min-range", v)?);
        }
        if let Some(b) = g.optional_choice::<Brightness>("brightness") {
            pass = pass.with_brightness(b);
        }
        if let Some(d) = g.optional_choice::<Disabled>("include-disabled") {
            pass = pass.with_include_disabled(d);
        }
        Ok(pass)
    }
}

/// `lower-fromsoft-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): BTL power's calibration and the
/// cone's falloff are the program's; an effect light's brightness is in BTL power.
pub const LOWER_FROMSOFT_LIGHTS: PassSpec = PassSpec {
    name: "lower-fromsoft-lights",
    options: &[
        OptionSpec::required(
            "candela-per-power",
            "1",
            "above 0: the candela of one unit of BTL power, the program's calibration (the world's core.light_unit)",
        ),
        OptionSpec::required(
            "spot-blend",
            "0.15",
            "0 to 1: a spot cone's blend (the game's falloff, 1 - (tan a / tan edge)^2, has no inner cone)",
        ),
        OptionSpec::optional(
            "min-range",
            "0",
            "metres, at least 0: the least range a light gets",
            "0: no least range",
        ),
        OptionSpec::optional_choice::<Brightness>(
            "brightness",
            "which value of an effect light's colour and multiplier is lowered",
            "settled: the value the property ends on",
        ),
        OptionSpec::optional_choice::<Disabled>(
            "include-disabled",
            "lower the effect lights of regions that start disabled and of configs the effect's first state does not \
             choose",
            "off: they are left out, counted by reason",
        ),
    ],
    make: |g| Ok(Box::new(LowerFromSoftLights::from_given(g)?)),
};

/// The value of `v` at `brightness`, and whether a looping property had no settled value.
fn pick(v: &EffectValue, brightness: Brightness) -> (&[f32], bool) {
    match brightness {
        Brightness::Initial => (&v.initial, false),
        Brightness::Settled => match &v.settled {
            Some(s) => (s, false),
            None => (&v.initial, true),
        },
    }
}

impl LowerFromSoftLights {
    fn lower_btl(
        &self,
        world: &World,
        log: &mut PassLog,
    ) -> Result<Vec<(primase_core::Entity, Light)>, String> {
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
                    if !source.cone_angle.is_finite() || !source.cone_width.is_finite() {
                        return Err(format!("{e:?}: non-finite cone angle"));
                    }
                    let Some([horizontal, vertical]) = spot_half_angles(source.cone_angle, source.cone_width)
                    else {
                        log.count(
                            "spot lights without a cone angle, which the game leaves off, left out",
                            1,
                        );
                        continue;
                    };
                    // the game's cone is a pyramid of two half-angles; a core spot has one: the circle of the same
                    // area on the unit-distance plane
                    let circle = if horizontal == vertical {
                        horizontal
                    } else {
                        log.approximate("spot cone of two angles as the circle of the same area", 1);
                        (horizontal.tan() * vertical.tan()).sqrt().atan()
                    };
                    let (least, most) = (1f64.to_radians(), 179f64.to_radians());
                    if !(least..=most).contains(&(2. * circle)) {
                        log.approximate("spot cones clamped to 1-179 degrees", 1);
                    }
                    let outer = ((2. * circle).clamp(least, most) / 2.) as f32;
                    log.approximate("spot inner cone from the fixed blend", 1);
                    primase_core::LightKind::Spot {
                        inner_cone: outer * (1. - self.spot_blend.get()),
                        outer_cone: outer,
                    }
                }
            };
            let intensity = f64::from(source.diffuse_power) * self.candela_per_power.get();
            let specular_intensity = f64::from(source.specular_power) * self.candela_per_power.get();
            if !intensity.is_finite()
                || intensity < 0.
                || !specular_intensity.is_finite()
                || specular_intensity < 0.
                || !source.radius.is_finite()
            {
                return Err(format!("{e:?}: invalid light power or radius"));
            }
            if source.radius < self.min_range.get() {
                log.count("ranges raised to the minimum", 1);
            }
            if source.exposure_weight != 0. {
                log.approximate(
                    "lights with an exposure weight, the game's per-frame exposure term not lowered",
                    1,
                );
            }
            log.approximate("colour bytes squared, the game's transfer", 1);
            log.approximate("intensity by the calibrated power", 1);
            lowered.push((
                e,
                Light {
                    kind,
                    color: source.diffuse.map(|c| engine_linear(f64::from(c) / 255.)),
                    intensity: intensity as f32,
                    range: Some(source.radius.max(self.min_range.get())),
                    cast_shadows: Some(source.cast_shadows),
                    radius: None,
                    // the record's own specular colour and power, as its diffuse ones: the game's light records hold
                    // each colour squared times its power, apart
                    specular: Some(LightSpecular {
                        color: source.specular.map(|c| engine_linear(f64::from(c) / 255.)),
                        intensity: specular_intensity as f32,
                    }),
                },
            ));
        }
        Ok(lowered)
    }

    fn lower_effects(
        &self,
        world: &World,
        log: &mut PassLog,
    ) -> Result<Vec<(primase_core::Entity, Light)>, String> {
        let mut lowered = Vec::new();
        for (e, source) in world.iter::<EffectLight>() {
            if !world.has::<Transform>(e) {
                return Err(format!("{e:?}: an effect light without a transform"));
            }
            if self.include_disabled == Disabled::Skip {
                if source.starts_disabled {
                    log.count("effect lights of regions that start disabled, left out", 1);
                    continue;
                }
                if !source.chosen_in_state_0 {
                    log.count(
                        "effect lights of a config the effect's first state does not choose, left out",
                        1,
                    );
                    continue;
                }
            }
            let mut looping = false;
            let mut numbers = |v: &EffectValue, what: &str, n: usize| -> Result<Vec<f32>, String> {
                let (values, looped) = pick(v, self.brightness);
                looping |= looped;
                if values.len() != n || values.iter().any(|x| !x.is_finite()) {
                    return Err(format!("{e:?}: {what} is not {n} finite numbers"));
                }
                Ok(values.to_vec())
            };
            let colour = numbers(&source.diffuse, "diffuse colour", 4)?;
            let mut brightness = f64::from(colour[3]);
            if let Some(m) = &source.diffuse_multiplier {
                let m = f64::from(numbers(m, "diffuse multiplier", 1)?[0]);
                brightness *= m;
            }
            if looping {
                log.count(
                    "looping colour or multiplier properties, lowered at their first keyframe",
                    1,
                );
            }
            if colour[..3].iter().any(|c| !(0. ..=1.).contains(c)) {
                return Err(format!("{e:?}: a colour component outside 0 to 1"));
            }
            if brightness <= 0. {
                log.count("effect lights dark at the lowered value, left out", 1);
                continue;
            }
            let settled = |v: &EffectValue, what: &str| -> Result<f32, String> {
                pick(v, Brightness::Settled)
                    .0
                    .first()
                    .copied()
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| format!("{e:?}: no {what}"))
            };
            let (kind, range) = match &source.shape {
                EffectShape::Point { radius } => {
                    if pick(radius, Brightness::Settled).1 {
                        log.count("looping radius properties, lowered at their first keyframe", 1);
                    }
                    (primase_core::LightKind::Point, settled(radius, "radius")?)
                }
                EffectShape::Spot {
                    near: _,
                    far,
                    radius_x,
                    radius_y,
                } => {
                    let (far, rx, ry) = (
                        settled(far, "far plane")?,
                        settled(radius_x, "x radius")?,
                        settled(radius_y, "y radius")?,
                    );
                    if !(far > 0. && rx >= 0. && ry >= 0.) {
                        return Err(format!("{e:?}: a spot cone of far {far}, radii {rx} and {ry}"));
                    }
                    if rx != ry {
                        log.approximate("elliptic spot cones taken at their wider half-angle", 1);
                    }
                    let full = 2. * f64::from(rx.max(ry)).atan2(f64::from(far)).to_degrees();
                    if !(1. ..=179.).contains(&full) {
                        log.approximate("spot cones clamped to 1-179 degrees", 1);
                    }
                    let outer = (full.clamp(1., 179.).to_radians() / 2.) as f32;
                    log.approximate("effect spot axis along the effect's local +Z", 1);
                    log.approximate("spot inner cone from the fixed blend", 1);
                    (
                        primase_core::LightKind::Spot {
                            inner_cone: outer * (1. - self.spot_blend.get()),
                            outer_cone: outer,
                        },
                        far,
                    )
                }
            };
            let intensity = brightness * self.candela_per_power.get();
            if !intensity.is_finite() || range.is_nan() || range <= 0. {
                return Err(format!("{e:?}: invalid light brightness or range"));
            }
            if range < self.min_range.get() {
                log.count("ranges raised to the minimum", 1);
            }
            log.approximate("effect light colour squared, the game's transfer", 1);
            log.approximate(
                "effect light specular colour, flicker and fade-out not lowered",
                1,
            );
            if matches!(source.shape, EffectShape::Spot { .. }) {
                log.approximate(
                    "effect spot multiplier as stored, no tenth (the game's code; not checked on a live spot)",
                    1,
                );
            }
            lowered.push((
                e,
                Light {
                    kind,
                    color: [0, 1, 2].map(|i| engine_linear(f64::from(colour[i]))),
                    intensity: intensity as f32,
                    range: Some(range.max(self.min_range.get())),
                    cast_shadows: Some(source.shadows),
                    radius: None,
                    // the effect's own specular is not lowered (the approximation above): its highlights take its colour
                    specular: None,
                },
            ));
        }
        Ok(lowered)
    }
}

impl Pass for LowerFromSoftLights {
    fn name(&self) -> &'static str {
        "lower-fromsoft-lights"
    }
    fn options(&self) -> Options {
        let mut options = Options::from([
            ("candela-per-power".to_owned(), self.candela_per_power.to_string()),
            ("min-range".to_owned(), self.min_range.to_string()),
            ("spot-blend".to_owned(), self.spot_blend.to_string()),
        ]);
        if self.brightness != Brightness::Settled {
            options.insert("brightness".to_owned(), self.brightness.text().to_owned());
        }
        if self.include_disabled != Disabled::Skip {
            options.insert(
                "include-disabled".to_owned(),
                self.include_disabled.text().to_owned(),
            );
        }
        options
    }
    /// Nothing is required: a read area may hold BTL records, effect lights, both or neither.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    /// A read region may hold no light records: nothing to lower, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[Light::NAME, LightUnit::NAME]
    }
    fn approximates(&self) -> &'static str {
        "power by the program's calibration; spot inner cone from a fixed blend; specular values not lowered; effect lights: an elliptic spot cone as its wider half-angle, specular, flicker and fade-out not lowered"
    }
    /// The lights, and the world's `core.light_unit` at `candela-per-power` where any is lowered (a light table packs
    /// their colours back into the stored values with it); a unit already stated at another factor is refused.
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let btl = self.lower_btl(world, log)?;
        let effects = self.lower_effects(world, log)?;
        if !btl.is_empty() || !effects.is_empty() {
            let unit = LightUnit {
                candela_per_unit: self.candela_per_power.get() as f32,
            };
            unit.verify(world)
                .map_err(|why| format!("candela-per-power as the lights' unit: {why}"))?;
            match world.resource::<LightUnit>() {
                Some(stated) if stated != &unit => {
                    return Err(format!(
                        "the world's lights are already in a unit of {} candela, not {}",
                        stated.candela_per_unit, unit.candela_per_unit
                    ));
                }
                _ => {
                    world.insert_resource(unit);
                }
            }
        }
        for (e, light) in btl {
            world.insert(e, light);
            log.count("lights lowered", 1);
        }
        for (e, light) in effects {
            world.insert(e, light);
            log.count("effect lights lowered", 1);
        }
        Ok(())
    }
}
