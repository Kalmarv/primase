//! A terrain's node layer and its chunks: TerrainNodeLayer, the quadtree of nodes (each naming the TNLChunk holding
//! its tiles), and TNLChunk, one node's nine blocks and its height range and material mask. Little-endian throughout.
//!
//! - **TerrainNodeLayer:** `01`, a u32 count n, n items of 25 bytes (a u16, 10 bytes, a u32 position, `00`, a u64
//!   TNLChunk ClassID), the count again. A position p is a node of a level-order quadtree: level L holds positions
//!   (4^L - 1) / 3 up to (4^(L+1) - 1) / 3 - 1, row by row (j = p - (4^L - 1) / 3, row j / 2^L along y, column j mod 2^L
//!   along x), a node covering 1 / 2^L of the world's side.
//! - **TNLChunk:** `01`, a u32 (the body's length less 5), the u16 0xFEED, a u16 tag, nine blocks each a u32 length and
//!   its bytes, an f32 a, an f32 b and a 32-byte mask; on a leaf, a tenth block (a u32 length and its bytes) after them.
//!   The tag is 0x1C14 on nodes with children and 0x9C14 on leaves; its low byte, 0x14, is the version this layout is
//!   read for (others are refused).
//!
//! The grammars are the Riders Republic notes' (docs/terrain.md §3, consumed exactly on every node of MAD_USNAT), the
//! quadtree order theirs (each node's a and b against the FloorHeightData cells under it, Spearman 0.96 to 1.00 per
//! level, other orders -0.65 or less). Blocks 0 and 2 to 7 are coded (see [`crate::tnl_code`]); blocks 1 and 8
//! are LZ4 blocks of planar BC1 and BC4 tiles.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::fields::{BodyError, LE, body_error, check, end};
use primase_foundations::bytes::{Cursor, Field};

/// A record type: the CRC-32 of `TerrainNodeLayer`.
pub const TERRAIN_NODE_LAYER: u32 = 0xDCE0_537E;
/// A record type: the CRC-32 of `TNLChunk`.
pub const TNL_CHUNK: u32 = 0x4A05_4F51;

/// Why a node layer or chunk body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("terrain {0}")]
    Body(#[from] BodyError),
}
body_error!(Error, Error::Body);

/// One item of a node layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerItem {
    /// The u16 leading it, as stored.
    pub word: u16,
    /// The 10 bytes after it, as stored.
    pub constant: [u8; 10],
    /// Its node's quadtree position.
    pub position: u32,
    /// Its node's TNLChunk ClassID.
    pub chunk: u64,
}

/// A TerrainNodeLayer body as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNodeLayer {
    pub items: Vec<LayerItem>,
}

impl StoredNodeLayer {
    pub fn parse(body: &[u8]) -> Result<StoredNodeLayer, Error> {
        let mut c = Cursor::new(body);
        let at = c.position();
        let first = c.array::<1>().field("first byte")?;
        check("first byte", at, 1, u64::from(first[0]))?;
        let n = c.u32(LE).field("item count")?;
        let mut items = Vec::new();
        for _ in 0..n {
            let word = c.u16(LE).field("item")?;
            let constant = c.array::<10>().field("item")?;
            let position = c.u32(LE).field("item position")?;
            let at = c.position();
            let zero = c.array::<1>().field("item byte")?;
            check("item byte", at, 0, u64::from(zero[0]))?;
            items.push(LayerItem {
                word,
                constant,
                position,
                chunk: c.u64(LE).field("item chunk")?,
            });
        }
        let at = c.position();
        let again = c.u32(LE).field("item count again")?;
        check("item count again", at, u64::from(n), u64::from(again))?;
        end(&c)?;
        Ok(StoredNodeLayer { items })
    }
}

/// A quadtree position's node: its level (0 the root), and its row (along y) and column (along x) at that level.
/// `None` past level 30.
pub fn node(position: u32) -> Option<(u32, u32, u32)> {
    let mut first = 0u32;
    for level in 0..31u32 {
        let side = 1u32.checked_shl(level)?;
        let count = side.checked_mul(side)?;
        let next = first.checked_add(count)?;
        if position < next {
            let j = position.checked_sub(first)?;
            return Some((level, j.checked_div(side)?, j.checked_rem(side)?));
        }
        first = next;
    }
    None
}

/// The quadtree position of the node at `level`, `row`, `column`; `None` outside the level.
pub fn position(level: u32, row: u32, column: u32) -> Option<u32> {
    let side = 1u32.checked_shl(level)?;
    if row >= side || column >= side {
        return None;
    }
    let first = (0..level).try_fold(0u32, |sum, l| {
        sum.checked_add(1u32.checked_shl(l)?.checked_pow(2)?)
    })?;
    first.checked_add(row.checked_mul(side)?)?.checked_add(column)
}

/// A TNLChunk body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredTnlChunk {
    pub tag: u16,
    /// The nine blocks, in stored order.
    pub blocks: [Vec<u8>; 9],
    /// The node's height range, as fractions of the terrain's.
    pub a: f32,
    pub b: f32,
    /// The set of the node's material values (block 2's), one bit each.
    pub mask: [u8; 32],
    /// A leaf's tenth block, as stored (what it holds isn't read).
    pub tenth: Option<Vec<u8>>,
}

impl StoredTnlChunk {
    pub fn parse(body: &[u8]) -> Result<StoredTnlChunk, Error> {
        let mut c = Cursor::new(body);
        let first = c.array::<1>().field("first byte")?;
        check("first byte", 0, 1, u64::from(first[0]))?;
        let length = c.u32(LE).field("length")?;
        check(
            "length",
            1,
            u64::try_from(body.len().saturating_sub(5)).unwrap_or(u64::MAX),
            u64::from(length),
        )?;
        let magic = c.u16(LE).field("magic")?;
        check("magic", 5, 0xFEED, u64::from(magic))?;
        let tag = c.u16(LE).field("tag")?;
        if tag != 0x1C14 && tag != 0x9C14 {
            return Err(BodyError::Value {
                what: "tag",
                at: 7,
                found: u64::from(tag),
                read: "0x1C14 or 0x9C14",
            }
            .into());
        }
        let blocks = [(); 9].map(|()| block(&mut c, "block"));
        let [b0, b1, b2, b3, b4, b5, b6, b7, b8] = blocks;
        let blocks = [b0?, b1?, b2?, b3?, b4?, b5?, b6?, b7?, b8?];
        let a = c.f32(LE).field("a")?;
        let b = c.f32(LE).field("b")?;
        let mask = c.array::<32>().field("mask")?;
        let tenth = if tag == 0x9C14 {
            Some(block(&mut c, "tenth block")?)
        } else {
            None
        };
        end(&c)?;
        Ok(StoredTnlChunk {
            tag,
            blocks,
            a,
            b,
            mask,
            tenth,
        })
    }

    /// Whether `value` is in the node's material mask.
    pub fn in_mask(&self, value: u8) -> bool {
        let (byte, bit) = (usize::from(value / 8), value % 8);
        self.mask.get(byte).is_some_and(|m| m >> bit & 1 == 1)
    }
}

/// A u32 length, then that many bytes.
fn block(c: &mut Cursor, what: &'static str) -> Result<Vec<u8>, BodyError> {
    let n = c.u32(LE).field(what)?;
    Ok(c.take(n as usize).field(what)?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadtree_positions_number_levels_row_by_row() {
        assert_eq!(node(0), Some((0, 0, 0)));
        assert_eq!(node(1), Some((1, 0, 0)));
        assert_eq!(node(4), Some((1, 1, 1)));
        assert_eq!(node(5), Some((2, 0, 0)));
        // the first level-8 node, and the last of 87,381
        assert_eq!(node(21_845), Some((8, 0, 0)));
        assert_eq!(node(87_380), Some((8, 255, 255)));
        for p in [0, 3, 20, 21_845, 50_000, 87_380] {
            let (l, r, c) = node(p).expect("node");
            assert_eq!(position(l, r, c), Some(p));
        }
        assert_eq!(position(1, 2, 0), None);
    }
}
