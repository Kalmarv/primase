//! JSON helpers: checked field access for the strict reader.

use serde_json::{Map, Value};

use crate::{Error, Result};

/// A JSON object being read, with the path used in errors (`meshes[3].primitives()[0]`).
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
    /// a non-empty `extensions` object (an extension can change what the other fields mean). `extras` is a key like any
    /// other: an object that has somewhere to keep it lists it, one that has not refuses it.
    pub fn only(&self, allowed: &[&str]) -> Result<()> {
        for (k, v) in self.map {
            if k == "extensions" {
                if v.as_object().is_some_and(|m| m.is_empty()) {
                    continue;
                }
                return Err(invalid(self.path, &format!("unsupported extensions {v}")));
            }
            if !allowed.contains(&k.as_str()) {
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
