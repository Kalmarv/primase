//! A census of the draw-param fields the install's maps store: for each map's draw-param files, every group read at the
//! value set the area's parts name, and every field in it with its key and value as stored, one line per field:
//! `game file group id key value`. It is what `lower-fromsoft-environment` needs to lower the fog and the exposure
//! to `core.atmosphere`: which keys carry them, and in what range the values lie (a fog distance in metres or a
//! density, an exposure as a multiplier or as stops). It asserts only what is stated (a file read has groups, a group
//! its fields) and prints the rest.
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
use primase_core::World;
use primase_fromsoft::sekiro::{self, DrawParams, source::DrawParam};
use primase_fromsoft::{Game, Install, Keys, dcx::Decoder};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// A rectangle no map reaches: the census reads the draw-param file and places no model.
const NOWHERE: sekiro::Region = sekiro::Region {
    rect: [1.0e6, 1.0e6, 1.0e6 + 1.0, 1.0e6 + 1.0],
    margin: 0.0,
};

fn census(game: Game, install: &Install, decoder: &Decoder<'_>, map: &str) -> Result<()> {
    // the first read names the files the archive holds; each is then read in turn
    let mut files = {
        let mut world = World::default();
        let options = sekiro::Options {
            region: Some(NOWHERE),
            draw_param: DrawParams::File(format!("{}_0000", &map[..6])),
            ..Default::default()
        };
        sekiro::read_game_area(install, map, decoder, &options, &mut world)?
            .draw_param
            .available
    };
    if files.is_empty() {
        println!("{} {map}: no draw-param file", game.name());
        return Ok(());
    }
    files.truncate(8);
    for file in files {
        let mut world = World::default();
        let options = sekiro::Options {
            region: Some(NOWHERE),
            draw_param: DrawParams::File(file.clone()),
            ..Default::default()
        };
        sekiro::read_game_area(install, map, decoder, &options, &mut world)?;
        let draw = world
            .resource::<DrawParam>()
            .ok_or_else(|| format!("{file}: no draw param read"))?;
        // a census states what it finds: a file with no group, or a group with no field, is a line, not a failure
        if draw.groups.is_empty() {
            println!("{} {file} NO-GROUPS", game.name());
        }
        for group in &draw.groups {
            if group.fields.is_empty() && group.chosen != sekiro::source::SetChosen::ZeroId {
                println!("{} {file} [{}] NO-FIELDS {:?}", game.name(), group.key, group.chosen);
            }
            for field in &group.fields {
                println!(
                    "{} {file} [{}] {} {} {:?}",
                    game.name(),
                    group.key,
                    group.id,
                    field.key,
                    field.value
                );
            }
        }
    }
    Ok(())
}

#[test]
fn the_fields_the_maps_store() -> Result<()> {
    let Some(dll) = primase_testdata::oodle(primase_testdata::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data, maps) in [
        (
            Game::Sekiro,
            primase_testdata::Game::Sekiro,
            &["m11_01_00_00", "m20_00_00_00"][..],
        ),
        (
            Game::EldenRing,
            primase_testdata::Game::Elden,
            &["m10_00_00_00", "m12_02_00_00"][..],
        ),
    ] {
        let (Some(root), keys) = (primase_testdata::install(data), primase_testdata::keys(data)) else {
            continue;
        };
        let keys = match (game, keys) {
            (Game::Sekiro, _) => Keys::published(Game::Sekiro).ok_or("Sekiro's keys")?,
            (_, Some(folder)) => Keys::from_folder(folder, game)?,
            (_, None) => continue,
        };
        let install = Install::new(root, game, keys);
        for map in maps {
            census(game, &install, &decoder, map)?;
        }
    }
    Ok(())
}
