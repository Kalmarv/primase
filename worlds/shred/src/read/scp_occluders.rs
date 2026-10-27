//! Occluder sections (type b) of the SCP blob, read as authored.
//!
//! A section holds `count` records of 16 bytes from its data offset, then for each record in order a 64-byte matrix
//! and an outline of line segments (each end a vec4 of f32 x, y, z and w = 1). Record words (big-endian): `0` the
//! name hash (sdbm of `RMOccluder`, `RMOccluder<N>` or, for 65 of the 341 on the disc, a name not known), `1` the offset of the record's matrix from the section start, `2` the offset
//! of its outline (the matrix's plus 64), `3` flags. The low half of the flags says the outline's shape: `0x0800` is
//! four segments (a rectangle's outline, 128 bytes) and `0x1801` twelve (a box's edges, 384 bytes); the high half
//! varies from record to record and is not understood, so it is kept. The matrices and outlines tile the section
//! exactly. The only consumer found is the Lua binding `set_occluder_enabled`; the engine has no section handler
//! for the type, so what the outline is measured in (the matrix's local space, by the stored values) is read from
//! the data, not from the engine.
use super::scene::Unread;
use super::scp_tables::{Section, matrix};
use crate::{Result, invalid};
use primase_core::Transform;
use primase_foundations::bytes::{Endian, Field, View};

const RECORD: usize = 16;
const MATRIX: usize = 64;
const SEGMENT: usize = 32;

/// The low flag half of an outline of four segments.
pub const SHAPE_RECTANGLE: u32 = 0x0800;
/// The low flag half of an outline of twelve segments.
pub const SHAPE_BOX: u32 = 0x1801;

fn segments(flags: u32) -> Option<usize> {
    match flags & 0xffff {
        SHAPE_RECTANGLE => Some(4),
        SHAPE_BOX => Some(12),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Occluder {
    pub section: usize,
    pub index: u32,
    /// Word 0.
    pub name: u32,
    /// Word 3 as stored: the low half is the shape ([`SHAPE_RECTANGLE`], [`SHAPE_BOX`]), the high half unknown.
    pub flags: u32,
    /// The record's matrix.
    pub world: Transform,
    /// The outline's segments, each as its two end points (x, y, z), as stored.
    pub outline: Vec<[[f32; 3]; 2]>,
}

pub fn decode(scp: &[u8], s: &Section, unread: &mut Vec<Unread>) -> Result<Vec<Occluder>> {
    let v = View::new(scp);
    let n = s.count as usize;
    let mut refuse = |reason| {
        unread.push(Unread {
            file: "scp",
            offset: s.offset,
            bytes: s.size,
            reason,
        });
        Ok(Vec::new())
    };
    let Some(records) = n
        .checked_mul(RECORD)
        .and_then(|b| b.checked_add(s.data))
        .filter(|&b| b <= s.size)
    else {
        return refuse("occluder section whose records do not fit");
    };
    let mut next = records;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let at = s.offset + s.data + i * RECORD;
        let word = |k: usize| v.u32(at + 4 * k, Endian::Big).field("occluder record");
        let Some(edges) = segments(word(3)?) else {
            return refuse("occluder section with an outline shape not decoded");
        };
        let len = MATRIX + edges * SEGMENT;
        if word(1)? as usize != next || word(2)? as usize != next + MATRIX || next + len > s.size {
            return Err(invalid(
                at + 4,
                "scp: occluder matrix and outline off their place",
            ));
        }
        let (m, o) = (s.offset + next, s.offset + next + MATRIX);
        let mut outline = Vec::with_capacity(edges);
        for e in 0..edges {
            let mut seg = [[0.; 3]; 2];
            for (end, p) in seg.iter_mut().enumerate() {
                let q = o + e * SEGMENT + end * 16;
                for (k, x) in p.iter_mut().enumerate() {
                    *x = v.f32(q + 4 * k, Endian::Big).field("occluder outline")?;
                    if !x.is_finite() {
                        return Err(invalid(q + 4 * k, "scp: nonfinite occluder outline"));
                    }
                }
                if v.f32(q + 12, Endian::Big).field("occluder outline")? != 1. {
                    return Err(invalid(q + 12, "scp: occluder outline w is not 1"));
                }
            }
            outline.push(seg);
        }
        out.push(Occluder {
            section: s.offset,
            index: i as u32,
            name: word(0)?,
            flags: word(3)?,
            world: matrix(v, m)?,
            outline,
        });
        next += len;
    }
    if next != s.size {
        return refuse("occluder section whose matrices and outlines do not tile it");
    }
    Ok(out)
}
