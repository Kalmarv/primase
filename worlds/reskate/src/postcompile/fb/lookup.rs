//! The shader lookup tables of `dingolevel_root`: material key -> program indices (program lookup, type 0xE2E6955B,
//! after the program records) and material key -> streamable texture set (type 0x2D254A89). Rows are a map: the first
//! row of a repeated key wins, as the game's reader appends.

use std::collections::BTreeMap;

use crate::postcompile::{Rd, Result, bad, capacity};

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
