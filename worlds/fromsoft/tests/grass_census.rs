//! A census of the grass Elden Ring's map parts name: for each map, the parts that store a grass config, the
//! `GrassTypeParam` rows the configs name (six ids each) and, resolved against `regulation.bin`, each row's models and
//! textures. It checks the param layout the reader assumes (`param::GrassParam`: rows of at least 276 bytes whose name
//! fields are printable text) against the install, and prints what the notices summarise:
//!
//! - `layout <map> data_version <v> row_size <n>` (or `unresolved <why>`);
//! - `parts <map> <n>`;
//! - `row <map> <id> parts <n> model <name> model1 <name> simple <name> flat <texture> billboard <texture> density <d>
//!   spacing <s> lod_range <r>`; `missing <map> <id>` for a row the param does not hold.
//!
//! Runs where the install, its keys (regulation.key) and the Oodle library are configured; otherwise it does nothing.
use fromsoft_formats::{Game, Install, Keys, dcx::Decoder};
use primase_core::World;
use primase_fromsoft::sekiro;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// A rectangle no map reaches: the census reads the layout and places no model.
const NOWHERE: sekiro::Region = sekiro::Region {
    rect: [1.0e6, 1.0e6, 1.0e6 + 1.0, 1.0e6 + 1.0],
    margin: 0.0,
};

#[test]
fn the_grass_the_maps_name() -> Result<()> {
    let Some(dll) = primase_testdata::oodle(primase_testdata::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let data = primase_testdata::Game::Elden;
    let (Some(root), Some(folder)) = (primase_testdata::install(data), primase_testdata::keys(data)) else {
        return Ok(());
    };
    let install = Install::new(root, Game::EldenRing, Keys::from_folder(folder, Game::EldenRing)?);
    for map in ["m10_00_00_00", "m12_02_00_00", "m60_39_54_00"] {
        let mut world = World::default();
        let options = sekiro::Options {
            region: Some(NOWHERE),
            ..Default::default()
        };
        let report = sekiro::read_game_area(&install, map, &decoder, &options, &mut world)?;
        let grass = &report.grass;
        match (&grass.layout, &grass.unresolved) {
            (Some((version, size)), _) => println!("layout {map} data_version {version} row_size {size}"),
            (None, Some(why)) => println!("unresolved {map} {why}"),
            (None, None) => {}
        }
        println!("parts {map} {} unset_slots {}", grass.parts, grass.unset);
        assert_eq!(grass.parts, report.grass_parts);
        for (id, parts) in &grass.named {
            match grass.types.get(id) {
                Some(t) => println!(
                    "row {map} {id} parts {parts} model {:?} model1 {:?} simple {:?} flat {:?} billboard {:?} density {} \
                     spacing {} lod_range {}",
                    t.model0,
                    t.model1,
                    t.simple_model,
                    t.flat_texture,
                    t.billboard_texture,
                    t.base_density,
                    t.spacing,
                    t.lod_range
                ),
                None => println!("missing {map} {id} parts {parts}"),
            }
        }
    }
    Ok(())
}
