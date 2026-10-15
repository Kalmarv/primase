//! A strict JSON reader and a deterministic writer that keep numbers as their text.
//!
//! The manifest's number fields are read as f32 by the reference reader straight from the decimal text, others as
//! f64 or as integers; going through one f64 for everything could round a float32 twice. So numbers stay text until
//! the field that reads them knows its type, and are written back from a typed value as its shortest round-trip
//! form ([`f32_text`], [`f64_text`]).
//!
//! Accepted: RFC 8259 JSON plus `//` and `/* */` comments (the reference reader skips them) and a UTF-8 byte-order
//! mark. Refused: duplicate keys, trailing commas, nesting deeper than 64, lone surrogates, control characters in
//! strings, anything after the value.

use std::fmt::Write as _;

use serde_json::Value;

/// A parsed JSON value. Objects keep their key order.
#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    /// The number's text, as written (valid JSON number grammar).
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn kind(&self) -> &'static str {
        match self {
            J::Null => "null",
            J::Bool(_) => "a bool",
            J::Num(_) => "a number",
            J::Str(_) => "a string",
            J::Arr(_) => "an array",
            J::Obj(_) => "an object",
        }
    }

    pub fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn num(text: String) -> J {
        J::Num(text)
    }

    pub fn f32(x: f32) -> J {
        J::Num(f32_text(x))
    }

    pub fn f64(x: f64) -> J {
        J::Num(f64_text(x))
    }

    pub fn int(x: impl Into<i64>) -> J {
        J::Num(x.into().to_string())
    }

    pub fn uint(x: u64) -> J {
        J::Num(x.to_string())
    }

    pub fn str(s: impl Into<String>) -> J {
        J::Str(s.into())
    }
}

/// A parse error with where it happened.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {col}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

const MAX_DEPTH: usize = 64;

/// Parse a whole document.
pub fn parse(bytes: &[u8]) -> Result<J, ParseError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut p = Parser { b: bytes, i: 0 };
    p.ws()?;
    let v = p.value(0)?;
    p.ws()?;
    if p.i != p.b.len() {
        return Err(p.err("text after the JSON value"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, message: impl Into<String>) -> ParseError {
        let upto = &self.b[..self.i.min(self.b.len())];
        let line = upto.iter().filter(|&&c| c == b'\n').count() + 1;
        let col = upto.iter().rev().take_while(|&&c| c != b'\n').count() + 1;
        ParseError {
            line,
            col,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.i += 1,
                Some(b'/') if self.b.get(self.i + 1) == Some(&b'/') => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                Some(b'/') if self.b.get(self.i + 1) == Some(&b'*') => {
                    let start = self.i;
                    self.i += 2;
                    loop {
                        match self.peek() {
                            None => {
                                self.i = start;
                                return Err(self.err("unterminated comment"));
                            }
                            Some(b'*') if self.b.get(self.i + 1) == Some(&b'/') => {
                                self.i += 2;
                                break;
                            }
                            _ => self.i += 1,
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn lit(&mut self, word: &[u8], v: J) -> Result<J, ParseError> {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("unexpected character"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<J, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested deeper than 64"));
        }
        match self.peek() {
            None => Err(self.err("unexpected end")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(J::Str(self.string()?)),
            Some(b't') => self.lit(b"true", J::Bool(true)),
            Some(b'f') => self.lit(b"false", J::Bool(false)),
            Some(b'n') => self.lit(b"null", J::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<J, ParseError> {
        self.i += 1;
        let mut kv: Vec<(String, J)> = Vec::new();
        self.ws()?;
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(J::Obj(kv));
        }
        loop {
            self.ws()?;
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a key"));
            }
            let at = self.i;
            let k = self.string()?;
            if kv.iter().any(|(x, _)| *x == k) {
                self.i = at;
                return Err(self.err(format!("key {k:?} twice")));
            }
            self.ws()?;
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':'"));
            }
            self.i += 1;
            self.ws()?;
            let v = self.value(depth + 1)?;
            kv.push((k, v));
            self.ws()?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(J::Obj(kv));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<J, ParseError> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws()?;
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(J::Arr(items));
        }
        loop {
            self.ws()?;
            items.push(self.value(depth + 1)?);
            self.ws()?;
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(J::Arr(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let h = self
            .b
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("short \\u escape"))?;
        let s = std::str::from_utf8(h).map_err(|_| self.err("bad \\u escape"))?;
        let v = u32::from_str_radix(s, 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.peek().is_some_and(|c| c != b'"' && c != b'\\' && c >= 0x20) {
                self.i += 1;
            }
            let run = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("invalid UTF-8"))?;
            out.push_str(run);
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.peek().ok_or_else(|| self.err("unterminated string"))?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if !self.b[self.i..].starts_with(b"\\u") {
                                    return Err(self.err("lone surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("lone surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("lone surrogate"));
                            } else {
                                hi
                            };
                            out.push(char::from_u32(cp).ok_or_else(|| self.err("bad \\u escape"))?);
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                Some(_) => return Err(self.err("control character in a string")),
            }
        }
    }

    fn number(&mut self) -> Result<J, ParseError> {
        let start = self.i;
        let digits = |p: &mut Self| {
            let s = p.i;
            while p.peek().is_some_and(|c| c.is_ascii_digit()) {
                p.i += 1;
            }
            p.i - s
        };
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.i += 1;
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    return Err(self.err("leading zero"));
                }
            }
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err(self.err("bad number")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if digits(self) == 0 {
                return Err(self.err("bad number"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return Err(self.err("bad number"));
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).expect("ASCII");
        Ok(J::Num(text.to_owned()))
    }
}

/// Whether a number's text is an integer (no fraction, no exponent).
pub fn is_integer_text(t: &str) -> bool {
    !t.contains(['.', 'e', 'E'])
}

/// The shortest text that reads back as exactly this f32 (Rust's shortest round-trip digits): integral values
/// without a fraction, exponent form outside 1e-5..1e16, negative zero as `-0.0` (so readers that take `-0` as
/// the integer 0 keep the sign).
pub fn f32_text(x: f32) -> String {
    float_text(
        x.is_sign_negative(),
        x == 0.0,
        f64::from(x).abs(),
        format!("{x}"),
        format!("{x:e}"),
    )
}

/// As [`f32_text`], for an f64.
pub fn f64_text(x: f64) -> String {
    float_text(
        x.is_sign_negative(),
        x == 0.0,
        x.abs(),
        format!("{x}"),
        format!("{x:e}"),
    )
}

fn float_text(negative: bool, zero: bool, abs: f64, plain: String, exp: String) -> String {
    assert!(abs.is_finite(), "JSON has no non-finite numbers");
    if zero {
        return if negative { "-0.0".into() } else { "0".into() };
    }
    if (1e-5..1e16).contains(&abs) { plain } else { exp }
}

/// Serialise with two-space indentation: objects one key per line, arrays of scalars on one line, other arrays one
/// item per line. Deterministic; ends with a newline.
pub fn write(v: &J) -> String {
    let mut out = String::new();
    write_value(&mut out, v, 0);
    out.push('\n');
    out
}

fn is_scalar(v: &J) -> bool {
    !matches!(v, J::Arr(_) | J::Obj(_))
}

fn write_value(out: &mut String, v: &J, indent: usize) {
    match v {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Num(t) => out.push_str(t),
        J::Str(s) => write_str(out, s),
        J::Arr(items) if items.is_empty() => out.push_str("[]"),
        J::Arr(items) if items.iter().all(is_scalar) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, x, indent);
            }
            out.push(']');
        }
        J::Arr(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                out.push_str(if i > 0 { ",\n" } else { "\n" });
                pad(out, indent + 1);
                write_value(out, x, indent + 1);
            }
            out.push('\n');
            pad(out, indent);
            out.push(']');
        }
        J::Obj(kv) if kv.is_empty() => out.push_str("{}"),
        J::Obj(kv) => {
            out.push('{');
            for (i, (k, x)) in kv.iter().enumerate() {
                out.push_str(if i > 0 { ",\n" } else { "\n" });
                pad(out, indent + 1);
                write_str(out, k);
                out.push_str(": ");
                write_value(out, x, indent + 1);
            }
            out.push('\n');
            pad(out, indent);
            out.push('}');
        }
    }
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// To serde_json, for data carried opaquely: integers that fit stay integers, other numbers become f64. Fails on a
/// number out of f64 range.
pub fn to_value(v: &J) -> Result<Value, String> {
    Ok(match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Num(t) => {
            if is_integer_text(t) {
                if let Ok(i) = t.parse::<i64>() {
                    return Ok(Value::from(i));
                }
                if let Ok(u) = t.parse::<u64>() {
                    return Ok(Value::from(u));
                }
            }
            let f: f64 = t.parse().map_err(|_| format!("number {t} unreadable"))?;
            Value::Number(serde_json::Number::from_f64(f).ok_or_else(|| format!("number {t} out of range"))?)
        }
        J::Str(s) => Value::String(s.clone()),
        J::Arr(items) => Value::Array(items.iter().map(to_value).collect::<Result<_, _>>()?),
        J::Obj(kv) => {
            let mut m = serde_json::Map::new();
            for (k, x) in kv {
                m.insert(k.clone(), to_value(x)?);
            }
            Value::Object(m)
        }
    })
}

/// From serde_json: integers as integers, floats in shortest form keeping a fraction (`2.0`), so the value reads
/// back as the same serde_json value.
pub fn from_value(v: &Value) -> J {
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                J::Num(i.to_string())
            } else if let Some(u) = n.as_u64() {
                J::Num(u.to_string())
            } else {
                let f = n.as_f64().expect("a serde_json number is i64, u64 or f64");
                let mut t = f64_text(f);
                if is_integer_text(&t) {
                    t.push_str(".0");
                }
                J::Num(t)
            }
        }
        Value::String(s) => J::Str(s.clone()),
        Value::Array(items) => J::Arr(items.iter().map(from_value).collect()),
        Value::Object(m) => J::Obj(m.iter().map(|(k, x)| (k.clone(), from_value(x))).collect()),
    }
}

/// How two documents compare: equal, equal only after reading some float as f32, or different (with a path).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compare {
    /// Numbers whose texts differ but whose f64 values are equal.
    pub f64_equal: u64,
    /// Numbers equal only as f32 (the same float32 bit pattern).
    pub f32_equal: u64,
    /// The first differences (JSON path: what).
    pub differences: Vec<String>,
}

/// Compare two documents as parsed values: objects by key (order ignored), arrays in order, numbers by text, else by
/// f64 value, else by f32 bit pattern (counted apart).
pub fn compare(a: &J, b: &J) -> Compare {
    let mut c = Compare::default();
    cmp(a, b, "$", &mut c);
    c
}

fn cmp(a: &J, b: &J, path: &str, c: &mut Compare) {
    fn differ(c: &mut Compare, path: &str, what: String) {
        if c.differences.len() < 50 {
            c.differences.push(format!("{path}: {what}"));
        }
    }
    match (a, b) {
        (J::Num(x), J::Num(y)) => {
            if x == y {
                return;
            }
            let (fx, fy) = (x.parse::<f64>(), y.parse::<f64>());
            if let (Ok(fx), Ok(fy)) = (fx, fy)
                && fx == fy
                && fx.is_sign_negative() == fy.is_sign_negative()
            {
                c.f64_equal += 1;
                return;
            }
            match (x.parse::<f32>(), y.parse::<f32>()) {
                (Ok(gx), Ok(gy)) if gx.to_bits() == gy.to_bits() => c.f32_equal += 1,
                _ => differ(c, path, format!("{x} vs {y}")),
            }
        }
        (J::Arr(x), J::Arr(y)) => {
            if x.len() != y.len() {
                differ(c, path, format!("{} vs {} items", x.len(), y.len()));
                return;
            }
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                cmp(p, q, &format!("{path}[{i}]"), c);
            }
        }
        (J::Obj(x), J::Obj(y)) => {
            for (k, p) in x {
                match b.get(k) {
                    Some(q) => cmp(p, q, &format!("{path}.{k}"), c),
                    None => differ(c, path, format!("key {k:?} only in the first")),
                }
            }
            for (k, _) in y {
                if a.get(k).is_none() {
                    differ(c, path, format!("key {k:?} only in the second"));
                }
            }
        }
        _ if a == b => {}
        _ => differ(c, path, format!("{} vs {}", a.kind(), b.kind())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<J, ParseError> {
        parse(s.as_bytes())
    }

    #[test]
    fn parses_and_keeps_number_text() {
        let v = p(
            r#"{"a": [1, -0, 0.30000001192092896, 1e-7, 2E+3], "b": {"c": null, "d": true}, "e": "xé😀\n"}"#,
        )
        .unwrap();
        assert_eq!(
            v.get("a"),
            Some(&J::Arr(
                ["1", "-0", "0.30000001192092896", "1e-7", "2E+3"]
                    .map(|t| J::Num(t.into()))
                    .to_vec()
            ))
        );
        assert_eq!(v.get("e"), Some(&J::Str("xé😀\n".into())));
        let keys: Vec<&str> = match &v {
            J::Obj(kv) => kv.iter().map(|(k, _)| k.as_str()).collect(),
            _ => unreachable!(),
        };
        assert_eq!(keys, ["a", "b", "e"]);
    }

    #[test]
    fn comments_and_bom_are_accepted() {
        let v = parse(b"\xEF\xBB\xBF// head\n{ /* x */ \"a\" : 1 // tail\n }").unwrap();
        assert_eq!(v.get("a"), Some(&J::Num("1".into())));
    }

    #[test]
    fn refuses_what_is_not_json() {
        for (bad, what) in [
            (r#"{"a": 1,}"#, "expected a key"),
            ("[1,]", "unexpected character"),
            (r#"{"a": 1, "a": 2}"#, "key \"a\" twice"),
            ("01", "leading zero"),
            ("1.", "bad number"),
            (".5", "unexpected character"),
            ("NaN", "unexpected character"),
            (r#""\ud800""#, "lone surrogate"),
            ("\"a\tb\"", "control character"),
            ("{} {}", "text after"),
            ("/* open", "unterminated comment"),
        ] {
            let e = p(bad).unwrap_err();
            assert!(e.message.contains(what), "{bad}: {e}");
        }
        let deep = "[".repeat(70) + &"]".repeat(70);
        assert!(p(&deep).unwrap_err().message.contains("deeper"));
        let e = p("{\n  \"a\": tru\n}").unwrap_err();
        assert_eq!((e.line, e.col), (2, 8));
    }

    #[test]
    fn float_text_is_shortest_and_round_trips() {
        assert_eq!(f32_text(0.5), "0.5");
        assert_eq!(f32_text(100.01), "100.01");
        assert_eq!(f32_text(250.0), "250");
        assert_eq!(f32_text(-0.0), "-0.0");
        assert_eq!(f32_text(0.0), "0");
        assert_eq!(f32_text(1e-7), "1e-7");
        assert_eq!(f32_text(3e20), "3e20");
        assert_eq!(f64_text(0.1), "0.1");
        assert_eq!(f64_text(0.30000001192092896), "0.30000001192092896");
        // a spread of finite bit patterns (subnormals to the largest), both signs
        let mut state: u32 = 0x1234_5678;
        for _ in 0..200_000 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = f32::from_bits(state);
            if !x.is_finite() {
                continue;
            }
            let t = f32_text(x);
            assert_eq!(t.parse::<f32>().unwrap().to_bits(), x.to_bits(), "{x:e} -> {t}");
            assert!(
                matches!(parse(t.as_bytes()), Ok(J::Num(_))),
                "{t} is a JSON number"
            );
        }
        for x in [
            f64::MIN_POSITIVE,
            5e-324,
            1.7976931348623157e308,
            123456.789,
            -2.5,
        ] {
            assert_eq!(f64_text(x).parse::<f64>().unwrap().to_bits(), x.to_bits());
        }
    }

    #[test]
    fn write_parse_round_trip_and_layout() {
        let src = r#"{"name": "a\"b", "region": [95, -1034.5], "list": [{"k": []}, [1, 2]], "o": {}}"#;
        let v = p(src).unwrap();
        let text = write(&v);
        assert_eq!(
            text,
            "{\n  \"name\": \"a\\\"b\",\n  \"region\": [95, -1034.5],\n  \"list\": [\n    {\n      \"k\": []\n    },\n    [1, 2]\n  ],\n  \"o\": {}\n}\n"
        );
        assert_eq!(p(&text).unwrap(), v);
    }

    #[test]
    fn serde_round_trip_keeps_int_and_float() {
        let v =
            p(r#"{"i": 3, "f": 3.0, "big": 18446744073709551615, "e": 1e3, "s": [null, false]}"#).unwrap();
        let s = to_value(&v).unwrap();
        assert_eq!(
            s,
            serde_json::json!({"i": 3, "f": 3.0, "big": 18446744073709551615u64, "e": 1000.0, "s": [null, false]})
        );
        let back = from_value(&s);
        assert_eq!(to_value(&back).unwrap(), s);
        assert_eq!(back.get("f"), Some(&J::Num("3.0".into())));
        assert!(to_value(&p("1e999").unwrap()).is_err());
    }

    #[test]
    fn compare_counts_float_forms() {
        let a = p(r#"{"x": 0.30000001192092896, "y": 90, "z": [1, 2], "w": "s"}"#).unwrap();
        let b = p(r#"{"w": "s", "y": 90.0, "z": [1, 2], "x": 0.3}"#).unwrap();
        let c = compare(&a, &b);
        assert_eq!((c.f64_equal, c.f32_equal, c.differences.len()), (1, 1, 0));
        let d = compare(&a, &p(r#"{"x": 0.31, "y": 90, "z": [1], "q": 1}"#).unwrap());
        assert_eq!(d.differences.len(), 4, "{:?}", d.differences);
    }
}
