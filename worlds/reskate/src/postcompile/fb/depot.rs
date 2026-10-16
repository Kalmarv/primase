//! ShaderBlockDepot resources (type 0x73312045): material key -> parameter block, and the block's parameters.
//!
//! Layout (relocatable): u64 header, u64 0, then `count` entries (u64 key, u64 descriptor offset); the blocks' data;
//! 16-aligned descriptors (u64 data offset, u64 block hash, u64 size); 16-aligned end; then the relocation list
//! (u32 offsets, zero-terminated). The 16-byte resource metadata holds (size, relocation size, version, count).

use crate::postcompile::{Rd, Result, bad};

const W: &str = "shader block depot";
pub const RTYPE: u32 = 0x7331_2045;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub hash: u64,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Depot {
    pub head: u64,
    pub version: u32,
    /// (material key, block index), in stored order.
    pub entries: Vec<(u64, usize)>,
    pub blocks: Vec<Block>,
}

pub fn read(d: &[u8], meta: &[u8; 16]) -> Result<Depot> {
    let m = Rd::new(meta, W);
    let (size, rsize, version, count) = (
        m.u32le(0)? as usize,
        m.u32le(4)? as usize,
        m.u32le(8)?,
        m.u32le(12)? as usize,
    );
    if size + rsize != d.len() {
        return Err(bad(
            W,
            0,
            format!("size {size} + relocations {rsize} != {}", d.len()),
        ));
    }
    let r = Rd::new(d, W);
    let mut dep = Depot {
        head: r.u64le(0)?,
        version,
        entries: Vec::with_capacity(count),
        blocks: Vec::new(),
    };
    let mut by_desc = std::collections::BTreeMap::new();
    for i in 0..count {
        let (key, desc) = (r.u64le(0x10 + 16 * i)?, r.u64le(0x18 + 16 * i)? as usize);
        let bi = match by_desc.get(&desc) {
            Some(&bi) => bi,
            None => {
                let (off, hash, len) = (
                    r.u64le(desc)? as usize,
                    r.u64le(desc + 8)?,
                    r.u64le(desc + 16)? as usize,
                );
                dep.blocks.push(Block {
                    hash,
                    data: r.bytes(off, len)?.to_vec(),
                });
                by_desc.insert(desc, dep.blocks.len() - 1);
                dep.blocks.len() - 1
            }
        };
        dep.entries.push((key, bi));
    }
    Ok(dep)
}

/// The layout Studio writes: header, key table, blocks in first-use order packed, 16-aligned descriptors, 16-aligned
/// end, relocation list (per entry: its block's descriptor data pointer at the block's first use, then the entry's
/// descriptor pointer), zero-terminated. Returns (payload, metadata).
pub fn write(dep: &Depot) -> (Vec<u8>, [u8; 16]) {
    let count = dep.entries.len();
    let mut out = Vec::new();
    out.extend(dep.head.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out.resize(0x10 + 16 * count, 0);
    let mut order = Vec::new();
    for &(_, bi) in &dep.entries {
        if !order.contains(&bi) {
            order.push(bi);
        }
    }
    let mut data_at = std::collections::BTreeMap::new();
    for &bi in &order {
        data_at.insert(bi, out.len());
        out.extend(&dep.blocks[bi].data);
    }
    out.resize(crate::postcompile::align(out.len(), 16), 0);
    let mut desc_at = std::collections::BTreeMap::new();
    for &bi in &order {
        desc_at.insert(bi, out.len());
        let b = &dep.blocks[bi];
        out.extend((data_at[&bi] as u64).to_le_bytes());
        out.extend(b.hash.to_le_bytes());
        out.extend((b.data.len() as u64).to_le_bytes());
    }
    let mut reloc = Vec::new();
    let mut done = std::collections::BTreeSet::new();
    for (i, &(key, bi)) in dep.entries.iter().enumerate() {
        out[0x10 + 16 * i..0x18 + 16 * i].copy_from_slice(&key.to_le_bytes());
        out[0x18 + 16 * i..0x20 + 16 * i].copy_from_slice(&(desc_at[&bi] as u64).to_le_bytes());
        if done.insert(bi) {
            reloc.push(desc_at[&bi] as u32);
        }
        reloc.push((0x10 + 16 * i + 8) as u32);
    }
    out.resize(crate::postcompile::align(out.len(), 16), 0);
    let size = out.len();
    reloc.push(0);
    for r in &reloc {
        out.extend(r.to_le_bytes());
    }
    let mut meta = [0u8; 16];
    for (i, v) in [size as u32, 4 * reloc.len() as u32, dep.version, count as u32]
        .iter()
        .enumerate()
    {
        meta[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    (out, meta)
}

/// Parameter value types and their element sizes; 0x14c5fc30 is a texture reference.
pub const TEXTURE: u32 = 0x14c5_fc30;
fn elem_size(ty: u32) -> Option<usize> {
    Some(match ty {
        0x2bb6_5c2d | 0x36ed_3444 | 0x8e4a_817e | 0x57f9_70ca | 0x7826_e998 => 4,
        0x885e_ff59 => 12,
        0x7f99_9829 => 16,
        0x2a36_e9d5 => 8,
        _ => return None,
    })
}

/// One parameter of a block: name hash (its top 16 bits name the texture slot), type, and where its value bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: u64,
    pub ty: u32,
    pub count: u16,
    pub extra: u16,
    /// Offset of the parameter record in the block, and its length.
    pub at: usize,
    pub len: usize,
}

impl Param {
    /// The texture slot (top 16 bits of the name hash) of a texture reference.
    pub fn slot(&self) -> u16 {
        (self.name >> 48) as u16
    }
    /// A texture reference's 33 value bytes: flag, then two GUIDs (the second the texture asset's file GUID).
    pub fn texture_ref<'a>(&self, block: &'a [u8]) -> Option<&'a [u8]> {
        (self.ty == TEXTURE).then(|| &block[self.at + 16..self.at + 49])
    }
}

/// Every parameter of a block, walking to its exact end.
pub fn params(block: &[u8]) -> Result<Vec<Param>> {
    let r = Rd::new(block, "shader block");
    let n = r.u32le(0)? as usize;
    let mut pos = 4;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (name, ty, count, extra) = (
            r.u64le(pos)?,
            r.u32le(pos + 8)?,
            r.u16le(pos + 12)?,
            r.u16le(pos + 14)?,
        );
        let len = if ty == TEXTURE {
            49
        } else {
            let e = elem_size(ty).ok_or_else(|| {
                bad(
                    "shader block",
                    pos,
                    format!("unknown type {ty:#010x} (parameter {i} of {n})"),
                )
            })?;
            20 + r.u32le(pos + 16)? as usize * e
        };
        r.bytes(pos, len)?;
        out.push(Param {
            name,
            ty,
            count,
            extra,
            at: pos,
            len,
        });
        pos += len;
    }
    if pos != block.len() {
        return Err(bad(
            "shader block",
            pos,
            format!("{} bytes after the last parameter", block.len() - pos),
        ));
    }
    Ok(out)
}
