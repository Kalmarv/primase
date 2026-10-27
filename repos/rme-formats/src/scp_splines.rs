//! Spline sections of the SCP blob, read as authored.
//!
//! A spline section (type 0, 1, 9, 0xA or 0xC; the low half of the flags varies and means nothing here) holds `count`
//! records of 56 bytes from its data offset, then their arrays; the record's pointers are offsets from the section
//! start. The five types are the game's five spline classes: the executable registers each type's loader with the
//! same spline manager (a pool of 5 classes of 1,000 records of 56 bytes) under class index 0, 1, 2, 3 and 4. Record words
//! (big-endian): `0` id (sdbm of the spline's name), `1` parent group id, `2` the id again (not always), `3` unread,
//! `4` 1 (3 once), `5` point count, `6` a second count (equal to `5` except in a few records), `7` and `8` small
//! integers, `9` surface id, `10` pointer to a 64-byte matrix, `11` pointer to `count` world-space points (3 x f32),
//! `12` pointer to `count` f32 that increase from 0 (not arc length), `13` unread.
use crate::scene::Unread;
use crate::scp_tables::Section;
use crate::{Result, invalid};
use primase_foundations::bytes::{Endian, Field, View};

const RECORD: usize = 56;

/// What a spline section's type says its splines are: the game's spline class of that index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SplineKind {
    /// Class 0, section type 0: splines that carry a surface id. See [`Spline::surface`]: they are the probable
    /// grind rails (their parent groups are named `grind1` to `grind3`); not proven, so the reader does not call
    /// them rails.
    RailCandidate,
    /// Class 1, type 1: the guide ribbons and flythrough cameras (`NavGuide_<set><n>`, `<name>_CamSpline`); points
    /// hover about 2 above the ground.
    Guide,
    /// Class 2, type 9: the walking network of pedestrians and AI skaters (edges between `AIJunction`/`NavJunction`
    /// chunks); every point lies on the ground.
    AiPath,
    /// Class 3, type 0xA: long splines (60 points in most) of the navigation layers.
    NavPath,
    /// Class 4, type 0xC: few and short (2 to a few points), in three level layers.
    ShortPath,
}
impl SplineKind {
    /// Every class, in class order.
    pub const ALL: [Self; 5] = [
        Self::RailCandidate,
        Self::Guide,
        Self::AiPath,
        Self::NavPath,
        Self::ShortPath,
    ];

    /// The kind of a section by its flags (`flags >> 16` is the type), `None` for any section that is not a
    /// spline section.
    pub fn of_section(flags: u32) -> Option<Self> {
        match flags >> 16 {
            0 => Some(Self::RailCandidate),
            1 => Some(Self::Guide),
            9 => Some(Self::AiPath),
            0xa => Some(Self::NavPath),
            0xc => Some(Self::ShortPath),
            _ => None,
        }
    }

    /// The game's spline class index.
    pub fn class(self) -> usize {
        match self {
            Self::RailCandidate => 0,
            Self::Guide => 1,
            Self::AiPath => 2,
            Self::NavPath => 3,
            Self::ShortPath => 4,
        }
    }

    /// A short label for what a spline of this kind is: `rail_candidate`, `guide`, `ai_path`, `nav_path` or `short_path`.
    pub fn curve_kind(self) -> &'static str {
        match self {
            Self::RailCandidate => "rail_candidate",
            Self::Guide => "guide",
            Self::AiPath => "ai_path",
            Self::NavPath => "nav_path",
            Self::ShortPath => "short_path",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spline {
    /// Offset of the section holding it, its flags and its index in the section.
    pub section: usize,
    pub flags: u32,
    pub index: u32,
    pub kind: SplineKind,
    /// sdbm of the spline's name.
    pub id: u32,
    pub parent: u32,
    /// Words 5 and 6: the point count and a second count that is equal except in a few records.
    pub counts: [u32; 2],
    /// Words 7 and 8: 0 for every spline outside class 0; for rail candidates 0 to 2 and 0 or 1. Meaning unproven.
    pub words: [u32; 2],
    /// Word 9. For rail candidates a `MAIN.DB` `surfaces.id` (proven by the data: all values are in the table and
    /// the snow surfaces occur only on the snow levels); 0 for every other spline.
    pub surface: u32,
    /// The 64-byte matrix word 10 points at: unproven (a spline-local frame; the points are already in level space).
    pub matrix: [f32; 16],
    /// World-space points, level space, as stored.
    pub points: Vec<[f32; 3]>,
    /// Per-point values that increase from zero, as stored. Not arc length.
    pub params: Vec<f32>,
}

/// Read every record of one spline section; a section that does not fit is reported unread and gives no splines.
pub fn decode(scp: &[u8], s: &Section, kind: SplineKind, unread: &mut Vec<Unread>) -> Result<Vec<Spline>> {
    let v = View::new(scp);
    let end = s.offset + s.size;
    let region = s.offset + s.data;
    let n = s.count as usize;
    let bad = |reason: &'static str, unread: &mut Vec<Unread>| {
        unread.push(Unread {
            file: "scp",
            offset: s.offset,
            bytes: s.size,
            reason,
        });
        Ok(Vec::new())
    };
    if n.checked_mul(RECORD)
        .and_then(|b| b.checked_add(region))
        .is_none_or(|b| b > end)
    {
        return bad("spline section whose records do not fit its size", unread);
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let at = region + i * RECORD;
        let word = |k: usize| v.u32(at + 4 * k, Endian::Big).field("spline record");
        let count = word(5)? as usize;
        let (pa, pb, pc) = (word(10)? as usize, word(11)? as usize, word(12)? as usize);
        let inside = |p: usize, len: usize| {
            p.checked_add(len)
                .and_then(|e| s.offset.checked_add(e))
                .is_some_and(|e| e <= end)
        };
        if !(inside(pa, 64)
            && count.checked_mul(12).is_some_and(|l| inside(pb, l))
            && count.checked_mul(4).is_some_and(|l| inside(pc, l)))
        {
            return Err(invalid(at, "scp: spline arrays outside their section"));
        }
        let f = |at: usize| -> Result<f32> {
            let x = v.f32(at, Endian::Big).field("spline data")?;
            if x.is_finite() {
                Ok(x)
            } else {
                Err(invalid(at, "scp: nonfinite spline value"))
            }
        };
        let mut matrix = [0.; 16];
        for (k, m) in matrix.iter_mut().enumerate() {
            *m = f(s.offset + pa + 4 * k)?;
        }
        let mut points = Vec::with_capacity(count);
        let mut params = Vec::with_capacity(count);
        for k in 0..count {
            let p = s.offset + pb + 12 * k;
            points.push([f(p)?, f(p + 4)?, f(p + 8)?]);
            params.push(f(s.offset + pc + 4 * k)?);
        }
        out.push(Spline {
            section: s.offset,
            flags: s.flags,
            index: i as u32,
            kind,
            id: word(0)?,
            parent: word(1)?,
            counts: [word(5)?, word(6)?],
            words: [word(7)?, word(8)?],
            surface: word(9)?,
            matrix,
            points,
            params,
        });
    }
    Ok(out)
}
