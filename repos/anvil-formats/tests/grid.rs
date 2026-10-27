//! Real grids (Riders Republic build 25394987): the Worlds' GridDescriptions with the stated u16 run (5 in a base
//! forge, 4 in its patch), their GridPartitions and cells, and the blocks they name, as the Riders Republic notes give
//! them (docs/placement.md §10, docs/world.md §1).
mod inputs;

use anvil_formats::archive::{Archive, At};
use anvil_formats::grid::{
    self, Cell, GRID_CELL_DATA_BLOCK, GRID_PARTITION, StoredGridCellDataBlock, StoredGridPartition, WORLD,
};
use inputs::{Game, install};

/// The first entry of type `kind` in the archive's forge `forge`.
fn first(archive: &Archive, forge: usize, kind: u32) -> At {
    let names = &archive.forge(forge).expect("forge").names;
    let entry = names
        .iter()
        .position(|n| n.type_id == kind)
        .expect("an entry of the type");
    At { forge, entry }
}

#[test]
fn white_room_world_partition_and_blocks() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&[
        dir.join("DataPC_MAD_WhiteRoom.forge"),
        dir.join("DataPC_MAD_WhiteRoom_patch_01.forge"),
    ])
    .expect("archive");
    let base = archive.resource(first(&archive, 0, WORLD)).expect("world");
    let patch = archive.resource(first(&archive, 1, WORLD)).expect("world");
    // the run's length is stated; the other one is refused
    assert!(grid::grid_descriptions(&base.body, 4).is_err());
    assert!(grid::grid_descriptions(&patch.body, 5).is_err());
    let grids = grid::grid_descriptions(&base.body, 5).expect("base grids");
    assert_eq!(
        grid::grid_descriptions(&patch.body, 4)
            .expect("patch grids")
            .len(),
        3
    );
    assert_eq!(grids.len(), 3);
    let l = &grids[0].layout;
    assert_eq!(
        (l.cell_size, l.bottom_left, l.dimension),
        (32, [-128, -128, 0], 8)
    );
    assert!(grids[1].partition.is_none() && grids[2].partition.is_none());
    let partition = grids[0].partition.expect("a partition").id;
    let at = archive.find(partition).expect("in the archive");
    let r = archive.resource(at).expect("partition");
    assert_eq!(r.type_id, GRID_PARTITION);
    let p = StoredGridPartition::parse(&r.body).expect("partition");
    assert_eq!(
        (p.levels, p.cells.len(), l.cell_count(p.levels)),
        (4, 85, Some(85))
    );
    let holding: Vec<usize> = (0..p.cells.len()).filter(|&k| p.cells[k] != 0).collect();
    assert_eq!(holding, vec![20, 27, 28, 35, 36, 84]);
    // each block's key is its cell's number
    for &k in &holding {
        let at = archive.find(p.cells[k]).expect("block");
        let r = archive.resource(at).expect("block");
        assert_eq!(r.type_id, GRID_CELL_DATA_BLOCK);
        let b = StoredGridCellDataBlock::parse(&r.body).expect("block");
        assert_eq!(b.key, u64::try_from(k).expect("cell"), "cell {k}");
    }
    // cell 20 is x [0, 32], y [-64, -32]
    assert_eq!(
        l.cell(p.levels, 20),
        Some(Cell {
            level: 0,
            column: 4,
            row: 2,
            min: [0, -64],
            size: 32
        })
    );
}

#[test]
fn usnat_layouts_and_first_cell() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&[dir.join("DataPC_MAD_USNAT.forge")]).expect("archive");
    let world = archive.resource(first(&archive, 0, WORLD)).expect("world");
    let grids = grid::grid_descriptions(&world.body, 5).expect("grids");
    let layouts: Vec<(i32, [i32; 3], i32)> = grids
        .iter()
        .map(|g| (g.layout.cell_size, g.layout.bottom_left, g.layout.dimension))
        .collect();
    assert_eq!(
        layouts,
        vec![
            (128, [-8192, -8192, 0], 128),
            (128, [-8192, -8192, 0], 128),
            (512, [-8192, -8192, 0], 32)
        ]
    );
    let mut cells = Vec::new();
    for g in &grids {
        let at = archive
            .find(g.partition.expect("a partition").id)
            .expect("partition");
        let p =
            StoredGridPartition::parse(&archive.resource(at).expect("partition").body).expect("partition");
        assert_eq!(g.layout.cell_count(p.levels), Some(p.cells.len()));
        cells.push((p.levels, p.cells.len()));
        if cells.len() == 1 {
            // GRN_Template_TerrainWorldSmall's cell 0: Cell00000_DataBlock, the grid's bottom-left 128 m
            let at = archive.find(p.cells[0]).expect("block");
            let b =
                StoredGridCellDataBlock::parse(&archive.resource(at).expect("block").body).expect("block");
            assert_eq!(b.key, 0);
            assert_eq!(
                g.layout.cell(p.levels, 0).map(|c| (c.min, c.size)),
                Some(([-8192, -8192], 128))
            );
        }
    }
    assert_eq!(cells, vec![(8, 21_845), (8, 21_845), (6, 1_365)]);
}
