//! The weapon params' fields at the offsets `sekiro::WeaponParams` reads them (Paramdex's layouts), checked on every stock
//! row of both installs: each weapon row's model id names a part binder the archives hold (a model of 0 is no model), its
//! hold row is a row of `WepAbsorpPosParam`, and each dummy polygon a hold row names is one of the player's model
//! (`c0000.flver`). A wrong offset would read other bytes as these numbers and miss on most rows; the counts are the
//! test's expectations. In Sekiro every check holds on every row; in Elden Ring 14 of 3,533 models have no binder,
//! 3,464 of the 3,636 rows name a hold row the param has, and 195 of the 776 dummy polygons the hold rows name are not
//! the player model's: 194 of them a body part's polygon, `2000 + n` (the Uchigatana's hang positions 2069 and 2070
//! among them), with `n` a polygon of the player's model, and one, 408, none.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}`, `tools.oodle6` (and for Elden Ring the regulation key in the keys
//! folder); a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::flver::Model;
use fromsoft_formats::{Binder, Game, Install, Keys, dcx::Decoder};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_fromsoft::sekiro::WeaponParams;
use primase_fromsoft::sekiro::effect_data::BODY_PART_DUMMIES;
use std::collections::BTreeSet;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// What the test counts for one install.
#[derive(Debug, Default, PartialEq, Eq)]
struct Census {
    weapons: usize,
    /// Weapon rows with a model, and those whose part binder (`wp_a_<id>`, `_l` or `_m`) the archives hold.
    with_model: usize,
    model_held: usize,
    /// Weapon rows whose hold row exists, and hold rows.
    hold_found: usize,
    hold_rows: usize,
    /// Dummy polygons the hold rows name (every model, every way), and those the player's model has.
    hold_dummies: usize,
    hold_dummies_found: usize,
    /// Of the others, those that are a body part's polygon (`2000 + n`) whose number `n` the player's model has.
    hold_dummies_body: usize,
    /// Weapon rows naming a trace effect in some slot.
    with_trace: usize,
}

#[test]
fn the_weapon_params_name_parts_hold_rows_and_dummy_polygons_that_exist() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    for (game, data) in [(Game::Sekiro, Data::Sekiro), (Game::EldenRing, Data::Elden)] {
        let (Some(root), Some(folder)) = (install(data), keys(data)) else {
            continue;
        };
        let installed = Install::new(root, game, Keys::from_folder(folder, game, None)?);
        let archive = installed.open()?;
        let params = WeaponParams::read(&installed, &archive, &decoder)?;
        let player = Binder::read(&archive.read("/chr/c0000.chrbnd.dcx", &decoder)?, &decoder)?;
        let flver = player
            .files
            .iter()
            .find(|f| {
                f.name
                    .as_deref()
                    .is_some_and(|n| n.to_ascii_lowercase().ends_with(".flver"))
            })
            .ok_or("c0000 holds no FLVER")?;
        let dummies: BTreeSet<i32> = Model::read(&decoder.decode_if_dcx(&flver.bytes)?)?
            .dummies
            .iter()
            .map(|d| i32::from(d.reference_id))
            .collect();
        let holds: BTreeSet<i32> = params.hold_rows().collect();
        let mut c = Census {
            hold_rows: holds.len(),
            ..Census::default()
        };
        for w in params.weapons()? {
            c.weapons += 1;
            if w.model > 0 {
                c.with_model += 1;
                let base = w.part();
                if [format!("{base}_l"), base.clone(), format!("{base}_m")]
                    .iter()
                    .any(|p| archive.contains(&format!("/parts/{p}.partsbnd.dcx")))
                {
                    c.model_held += 1;
                }
            }
            if holds.contains(&w.hold_row) {
                c.hold_found += 1;
            }
            if !w.traces.is_empty() {
                c.with_trace += 1;
            }
        }
        for row in &holds {
            for h in params.holds(*row)? {
                for d in [h.right, h.left, h.both, h.left_hang, h.right_hang, h.left_both]
                    .into_iter()
                    .flatten()
                {
                    c.hold_dummies += 1;
                    if dummies.contains(&d) {
                        c.hold_dummies_found += 1;
                    } else if (BODY_PART_DUMMIES..BODY_PART_DUMMIES + 1_000).contains(&d)
                        && dummies.contains(&(d - BODY_PART_DUMMIES))
                    {
                        c.hold_dummies_body += 1;
                    }
                }
            }
        }
        println!("FS_WEAPON_PARAMS={game:?} {c:?}");
        let expected = if game == Game::Sekiro {
            Census {
                weapons: 173,
                with_model: 66,
                model_held: 66,
                hold_found: 66,
                hold_rows: 33,
                hold_dummies: 38,
                hold_dummies_found: 38,
                hold_dummies_body: 0,
                with_trace: 38,
            }
        } else {
            Census {
                weapons: 3_636,
                with_model: 3_533,
                model_held: 3_519,
                hold_found: 3_464,
                hold_rows: 70,
                hold_dummies: 776,
                hold_dummies_found: 581,
                hold_dummies_body: 194,
                with_trace: 3_052,
            }
        };
        assert_eq!(c, expected, "{game:?}");
    }
    Ok(())
}
