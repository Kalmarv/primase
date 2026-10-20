//! Validated values of the FromSoft passes' settings. Each is made only through its checked constructor (which the
//! passes' option parsing also uses), so a pass built from a struct literal holds no value its options would refuse.
use std::collections::BTreeSet;
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

/// A HitMtrl class: a HitMtrlParam ID's base value, the ID modulo 100 (0 to 99).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HitClass(u8);
impl HitClass {
    pub fn new(class: u8) -> Result<Self, String> {
        if class < 100 {
            Ok(HitClass(class))
        } else {
            Err(format!("{class} is not a HitMtrl class (0 to 99)"))
        }
    }
    /// The class of a stored HitMtrlParam ID; `None` for -1 (no material bound) or another negative ID.
    pub fn of(material: i32) -> Option<Self> {
        u8::try_from(material.checked_rem(100)?).ok().map(HitClass)
    }
    /// The classes `classes`, each checked.
    pub fn set(classes: impl IntoIterator<Item = u8>) -> Result<BTreeSet<Self>, String> {
        classes.into_iter().map(Self::new).collect()
    }
    /// Classes joined by `+` (empty: none).
    pub fn parse_set(key: &str, text: &str) -> Result<BTreeSet<Self>, String> {
        text.split('+')
            .filter(|c| !c.is_empty())
            .map(|c| Self::new(number(key, c)?).map_err(|e| format!("{key}: {e}")))
            .collect()
    }
    pub fn get(self) -> u8 {
        self.0
    }
}
impl fmt::Display for HitClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A non-empty set of MSB collision part hit filters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitFilters(BTreeSet<u8>);
impl HitFilters {
    pub fn new(filters: impl IntoIterator<Item = u8>) -> Result<Self, String> {
        let set: BTreeSet<u8> = filters.into_iter().collect();
        if set.is_empty() {
            return Err("no hit filter: which placed collision is ground is the program's choice".into());
        }
        Ok(HitFilters(set))
    }
    /// Filters joined by `+`.
    pub fn parse(key: &str, text: &str) -> Result<Self, String> {
        Self::new(
            text.split('+')
                .filter(|f| !f.is_empty())
                .map(|f| number::<u8>(key, f))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|e| format!("{key}: {e}"))
    }
    pub fn contains(&self, filter: u8) -> bool {
        self.0.contains(&filter)
    }
}
impl fmt::Display for HitFilters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let joined: Vec<String> = self.0.iter().map(u8::to_string).collect();
        f.write_str(&joined.join("+"))
    }
}

/// Items joined by `+`, as options print sets.
pub(crate) fn joined<T: fmt::Display>(items: impl IntoIterator<Item = T>) -> String {
    items
        .into_iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join("+")
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
        assert!(HitClass::new(100).is_err());
        assert!(HitFilters::new([]).is_err());
        assert_eq!(HitClass::of(-1), None);
        assert_eq!(HitClass::of(1_036), Some(HitClass(36)));
        let m = Metres::new(50.25).unwrap();
        assert_eq!(Metres::parse("reach", &m.to_string()).unwrap(), m);
        let f = HitFilters::parse("hit-filters", "27+8").unwrap();
        assert_eq!(f.to_string(), "8+27");
        assert!(HitClass::parse_set("water", "22+x").is_err());
    }
}
