//! `sample-fromsoft-draw-params{hour}`: the area's draw-param environment (`fromsoft.draw_param`) at one hour of the day.
//!
//! The read keeps every stored sample of a field (a field with several time-of-day keys is several [`DrawField`]s of one
//! key, each with its hour). This pass is the program's choice of an hour: each such field becomes one sample at that
//! hour. Between two keys, floats and float vectors are interpolated linearly over the 24-hour day (key 24 is key 0);
//! every other type holds the preceding key. That is a preview policy, not the engine's curve (none is established), and
//! it is counted by name. Sets the read left unresolved are not part of the scene's environment and stay as stored.
use crate::gparam::Value;
use crate::sekiro::source::{DrawField, DrawParam, SetChosen};
use primase_core::Resource;
use primase_core::World;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};

/// What a lowering counts for each field it interpolated.
pub const INTERPOLATED: &str = "draw-param field interpolated linearly between its stored hours over the 24-hour day (angles included): a preview policy, not the engine's curve";
/// What a lowering counts for each field of a type that is not interpolated.
pub const HELD: &str = "draw-param field of a discrete type holds its preceding stored hour";

/// `sample-fromsoft-draw-params{hour}`: see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleFromSoftDrawParams {
    hour: f32,
}

impl SampleFromSoftDrawParams {
    /// The hour of the day, finite, 0 to 24 (24 is midnight).
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
        "the hour of the day, 0 to 24 (24 is midnight), at which each time-of-day field of the draw-param environment is sampled",
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
    /// An area read without a draw-param file holds no environment: nothing to sample, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "floats and float vectors between two stored hours interpolated linearly, angles too; other types hold the preceding hour"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(env) = world.resource_mut::<DrawParam>() else {
            log.count("areas with no draw-param environment", 1);
            return Ok(());
        };
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
                let samples = same
                    .iter()
                    .map(|&i| {
                        let field = &group.fields[i];
                        match field.time {
                            Some(time) if time.is_finite() && (0.0..=24.0).contains(&time) => {
                                Ok((time.rem_euclid(24.0), &field.value))
                            }
                            _ => Err(why(format!(
                                "{}: a stored hour that is missing or outside 0 to 24",
                                field.key
                            ))),
                        }
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let (value, how) = sample(samples, self.hour).map_err(|e| why(format!("{key}: {e}")))?;
                match how {
                    How::Stored => {}
                    How::Interpolated => log.approximate(INTERPOLATED, 1),
                    How::Held => log.approximate(HELD, 1),
                }
                let template = &group.fields[first];
                fields.push(DrawField {
                    key: template.key.clone(),
                    name: template.name.clone(),
                    value,
                    time: Some(self.hour),
                });
                log.count("draw-param fields sampled at the hour", 1);
            }
            group.fields = fields;
        }
        Ok(())
    }
}

/// How a sampled value came from the stored ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum How {
    /// A stored value, at a stored hour (or the only one): nothing approximated.
    Stored,
    Interpolated,
    Held,
}

/// The value at `hour` of keys `(hour, value)`, each hour in 0 to 24 (24 is hour 0). Two keys at one hour must hold one
/// value; keys of different types are refused.
fn sample(mut keys: Vec<(f32, &Value)>, hour: f32) -> Result<(Value, How), String> {
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    for pair in keys.windows(2) {
        if pair[0].0 == pair[1].0 && pair[0].1 != pair[1].1 {
            return Err(format!("conflicting values at hour {}", pair[0].0));
        }
    }
    keys.dedup_by(|a, b| a.0 == b.0);
    let hour = hour.rem_euclid(24.0);
    if let Some((_, value)) = keys.iter().find(|(time, _)| *time == hour) {
        return Ok(((*value).clone(), How::Stored));
    }
    let [first, ..] = keys.as_slice() else {
        return Err("no stored hours".into());
    };
    if keys.len() == 1 {
        return Ok((first.1.clone(), How::Stored));
    }
    for (a, b) in keys.iter().zip(keys.iter().cycle().skip(1)).take(keys.len()) {
        let span = (b.0 - a.0).rem_euclid(24.0);
        let elapsed = (hour - a.0).rem_euclid(24.0);
        if elapsed < span {
            return interpolate(a.1, b.1, elapsed / span);
        }
    }
    Err("no stored hours bracket the hour".into())
}

fn interpolate(a: &Value, b: &Value, t: f32) -> Result<(Value, How), String> {
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return Err("stored values of different types".into());
    }
    let lane = |a: f32, b: f32| {
        let value = a * (1.0 - t) + b * t;
        if value.is_finite() {
            Ok(value)
        } else {
            Err("a non-finite interpolated value".to_owned())
        }
    };
    let value = match (a, b) {
        (Value::Float(a), Value::Float(b)) => {
            let t = f64::from(t);
            let value = a * (1.0 - t) + b * t;
            if !value.is_finite() {
                return Err("a non-finite interpolated value".into());
            }
            Value::Float(value)
        }
        (Value::Vec2(a), Value::Vec2(b)) => Value::Vec2([lane(a[0], b[0])?, lane(a[1], b[1])?]),
        (Value::Vec3(a), Value::Vec3(b)) => {
            Value::Vec3([lane(a[0], b[0])?, lane(a[1], b[1])?, lane(a[2], b[2])?])
        }
        (Value::Vec4(a), Value::Vec4(b)) => Value::Vec4([
            lane(a[0], b[0])?,
            lane(a[1], b[1])?,
            lane(a[2], b[2])?,
            lane(a[3], b[3])?,
        ]),
        _ => return Ok((a.clone(), How::Held)),
    };
    Ok((value, How::Interpolated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::source::DrawGroup;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};

    fn at(key: &str, time: f32, value: Value) -> DrawField {
        DrawField {
            key: key.into(),
            name: String::new(),
            value,
            time: Some(time),
        }
    }

    fn world(fields: Vec<DrawField>, chosen: SetChosen) -> World {
        let mut w = World::new();
        w.insert_resource(DrawParam {
            file: "fixture".into(),
            version: 5,
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

    /// Hand-worked: keys 2 at 6 and 8 at 18 meet 5 at 12; the day wraps, so 0 lies halfway from 18 to 6 (5 again) and
    /// 24 is 0.
    #[test]
    fn a_field_is_interpolated_between_its_hours_over_the_wrap_and_counted() {
        for (hour, expected) in [("6", 2.0), ("12", 5.0), ("18", 8.0), ("0", 5.0), ("24", 5.0)] {
            let mut w = world(
                vec![
                    at("Brightness", 6.0, Value::Float(2.0)),
                    at("Brightness", 18.0, Value::Float(8.0)),
                ],
                SetChosen::OnlySet,
            );
            let report = run(&mut w, hour).unwrap();
            assert_eq!(
                values(&w),
                [(
                    "Brightness".to_owned(),
                    Value::Float(expected),
                    Some(hour.parse::<f32>().unwrap())
                )],
                "{hour}"
            );
            let log = &report.steps[0].log;
            let counted = log.approximations.get(INTERPOLATED).copied().unwrap_or(0);
            assert_eq!(
                counted,
                u64::from(hour == "12" || hour == "0" || hour == "24"),
                "{hour}: {:?}",
                log.approximations
            );
        }
    }

    #[test]
    fn vectors_interpolate_discrete_types_hold_and_a_single_sample_is_untouched() {
        let mut w = world(
            vec![
                at("Colour", 0.0, Value::Vec4([0., 0., 0., 1.])),
                at("Flag", 6.0, Value::Bool(false)),
                at("Once", 3.0, Value::Int(4)),
                at("Colour", 12.0, Value::Vec4([1., 2., 4., 1.])),
                at("Flag", 18.0, Value::Bool(true)),
            ],
            SetChosen::ZeroId,
        );
        let report = run(&mut w, "6").unwrap();
        assert_eq!(
            values(&w),
            [
                ("Colour".to_owned(), Value::Vec4([0.5, 1., 2., 1.]), Some(6.0)),
                ("Flag".to_owned(), Value::Bool(false), Some(6.0)),
                ("Once".to_owned(), Value::Int(4), Some(3.0)),
            ]
        );
        // 6 is a stored hour of Flag: stored, not held
        let log = &report.steps[0].log;
        assert_eq!(log.approximations.get(INTERPOLATED), Some(&1));
        assert_eq!(log.approximations.get(HELD), None);
        let mut w = world(
            vec![
                at("Flag", 6.0, Value::Bool(false)),
                at("Flag", 18.0, Value::Bool(true)),
            ],
            SetChosen::OnlySet,
        );
        let report = run(&mut w, "12").unwrap();
        assert_eq!(values(&w)[0].1, Value::Bool(false));
        assert_eq!(report.steps[0].log.approximations.get(HELD), Some(&1));
        let mut w = world(
            vec![
                at("Flag", 6.0, Value::Bool(false)),
                at("Flag", 18.0, Value::Bool(true)),
            ],
            SetChosen::OnlySet,
        );
        run(&mut w, "0").unwrap();
        assert_eq!(
            values(&w)[0].1,
            Value::Bool(true),
            "the wrap holds the last key of the day"
        );
    }

    #[test]
    fn unresolved_sets_stay_as_stored_and_bad_input_is_a_named_error() {
        let stored = vec![
            at("Density", 0.0, Value::Float(1.0)),
            at("Density", 12.0, Value::Float(2.0)),
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
            at("Density", 6.0, Value::Float(1.0)),
            at("Density", 6.0, Value::Float(2.0)),
        ];
        assert!(
            run(&mut world(conflicting, SetChosen::OnlySet), "6")
                .unwrap_err()
                .contains("conflicting")
        );
        let mut untimed = stored;
        untimed[1].time = None;
        assert!(
            run(&mut world(untimed, SetChosen::OnlySet), "6")
                .unwrap_err()
                .contains("hour")
        );
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
