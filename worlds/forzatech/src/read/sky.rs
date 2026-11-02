//! The emissive scale of a sky: the `EmissiveScales.TrackEmissiveEV` curve of a `Sky` of a track's time-of-day file
//! (`media\TimeOfDay\TimeOfDay_<track>.ppr`; Tokyo's is `TimeOfDay_Brio.ppr`), found by the sky's name. The caller
//! names the sky (`Forte_Summer_01`, the summer sky of the open world): there is no default, and a name two skies share
//! is refused.
//!
//! The curve is the sky's scale on the emissive surfaces of the track over the day (`Forte_Summer_01`: 1 at night, 50
//! from its 0.149 to its 0.899). What the game does with it is not established: its unit is not proven (the files do not
//! say whether it is cd/m2), and the clock point of noon on the curve's 0 to 1 axis is not either. So a program that
//! needs one number from it takes the curve's peak ([`SkyEmission::peak`]): the day value, which does not depend on
//! the clock. That choice is an approximation, and [`SkyEmission::notice`] names and counts it.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the time-of-day file the caller names"
)]
use crate::{Error, Result};
use forzatech_formats::ppr::{Ppr, Value};
use primase_core::convert::{Notice, NoticeKind};
use std::path::Path;

/// The kind of group that holds a sky.
const SKY: &str = "Sky";
/// The property that holds the emissive scale of a sky's track.
const TRACK_EMISSIVE_EV: &str = "EmissiveScales.TrackEmissiveEV";

fn sky_error(detail: String) -> Error {
    Error::Sky(detail)
}

/// A sky's `EmissiveScales.TrackEmissiveEV` curve, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyEmission {
    sky: String,
    keys: Vec<(f32, f32)>,
}

impl SkyEmission {
    /// The curve of the sky named `sky` in the bytes of a time-of-day file: refused when the file does not read, no sky
    /// or more than one has the name, the sky has no such property or it is not a curve of one scalar component, or
    /// the curve has no key or a key that is not finite.
    pub fn parse(bytes: &[u8], sky: &str) -> Result<SkyEmission> {
        let ppr = Ppr::parse(bytes)?;
        let group = ppr
            .group(SKY, sky)?
            .ok_or_else(|| sky_error(format!("the file has no sky named {sky:?}")))?;
        let leaf = ppr
            .leaf(group, TRACK_EMISSIVE_EV)?
            .ok_or_else(|| sky_error(format!("the sky {sky:?} has no {TRACK_EMISSIVE_EV}")))?;
        let keys: Vec<(f32, f32)> = match &leaf.value {
            Value::Curve(curve) => curve.scalar_keys().map(Iterator::collect),
            _ => None,
        }
        .ok_or_else(|| {
            sky_error(format!(
                "{TRACK_EMISSIVE_EV} of {sky:?} is not a curve of one scalar component"
            ))
        })?;
        if keys.is_empty() || keys.iter().any(|&(t, v)| !(t.is_finite() && v.is_finite())) {
            return Err(sky_error(format!(
                "{TRACK_EMISSIVE_EV} of {sky:?} has no key, or a key that is not finite"
            )));
        }
        Ok(SkyEmission {
            sky: sky.to_owned(),
            keys,
        })
    }

    /// The curve of the sky named `sky` in the time-of-day file at `path`.
    pub fn open(path: impl AsRef<Path>, sky: &str) -> Result<SkyEmission> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        SkyEmission::parse(&bytes, sky)
    }

    /// The sky's name.
    pub fn sky(&self) -> &str {
        &self.sky
    }

    /// The curve's keys as (time, value), time on the file's 0 to 1 axis.
    pub fn keys(&self) -> &[(f32, f32)] {
        &self.keys
    }

    /// The curve's largest key value: the sky's day value of the scale, whichever clock point is noon. A value
    /// `with_emission` refuses (zero or less) is refused here, with the sky's name.
    pub fn peak(&self) -> Result<f64> {
        let peak = self
            .keys
            .iter()
            .map(|&(_, v)| v)
            .fold(f32::NEG_INFINITY, f32::max);
        if peak > 0.0 {
            Ok(f64::from(peak))
        } else {
            Err(sky_error(format!(
                "{TRACK_EMISSIVE_EV} of {:?} peaks at {peak}: not a scale above 0",
                self.sky
            )))
        }
    }

    /// The approximation [`SkyEmission::peak`] makes, for the conversion's report: one value stands for the curve of
    /// its keys, named with the sky, the keys and the range.
    pub fn notice(&self, stage: &str) -> Notice {
        let lo = self.keys.iter().map(|&(_, v)| v).fold(f32::INFINITY, f32::min);
        let hi = self
            .keys
            .iter()
            .map(|&(_, v)| v)
            .fold(f32::NEG_INFINITY, f32::max);
        Notice::new(
            stage,
            NoticeKind::Approximation,
            "TrackEmissiveEV's peak taken as cd/m2 per unit emissive strength (its unit is not proven)",
            1,
        )
        .example(format!(
            "{}: peak of {} keys, {lo} to {hi}",
            self.sky,
            self.keys.len()
        ))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "tests on files built by hand"
)]
mod tests {
    use super::*;

    const NAMES: [&str; 7] = [
        "TimeOfDay_Test",
        "Sky",
        "Summer",
        "Winter",
        "EmissiveScales.TrackEmissiveEV",
        "guid",
        "Other",
    ];

    fn leaf(kind: u8, name: u16, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![kind];
        b.extend_from_slice(&name.to_le_bytes());
        b.extend_from_slice(&(name + 1).to_le_bytes());
        b.extend_from_slice(payload);
        b
    }

    /// A scalar curve payload: one component, flag 0, keys of (time, value).
    fn curve(keys: &[(f32, f32)]) -> Vec<u8> {
        let mut b = 1u32.to_le_bytes().to_vec();
        b.push(0);
        b.extend_from_slice(&u32::try_from(keys.len()).unwrap().to_le_bytes());
        for (t, v) in keys {
            b.extend_from_slice(&t.to_le_bytes());
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    fn group(label: u16, leaves: &[Vec<u8>]) -> Vec<u8> {
        let mut b = 1u16.to_le_bytes().to_vec();
        for f in [0.0f32, 1440.0] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.extend_from_slice(&label.to_le_bytes());
        b.extend_from_slice(&u32::try_from(leaves.len()).unwrap().to_le_bytes());
        for l in leaves {
            b.extend_from_slice(l);
        }
        b
    }

    fn file(groups: &[Vec<u8>]) -> Vec<u8> {
        let mut b = 3u32.to_le_bytes().to_vec();
        b.extend_from_slice(&u32::try_from(NAMES.len()).unwrap().to_le_bytes());
        for n in NAMES {
            b.push(u8::try_from(n.len()).unwrap());
            b.extend_from_slice(n.as_bytes());
        }
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&u32::try_from(groups.len()).unwrap().to_le_bytes());
        for g in groups {
            b.extend_from_slice(g);
        }
        b
    }

    fn summer() -> Vec<u8> {
        let day = curve(&[(0.0, 1.0), (0.15, 50.0), (0.9, 50.0), (1.0, 1.0)]);
        group(2, &[leaf(5, 4, &day)])
    }

    #[test]
    fn the_peak_of_a_skys_curve_is_its_day_value() {
        let e = SkyEmission::parse(&file(&[summer()]), "Summer").unwrap();
        assert_eq!(e.sky(), "Summer");
        assert_eq!(e.keys(), [(0.0, 1.0), (0.15, 50.0), (0.9, 50.0), (1.0, 1.0)]);
        assert_eq!(e.peak().unwrap(), 50.0);
        let n = e.notice("test");
        assert_eq!((n.kind, n.count), (NoticeKind::Approximation, 1));
        assert!(
            n.reason
                .contains("peak taken as cd/m2 per unit emissive strength"),
            "{}",
            n.reason
        );
        assert_eq!(n.examples, ["Summer: peak of 4 keys, 1 to 50"]);
    }

    #[test]
    fn a_sky_the_caller_must_name_exactly() {
        let winter = group(3, &[leaf(5, 4, &curve(&[(0.0, 2.0), (1.0, 2.0)]))]);
        let bytes = file(&[summer(), winter.clone()]);
        assert_eq!(SkyEmission::parse(&bytes, "Winter").unwrap().peak().unwrap(), 2.0);
        // no sky of the name, and the name is not a prefix match
        for name in ["Spring", "Summe", ""] {
            let e = SkyEmission::parse(&bytes, name).unwrap_err();
            assert!(e.to_string().contains("no sky named"), "{e}");
        }
        // two skies of one name
        let e = SkyEmission::parse(&file(&[summer(), summer()]), "Summer").unwrap_err();
        assert!(e.to_string().contains("more than one group"), "{e}");
    }

    #[test]
    fn what_is_not_a_scale_is_refused() {
        // no such property in the sky
        let bare = group(2, &[leaf(1, 4, &7i32.to_le_bytes())]);
        let e = SkyEmission::parse(&file(&[bare]), "Summer").unwrap_err();
        assert!(
            e.to_string().contains("not a curve of one scalar component"),
            "{e}"
        );
        let none = group(2, &[leaf(1, 5, &7i32.to_le_bytes())]);
        let e = SkyEmission::parse(&file(&[none]), "Summer").unwrap_err();
        assert!(
            e.to_string().contains("has no EmissiveScales.TrackEmissiveEV"),
            "{e}"
        );
        // a curve with no key
        let empty = group(2, &[leaf(5, 4, &curve(&[]))]);
        assert!(SkyEmission::parse(&file(&[empty]), "Summer").is_err());
        // a curve that never rises above 0 gives no scale
        let zero = group(2, &[leaf(5, 4, &curve(&[(0.0, 0.0), (1.0, -1.0)]))]);
        let e = SkyEmission::parse(&file(&[zero]), "Summer")
            .unwrap()
            .peak()
            .unwrap_err();
        assert!(e.to_string().contains("not a scale above 0"), "{e}");
        // bytes that are not a time-of-day file
        assert!(SkyEmission::parse(b"nope", "Summer").is_err());
    }
}
