//! `sample-fromsoft-draw-params` on the installed files: every field each stores at several hours, of every value set,
//! sampled through the day as the game samples it.
//!
//! The files: Elden Ring's every layer the Stormveil gate (`m10_00`) can draw with (the map's own files, and every
//! `m00_00_<id>` file and weather or event layer the archive holds: 98 whose axis starts at 6:30 and 4 at 7:00, all of a
//! period of 24 hours), and every draw-param file of Sekiro's maps (`m10` to `m25`). For each file and each hour of the
//! day in half hours (and 12.9, the hour a frame of the game stated at the gate):
//! - the pass runs without an error: every field's keys are ascending, timed and of one type;
//! - every field stored at several hours is sampled or left out by name, for one of the reasons the pass states;
//! - at the hour of each of its keys on the game's clock (the file's offset plus the key's time, past midnight taken
//!   off), a sampled field is that key's value as stored, the oracle the file itself gives.
//!
//! It prints the counts by outcome and every reason a field was left out, with the keys. Runs where the install, its keys
//! and the Oodle library are configured; otherwise it does nothing.
mod inputs;

use fromsoft_formats::gparam::{Gparam, TimeAxis};
use fromsoft_formats::{Archive, Game, Keys, dcx::Decoder};
use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::passes::draw_params::{AT_KEY, SAMPLE_FROMSOFT_DRAW_PARAMS, SAMPLED};
use primase_fromsoft::sekiro::source::{DefaultHour, DrawField, DrawGroup, DrawParam, FieldType, SetChosen};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The draw-param files of `game` the archive holds under the map prefixes `prefixes`, with `layers` (suffixes of a
/// file's stem) tried on each.
fn stems(archive: &Archive, prefixes: &[String], layers: &[&str]) -> Vec<String> {
    let mut stems = Vec::new();
    for prefix in prefixes {
        for id in 0..10_000 {
            for layer in layers {
                let stem = format!("{prefix}_{id:04}{layer}");
                if archive.contains(&format!("/param/drawparam/{stem}.gparam.dcx")) {
                    stems.push(stem);
                }
            }
        }
    }
    stems
}

/// The file's every value set of every group as one group of the environment: each field's samples at that set.
fn environment(game: Game, stem: &str, data: &Gparam) -> DrawParam {
    let mut groups = Vec::new();
    for group in &data.groups {
        let ids: BTreeSet<i32> = group.fields.iter().flat_map(|f| f.ids.iter().copied()).collect();
        for id in ids {
            let mut fields = Vec::new();
            for field in &group.fields {
                for index in field.indices_of(id) {
                    fields.push(DrawField {
                        key: field.key.clone(),
                        name: field.name.clone(),
                        value: field.values.get(index).expect("a value at each index"),
                        stored: FieldType::of(&field.values),
                        time: field.times.get(index).copied(),
                    });
                }
            }
            groups.push(DrawGroup {
                key: group.key.clone(),
                name: group.name.clone(),
                id,
                chosen: SetChosen::OnlySet,
                fields,
            });
        }
    }
    DrawParam {
        game,
        file: stem.to_owned(),
        version: data.version,
        time: data.time,
        groups,
        default_hour: DefaultHour {
            hour: 12.0,
            source: "this test".into(),
        },
    }
}

/// `env` sampled at `hour` by the pass as a pipeline runs it.
fn sample(env: &DrawParam, hour: f32) -> Result<(DrawParam, RunReport)> {
    let mut registry = PassRegistry::new();
    registry.add(&SAMPLE_FROMSOFT_DRAW_PARAMS);
    let manager = PassManager::new(registry).with_pass_checks(true);
    let mut world = World::new();
    world.insert_resource(env.clone());
    let mut report = RunReport::default();
    let pipeline = manager
        .registry
        .parse(&format!("sample-fromsoft-draw-params{{hour={hour}}}"))?;
    manager
        .run(&mut world, &pipeline, &mut report)
        .map_err(|e| format!("{} at {hour}: {e}", env.file))?;
    let env = world
        .resource::<DrawParam>()
        .cloned()
        .ok_or("no environment after sampling")?;
    Ok((env, report))
}

#[derive(Default)]
struct Tally {
    files: usize,
    axes: BTreeMap<String, usize>,
    timelines: u64,
    runs: u64,
    counts: BTreeMap<String, u64>,
    omitted: BTreeMap<String, u64>,
    approximated: BTreeMap<String, u64>,
    keys_checked: u64,
    keys_off_the_clock: u64,
}

fn census(game: Game, archive: &Archive, decoder: &Decoder<'_>, stems: &[String]) -> Result<Tally> {
    let mut tally = Tally::default();
    let mut hours: Vec<f32> = (0..=48).map(|h| h as f32 / 2.0).collect();
    hours.push(12.9);
    for stem in stems {
        let bytes = archive.read(&format!("/param/drawparam/{stem}.gparam.dcx"), decoder)?;
        let data = Gparam::read(&bytes)?;
        let env = environment(game, stem, &data);
        tally.files += 1;
        let (offset, period) = data.time.map_or((0.0, 0.0), |t: TimeAxis| t.sampling());
        *tally
            .axes
            .entry(format!("offset {offset} period {period}"))
            .or_default() += 1;
        // the timelines: a key of a group's set stored at several hours, with its samples
        let mut timelines: Vec<(usize, &str, Vec<&DrawField>)> = Vec::new();
        for (g, group) in env.groups.iter().enumerate() {
            let mut by_key: BTreeMap<&str, Vec<&DrawField>> = BTreeMap::new();
            for field in &group.fields {
                by_key.entry(field.key.as_str()).or_default().push(field);
            }
            for (key, samples) in by_key {
                if samples.len() > 1 {
                    timelines.push((g, key, samples));
                }
            }
        }
        tally.timelines += timelines.len() as u64;
        for &hour in &hours {
            let (_, report) = sample(&env, hour)?;
            let log = &report.steps[0].log;
            let sampled = log.counts.get(SAMPLED).copied().unwrap_or(0);
            let omitted: u64 = log.omitted.values().sum();
            assert_eq!(
                sampled + omitted,
                timelines.len() as u64,
                "{stem} at {hour}: every timeline sampled or left out"
            );
            tally.runs += 1;
            for (reason, n) in &log.counts {
                *tally.counts.entry(reason.clone()).or_default() += n;
            }
            for (reason, n) in &log.omitted {
                *tally.omitted.entry(reason.clone()).or_default() += n;
            }
            for (reason, n) in &log.approximations {
                *tally.approximated.entry(reason.clone()).or_default() += n;
            }
        }
        // at each key's hour on the game's clock, a sampled field is the key as stored: the keys by that hour
        let mut at_hour: BTreeMap<u32, Vec<(usize, &str, &DrawField)>> = BTreeMap::new();
        for (g, key, samples) in &timelines {
            for sample_ in samples {
                let time = sample_.time.expect("a timed sample");
                let mut hour = offset + time;
                if period != 0.0 && hour > 24.0 {
                    hour -= period;
                }
                if (0.0..=24.0).contains(&hour) {
                    at_hour
                        .entry(hour.to_bits())
                        .or_default()
                        .push((*g, key, sample_));
                } else {
                    tally.keys_off_the_clock += 1;
                }
            }
        }
        for (hour, keys) in at_hour {
            let hour = f32::from_bits(hour);
            let (sampled, report) = sample(&env, hour)?;
            let log = &report.steps[0].log;
            for (g, key, stored) in keys {
                let Some(field) = sampled.groups[g].fields.iter().find(|f| f.key == key) else {
                    assert!(
                        log.omitted.keys().any(|r| r.ends_with(&format!(": {key}"))),
                        "{stem} {key} at {hour}: neither sampled nor left out by name"
                    );
                    continue;
                };
                assert_eq!(
                    field.value, stored.value,
                    "{stem} [{}] {key}: at {hour}, its key at {:?}",
                    env.groups[g].key, stored.time
                );
                assert!(log.counts.get(AT_KEY).is_some_and(|n| *n > 0));
                tally.keys_checked += 1;
            }
        }
    }
    Ok(tally)
}

fn print(game: Game, tally: &Tally) {
    let name = game.name();
    println!(
        "SAMPLING {name} files {} axes {:?} timelines {} runs {} keys_checked {} keys_off_the_clock {}",
        tally.files, tally.axes, tally.timelines, tally.runs, tally.keys_checked, tally.keys_off_the_clock
    );
    for (reason, n) in &tally.counts {
        println!("SAMPLING {name} COUNT {n} {reason}");
    }
    for (reason, n) in &tally.approximated {
        println!("SAMPLING {name} APPROXIMATED {n} {reason}");
    }
    for (reason, n) in &tally.omitted {
        println!("SAMPLING {name} OMITTED {n} {reason}");
    }
}

#[test]
fn every_installed_timeline_samples_as_the_game_does() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data) in [
        (Game::EldenRing, inputs::Game::Elden),
        (Game::Sekiro, inputs::Game::Sekiro),
    ] {
        let (Some(root), Some(folder)) = (inputs::install(data), inputs::keys(data)) else {
            continue;
        };
        let keys = Keys::from_folder(folder, game, game.param_key_sha256())?;
        let archive = common::install(root, game, keys).open()?;
        let tally = match game {
            Game::EldenRing => {
                let prefixes = ["m10_00".to_owned(), "m00_00".to_owned()];
                let layers = [
                    "",
                    "_weatherbase",
                    "_weatheroutdoor",
                    "_commonevent",
                    "_commoneventmapunique",
                ];
                let tally = census(game, &archive, &decoder, &stems(&archive, &prefixes, &layers))?;
                // the files a frame of the game is drawn with at the gate: 98 on an axis from 6:30, 4 from 7:00
                assert_eq!(
                    tally.axes,
                    BTreeMap::from([
                        ("offset 6.5 period 24".to_owned(), 98),
                        ("offset 7 period 24".to_owned(), 4)
                    ])
                );
                tally
            }
            Game::Sekiro => {
                let prefixes: Vec<String> = (10..=25)
                    .flat_map(|area| (0..=2).map(move |block| format!("m{area}_{block:02}")))
                    .collect();
                let tally = census(game, &archive, &decoder, &stems(&archive, &prefixes, &[""]))?;
                assert!(tally.files > 0, "Sekiro's maps hold draw-param files");
                tally
            }
        };
        print(game, &tally);
        assert!(tally.keys_checked > 0, "{}: some key checked", game.name());
    }
    Ok(())
}
