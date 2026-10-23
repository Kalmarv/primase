//! The grass system on Riders Republic's forges (`installs.riders`): the setup Entity of the whole-world cell, its 46
//! models and its library, and the terrain nodes' group rasters, as the notes count them (docs/grass.md §2, §3).
use std::collections::BTreeMap;

use primase_anvil::components::{AnvilGrass, AnvilGrassModel, AnvilMesh, AnvilTerrain, AnvilTerrainGroups};
use primase_anvil::read::convert::{
    AnvilOptions, AnvilReader, CellSelection, Edition, ForgeFile, GridSelection, TerrainSelection,
};
use primase_anvil::read::placements::{InstancedCollision, InstancedLevels};
use primase_core::convert::Reader;
use primase_core::frame::SourceFrame;
use primase_core::{Instance, World};
use primase_testdata::{Game, install};

fn usnat(
    dir: &std::path::Path,
    grid: &str,
    cells: Vec<usize>,
    terrain: Option<TerrainSelection>,
) -> AnvilOptions {
    AnvilOptions {
        forges: [
            ("DataPC_MAD_USNAT_patch_01.forge", Edition::Patch),
            ("DataPC_MAD_USNAT.forge", Edition::Base),
        ]
        .into_iter()
        .map(|(name, edition)| ForgeFile {
            path: dir.join(name),
            edition,
        })
        .collect(),
        frame: SourceFrame::ENU,
        grids: vec![GridSelection {
            grid: grid.into(),
            cells: CellSelection::Numbered(cells),
        }],
        instanced_levels: InstancedLevels::Every,
        instanced_collision: InstancedCollision::PerInstance,
        terrain,
    }
}

/// The whole-world cell 21844 lists the grass group in one of the grids: its setup (the GrassComponent and its library
/// of 46 models and 190 groups with 218 rules) and the 46 GrassVisual models, each a placement of its Mesh marked with
/// its number.
#[test]
fn the_whole_world_cell_holds_the_setup_and_the_46_models() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut found: Vec<(String, World)> = Vec::new();
    for grid in ["ShortRange", "GRN_Template_TerrainWorldSmall", "LongRange"] {
        let Ok(mut reader) = AnvilReader::open(&usnat(&dir, grid, vec![21844], None)) else {
            continue;
        };
        let Ok((world, _)) = reader.read() else {
            continue;
        };
        if world.iter::<AnvilGrass>().next().is_some() {
            found.push((grid.to_owned(), world));
        }
    }
    assert_eq!(
        found.len(),
        1,
        "grids holding the setup: {:?}",
        found.iter().map(|f| &f.0).collect::<Vec<_>>()
    );
    let world = &found[0].1;
    let setups: Vec<_> = world.iter::<AnvilGrass>().collect();
    assert_eq!(setups.len(), 1);
    let grass = setups[0].1;
    assert_eq!(grass.setup.entities.len(), 46);
    assert_eq!(grass.setup.visibility_distance, Some(250));
    assert_eq!(grass.library.models.len(), 46);
    assert_eq!(grass.library.groups.len(), 190);
    assert_eq!(
        grass.library.groups.iter().map(|g| g.rules.len()).sum::<usize>(),
        218
    );
    // the 46 models, once each, each a placement of a Mesh
    let mut models: BTreeMap<u8, usize> = BTreeMap::new();
    for (e, m) in world.iter::<AnvilGrassModel>() {
        *models.entry(m.model).or_default() += 1;
        let placed = world.get::<Instance>(e).expect("a placement");
        assert!(
            world.get::<AnvilMesh>(placed.prototype).is_some(),
            "model {} has no mesh",
            m.model
        );
    }
    assert_eq!(models.len(), 46);
    assert!(models.keys().copied().eq(0..46));
    assert!(models.values().all(|&n| n == 1));
}

/// The CITYSCHOOL cell 9380 (128 m square: its four level-8 nodes, 65,536 samples) has 345 samples with a group: group
/// 60 on 194, group 68 on 145 and group 65 on 6 (docs/grass.md §3); the rest of the cell has none (254).
#[test]
fn the_cityschool_cell_has_the_notes_group_samples() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let terrain = TerrainSelection {
        origin: [-8192.0, -8192.0],
        side: 16_384.0,
        level: 8,
        // inside the cell's square, so the four nodes under it and none beside them
        rect: [-3583.0, 1153.0, -3457.0, 1279.0],
        layer: None,
        bank: None,
        noise: None,
        groups: true,
    };
    let mut reader = AnvilReader::open(&usnat(&dir, "ShortRange", vec![9380], Some(terrain))).unwrap();
    let (world, _) = reader.read().unwrap();
    let (e, groups) = world.iter::<AnvilTerrainGroups>().next().expect("terrain groups");
    assert_eq!(groups.tiles.len(), 4);
    assert_eq!(world.get::<AnvilTerrain>(e).map(|t| t.tiles.len()), Some(4));
    let mut tally: BTreeMap<u8, usize> = BTreeMap::new();
    let mut samples = 0usize;
    for tile in &groups.tiles {
        assert_eq!((tile.group.len(), tile.weight.len()), (132 * 132, 132 * 132));
        // the node's core: its 128 x 128 from sample (2, 2)
        for i in 2..130 {
            for j in 2..130 {
                samples += 1;
                let g = tile.group[i * 132 + j];
                if g != 254 {
                    *tally.entry(g).or_default() += 1;
                }
            }
        }
    }
    assert_eq!(samples, 65_536);
    assert_eq!(tally, BTreeMap::from([(60, 194), (65, 6), (68, 145)]));
}
