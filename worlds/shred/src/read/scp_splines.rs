//! Spline sections of the SCP blob, read as authored.
//!
//! A spline section (type 0, 1 or 9 with low flags 0x80/0x81 for types 0 and 1) holds `count` records of 56 bytes
//! from its data offset, then their arrays; the record's pointers are offsets from the section start. Record words
//! (big-endian): `0` id (sdbm of the spline's name), `1` parent group id, `2` the id again (not always), `3` unread,
//! `4` 1 (3 once), `5` point count, `6` a second count (equal to `5` except in a few records), `7` and `8` small
//! integers, `9` surface id, `10` pointer to a 64-byte matrix, `11` pointer to `count` world-space points (3 x f32),
//! `12` pointer to `count` f32 that increase from 0 (not arc length), `13` unread.
use super::scene::Unread;
use super::scp_tables::Section;
use crate::{Result, invalid};
use primase_bytes::{Endian, Field, View};

const RECORD: usize = 56;

/// What a spline section's type says its splines are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SplineKind {
    /// Section type 9: the walking network of pedestrians and AI skaters (edges between `AIJunction`/`NavJunction`
    /// chunks); every point lies on the ground.
    AiPath,
    /// Section type 1: the guide ribbons and flythrough cameras (`NavGuide_<set><n>`, `<name>_CamSpline`); points
    /// hover about 2 above the ground.
    Guide,
    /// Section type 0, flags 0x80/0x81: splines that carry a surface id. See [`Spline::surface`]: they are the
    /// probable grind rails; not proven, so the reader does not call them rails.
    RailCandidate,
}
impl SplineKind {
    /// The kind of a section by its flags, `None` for any section that is not a spline section.
    pub fn of_section(flags: u32) -> Option<Self> {
        match (flags >> 16, flags & 0xffff) {
            (9, _) => Some(Self::AiPath),
            (1, 0x80 | 0x81) => Some(Self::Guide),
            (0, 0x80 | 0x81) => Some(Self::RailCandidate),
            _ => None,
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
    /// Words 7 and 8: 0 for every AI path and guide; for rail candidates 0 to 2 and 0 or 1. Meaning unproven.
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
