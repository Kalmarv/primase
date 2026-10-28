//! The draw-param file one collision part's scene config names ([`sekiro::DrawParams::Collision`]), read as the area reader
//! reads a file ([`sekiro::read_area_draw_param`]). Expected values come from an independent census of the layouts' scene
//! configs (a separate parser of the part records, run for this test):
//!
//! - Elden Ring's `m10_00_00_00` (Stormveil): 111 collisions naming the ids 0 (61), 100 (33), 400 (10), 500 (1), 600 (4), 810 (1)
//!   and 900 (1). The map's files are 0000 to 0900 in hundreds but 0700; `h014000` names 0500, which exists, and `h014500`
//!   names 0810, which does not.
//! - Sekiro's `m11_01_00_00` (Ashina Castle): 221 collisions naming 0 (165), 100 (29), 400 (2) and 700 (25); `h008700` names
//!   0700 and `h006200` names 0400, both files of the map.
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{self, DrawParams};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

#[test]
fn a_collisions_scene_config_names_the_file_read() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data, map, family, ids, asked) in [
        (
            Game::EldenRing,
            inputs::Game::Elden,
            "m10_00_00_00",
            "m10_00",
            BTreeMap::from([
                (0, 61),
                (100, 33),
                (400, 10),
                (500, 1),
                (600, 4),
                (810, 1),
                (900, 1),
            ]),
            // (collision, the id it names, whether the map has that file)
            [("h014000", 500, true), ("h014500", 810, false)],
        ),
        (
            Game::Sekiro,
            inputs::Game::Sekiro,
            "m11_01_00_00",
            "m11_01",
            BTreeMap::from([(0, 165), (100, 29), (400, 2), (700, 25)]),
            [("h008700", 700, true), ("h006200", 400, true)],
        ),
    ] {
        let (Some(root), Some(folder)) = (inputs::install(data), inputs::keys(data)) else {
            continue;
        };
        let keys = Keys::from_folder(folder, game, game.param_key_sha256())?;
        let archive = common::install(root, game, keys).open()?;
        for (collision, id, held) in asked {
            let (draw, report) = sekiro::read_area_draw_param(
                &archive,
                &decoder,
                map,
                &DrawParams::Collision(collision.into()),
            )?;
            // every collision of the map is counted under the id it names
            assert_eq!(report.collision_ids, ids, "{map}");
            let choice = report.collision.as_ref().expect("the collision's choice");
            assert_eq!((choice.collision.as_str(), choice.id), (collision, id), "{map}");
            if held {
                let stem = format!("{family}_{id:04}");
                assert_eq!(choice.file.as_deref(), Some(stem.as_str()), "{map} {collision}");
                let draw = draw.expect("the file is read");
                assert_eq!(draw.file, stem);
                assert!(report.missing.is_none());
            } else {
                assert!(choice.file.is_none() && draw.is_none());
                assert!(report.missing.as_deref().is_some_and(|m| m.starts_with("0810")));
            }
        }
        // a collision the map does not have is refused by name, not read as another file
        let (draw, report) =
            sekiro::read_area_draw_param(&archive, &decoder, map, &DrawParams::Collision("h999999".into()))?;
        assert!(draw.is_none() && report.collision.is_none());
        assert!(
            report
                .missing
                .as_deref()
                .is_some_and(|m| m.starts_with("collision h999999"))
        );
    }
    Ok(())
}
