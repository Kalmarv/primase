//! The shader lookup tables of `dingolevel_root`: material key -> program indices (program lookup, type 0xE2E6955B,
//! after the program records) and material key -> streamable texture set (type 0x2D254A89). Rows are a map: the first
//! row of a repeated key wins, as the game's reader appends.

use std::collections::BTreeMap;

use crate::{Rd, Result, bad, capacity};

pub const PROGRAM_LOOKUP: u32 = 0xE2E6_955B;
pub const TEXTURE_LOOKUP: u32 = 0x2D25_4A89;

/// A program lookup resource: everything before the rows (the program records), and the rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramLookup {
    pub head: Vec<u8>,
    pub rows: BTreeMap<u64, Vec<u32>>,
    /// Keys in stored order (equal to sorted order in every table seen; kept to reproduce the bytes).
    pub order: Vec<u64>,
}

pub fn read_programs(d: &[u8], meta: &[u8; 16]) -> Result<ProgramLookup> {
    let m = Rd::new(meta, "program lookup meta");
    let (program, reloc, lookup) = (m.u32le(0)? as usize, m.u32le(4)? as usize, m.u32le(8)? as usize);
    let start = program + reloc;
    if start > d.len() || lookup < 4 || lookup != d.len() - start {
        return Err(bad("program lookup", 0, "metadata does not match the payload"));
    }
    let r = Rd::new(d, "program lookup");
    let count = r.u32le(start)? as usize;
    let mut pos = start + 4;
    let mut out = ProgramLookup {
        head: d[..start].to_vec(),
        rows: BTreeMap::new(),
        order: Vec::with_capacity(capacity("program lookup", count, 12, d.len() - start - 4)?),
    };
    for _ in 0..count {
        let (key, n) = (r.u64le(pos)?, r.u32le(pos + 8)? as usize);
        pos += 12;
        if n == 0 || n > 4096 {
            return Err(bad("program lookup", pos, format!("row of {n} programs")));
        }
        let row = (0..n).map(|i| r.u32le(pos + 4 * i)).collect::<Result<Vec<_>>>()?;
        pos += 4 * n;
        if out.rows.contains_key(&key) {
            return Err(bad("program lookup", pos, format!("repeated key {key:016x}")));
        }
        out.rows.insert(key, row);
        out.order.push(key);
    }
    if pos != d.len() {
        return Err(bad("program lookup", pos, "trailing data"));
    }
    Ok(out)
}

/// The payload and metadata (only the rows' length changes) of a program lookup; rows in `order`, then any new keys
/// sorted.
pub fn write_programs(t: &ProgramLookup, meta: &[u8; 16]) -> (Vec<u8>, [u8; 16]) {
    let mut keys = t.order.clone();
    let mut extra: Vec<u64> = t.rows.keys().copied().filter(|k| !t.order.contains(k)).collect();
    keys.retain(|k| t.rows.contains_key(k));
    keys.append(&mut extra);
    let mut out = t.head.clone();
    out.extend((keys.len() as u32).to_le_bytes());
    for k in keys {
        let row = &t.rows[&k];
        out.extend(k.to_le_bytes());
        out.extend((row.len() as u32).to_le_bytes());
        for p in row {
            out.extend(p.to_le_bytes());
        }
    }
    let mut m = *meta;
    m[8..12].copy_from_slice(&((out.len() - t.head.len()) as u32).to_le_bytes());
    (out, m)
}

/// Streamable texture set rows (u64 key, u64 value), in stored order.
pub fn read_texture_sets(d: &[u8]) -> Result<Vec<(u64, u64)>> {
    if !d.len().is_multiple_of(16) {
        return Err(bad("texture set lookup", d.len(), "not a whole number of rows"));
    }
    let r = Rd::new(d, "texture set lookup");
    (0..d.len() / 16)
        .map(|i| Ok((r.u64le(16 * i)?, r.u64le(16 * i + 8)?)))
        .collect()
}

pub fn write_texture_sets(rows: &[(u64, u64)]) -> Vec<u8> {
    rows.iter()
        .flat_map(|(k, v)| k.to_le_bytes().into_iter().chain(v.to_le_bytes()))
        .collect()
}

/// The size of a program record.
pub const RECORD_LEN: usize = 168;

/// One program record of a program lookup's head, as stored (168 bytes), with the fields read from it: its key (the
/// first eight bytes) and the ids of the permutations it runs, an array of `u64` (the count at 0x60, a pointer to the
/// array at 0x64). The rest of the record (arrays of state the permutations are drawn with) is kept as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramRecord {
    pub key: u64,
    pub permutations: Vec<u64>,
    pub raw: [u8; RECORD_LEN],
}

/// The program records of a program lookup: the head's first word is their count, the eight bytes after it the
/// offset of the first, and the records follow one another. Refused: records or a permutation array outside the
/// program section the metadata states.
pub fn read_records(d: &[u8], meta: &[u8; 16]) -> Result<Vec<ProgramRecord>> {
    let m = Rd::new(meta, "program lookup meta");
    let program = m.u32le(0)? as usize;
    let r = Rd::new(
        d.get(..program)
            .ok_or_else(|| bad("program records", 0, "metadata does not match the payload"))?,
        "program records",
    );
    let (count, first) = (r.u32le(0)? as usize, r.u64le(4)? as usize);
    count
        .checked_mul(RECORD_LEN)
        .and_then(|n| n.checked_add(first))
        .filter(|&end| end <= program)
        .ok_or_else(|| {
            bad(
                "program records",
                first,
                format!("{count} records past the program section"),
            )
        })?;
    (0..count)
        .map(|i| {
            let at = first + i * RECORD_LEN;
            let (n, ptr) = (r.u32le(at + 0x60)? as usize, r.u64le(at + 0x64)? as usize);
            // the whole array lies in the section before any id is read
            r.bytes(
                ptr,
                n.checked_mul(8)
                    .ok_or_else(|| bad("program records", at, "permutation count"))?,
            )?;
            Ok(ProgramRecord {
                key: r.u64le(at)?,
                permutations: (0..n).map(|j| r.u64le(ptr + 8 * j)).collect::<Result<_>>()?,
                raw: r.arr::<RECORD_LEN>(at)?,
            })
        })
        .collect()
}

/// ExpressionShaderStreamableTextureSet: what a texture lookup row's value is the id of. It lists the texture
/// parameters of a material, not texture assets: neither the id of a texture's resource nor the GUID of its asset
/// occurs in the set of any of the 64 materials of a sample of 15 bundles (0 of 987 references; the link from a
/// material to its textures is the depot block's, [`crate::textures`]). Layout, as read from the first 192 bytes of
/// three sets (5, 2 and 14 entries; not read by this crate):
///
/// ```text
/// 0x00 u32 count    0x04 u32 end of the entries (16 + 32 x count in all three)    0x08 8 zero bytes
/// 0x10 count x 32 bytes: u32 1, u32 0x21, f32 1.0, u32 0, u64 parameter name hash (0x37e6de277c4250cc, the
///      first of one set, is a name in depot blocks), u32 type (0x14c5fc30, a texture reference), u16 1, u16 a
///      word that is not a function of the name hash (0x3487 occurs under two different hashes)
/// then a tail (88, 44 and 236 bytes in the three), not decoded
/// ```
pub const STREAMABLE_TEXTURE_SET: u32 = 0x7A63_0735;
