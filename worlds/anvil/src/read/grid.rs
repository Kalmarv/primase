//! A world's grids: the World's GridDescriptions (each a GridLayout and its GridPartition), a GridPartition's cells
//! (each the ClassID of the GridCellDataBlock listing what the cell holds), and a GridCellDataBlock's list. Every type
//! id here is the CRC-32 of the type's name. Little-endian throughout.
//!
//! - **World** (its start; the rest isn't read): `01`, a u32 count n, n GridDescriptions, each a u64 id, 0xAFE5F9B4,
//!   then a GridLayout: a u64 0, 0x7DB083ED, a run of u16, five i32, 3 bytes, a reference to its GridPartition (or
//!   none). The run is 5 u16 in the base forges' Worlds and 4 in their patches' (all 12 GridLayouts of Riders
//!   Republic, in MAD_WhiteRoom and MAD_USNAT); nothing before it in the body, its entry's blocks or the forge header
//!   tells them apart, so its length is the caller's stated choice, and the reader refuses a World whose fields after
//!   it don't follow.
//! - **GridPartition**: `01`, a u32 level count, a u32 cell count c, c cells each a u64 0, 0x3435AD40 (GridCell),
//!   `00` and a u64 (a GridCellDataBlock's ClassID, or 0), then a u32 0.
//! - **GridCellDataBlock**: `01`, a u32 count n, n references, a u32 (n less those to GIDynResources on every block
//!   the notes measured), a u64 (its cell's number, or a DataLayer's ClassID).
//!
//! Grammars from the Riders Republic notes (docs/world.md §1-2, docs/placement.md §5 and §10), consumed to the last
//! byte by the census (worlds/anvil/tests/census.rs) on every GridPartition and GridCellDataBlock of the game. The
//! five i32's names are AnvilToolkit's AC schemas' (a tool claim); what the u16 run, the 3 bytes and the u32 before
//! the key are isn't read.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::reference::{self, Reference};

/// A record type: the CRC-32 of `World`.
pub const WORLD: u32 = 0xFBB6_3E47;
/// A record type: the CRC-32 of `GridPartition`.
pub const GRID_PARTITION: u32 = 0x0E2F_4444;
/// A record type: the CRC-32 of `GridCellDataBlock`.
pub const GRID_CELL_DATA_BLOCK: u32 = 0xAC2B_BF68;
/// The CRC-32 of `GridDescription`.
const GRID_DESCRIPTION: u32 = 0xAFE5_F9B4;
/// The CRC-32 of `GridLayout`.
const GRID_LAYOUT: u32 = 0x7DB0_83ED;
/// The CRC-32 of `GridCell`.
const GRID_CELL: u32 = 0x3435_AD40;

/// Why a grid body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("grid body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("grid {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("grid reference at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: reference::Error,
    },
    #[error("grid body: {0} bytes after its end")]
    Trailing(usize),
}

/// Little-endian fields read in order, each bounded.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let short = Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        };
        let end = self.at.checked_add(N).ok_or(Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        })?;
        let s: [u8; N] = self
            .b
            .get(self.at..end)
            .and_then(|s| s.try_into().ok())
            .ok_or(short)?;
        self.at = end;
        Ok(s)
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array(what)?))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    fn i32(&mut self, what: &'static str) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.array(what)?))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }
    /// A u32 count of items of `each` bytes the rest of the body can hold.
    fn count(&mut self, what: &'static str, each: usize) -> Result<usize, Error> {
        let at = self.at;
        let n = usize::try_from(self.u32(what)?).unwrap_or(usize::MAX);
        let left = self.b.len().saturating_sub(self.at);
        if n.checked_mul(each.max(1)).is_none_or(|bytes| bytes > left) {
            return Err(Error::Value {
                what,
                at,
                found: u64::try_from(n).unwrap_or(u64::MAX),
                read: "a count the body can hold",
            });
        }
        Ok(n)
    }
    fn expect(&mut self, what: &'static str, want: u64, width: usize) -> Result<(), Error> {
        let at = self.at;
        let found = match width {
            1 => u64::from(u8::from_le_bytes(self.array(what)?)),
            4 => u64::from(self.u32(what)?),
            _ => self.u64(what)?,
        };
        if found == want {
            Ok(())
        } else {
            Err(Error::Value {
                what,
                at,
                found,
                read: "the grammar's value",
            })
        }
    }
    /// An embedded object's header: a u64 id and its type, which must be `kind`.
    fn object(&mut self, what: &'static str, kind: u32) -> Result<u64, Error> {
        let id = self.u64(what)?;
        self.expect(what, u64::from(kind), 4)?;
        Ok(id)
    }
    fn reference(&mut self) -> Result<Option<Reference>, Error> {
        let at = self.at;
        reference::parse(self.array::<{ reference::LENGTH }>("reference")?)
            .map_err(|source| Error::Reference { at, source })
    }
    fn end(&self) -> Result<(), Error> {
        match self.b.len().saturating_sub(self.at) {
            0 => Ok(()),
            left => Err(Error::Trailing(left)),
        }
    }
}

/// A GridLayout as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridLayout {
    /// The u16 run before the five i32, as stored.
    pub words: Vec<u16>,
    /// The first i32 (CellSize in the AC schemas): a level-0 cell's side.
    pub cell_size: i32,
    /// The next three (BottomLeftX, Y, Z): the grid's corner.
    pub bottom_left: [i32; 3],
    /// The fifth (GridDimensionLevel0): level 0's cells along a side.
    pub dimension: i32,
    /// The 3 bytes after them, as stored.
    pub after: [u8; 3],
}

/// One of a World's GridDescriptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridDescription {
    pub id: u64,
    pub layout: GridLayout,
    pub partition: Option<Reference>,
}

/// A World's GridDescriptions, from the start of its body; `words` is the stated length of each GridLayout's u16 run.
pub fn grid_descriptions(body: &[u8], words: usize) -> Result<Vec<GridDescription>, Error> {
    let mut c = Cursor { b: body, at: 0 };
    c.expect("World first byte", 1, 1)?;
    let n = c.count("GridDescription count", 0x30)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let id = c.object("GridDescription", GRID_DESCRIPTION)?;
        c.expect("GridLayout id", 0, 8)?;
        c.expect("GridLayout type", u64::from(GRID_LAYOUT), 4)?;
        let words = (0..words)
            .map(|_| c.u16("GridLayout run"))
            .collect::<Result<Vec<_>, _>>()?;
        let cell_size = c.i32("GridLayout cell size")?;
        let bottom_left = [
            c.i32("GridLayout bottom left")?,
            c.i32("GridLayout bottom left")?,
            c.i32("GridLayout bottom left")?,
        ];
        let dimension = c.i32("GridLayout dimension")?;
        let after = c.array::<3>("GridLayout")?;
        let partition = c.reference()?;
        out.push(GridDescription {
            id,
            layout: GridLayout {
                words,
                cell_size,
                bottom_left,
                dimension,
                after,
            },
            partition,
        });
    }
    Ok(out)
}

/// A GridPartition as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredGridPartition {
    pub levels: u32,
    /// Each cell's u64: the ClassID of its GridCellDataBlock, or 0. A cell's number is its position here (the notes:
    /// each block's key equals its position on all 44,318 USNAT and 6 of 6 WhiteRoom cells holding one).
    pub cells: Vec<u64>,
}

impl StoredGridPartition {
    pub fn parse(body: &[u8]) -> Result<StoredGridPartition, Error> {
        let mut c = Cursor { b: body, at: 0 };
        c.expect("GridPartition first byte", 1, 1)?;
        let levels = c.u32("level count")?;
        let n = c.count("cell count", 21)?;
        let mut cells = Vec::with_capacity(n);
        for _ in 0..n {
            c.expect("GridCell id", 0, 8)?;
            c.expect("GridCell type", u64::from(GRID_CELL), 4)?;
            c.expect("GridCell byte", 0, 1)?;
            cells.push(c.u64("GridCell data block")?);
        }
        c.expect("GridPartition end", 0, 4)?;
        c.end()?;
        Ok(StoredGridPartition { levels, cells })
    }
}

/// A GridCellDataBlock as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredGridCellDataBlock {
    /// What it lists (Entities, EntityGroups, GIDynResources and others), in stored order.
    pub objects: Vec<Reference>,
    /// The u32 after them, as stored.
    pub word: u32,
    /// The u64 ending it: its cell's number, or a DataLayer's ClassID.
    pub key: u64,
}

impl StoredGridCellDataBlock {
    pub fn parse(body: &[u8]) -> Result<StoredGridCellDataBlock, Error> {
        let mut c = Cursor { b: body, at: 0 };
        c.expect("GridCellDataBlock first byte", 1, 1)?;
        let n = c.count("object count", reference::LENGTH)?;
        let mut objects = Vec::with_capacity(n);
        for _ in 0..n {
            let at = c.at;
            objects.push(c.reference()?.ok_or(Error::Value {
                what: "listed reference",
                at,
                found: 3,
                read: "a reference (tag 1)",
            })?);
        }
        let word = c.u32("word after the objects")?;
        let key = c.u64("key")?;
        c.end()?;
        Ok(StoredGridCellDataBlock { objects, word, key })
    }
}

/// A grid cell's place: its level (0 the finest), column and row, and its square in the grid's horizontal plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub level: u32,
    pub column: i64,
    pub row: i64,
    /// The corner with the least coordinates (the plane of the GridLayout's first two corner coordinates).
    pub min: [i64; 2],
    /// Its side.
    pub size: i64,
}

impl GridLayout {
    /// The cell numbered `k` of a partition with `levels` levels: level 0 (`dimension` cells a side of `cell_size`)
    /// first, then each coarser level (half the cells a side, twice the size), each row by row from the bottom-left
    /// corner. `None` past the last cell, or where a level's dimension or size doesn't hold.
    ///
    /// The numbering is the notes' best fit, not stored: of 4 orderings it is the one under which a listed entity's
    /// world bounds overlap its cell on 7,769 of 7,803 sampled USNAT listings (44 of 46 in WhiteRoom; the next best
    /// 264). That the cell count is the levels' squared dimensions summed is stored (21,845, 1,365 and 85).
    pub fn cell(&self, levels: u32, k: usize) -> Option<Cell> {
        let mut k = i64::try_from(k).ok()?;
        for level in 0..levels {
            let dim = i64::from(self.dimension).checked_shr(level)?;
            let size = i64::from(self.cell_size).checked_mul(1i64.checked_shl(level)?)?;
            let here = dim.checked_mul(dim)?;
            if dim < 1 || size < 1 {
                return None;
            }
            if k < here {
                let (column, row) = (k.checked_rem(dim)?, k.checked_div(dim)?);
                let [x, y, _] = self.bottom_left.map(i64::from);
                return Some(Cell {
                    level,
                    column,
                    row,
                    min: [
                        x.checked_add(column.checked_mul(size)?)?,
                        y.checked_add(row.checked_mul(size)?)?,
                    ],
                    size,
                });
            }
            k = k.checked_sub(here)?;
        }
        None
    }

    /// The cells of a partition with `levels` levels under [`GridLayout::cell`]'s numbering.
    pub fn cell_count(&self, levels: u32) -> Option<usize> {
        let mut total = 0usize;
        for level in 0..levels {
            let dim = usize::try_from(self.dimension).ok()?.checked_shr(level)?;
            total = total.checked_add(dim.checked_mul(dim)?)?;
        }
        Some(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WhiteRoom's layout (32, -128, -128, 0, 8): 8x8 + 4x4 + 2x2 + 1 cells.
    fn white_room() -> GridLayout {
        GridLayout {
            words: Vec::new(),
            cell_size: 32,
            bottom_left: [-128, -128, 0],
            dimension: 8,
            after: [0; 3],
        }
    }

    #[test]
    fn cells_number_level_0_first_row_by_row() {
        let g = white_room();
        assert_eq!(g.cell_count(4), Some(85));
        // the notes' cell 20: x [0, 32], y [-64, -32]
        assert_eq!(
            g.cell(4, 20),
            Some(Cell {
                level: 0,
                column: 4,
                row: 2,
                min: [0, -64],
                size: 32
            })
        );
        assert_eq!(
            g.cell(4, 64).map(|c| (c.level, c.min, c.size)),
            Some((1, [-128, -128], 64))
        );
        assert_eq!(
            g.cell(4, 84).map(|c| (c.level, c.min, c.size)),
            Some((3, [-128, -128], 256))
        );
        assert_eq!(g.cell(4, 85), None);
    }

    #[test]
    fn a_partition_and_a_block_read_and_others_refused() {
        let mut p = vec![1, 1, 0, 0, 0, 2, 0, 0, 0];
        for id in [0u64, 0x1F_4C36_9681] {
            p.extend([0; 8]);
            p.extend(GRID_CELL.to_le_bytes());
            p.push(0);
            p.extend(id.to_le_bytes());
        }
        p.extend([0; 4]);
        let read = StoredGridPartition::parse(&p).expect("partition");
        assert_eq!((read.levels, read.cells), (1, vec![0, 0x1F_4C36_9681]));
        p.push(0);
        assert!(matches!(StoredGridPartition::parse(&p), Err(Error::Trailing(1))));
        // the USNAT block Cell00001_DataBlock, whole
        let b = [
            1, 1, 0, 0, 0, 1, 1, 0x48, 0x44, 0xF0, 0x1B, 0xD4, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
        ];
        let read = StoredGridCellDataBlock::parse(&b).expect("block");
        assert_eq!(
            (
                read.objects.iter().map(|r| r.id).collect::<Vec<_>>(),
                read.word,
                read.key
            ),
            (vec![0x01D4_1BF0_4448], 1, 1)
        );
        assert!(StoredGridCellDataBlock::parse(&b[..26]).is_err());
    }
}
