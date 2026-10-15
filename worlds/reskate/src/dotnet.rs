//! What the reference writer's runtime does with numbers and JSON, reproduced: .NET's shortest round-trip number
//! text, `Math.Round(double, digits)`, and `System.Text.Json`'s `Utf8JsonWriter` layout and escaping (default
//! encoder; indented output uses CRLF and two spaces). Checked against a fixture recorded from the runtime.

use std::fmt::Write as _;

/// .NET's text for a float: the shortest digits that read back as the same value, positional when the decimal
/// scale is in `-3..=precision`, else `d.dddE+XX` (at least two exponent digits). `precision` is 9 for `float`,
/// 17 for `double`.
fn net_float(negative: bool, zero: bool, exp_text: String, precision: i32) -> String {
    if zero {
        return if negative { "-0".into() } else { "0".into() };
    }
    // exp_text is Rust's shortest `{:e}`: "-1.2345e-7"
    let t = exp_text.trim_start_matches('-');
    let (mant, exp) = t.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("exponent");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let scale = exp + 1;
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if scale > precision || scale < -3 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let _ = write!(out, "E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    } else if scale <= 0 {
        out.push_str("0.");
        for _ in 0..-scale {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let s = scale as usize;
        if digits.len() <= s {
            out.push_str(&digits);
            for _ in digits.len()..s {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..s]);
            out.push('.');
            out.push_str(&digits[s..]);
        }
    }
    out
}

/// Shortest digits as .NET picks them: where the value lies exactly halfway between two shortest candidates,
/// .NET (Dragon4) takes the one with the even last digit; Rust's shortest formatting takes the upper one. So when
/// Rust's last digit is odd and the exact expansion is that halfway point, step down to the even one.
/// `exact` is the value's exact decimal expansion in `{:e}` form (enough digits to be exact).
fn ties_to_even(shortest: String, exact: impl FnOnce() -> String) -> String {
    let (mant, exp) = shortest.split_once('e').expect("{:e} has an exponent");
    let last = mant.chars().last().expect("digits");
    let Some(d) = last.to_digit(10).filter(|d| d % 2 == 1) else {
        return shortest;
    };
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let e = exact();
    let (em, eexp) = e.split_once('e').expect("{:e} has an exponent");
    let exact_digits: String = em.chars().filter(|c| c.is_ascii_digit()).collect();
    let exact_digits = exact_digits.trim_end_matches('0');
    // halfway: exactly one more digit, a 5, after the digits one below ours (same exponent)
    let n = digits.len();
    let below = format!("{}{}", &digits[..n - 1], d - 1);
    if eexp == exp && exact_digits.len() == n + 1 && exact_digits.ends_with('5') && exact_digits[..n] == below
    {
        let mut m = mant[..mant.len() - 1].to_owned();
        m.push(char::from_digit(d - 1, 10).expect("digit"));
        return format!("{m}e{exp}");
    }
    shortest
}

/// `float.ToString()` / `Utf8JsonWriter.WriteNumberValue(float)`.
pub fn f32_text(x: f32) -> String {
    assert!(x.is_finite(), "JSON has no non-finite numbers");
    let s = ties_to_even(format!("{x:e}"), || format!("{x:.150e}"));
    net_float(x.is_sign_negative(), x == 0.0, s, 9)
}

/// `double.ToString()` / `Utf8JsonWriter.WriteNumberValue(double)`.
pub fn f64_text(x: f64) -> String {
    assert!(x.is_finite(), "JSON has no non-finite numbers");
    let s = ties_to_even(format!("{x:e}"), || format!("{x:.800e}"));
    net_float(x.is_sign_negative(), x == 0.0, s, 17)
}

/// `Math.Round(value, digits)` (to even), for `digits` 0-15: scaled by the power of ten, rounded, divided back,
/// unless the value is too large for that to matter.
pub fn round(value: f64, digits: u32) -> f64 {
    const POW10: [f64; 16] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15,
    ];
    if value.abs() < 1e16 {
        let p = POW10[digits as usize];
        (value * p).round_ties_even() / p
    } else {
        value
    }
}

/// The reference writer's `JsonExt.R`: rounded to 4 decimals, 0 when not finite.
pub fn r4(v: f64) -> f64 {
    if v.is_finite() { round(v, 4) } else { 0.0 }
}

/// The default encoder's escaping: `"`, `\`, control characters, `<>&'+` and backquote, and everything outside
/// printable ASCII as `\uXXXX` (UTF-16 units, upper-case hex); `\b \t \n \f \r` by their short forms.
pub fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            ' '..='~' if !matches!(c, '"' | '<' | '>' | '&' | '\'' | '+' | '`') => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{u:04X}");
                }
            }
        }
    }
    out.push('"');
}

/// `Utf8JsonWriter`: compact, or indented (two spaces, CRLF, `"key": value`, empty containers as `{}` / `[]`).
#[derive(Debug)]
pub struct JsonWriter {
    out: String,
    indented: bool,
    /// Per open container: whether it has an item yet.
    stack: Vec<bool>,
}

impl JsonWriter {
    pub fn new(indented: bool) -> Self {
        JsonWriter {
            out: String::new(),
            indented,
            stack: Vec::new(),
        }
    }

    pub fn finish(self) -> String {
        assert!(self.stack.is_empty(), "unclosed JSON container");
        self.out
    }

    fn newline(&mut self) {
        self.out.push_str("\r\n");
        for _ in 0..self.stack.len() {
            self.out.push_str("  ");
        }
    }

    /// Before an item (property or array value) of the current container.
    fn item(&mut self) {
        if let Some(has) = self.stack.last_mut() {
            let had = *has;
            *has = true;
            if had {
                self.out.push(',');
            }
            if self.indented {
                self.newline();
            }
        }
    }

    fn key(&mut self, name: &str) {
        self.item();
        escape(name, &mut self.out);
        self.out.push(':');
        if self.indented {
            self.out.push(' ');
        }
    }

    fn open(&mut self, c: char) {
        self.out.push(c);
        self.stack.push(false);
    }

    fn close(&mut self, c: char) {
        let had = self.stack.pop().expect("close without open");
        if had && self.indented {
            self.newline();
        }
        self.out.push(c);
    }

    pub fn start_object(&mut self) {
        self.item();
        self.open('{');
    }
    pub fn start_object_named(&mut self, name: &str) {
        self.key(name);
        self.open('{');
    }
    pub fn end_object(&mut self) {
        self.close('}');
    }
    pub fn start_array(&mut self) {
        self.item();
        self.open('[');
    }
    pub fn start_array_named(&mut self, name: &str) {
        self.key(name);
        self.open('[');
    }
    pub fn end_array(&mut self) {
        self.close(']');
    }

    pub fn string(&mut self, name: &str, v: &str) {
        self.key(name);
        escape(v, &mut self.out);
    }
    pub fn string_value(&mut self, v: &str) {
        self.item();
        escape(v, &mut self.out);
    }
    pub fn bool(&mut self, name: &str, v: bool) {
        self.key(name);
        self.out.push_str(if v { "true" } else { "false" });
    }
    pub fn int(&mut self, name: &str, v: i64) {
        self.key(name);
        let _ = write!(self.out, "{v}");
    }
    pub fn int_value(&mut self, v: i64) {
        self.item();
        let _ = write!(self.out, "{v}");
    }
    pub fn uint(&mut self, name: &str, v: u64) {
        self.key(name);
        let _ = write!(self.out, "{v}");
    }
    pub fn f64(&mut self, name: &str, v: f64) {
        self.key(name);
        self.out.push_str(&f64_text(v));
    }
    pub fn f64_value(&mut self, v: f64) {
        self.item();
        self.out.push_str(&f64_text(v));
    }
    pub fn f32(&mut self, name: &str, v: f32) {
        self.key(name);
        self.out.push_str(&f32_text(v));
    }
    pub fn f32_value(&mut self, v: f32) {
        self.item();
        self.out.push_str(&f32_text(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let mut w = JsonWriter::new(true);
        w.start_object();
        w.int("format", 1);
        w.start_array_named("a");
        w.end_array();
        w.start_array_named("n");
        w.f32_value(1.5);
        w.end_array();
        w.end_object();
        assert_eq!(
            w.finish(),
            "{\r\n  \"format\": 1,\r\n  \"a\": [],\r\n  \"n\": [\r\n    1.5\r\n  ]\r\n}"
        );
        let mut c = JsonWriter::new(false);
        c.start_object();
        c.string("k", "v");
        c.start_array_named("x");
        c.int_value(1);
        c.int_value(2);
        c.end_array();
        c.end_object();
        assert_eq!(c.finish(), "{\"k\":\"v\",\"x\":[1,2]}");
    }
}
