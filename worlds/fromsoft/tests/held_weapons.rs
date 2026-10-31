//! Weapons held where the game holds them: the player's parts read with a weapon row in the right hand, the character's
//! own model's dummy polygons placed on them (where weapons stand, with Elden Ring's body part's in the 2000s), the body
//! assembled without the weapon (`leave-out=`), every asset placed, and `hold-fromsoft-weapons{grip=one-handed}`
//! attaching each weapon model's rig to the dummy polygon its `WepAbsorpPosParam` row names. Sekiro's Kusabimaru (row
//! 8000000, hold row 5000): the blade (model 0) at dummy polygon 20 (the right hand's `R_Weapon`), the scabbard (model 1)
//! at 147 (`Sheath`), model 2 at 149 (`LargeSheath`). Elden Ring's Uchigatana (9000000, hold row 29): the blade at 20, the
//! scabbard, with the sword drawn, at 2069: the body part `bd_m_1360_l`'s polygon 69, on the left hip. Every weapon's model 0
//! has polygon 301, the socket `dummy 10301` its effects name. Elden Ring's Caestus (21000000, hold row 42) held in both hands
//! (`grip=two-handed-right`): its second fist (model 1, at 2117, which no body part has, shown in the left hand) on the
//! character's own polygon 117, on the left hand, where the game draws it. Elden Ring's Leontiel's Greatsword (3560000,
//! hold row 250) in one hand: the blade at 20, model 2 (a rolled cloth) at 2030, the body part's polygon 30 on the left
//! hip, where the game draws it. Model 1 (at 2116, shown on the body, which no worn part has) is omitted with a named
//! placement refusal: its absence in the captured idle holds does not establish a general visibility rule.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}` and `tools.oodle6`; a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::{Game, Install, Keys, dcx::Decoder};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Asset, Attachment, Name, RigInstance, Role, Skeleton, Socket, Sockets, Source, World};
use primase_fromsoft::sekiro::effect_data::{EquippedWeapons, HeldWeapon};
use primase_fromsoft::sekiro::{self, CharacterOptions};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The weapon models' attachments: (model, the socket's name), by model.
fn held(w: &World) -> Vec<(u8, Option<String>)> {
    let mut out: Vec<(u8, Option<String>)> = w
        .iter::<HeldWeapon>()
        .map(|(root, h)| {
            let skeleton = w
                .get::<Asset>(root)
                .and_then(|a| a.members.iter().find(|m| m.1 == Role::Rig))
                .map(|m| m.0);
            let socket = w
                .iter::<RigInstance>()
                .find(|(_, r)| Some(r.skeleton) == skeleton)
                .and_then(|(rig, _)| w.get::<Attachment>(rig))
                .and_then(|a| {
                    let holder = w.get::<RigInstance>(a.rig)?.skeleton;
                    let s: &Socket = w.get::<Sockets>(holder)?.0.iter().find(|s| s.id == a.socket)?;
                    Some(s.name.clone())
                });
            (h.model, socket)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_player_holds_a_weapon_at_the_dummy_polygons_its_rows_name() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    // (game, its data, the parts worn, the weapon row, the grip, each model's socket, the models left out)
    for (game, data, parts, row, grip, expected, left_out) in [
        (
            Game::Sekiro,
            Data::Sekiro,
            &["fc_m_0200_l", "bd_m_9040_l", "am_m_9000_l", "lg_m_9000_l"][..],
            8_000_000,
            "one-handed",
            vec![
                (0, Some("dummy 20".to_owned())),
                (1, Some("dummy 147".to_owned())),
                (2, Some("dummy 149".to_owned())),
            ],
            0,
        ),
        (
            Game::EldenRing,
            Data::Elden,
            &["hd_m_1360_l", "bd_m_1360_l", "am_m_1360_l", "lg_m_1360_l"][..],
            9_000_000,
            "one-handed",
            vec![
                (0, Some("dummy 20".to_owned())),
                (1, Some("dummy 2069".to_owned())),
            ],
            0,
        ),
        (
            Game::EldenRing,
            Data::Elden,
            &["hd_m_1360_l", "bd_m_1360_l", "am_m_1360_l", "lg_m_1360_l"][..],
            21_000_000,
            "two-handed-right",
            vec![
                (0, Some("dummy 115".to_owned())),
                (1, Some("dummy 117".to_owned())),
            ],
            0,
        ),
        (
            Game::EldenRing,
            Data::Elden,
            &["hd_m_1360_l", "bd_m_1360_l", "am_m_1360_l", "lg_m_1360_l"][..],
            3_560_000,
            "one-handed",
            vec![
                (0, Some("dummy 20".to_owned())),
                (2, Some("dummy 2030".to_owned())),
            ],
            1,
        ),
    ] {
        let (Some(root), Some(folder)) = (install(data), keys(data)) else {
            continue;
        };
        let installed = Install::new(root, game, Keys::from_folder(folder, game, None)?);
        // read directly, and through the reader a program uses (which lists the clips apart)
        for through_reader in [false, true] {
            let mut w = if through_reader {
                let mut reader = sekiro::CharacterReader::parts(installed.clone(), parts, decoder)
                    .with_animations("c0000")
                    .with_clip_effects()
                    .with_equipped_weapons(EquippedWeapons::right_hand(row));
                primase_core::convert::Reader::read(&mut reader)
                    .map_err(|e| e.to_string())?
                    .0
            } else {
                let options = CharacterOptions {
                    animations: Some("c0000".to_owned()),
                    animation_clips: Some(Vec::new()),
                    clip_effects: true,
                    equipped_weapons: EquippedWeapons::right_hand(row),
                    ..CharacterOptions::default()
                };
                let mut w = World::new();
                sekiro::read_parts_with(&installed, parts, &decoder, &options, &mut w)?;
                w
            };
            // the weapon's models are marked held
            let held_sources: Vec<String> = w
                .iter::<HeldWeapon>()
                .filter_map(|(root, _)| w.get::<Source>(root).map(|s| s.0.clone()))
                .collect();
            assert_eq!(
                held_sources.len(),
                expected.len() + left_out,
                "{game:?}: {held_sources:?}"
            );
            let mut registry = PassRegistry::new();
            sekiro::passes::register(&mut registry);
            primase_animation::register(&mut registry);
            registry.add(&primase_core::PLACE_ASSETS);
            let m = PassManager::new(registry);
            let pipeline = format!(
                "lower-fromsoft-geometry{{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}}, \
             assemble-character{{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=assembled,leave-out={}}}, \
             place-assets, hold-fromsoft-weapons{{grip={grip}}}",
                held_sources.join("+")
            );
            let mut run = RunReport::default();
            m.run(&mut w, &m.registry.parse(&pipeline)?, &mut run)?;
            assert_eq!(held(&w), expected, "{game:?} {row}");
            // a blade's polygon 301 is the socket its effects name, on the blade's own skeleton
            let blade_sockets = w
                .iter::<HeldWeapon>()
                .filter(|(_, h)| h.model == 0)
                .find_map(|(root, _)| {
                    let rig = w.get::<Asset>(root)?.members.iter().find(|m| m.1 == Role::Rig)?.0;
                    w.get::<Sockets>(rig)
                });
            assert!(
                blade_sockets.is_some_and(|s| s.0.iter().any(|k| k.name == "dummy 10301")),
                "{game:?} {row}"
            );
            // Missing body sockets are an implementation refusal, not an inferred native visibility rule.
            let hold = run.steps.last().ok_or("no steps")?;
            assert_eq!(
                hold.log.counts.get("held weapon models with unresolved placement at a missing body socket, omitted by name").copied().unwrap_or(0),
                left_out as u64,
                "{game:?} {row}"
            );
            if left_out != 0 {
                assert!(
                    hold.log
                        .omitted
                        .keys()
                        .any(|why| why.contains("2116") && why.contains("placement refused")),
                    "{:?}",
                    hold.log.omitted
                );
            }
            // the character's skeleton is one, with its sockets
            let assembled = w
                .iter::<Skeleton>()
                .find(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == "assembled skeleton"))
                .map(|(e, _)| e)
                .ok_or("no assembled skeleton")?;
            assert!(
                w.get::<Sockets>(assembled)
                    .is_some_and(|s| s.0.iter().any(|k| k.name == "dummy 20"))
            );
            assert!(
                w.verify_components().is_empty(),
                "{game:?}: {:?}",
                w.verify_components()
            );
        }
    }
    Ok(())
}
