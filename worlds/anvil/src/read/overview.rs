//! What a world's grids hold, cell by cell, from the grid tables alone: each grid's layout and levels and every cell's
//! square, whether it has a GridCellDataBlock and, when asked, how many objects the block lists. A GridCellDataBlock is
//! read only for its count and only when [`Counts::Read`] says so; without it the overview costs the World and the
//! partitions, not the 15 GB of the forge.
use primase_core::convert::Result;

use crate::read::convert::{ForgeFile, entry_name, world_descriptions};
use anvil_formats::archive::Archive;
use anvil_formats::grid::{GridLayout, StoredGridCellDataBlock, StoredGridPartition};
use anvil_formats::listing::Kind;

/// Whether an overview reads each cell's block for the number of objects it lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counts {
    /// No block is read: [`CellOverview::object_count`] is `None`.
    Skip,
    /// Every cell's block is read: `Some(n)`, `0` for a cell without one.
    Read,
}

/// One cell of a grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellOverview {
    /// The cell's number: its position in the GridPartition.
    pub index: usize,
    /// 0 the finest; each coarser level has squares twice the side.
    pub level: u32,
    /// The square's corner with the least coordinates, in the grid's units (metres; the game's x east, y north).
    pub min: [i64; 2],
    /// The square's side.
    pub size: i64,
    /// The partition names a GridCellDataBlock for the cell.
    pub has_block: bool,
    /// The objects its block lists, where [`Counts::Read`] asked.
    pub object_count: Option<u32>,
}

/// One grid of the World.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridOverview {
    /// The GridPartition's name: the part of its entry's name after the last `" - "`, as a [`GridSelection`] names it.
    ///
    /// [`GridSelection`]: crate::read::convert::GridSelection
    pub name: String,
    /// The GridPartition entry's name as the forge stores it.
    pub entry: String,
    pub layout: GridLayout,
    pub levels: u32,
    /// Every cell, in the partition's order.
    pub cells: Vec<CellOverview>,
}

/// The World's grids that have a GridPartition, in the World's order, with every cell of each. The first World in
/// `forges`' order answers, and each forge's edition says how its layouts read, as for [`AnvilReader`].
///
/// [`AnvilReader`]: crate::read::convert::AnvilReader
pub fn grids(archive: &mut Archive, forges: &[ForgeFile], counts: Counts) -> Result<Vec<GridOverview>> {
    let mut out = Vec::new();
    for d in world_descriptions(archive, forges)? {
        let Some(partition) = d.partition else {
            continue;
        };
        let Some(at) = archive.find(partition.id) else {
            continue;
        };
        let entry = entry_name(archive, at);
        let name = entry.rsplit(" - ").next().unwrap_or(&entry).to_owned();
        let r = archive.resource(at)?;
        if r.type_id != Kind::GridPartition.type_id() {
            return Err(format!("{entry}: type {:#010x}, not a GridPartition", r.type_id).into());
        }
        let p = StoredGridPartition::parse(&r.body).map_err(|e| format!("{entry}: {e}"))?;
        let mut cells = Vec::with_capacity(p.cells.len());
        for (index, &block) in p.cells.iter().enumerate() {
            let cell = d
                .layout
                .cell(p.levels, index)
                .ok_or_else(|| format!("{entry}: no cell {index} of {}", p.cells.len()))?;
            let object_count = match (counts, block) {
                (Counts::Skip, _) => None,
                (Counts::Read, 0) => Some(0),
                (Counts::Read, block) => {
                    let at = archive
                        .find(block)
                        .ok_or_else(|| format!("{entry} cell {index}: block {block:#x} in no forge"))?;
                    let b = archive.resource(at)?;
                    if b.type_id != Kind::GridCellDataBlock.type_id() {
                        return Err(format!(
                            "{entry} cell {index}: block {block:#x}: type {:#010x}",
                            b.type_id
                        )
                        .into());
                    }
                    let listing = StoredGridCellDataBlock::parse(&b.body)
                        .map_err(|e| format!("{entry} cell {index}: {e}"))?;
                    Some(u32::try_from(listing.objects.len()).unwrap_or(u32::MAX))
                }
            };
            cells.push(CellOverview {
                index,
                level: cell.level,
                min: cell.min,
                size: cell.size,
                has_block: block != 0,
                object_count,
            });
        }
        out.push(GridOverview {
            name,
            entry,
            layout: d.layout,
            levels: p.levels,
            cells,
        });
    }
    Ok(out)
}

/// [`grids`] over the forges at `forges`' paths, opened for it.
pub fn grids_of(forges: &[ForgeFile], counts: Counts) -> Result<Vec<GridOverview>> {
    let paths: Vec<&std::path::PathBuf> = forges.iter().map(|f| &f.path).collect();
    grids(&mut Archive::open(&paths)?, forges, counts)
}
