//! Unity editor versions, as files write them (`6000.3.12f1`) and as type tree packages order them.

use std::fmt;

use crate::{Result, invalid};

/// A Unity version packed into one ordered number: major (16 bits), minor (16), build (16), release type (8: a 0,
/// b 1, c 2, f 3, p 4, x 5), number (8). A version without a release type (`5.0.0`) is final, number 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnityVersion(pub u64);

const TYPES: [char; 6] = ['a', 'b', 'c', 'f', 'p', 'x'];

impl UnityVersion {
    pub fn new(major: u16, minor: u16, build: u16, kind: u8, number: u8) -> Self {
        UnityVersion(
            u64::from(major) << 48
                | u64::from(minor) << 32
                | u64::from(build) << 16
                | u64::from(kind) << 8
                | u64::from(number),
        )
    }

    /// `major.minor.build[type number]`, e.g. `6000.3.12f1`; anything after the number is ignored.
    pub fn parse(s: &str) -> Result<Self> {
        let bad = || invalid(format!("Unity version {s:?}"));
        let mut parts = s.splitn(3, '.');
        let major: u16 = parts.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
        let minor: u16 = parts.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
        let rest = parts.next().ok_or_else(bad)?;
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let build: u16 = rest[..digits].parse().map_err(|_| bad())?;
        let tail = &rest[digits..];
        let Some(t) = tail.chars().next() else {
            return Ok(Self::new(major, minor, build, 3, 0));
        };
        let kind = TYPES.iter().position(|&c| c == t).ok_or_else(bad)? as u8;
        let num = &tail[1..];
        let n = num.find(|c: char| !c.is_ascii_digit()).unwrap_or(num.len());
        let number: u8 = if n == 0 {
            0
        } else {
            num[..n].parse().map_err(|_| bad())?
        };
        Ok(Self::new(major, minor, build, kind, number))
    }

    pub fn major(self) -> u16 {
        (self.0 >> 48) as u16
    }
    pub fn minor(self) -> u16 {
        (self.0 >> 32) as u16
    }
    pub fn build(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

impl fmt::Display for UnityVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = TYPES.get(((self.0 >> 8) & 0xFF) as usize).copied().unwrap_or('?');
        write!(
            f,
            "{}.{}.{}{kind}{}",
            self.major(),
            self.minor(),
            self.build(),
            self.0 & 0xFF
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        let v = UnityVersion::parse("6000.3.12f1").unwrap();
        assert_eq!(v, UnityVersion::new(6000, 3, 12, 3, 1));
        assert_eq!(v.to_string(), "6000.3.12f1");
        assert_eq!(
            UnityVersion::parse("5.0.0").unwrap(),
            UnityVersion::new(5, 0, 0, 3, 0)
        );
        assert!(UnityVersion::parse("2022.3.1b2").unwrap() < UnityVersion::parse("2022.3.1f1").unwrap());
        assert!(UnityVersion::parse("2022.3.1f1-custom").is_ok());
        for bad in ["", "6000", "6000.3", "6000.3.x", "6000.3.12q1"] {
            assert!(UnityVersion::parse(bad).is_err(), "{bad}");
        }
    }
}
