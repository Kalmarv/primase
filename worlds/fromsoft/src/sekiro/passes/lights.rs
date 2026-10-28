//! `lower-fromsoft-lights`: BTL light records (`fromsoft.light`) and, where the program gives their calibration, the
//! point and spot lights of effect files (`fromsoft.effect_light`) as core point and spot lights. BTL's colour transfer
//! function, the unit of its powers and its cone's falloff have not been established against the game's lighting
//! shader, so each interpretation is a named, counted assumption and the power's calibration is the program's. An effect
//! light's brightness has no unit in the data either, and none that relates it to a BTL power: the program states it.
use super::settings::{Factor, Fraction, Metres};
use crate::sekiro::source::{EffectLight, EffectShape, EffectValue, LightKind, LightSource};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Light, Transform, World};

/// A spot light's stored brightness multipliers are ten times the unit a point light's are in: the fxr library
/// (EvenTorset/fxr) reads and prints them divided by ten, in every game but Dark Souls III. This is the library's
/// convention, **sourced**, not something our data shows (the data holds the stored values only); it is one constant so
/// that a check against the game can replace it.
pub const SPOT_MULTIPLIER_UNIT: f64 = 0.1;

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

/// How effect lights are lowered, once the program has given their brightness unit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EffectCalibration {
    /// BTL powers of one unit of effect brightness.
    power_per_unit: Factor,
    brightness: Brightness,
    include_disabled: Disabled,
}

/// Options: `candela-per-power` (required: candela per unit of the stored diffuse power; no default, it is a
/// calibration), `spot-blend` (required: the inner cone as `1 - blend` of the outer; BTL's cone falloff is not
/// established, so the blend is the program's assumption) and `min-range` (metres; a light's range is its stored
/// radius, at least this; default 0, the radius as stored).
///
/// Colour is the diffuse bytes read as sRGB; intensity the diffuse power times `candela-per-power`; a spot's outer
/// cone is the stored angle, its full cone clamped to 1-179 degrees; shadows as stored. The light keeps its
/// placement (a spot shines along its local -Z) and its `fromsoft.light` record (raw powers, radius, specular
/// values and tangent). Directional lights are left unlowered, counted.
///
/// Effect lights (`fromsoft.effect_light`, read with `Options::effect_lights`) are lowered when `effect-power-per-unit`
/// is given (no default: the relation of the files' brightness to a BTL power is the program's, see below), and
/// otherwise left unlowered, counted as `effect lights without an effect-power calibration`. Their colour is the
/// stored red, green and blue read as sRGB like BTL's bytes (every component of every light of both installs is a whole
/// number of 255ths, the representation of a byte). Their brightness is the colour's alpha times the stored diffuse
/// multiplier (a spot's times [`SPOT_MULTIPLIER_UNIT`], sourced) at the value `brightness=` picks (`settled`, the
/// default, or `initial`: the first keyframe), their intensity that times `effect-power-per-unit` times
/// `candela-per-power`. A light dark at that value is not lowered, counted. A point's range is its radius (its settled
/// value), a spot's its far plane; a spot's cone is the wider of the half-angles `atan(radius / far)` of its elliptic base
/// and its axis the effect's local +Z, both counted approximations, with `spot-blend` for the inner cone. Lights of a
/// region that starts disabled and lights of a config the effect's first state does not choose are left out,
/// counted by reason, unless `include-disabled=on`.
///
/// **The brightness unit is an estimate, not something the data gives.** Per area, the median of the placed live point
/// lights' alpha times settled multiplier against the median power of the area's BTL point lights has ratios from 1.6 to
/// 320 (median 16) over the 68 Elden Ring areas read with both kinds and 0.25 to 64 (median 4) over Sekiro's 9: BTL powers are authored per area, the
/// effects' multipliers are not. Lights of both kinds within 1.5 m of each other (taken to be one fixture) give a median
/// ratio of 5, quartiles 3.2 and 10 (1,227 pairs; Sekiro's 35 have the same median, quartiles 2.5 and 6.4): about 0.2 BTL powers per unit of effect brightness, to a factor of
/// 2 to 3 either way, an inference. The unit is therefore a calibration, as BTL's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerFromSoftLights {
    /// Candela per unit of the stored diffuse power.
    candela_per_power: Factor,
    /// The least range a light gets.
    min_range: Metres,
    /// The inner cone as `1 - blend` of the outer.
    spot_blend: Fraction,
    /// How effect lights are lowered; `None`: not lowered, counted.
    effect: Option<EffectCalibration>,
}

impl LowerFromSoftLights {
    /// The lights at the program's calibration and spot blend, ranges as stored (`min-range` 0), effect lights not lowered.
    pub fn new(candela_per_power: Factor, spot_blend: Fraction) -> Self {
        LowerFromSoftLights {
            candela_per_power,
            min_range: Metres::new(0.).expect("zero"),
            spot_blend,
            effect: None,
        }
    }
    /// The least range a light gets.
    pub fn with_min_range(mut self, min_range: Metres) -> Self {
        self.min_range = min_range;
        self
    }
    /// Lower effect lights as well, one unit of their brightness (alpha times multiplier) being `power_per_unit` BTL
    /// powers: settled brightness, disabled lights left out (see [`LowerFromSoftLights::with_brightness`],
    /// [`LowerFromSoftLights::with_include_disabled`]).
    pub fn with_effect_power(mut self, power_per_unit: Factor) -> Self {
        self.effect = Some(EffectCalibration {
            power_per_unit,
            brightness: Brightness::Settled,
            include_disabled: Disabled::Skip,
        });
        self
    }
    /// Which keyframe of an effect light's colour and multiplier is lowered; only with an effect calibration.
    pub fn with_brightness(mut self, brightness: Brightness) -> Self {
        if let Some(effect) = &mut self.effect {
            effect.brightness = brightness;
        }
        self
    }
    /// Whether disabled effect lights are lowered; only with an effect calibration.
    pub fn with_include_disabled(mut self, include_disabled: Disabled) -> Self {
        if let Some(effect) = &mut self.effect {
            effect.include_disabled = include_disabled;
        }
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
    /// BTL powers of one unit of effect brightness, if effect lights are lowered.
    pub fn effect_power(&self) -> Option<Factor> {
        self.effect.map(|e| e.power_per_unit)
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
        match g.optional("effect-power-per-unit") {
            Some(v) => {
                pass = pass.with_effect_power(Factor::parse("effect-power-per-unit", v)?);
                if let Some(b) = g.optional_choice::<Brightness>("brightness") {
                    pass = pass.with_brightness(b);
                }
                if let Some(d) = g.optional_choice::<Disabled>("include-disabled") {
                    pass = pass.with_include_disabled(d);
                }
            }
            None => {
                for key in ["brightness", "include-disabled"] {
                    if g.optional(key).is_some() {
                        return Err(format!(
                            "{key} is about effect lights, which need effect-power-per-unit"
                        ));
                    }
                }
            }
        }
        Ok(pass)
    }
}

/// `lower-fromsoft-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): BTL power's calibration and the
/// cone's falloff are the program's, and so is an effect light's brightness unit.
pub const LOWER_FROMSOFT_LIGHTS: PassSpec = PassSpec {
    name: "lower-fromsoft-lights",
    options: &[
        OptionSpec::required(
            "candela-per-power",
            "1",
            "at least 0: the candela of one unit of BTL power, the program's calibration",
        ),
        OptionSpec::required(
            "spot-blend",
            "0.15",
            "0 to 1: a spot cone's blend (BTL's cone falloff is not established)",
        ),
        OptionSpec::optional(
            "min-range",
            "0",
            "metres, at least 0: the least range a light gets",
            "0: no least range",
        ),
        OptionSpec::optional(
            "effect-power-per-unit",
            "1",
            "at least 0: the BTL powers of one unit of an effect light's brightness (alpha times multiplier), the program's \
             calibration (not derivable from the data)",
            "effect lights are not lowered, counted",
        ),
        OptionSpec::optional_choice::<Brightness>(
            "brightness",
            "which value of an effect light's colour and multiplier is lowered (needs effect-power-per-unit)",
            "settled: the value the property ends on",
        ),
        OptionSpec::optional_choice::<Disabled>(
            "include-disabled",
            "lower the effect lights of regions that start disabled and of configs the effect's first state does not \
             choose (needs effect-power-per-unit)",
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
                        .map(|c| primase_foundations::color::srgb_to_linear(f64::from(c) / 255.) as f32),
                    intensity: intensity as f32,
                    range: Some(source.radius.max(self.min_range.get())),
                    cast_shadows: Some(source.cast_shadows),
                    radius: None,
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
            let Some(effect) = self.effect else {
                log.count(
                    "effect lights without an effect-power calibration, not lowered",
                    1,
                );
                continue;
            };
            if !world.has::<Transform>(e) {
                return Err(format!("{e:?}: an effect light without a transform"));
            }
            if effect.include_disabled == Disabled::Skip {
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
                let (values, looped) = pick(v, effect.brightness);
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
                brightness *= match source.shape {
                    EffectShape::Spot { .. } => m * SPOT_MULTIPLIER_UNIT,
                    EffectShape::Point { .. } => m,
                };
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
            let intensity = brightness * effect.power_per_unit.get() * self.candela_per_power.get();
            if !intensity.is_finite() || range.is_nan() || range <= 0. {
                return Err(format!("{e:?}: invalid light brightness or range"));
            }
            if range < self.min_range.get() {
                log.count("ranges raised to the minimum", 1);
            }
            log.approximate("effect light colour read as sRGB", 1);
            log.approximate("effect light intensity by the calibrated brightness unit", 1);
            log.approximate(
                "effect light specular colour, flicker and fade-out not lowered",
                1,
            );
            if matches!(source.shape, EffectShape::Spot { .. }) {
                log.approximate(
                    "effect spot multiplier unit (1/10 of the stored value), the fxr library's",
                    1,
                );
            }
            lowered.push((
                e,
                Light {
                    kind,
                    color: [0, 1, 2]
                        .map(|i| primase_foundations::color::srgb_to_linear(f64::from(colour[i])) as f32),
                    intensity: intensity as f32,
                    range: Some(range.max(self.min_range.get())),
                    cast_shadows: Some(source.shadows),
                    radius: None,
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
        if let Some(effect) = self.effect {
            options.insert(
                "effect-power-per-unit".to_owned(),
                effect.power_per_unit.to_string(),
            );
            if effect.brightness != Brightness::Settled {
                options.insert("brightness".to_owned(), effect.brightness.text().to_owned());
            }
            if effect.include_disabled != Disabled::Skip {
                options.insert(
                    "include-disabled".to_owned(),
                    effect.include_disabled.text().to_owned(),
                );
            }
        }
        options
    }
    /// BTL light records; with an effect calibration, either kind of light (a region may hold only one), so none.
    fn requires(&self) -> &[&'static str] {
        if self.effect.is_some() {
            &[]
        } else {
            &[LightSource::NAME]
        }
    }
    /// A read region may hold no light records: nothing to lower, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[Light::NAME]
    }
    fn approximates(&self) -> &'static str {
        "BTL colour bytes as sRGB; power by the program's calibration; spot inner cone from a fixed blend; specular values not lowered; effect lights: brightness by the program's unit, an elliptic spot cone as its wider half-angle, specular, flicker and fade-out not lowered"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let btl = self.lower_btl(world, log)?;
        let effects = self.lower_effects(world, log)?;
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
