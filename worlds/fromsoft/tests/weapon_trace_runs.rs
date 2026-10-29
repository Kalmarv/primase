//! A weapon's trail runs through a run of its model's dummy polygons, as Elden Ring's trace slot takes them (its executable,
//! decompiled; `effect_data::trace_run`): from the trace's head up to its tail, or, where no tail is given (-1), up while the
//! model has the next id, stopping at the first id the model lacks; a tail at or below the head gives none, and the game
//! draws no trail of a run of fewer than two.
//!
//! The census walks every trace slot of every weapon row whose model the archives hold, on that model's first FLVER (the
//! blade, model 0), and counts the slots by whether they give a tail and how long their run is: the test's expectations.
//! Elden Ring's Uchigatana (9000000, head 300, no tail) runs 300, 301, 302 and 303, from its guard to its tip, and Sekiro's
//! Kusabimaru (8000000, 300 to 301) runs its two. Read as a clip loads, the Uchigatana's trail in its first light attack
//! (`a029_030000`) runs through the right hand's first weapon's 10300 to 10303.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}` and `tools.oodle6`; a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::flver::Model;
use fromsoft_formats::{Binder, Game, Install, Keys, dcx::Decoder};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::convert::Reader;
use primase_core::{ClipKey, Name};
use primase_fromsoft::sekiro::effect_data::{ClipEffectUses, EquippedWeapons, UseRoute, trace_run};
use primase_fromsoft::sekiro::{self, WeaponParams};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The reference ids of the dummy polygons of weapon model `base`'s first FLVER, from the part binder the archives hold;
/// `None` where they hold none.
fn blade_dummies(
    archive: &fromsoft_formats::Archive,
    decoder: &Decoder<'_>,
    base: &str,
) -> Result<Option<BTreeSet<i32>>> {
    let Some(path) = [format!("{base}_l"), base.to_owned(), format!("{base}_m")]
        .iter()
        .map(|p| format!("/parts/{p}.partsbnd.dcx"))
        .find(|p| archive.contains(p))
    else {
        return Ok(None);
    };
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let blade = binder
        .files
        .iter()
        .find(|f| {
            f.name.as_deref().is_some_and(|n| {
                let n = n.to_ascii_lowercase();
                n.ends_with(&format!("{base}.flver")) || n.ends_with(&format!("{base}_l.flver"))
            })
        })
        .ok_or_else(|| format!("{path} holds no {base}.flver"))?;
    let model = Model::read(&decoder.decode_if_dcx(&blade.bytes)?)?;
    Ok(Some(
        model.dummies.iter().map(|d| i32::from(d.reference_id)).collect(),
    ))
}

#[test]
fn every_trace_slot_runs_through_its_weapons_dummy_polygons_as_the_game_takes_them() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library).map_err(|e| e.to_string())?;
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
        let mut models: BTreeMap<String, Option<BTreeSet<i32>>> = BTreeMap::new();
        // (a tail given, not -1; the run's length) -> trace slots
        let mut census: BTreeMap<(bool, usize), usize> = BTreeMap::new();
        let mut runs: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        for weapon in params.weapons()? {
            if weapon.model == 0 || weapon.traces.is_empty() {
                continue;
            }
            let base = weapon.part();
            if !models.contains_key(&base) {
                let dummies = blade_dummies(&archive, &decoder, &base)?;
                models.insert(base.clone(), dummies);
            }
            let Some(dummies) = &models[&base] else {
                continue;
            };
            for (slot, trace) in &weapon.traces {
                let run = trace_run(trace, dummies);
                *census.entry((trace.tail != -1, run.len())).or_default() += 1;
                if *slot == 0 {
                    runs.insert(weapon.row, run);
                }
            }
        }
        println!("FS_TRACE_RUNS={game:?} {census:?}");
        let expected: BTreeMap<(bool, usize), usize> = if game == Game::Sekiro {
            assert_eq!(runs[&8_000_000], [300, 301]);
            BTreeMap::from([((true, 0), 5), ((true, 2), 33)])
        } else {
            assert_eq!(runs[&9_000_000], [300, 301, 302, 303]);
            BTreeMap::from([
                ((false, 0), 539),
                ((false, 1), 1),
                ((false, 2), 931),
                ((false, 3), 309),
                ((false, 4), 391),
                ((false, 5), 75),
                ((false, 6), 113),
                ((false, 8), 13),
                ((false, 9), 13),
                ((true, 0), 255),
                ((true, 2), 550),
                ((true, 3), 111),
                ((true, 4), 64),
                ((true, 5), 44),
                ((true, 6), 3),
                ((true, 7), 1),
            ])
        };
        assert_eq!(census, expected, "{game:?}");
    }
    Ok(())
}

#[test]
fn the_uchigatana_trail_runs_from_its_guard_to_its_tip_as_a_clip_loads() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library).map_err(|e| e.to_string())?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let (Some(root), Some(folder)) = (install(Data::Elden), keys(Data::Elden)) else {
        return Ok(());
    };
    let game = Game::EldenRing;
    let installed = Install::new(root, game, Keys::from_folder(folder, game, None)?);
    let mut reader = sekiro::CharacterReader::animations(installed, "c0000", decoder)
        .with_clip_effects()
        .with_equipped_weapons(EquippedWeapons::right_hand(9_000_000));
    let (plan, _) = reader.read().map_err(|e| e.to_string())?;
    let (entity, _) = plan
        .iter::<ClipKey>()
        .find(|(e, _)| plan.get::<Name>(*e).is_some_and(|n| n.0 == "a029_030000"))
        .ok_or("a029_030000 not listed")?;
    let (loaded, _) = reader.load(&plan, &[entity].into()).map_err(|e| e.to_string())?;
    let uses = loaded
        .get::<ClipEffectUses>(entity)
        .ok_or("no effect uses on the clip")?;
    let trails: Vec<(Option<i32>, Vec<i32>)> = uses
        .uses
        .iter()
        .filter(|u| {
            matches!(
                u.route,
                UseRoute::WeaponTrace {
                    row: 9_000_000,
                    slot: 0
                }
            )
        })
        .map(|u| (u.dummy, u.span.clone()))
        .collect();
    assert!(!trails.is_empty(), "{:?}", uses.lost);
    assert!(
        trails
            .iter()
            .all(|t| *t == (Some(10_300), vec![10_301, 10_302, 10_303])),
        "{trails:?}"
    );
    Ok(())
}
