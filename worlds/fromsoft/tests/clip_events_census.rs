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
//! The installs also store events that begin before their clip: 6 in Sekiro and 26 in Elden Ring, every one at exactly
//! -1/30 s. The player's Elden Ring animations hold 8 of them (type 16): the 2 animations with a motion file of their own
//! load and lower to `core.clip_events` with the start as stored, and the other 6, which have none, are named with where
//! their motion comes from.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}`, `tools.oodle6` and `oracles.uxm.{sekiro,elden}/dictionary.txt`
//! (the census), `installs.elden`, `keys.elden`, `tools.oodle6` and `oracles.uxm.elden/dictionary.txt` (the player's
//! events); a game that is not configured is skipped.
mod inputs;

use fromsoft_formats::tae::{Header, Tae};
use fromsoft_formats::{Archive, Binder, Game, Keys, NameDictionary, dcx::Decoder};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use primase_core::convert::Reader;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{ClipEvents, ClipKey, Entity, Name, World};
use primase_fromsoft::game::ParamKey;
use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Calls `each` with every distinct event file of each configured game's animation binders (the binder's path and the
/// file), once per content; returns the games walked.
fn each_event_file(decoder: &Decoder<'_>, mut each: impl FnMut(Game, &str, &Tae)) -> Result<Vec<Game>> {
    let mut walked = Vec::new();
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
        for path in paths {
            let binder = Binder::read(&archive.read(path, decoder)?, decoder)?;
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
                each(game, path, &Tae::read_member(&binder, member)?);
            }
        }
        walked.push(game);
    }
    Ok(walked)
}

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
    #[derive(Default)]
    struct Count {
        files: usize,
        events: usize,
        templated: usize,
        lost: usize,
        shorter: usize,
        other: BTreeMap<String, usize>,
    }
    let mut counts: BTreeMap<Game, Count> = BTreeMap::new();
    let walked = each_event_file(&decoder, |game, _, tae| {
        let c = counts.entry(game).or_default();
        c.files += 1;
        for animation in &tae.animations {
            for decoded in primase_fromsoft::sekiro::decode_clip_events(game, &animation.events) {
                c.events += 1;
                if decoded.name.is_some() || !decoded.values.is_empty() {
                    c.templated += 1;
                }
                for refusal in &decoded.refusals {
                    if refusal.starts_with("stored shorter than the community template") {
                        c.shorter += 1;
                    } else if refusal.contains("needs ") && refusal.contains(" bytes at ") {
                        c.lost += 1;
                    } else {
                        *c.other.entry(refusal.chars().take(60).collect()).or_default() += 1;
                    }
                }
            }
        }
    })?;
    for game in walked {
        let c = counts.remove(&game).unwrap_or_default();
        println!(
            "{}: {} distinct event files, {} events, {} with a template, {} shorter with a named field unread, {} refused for a truncated field; other refusals {:?}",
            game.name(),
            c.files,
            c.events,
            c.templated,
            c.shorter,
            c.lost,
            c.other
        );
        // no event loses its values to a truncated field, and none violates an asserted value
        assert_eq!(c.lost, 0, "{}", game.name());
        assert!(c.other.keys().all(|r| !r.contains("asserts")), "{:?}", c.other);
        // the events without a template are Sekiro's 77 of types 340, 341 and 350
        assert_eq!(
            c.other
                .get("no community parameter template")
                .copied()
                .unwrap_or(0),
            if game == Game::Sekiro { 77 } else { 0 },
            "{}",
            game.name()
        );
    }
    Ok(())
}

/// The events of the installs that begin before their clip, by type: every one starts at exactly -1/30 s (the f32 the
/// files store), none has a non-finite time, and they are these.
#[test]
fn the_events_that_begin_before_their_clip_all_start_one_frame_early() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let mut early: BTreeMap<Game, BTreeMap<i32, usize>> = BTreeMap::new();
    let mut other_starts: Vec<(Game, String, f32)> = Vec::new();
    let mut nonfinite = 0usize;
    let walked = each_event_file(&decoder, |game, path, tae| {
        for animation in &tae.animations {
            for event in &animation.events {
                if !event.start.is_finite() || !event.end.is_finite() {
                    nonfinite += 1;
                }
                if event.start < 0.0 {
                    *early.entry(game).or_default().entry(event.kind).or_default() += 1;
                    if event.start != -1.0 / 30.0 {
                        other_starts.push((game, format!("{path} {}", tae.number(animation)), event.start));
                    }
                }
            }
        }
    })?;
    assert_eq!(nonfinite, 0);
    assert!(other_starts.is_empty(), "{other_starts:?}");
    for game in walked {
        let found = early.remove(&game).unwrap_or_default();
        println!(
            "{}: events that begin before their clip, by type: {found:?}",
            game.name()
        );
        let expected: BTreeMap<i32, usize> = match game {
            Game::Sekiro => [(16, 1), (112, 5)].into(),
            Game::EldenRing => [
                (0, 1),
                (1, 1),
                (16, 8),
                (66, 1),
                (118, 1),
                (129, 1),
                (700, 1),
                (711, 10),
                (760, 1),
                (787, 1),
            ]
            .into(),
        };
        assert_eq!(found, expected, "{}", game.name());
    }
    Ok(())
}

/// Elden Ring character clips, loaded and lowered ([`lowered_clips`]).
struct Lowered {
    world: World,
    /// The clips asked for, by name.
    clips: BTreeMap<String, Entity>,
    /// Every clip the character's reader lists.
    listed: BTreeSet<String>,
}

/// Elden Ring character `character`'s clips `clips`, loaded (`CharacterReader::animations`) and lowered through
/// `lower-havok-clips` and `lower-fromsoft-clip-events` with the pass checks on; `None` when the install is not configured.
fn lowered_clips(character: &str, clips: &[&str]) -> Result<Option<Lowered>> {
    let (Some(dir), Some(folder), Some(library)) =
        (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        return Ok(None);
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let install = common::install(
        dir,
        Game::EldenRing,
        Keys::from_folder(folder, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let mut reader = primase_fromsoft::sekiro::CharacterReader::animations(install, character, decoder);
    let (plan, _) = reader.read().map_err(|error| error.to_string())?;
    let named: BTreeMap<String, Entity> = plan
        .iter::<ClipKey>()
        .filter_map(|(e, _)| Some((plan.get::<Name>(e)?.0.clone(), e)))
        .collect();
    let listed: BTreeSet<String> = named.keys().cloned().collect();
    let asked: BTreeMap<String, Entity> = named
        .into_iter()
        .filter(|(name, _)| clips.contains(&name.as_str()))
        .collect();
    assert_eq!(asked.len(), clips.len(), "{character}: {:?}", asked.keys());
    let (mut loaded, _) = reader
        .load(&plan, &asked.values().copied().collect())
        .map_err(|error| error.to_string())?;
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    primase_havok::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    let mut run = RunReport::default();
    manager.run(
        &mut loaded,
        &manager.registry.parse(&format!(
            "lower-havok-clips{{absent=neutral,rate=1,root-motion=leave,clips={}}}, lower-fromsoft-clip-events{{game=elden}}",
            clips.join(";")
        ))?,
        &mut run,
    )?;
    assert!(loaded.verify_components().is_empty());
    Ok(Some(Lowered {
        world: loaded,
        clips: asked,
        listed,
    }))
}

/// Where an animation of an event file gets its motion, as its record and the character's binders state it.
#[derive(Debug, PartialEq)]
enum Motion {
    /// A motion file of its own name is a member of one of the binders.
    Own,
    /// Its header takes the motion of the animation of this number (its group's).
    Imports(i32),
    /// Its header takes both the motion and the events of the animation of this number.
    ImportsAll(i32),
    /// Its header names this motion file, and no binder holds a motion file of the animation's name.
    Missing(String),
}

/// The player's Elden Ring animations hold 8 events of type 16 that begin 1/30 s before their clip, all in
/// `c0000.anibnd`'s event files. 2 of the 8 animations have a motion file of their own, and are the clips
/// `CharacterReader::animations` lists: they load and `lower-fromsoft-clip-events` lowers each event with its start and end
/// as stored. The other 6 have none, and the reader lists none of them: 2 take the motion of another animation of their
/// file (`a000_004111` and `a000_004121`, 4020's), 2 take its motion and its events (`a231_036011` and `a231_036511`,
/// `a044_036011`'s and `a044_036511`'s), and 2 name a motion file (`a014_000100.hkt`, `a016_000100.hkt`) of which none of
/// the 14 c0000 animation binders the dictionary names holds one of the animation's name (the rest of the install is not
/// searched). Needs `oracles.uxm.elden/dictionary.txt` besides the install.
#[test]
fn the_players_events_that_begin_before_their_clip_lower_as_stored() -> Result<()> {
    let (Some(root), Some(folder), Some(library), Some(names)) = (
        install(Data::Elden),
        keys(Data::Elden),
        oodle(Tool::Oodle6),
        oracle(OracleSet::Uxm, Data::Elden, "dictionary.txt"),
    ) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let archive: Archive = common::open_game(
        root,
        Game::EldenRing,
        &Keys::from_folder(folder, Game::EldenRing, None)?,
    )?;
    let dictionary = NameDictionary::read_for(names, Game::EldenRing)?;
    let binders: BTreeSet<&str> = dictionary
        .names
        .values()
        .flatten()
        .map(String::as_str)
        .filter(|p| p.starts_with("/chr/c0000") && p.ends_with(".anibnd.dcx") && archive.contains(p))
        .collect();
    assert_eq!(binders.len(), 14, "{binders:?}");
    // the stems of the binders' members, and the animations of c0000.anibnd's event files by name
    let mut stems = BTreeSet::new();
    let mut animations = BTreeMap::new();
    for path in &binders {
        let binder = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
        for member in &binder.files {
            let Some(name) = member.name.as_deref() else {
                continue;
            };
            let short = name.rsplit(['\\', '/']).next().unwrap_or(name).to_ascii_lowercase();
            if *path == "/chr/c0000.anibnd.dcx" && short.ends_with(".tae") {
                let tae = Tae::read_member(&binder, member)?;
                for animation in tae.animations.iter() {
                    let number = tae.number(animation);
                    let label = format!("a{:03}_{:06}", number / 1_000_000, number % 1_000_000);
                    animations.insert(label, (animation.header.clone(), animation.file_name.clone()));
                }
            }
            stems.insert(short.split('.').next().unwrap_or_default().to_owned());
        }
    }
    let motion = |label: &str| -> Result<Motion> {
        let (header, file) = animations.get(label).ok_or_else(|| format!("{label}: no event file record"))?;
        Ok(match header {
            Header::ImportOther { from, .. } => Motion::ImportsAll(*from),
            Header::Standard {
                imports_hkx: true,
                import_hkx_source,
                ..
            } => Motion::Imports(*import_hkx_source),
            _ if stems.contains(label) => Motion::Own,
            _ => Motion::Missing(file.clone().unwrap_or_default()),
        })
    };
    // each animation, where its motion comes from, and its event's stored end
    let expected = [
        ("a000_004020", Motion::Own, 0.3),
        ("a000_004111", Motion::Imports(4020), 0.3),
        ("a000_004121", Motion::Imports(4020), 0.3),
        ("a014_000100", Motion::Missing("a014_000100.hkt".into()), 0.266_666_68),
        ("a016_000100", Motion::Missing("a016_000100.hkt".into()), 0.266_666_68),
        ("a044_036011", Motion::Own, 0.0),
        ("a231_036011", Motion::ImportsAll(44_036_011), 0.0),
        ("a231_036511", Motion::ImportsAll(44_036_511), 0.0),
    ];
    for (label, source, _) in &expected {
        assert_eq!(&motion(label)?, source, "{label}");
    }
    let clips: BTreeMap<&str, f32> = expected
        .iter()
        .filter(|(_, source, _)| *source == Motion::Own)
        .map(|(label, _, end)| (*label, *end))
        .collect();
    assert_eq!(clips.len(), 2);
    let names: Vec<&str> = clips.keys().copied().collect();
    let Some(lowered) = lowered_clips("c0000", &names)? else {
        return Ok(());
    };
    // the reader lists the two, and none of the other six
    let listed: Vec<&str> = expected
        .iter()
        .map(|(label, _, _)| *label)
        .filter(|label| lowered.listed.contains(*label))
        .collect();
    assert_eq!(listed, names);
    for (name, entity) in &lowered.clips {
        let events = lowered
            .world
            .get::<ClipEvents>(*entity)
            .ok_or_else(|| format!("{name}: no core.clip_events"))?;
        let early: Vec<(i64, f32, Option<f32>)> = events
            .events
            .iter()
            .filter(|e| e.start < 0.0)
            .map(|e| (e.kind, e.start, e.end))
            .collect();
        assert_eq!(early, [(16, -1.0 / 30.0, Some(clips[name.as_str()]))], "{name}");
    }
    Ok(())
}
