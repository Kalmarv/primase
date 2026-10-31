//! `sample-fromsoft-draw-params{hour}`: the area's draw-param environment (`fromsoft.draw_param`) at one hour of the day.
//!
//! The read keeps every stored sample of a field (a field with several time-of-day keys is several [`DrawField`]s of one
//! key, each with its time). This pass is the program's choice of the hour: each such field becomes the value the game
//! computes at that hour of its clock, in the game's two steps. Both are read from the executables (Sekiro 1.06; Elden
//! Ring, executable file version 2.7.2.0), whose lookups are one template.
//!
//! The lookup turns the hour into a key, or two keys and a weight. The file's time axis gives an offset `O` and a period
//! `P` ([`fromsoft_formats::gparam::TimeAxis::sampling`]; a file before version 5 has none: 0 and 0). With `t` the hour:
//! where `P` is not 0 (the axis wraps) `P` is taken from `t` while `t > P`; then `O` is taken from `t`, and `P` added
//! back where that leaves it below 0. The keys are searched in stored order:
//! - the first key within 0.001 of `t` is the value, with no blend;
//! - else the first key above `t` closes a blend from the key before it, at the weight `(t - before) / (key - before)`;
//! - past the last key, a wrapping axis blends from the last key to the first at `(t - last) / ((P - last) + first)`, and
//!   one that does not wrap is the last key;
//! - before the first key, a wrapping axis blends from the last key to the first at `t / ((P - last) + first)`. That is
//!   the game's quirk, kept: the time since the last key is `t + P - last`, so its weight falls back to 0 (the last key's
//!   value) at the axis's 0, climbs to below 1 by the first key and jumps to it there. An axis that does not wrap blends
//!   from the first key to itself at `(t - first) / 0`, an infinite negative weight.
//!
//! All of it in 32-bit floats, as the game computes it. The game starts each search at the key it found the time before,
//! which is the search from the first key only where no two keys are within 0.002 of each other (no time lies within
//! 0.001 of both): a field whose keys are not ascending by more than that is refused.
//!
//! The blend is the slot's the game makes for the field's key, of the type and subtype it registers
//! (`assets/draw_param_fields`, the registrations read from each executable). With no blend both keys are the one
//! found, and every slot gives it. From key `a` to key `b` at weight `w`:
//! - a bool takes `b`, in both games;
//! - Elden Ring: a field it registers without interpolation (the flag 0: enums, resolutions, counts and others) takes `b`.
//!   One registered with it (a 32-bit float, integer or unsigned integer, or a float vector) is `a` at `w <= 0`, `b` at
//!   `w >= 1` or where `a == b`, else `(1 - w) a + w b` per component; integers go through 32-bit floats and are
//!   truncated;
//! - Sekiro: every other field is `(1 - w) a + w b` per component, unclamped; integers and colour bytes go through 32-bit
//!   floats and are truncated as the processor truncates (a value out of range is the lowest 32-bit integer, and 0 for an
//!   unsigned integer or colour byte, which keep the low bits of the lowest 64-bit one). At the infinite weight the blend
//!   is not a number: a float field there is left out, an integer one takes the processor's value (counted);
//! - an angle (a two-float field registered with subtype 23: the light angles) blends as its direction `(sin ry cos rx,
//!   -sin rx, cos ry cos rx)` (both games; Elden Ring at its flag): `a` at `w = 0`; `b` at `w = 1` or where every
//!   component is within 2^-23 of `b`'s; else `a` turned towards `b` in their plane by `w` times the angle between them
//!   (`b` where that reaches past it; `a` where the two are within a sine of 0.01 of one line). The direction is stored
//!   back as angles that give it. This platform's sine, cosine and arc cosine stand in for the game's (its own
//!   polynomial for the sine and cosine), and the angles are this pass's, so the last bits may differ: counted as an
//!   approximation ([`ANGLE_FUNCTIONS`]). At the infinite weight the turn is by an infinite angle, whose sine and cosine
//!   by the game's polynomial have not been read: such an angle is left out.
//!
//! A field is left out, named in the log's omissions, where the game's value is not one this pass can state: a key the
//! game does not register (it never reads it); a file type that is not the registered one; a float of subtype 25 (Elden
//! Ring's `VolumetricFog Global HeightFogOffset`), to whose keys the slot adds a value the game computes while running
//! before it blends; an 8-bit integer, or an Elden Ring colour, whose slot's blend has not been read; and the float that
//! is not a number and the angle turned by the infinite weight above.
//!
//! A field of one sample is the game's at every hour (the lookup's one key) and stays as stored; so do the sets the read
//! left unresolved, which are not part of the scene's environment. `lower-fromsoft-environment` samples the same way at
//! the lowering pass's default hour where no program chose one ([`sample_at`]). The hour is the scene's: the pass states it as the
//! world's `core.time_of_day`, which the programs that key their values by the hour read.
use crate::sekiro::source::{DrawField, DrawParam, FieldType, SetChosen};
use fromsoft_formats::Game;
use fromsoft_formats::gparam::Value;
use primase_core::Resource;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{TimeOfDay, World};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// A lowering pass's documented fallback, never data attached by a reader.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DefaultHour {
    pub hour: f32,
    pub source: String,
}

/// The hour of the day the lowering passes of `game` is lowered at where the program samples none, and where it comes from. Neither
/// game's data states the hour a map is entered at (`tests/time_of_day_census.rs`, every event script of the installs):
/// - Sekiro has no clock. Its scripts set a period of the day by story event (`2003:81` and `2002:13`, the schema's
///   morning, noon, afternoon, evening and night), and no param maps a period to an hour, so that mapping is the
///   executable's. A file's time-of-day keys are hours (0, 2, 4, 6, 12, 18 and 22 in `m11_01_0000`'s light set), and
///   the first, hour 0, is black in some sets: no sun, a state no frame of the game has shown. A running game on
///   `m11_01` showed the sun of the 18:00 keys (its `cbLight` equal to `DiffColor0` and `SpecColor0` at 18:00, to float
///   precision): 18 is that observed hour.
/// - Elden Ring keeps a clock that passes through every hour; its scripts set it only in story and NPC events (nine
///   `2001:4` calls, at 3:30, 8:00, 10:30, 22:30 and 23:45), none on entering a map: 12, noon, is chosen.
pub(crate) fn default_hour(game: Game) -> DefaultHour {
    let (hour, source) = match game {
        Game::Sekiro => (
            18.0,
            "observed on m11_01 in one running game (cbLight equal to the 18:00 keys); the period-to-hour mapping is in the executable",
        ),
        _ => (12.0, "no entering hour in the data; noon chosen"),
    };
    DefaultHour {
        hour,
        source: source.to_owned(),
    }
}

/// What a sampling counts for each field it sampled at the hour.
pub const SAMPLED: &str = "draw-param fields sampled at the hour";
/// Of [`SAMPLED`], each the lookup found at one of its keys: that key's value.
pub const AT_KEY: &str = "draw-param field at one of its keys";
/// Of [`SAMPLED`], each blended between two keys as its slot blends.
pub const BLENDED: &str = "draw-param field blended between two keys as the game's slot blends";
/// Of [`SAMPLED`], each between two keys whose slot takes the second key's value.
pub const SECOND_KEY: &str = "draw-param field between two keys taking the second's value (a bool, or a field Elden Ring registers without interpolation)";
/// Of [`BLENDED`], each Sekiro integer field blended at the infinite weight before its first key.
pub const INFINITE_WEIGHT: &str = "draw-param integer field before its first key on an axis that does not wrap: the processor's truncation of the game's blend, which is not a number";
/// What a sampling counts as approximated for each angle it blends.
pub const ANGLE_FUNCTIONS: &str = "draw-param angle blended with this platform's sine, cosine and arc cosine in place of the game's own, and its direction stored back as angles: the last bits may differ";

/// `sample-fromsoft-draw-params{hour}`: see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleFromSoftDrawParams {
    hour: f32,
}

impl SampleFromSoftDrawParams {
    /// The hour of the game's clock, finite, 0 to 24 (24 is midnight).
    pub fn new(hour: f32) -> Result<Self, String> {
        if hour.is_finite() && (0.0..=24.0).contains(&hour) {
            Ok(SampleFromSoftDrawParams { hour })
        } else {
            Err(format!("hour={hour} is not a finite number from 0 to 24"))
        }
    }
    pub fn hour(&self) -> f32 {
        self.hour
    }
    fn from_given(g: &Given) -> Result<Self, String> {
        let text = g.required("hour");
        Self::new(
            text.parse::<f32>()
                .map_err(|_| format!("hour={text} is not a number"))?,
        )
    }
}

/// `sample-fromsoft-draw-params`' declaration ([`primase_core::pass::PassRegistry::add`]): the hour is the program's.
pub const SAMPLE_FROMSOFT_DRAW_PARAMS: PassSpec = PassSpec {
    name: "sample-fromsoft-draw-params",
    options: &[OptionSpec::required(
        "hour",
        "12",
        "the hour of the game's clock, 0 to 24 (24 is midnight), at which each time-of-day field of the draw-param environment is sampled as the game samples it",
    )],
    make: |g| Ok(Box::new(SampleFromSoftDrawParams::from_given(g)?)),
};

impl Pass for SampleFromSoftDrawParams {
    fn name(&self) -> &'static str {
        "sample-fromsoft-draw-params"
    }
    fn options(&self) -> Options {
        Options::from([("hour".to_owned(), self.hour.to_string())])
    }
    fn requires(&self) -> &[&'static str] {
        &[DrawParam::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[DrawParam::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[TimeOfDay::NAME]
    }
    /// An area read without a draw-param file holds no environment: nothing to sample, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "angles blended between two keys with this platform's sine, cosine and arc cosine in place of the game's own, and stored back as angles"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        world.insert_resource(TimeOfDay { hour: self.hour });
        let Some(env) = world.resource_mut::<DrawParam>() else {
            log.count("areas with no draw-param environment", 1);
            return Ok(());
        };
        sample_at(env, self.hour, log)
    }
}

/// `env` at `hour` of the game's clock (0 to 24): each field of a resolved set stored at several times becomes the one
/// value the game computes, or is left out, as the module documentation states; each counted in `log`.
pub(crate) fn sample_at(env: &mut DrawParam, hour: f32, log: &mut PassLog) -> Result<(), String> {
    let axis = env.time.map_or((0.0, 0.0), |time| time.sampling());
    if !(axis.0.is_finite() && axis.1.is_finite() && axis.1 >= 0.0) {
        return Err(format!(
            "{}: a time axis of offset {} and period {}: the game's lookup does not end on a period that is negative or not a finite number",
            env.file, axis.0, axis.1
        ));
    }
    let game = env.game;
    let registered = registrations(game)?;
    for group in &mut env.groups {
        if group.chosen == SetChosen::Unresolved {
            continue;
        }
        let why = |reason: String| format!("{} {}: {reason}", group.key, group.id);
        let mut fields: Vec<DrawField> = Vec::new();
        let mut taken = vec![false; group.fields.len()];
        for first in 0..group.fields.len() {
            if taken[first] {
                continue;
            }
            let key = &group.fields[first].key;
            let same: Vec<usize> = (first..group.fields.len())
                .filter(|&i| group.fields[i].key == *key)
                .collect();
            for &i in &same {
                taken[i] = true;
            }
            if same.len() == 1 {
                fields.push(group.fields[first].clone());
                continue;
            }
            let samples: Vec<&DrawField> = same.iter().map(|&i| &group.fields[i]).collect();
            let times = keys(&samples).map_err(|e| why(format!("{key}: {e}")))?;
            let stored = samples[0].stored;
            let rule = match rule(game, registered.get(key.as_str()), stored) {
                Ok(rule) => rule,
                Err(left_out) => {
                    log.omit(&format!("draw-param field left out, {left_out}: {key}"), 1);
                    continue;
                }
            };
            let value = match find(&times, hour, axis) {
                Found::Key(i) => {
                    log.count(AT_KEY, 1);
                    samples[i].value.clone()
                }
                Found::Blend { a, b, w } => {
                    let (a, b) = (&samples[a].value, &samples[b].value);
                    match blend(rule, a, b, w).map_err(|e| why(format!("{key}: {e}")))? {
                        Blended::Second(value) => {
                            log.count(SECOND_KEY, 1);
                            value
                        }
                        Blended::Value(value) => {
                            log.count(BLENDED, 1);
                            if rule == Rule::Angle {
                                log.approximate(ANGLE_FUNCTIONS, 1);
                            }
                            if w.is_infinite() {
                                log.count(INFINITE_WEIGHT, 1);
                            }
                            value
                        }
                        Blended::LeftOut(why) => {
                            log.omit(&format!("draw-param field left out, {why}: {key}"), 1);
                            continue;
                        }
                    }
                }
            };
            let template = samples[0];
            fields.push(DrawField {
                key: template.key.clone(),
                name: template.name.clone(),
                value,
                stored,
                time: Some(hour),
            });
            log.count(SAMPLED, 1);
        }
        group.fields = fields;
    }
    Ok(())
}

/// The keys' times of the samples of one field, in stored order: each stored and finite, the samples of one type, and
/// no two within 0.002 of each other in ascending order (see the module documentation).
fn keys(samples: &[&DrawField]) -> Result<Vec<f32>, String> {
    let times = samples
        .iter()
        .map(|s| match s.time {
            Some(time) if time.is_finite() => Ok(time),
            _ => Err("a stored hour that is missing or not a finite number".to_owned()),
        })
        .collect::<Result<Vec<f32>, String>>()?;
    if samples.iter().any(|s| s.stored != samples[0].stored) {
        return Err("stored values of different types".into());
    }
    for (i, pair) in times.windows(2).enumerate() {
        let (before, after) = (pair[0], pair[1]);
        if before == after && samples[i].value != samples[i + 1].value {
            return Err(format!("conflicting values at hour {before}"));
        }
        if after - 0.001 <= before + 0.001 {
            return Err(format!(
                "keys at hours {before} and then {after}, not ascending by more than 0.002: the game's value would depend on the hour it looked up before"
            ));
        }
    }
    Ok(times)
}

/// What the game's lookup finds: one key, or a blend from key `a` to key `b` (indices) at weight `w`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Found {
    Key(usize),
    Blend { a: usize, b: usize, w: f32 },
}

/// The game's lookup at `hour` among keys at `times` (two or more, in stored order, ascending by more than 0.002) on the
/// axis `(offset, period)`, period finite and not negative: see the module documentation.
fn find(times: &[f32], hour: f32, (offset, period): (f32, f32)) -> Found {
    let n = times.len();
    if n == 1 {
        return Found::Key(0);
    }
    let wrap = period != 0.0;
    let mut t = hour;
    if wrap {
        while t > period {
            t -= period;
        }
    }
    t -= offset;
    if t < 0.0 {
        t += period;
    }
    let (first, last) = (times[0], times[n - 1]);
    for (i, &key) in times.iter().enumerate() {
        if t >= key - 0.001 && key + 0.001 >= t {
            return Found::Key(i);
        }
        if key > t {
            if wrap && i == 0 {
                // the quirk: the time since the last key would be `t + period - last`
                return Found::Blend {
                    a: n - 1,
                    b: 0,
                    w: t / ((period - last) + first),
                };
            }
            // without a wrap, before the first key: from it to itself at (t - first) / 0
            let a = i.saturating_sub(1);
            return Found::Blend {
                a,
                b: i,
                w: (t - times[a]) / (key - times[a]),
            };
        }
    }
    if wrap {
        Found::Blend {
            a: n - 1,
            b: 0,
            w: (t - last) / ((period - last) + first),
        }
    } else {
        Found::Key(n - 1)
    }
}

/// How a field's slot blends two keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// The second key (a bool; an Elden Ring field registered without interpolation).
    Second,
    /// Elden Ring's linear blend: the first key at a weight of 0 or less, the second at 1 or more or where they are equal.
    Clamped,
    /// Sekiro's linear blend, unclamped.
    Unclamped,
    /// The angle's direction turned from one key's towards the other's.
    Angle,
}

/// The blend of the slot `game` makes for a field it registers as `registered` (none: not registered), stored as
/// `stored`; or why the field is left out.
fn rule(game: Game, registered: Option<&Registered>, stored: FieldType) -> Result<Rule, String> {
    let Some(registered) = registered else {
        return Err("the game does not register it, so never reads it".into());
    };
    if registered.code != stored.code() {
        return Err(format!(
            "stored as {stored:?}, which the game registers as type {}",
            registered.code
        ));
    }
    let interpolate = registered.interpolate != Some(false);
    match (game, registered.code, registered.subtype) {
        (_, 11, _) => Ok(Rule::Second),
        (_, 9, 25) => {
            Err("the game adds a value it computes while running to its keys before blending".into())
        }
        (Game::EldenRing, _, _) if !interpolate && [3, 7, 9, 12, 13, 14].contains(&registered.code) => {
            Ok(Rule::Second)
        }
        (_, 12, 23) => Ok(Rule::Angle),
        (Game::EldenRing, 3 | 7 | 9 | 12 | 13 | 14, _) => Ok(Rule::Clamped),
        (Game::Sekiro, 3 | 7 | 9 | 12 | 13 | 14 | 15, _) => Ok(Rule::Unclamped),
        (_, code, _) => Err(format!(
            "the blend of the game's slot of type {code} has not been read"
        )),
    }
}

/// A blend's outcome.
#[derive(Clone, Debug, PartialEq)]
enum Blended {
    /// The second key's value, as its slot takes it.
    Second(Value),
    /// The slot's blend.
    Value(Value),
    /// No value this pass can state, and why: Sekiro's float blend at the infinite weight is not a number; an angle's
    /// there goes through the game's own sine and cosine of an infinite angle.
    LeftOut(&'static str),
}

/// The slot's blend by `rule` from `a` to `b` (of one type, the registered one) at weight `w`.
fn blend(rule: Rule, a: &Value, b: &Value, w: f32) -> Result<Blended, String> {
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return Err("stored values of different types".into());
    }
    if rule == Rule::Second {
        return Ok(Blended::Second(b.clone()));
    }
    if rule == Rule::Angle {
        let (Value::Vec2(a), Value::Vec2(b)) = (a, b) else {
            return Err("an angle that is not two floats".into());
        };
        return Ok(match angle(*a, *b, w) {
            Some(angle) => Blended::Value(Value::Vec2(angle)),
            None => Blended::LeftOut(
                "the game's angle blend before its first key on an axis that does not wrap takes the sine and cosine of an infinite angle, its own functions' value there not read",
            ),
        });
    }
    let clamped = rule == Rule::Clamped;
    if clamped && w <= 0.0 {
        return Ok(Blended::Value(a.clone()));
    }
    if clamped && w >= 1.0 {
        return Ok(Blended::Value(b.clone()));
    }
    if clamped && a == b {
        return Ok(Blended::Value(b.clone()));
    }
    // `(1 - w) a + w b` in 32-bit floats, as both games compute it
    let lerp = |a: f32, b: f32| (1.0 - w) * a + w * b;
    const NOT_A_NUMBER: Blended = Blended::LeftOut(
        "the game's blend before its first key on an axis that does not wrap is not a number",
    );
    let floats = |a: &[f32], b: &[f32]| -> Option<Vec<f32>> {
        let out: Vec<f32> = a.iter().zip(b).map(|(&a, &b)| lerp(a, b)).collect();
        out.iter().all(|v| !v.is_nan()).then_some(out)
    };
    let value = match (a, b) {
        (Value::Float(a), Value::Float(b)) => {
            // a 32-bit float is stored (the registered type), so the conversions are exact
            match floats(&[*a as f32], &[*b as f32]) {
                Some(v) => Value::Float(f64::from(v[0])),
                None => return Ok(NOT_A_NUMBER),
            }
        }
        (Value::Vec2(a), Value::Vec2(b)) => match floats(a, b) {
            Some(v) => Value::Vec2([v[0], v[1]]),
            None => return Ok(NOT_A_NUMBER),
        },
        (Value::Vec3(a), Value::Vec3(b)) => match floats(a, b) {
            Some(v) => Value::Vec3([v[0], v[1], v[2]]),
            None => return Ok(NOT_A_NUMBER),
        },
        (Value::Vec4(a), Value::Vec4(b)) => match floats(a, b) {
            Some(v) => Value::Vec4([v[0], v[1], v[2], v[3]]),
            None => return Ok(NOT_A_NUMBER),
        },
        (Value::Int(a), Value::Int(b)) => {
            let (Ok(a), Ok(b)) = (i32::try_from(*a), i32::try_from(*b)) else {
                return Err("a 32-bit integer out of range".into());
            };
            Value::Int(i64::from(truncate_i32(lerp(a as f32, b as f32))))
        }
        (Value::Uint(a), Value::Uint(b)) => {
            let (Ok(a), Ok(b)) = (u32::try_from(*a), u32::try_from(*b)) else {
                return Err("a 32-bit unsigned integer out of range".into());
            };
            // converted from the 64-bit value, truncated to 64 bits, and the low 32 kept
            Value::Uint(u64::from(truncate_i64(lerp(a as f32, b as f32)) as u32))
        }
        (Value::Rgba(a), Value::Rgba(b)) => Value::Rgba(std::array::from_fn(|i| {
            // each byte as Sekiro's colour slot blends it: through a float, truncated to 64 bits, the low byte kept
            truncate_i64(lerp(f32::from(a[i]), f32::from(b[i]))) as u8
        })),
        _ => return Err("a stored value the slot does not blend".into()),
    };
    Ok(Blended::Value(value))
}

/// The processor's truncation of a float to a 32-bit integer (`cvttss2si`): a value that is not a number or out of
/// range is the lowest.
fn truncate_i32(v: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&v) {
        v as i32
    } else {
        i32::MIN
    }
}

/// The processor's truncation of a float to a 64-bit integer (`cvttss2si` to a 64-bit register): a value that is not a
/// number or out of range is the lowest.
fn truncate_i64(v: f32) -> i64 {
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&v) {
        v as i64
    } else {
        i64::MIN
    }
}

/// The direction of the angle `(rx, ry)`, as the game turns a light's angle into one.
fn direction([rx, ry]: [f32; 2]) -> [f32; 3] {
    [ry.sin() * rx.cos(), -rx.sin(), ry.cos() * rx.cos()]
}

/// The angle blend from `a` to `b` at `w`: the stored key where the slot returns one, else the turned direction's
/// angles (see the module documentation); none where the turn is by an infinite weight.
fn angle(a: [f32; 2], b: [f32; 2], w: f32) -> Option<[f32; 2]> {
    let (da, db) = (direction(a), direction(b));
    if w == 0.0 {
        return Some(a);
    }
    if w == 1.0 {
        return Some(b);
    }
    let eps = f32::EPSILON;
    if (0..3).all(|i| da[i] >= db[i] - eps && db[i] + eps >= da[i]) {
        return Some(b);
    }
    if !w.is_finite() {
        return None;
    }
    let dot = |p: [f32; 3], q: [f32; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
    let turned = dot(db, da) / (dot(db, db).sqrt() * dot(da, da).sqrt());
    let (sin, cos) = (turned.acos() * w).sin_cos();
    // turn `a` towards `b` by the angle whose sine and cosine these are, in the basis of `b` and the unit part of `a`
    // across it
    let along = dot(db, da);
    if along > cos {
        return Some(b);
    }
    let across = [
        da[0] - db[0] * along,
        da[1] - db[1] * along,
        da[2] - db[2] * along,
    ];
    let length = dot(across, across);
    if 0.0001 > length {
        return Some(a);
    }
    let length = length.sqrt();
    let n = [across[0] / length, across[1] / length, across[2] / length];
    let on_n = n[1] * da[1] + n[0] * da[0] + da[2] * n[2];
    let on_b = along * cos - on_n * -sin;
    let on_n = along * -sin + on_n * cos;
    let out = [
        db[0] * on_b + n[0] * on_n,
        db[1] * on_b + n[1] * on_n,
        db[2] * on_b + n[2] * on_n,
    ];
    // the angles whose direction this is: `-sin rx` is y, and `(sin ry, cos ry)` scaled by `cos rx` are x and z
    Some([
        (-out[1]).atan2((out[0] * out[0] + out[2] * out[2]).sqrt()),
        out[0].atan2(out[2]),
    ])
}

/// A field key's registration in a game (`assets/draw_param_fields`): its GPARAM type code, its subtype, and the
/// interpolate flag Elden Ring's slots read (none for Sekiro's, which read none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Registered {
    code: u8,
    subtype: u32,
    interpolate: Option<bool>,
}

/// The field keys `game` registers, by key.
fn registrations(game: Game) -> Result<&'static BTreeMap<String, Registered>, String> {
    static SEKIRO: OnceLock<Result<BTreeMap<String, Registered>, String>> = OnceLock::new();
    static ELDEN_RING: OnceLock<Result<BTreeMap<String, Registered>, String>> = OnceLock::new();
    let table = match game {
        Game::Sekiro => {
            SEKIRO.get_or_init(|| parse(include_str!("../../../assets/draw_param_fields/sekiro.tsv")))
        }
        Game::EldenRing => {
            ELDEN_RING.get_or_init(|| parse(include_str!("../../../assets/draw_param_fields/elden_ring.tsv")))
        }
    };
    table.as_ref().map_err(Clone::clone)
}

/// A registration table: `#` lines of comment, then `key<TAB>type<TAB>subtype<TAB>interpolate` (`0`, `1`, or `-` for
/// none), a character outside ASCII in a key written `\u{hex}`.
fn parse(text: &str) -> Result<BTreeMap<String, Registered>, String> {
    let mut table = BTreeMap::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.starts_with('#')) {
        let bad = || format!("draw-param field table line {}: {line:?}", n + 1);
        let [key, code, subtype, interpolate] = line.split('\t').collect::<Vec<_>>()[..] else {
            return Err(bad());
        };
        let interpolate = match interpolate {
            "0" => Some(false),
            "1" => Some(true),
            "-" => None,
            _ => return Err(bad()),
        };
        let registered = Registered {
            code: code.parse().map_err(|_| bad())?,
            subtype: subtype.parse().map_err(|_| bad())?,
            interpolate,
        };
        if table.insert(unescape(key).ok_or_else(bad)?, registered).is_some() {
            return Err(bad());
        }
    }
    Ok(table)
}

/// `key` with each `\u{hex}` the character it names.
fn unescape(key: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = key;
    while let Some(at) = rest.find("\\u{") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 3..];
        let end = tail.find('}')?;
        out.push(char::from_u32(u32::from_str_radix(&tail[..end], 16).ok()?)?);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each game's lowering pass states its default hour and where it comes from: Sekiro's the hour a running game showed,
    /// Elden Ring's a chosen noon.
    #[test]
    fn a_lowering_pass_states_its_games_default_hour() {
        let sekiro = default_hour(Game::Sekiro);
        assert_eq!(sekiro.hour, 18.0);
        assert!(
            sekiro.source.starts_with("observed on m11_01"),
            "{}",
            sekiro.source
        );
        let elden = default_hour(Game::EldenRing);
        assert_eq!(elden.hour, 12.0);
        assert_eq!(elden.source, "no entering hour in the data; noon chosen");
    }

    use crate::sekiro::source::DrawGroup;
    use fromsoft_formats::gparam::TimeAxis;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};

    fn at(key: &str, time: f32, value: Value) -> DrawField {
        DrawField {
            key: key.into(),
            name: String::new(),
            stored: FieldType::fixture(&value),
            value,
            time: Some(time),
        }
    }

    /// Elden Ring's axis: offset 6.5, flag set (period 24).
    const ELDEN_AXIS: TimeAxis = TimeAxis {
        offset_flag: 1,
        offset: 6.5,
        period: 0.0,
    };
    /// A Sekiro axis that wraps at 24 with no offset.
    const DAY: TimeAxis = TimeAxis {
        offset_flag: 1,
        offset: 0.0,
        period: 24.0,
    };

    fn world_of(game: Game, time: Option<TimeAxis>, fields: Vec<DrawField>, chosen: SetChosen) -> World {
        let mut w = World::new();
        w.insert_resource(DrawParam {
            reinhard: None,
            game,
            file: "fixture".into(),
            version: 5,
            time,
            groups: vec![DrawGroup {
                key: "Light".into(),
                name: String::new(),
                id: 7,
                chosen,
                fields,
            }],
        });
        w
    }

    fn world(fields: Vec<DrawField>, chosen: SetChosen) -> World {
        world_of(Game::Sekiro, Some(DAY), fields, chosen)
    }

    /// The pass as a pipeline runs it, so what it changes is checked against what it declares.
    fn run(w: &mut World, hour: &str) -> Result<RunReport, String> {
        let mut registry = PassRegistry::new();
        registry.add(&SAMPLE_FROMSOFT_DRAW_PARAMS);
        let manager = PassManager::new(registry).with_pass_checks(true);
        let mut report = RunReport::default();
        let pipeline = manager
            .registry
            .parse(&format!("sample-fromsoft-draw-params{{hour={hour}}}"))
            .map_err(|e| e.to_string())?;
        manager
            .run(w, &pipeline, &mut report)
            .map_err(|e| e.to_string())?;
        Ok(report)
    }

    fn values(w: &World) -> Vec<(String, Value, Option<f32>)> {
        w.resource::<DrawParam>().unwrap().groups[0]
            .fields
            .iter()
            .map(|f| (f.key.clone(), f.value.clone(), f.time))
            .collect()
    }

    // keys each game registers, from the tables
    const ER_FLOAT: &str = "Directional Light DiffColor0"; // Vec4, interpolated
    const ER_SCALAR: &str = "Directional Light Sharpness0"; // F32, interpolated
    const ER_ENUM: &str = "ShadowParam Cascade0 Resolution"; // I32, enum: not interpolated
    const ANGLE: &str = "Directional Light Angle0";
    const BOOL: &str = "Directional Enable LocalLight1";
    const SEKIRO_INT: &str = "GS Presents - LightShaft SkyMaskMode"; // I32, enum

    #[test]
    fn the_tables_read_and_name_each_games_fields() {
        let er = registrations(Game::EldenRing).unwrap();
        let sekiro = registrations(Game::Sekiro).unwrap();
        assert_eq!((er.len(), sekiro.len()), (415, 340));
        let r = |code, subtype, interpolate| Registered {
            code,
            subtype,
            interpolate,
        };
        assert_eq!(er[ANGLE], r(12, 23, Some(true)));
        assert_eq!(er[ER_FLOAT], r(14, 20, Some(true)));
        assert_eq!(er[ER_SCALAR], r(9, 0, Some(true)));
        assert_eq!(er[ER_ENUM], r(3, 22, Some(false)));
        assert_eq!(er[BOOL], r(11, 1, Some(false)));
        assert_eq!(er["VolumetricFog Global HeightFogOffset"], r(9, 25, Some(true)));
        assert_eq!(sekiro[ANGLE], r(12, 23, None));
        assert_eq!(sekiro[SEKIRO_INT], r(3, 22, None));
        // the keys outside ASCII
        assert_eq!(
            er["Gs Presents\u{3010}Bloom\u{3011} Enable"],
            r(3, 1, Some(false))
        );
        assert!(sekiro.contains_key("[GS]Lens Simulation Param Av (\u{7d5e}\u{308a}\u{5024})"));
        assert_eq!(unescape("a\\u{3010}b"), Some("a\u{3010}b".to_owned()));
        assert_eq!(unescape("a\\u{zz}"), None);
    }

    /// Hand-worked from the lookup the module documentation states.
    #[test]
    fn the_lookup_is_the_games_with_its_quirk() {
        let elden = ELDEN_AXIS.sampling();
        let keys = [0.0, 4.0, 10.0, 17.5];
        // 12.9 is 6.4 on the axis: 0.4 of the way from 4 to 10
        assert_eq!(
            find(&keys, 12.9, elden),
            Found::Blend {
                a: 1,
                b: 2,
                w: ((12.9f32 - 6.5) - 4.0) / 6.0
            }
        );
        // 6:30 is the axis's 0, and a key within 0.001 is the key
        assert_eq!(find(&keys, 6.5, elden), Found::Key(0));
        assert_eq!(find(&keys, 6.5005, elden), Found::Key(0));
        // 2:00 is 19.5 on the axis, past the last key: 2 hours into the 6.5 to the first key
        assert_eq!(
            find(&keys, 2.0, elden),
            Found::Blend {
                a: 3,
                b: 0,
                w: 2.0 / 6.5
            }
        );
        // midnight is 17.5: the last key
        assert_eq!(find(&keys, 24.0, elden), Found::Key(3));
        // the quirk: keys at 2 and 22 of a day; at 1 the time since 22 is 3 of the 4 hours, the game's weight 1/4
        let day = DAY.sampling();
        assert_eq!(find(&[2.0, 22.0], 1.0, day), Found::Blend { a: 1, b: 0, w: 0.25 });
        assert_eq!(find(&[2.0, 22.0], 0.0, day), Found::Blend { a: 1, b: 0, w: 0.0 });
        assert_eq!(find(&[2.0, 22.0], 24.0, day), Found::Blend { a: 1, b: 0, w: 0.5 });
        // no wrap: before the first key the infinite weight, past the last the last key
        let none = (0.0, 0.0);
        assert_eq!(
            find(&[6.0, 18.0], 3.0, none),
            Found::Blend {
                a: 0,
                b: 0,
                w: f32::NEG_INFINITY
            }
        );
        assert_eq!(find(&[6.0, 18.0], 20.0, none), Found::Key(1));
        assert_eq!(
            find(&[6.0, 18.0], 12.0, none),
            Found::Blend { a: 0, b: 1, w: 0.5 }
        );
        // a period stored with the flag clear: Sekiro's 24 without the flag wraps at it too
        let stored = TimeAxis {
            offset_flag: 0,
            offset: 9.0,
            period: 24.0,
        };
        assert_eq!(stored.sampling(), (0.0, 24.0));
    }

    #[test]
    fn elden_ring_blends_at_its_flags_and_takes_the_second_key_without_them() {
        let fields = vec![
            at(ER_SCALAR, 0.0, Value::Float(2.0)),
            at(ER_FLOAT, 0.0, Value::Vec4([0., 0., 0., 1.])),
            at(ER_ENUM, 0.0, Value::Int(2048)),
            at(BOOL, 0.0, Value::Bool(false)),
            at(ER_SCALAR, 12.0, Value::Float(8.0)),
            at(ER_FLOAT, 12.0, Value::Vec4([1., 2., 4., 1.])),
            at(ER_ENUM, 12.0, Value::Int(4096)),
            at(BOOL, 12.0, Value::Bool(true)),
        ];
        // 9:30 is 3 on the axis: a quarter of the way from 0 to 12
        let mut w = world_of(
            Game::EldenRing,
            Some(ELDEN_AXIS),
            fields.clone(),
            SetChosen::OnlySet,
        );
        let report = run(&mut w, "9.5").unwrap();
        assert_eq!(
            values(&w),
            [
                (ER_SCALAR.to_owned(), Value::Float(3.5), Some(9.5)),
                (ER_FLOAT.to_owned(), Value::Vec4([0.25, 0.5, 1., 1.]), Some(9.5)),
                (ER_ENUM.to_owned(), Value::Int(4096), Some(9.5)),
                (BOOL.to_owned(), Value::Bool(true), Some(9.5)),
            ]
        );
        let log = &report.steps[0].log;
        assert_eq!(log.counts.get(BLENDED), Some(&2));
        assert_eq!(log.counts.get(SECOND_KEY), Some(&2));
        assert_eq!(log.counts.get(SAMPLED), Some(&4));
        assert!(log.approximations.is_empty(), "{:?}", log.approximations);
        // 18:30 is 12 on the axis: the second keys, stored
        let mut w = world_of(
            Game::EldenRing,
            Some(ELDEN_AXIS),
            fields.clone(),
            SetChosen::OnlySet,
        );
        let report = run(&mut w, "18.5").unwrap();
        assert_eq!(values(&w)[0].1, Value::Float(8.0));
        assert_eq!(values(&w)[2].1, Value::Int(4096));
        assert_eq!(report.steps[0].log.counts.get(AT_KEY), Some(&4));
        // 3:30 is 21: from the last key to the first (the axis wraps), 9 of the 12 hours, three quarters of the way;
        // the bool and the enum take the first key's value, the second of that blend
        let mut w = world_of(Game::EldenRing, Some(ELDEN_AXIS), fields, SetChosen::OnlySet);
        run(&mut w, "3.5").unwrap();
        assert_eq!(
            values(&w).into_iter().map(|v| v.1).collect::<Vec<_>>(),
            [
                Value::Float(3.5),
                Value::Vec4([0.25, 0.5, 1., 1.]),
                Value::Int(2048),
                Value::Bool(false)
            ]
        );
    }

    /// Elden Ring's clamp and equality: before its first key a file whose axis did not wrap would take the first key, and
    /// integers blend through floats and truncate.
    #[test]
    fn elden_ring_clamps_and_truncates() {
        let rule = Rule::Clamped;
        let (a, b) = (Value::Float(2.0), Value::Float(8.0));
        assert_eq!(
            blend(rule, &a, &b, f32::NEG_INFINITY),
            Ok(Blended::Value(a.clone()))
        );
        assert_eq!(blend(rule, &a, &b, 1.5), Ok(Blended::Value(b.clone())));
        assert_eq!(blend(rule, &a, &b, 0.5), Ok(Blended::Value(Value::Float(5.0))));
        // 6 to 16 a tenth of the way is 7, which 32-bit floats round to 6.9999995 (numpy's float32 gives the same):
        // truncated to 6
        assert_eq!(
            blend(rule, &Value::Int(6), &Value::Int(16), 0.1),
            Ok(Blended::Value(Value::Int(6)))
        );
        assert_eq!(
            blend(rule, &Value::Uint(6), &Value::Uint(16), 0.1),
            Ok(Blended::Value(Value::Uint(6)))
        );
        assert_eq!(
            blend(rule, &Value::Uint(7), &Value::Uint(7), 0.1),
            Ok(Blended::Value(Value::Uint(7)))
        );
    }

    /// Sekiro blends every type but the bool linearly and unclamped: on an axis that does not wrap, before the first key
    /// a float is not a number (left out) and an integer the processor's truncation of one.
    #[test]
    fn sekiro_blends_unclamped_and_before_the_first_key_without_a_wrap() {
        let fields = vec![
            at(ER_SCALAR, 6.0, Value::Float(2.0)),
            at(SEKIRO_INT, 6.0, Value::Int(1)),
            at(BOOL, 6.0, Value::Bool(false)),
            at(ER_SCALAR, 18.0, Value::Float(8.0)),
            at(SEKIRO_INT, 18.0, Value::Int(4)),
            at(BOOL, 18.0, Value::Bool(true)),
        ];
        // the enum blends: a third of the way from 1 to 4 is 2
        let mut w = world_of(Game::Sekiro, None, fields.clone(), SetChosen::OnlySet);
        run(&mut w, "10").unwrap();
        assert_eq!(
            values(&w).into_iter().map(|v| v.1).collect::<Vec<_>>(),
            [Value::Float(4.0), Value::Int(2), Value::Bool(true)]
        );
        let mut w = world_of(Game::Sekiro, None, fields, SetChosen::OnlySet);
        let report = run(&mut w, "3").unwrap();
        assert_eq!(
            values(&w),
            [
                (SEKIRO_INT.to_owned(), Value::Int(i64::from(i32::MIN)), Some(3.0)),
                (BOOL.to_owned(), Value::Bool(false), Some(3.0)),
            ]
        );
        let log = &report.steps[0].log;
        assert_eq!(log.counts.get(INFINITE_WEIGHT), Some(&1));
        assert_eq!(
            log.omitted.keys().collect::<Vec<_>>(),
            [&format!(
                "draw-param field left out, the game's blend before its first key on an axis that does not wrap is not a number: {ER_SCALAR}"
            )]
        );
        // unsigned integers and colour bytes keep the low bits of the lowest 64-bit integer: 0
        let inf = f32::NEG_INFINITY;
        let rule = Rule::Unclamped;
        assert_eq!(
            blend(rule, &Value::Uint(3), &Value::Uint(3), inf),
            Ok(Blended::Value(Value::Uint(0)))
        );
        assert_eq!(
            blend(
                rule,
                &Value::Rgba([9, 0, 255, 1]),
                &Value::Rgba([9, 0, 255, 1]),
                inf
            ),
            Ok(Blended::Value(Value::Rgba([0; 4])))
        );
        // and unclamped blends past the keys
        assert_eq!(
            blend(rule, &Value::Float(2.0), &Value::Float(8.0), 1.5),
            Ok(Blended::Value(Value::Float(11.0)))
        );
        assert_eq!(
            blend(
                rule,
                &Value::Rgba([0, 10, 200, 255]),
                &Value::Rgba([100, 20, 100, 255]),
                0.25
            ),
            Ok(Blended::Value(Value::Rgba([25, 12, 175, 255])))
        );
    }

    /// An angle turns its direction: halfway between two directions is the direction at half the angle from each, in
    /// their plane (an oracle in 64-bit floats: `(sin((1 - w) t) a + sin(w t) b) / sin t`).
    #[test]
    fn an_angle_turns_its_direction_towards_the_next_key() {
        let (a, b) = ([-1.0f32, 0.25], [-0.5f32, 1.5]);
        for w in [0.25f32, 0.5, 0.8] {
            let got = direction(angle(a, b, w).unwrap());
            let (da, db) = (direction(a), direction(b));
            let wide = |v: [f32; 3]| v.map(f64::from);
            let (da, db) = (wide(da), wide(db));
            let t = (da[0] * db[0] + da[1] * db[1] + da[2] * db[2]).acos();
            let w = f64::from(w);
            let want: Vec<f64> = (0..3)
                .map(|i| (((1.0 - w) * t).sin() * da[i] + (w * t).sin() * db[i]) / t.sin())
                .collect();
            for i in 0..3 {
                assert!(
                    (f64::from(got[i]) - want[i]).abs() < 1e-6,
                    "{w}: {got:?} {want:?}"
                );
            }
        }
        // the keys where the slot returns them: stored as they are
        assert_eq!(angle(a, b, 0.0), Some(a));
        assert_eq!(angle(a, b, 1.0), Some(b));
        assert_eq!(angle(a, a, 0.5), Some(a));
        assert_eq!(angle(a, a, f32::NEG_INFINITY), Some(a));
        // an angle a full turn from another is its direction: the second key, as stored
        let turned = [a[0], a[1] + std::f32::consts::TAU];
        assert_eq!(angle(a, turned, 0.5), Some(turned));
        // the turn by the infinite weight is the game's own sine and cosine of an infinite angle: no value
        assert_eq!(angle(a, b, f32::NEG_INFINITY), None);
        // through the pass, with the approximation counted
        let mut w = world_of(
            Game::EldenRing,
            Some(ELDEN_AXIS),
            vec![at(ANGLE, 0.0, Value::Vec2(a)), at(ANGLE, 12.0, Value::Vec2(b))],
            SetChosen::OnlySet,
        );
        let report = run(&mut w, "12.5").unwrap();
        assert_eq!(values(&w)[0].1, Value::Vec2(angle(a, b, 0.5).unwrap()));
        assert_eq!(report.steps[0].log.approximations.get(ANGLE_FUNCTIONS), Some(&1));
    }

    /// What the game does not read, or reads with what no file states, is left out by name; a field of one sample stays.
    #[test]
    fn fields_whose_value_the_game_does_not_state_are_left_out_by_name() {
        let fields = vec![
            at("Not A Field", 6.0, Value::Float(1.0)),
            at("VolumetricFog Global HeightFogOffset", 6.0, Value::Float(1.0)),
            at("ShadowParam Cascade Count", 6.0, Value::Uint(2)),
            at("Once", 3.0, Value::Int(4)),
            at("Not A Field", 18.0, Value::Float(2.0)),
            at("VolumetricFog Global HeightFogOffset", 18.0, Value::Float(2.0)),
            at("ShadowParam Cascade Count", 18.0, Value::Uint(4)),
        ];
        let mut w = world_of(Game::EldenRing, Some(ELDEN_AXIS), fields, SetChosen::ZeroId);
        let report = run(&mut w, "12").unwrap();
        assert_eq!(values(&w), [("Once".to_owned(), Value::Int(4), Some(3.0))]);
        let omitted: Vec<&String> = report.steps[0].log.omitted.keys().collect();
        assert_eq!(
            omitted,
            [
                "draw-param field left out, stored as U32, which the game registers as type 3: ShadowParam Cascade Count",
                "draw-param field left out, the game adds a value it computes while running to its keys before blending: VolumetricFog Global HeightFogOffset",
                "draw-param field left out, the game does not register it, so never reads it: Not A Field",
            ]
        );
    }

    #[test]
    fn unresolved_sets_stay_as_stored_and_bad_input_is_a_named_error() {
        let stored = vec![
            at(ER_SCALAR, 0.0, Value::Float(1.0)),
            at(ER_SCALAR, 12.0, Value::Float(2.0)),
        ];
        let mut w = world(stored.clone(), SetChosen::Unresolved);
        run(&mut w, "6").unwrap();
        assert_eq!(values(&w).len(), 2);
        for hour in ["25", "-1", "nan", "x"] {
            assert!(
                run(&mut world(stored.clone(), SetChosen::OnlySet), hour).is_err(),
                "{hour}"
            );
        }
        let conflicting = vec![
            at(ER_SCALAR, 6.0, Value::Float(1.0)),
            at(ER_SCALAR, 6.0, Value::Float(2.0)),
        ];
        assert!(
            run(&mut world(conflicting, SetChosen::OnlySet), "6")
                .unwrap_err()
                .contains("conflicting")
        );
        // keys out of order, or too close to tell apart, make the game's value depend on its previous lookup
        for times in [[12.0, 6.0], [6.0, 6.0015]] {
            let keys = vec![
                at(ER_SCALAR, times[0], Value::Float(1.0)),
                at(ER_SCALAR, times[1], Value::Float(2.0)),
            ];
            assert!(
                run(&mut world(keys, SetChosen::OnlySet), "6")
                    .unwrap_err()
                    .contains("looked up before"),
                "{times:?}"
            );
        }
        let mut untimed = stored;
        untimed[1].time = None;
        assert!(
            run(&mut world(untimed, SetChosen::OnlySet), "6")
                .unwrap_err()
                .contains("hour")
        );
        let mut w = world_of(
            Game::Sekiro,
            Some(TimeAxis {
                offset_flag: 0,
                offset: 0.0,
                period: -24.0,
            }),
            vec![],
            SetChosen::OnlySet,
        );
        assert!(run(&mut w, "6").unwrap_err().contains("period"));
    }

    #[test]
    fn an_area_without_a_draw_param_file_skips_the_step() {
        let mut w = World::new();
        let report = run(&mut w, "12").unwrap();
        let step = &report.steps[0];
        assert!(
            !step.skipped.is_empty()
                || step
                    .log
                    .counts
                    .contains_key("areas with no draw-param environment")
        );
    }
}
