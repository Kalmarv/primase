//! Values the GTA passes take, each checked once where it is made: [`GroundRect`] (a rectangle of GTA's ground) and
//! [`Hour`] (an hour of GTA's clock). A pass holding one needs no check of its own, and a pass's options parse into
//! the same types.
use primase_core::pass::{Given, OptionSpec, Options};

/// The hours GTA's night-only surfaces and lights are on, bit h for hour h: 00-05 and 20-23, the hours the city's
/// street lamps' light records name (`time_flags` 0xF0003F on the lamps of every region read so far).
pub const NIGHT_HOURS: u32 = 0x00F0_003F;

/// Hour `hour` (0-23) is a night hour ([`NIGHT_HOURS`]).
pub fn is_night(hour: u8) -> bool {
    hour < 24 && NIGHT_HOURS & (1 << hour) != 0
}

/// A rectangle of GTA's ground: metres east (x) and north (y), half-open (`x0 <= x < x1`, `y0 <= y < y1`), every
/// value finite with `x0 < x1` and `y0 < y1`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct GroundRect([f32; 4]);

impl GroundRect {
    /// `[x0, y0, x1, y1]`: an error unless every value is finite, `x0 < x1` and `y0 < y1`.
    pub fn new(rect: [f32; 4]) -> Result<GroundRect, String> {
        let [x0, y0, x1, y1] = rect;
        if !rect.iter().all(|v| v.is_finite()) || x0 >= x1 || y0 >= y1 {
            return Err(format!(
                "rectangle {rect:?}: x0, y0, x1, y1 finite, with x0 < x1 and y0 < y1"
            ));
        }
        Ok(GroundRect(rect))
    }

    /// `[x0, y0, x1, y1]`.
    pub fn get(self) -> [f32; 4] {
        self.0
    }

    /// Whether ground point (`x`, `y`) lies in it (half-open).
    pub fn contains(self, x: f32, y: f32) -> bool {
        let [x0, y0, x1, y1] = self.0;
        x >= x0 && x < x1 && y >= y0 && y < y1
    }

    /// Options `x0`, `y0`, `x1`, `y1` of a declaration that has them ([`X0`]..[`Y1`], or [`X0_REQUIRED`]..
    /// [`Y1_REQUIRED`]): all four (checked as [`GroundRect::new`]) or none.
    pub fn from_given(g: &Given) -> Result<Option<GroundRect>, String> {
        let num = |k: &str| -> Result<Option<f32>, String> {
            g.optional(k)
                .map(|v| v.parse().map_err(|_| format!("{k}: not a number: {v}")))
                .transpose()
        };
        match [num("x0")?, num("y0")?, num("x1")?, num("y1")?] {
            [Some(a), Some(b), Some(c), Some(d)] => GroundRect::new([a, b, c, d]).map(Some),
            [None, None, None, None] => Ok(None),
            _ => Err("x0, y0, x1 and y1 go together".into()),
        }
    }

    /// The rectangle as options `x0`, `y0`, `x1`, `y1`.
    pub fn insert_options(self, o: &mut Options) {
        let [x0, y0, x1, y1] = self.0;
        for (k, v) in [("x0", x0), ("y0", y0), ("x1", x1), ("y1", y1)] {
            o.insert(k.to_owned(), v.to_string());
        }
    }
}

/// The rectangle's west edge, `x0`: with `y0`, `x1` and `y1`, all four or none (none: no rectangle, the whole map).
pub const X0: OptionSpec = OptionSpec::optional(
    "x0",
    "0",
    "metres east: the rectangle's west edge, with y0, x1 and y1",
    "the whole map, unless all four are given",
);
/// The rectangle's south edge, `y0`.
pub const Y0: OptionSpec = OptionSpec::optional(
    "y0",
    "0",
    "metres north: the rectangle's south edge, with x0, x1 and y1",
    "the whole map, unless all four are given",
);
/// The rectangle's east edge, `x1`.
pub const X1: OptionSpec = OptionSpec::optional(
    "x1",
    "1",
    "metres east: the rectangle's east edge, with x0, y0 and y1",
    "the whole map, unless all four are given",
);
/// The rectangle's north edge, `y1`.
pub const Y1: OptionSpec = OptionSpec::optional(
    "y1",
    "1",
    "metres north: the rectangle's north edge, with x0, y0 and x1",
    "the whole map, unless all four are given",
);
/// [`X0`], required: a pass whose region is the caller's choice.
pub const X0_REQUIRED: OptionSpec = OptionSpec::required("x0", "0", "metres east: the region's west edge");
/// [`Y0`], required.
pub const Y0_REQUIRED: OptionSpec = OptionSpec::required("y0", "0", "metres north: the region's south edge");
/// [`X1`], required.
pub const X1_REQUIRED: OptionSpec = OptionSpec::required("x1", "1", "metres east: the region's east edge");
/// [`Y1`], required.
pub const Y1_REQUIRED: OptionSpec = OptionSpec::required("y1", "1", "metres north: the region's north edge");

/// An hour of GTA's clock, 0-23.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct Hour(u8);

impl Hour {
    /// `hour`: an error unless it is 0-23.
    pub fn new(hour: u8) -> Result<Hour, String> {
        if hour < 24 {
            Ok(Hour(hour))
        } else {
            Err(format!("hour {hour}: 0-23"))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }

    /// Whether it is one of GTA's night hours ([`crate::passes::rect::NIGHT_HOURS`]).
    pub fn is_night(self) -> bool {
        is_night(self.0)
    }

    /// Option text `v` (`0`-`23`).
    pub fn parse(v: &str) -> Result<Hour, String> {
        v.parse::<u8>()
            .map_err(|_| format!("hour: not 0-23: {v}"))
            .and_then(Hour::new)
    }
}

impl std::fmt::Display for Hour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The street lamps' hours are night: 00-05 and 20-23.
    #[test]
    fn the_lamps_hours_are_night() {
        let night: Vec<u8> = (0..24).filter(|&h| is_night(h)).collect();
        assert_eq!(night, vec![0, 1, 2, 3, 4, 5, 20, 21, 22, 23]);
        assert!(!is_night(24));
    }

    #[test]
    fn a_rectangle_is_checked_once_and_reads_back_as_its_options() {
        assert!(GroundRect::new([0.0, 0.0, 1.0, 1.0]).is_ok());
        for bad in [
            [1.0, 0.0, 1.0, 2.0],
            [0.0, 3.0, 1.0, 2.0],
            [f32::NAN, 0.0, 1.0, 1.0],
            [0.0, 0.0, f32::INFINITY, 1.0],
        ] {
            assert!(GroundRect::new(bad).is_err(), "{bad:?}");
        }
        let r = GroundRect::new([95.0, -1034.0, 295.5, -834.0]).unwrap();
        assert!(r.contains(95.0, -1034.0) && !r.contains(295.5, -900.0));
        // through a declaration that takes a rectangle: all four or none, checked as GroundRect::new
        let volumes = [("foliage", "keep"), ("river", "keep")].map(|(k, v)| (k.to_owned(), v.to_owned()));
        let mut o = Options::from(volumes.clone());
        r.insert_options(&mut o);
        let made = |o: &Options| crate::passes::collision::LOWER_GTA_COLLISION.instance(o);
        assert_eq!(made(&o).unwrap().options(), o);
        assert!(
            !made(&Options::from(volumes))
                .unwrap()
                .options()
                .contains_key("x0")
        );
        o.remove("y1");
        assert!(made(&o).is_err());
        o.insert("y1".into(), "-2000".into());
        assert!(made(&o).is_err());
    }

    #[test]
    fn an_hour_is_0_to_23() {
        assert_eq!(Hour::new(23).map(Hour::get), Ok(23));
        assert!(Hour::new(24).is_err());
        assert!(Hour::parse("7.5").is_err() && Hour::parse("-1").is_err());
        assert!(Hour::new(22).unwrap().is_night() && !Hour::new(12).unwrap().is_night());
    }
}
