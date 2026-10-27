//! A census of the Havok files a character's binders hold, for twenty characters of each game, so that a reader for a
//! class (cloth, ragdoll, rigid bodies) is written for a class the files are known to hold, not for one a community
//! tool names. It prints; it asserts only that the reads that start succeed. The lines:
//!
//! - `CHARACTERS <game> <ids>`: the twenty characters taken (every `cNNNN` whose `chrbnd` or `anibnd` the install has,
//!   twenty of them evenly spread over the list);
//! - `HAVOK <game> <id> <binder> members <n> classes <class: items, ...> example <member>`: the Havok members of one
//!   binder grouped by the classes their items have, so a binder whose members all share a layout is one line.
//!   Animation clips (`aNNN_NNNNNN.hkx`, hundreds a character) are read three at a time and counted;
//! - `HAVOKERR <game> <id> <binder> <member> <why>`: a member that is no tagfile this reader reads (its file kind);
//! - `CLASS <game> <class> fields [..]`: the stored field names of each class whose name says cloth, ragdoll, rigid body,
//!   constraint or physics, once per game, so a reader's fields are the file's own;
//! - `EXT <game> <binder kind> <extension> members <n> characters <k> read <yes|no>`: every member of the twenty
//!   characters' binders by binder kind and extension, and whether a primase reader keeps it (`yes` for the model, its
//!   textures, the cloth companion, the animation binder's tagfiles and event files; the rest is what a notice names as
//!   not read).
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
use fromsoft_formats::{Binder, Game, Install, Keys, dcx::Decoder};
use havok_formats::{Compendium, Tagfile};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The binder kinds a character has, and the variants of the animation and behavior binders.
const KINDS: [&str; 4] = ["chrbnd", "anibnd", "behbnd", "texbnd"];
const VARIANTS: [&str; 4] = ["", "_div00", "_div01", "_div02"];
const TAKEN: usize = 20;

/// Whether a primase reader keeps a member of a binder of `kind` with extension `ext`.
fn kept(kind: &str, ext: &str) -> bool {
    matches!(
        (kind, ext),
        ("chrbnd", "flver" | "clm2" | "tpf") | ("texbnd", "tpf") | ("anibnd", "hkx" | "tae" | "compendium")
    )
}

/// A clip's member name (`a000_003000.hkx`).
fn is_clip(leaf: &str) -> bool {
    leaf.strip_prefix('a')
        .and_then(|rest| rest.strip_suffix(".hkx"))
        .and_then(|rest| rest.split_once('_'))
        .is_some_and(|(group, number)| {
            !group.is_empty()
                && !number.is_empty()
                && group.bytes().chain(number.bytes()).all(|b| b.is_ascii_digit())
        })
}

fn interesting(class: &str) -> bool {
    let lower = class.to_ascii_lowercase();
    ["cloth", "ragdoll", "rigid", "constraint", "physics"]
        .iter()
        .any(|word| lower.contains(word))
}

fn census(game: Game, install: &Install, decoder: &Decoder<'_>) -> Result<()> {
    let archive = install.open()?;
    let held: Vec<String> = (0..10_000)
        .map(|n| format!("c{n:04}"))
        .filter(|id| {
            archive.contains(&format!("/chr/{id}.chrbnd.dcx"))
                || archive.contains(&format!("/chr/{id}.anibnd.dcx"))
        })
        .collect();
    let ids: Vec<&String> = (0..TAKEN.min(held.len()))
        .map(|i| &held[i * held.len() / TAKEN.min(held.len())])
        .collect();
    println!(
        "CHARACTERS {} {} of {}: {:?}",
        game.name(),
        ids.len(),
        held.len(),
        ids
    );
    let mut ext: BTreeMap<(String, String), (usize, BTreeSet<String>)> = BTreeMap::new();
    let mut shown = BTreeSet::new();
    for id in ids {
        for kind in KINDS {
            for variant in VARIANTS {
                let name = format!("{id}{variant}.{kind}");
                let path = format!("/chr/{name}.dcx");
                if !archive.contains(&path) {
                    continue;
                }
                let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
                for f in &binder.files {
                    let leaf = f
                        .name
                        .as_deref()
                        .unwrap_or("")
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or("");
                    let e = leaf.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
                    let entry = ext.entry((kind.to_owned(), e)).or_default();
                    entry.0 += 1;
                    entry.1.insert(id.clone());
                }
                let compendium = match binder.files.iter().find(|f| {
                    f.name
                        .as_deref()
                        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".compendium"))
                }) {
                    Some(f) => Some(Compendium::read(&decoder.decode_if_dcx(&f.bytes)?)?),
                    None => None,
                };
                let mut groups: BTreeMap<String, (usize, String)> = BTreeMap::new();
                let mut clips_read = 0;
                for f in &binder.files {
                    let member = f.name.as_deref().unwrap_or("");
                    let leaf = member
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(member)
                        .to_ascii_lowercase();
                    if !leaf.ends_with(".hkx") {
                        continue;
                    }
                    if is_clip(&leaf) {
                        if clips_read == 3 {
                            groups
                                .entry("(clips beyond the three read)".to_owned())
                                .or_default()
                                .0 += 1;
                            continue;
                        }
                        clips_read += 1;
                    }
                    let bytes = decoder.decode_if_dcx(&f.bytes)?;
                    let tag = match Tagfile::read(&bytes, compendium.as_ref()) {
                        Ok(tag) => tag,
                        Err(e) => {
                            println!("HAVOKERR {} {id} {name} {leaf} {}", game.name(), e.detail);
                            continue;
                        }
                    };
                    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
                    for item in tag.items.iter().skip(1) {
                        if let Some(t) = tag.types.get(item.type_index) {
                            *classes.entry(t.name.as_str()).or_default() += 1;
                            if interesting(&t.name) && shown.insert(t.name.clone()) {
                                println!(
                                    "CLASS {} {} fields {:?}",
                                    game.name(),
                                    t.name,
                                    t.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()
                                );
                            }
                        }
                    }
                    // the cloth and ragdoll reader on every file that holds either: what it read, or why not
                    if classes
                        .keys()
                        .any(|c| c.starts_with("hclClothData") || c.starts_with("hknpRagdollData"))
                    {
                        match havok_formats::cloth::read(&tag) {
                            Ok(physics) => {
                                let sims: Vec<_> =
                                    physics.cloths.iter().flat_map(|c| &c.sim_cloths).collect();
                                let unread: BTreeSet<&str> = sims
                                    .iter()
                                    .flat_map(|s| &s.constraint_sets)
                                    .filter_map(|s| match s {
                                        havok_formats::cloth::ConstraintSet::Unread { class } => {
                                            Some(class.as_str())
                                        }
                                        _ => None,
                                    })
                                    .collect();
                                println!(
                                    "PHYSICS {} {id} {leaf} cloths {} sim_cloths {} particles {} fixed {} poses {} sets {} \
                                     unread_sets {unread:?} ragdolls {} ragdoll_bones {} constraints {}",
                                    game.name(),
                                    physics.cloths.len(),
                                    sims.len(),
                                    sims.iter().map(|s| s.particles.len()).sum::<usize>(),
                                    sims.iter().map(|s| s.fixed_particles.len()).sum::<usize>(),
                                    sims.iter().map(|s| s.poses.len()).sum::<usize>(),
                                    sims.iter().map(|s| s.constraint_sets.len()).sum::<usize>(),
                                    physics.ragdolls.len(),
                                    physics
                                        .ragdolls
                                        .iter()
                                        .map(|r| r.skeleton.bones.len())
                                        .sum::<usize>(),
                                    physics.constraints.len(),
                                );
                            }
                            Err(e) => println!("PHYSICSERR {} {id} {leaf} {e}", game.name()),
                        }
                    }
                    let key = classes
                        .iter()
                        .map(|(c, n)| format!("{c}: {n}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let entry = groups.entry(key).or_insert_with(|| (0, leaf.clone()));
                    entry.0 += 1;
                }
                for (classes, (members, example)) in groups {
                    println!(
                        "HAVOK {} {id} {name} members {members} classes [{classes}] example {example}",
                        game.name()
                    );
                }
            }
        }
    }
    for ((kind, e), (members, characters)) in ext {
        println!(
            "EXT {} {kind} {e:?} members {members} characters {} read {}",
            game.name(),
            characters.len(),
            if kept(&kind, &e) { "yes" } else { "no" }
        );
    }
    Ok(())
}

#[test]
fn what_the_character_binders_hold_of_havok() -> Result<()> {
    let Some(dll) = primase_testdata::oodle(primase_testdata::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data) in [
        (Game::Sekiro, primase_testdata::Game::Sekiro),
        (Game::EldenRing, primase_testdata::Game::Elden),
    ] {
        let (Some(root), keys) = (primase_testdata::install(data), primase_testdata::keys(data)) else {
            continue;
        };
        let keys = match (game, keys) {
            (_, Some(folder)) => Keys::from_folder(folder, game, None)?,
            (_, None) => continue,
        };
        let install = Install::new(root, game, keys);
        census(game, &install, &decoder)?;
    }
    Ok(())
}
