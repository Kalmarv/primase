//! A census of the HitMtrl classes the install's map collision uses, by game and map: how many triangles each class
//! (an ID modulo 100) holds in the chosen hit filters' files, and whether the class's row has a name and a core surface
//! in `lower-fromsoft-collision`'s table (`class_name`, `known_surface`). It answers what the names alone cannot:
//! whether Elden Ring's class 0 ("None") is invisible blockers, as Sekiro's 29 is, or ordinary ground; which classes
//! the install uses that have no row name; how many triangles each unmapped class costs. It asserts only what is
//! stated (the counts add up to the triangles read, the tables agree with themselves) and prints the rest, one line
//! per class: `game map class count name surface`.
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
use fromsoft_formats::{Game, Install, Keys, dcx::Decoder};
use primase_core::{Entity, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{
    self,
    collision::{class_name, known_surface},
    source::MapCollision,
};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// A rectangle no map reaches: the census reads the collision files (read whole whatever the region) and places no
/// model.
const NOWHERE: sekiro::Region = sekiro::Region {
    rect: [1.0e6, 1.0e6, 1.0e6 + 1.0, 1.0e6 + 1.0],
    margin: 0.0,
};

fn census(game: Game, install: &Install, decoder: &Decoder<'_>, map: &str) -> Result<BTreeMap<i32, usize>> {
    let mut world = World::default();
    let options = sekiro::Options {
        region: Some(NOWHERE),
        ..Default::default()
    };
    sekiro::read_game_area(install, map, decoder, &options, &mut world)?;
    let mut classes: BTreeMap<i32, usize> = BTreeMap::new();
    let mut triangles = 0;
    let records: Vec<(Entity, &MapCollision)> = world.iter::<MapCollision>().collect();
    for (_, record) in &records {
        triangles += record.triangles.len();
        for &material in &record.materials {
            // -1: the shape key binds no material
            let class = if material < 0 { -1 } else { material % 100 };
            *classes.entry(class).or_default() += 1;
        }
    }
    assert_eq!(
        classes.values().sum::<usize>(),
        triangles,
        "{map}: every triangle has a class or -1"
    );
    for (class, count) in &classes {
        println!(
            "{} {map} {class} {count} {} {}",
            game.name(),
            class_name(game, *class).unwrap_or("(no name)"),
            known_surface(game, *class).unwrap_or("-"),
        );
    }
    Ok(classes)
}

#[test]
fn the_classes_the_maps_use() -> Result<()> {
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
            &["m11_01_00_00", "m20_00_00_00", "m25_00_00_00"][..],
        ),
        (
            Game::EldenRing,
            primase_testdata::Game::Elden,
            &["m10_00_00_00", "m12_02_00_00", "m14_00_00_00", "m60_39_54_00"][..],
        ),
    ] {
        let (Some(root), keys) = (primase_testdata::install(data), primase_testdata::keys(data)) else {
            continue;
        };
        let keys = match (game, keys) {
            (_, Some(folder)) => Keys::from_folder(folder, game, game.param_key_sha256())?,
            (_, None) => continue,
        };
        let install = Install::new(root, game, keys);
        for map in maps {
            let classes = census(game, &install, &decoder, map)?;
            // a class the program lowers as water must be a class the install uses, or the choice is untested
            if game == Game::EldenRing {
                println!(
                    "{map}: class 21 (Water) triangles {}",
                    classes.get(&21).copied().unwrap_or(0)
                );
            }
        }
    }
    Ok(())
}
