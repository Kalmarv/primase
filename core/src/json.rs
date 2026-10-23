//! JSON views of components that never lose a value.
//!
//! `serde_json::to_value` writes a non-finite float as `null`, so a NaN in a transform would show as a value that
//! is not there. [`to_value`] refuses instead: it first walks the value with a serializer that only checks floats,
//! then converts.

use std::fmt;

use serde::Serialize;
use serde::ser::{self, Impossible};
use serde_json::Value;

/// Why a component has no faithful JSON view.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct JsonError(pub String);

impl ser::Error for JsonError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        JsonError(msg.to_string())
    }
}

/// `serde_json::to_value`, but a non-finite float (which JSON cannot hold) is an error naming where it is.
pub fn to_value<T: Serialize + ?Sized>(v: &T) -> Result<Value, JsonError> {
    v.serialize(&mut Check { path: Vec::new() })?;
    serde_json::to_value(v).map_err(|e| JsonError(e.to_string()))
}

/// JSON text of a value (two-space indented when `pretty`, with a final newline), serialized directly. Each f32 is
/// written as [`f32_decimal`] gives it, so it reads back as the same f32 both where a reader parses it as f32 and where
/// it parses f64 and narrows (a JSON value's `as_f64`, JavaScript): its shortest digits nearly always. serde_json's own
/// f32 digits read back exactly as f32 but, for two f32s, round twice through f64. A non-finite float is an error
/// naming where it is (serde_json would write `null`).
pub fn to_vec<T: Serialize + ?Sized>(v: &T, pretty: bool) -> Result<Vec<u8>, JsonError> {
    v.serialize(&mut Check { path: Vec::new() })?;
    let mut out = Vec::new();
    let written = if pretty {
        v.serialize(&mut serde_json::Serializer::with_formatter(
            &mut out,
            ExactF32(serde_json::ser::PrettyFormatter::new()),
        ))
    } else {
        v.serialize(&mut serde_json::Serializer::with_formatter(
            &mut out,
            ExactF32(serde_json::ser::CompactFormatter),
        ))
    };
    written.map_err(|e| JsonError(e.to_string()))?;
    if pretty {
        out.push(b'\n');
    }
    Ok(out)
}

/// A serde_json formatter that writes each f32 as [`f32_decimal`] (through the inner formatter's f64 form) and
/// everything else as the inner formatter does.
struct ExactF32<F>(F);

impl<F: serde_json::ser::Formatter> serde_json::ser::Formatter for ExactF32<F> {
    fn write_f32<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, value: f32) -> std::io::Result<()> {
        match f32_decimal(value) {
            Ok(d) => self.0.write_f64(writer, d),
            // the check before writing refuses a non-finite value: as serde_json would, should one get here
            Err(_) => self.0.write_null(writer),
        }
    }
    fn write_f64<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()> {
        self.0.write_f64(writer, value)
    }
    fn begin_array<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.begin_array(writer)
    }
    fn end_array<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.end_array(writer)
    }
    fn begin_array_value<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        self.0.begin_array_value(writer, first)
    }
    fn end_array_value<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.end_array_value(writer)
    }
    fn begin_object<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.begin_object(writer)
    }
    fn end_object<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.end_object(writer)
    }
    fn begin_object_key<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        self.0.begin_object_key(writer, first)
    }
    fn end_object_key<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.end_object_key(writer)
    }
    fn begin_object_value<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.begin_object_value(writer)
    }
    fn end_object_value<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        self.0.end_object_value(writer)
    }
}

/// An f32 as a JSON number that reads back as the same f32 where a reader parses the number as f64 and narrows it (as
/// serde_json and most JSON readers do): [`f32_decimal`], which serde_json writes with its shortest f64 digits. A
/// non-finite value is an error: JSON has no NaN or infinity.
pub fn f32_value(x: f32) -> Result<Value, JsonError> {
    Ok(Value::from(f32_decimal(x)?))
}

/// A finite f32 as the f64 a JSON field holds, chosen so that the number written and read back as f64, then narrowed,
/// is the same f32. That is its shortest round-trip decimal (`0.1`, not the widened `0.10000000149011612`) where the
/// f64 nearest those digits narrows back to it; where it does not (the digits' f64 lies across an f32 rounding
/// boundary, as for the bits `0x15ae43fd`: parsed straight to f32 the digits are exact, through f64 they round twice),
/// the f32's own value, exactly, whose longer digits read back without loss. A non-finite value is an error.
pub fn f32_decimal(x: f32) -> Result<f64, JsonError> {
    if !x.is_finite() {
        return Err(JsonError(format!("{x}: JSON has no non-finite numbers")));
    }
    let shortest: f64 = x
        .to_string()
        .parse()
        .map_err(|e| JsonError(format!("{x}: its decimal does not read back: {e}")))?;
    Ok(if (shortest as f32).to_bits() == x.to_bits() {
        shortest
    } else {
        f64::from(x)
    })
}

/// [`f32_value`] of each.
pub fn f32s_value(xs: &[f32]) -> Result<Value, JsonError> {
    xs.iter()
        .map(|&x| f32_value(x))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

/// An f64 as a JSON number; a non-finite value (which `Value::from` would turn into `null`) is an error.
pub fn f64_value(x: f64) -> Result<Value, JsonError> {
    if !x.is_finite() {
        return Err(JsonError(format!("{x}: JSON has no non-finite numbers")));
    }
    Ok(Value::from(x))
}

/// Walks a value, failing at the first non-finite float.
struct Check {
    path: Vec<String>,
}

impl Check {
    fn float(&self, x: f64) -> Result<(), JsonError> {
        if x.is_finite() {
            Ok(())
        } else {
            let at = if self.path.is_empty() {
                "the value".to_owned()
            } else {
                self.path.join(".")
            };
            Err(JsonError(format!("{x} at {at}: JSON has no non-finite numbers")))
        }
    }

    fn at<T: Serialize + ?Sized>(&mut self, key: String, v: &T) -> Result<(), JsonError> {
        self.path.push(key);
        let r = v.serialize(&mut *self);
        self.path.pop();
        r
    }
}

/// Seq-like compound: counts its items for the path.
struct Items<'a> {
    c: &'a mut Check,
    i: usize,
}

impl Items<'_> {
    fn item<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), JsonError> {
        let key = format!("[{}]", self.i);
        self.i += 1;
        self.c.at(key, v)
    }
}

impl<'a> ser::Serializer for &'a mut Check {
    type Ok = ();
    type Error = JsonError;
    type SerializeSeq = Items<'a>;
    type SerializeTuple = Items<'a>;
    type SerializeTupleStruct = Items<'a>;
    type SerializeTupleVariant = Items<'a>;
    type SerializeMap = Items<'a>;
    type SerializeStruct = Items<'a>;
    type SerializeStructVariant = Items<'a>;

    fn serialize_bool(self, _: bool) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_i8(self, _: i8) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_i16(self, _: i16) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_i32(self, _: i32) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_i64(self, _: i64) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_u8(self, _: u8) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_u16(self, _: u16) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_u32(self, _: u32) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_u64(self, _: u64) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_i128(self, _: i128) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_u128(self, _: u128) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> Result<(), JsonError> {
        self.float(f64::from(v))
    }
    fn serialize_f64(self, v: f64) -> Result<(), JsonError> {
        self.float(v)
    }
    fn serialize_char(self, _: char) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_str(self, _: &str) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_none(self) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<(), JsonError> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, _: &'static str) -> Result<(), JsonError> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<(), JsonError> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<(), JsonError> {
        self.at(variant.to_owned(), v)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Items<'a>, JsonError> {
        Ok(Items { c: self, i: 0 })
    }
    fn serialize_tuple(self, _: usize) -> Result<Items<'a>, JsonError> {
        Ok(Items { c: self, i: 0 })
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Items<'a>, JsonError> {
        Ok(Items { c: self, i: 0 })
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Items<'a>, JsonError> {
        Ok(Items { c: self, i: 0 })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Items<'a>, JsonError> {
        Ok(Items { c: self, i: 0 })
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Items<'a>, JsonError> {
        Ok(Items { c: self, i: 0 })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Items<'a>, JsonError> {
        Ok(Items { c: self, i: 0 })
    }
}

impl ser::SerializeSeq for Items<'_> {
    type Ok = ();
    type Error = JsonError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), JsonError> {
        self.item(v)
    }
    fn end(self) -> Result<(), JsonError> {
        Ok(())
    }
}

impl ser::SerializeTuple for Items<'_> {
    type Ok = ();
    type Error = JsonError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), JsonError> {
        self.item(v)
    }
    fn end(self) -> Result<(), JsonError> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for Items<'_> {
    type Ok = ();
    type Error = JsonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), JsonError> {
        self.item(v)
    }
    fn end(self) -> Result<(), JsonError> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for Items<'_> {
    type Ok = ();
    type Error = JsonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), JsonError> {
        self.item(v)
    }
    fn end(self) -> Result<(), JsonError> {
        Ok(())
    }
}

/// Map keys are checked with a serializer that takes only strings (what JSON keys can be), values by path.
impl ser::SerializeMap for Items<'_> {
    type Ok = ();
    type Error = JsonError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, k: &T) -> Result<(), JsonError> {
        // the key names the next value in error paths
        self.c
            .path
            .push(k.serialize(KeyText).unwrap_or_else(|_| "?".to_owned()));
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), JsonError> {
        let r = v.serialize(&mut *self.c);
        self.c.path.pop();
        r
    }
    fn end(self) -> Result<(), JsonError> {
        Ok(())
    }
}

impl ser::SerializeStruct for Items<'_> {
    type Ok = ();
    type Error = JsonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result<(), JsonError> {
        self.c.at(key.to_owned(), v)
    }
    fn end(self) -> Result<(), JsonError> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for Items<'_> {
    type Ok = ();
    type Error = JsonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result<(), JsonError> {
        self.c.at(key.to_owned(), v)
    }
    fn end(self) -> Result<(), JsonError> {
        Ok(())
    }
}

/// A map key's text, for error paths (strings and integers; anything else is `?`).
struct KeyText;

macro_rules! key_int {
    ($($f:ident: $t:ty),*) => {$(
        fn $f(self, v: $t) -> Result<String, JsonError> {
            Ok(v.to_string())
        }
    )*};
}

impl ser::Serializer for KeyText {
    type Ok = String;
    type Error = JsonError;
    type SerializeSeq = Impossible<String, JsonError>;
    type SerializeTuple = Impossible<String, JsonError>;
    type SerializeTupleStruct = Impossible<String, JsonError>;
    type SerializeTupleVariant = Impossible<String, JsonError>;
    type SerializeMap = Impossible<String, JsonError>;
    type SerializeStruct = Impossible<String, JsonError>;
    type SerializeStructVariant = Impossible<String, JsonError>;

    key_int!(serialize_i8: i8, serialize_i16: i16, serialize_i32: i32, serialize_i64: i64, serialize_u8: u8,
        serialize_u16: u16, serialize_u32: u32, serialize_u64: u64, serialize_bool: bool, serialize_char: char,
        serialize_f32: f32, serialize_f64: f64);

    fn serialize_str(self, v: &str) -> Result<String, JsonError> {
        Ok(v.to_owned())
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<String, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_none(self) -> Result<String, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<String, JsonError> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<String, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_unit_struct(self, n: &'static str) -> Result<String, JsonError> {
        Ok(n.to_owned())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, v: &'static str) -> Result<String, JsonError> {
        Ok(v.to_owned())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<String, JsonError> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<String, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct, JsonError> {
        Err(JsonError("key".into()))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, JsonError> {
        Err(JsonError("key".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct T {
        cols: [[f32; 3]; 2],
        m: BTreeMap<u32, Vec<f64>>,
        o: Option<f32>,
    }

    /// The writer's number form reads back bit-exact through serde_json's f64 parse (what our reader and most glTF
    /// readers do), over every exponent and a spread of mantissas, plus the edge values.
    #[test]
    fn f32_text_round_trips() {
        let mut x: u32 = 0x9E37_79B9;
        let check = |bits: u32| {
            let f = f32::from_bits(bits);
            if !f.is_finite() {
                return;
            }
            let text = serde_json::to_string(&f32_value(f).unwrap()).unwrap();
            let back: Value = serde_json::from_str(&text).unwrap();
            let g = back.as_f64().unwrap() as f32;
            assert_eq!(g.to_bits(), bits, "{f:?} -> {text} -> {g:?}");
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
        assert_eq!(serde_json::to_string(&f32_value(-0.0).unwrap()).unwrap(), "-0.0");
        assert_eq!(serde_json::to_string(&f32_value(0.1).unwrap()).unwrap(), "0.1");
        // the two the review found: their shortest digits round twice through f64, so their own value is written
        for bits in [0x15ae_43fd, 0x95ae_43fd] {
            check(bits);
            let f = f32::from_bits(bits);
            assert_eq!(f32_decimal(f).unwrap(), f64::from(f), "{bits:#x}");
        }
        assert!(
            f32_value(f32::NAN).is_err()
                && f32_decimal(f32::INFINITY).is_err()
                && f64_value(f64::NAN).is_err()
        );
    }

    /// JSON text writes each f32 so that it reads back both as f32 and through f64: the two whose shortest digits round
    /// twice through f64, as fields of a struct, compact and pretty; and an f32 field keeps its short digits where
    /// those are exact.
    #[test]
    fn json_text_f32_fields_read_back_through_f64() {
        #[derive(Serialize)]
        struct R {
            x: f32,
            v: Vec<f32>,
        }
        #[derive(serde::Deserialize)]
        struct R2 {
            v: Vec<f32>,
        }
        let hard: Vec<f32> = [0x15ae_43fd, 0x95ae_43fd].map(f32::from_bits).to_vec();
        for pretty in [false, true] {
            let text = to_vec(
                &R {
                    x: 0.1,
                    v: hard.clone(),
                },
                pretty,
            )
            .unwrap();
            let back: serde_json::Value = serde_json::from_slice(&text).unwrap();
            assert_eq!(back["x"].as_f64().unwrap() as f32, 0.1);
            assert!(
                String::from_utf8_lossy(&text).contains("0.1"),
                "{}",
                String::from_utf8_lossy(&text)
            );
            for (k, f) in hard.iter().enumerate() {
                assert_eq!(
                    (back["v"][k].as_f64().unwrap() as f32).to_bits(),
                    f.to_bits(),
                    "pretty {pretty}"
                );
                // and as f32, directly
                let r: Vec<f32> = serde_json::from_slice::<R2>(&text).unwrap().v;
                assert_eq!(r[k].to_bits(), f.to_bits());
            }
        }
    }

    /// 128-bit integers are numbers the check lets through, as serde_json writes them.
    #[test]
    fn wide_integers_pass_the_check() {
        assert_eq!(to_value(&5i128).unwrap(), Value::from(5));
        assert_eq!(
            to_vec(&(u128::from(u64::MAX) + 1), false).unwrap(),
            b"18446744073709551616"
        );
    }

    /// Direct JSON text keeps f32 fields' own digits and refuses a non-finite value.
    #[test]
    fn json_text_is_exact_and_finite() {
        #[derive(Serialize)]
        struct R {
            x: f32,
            v: [f32; 2],
            d: f64,
        }
        let r = R {
            x: 0.1,
            v: [-0.0, 1e-30],
            d: 0.1,
        };
        assert_eq!(
            String::from_utf8(to_vec(&r, false).unwrap()).unwrap(),
            r#"{"x":0.1,"v":[-0.0,1e-30],"d":0.1}"#
        );
        assert!(to_vec(&r, true).unwrap().ends_with(b"}\n"));
        let bad = R { x: f32::NAN, ..r };
        assert_eq!(
            to_vec(&bad, false).unwrap_err().0,
            "NaN at x: JSON has no non-finite numbers"
        );
    }

    #[test]
    fn non_finite_floats_are_errors_with_their_path() {
        let ok = T {
            cols: [[1., 2., 3.], [4., 5., 6.]],
            m: BTreeMap::from([(7, vec![0.5])]),
            o: Some(-0.0),
        };
        assert_eq!(
            to_value(&ok).unwrap(),
            serde_json::json!({"cols": [[1., 2., 3.], [4., 5., 6.]], "m": {"7": [0.5]}, "o": -0.0})
        );
        let mut bad = ok;
        bad.cols[1][2] = f32::NAN;
        assert_eq!(
            to_value(&bad).unwrap_err().0,
            "NaN at cols.[1].[2]: JSON has no non-finite numbers"
        );
        bad.cols[1][2] = 0.0;
        bad.m.insert(9, vec![1.0, f64::INFINITY]);
        assert_eq!(
            to_value(&bad).unwrap_err().0,
            "inf at m.9.[1]: JSON has no non-finite numbers"
        );
        bad.m.remove(&9);
        bad.o = Some(f32::NEG_INFINITY);
        assert_eq!(
            to_value(&bad).unwrap_err().0,
            "-inf at o: JSON has no non-finite numbers"
        );
    }
}
