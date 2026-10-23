//! The grid overview ([`primase_anvil::read::overview`]) on Riders Republic's forges (`PRIMASE_ANVIL_DIR`): every
//! grid's layout and cells from the tables, the block counts as the notes and the reader's own listings give them.
use primase_anvil::read::convert::{
    AnvilOptions, AnvilReader, CellSelection, Edition, ForgeFile, GridSelection,
};
use primase_anvil::read::overview::{Counts, grids_of};
use primase_anvil::read::placements::{InstancedCollision, InstancedLevels};
use primase_core::frame::SourceFrame;
use primase_testdata::real_data;

fn forges(dir: &std::path::Path, files: [(&str, Edition); 2]) -> Vec<ForgeFile> {
    files
        .into_iter()
        .map(|(name, edition)| ForgeFile {
            path: dir.join(name),
            edition,
        })
        .collect()
}

fn usnat(dir: &std::path::Path) -> Vec<ForgeFile> {
    forges(
        dir,
        [
            ("DataPC_MAD_USNAT_patch_01.forge", Edition::Patch),
            ("DataPC_MAD_USNAT.forge", Edition::Base),
        ],
    )
}

#[test]
fn white_room_grids_have_the_notes_cells_and_counts() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let forges = forges(
        &dir,
        [
            ("DataPC_MAD_WhiteRoom_patch_01.forge", Edition::Patch),
            ("DataPC_MAD_WhiteRoom.forge", Edition::Base),
        ],
    );
    let skipped = grids_of(&forges, Counts::Skip).unwrap();
    let read = grids_of(&forges, Counts::Read).unwrap();
    assert_eq!(skipped.len(), read.len());
    assert!(
        skipped
            .iter()
            .flat_map(|g| &g.cells)
            .all(|c| c.object_count.is_none())
    );
    let room = read
        .iter()
        .find(|g| g.name == "MAD_WhiteRoom")
        .expect("MAD_WhiteRoom");
    // 8 x 8 + 4 x 4 + 2 x 2 + 1 cells of (32, -128, -128, 8): the notes' layout
    assert_eq!(
        (room.layout.cell_size, room.layout.dimension, room.levels),
        (32, 8, 4)
    );
    assert_eq!(room.cells.len(), 85);
    assert_eq!(room.cells.iter().filter(|c| c.level == 0).count(), 64);
    // the notes' cells: 20, 27, 28, 35 and 36 list 9, 5, 4, 8 and 24 objects
    for (cell, count) in [(20, 9), (27, 5), (28, 4), (35, 8), (36, 24)] {
        let c = room.cells[cell];
        assert_eq!(
            (c.index, c.level, c.has_block, c.object_count),
            (cell, 0, true, Some(count))
        );
    }
    assert_eq!(room.cells[20].min, [0, -64]);
    assert_eq!(room.cells[20].size, 32);
    // a count is Some exactly where the reader would list: none without a block reads 0
    assert!(
        room.cells
            .iter()
            .all(|c| c.has_block || c.object_count == Some(0))
    );
}

#[test]
fn usnat_grids_agree_with_the_readers_listing() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let forges = usnat(&dir);
    let overview = grids_of(&forges, Counts::Read).unwrap();
    assert!(overview.len() >= 2);
    for g in &overview {
        println!(
            "grid {} ({}): {} levels, cells {} with a block {}, objects {}",
            g.name,
            g.entry,
            g.levels,
            g.cells.len(),
            g.cells.iter().filter(|c| c.has_block).count(),
            g.cells
                .iter()
                .filter_map(|c| c.object_count)
                .map(u64::from)
                .sum::<u64>()
        );
        assert_eq!(Some(g.cells.len()), g.layout.cell_count(g.levels));
    }
    // each grid's cells with a block and objects listed, as the tables give them (the notes: 44,318 USNAT cells hold a
    // block)
    let tally: Vec<(&str, usize, usize, u64)> = overview
        .iter()
        .map(|g| {
            (
                g.name.as_str(),
                g.cells.len(),
                g.cells.iter().filter(|c| c.has_block).count(),
                g.cells.iter().filter_map(|c| c.object_count).map(u64::from).sum(),
            )
        })
        .collect();
    assert_eq!(
        tally,
        [
            ("GRN_Template_TerrainWorldSmall", 21_845, 21_145, 238_499),
            ("ShortRange", 21_845, 21_818, 379_861),
            ("LongRange", 1_365, 1_355, 26_131),
        ]
    );
    assert_eq!(tally.iter().map(|t| t.2).sum::<usize>(), 44_318);
    // the cells the reader's own listing counts for cityschool: its notice names each cell's object count
    let ground = overview
        .iter()
        .find(|g| g.name == "GRN_Template_TerrainWorldSmall")
        .expect("GRN_Template_TerrainWorldSmall");
    let short = overview
        .iter()
        .find(|g| g.name == "ShortRange")
        .expect("ShortRange");
    let options = AnvilOptions {
        forges: forges.clone(),
        frame: SourceFrame::ENU,
        grids: ["ShortRange", "GRN_Template_TerrainWorldSmall"]
            .map(|grid| GridSelection {
                grid: grid.into(),
                cells: CellSelection::Numbered(vec![9380]),
            })
            .into(),
        instanced_levels: InstancedLevels::Every,
        instanced_collision: InstancedCollision::PerInstance,
        terrain: None,
    };
    let mut reader = AnvilReader::open(&options).unwrap();
    let from_reader = reader.grids(Counts::Read).unwrap();
    assert_eq!(from_reader, overview);
    let (_, lines) = primase_core::convert::Reader::read(&mut reader).unwrap();
    let cells = &lines
        .iter()
        .find(|(l, _)| l == "anvil cells")
        .expect("anvil cells")
        .1;
    assert_eq!(
        cells,
        &format!(
            "ShortRange cell 9380 (level 0): {} objects; GRN_Template_TerrainWorldSmall cell 9380 (level 0): {} objects",
            short.cells[9380].object_count.unwrap(),
            ground.cells[9380].object_count.unwrap()
        )
    );
}
