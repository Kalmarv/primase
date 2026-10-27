//! A census of the shader families the install's map materials use, by the class the reader's table gives them
//! (`water`, `decal`, `leaves`, `alpha`, `additive`, `cutout`, `opaque`, or skipped): every shader name of a kept
//! material slot, its class and the slots it names, per map. It answers what the tables' patterns cannot: which
//! shader names the game uses for water and decals that a pattern misses (`M[Flow]_Water`, a river's
//! `M[ARSN]_Water_3normaltet`: a name containing `Water` or `Decal` that is not classed so is listed with `!`), and
//! whether any name classed as water or decal is not one. It asserts only that every slot is counted once and prints
//! the rest, one line per shader: `game map class slots shader`.
//!
//! Maps of more than 1500 parts are read at the 100 m square the repository's other area tests use for Sekiro's
//! m11_01 and skipped otherwise (a whole castle is more than a test reads). Runs where the install, its keys and the
//! Oodle library are configured; otherwise it does nothing.
mod inputs;

use primase_core::World;
use primase_fromsoft::msb::MapLayout;
use primase_fromsoft::{Game, Install, Keys, dcx::Decoder, sekiro};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

const WHOLE_AREA_MAX_PARTS: usize = 1500;
/// The courtyard of Ashina Castle (m11_01).
const COURTYARD: [f32; 4] = [-150.0, 200.0, -50.0, 300.0];

fn census(game: Game, install: &Install, decoder: &Decoder<'_>, map: &str) -> Result<()> {
    let archive = install.open()?;
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{map}.msb.dcx"), decoder)?)?;
    let region = if layout.parts.len() <= WHOLE_AREA_MAX_PARTS {
        None
    } else if map == "m11_01_00_00" {
        Some(sekiro::Region {
            rect: COURTYARD,
            margin: 30.0,
        })
    } else {
        println!(
            "{} {map}: {} parts, skipped (more than a read keeps whole)",
            game.name(),
            layout.parts.len()
        );
        return Ok(());
    };
    let options = sekiro::Options {
        region,
        ..Default::default()
    };
    let mut world = World::default();
    let report = sekiro::read_game_area(install, map, decoder, &options, &mut world)?;
    // class of each shader: the materials the read kept, by shader
    let mut classes: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (_, m) in world.iter::<primase_fromsoft::materials::Material>() {
        let class = match &m.skip {
            Some(skip) => format!("skip:{skip:?}").to_lowercase(),
            None => m.class.clone(),
        };
        *classes.entry((class, m.shader.clone())).or_default() += 1;
    }
    for ((class, shader), slots) in &classes {
        let named_water = shader.contains("Water") || shader.contains("Waterfall");
        let named_decal = shader.contains("Decal");
        let missed = (named_water && class != "water") || (named_decal && class != "decal");
        println!(
            "{} {map} {class} {slots} {}{shader}",
            game.name(),
            if missed { "! " } else { "" }
        );
    }
    println!(
        "{} {map}: classes of slots {:?}, {} object placements, {} grass parts",
        game.name(),
        report.material_classes,
        report.object_placements,
        report.grass_parts
    );
    assert_eq!(
        classes.values().sum::<usize>(),
        report.materials,
        "{map}: every material is counted once"
    );
    Ok(())
}

#[test]
fn the_shader_families_the_maps_use() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
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
            inputs::Game::Sekiro,
            &[
                "m11_01_00_00",
                "m20_00_00_00",
                "m25_00_00_00",
                "m17_00_00_00",
                "m10_00_00_00",
            ][..],
        ),
        (
            Game::EldenRing,
            inputs::Game::Elden,
            &["m10_00_00_00", "m12_02_00_00", "m14_00_00_00", "m31_12_00_00"][..],
        ),
    ] {
        let (Some(root), keys) = (inputs::install(data), inputs::keys(data)) else {
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
