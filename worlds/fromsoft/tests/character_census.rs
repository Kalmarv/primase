//! A census of what a character's files hold that primase now keeps beyond the model and its clips, for a few characters
//! of each game: the FLVER's dummy polygons (and the sockets they make), the cloth companion files, the event files
//! (`.tae`) against the animations they are numbered as, and the NPC param's descriptive fields. It prints; it asserts
//! only that the reads succeed. The lines:
//!
//! - `REPORT <game> <id> bones .. dummies .. unplaced .. cloth_files .. unread [..]`: what the read found and left;
//! - `EVENTS <game> <id> tae_files .. records .. unmatched .. clips .. without_events .. events .. refused ..`: the
//!   counts of the event read;
//! - `NUMBERS <game> <id> tae [..] clips [..]`: the first animation numbers the event files hold beside the first names
//!   of the animations the binders hold, so the numbering rule (`aGGG_NNNNNN` is `GGG * 1000000 + NNNNNN`) can be read
//!   against both;
//! - `DUMMY <game> <id> reference .. parent <bone> attach <bone> position [..] -> socket on <joint> offset [..]`: the
//!   first dummies with the bones they name, so that a hand's dummy standing on the hand shows;
//! - `NPC <game> <row> ...`: the first rows' descriptive fields.
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
use primase_core::{Skeleton, Sockets, World};
use primase_fromsoft::sekiro::character_data::{Cloth, Dummies, NpcStatRows};
use primase_fromsoft::sekiro::{self, CharacterOptions};
use primase_fromsoft::tae::Tae;
use primase_fromsoft::{Binder, Game, Install, Keys, dcx::Decoder};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

fn census(game: Game, install: &Install, decoder: &Decoder<'_>, ids: &[&str]) -> Result<()> {
    let archive = install.open()?;
    for id in ids {
        let mut world = World::default();
        let options = CharacterOptions {
            animations: Some((*id).to_owned()),
            ..Default::default()
        };
        let report = match sekiro::read_character_with(install, id, decoder, options, &mut world) {
            Ok(report) => report,
            Err(e) => {
                println!("UNREAD {} {id} {e}", game.name());
                continue;
            }
        };
        println!(
            "REPORT {} {id} bones {} dummies {} unplaced {} cloth_files {} unread {:?} npc_stats_unread {:?}",
            game.name(),
            report.bones,
            report.dummies,
            report.dummies_unplaced,
            report.cloth_files,
            report.unread,
            report.npc_stats_unread
        );
        if let Some(a) = &report.animations {
            println!(
                "EVENTS {} {id} tae_files {} records {} unmatched {} clips {} without_events {} events {} refused {:?}",
                game.name(),
                a.tae_files,
                a.tae_animations,
                a.tae_unmatched,
                a.animations,
                a.clips_without_events,
                a.events,
                a.refused
            );
        }
        // the numbers the event files hold beside the names of the animations
        let path = format!("/chr/{id}.anibnd.dcx");
        if archive.contains(&path) {
            let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
            for f in binder
                .files
                .iter()
                .filter(|f| f.name.as_deref().is_some_and(|n| n.to_ascii_lowercase().ends_with(".tae")))
            {
                if let Ok(tae) = Tae::read(&decoder.decode_if_dcx(&f.bytes)?) {
                    let numbers: Vec<i64> = tae.animations.iter().take(10).map(|a| a.id).collect();
                    let clips = sekiro::clip_names(&archive, decoder, id)?
                        .unwrap_or_default()
                        .into_iter()
                        .take(10)
                        .collect::<Vec<_>>();
                    println!(
                        "NUMBERS {} {id} {} event bank {} tae {numbers:?} clips {clips:?}",
                        game.name(),
                        f.name.as_deref().unwrap_or(""),
                        tae.event_bank
                    );
                }
            }
        }
        // the dummies against the bones they name, and the sockets they made
        let dummies: Vec<_> = world.iter::<Dummies>().flat_map(|(_, d)| d.0.clone()).collect();
        for (e, sockets) in world.iter::<Sockets>() {
            let Some(skeleton) = world.get::<Skeleton>(e) else {
                continue;
            };
            let bone = |i: i16| {
                usize::try_from(i)
                    .ok()
                    .and_then(|i| skeleton.joints.get(i))
                    .map_or("none", |j| j.name.as_str())
            };
            for socket in sockets.0.iter().take(12) {
                let d = &dummies[socket.id.0 as usize];
                println!(
                    "DUMMY {} {id} reference {} parent {} attach {} position {:?} -> socket on {} offset {:?}",
                    game.name(),
                    d.reference_id,
                    bone(d.parent_bone),
                    bone(d.attach_bone),
                    d.position,
                    skeleton
                        .joints
                        .iter()
                        .find(|j| j.id == socket.joint)
                        .map_or("?", |j| j.name.as_str()),
                    socket.offset.cols[3]
                );
            }
        }
        for (_, cloth) in world.iter::<Cloth>() {
            for file in &cloth.0 {
                println!(
                    "CLOTH {} {id} {} meshes {} entries {}",
                    game.name(),
                    file.file,
                    file.meshes.len(),
                    file.meshes.iter().map(Vec::len).sum::<usize>()
                );
            }
        }
        if let Some(stats) = world.resource::<NpcStatRows>() {
            for row in stats.0.iter().take(3) {
                println!("NPC {} {} {:?}", game.name(), row.id, row.fields());
            }
            println!("NPC {} rows {}", game.name(), stats.0.len());
        }
    }
    Ok(())
}

#[test]
fn what_the_characters_hold() -> Result<()> {
    let Some(dll) = primase_testdata::oodle(primase_testdata::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data, ids) in [
        (
            Game::Sekiro,
            primase_testdata::Game::Sekiro,
            &["c1010", "c1100", "c1000", "c1320"][..],
        ),
        (
            Game::EldenRing,
            primase_testdata::Game::Elden,
            &["c3000", "c2010", "c4290"][..],
        ),
    ] {
        let (Some(root), keys) = (primase_testdata::install(data), primase_testdata::keys(data)) else {
            continue;
        };
        let keys = match (game, keys) {
            (Game::Sekiro, _) => Keys::published(Game::Sekiro).ok_or("Sekiro's keys")?,
            (_, Some(folder)) => Keys::from_folder(folder, game)?,
            (_, None) => continue,
        };
        let install = Install::new(root, game, keys);
        census(game, &install, &decoder, ids)?;
    }
    Ok(())
}
