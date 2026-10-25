//! The game's own text: the names it shows for what its params describe. An NPC param row states a `nameId`
//! ([`crate::param::NpcStats`]); the item message binder's NPC-name FMG ([`crate::fmg`]) holds the string for it, in the
//! language read ([`LANGUAGE`]). This is the game's statement of what a character is called, the first source of a name
//! (a community list's guesses come after it).
use std::collections::BTreeMap;

use crate::fmg::Fmg;
use crate::param::NpcStats;
use crate::{Archive, Binder, Game, Install, Result, dcx::Decoder, invalid};

const FORMAT: &str = "FromSoftware messages";

/// The language of the message binders read (`msg/<language>/`).
pub const LANGUAGE: &str = "engus";

/// The message binders of `game` that hold NPC names, and in each the binder id of its NPC-name FMG
/// (Smithbox's `SoulsFmg` table: `NpcName` is 18 in both games' `item.msgbnd`, Elden Ring's DLC binders hold theirs
/// beside it). The first binder is the game's own and must be there; the others are added content a install may lack.
fn npc_name_binders(game: Game) -> Vec<(String, i32)> {
    let binder = |name: &str| format!("/msg/{LANGUAGE}/{name}.msgbnd.dcx");
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
pub fn npc_names(install: &Install, archive: &Archive, decoder: &Decoder<'_>) -> Result<BTreeMap<i32, String>> {
    let param = super::params::read_param(install, archive, decoder, "NpcParam.param")?;
    let stats = NpcStats::from_param(&param, &install.game.npc_stats())?;
    let mut text = BTreeMap::new();
    for (position, (path, id)) in npc_name_binders(install.game).into_iter().enumerate() {
        if !archive.contains(&path) {
            if position == 0 {
                return Err(invalid(FORMAT, 0, format!("{path} is not in the archives")));
            }
            continue;
        }
        let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
        for (id, string) in fmg_of(&binder, id, &path)?.entries {
            text.entry(id).or_insert(string);
        }
    }
    Ok(npc_row_names(&stats, &text))
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
            &[(100_000, &[Some("Genichiro Ashina"), Some("%null%"), Some("  "), Some("Isshin Ashina")] as &[Option<&str>])],
            true,
        ))
        .unwrap();
        let rows = [
            stats(1, 100_000),
            stats(2, 100_001), // the engine's placeholder
            stats(3, 100_002), // blank
            stats(4, 100_003),
            stats(5, 0),       // names nothing
            stats(6, 555),     // an id the FMG does not hold
        ];
        assert_eq!(
            npc_row_names(&rows, &fmg.entries).into_iter().collect::<Vec<_>>(),
            [(1, "Genichiro Ashina".to_owned()), (4, "Isshin Ashina".to_owned())]
        );
    }

    #[test]
    fn the_fmg_is_found_by_its_binder_id() {
        let file = |id, bytes| BinderFile { id: Some(id), name: None, flags: 0, bytes };
        let binder = Binder {
            version: [0; 8],
            format: 0,
            files: vec![
                file(10, build(&[(1, &[Some("a goods name")] as &[Option<&str>])], true)),
                file(18, build(&[(7, &[Some("Wolf")] as &[Option<&str>])], true)),
            ],
        };
        assert_eq!(fmg_of(&binder, 18, "item.msgbnd").unwrap().entries[&7], "Wolf");
        let why = fmg_of(&binder, 328, "item.msgbnd").unwrap_err().to_string();
        assert!(why.contains("item.msgbnd holds no FMG of binder id 328"), "{why}");
    }

    #[test]
    fn each_game_names_the_npc_fmg_of_its_item_binder() {
        let sekiro = npc_name_binders(Game::Sekiro);
        assert_eq!(sekiro, [("/msg/engus/item.msgbnd.dcx".to_owned(), 18)]);
        let elden = npc_name_binders(Game::EldenRing);
        assert_eq!(elden[0], sekiro[0]);
        assert_eq!(elden.len(), 3, "the base game's binder, then the two DLC's");
    }
}
