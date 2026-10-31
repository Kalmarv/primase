//! Validated values of the FromSoft passes' settings. Each is made only through its checked constructor (which the
//! passes' option parsing also uses), so a pass built from a struct literal holds no value its options would refuse.
use std::fmt;

fn number<T: std::str::FromStr>(key: &str, text: &str) -> Result<T, String> {
    text.parse::<T>()
        .map_err(|_| format!("{key}={text} is not a number"))
}

/// A distance in metres: finite, at least 0.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Metres(f32);
impl Metres {
    pub fn new(metres: f32) -> Result<Self, String> {
        if metres.is_finite() && metres >= 0. {
            Ok(Metres(metres))
        } else {
            Err(format!("{metres} is not a distance of 0 or more metres"))
        }
    }
    pub fn parse(key: &str, text: &str) -> Result<Self, String> {
        Self::new(number(key, text)?).map_err(|e| format!("{key}: {e}"))
    }
    /// A distance, or `none` for no distance (an option stated as having none).
    pub fn parse_or_none(key: &str, text: &str) -> Result<Option<Self>, String> {
        if text == "none" {
            Ok(None)
        } else {
            Self::parse(key, text).map(Some)
        }
    }
    /// The text [`Metres::parse_or_none`] reads back.
    pub fn text_or_none(m: Option<Self>) -> String {
        m.map_or_else(|| "none".to_owned(), |m| m.to_string())
    }
    pub fn get(self) -> f32 {
        self.0
    }
}
impl fmt::Display for Metres {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A time in seconds: finite.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Seconds(f32);
impl Seconds {
    pub fn new(seconds: f32) -> Result<Self, String> {
        if seconds.is_finite() {
            Ok(Seconds(seconds))
        } else {
            Err(format!("{seconds} is not a finite number of seconds"))
        }
    }
    pub fn parse(key: &str, text: &str) -> Result<Self, String> {
        Self::new(number(key, text)?).map_err(|e| format!("{key}: {e}"))
    }
    pub fn get(self) -> f32 {
        self.0
    }
}
impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The largest edge of an image in pixels: 1 to 4096.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PixelCap(u32);
impl PixelCap {
    pub fn new(pixels: u32) -> Result<Self, String> {
        if (1..=4096).contains(&pixels) {
            Ok(PixelCap(pixels))
        } else {
            Err(format!("{pixels} is not a size of 1 to 4096 pixels"))
        }
    }
    pub fn parse(key: &str, text: &str) -> Result<Self, String> {
        Self::new(number(key, text)?).map_err(|e| format!("{key}: {e}"))
    }
    pub fn get(self) -> u32 {
        self.0
    }
}
impl fmt::Display for PixelCap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A fraction: finite, 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Fraction(f32);
impl Fraction {
    pub fn new(fraction: f32) -> Result<Self, String> {
        if (0. ..=1.).contains(&fraction) {
            Ok(Fraction(fraction))
        } else {
            Err(format!("{fraction} is not a fraction from 0 to 1"))
        }
    }
    pub fn parse(key: &str, text: &str) -> Result<Self, String> {
        Self::new(number(key, text)?).map_err(|e| format!("{key}: {e}"))
    }
    pub fn get(self) -> f32 {
        self.0
    }
}
impl fmt::Display for Fraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A scale factor: finite, at least 0.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Factor(f64);
impl Factor {
    pub fn new(factor: f64) -> Result<Self, String> {
        if factor.is_finite() && factor >= 0. {
            Ok(Factor(factor))
        } else {
            Err(format!("{factor} is not a finite number at least 0"))
        }
    }
    pub fn parse(key: &str, text: &str) -> Result<Self, String> {
        Self::new(number(key, text)?).map_err(|e| format!("{key}: {e}"))
    }
    pub fn get(self) -> f64 {
        self.0
    }
}
impl fmt::Display for Factor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_outside_their_range_are_refused_and_printed_values_parse_back() {
        assert!(Metres::new(-1.).is_err() && Metres::new(f32::NAN).is_err());
        assert!(Seconds::new(f32::INFINITY).is_err());
        assert!(PixelCap::new(0).is_err() && PixelCap::new(4097).is_err());
        assert!(Fraction::new(1.5).is_err());
        assert!(Factor::new(-0.5).is_err());
        let m = Metres::new(50.25).unwrap();
        assert_eq!(Metres::parse("reach", &m.to_string()).unwrap(), m);
    }
}
