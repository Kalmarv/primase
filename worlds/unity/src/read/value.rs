//! A decoded object: the value tree its type tree describes. Arrays of a primitive type are kept as typed vectors (a
//! mesh's vertex data or a terrain's heights are millions of elements).

use std::sync::Arc;

use serde_json::{Map, Value as Json};

/// A typed array of primitives.
#[derive(Clone, Debug, PartialEq)]
pub enum Array {
    Bool(Vec<bool>),
    I8(Vec<i8>),
    /// `UInt8` / `char` elements, and `TypelessData`.
    U8(Vec<u8>),
    I16(Vec<i16>),
    U16(Vec<u16>),
    I32(Vec<i32>),
    U32(Vec<u32>),
    I64(Vec<i64>),
    U64(Vec<u64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    /// Any other element type.
    Values(Vec<Value>),
}

impl Array {
    pub fn len(&self) -> usize {
        match self {
            Array::Bool(v) => v.len(),
            Array::I8(v) => v.len(),
            Array::U8(v) => v.len(),
            Array::I16(v) => v.len(),
            Array::U16(v) => v.len(),
            Array::I32(v) => v.len(),
            Array::U32(v) => v.len(),
            Array::I64(v) => v.len(),
            Array::U64(v) => v.len(),
            Array::F32(v) => v.len(),
            Array::F64(v) => v.len(),
            Array::Values(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    /// Signed integers of any width.
    Int(i64),
    /// Unsigned integers of any width.
    UInt(u64),
    Float(f32),
    Double(f64),
    String(String),
    Array(Array),
    Pair(Box<(Value, Value)>),
    /// Fields in type tree order.
    Struct(Vec<(Arc<str>, Value)>),
}

impl Value {
    /// The field `name` of a struct.
    pub fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Struct(fields) => fields.iter().find(|(k, _)| &**k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a path of field names.
    pub fn at(&self, path: &[&str]) -> Option<&Value> {
        path.iter().try_fold(self, |v, k| v.get(k))
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Int(i) => Some(i),
            Value::UInt(u) => i64::try_from(u).ok(),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Value::Float(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Array> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Array(Array::U8(b)) => Some(b),
            _ => None,
        }
    }

    /// The value as JSON, byte arrays replaced by `bytes(b)` (e.g. a length and a digest, so a comparison need not
    /// hold millions of numbers). Non-finite floats become strings (`"NaN"`, `"inf"`, `"-inf"`).
    pub fn to_json(&self, bytes: &dyn Fn(&[u8]) -> Json) -> Json {
        fn float(x: f64) -> Json {
            serde_json::Number::from_f64(x).map_or_else(|| Json::String(format!("{x}")), Json::Number)
        }
        let list = |it: &mut dyn Iterator<Item = Json>| Json::Array(it.collect());
        match self {
            Value::Bool(b) => Json::Bool(*b),
            Value::Int(i) => Json::from(*i),
            Value::UInt(u) => Json::from(*u),
            Value::Float(f) => float(f64::from(*f)),
            Value::Double(d) => float(*d),
            Value::String(s) => Json::String(s.clone()),
            Value::Pair(p) => Json::Array(vec![p.0.to_json(bytes), p.1.to_json(bytes)]),
            Value::Struct(fields) => {
                let mut m = Map::new();
                for (k, v) in fields {
                    m.insert(k.to_string(), v.to_json(bytes));
                }
                Json::Object(m)
            }
            Value::Array(a) => match a {
                Array::U8(b) => bytes(b),
                Array::Bool(v) => list(&mut v.iter().map(|&x| Json::Bool(x))),
                Array::I8(v) => list(&mut v.iter().map(|&x| Json::from(x))),
                Array::I16(v) => list(&mut v.iter().map(|&x| Json::from(x))),
                Array::U16(v) => list(&mut v.iter().map(|&x| Json::from(x))),
                Array::I32(v) => list(&mut v.iter().map(|&x| Json::from(x))),
                Array::U32(v) => list(&mut v.iter().map(|&x| Json::from(x))),
                Array::I64(v) => list(&mut v.iter().map(|&x| Json::from(x))),
                Array::U64(v) => list(&mut v.iter().map(|&x| Json::from(x))),
                Array::F32(v) => list(&mut v.iter().map(|&x| float(f64::from(x)))),
                Array::F64(v) => list(&mut v.iter().map(|&x| float(x))),
                Array::Values(v) => list(&mut v.iter().map(|x| x.to_json(bytes))),
            },
        }
    }
}
