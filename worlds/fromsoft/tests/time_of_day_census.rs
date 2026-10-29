//! Census: the event-script calls that set the time of day, in every script of the installed games (the common scripts
//! and every map's, probed by path), each with its event, its place in the event and its time argument. Sekiro's are
//! `2003:81` ("Set Lighting", a time of day of the schema's `Time of Day` enum: 1 morning, 2 noon, 3 afternoon,
//! 4 evening, 5 night) and `2002:13` ("Play Cutscene and Warp Player With Lighting", the same enum); `2002:6`
//! ("Play Cutscene, Change Time Period and Warp Player") is in the schema but no script calls it. Elden Ring's is
//! `2001:4` ("Set Current Time", hours, minutes and seconds). It is the evidence for the hour a read is lowered at where
//! the program chooses none ([`primase_fromsoft::sekiro::passes::environment`]): it asserts only that each call's time
//! is in its enum's or the clock's range, and prints the calls.
//!
//! Runs where the installs, their keys and the Oodle library are configured; a game that is not is skipped.
mod common;
mod inputs;

use fromsoft_formats::emedf::{self, Value};
use fromsoft_formats::emevd::Script;
use fromsoft_formats::{Archive, Game, Keys, dcx::Decoder};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Every event-script path the archive holds: the common scripts, every map block's, and the open-world tiles'.
fn scripts(archive: &Archive) -> Vec<String> {
    let mut names = vec!["common".to_owned(), "common_func".to_owned()];
    for a in 10..100 {
        for b in 0..100 {
            names.push(format!("m{a:02}_{b:02}_00_00"));
        }
    }
    for a in [60, 61] {
        for x in 0..100 {
            for y in 0..100 {
                names.push(format!("m{a}_{x:02}_{y:02}_00"));
            }
        }
    }
    names
        .into_iter()
        .map(|n| format!("/event/{n}.emevd.dcx"))
        .filter(|p| archive.contains(p))
        .collect()
}

/// The time-setting instructions of `game`: their bank and id, and the index of the argument that holds the time (the
/// period, or the hour).
fn time_instructions(game: Game) -> &'static [(i32, i32, usize)] {
    match game {
        Game::Sekiro => &[(2003, 81, 0), (2002, 13, 6), (2002, 6, 6)],
        _ => &[(2001, 4, 0)],
    }
}

#[test]
fn the_calls_that_set_the_time_of_day() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    for (game, data) in [
        (Game::Sekiro, inputs::Game::Sekiro),
        (Game::EldenRing, inputs::Game::Elden),
    ] {
        let (Some(root), Some(folder)) = (inputs::install(data), inputs::keys(data)) else {
            continue;
        };
        let archive = common::open_game(root, game, &Keys::from_folder(folder, game, None)?)?;
        let paths = scripts(&archive);
        assert!(!paths.is_empty(), "{}: no event script", game.name());
        let wanted = time_instructions(game);
        let mut by_time: BTreeMap<(i32, i32, String), usize> = BTreeMap::new();
        for path in &paths {
            let script = Script::read(&archive.read(path, &decoder)?).map_err(|e| format!("{path}: {e}"))?;
            for event in &script.events {
                for (index, typed) in emedf::decode_event(game, event).into_iter().enumerate() {
                    let Ok(typed) = typed else { continue };
                    let Some(&(bank, id, at)) = wanted
                        .iter()
                        .find(|(b, i, _)| (*b, *i) == (typed.def.bank, typed.def.id))
                    else {
                        continue;
                    };
                    let args: Vec<String> = typed
                        .args
                        .iter()
                        .map(|a| match a.value {
                            _ if a.substituted => format!("{}=<parameter>", a.def.name),
                            Value::F32(v) => format!("{}={v}", a.def.name),
                            v => format!("{}={}", a.def.name, v.as_i64().unwrap_or_default()),
                        })
                        .collect();
                    let time = typed
                        .args
                        .get(at)
                        .filter(|a| !a.substituted)
                        .and_then(|a| a.value.as_i64());
                    if let Some(time) = time {
                        let range = if game == Game::Sekiro { 1..=5 } else { 0..=23 };
                        assert!(range.contains(&time), "{path} event {}: time {time}", event.id);
                    }
                    *by_time
                        .entry((bank, id, time.map_or("<parameter>".to_owned(), |t| t.to_string())))
                        .or_default() += 1;
                    println!(
                        "TIME {} {path} event {} #{index}/{} {bank}:{id} {}: {}",
                        game.name(),
                        event.id,
                        event.instructions.len(),
                        typed.def.name,
                        args.join(", ")
                    );
                }
            }
        }
        println!(
            "TIME {} {} scripts; calls by instruction and time: {by_time:?}",
            game.name(),
            paths.len()
        );
    }
    Ok(())
}
