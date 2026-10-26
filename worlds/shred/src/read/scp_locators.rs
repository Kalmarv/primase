//! Locator sections (type 5) of the SCP blob, read as authored.
//!
//! A section holds `count` records of 16 bytes from its data offset, then `count` 64-byte matrices; its size is
//! `data + count * 80`. Record words (big-endian): `0` a class hash (sdbm of `RMLocator` in the one record there is),
//! `1` the name hash, `2` a kind hash (sdbm of `Waypoint`), `3` the matrix's offset from the section start. The
//! files hold one such record (the `echo` level's); the class and kind are named by their hashes.
use super::scene::Unread;
use super::scp_tables::{Section, matrix};
use crate::{Result, invalid};
use primase_bytes::{Endian, Field, View};
use primase_core::Transform;

const RECORD: usize = 16;
const MATRIX: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct Locator {
    pub section: usize,
    pub index: u32,
    pub class: u32,
    pub name: u32,
    pub kind: u32,
    pub world: Transform,
}

/// The class and kind names whose hashes the locator record carries (sdbm of the name).
pub const CLASS_LOCATOR: &str = "RMLocator";
pub const KIND_WAYPOINT: &str = "Waypoint";

pub fn decode(scp: &[u8], s: &Section, unread: &mut Vec<Unread>) -> Result<Vec<Locator>> {
    let v = View::new(scp);
    let n = s.count as usize;
    if n.checked_mul(RECORD + MATRIX)
        .and_then(|b| b.checked_add(s.data))
        .is_none_or(|b| b != s.size)
    {
        unread.push(Unread {
            file: "scp",
            offset: s.offset,
            bytes: s.size,
            reason: "locator section whose size is not data offset + count * 80",
        });
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let at = s.offset + s.data + i * RECORD;
        let word = |k: usize| v.u32(at + 4 * k, Endian::Big).field("locator record");
        let rel = word(3)? as usize;
        if rel < s.data + n * RECORD
            || !rel.is_multiple_of(4)
            || rel.checked_add(MATRIX).is_none_or(|e| e > s.size)
        {
            return Err(invalid(at + 12, "scp: locator matrix outside its array"));
        }
        out.push(Locator {
            section: s.offset,
            index: i as u32,
            class: word(0)?,
            name: word(1)?,
            kind: word(2)?,
            world: matrix(v, s.offset + rel)?,
        });
    }
    Ok(out)
}
