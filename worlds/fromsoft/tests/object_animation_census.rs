//! A census of what the install's placed-object binders hold besides their model: Sekiro's `/obj/*.objbnd.dcx`,
//! Elden Ring's `/asset/aeg/*/*.geombnd.dcx`. The area read's `object_animations` option reads the animation binder as
//! a `.anibnd` member of the model's binder (Sekiro: 233 of 1395, none beside the binder as a file of its own, which
//! this census still checks for Elden Ring), so this prints what is there instead of assuming:
//!
//! - `members <game> <extension> <count>`: the extensions of every member of every object binder, so an animation
//!   stored under another name shows;
//! - `nested <game> <count>` and `beside <game> <count>`: binders with an `.anibnd` member, and with an `.anibnd.dcx`
//!   binder beside them in the archive;
//! - `sample <game> <binder> <members of its animation binder>`: the first few of each, to read the names from.
//!
//! It asserts only that the binders read. Runs where the install, the dictionary and the Oodle library are configured;
//! otherwise it does nothing.
mod inputs;

use inputs::{Game, OracleSet, Tool, install, oodle, oracle};
use primase_fromsoft::{Archive, Binder, NameDictionary, dcx::Decoder};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[allow(unsafe_code)]
fn native(
    path: &inputs::OodleLibrary,
) -> primase_foundations::compress::Result<primase_foundations::compress::oodle::Oodle> {
    // SAFETY: caller explicitly supplies a trusted native version-6 decoder.
    unsafe {
        primase_foundations::compress::oodle::Oodle::open(
            path.path(),
            primase_foundations::compress::oodle::Version::V6,
        )
    }
}

fn leaf(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
}

fn census(
    game: Game,
    kind: primase_fromsoft::Game,
    label: &str,
    is_object_binder: impl Fn(&str) -> Option<&'static str>,
) -> Result<()> {
    let Some(dir) = install(game) else {
        return Ok(());
    };
    // each game opens with its own keys: Sekiro's are published, Elden Ring's are the install owner's folder
    let keys = match (kind, inputs::keys(game)) {
        (primase_fromsoft::Game::Sekiro, _) => {
            primase_fromsoft::Keys::published(kind).ok_or("Sekiro's keys")?
        }
        (_, Some(folder)) => primase_fromsoft::Keys::from_folder(folder, kind)?,
        (_, None) => return Ok(()),
    };
    let Some(names) = oracle(OracleSet::Uxm, game, "dictionary.txt") else {
        return Ok(());
    };
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let oodle = native(&dll)?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1024 * 1024 * 1024,
    };
    let archive = Archive::open_game(dir, kind, &keys)?;
    let names = NameDictionary::read_for(names, kind)?;
    let mut members: BTreeMap<String, usize> = BTreeMap::new();
    let (mut binders, mut nested, mut beside, mut samples) = (0, 0, 0, 0);
    let mut failures = Vec::new();
    for (&hash, paths) in &names.names {
        if !archive.entries.contains_key(&hash) {
            continue;
        }
        for path in paths {
            let Some(suffix) = is_object_binder(path) else {
                continue;
            };
            let result = (|| -> Result<()> {
                let binder = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
                binders += 1;
                let mut animation = None;
                for f in &binder.files {
                    let name = leaf(f.name.as_deref().unwrap_or(""));
                    let extension = name.split_once('.').map_or("(none)", |(_, e)| e).to_owned();
                    *members.entry(extension).or_default() += 1;
                    if name.ends_with(".anibnd") || name.ends_with(".anibnd.dcx") {
                        animation = Some(Binder::read(&decoder.decode_if_dcx(&f.bytes)?, &decoder)?);
                    }
                }
                let sibling = path.strip_suffix(suffix).map(|stem| format!("{stem}.anibnd.dcx"));
                if let Some(sibling) = sibling.filter(|p| archive.contains(p)) {
                    beside += 1;
                    if animation.is_none() {
                        animation = Some(Binder::read(&archive.read(&sibling, &decoder)?, &decoder)?);
                    }
                }
                if let Some(animation) = animation {
                    nested += 1;
                    if samples < 6 {
                        samples += 1;
                        let list: Vec<String> = animation
                            .files
                            .iter()
                            .take(8)
                            .map(|f| leaf(f.name.as_deref().unwrap_or("")))
                            .collect();
                        println!(
                            "sample {label} {path} {} members: {list:?}",
                            animation.files.len()
                        );
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                failures.push(format!("{path}: {e}"));
            }
        }
    }
    println!("binders {label} {binders}");
    for (extension, count) in &members {
        println!("members {label} {extension} {count}");
    }
    println!("nested-or-beside {label} {nested} (beside {beside})");
    for f in &failures {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} unreadable binders", failures.len());
    Ok(())
}

#[test]
fn what_the_object_binders_hold() -> Result<()> {
    census(Game::Sekiro, primase_fromsoft::Game::Sekiro, "sekiro", |p| {
        (p.starts_with("/obj/") && p.ends_with(".objbnd.dcx")).then_some(".objbnd.dcx")
    })?;
    census(Game::Elden, primase_fromsoft::Game::EldenRing, "elden", |p| {
        (p.starts_with("/asset/aeg/") && p.ends_with(".geombnd.dcx")).then_some(".geombnd.dcx")
    })?;
    Ok(())
}
