//! Every event of every animation binder of the installs (`/chr/*.anibnd.dcx` and `/obj/*.anibnd.dcx` that the
//! dictionaries name and the archives hold, each distinct event file once) through the community templates.
//!
//! An independent census of all the installs' animation binders (a separate parser of the TAE header, animations and
//! events, the template read from the XML) finds that no asserted value is violated, that 77 Sekiro events (types 340,
//! 341 and 350) have no template, and that 4,627 Sekiro and 268 Elden Ring events store fewer bytes than their template's
//! fields need. The event reader gives an event the bytes up to the next record, padding included, so the events whose
//! shortfall the padding covers decode their last fields from padding zeros and are not short here: what this test
//! sees is the remainder, which keeps its stored fields and says which it lacks, where it used to lose all its typed
//! values to "needs N bytes".
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}`, `tools.oodle6` and `oracles.uxm.{sekiro,elden}/dictionary.txt`;
//! a game that is not configured is skipped.
mod inputs;

use fromsoft_formats::{Archive, Binder, Game, Keys, NameDictionary, dcx::Decoder, tae::Tae};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn events_stored_shorter_than_their_template_keep_the_fields_they_store() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    for (game, data) in [(Game::Sekiro, Data::Sekiro), (Game::EldenRing, Data::Elden)] {
        let (Some(root), Some(folder), Some(names)) = (
            install(data),
            keys(data),
            oracle(OracleSet::Uxm, data, "dictionary.txt"),
        ) else {
            continue;
        };
        let archive: Archive = common::open_game(root, game, &Keys::from_folder(folder, game, None)?)?;
        let dictionary = NameDictionary::read_for(names, game)?;
        let paths: BTreeSet<&str> = dictionary
            .names
            .values()
            .flatten()
            .map(String::as_str)
            .filter(|p| {
                (p.starts_with("/chr/") || p.starts_with("/obj/"))
                    && p.ends_with(".anibnd.dcx")
                    && archive.contains(p)
            })
            .collect();
        let mut seen = BTreeSet::new();
        let (mut events, mut templated, mut lost, mut shorter, mut other) =
            (0, 0, 0, 0, BTreeMap::<String, usize>::new());
        for path in paths {
            let binder = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
            for member in &binder.files {
                if !member
                    .name
                    .as_deref()
                    .is_some_and(|n| n.to_ascii_lowercase().ends_with(".tae"))
                {
                    continue;
                }
                let mut hasher = DefaultHasher::new();
                member.bytes.hash(&mut hasher);
                if !seen.insert((member.bytes.len(), hasher.finish())) {
                    continue;
                }
                for animation in &Tae::read_member(&binder, member)?.animations {
                    for decoded in primase_fromsoft::sekiro::decode_clip_events(game, &animation.events) {
                        events += 1;
                        if decoded.name.is_some() || !decoded.values.is_empty() {
                            templated += 1;
                        }
                        for refusal in &decoded.refusals {
                            if refusal.starts_with("stored shorter than the community template") {
                                shorter += 1;
                            } else if refusal.contains("needs ") && refusal.contains(" bytes at ") {
                                lost += 1;
                            } else {
                                *other.entry(refusal.chars().take(60).collect()).or_default() += 1;
                            }
                        }
                    }
                }
            }
        }
        println!(
            "{}: {} distinct event files, {events} events, {templated} with a template, {shorter} shorter with a named field unread, {lost} refused for a truncated field; other refusals {other:?}",
            game.name(),
            seen.len()
        );
        // no event loses its values to a truncated field, and none violates an asserted value
        assert_eq!(lost, 0, "{}", game.name());
        assert!(other.keys().all(|r| !r.contains("asserts")), "{other:?}");
        // the events without a template are Sekiro's 77 of types 340, 341 and 350
        assert_eq!(
            other.get("no community parameter template").copied().unwrap_or(0),
            if game == Game::Sekiro { 77 } else { 0 },
            "{}",
            game.name()
        );
    }
    Ok(())
}
