//! Gates: correctness is part of the API.
//!
//! A gate compares two independent implementations (or one implementation and a fixture) item by item over a corpus
//! and reports how many agree exactly, how many within a stated tolerance, and which differ. Every decoder in the
//! workspace has one; every reader has a census over a game's real data. See CONTRIBUTING.md "Gates".

use std::fmt;

/// How far two outputs may differ and still pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tolerance {
    /// Bit for bit.
    Exact,
    /// Largest absolute difference allowed per element (e.g. 1 grey level against Pillow).
    Abs(f64),
}

/// One item that failed a gate.
#[derive(Clone, Debug, PartialEq)]
pub struct GateFailure {
    /// Which item (fixture name, file path, texture key).
    pub item: String,
    pub detail: String,
}

/// The outcome of a gate over a corpus.
#[derive(Clone, Debug, PartialEq)]
pub struct GateReport {
    pub name: String,
    pub tolerance: Tolerance,
    pub checked: u64,
    pub exact: u64,
    /// Not exact but within the tolerance.
    pub within: u64,
    pub failures: Vec<GateFailure>,
}

impl GateReport {
    pub fn new(name: impl Into<String>, tolerance: Tolerance) -> Self {
        GateReport {
            name: name.into(),
            tolerance,
            checked: 0,
            exact: 0,
            within: 0,
            failures: Vec::new(),
        }
    }

    /// Record an item whose largest element difference is `max_diff` (0 for exact).
    pub fn record(&mut self, item: impl Into<String>, max_diff: f64) {
        self.checked += 1;
        if max_diff == 0.0 {
            self.exact += 1;
            return;
        }
        match self.tolerance {
            Tolerance::Abs(t) if max_diff <= t => self.within += 1,
            _ => self.failures.push(GateFailure {
                item: item.into(),
                detail: format!("max difference {max_diff}"),
            }),
        }
    }

    /// Record an item that failed outright (could not be decoded, sizes differ, ...).
    pub fn fail(&mut self, item: impl Into<String>, detail: impl Into<String>) {
        self.checked += 1;
        self.failures.push(GateFailure {
            item: item.into(),
            detail: detail.into(),
        });
    }

    /// Compare two byte buffers element by element.
    pub fn compare_bytes(&mut self, item: impl Into<String>, a: &[u8], b: &[u8]) {
        if a.len() != b.len() {
            self.fail(item, format!("lengths differ: {} vs {}", a.len(), b.len()));
            return;
        }
        let max = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        self.record(item, f64::from(max));
    }

    pub fn passed(&self) -> bool {
        self.failures.is_empty() && self.checked > 0
    }

    /// Panic with the report if the gate did not pass (for tests).
    #[track_caller]
    pub fn assert_passed(&self) {
        assert!(self.passed(), "{self}");
    }
}

impl fmt::Display for GateReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "gate {}: {} checked, {} exact, {} within {:?}, {} failed",
            self.name,
            self.checked,
            self.exact,
            self.within,
            self.tolerance,
            self.failures.len()
        )?;
        for fail in self.failures.iter().take(20) {
            write!(f, "\n  {}: {}", fail.item, fail.detail)?;
        }
        if self.failures.len() > 20 {
            write!(f, "\n  ... {} more", self.failures.len() - 20)?;
        }
        Ok(())
    }
}

/// How two JSON documents compare as their consumer reads them (CONTRIBUTING.md "What correct means").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JsonCompare {
    /// Numbers compared.
    pub numbers: u64,
    /// Numbers equal in value where one side is written as an integer and the other as a float (`2` and `2.0`).
    pub integer_and_float: u64,
    /// Numbers that differ as f64 but read as the same f32.
    pub f32_equal: u64,
    /// Numbers that differ as f32 by at most the allowed ulps.
    pub within_ulps: u64,
    /// The largest f32 ulp distance among those.
    pub max_ulps: u64,
    /// The first differences, as `JSON path: what`.
    pub differences: Vec<String>,
}

impl JsonCompare {
    pub fn equal(&self) -> bool {
        self.differences.is_empty()
    }
}

/// Compare two JSON documents as parsed values: objects by key (key order ignored), arrays in order, strings,
/// booleans and null exactly, two integers exactly, other numbers by value: equal as f64, else equal as f32
/// (counted), else within `f32_ulps` f32 steps of each other (counted), else different. A negative zero differs
/// from a positive one unless `f32_ulps` allows a step.
pub fn compare_json(a: &serde_json::Value, b: &serde_json::Value, f32_ulps: u64) -> JsonCompare {
    let mut c = JsonCompare::default();
    compare_value(a, b, "$", f32_ulps, &mut c);
    c
}

/// The distance in f32 steps between two finite f32s (0 between the two zeros).
pub fn f32_ulps(x: f32, y: f32) -> u64 {
    fn ordered(v: f32) -> i64 {
        let i = i64::from(v.to_bits() as i32);
        if i < 0 { i64::from(i32::MIN) - i } else { i }
    }
    ordered(x).abs_diff(ordered(y))
}

fn compare_value(a: &serde_json::Value, b: &serde_json::Value, path: &str, ulps: u64, c: &mut JsonCompare) {
    use serde_json::Value;
    fn differ(c: &mut JsonCompare, path: &str, what: String) {
        if c.differences.len() < 50 {
            c.differences.push(format!("{path}: {what}"));
        } else if c.differences.len() == 50 {
            c.differences.push("... more".to_owned());
        }
    }
    fn kind(v: &Value) -> &'static str {
        match v {
            Value::Null => "null",
            Value::Bool(_) => "a bool",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            c.numbers += 1;
            if !x.is_f64() && !y.is_f64() {
                if x != y {
                    differ(c, path, format!("{x} vs {y}"));
                }
                return;
            }
            let (fx, fy) = (
                x.as_f64().expect("a JSON number has an f64 value"),
                y.as_f64().expect("a JSON number has an f64 value"),
            );
            if fx.to_bits() == fy.to_bits() {
                if x.is_f64() != y.is_f64() {
                    c.integer_and_float += 1;
                }
                return;
            }
            let (gx, gy) = (fx as f32, fy as f32);
            if gx.to_bits() == gy.to_bits() {
                c.f32_equal += 1;
                return;
            }
            let d = f32_ulps(gx, gy);
            if ulps > 0 && gx.is_finite() && gy.is_finite() && d <= ulps {
                c.within_ulps += 1;
                c.max_ulps = c.max_ulps.max(d);
            } else {
                differ(c, path, format!("{x} vs {y}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                differ(c, path, format!("{} vs {} items", x.len(), y.len()));
                return;
            }
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                compare_value(p, q, &format!("{path}[{i}]"), ulps, c);
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            for (k, p) in x {
                match y.get(k) {
                    Some(q) => compare_value(p, q, &format!("{path}.{k}"), ulps, c),
                    None => differ(c, path, format!("key {k:?} only in the first")),
                }
            }
            for k in y.keys() {
                if !x.contains_key(k) {
                    differ(c, path, format!("key {k:?} only in the second"));
                }
            }
        }
        _ if a == b => {}
        (Value::String(x), Value::String(y)) => differ(c, path, format!("{x:?} vs {y:?}")),
        _ => differ(c, path, format!("{} vs {}", kind(a), kind(b))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_counts_exact_within_and_failed() {
        let mut r = GateReport::new("t", Tolerance::Abs(1.0));
        r.compare_bytes("a", &[1, 2, 3], &[1, 2, 3]);
        r.compare_bytes("b", &[1, 2, 3], &[1, 3, 3]);
        r.compare_bytes("c", &[1, 2, 3], &[1, 5, 3]);
        r.compare_bytes("d", &[1], &[1, 2]);
        assert_eq!((r.checked, r.exact, r.within, r.failures.len()), (4, 1, 1, 2));
        assert!(!r.passed());
        assert!(
            !GateReport::new("empty", Tolerance::Exact).passed(),
            "a gate that checked nothing did not pass"
        );
    }

    #[test]
    fn json_compares_parsed_values() {
        let p = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        let a = p(r#"{"x": 0.30000001192092896, "y": 90, "z": [1, 2], "w": "s", "n": -0.0, "i": 7}"#);
        let b = p(r#"{"w": "s", "y": 90.0, "z": [1, 2], "x": 0.3, "n": -0.0, "i": 7}"#);
        let c = compare_json(&a, &b, 0);
        assert!(c.equal(), "{:?}", c.differences);
        assert_eq!((c.numbers, c.integer_and_float, c.f32_equal), (6, 1, 1));
        let d = compare_json(
            &a,
            &p(r#"{"x": 0.31, "y": 90, "z": [1], "q": 1, "n": 0, "i": 7.5}"#),
            0,
        );
        assert_eq!(d.differences.len(), 6, "{:?}", d.differences);
        // a step of f32 is a difference without a tolerance, counted within one
        let (one, next) = (
            p("[1.0]"),
            p(&format!("[{}]", f32::from_bits(1.0f32.to_bits() + 1))),
        );
        assert!(!compare_json(&one, &next, 0).equal());
        let w = compare_json(&one, &next, 1);
        assert!(w.equal());
        assert_eq!((w.within_ulps, w.max_ulps), (1, 1));
        // integers are exact, whatever the tolerance
        assert!(!compare_json(&p("[16777217]"), &p("[16777216]"), 4).equal());
        assert_eq!(f32_ulps(0.0, -0.0), 0);
        assert_eq!(f32_ulps(-f32::from_bits(1), f32::from_bits(1)), 2);
    }
}
