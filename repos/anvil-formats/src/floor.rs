//! A FloorHeightData body: a square grid of 16-bit height words, stored in 4x4-cell blocks, and its trailer. Little-
//! endian throughout:
//!
//! `01 00 00 00`, an f32 (2.0 in MAD_USNAT, 0.0 in MAD_WhiteRoom's empty one: the cell side, 2 x 8192 = 16,384 the
//! world's side), `00`; then (dx / 4) x (dy / 4) blocks of 8 bytes, row by row (a row along x), each a u16 `lo`, a
//! u16 `hi` and a u32 of sixteen 2-bit indices, cell (r, c) of the block (r along y) at bits 2 (4 r + c); then 51
//! bytes: a u32 0; an f32 (0.0618794, bits 0x3D7D7540: metres per word step, by the Riders Republic notes' fit of
//! 20,000 ground-snapped Entities, 0.06198, 0.16% from it); an f32 (338.86); an f32 origin x, y, z (-8192, -8192,
//! -3584); a u32 0; a u32 dx and dy (8192, 8192); `01 01`, a u32 4, an f32 2.0, `01`, a u32 1.
//!
//! The grammar is the Riders Republic notes' (docs/terrain.md §2, with the header's f32 at 4, not 1), consumed
//! exactly by both of the game's FloorHeightData (the blocks fill the body between header and trailer), and `lo` is
//! at most `hi` in all 4,194,304 of MAD_USNAT's blocks. The cell order within a block is the notes' (block borders
//! continuous: a mean step of 14.25 words across them along x, 86.02 with each block's rows and columns swapped, over
//! 2,397,037 border pairs of every 7th row; measured on the game's data), and a cell's place, x =
//! origin x + cell side x column (y likewise by row), theirs against Entities (correlation 0.917 over 6,000; other
//! orientations 0.18 or less). What a cell's word is from `lo`, `hi` and its index, its height and where in the cell
//! the sample lies are not read here. The z origin, 338.86 and the trailer's last fields aren't
//! read.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::fields::LE;
use primase_foundations::bytes::{Field, FieldError, View};

/// A FloorHeightData block: two words and sixteen 2-bit indices between them, cell (r, c) of the block (r along y)
/// at bits 2 (4 r + c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloorBlock {
    pub lo: u16,
    pub hi: u16,
    pub indices: u32,
}

/// A record type: the CRC-32 of `FloorHeightData`.
pub const FLOOR_HEIGHT_DATA: u32 = 0x92AD_A771;

/// The bytes before the blocks.
const HEAD: usize = 9;
/// The bytes after them.
const TRAILER: usize = 51;

/// Why a FloorHeightData body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("FloorHeightData body: {0} bytes, fewer than its header and trailer")]
    Short(usize),
    #[error("FloorHeightData {0}")]
    Read(#[from] FieldError),
    #[error("FloorHeightData {what}: {found:#x}, not {read}")]
    Value {
        what: &'static str,
        found: u64,
        read: &'static str,
    },
    #[error("FloorHeightData: {have} bytes of blocks, not the {want} its {dx} x {dy} cells take")]
    Blocks {
        have: usize,
        want: usize,
        dx: u32,
        dy: u32,
    },
}

/// The trailer's fields, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Trailer {
    /// Metres per word step (the notes' fit, to 0.16%).
    pub scale: f32,
    /// The f32 after it (338.86), unread.
    pub unknown: f32,
    /// The grid's corner: x and y where column and row 0 lie; z unread.
    pub origin: [f32; 3],
    /// Cells along x and y.
    pub cells: [u32; 2],
    /// The 15 bytes after them, as stored.
    pub rest: [u8; 15],
}

/// A FloorHeightData body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredFloorHeight {
    /// The cell side.
    pub cell: f32,
    /// The blocks, row by row.
    pub blocks: Vec<FloorBlock>,
    pub trailer: Trailer,
}

impl StoredFloorHeight {
    pub fn parse(body: &[u8]) -> Result<StoredFloorHeight, Error> {
        let data = body
            .len()
            .checked_sub(HEAD)
            .and_then(|n| n.checked_sub(TRAILER))
            .ok_or(Error::Short(body.len()))?;
        let value = |what, found: u64, want: u64, read| {
            if found == want {
                Ok(())
            } else {
                Err(Error::Value { what, found, read })
            }
        };
        value(
            "head",
            u64::from(View::new(body).u32(0, LE).field("head")?),
            1,
            "01 00 00 00",
        )?;
        value(
            "byte 8",
            u64::from(View::new(body).u8(8).field("byte 8")?),
            0,
            "0",
        )?;
        let t = &body.get(HEAD.saturating_add(data)..).unwrap_or_default();
        value(
            "trailer word 0",
            u64::from(View::new(t).u32(0, LE).field("trailer word 0")?),
            0,
            "0",
        )?;
        value(
            "trailer word 24",
            u64::from(View::new(t).u32(24, LE).field("trailer word 24")?),
            0,
            "0",
        )?;
        let cells = [
            View::new(t).u32(28, LE).field("trailer cells")?,
            View::new(t).u32(32, LE).field("trailer cells")?,
        ];
        let mut rest = [0u8; 15];
        rest.copy_from_slice(t.get(36..).unwrap_or_default());
        let trailer = Trailer {
            scale: View::new(t).f32(4, LE).field("trailer scale")?,
            unknown: View::new(t).f32(8, LE).field("trailer word 8")?,
            origin: [
                View::new(t).f32(12, LE).field("trailer origin")?,
                View::new(t).f32(16, LE).field("trailer origin")?,
                View::new(t).f32(20, LE).field("trailer origin")?,
            ],
            cells,
            rest,
        };
        let [dx, dy] = cells;
        let want = (dx.checked_rem(4) == Some(0) && dy.checked_rem(4) == Some(0))
            .then(|| {
                let side = |d: u32| usize::try_from(d.checked_div(4)?).ok();
                side(dx)?.checked_mul(side(dy)?)?.checked_mul(8)
            })
            .flatten();
        if want != Some(data) {
            return Err(Error::Blocks {
                have: data,
                want: want.unwrap_or(usize::MAX),
                dx,
                dy,
            });
        }
        let blocks = body
            .get(HEAD..HEAD.saturating_add(data))
            .unwrap_or_default()
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&[a, b, c, d, e, f, g, h]| FloorBlock {
                lo: u16::from_le_bytes([a, b]),
                hi: u16::from_le_bytes([c, d]),
                indices: u32::from_le_bytes([e, f, g, h]),
            })
            .collect();
        Ok(StoredFloorHeight {
            cell: View::new(body).f32(4, LE).field("cell side")?,
            blocks,
            trailer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4x4-cell grid (one block) with the given block, cell side 2 and origin (-4, -4).
    fn body(block: [u8; 8]) -> Vec<u8> {
        let mut b = vec![1, 0, 0, 0];
        b.extend(2.0f32.to_le_bytes());
        b.push(0);
        b.extend(block);
        b.extend(0u32.to_le_bytes());
        for f in [0.061_879_4_f32, 338.86, -4.0, -4.0, -3584.0] {
            b.extend(f.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        b.extend([1, 1, 4, 0, 0, 0, 0, 0, 0, 0x40, 1, 1, 0, 0, 0]);
        b
    }

    #[test]
    fn a_block_reads_and_its_cells_take_their_indices_in_order() {
        // lo 100, hi 400; cell (r, c) index (4 r + c) mod 4
        let bits: u32 = (0..16).map(|k| (k % 4) << (2 * k)).sum();
        let mut block = [0u8; 8];
        block[..2].copy_from_slice(&100u16.to_le_bytes());
        block[2..4].copy_from_slice(&400u16.to_le_bytes());
        block[4..].copy_from_slice(&bits.to_le_bytes());
        let f = StoredFloorHeight::parse(&body(block)).expect("floor");
        assert_eq!((f.cell, f.trailer.cells, f.blocks.len()), (2.0, [4, 4], 1));
        assert_eq!(f.trailer.origin, [-4.0, -4.0, -3584.0]);
        assert_eq!(
            f.blocks.first(),
            Some(&FloorBlock {
                lo: 100,
                hi: 400,
                indices: bits
            })
        );
        let mut long = body(block);
        long.insert(20, 0);
        assert!(StoredFloorHeight::parse(&long).is_err());
    }
}
