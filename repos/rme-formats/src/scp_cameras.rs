//! Spot camera sections (type 3) of the SCP blob, read as authored.
//!
//! A section holds `count` records of 48 bytes from its data offset, then `count * 2` 64-byte matrices; its size is
//! `data + count * 176`. The section's engine handler copies each record's two matrices into the object whose id the
//! record names, which is how it is known to be a placement of two frames. Record words (big-endian): `0` the
//! record's own id (the same hash in most records), `1`, `3`, `9`, `10` and `11` pool leftovers (not read), `2` the
//! hash of a spot name (`BigJumpOne`, `Chlg17_ObjSpot_Cam12_AnyTrick`: names of the database's spots), `4` the
//! parent id, `5` an f32 (53.638 in every record), `6` a flag (1 in every record), `7` and `8` the offsets, from the
//! section start, of the two matrices. That these are cameras is read from the spot names (`..._Cam12_...`) and the
//! frame pair; the engine function that uses them was not traced.
use crate::Affine;
use crate::scene::Unread;
use crate::scp_tables::{Section, matrix};
use crate::{Result, invalid};
use primase_foundations::bytes::{Endian, Field, View};

const RECORD: usize = 48;
const MATRIX: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct SpotCamera {
    pub section: usize,
    pub index: u32,
    /// Word 0.
    pub id: u32,
    /// Word 2: sdbm of a spot name as written.
    pub name: u32,
    /// Word 4.
    pub parent: u32,
    /// Word 5 as stored.
    pub value: f32,
    /// Word 6.
    pub flag: u32,
    /// The first matrix (word 7): the frame the record places, which the second looks along. Unproven reading.
    pub camera: Affine,
    /// The second matrix (word 8): the frame of the point looked at. Unproven reading: its linear part is
    /// diag(-1, 1, 1) in 159 of 281 records.
    pub target: Affine,
}

pub fn decode(scp: &[u8], s: &Section, unread: &mut Vec<Unread>) -> Result<Vec<SpotCamera>> {
    let v = View::new(scp);
    let n = s.count as usize;
    if n.checked_mul(RECORD + 2 * MATRIX)
        .and_then(|b| b.checked_add(s.data))
        .is_none_or(|b| b != s.size)
    {
        unread.push(Unread {
            file: "scp",
            offset: s.offset,
            bytes: s.size,
            reason: "spot camera section whose size is not data offset + count * 176",
        });
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(n);
    let (low, high) = (s.data + n * RECORD, s.size);
    for i in 0..n {
        let at = s.offset + s.data + i * RECORD;
        let word = |k: usize| v.u32(at + 4 * k, Endian::Big).field("spot camera record");
        let mut frames = [Affine { cols: [[0.; 3]; 4] }; 2];
        for (frame, k) in frames.iter_mut().zip([7, 8]) {
            let rel = word(k)? as usize;
            if rel < low || !rel.is_multiple_of(4) || rel.checked_add(MATRIX).is_none_or(|e| e > high) {
                return Err(invalid(at + 4 * k, "scp: spot camera matrix outside its array"));
            }
            *frame = matrix(v, s.offset + rel)?;
        }
        let value = v.f32(at + 20, Endian::Big).field("spot camera record")?;
        if !value.is_finite() {
            return Err(invalid(at + 20, "scp: nonfinite spot camera value"));
        }
        out.push(SpotCamera {
            section: s.offset,
            index: i as u32,
            id: word(0)?,
            name: word(2)?,
            parent: word(4)?,
            value,
            flag: word(6)?,
            camera: frames[0],
            target: frames[1],
        });
    }
    Ok(out)
}
