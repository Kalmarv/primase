//! Program-selected static sampling of GPARAM's stored time-of-day values.
//! Interpolation between keys is a preview policy, not an established engine spline.
use super::{Field, Value};

impl Field {
    /// Sample one value id at an hour in 0..=24 (24 is midnight). Floating-point scalars/vectors
    /// interpolate linearly over the 24-hour wrap; discrete values hold the preceding key.
    /// No value from another id participates. A single value is constant. Conflicting keys at
    /// the same hour, missing times for a varying field, and invalid times are refused by name.
    pub fn sample_at_hour(&self, id: i32, hour: f32) -> Result<Option<Value>, String> {
        if !hour.is_finite() || !(0.0..=24.0).contains(&hour) {
            return Err("hour must be finite and in 0..=24".into());
        }
        let indices: Vec<_> = self.indices_of(id).collect();
        let Some(&first) = indices.first() else { return Ok(None); };
        if indices.len() == 1 { return Ok(self.values.get(first)); }
        let why = |reason: &str| format!("GPARAM field {} id {id}: {reason}", self.key);
        let mut samples = Vec::new();
        for index in indices {
            let time = *self.times.get(index).ok_or_else(|| why("varying value has no stored hour"))?;
            if !time.is_finite() || !(0.0..=24.0).contains(&time) {
                return Err(why("stored hour is not finite or outside 0..=24"));
            }
            let value = self.values.get(index).ok_or_else(|| why("value index outside stored values"))?;
            samples.push((time.rem_euclid(24.0), value));
        }
        samples.sort_by(|a,b| a.0.total_cmp(&b.0));
        for pair in samples.windows(2) {
            if let [a,b] = pair && a.0 == b.0 && a.1 != b.1 {
                return Err(why("conflicting values at the same hour"));
            }
        }
        samples.dedup_by(|a,b| a.0 == b.0);
        let hour = hour.rem_euclid(24.0);
        if samples.len() == 1 { return Ok(samples.first().map(|(_,value)| value.clone())); }
        if let Some((_,value)) = samples.iter().find(|(time,_)| *time == hour) { return Ok(Some(value.clone())); }
        for (a,b) in samples.iter().zip(samples.iter().cycle().skip(1)).take(samples.len()) {
            let span = (b.0-a.0).rem_euclid(24.0);
            let elapsed = (hour-a.0).rem_euclid(24.0);
            if elapsed < span {
                return interpolate(&a.1,&b.1,elapsed/span).map(Some).map_err(why);
            }
        }
        Err(why("no bracketing time keys"))
    }
}

fn interpolate(a: &Value, b: &Value, t: f32) -> Result<Value, &'static str> {
    let lane = |a:f32,b:f32| {
        let value = a*(1.0-t)+b*t;
        if value.is_finite() { Ok(value) } else { Err("nonfinite interpolated value") }
    };
    Ok(match (a,b) {
        (Value::Float(a),Value::Float(b)) => {
            let t = f64::from(t); let value = a*(1.0-t)+b*t;
            if !value.is_finite() { return Err("nonfinite interpolated value"); }
            Value::Float(value)
        }
        (Value::Vec2(a),Value::Vec2(b)) => Value::Vec2([lane(a[0],b[0])?,lane(a[1],b[1])?]),
        (Value::Vec3(a),Value::Vec3(b)) => Value::Vec3([lane(a[0],b[0])?,lane(a[1],b[1])?,lane(a[2],b[2])?]),
        (Value::Vec4(a),Value::Vec4(b)) => Value::Vec4([lane(a[0],b[0])?,lane(a[1],b[1])?,lane(a[2],b[2])?,lane(a[3],b[3])?]),
        _ => a.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gparam::Values;
    fn field() -> Field {
        Field { key:"Light".into(),name:String::new(),ids:vec![7,9,7],times:vec![6.0,12.0,18.0],
            values:Values::F32(vec![2.0,99.0,8.0]) }
    }
    #[test]
    fn hour_sampling_uses_only_the_selected_id_and_wraps_midnight() {
        let f=field();
        for (hour,value) in [(6.0,2.0),(12.0,5.0),(18.0,8.0),(0.0,5.0),(24.0,5.0)] {
            assert_eq!(f.sample_at_hour(7,hour).unwrap(),Some(Value::Float(value)));
        }
        assert_eq!(f.sample_at_hour(9,0.0).unwrap(),Some(Value::Float(99.0)));
        assert_eq!(f.sample_at_hour(1,12.0).unwrap(),None);
    }
    #[test]
    fn discrete_values_hold_the_previous_key_including_the_wrap() {
        let f=Field { ids:vec![7,7],times:vec![6.0,18.0],values:Values::Bool(vec![false,true]),..field() };
        assert_eq!(f.sample_at_hour(7,12.0).unwrap(),Some(Value::Bool(false)));
        assert_eq!(f.sample_at_hour(7,0.0).unwrap(),Some(Value::Bool(true)));
    }
    #[test]
    fn ambiguous_times_and_invalid_hour_are_named_errors() {
        let mut f=field(); f.times=vec![0.0,12.0,24.0];
        assert!(f.sample_at_hour(7,1.0).unwrap_err().contains("conflicting"));
        for hour in [f32::NAN,f32::INFINITY,-1.0,25.0] { assert!(f.sample_at_hour(7,hour).is_err()); }
        f.times.clear(); assert!(f.sample_at_hour(7,1.0).unwrap_err().contains("no stored hour"));
    }
}
