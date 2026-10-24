//! A part binder of several models reads each as an asset of its own, named after its file: Sekiro's Loaded Umbrella
//! (`wp_a_0761_l`: the umbrella closed and open, each with an animation binder of its own) and Elden Ring's
//! `wp_a_1152_l` (one mesh in two materials). Each form's other members go with it: its animation binder is its
//! asset's clips when they are asked for, and is named on its report as not read otherwise.
use std::collections::BTreeMap;

use primase_core::{Asset, Name, Role, Source, World};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::sekiro::{CharacterOptions, CharacterReport, part_forms, read_parts_with};
use primase_fromsoft::{Game, Install, Keys};
use primase_testdata::Tool;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// Each asset of `world` by name: its source and its members' roles.
fn assets(world: &World) -> BTreeMap<String, (String, Vec<Role>)> {
    world
        .iter::<Asset>()
        .map(|(root, a)| {
            (
                world.get::<Name>(root).map(|n| n.0.clone()).unwrap_or_default(),
                (
                    world.get::<Source>(root).map(|s| s.0.clone()).unwrap_or_default(),
                    a.members.iter().map(|m| m.1).collect(),
                ),
            )
        })
        .collect()
}

/// A report's name, binder, forms, bones, meshes and unread members.
type Form<'r> = (&'r str, &'r str, usize, usize, usize, Vec<&'r str>);

/// Each report's [`Form`].
fn forms(reports: &[CharacterReport]) -> Vec<Form<'_>> {
    reports
        .iter()
        .map(|r| {
            (
                r.id.as_str(),
                r.binder.as_str(),
                r.forms,
                r.bones,
                r.meshes,
                r.unread.iter().map(String::as_str).collect(),
            )
        })
        .collect()
}

#[test]
fn a_part_binders_forms_are_assets_of_their_own() -> Result<()> {
    let Some(dll) = primase_testdata::tool(Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    if let Some(root) = primase_testdata::install(primase_testdata::Game::Sekiro) {
        let install = Install::sekiro(&root);
        let archive = install.open()?;
        assert_eq!(
            part_forms(&archive, &decoder, "wp_a_0761_l")?,
            ["wp_a_0761_l", "wp_a_0761_1_l"]
        );
        // a binder of textures alone is refused, naming them
        let refused = part_forms(&archive, &decoder, "fc_m_0000")
            .err()
            .ok_or("fc_m_0000 holds no FLVER")?;
        assert!(
            refused
                .to_string()
                .contains("holds no model (no FLVER): it holds FC_M_0000.tpf"),
            "{refused}"
        );
        // the animation binders unread: each named on its own form's report
        let mut world = World::new();
        let (reports, _) = read_parts_with(
            &install,
            &["wp_a_0761_l"],
            &decoder,
            &CharacterOptions::default(),
            &mut world,
        )?;
        assert_eq!(
            forms(&reports),
            [
                (
                    "wp_a_0761_l",
                    "parts/wp_a_0761_l",
                    2,
                    64,
                    1,
                    vec!["wp_a_0761.anibnd"]
                ),
                (
                    "wp_a_0761_1_l",
                    "parts/wp_a_0761_l",
                    2,
                    87,
                    2,
                    vec!["wp_a_0761_1.anibnd"]
                ),
            ]
        );
        let by_name = assets(&world);
        assert_eq!(
            by_name
                .iter()
                .map(|(n, (s, _))| (n.as_str(), s.as_str()))
                .collect::<Vec<_>>(),
            [
                ("wp_a_0761_1_l", "parts/wp_a_0761_l/wp_a_0761_1_l"),
                ("wp_a_0761_l", "parts/wp_a_0761_l"),
            ]
        );
        assert!(
            by_name
                .values()
                .all(|(_, roles)| roles == &[Role::Rig, Role::Part])
        );

        // the animation binders read: each its form's clips
        let mut world = World::new();
        let (reports, _) = read_parts_with(
            &install,
            &["wp_a_0761_l"],
            &decoder,
            &CharacterOptions {
                form_animations: true,
                ..CharacterOptions::default()
            },
            &mut world,
        )?;
        let by_name = assets(&world);
        // the closed umbrella's one animation, the open one's 25, each over a Havok skeleton of 76 bones
        let read: Vec<(&str, usize, usize)> = reports
            .iter()
            .filter_map(|r| {
                r.animations
                    .as_ref()
                    .map(|a| (r.id.as_str(), a.animations, a.bones))
            })
            .collect();
        assert_eq!(read, [("wp_a_0761_l", 1, 76), ("wp_a_0761_1_l", 25, 76)]);
        for r in &reports {
            let read = r
                .animations
                .as_ref()
                .ok_or("the form's animations were asked for")?;
            println!(
                "{}: {} animations, {} Havok bones, refused {:?}, unread {:?}",
                r.id, read.animations, read.bones, read.refused, read.unread
            );
            assert!(r.unread.is_empty(), "{}: {:?}", r.id, r.unread);
            assert!(read.bones > 0, "{}: a Havok skeleton", r.id);
            let clips = by_name[&r.id]
                .1
                .iter()
                .filter(|role| **role == Role::Clip)
                .count();
            assert_eq!(clips, read.animations, "{}: its clips are its asset's", r.id);
        }
    }
    if let (Some(root), Some(keys)) = (
        primase_testdata::install(primase_testdata::Game::Elden),
        primase_testdata::keys(primase_testdata::Game::Elden),
    ) {
        let install = Install::new(root, Game::EldenRing, Keys::from_folder(keys, Game::EldenRing)?);
        let refused = part_forms(&install.open()?, &decoder, "fg_a_6000")
            .err()
            .ok_or("fg_a_6000 holds no FLVER")?;
        assert!(
            refused
                .to_string()
                .contains("holds no model (no FLVER): it holds FG_A_6000.tpf"),
            "{refused}"
        );
        let mut world = World::new();
        let (reports, _) = read_parts_with(
            &install,
            &["wp_a_1152_l"],
            &decoder,
            &CharacterOptions::default(),
            &mut world,
        )?;
        assert_eq!(
            forms(&reports),
            [
                ("wp_a_1152_l", "parts/wp_a_1152_l", 2, 4, 1, vec![]),
                ("wp_a_1152_1_l", "parts/wp_a_1152_l", 2, 3, 1, vec![]),
            ]
        );
        assert_eq!(assets(&world).len(), 2);
    }
    Ok(())
}
