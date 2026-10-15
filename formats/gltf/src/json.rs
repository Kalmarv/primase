//! JSON helpers: exact f32 numbers for the writer, checked field access for the strict reader.

use serde_json::{Map, Number, Value};

use crate::{Error, Result};

/// An f32 as a JSON number in its shortest round-trip form (`0.1`, `-0.0`, `1e-30`). JSON text written this way
/// reads back as the same f32 through an f64 parse (a decimal of at most 9 significant digits is exact in f64, and
/// its nearest f32 is the original; `tests::f32_text_round_trips` checks it). Panics on a non-finite value: callers
/// check finiteness first, since JSON has no NaN or infinity.
pub fn f32_value(x: f32) -> Value {
    assert!(x.is_finite(), "non-finite {x} reached JSON");
    // `{:?}` keeps the sign of zero and the decimal point, so the number stays a float in serde_json.
    Value::Number(
        format!("{x:?}")
            .parse::<Number>()
            .expect("Debug of a finite f32 is a JSON number"),
    )
}

pub fn f32s_value(xs: &[f32]) -> Value {
    Value::Array(xs.iter().map(|&x| f32_value(x)).collect())
}

/// A JSON object being read, with the path used in errors (`meshes[3].primitives[0]`).
#[derive(Clone, Copy, Debug)]
pub struct Obj<'a> {
    pub map: &'a Map<String, Value>,
    pub path: &'a str,
}

impl<'a> Obj<'a> {
    pub fn new(v: &'a Value, path: &'a str) -> Result<Self> {
        match v {
            Value::Object(map) => Ok(Obj { map, path }),
            _ => Err(invalid(path, "not an object")),
        }
    }

    /// Error on any key outside `allowed` (strict reader: what we don't understand is not skipped silently), and on
    /// a non-empty `extensions` object (an extension can change what the other fields mean).
    pub fn only(&self, allowed: &[&str]) -> Result<()> {
        for (k, v) in self.map {
            if k == "extensions" {
                if v.as_object().is_some_and(|m| m.is_empty()) {
                    continue;
                }
                return Err(invalid(self.path, &format!("unsupported extensions {v}")));
            }
            if k != "extras" && !allowed.contains(&k.as_str()) {
                return Err(invalid(self.path, &format!("unsupported property {k:?}")));
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key)
    }

    pub fn u64_opt(&self, key: &str) -> Result<Option<u64>> {
        match self.map.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| invalid(self.path, &format!("{key} is not a non-negative integer"))),
        }
    }

    pub fn u64(&self, key: &str) -> Result<u64> {
        self.u64_opt(key)?
            .ok_or_else(|| invalid(self.path, &format!("missing {key}")))
    }

    pub fn usize(&self, key: &str) -> Result<usize> {
        usize::try_from(self.u64(key)?).map_err(|_| invalid(self.path, &format!("{key} too large")))
    }

    pub fn usize_opt(&self, key: &str) -> Result<Option<usize>> {
        self.u64_opt(key)?
            .map(|v| usize::try_from(v).map_err(|_| invalid(self.path, &format!("{key} too large"))))
            .transpose()
    }

    pub fn str_opt(&self, key: &str) -> Result<Option<&'a str>> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(invalid(self.path, &format!("{key} is not a string"))),
        }
    }

    pub fn bool_opt(&self, key: &str) -> Result<Option<bool>> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(invalid(self.path, &format!("{key} is not a boolean"))),
        }
    }

    pub fn f32s_opt(&self, key: &str) -> Result<Option<Vec<f32>>> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_f64()
                        .map(|x| x as f32)
                        .filter(|x| x.is_finite())
                        .ok_or_else(|| invalid(self.path, &format!("{key} holds a non-number")))
                })
                .collect::<Result<Vec<f32>>>()
                .map(Some),
            Some(_) => Err(invalid(self.path, &format!("{key} is not an array"))),
        }
    }
}

pub fn invalid(item: &str, detail: &str) -> Error {
    Error::Invalid {
        item: item.to_owned(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer's number form reads back bit-exact through serde_json's f64 parse (what our reader and most glTF
    /// readers do), over every exponent and a spread of mantissas, plus the edge values.
    #[test]
    fn f32_text_round_trips() {
        let mut checked = 0u64;
        let mut x: u32 = 0x9E37_79B9;
        let mut check = |bits: u32| {
            let f = f32::from_bits(bits);
            if !f.is_finite() {
                return;
            }
            let text = serde_json::to_string(&f32_value(f)).unwrap();
            let back: Value = serde_json::from_str(&text).unwrap();
            let g = back.as_f64().unwrap() as f32;
            assert_eq!(g.to_bits(), bits, "{f:?} -> {text} -> {g:?}");
            checked += 1;
        };
        for bits in [
            0,
            0x8000_0000,
            1,
            0x8000_0001,
            0x007F_FFFF,
            0x0080_0000,
            0x7F7F_FFFF,
            0xFF7F_FFFF,
        ] {
            check(bits);
        }
        for _ in 0..2_000_000 {
            // xorshift32: every exponent, sign and mantissa pattern is reached.
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            check(x);
        }
        assert!(checked > 1_990_000);
        assert_eq!(serde_json::to_string(&f32_value(-0.0)).unwrap(), "-0.0");
        assert_eq!(serde_json::to_string(&f32_value(0.1)).unwrap(), "0.1");
    }
}
