//! The game's own text: the names it shows for what its params describe. An NPC param row states a `nameId`
//! ([`crate::param::NpcStats`]); the item message binder's NPC-name FMG ([`crate::fmg`]) holds the string for it, in the
//! language the caller explicitly requests. This is the game's statement of what a character is called, the first source of a name
//! (a community list's guesses come after it).
use std::collections::BTreeMap;

use crate::fmg::Fmg;
use crate::param::NpcStats;
use crate::{Archive, Binder, Game, Install, Result, dcx::Decoder, invalid};

const FORMAT: &str = "FromSoftware messages";

/// Read the authored NPC-row/FMG join into the World for name-fromsoft-entities.
/// This is explicit reader work: any error leaves the old table unchanged, and a
/// later pure naming pass neither reopens an install nor picks a different language.
pub fn npc_names_into_world(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    dictionary: &crate::NameDictionary,
    language: &str,
    world: &mut primase_core::World,
) -> Result<usize> {
    let read = npc_names(install, archive, decoder, dictionary, language)?;
    let count = read.names.len();
    let names = crate::sekiro::character_data::NpcNames {
        game: install.game,
        language: language.into(),
        names: read.names,
        report: read.report,
    };
    primase_core::Resource::verify(&names, world).map_err(|reason| invalid(FORMAT, 0, reason))?;
    world.insert_resource(names);
    Ok(count)
}

/// NPC text together with every absent optional binder and repeated FMG id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NpcNamesRead {
    pub names: BTreeMap<i32, String>,
    pub report: Vec<String>,
}

/// Actual message language folders represented by the supplied dictionary AND archive.
/// A hash-only archive cannot enumerate names without this caller-provided index.
pub fn message_languages(archive: &Archive, dictionary: &crate::NameDictionary) -> Vec<String> {
    let mut languages = std::collections::BTreeSet::new();
    for path in dictionary.names.values().flatten() {
        if archive.contains(path)
            && let Some(rest) = path.strip_prefix("/msg/")
            && let Some((language, _)) = rest.split_once('/')
            && !language.is_empty()
        {
            languages.insert(language.to_owned());
        }
    }
    languages.into_iter().collect()
}
fn choose_language<'a>(language: &'a str, available: &[String]) -> Result<&'a str> {
    if language.is_empty() || !available.iter().any(|option| option == language) {
        return Err(invalid(
            FORMAT,
            0,
            format!(
                "language {language:?} is absent or unknown; msg/<lang> options present in the archive: {available:?}"
            ),
        ));
    }
    Ok(language)
}
fn merge_text(
    text: &mut BTreeMap<i32, String>,
    origins: &mut BTreeMap<i32, String>,
    path: &str,
    entries: BTreeMap<i32, String>,
    report: &mut Vec<String>,
) {
    for (id, string) in entries {
        if let Some(first) = text.get(&id) {
            report.push(format!(
                "duplicate FMG id {id}: {} and {path}; strings {}; first text retained",
                origins[&id],
                if first == &string { "identical" } else { "differ" }
            ));
        } else {
            origins.insert(id, path.into());
            text.insert(id, string);
        }
    }
}

/// The message binders of `game` that hold NPC names, and in each the binder id of its NPC-name FMG
/// (Smithbox's `SoulsFmg` table: `NpcName` is 18 in both games' `item.msgbnd`, Elden Ring's DLC binders hold theirs
/// beside it). The first binder is the game's own and must be there; the others are added content a install may lack.
fn npc_name_binders(game: Game, language: &str) -> Vec<(String, i32)> {
    let binder = |name: &str| format!("/msg/{language}/{name}.msgbnd.dcx");
    match game {
        Game::Sekiro => vec![(binder("item"), 18)],
        Game::EldenRing => vec![
            (binder("item"), 18),
            (binder("item_dlc01"), 328),
            (binder("item_dlc02"), 428),
        ],
    }
}

/// The FMG of binder id `id` in `binder` (read from `path`).
fn fmg_of(binder: &Binder, id: i32, path: &str) -> Result<Fmg> {
    let file = binder
        .files
        .iter()
        .find(|file| file.id == Some(id))
        .ok_or_else(|| invalid(FORMAT, 0, format!("{path} holds no FMG of binder id {id}")))?;
    Fmg::read(&file.bytes)
}

/// The name of every NPC param row that has one: `stats`' `nameId` looked up in `text`. A row with no `nameId` (0 or
/// less), an id the FMG does not hold, an empty string or the engine's `%null%` placeholder has none.
pub fn npc_row_names(stats: &[NpcStats], text: &BTreeMap<i32, String>) -> BTreeMap<i32, String> {
    stats
        .iter()
        .filter(|row| row.name_id > 0)
        .filter_map(|row| {
            let name = text.get(&row.name_id)?.trim();
            (!name.is_empty() && name != "%null%").then(|| (row.id, name.to_owned()))
        })
        .collect()
}

/// The NPC param's rows by id with the name the game gives each (`NpcParam.nameId` into the NPC-name FMG), for the rows
/// that have one. The param is read as `read_param` reads it and its descriptive fields as [`NpcStats::from_param`]
/// does; an install whose layout those do not match, or that lacks the game's own item message binder, is refused
/// saying so.
pub fn npc_names(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    dictionary: &crate::NameDictionary,
    language: &str,
) -> Result<NpcNamesRead> {
    if install.game != archive.game() {
        return Err(invalid(FORMAT, 0, "install and archive games differ"));
    }
    let languages = message_languages(archive, dictionary);
    let language = choose_language(language, &languages)?;
    let param = super::params::read_param(install, archive, decoder, "NpcParam.param")?;
    let stats = NpcStats::from_param(&param, &install.game.npc_stats())?;
    let mut text = BTreeMap::new();
    let mut origins = BTreeMap::new();
    let mut report = Vec::new();
    for (position, (path, id)) in npc_name_binders(install.game, language).into_iter().enumerate() {
        if !archive.contains(&path) {
            if position == 0 {
                return Err(invalid(FORMAT, 0, format!("{path} is not in the archives")));
            }
            report.push(format!("optional NPC-name binder absent: {path}"));
            continue;
        }
        let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
        merge_text(
            &mut text,
            &mut origins,
            &path,
            fmg_of(&binder, id, &path)?.entries,
            &mut report,
        );
    }
    Ok(NpcNamesRead {
        names: npc_row_names(&stats, &text),
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BinderFile;
    use crate::fmg::tests::build;

    fn stats(id: i32, name_id: i32) -> NpcStats {
        NpcStats {
            id,
            behavior_variation_id: 0,
            name_id,
            turn_velocity: 0.0,
            hit_height: 0.0,
            hit_radius: 0.0,
            weight: 0,
            hit_y_offset: 0.0,
            hp: 0,
            mp: 0,
            get_soul: 0,
        }
    }

    #[test]
    fn a_row_is_named_by_its_name_id_in_the_npc_name_fmg() {
        let fmg = Fmg::read(&build(
            &[(
                100_000,
                &[
                    Some("Genichiro Ashina"),
                    Some("%null%"),
                    Some("  "),
                    Some("Isshin Ashina"),
                ] as &[Option<&str>],
            )],
            true,
        ))
        .unwrap();
        let rows = [
            stats(1, 100_000),
            stats(2, 100_001), // the engine's placeholder
            stats(3, 100_002), // blank
            stats(4, 100_003),
            stats(5, 0),   // names nothing
            stats(6, 555), // an id the FMG does not hold
        ];
        assert_eq!(
            npc_row_names(&rows, &fmg.entries).into_iter().collect::<Vec<_>>(),
            [
                (1, "Genichiro Ashina".to_owned()),
                (4, "Isshin Ashina".to_owned())
            ]
        );
    }

    #[test]
    fn the_fmg_is_found_by_its_binder_id() {
        let file = |id, bytes| BinderFile {
            id: Some(id),
            name: None,
            flags: 0,
            bytes,
        };
        let binder = Binder {
            version: [0; 8],
            format: 0,
            files: vec![
                file(
                    10,
                    build(&[(1, &[Some("a goods name")] as &[Option<&str>])], true),
                ),
                file(18, build(&[(7, &[Some("Wolf")] as &[Option<&str>])], true)),
            ],
        };
        assert_eq!(fmg_of(&binder, 18, "item.msgbnd").unwrap().entries[&7], "Wolf");
        let why = fmg_of(&binder, 328, "item.msgbnd").unwrap_err().to_string();
        assert!(why.contains("item.msgbnd holds no FMG of binder id 328"), "{why}");
    }

    #[test]
    fn each_game_names_the_npc_fmg_of_its_item_binder() {
        let sekiro = npc_name_binders(Game::Sekiro, "engus");
        assert_eq!(sekiro, [("/msg/engus/item.msgbnd.dcx".to_owned(), 18)]);
        let elden = npc_name_binders(Game::EldenRing, "engus");
        assert_eq!(elden[0], sekiro[0]);
        assert_eq!(elden.len(), 3, "the base game's binder, then the two DLC's");
    }
    #[test]
    fn language_is_explicit_and_duplicate_ids_are_reported_with_both_origins() {
        let options = vec!["engus".into(), "jpnjp".into()];
        for language in ["", "unknown", "../engus"] {
            let why = choose_language(language, &options).unwrap_err().to_string();
            assert!(why.contains("engus") && why.contains("jpnjp"));
        }
        assert_eq!(
            npc_name_binders(Game::Sekiro, choose_language("jpnjp", &options).unwrap())[0].0,
            "/msg/jpnjp/item.msgbnd.dcx"
        );
        let (mut text, mut origins, mut report) = (BTreeMap::new(), BTreeMap::new(), Vec::new());
        merge_text(
            &mut text,
            &mut origins,
            "base",
            BTreeMap::from([(7, "A".into()), (8, "B".into())]),
            &mut report,
        );
        merge_text(
            &mut text,
            &mut origins,
            "dlc",
            BTreeMap::from([(7, "A".into()), (8, "C".into())]),
            &mut report,
        );
        assert_eq!(text[&8], "B");
        assert_eq!(report.len(), 2);
        assert!(report[0].contains("id 7: base and dlc; strings identical"));
        assert!(report[1].contains("id 8: base and dlc; strings differ"));
    }
    #[test]
    fn language_options_intersect_dictionary_paths_with_actual_archive_entries() {
        let mut archive = Archive::empty(Game::Sekiro);
        let dictionary = crate::NameDictionary::parse(
            "/msg/engus/item.msgbnd.dcx\n/msg/jpnjp/item.msgbnd.dcx\n/chr/c1010.chrbnd.dcx",
        )
        .unwrap();
        let path = "/msg/jpnjp/item.msgbnd.dcx";
        let hash = archive.hash(path);
        archive.entries.insert(
            hash,
            crate::archive::IndexedEntry {
                shard: 1,
                entry: crate::bhd::Entry {
                    hash,
                    offset: 0,
                    padded_size: 0,
                    logical_size: 0,
                    aes: None,
                    sha: None,
                },
            },
        );
        assert_eq!(message_languages(&archive, &dictionary), ["jpnjp"]);
    }
}
