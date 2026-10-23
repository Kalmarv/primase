//! The Unity scene as a conversion's reader ([`primase_unity::convert::UnityReader`]) for the player
//! `installs.sledding/Sledding Game_Data` read with `tools.unity_tpk`: its read is the level read (the same entities
//! and components as [`primase_unity::level::read_level`] makes, which `hierarchy.rs`, `mesh.rs`, `texture.rs` and the
//! others gate against UnityPy), kept to the namespaces it declares, with the reader's notices the level report's own
//! counts; a part is the closure of its roots with entity ids unchanged, what a load adds stays inside the namespaces,
//! and every prototype with geometry has the box of its mesh.
//!
//! A hand-worked check, not an oracle comparison: the independent comparisons are the gates above.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use primase_core::convert::{NoticeKind, Reader, check_emits, check_load_emits};
use primase_core::{Entity, Instance, World};
use primase_testdata::{Game, Tool, install_file, tool};
use primase_unity::TypeTreePackage;
use primase_unity::convert::UnityReader;
use primase_unity::level::read_level;
use primase_unity::player::Player;

#[test]
fn the_reader_reads_the_level_and_loads_parts_of_it() {
    let (Some(data), Some(tpk)) = (
        install_file(Game::Sledding, "Sledding Game_Data"),
        tool(Tool::UnityTpk),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    // the player's first scene that holds renderers: its second file (the first is the boot scene)
    let level = player
        .scenes()
        .unwrap()
        .get(1)
        .map_or("level1".to_owned(), |s| s.file.clone());

    let mut direct = World::new();
    let level_report = read_level(&player, &level, &mut direct).unwrap();

    let mut reader = UnityReader::new(&player, &level);
    let (world, report) = reader.read().unwrap();
    check_emits(&world, reader.emits(), "read").unwrap();
    assert_eq!(world.entity_count(), direct.entity_count());
    assert_eq!(world.component_names(), direct.component_names());
    assert_eq!(world.resource_names(), direct.resource_names());
    assert!(
        report
            .iter()
            .any(|(label, text)| label == "unity level" && text.starts_with(&level))
    );

    // the notices are the level report's counts: each reason once, with its kind
    let notices = reader.notices();
    let counted =
        |kind: NoticeKind| -> u64 { notices.iter().filter(|n| n.kind == kind).map(|n| n.count).sum() };
    let unsupported: u64 = level_report.textures_not_decoded.values().sum::<u64>()
        + level_report.not_reproduced.values().sum::<u64>()
        + level_report.colliders_not_colliding.values().sum::<u64>();
    assert_eq!(counted(NoticeKind::Unsupported), unsupported);
    assert_eq!(
        counted(NoticeKind::Approximation),
        level_report.notices.values().sum::<u64>()
    );
    assert!(reader.notices().is_empty(), "taken once");
    println!(
        "{level}: {} entities; {} notices ({unsupported} unsupported items)",
        world.entity_count(),
        notices.len()
    );

    // a part: the placements of the first prototype that has geometry, and what they reach
    let proto = world
        .iter::<Instance>()
        .map(|(_, i)| i.prototype)
        .find(|&p| reader.prototype_box(&world, p).is_some())
        .expect("a prototype with geometry");
    let b = reader.prototype_box(&world, proto).unwrap();
    assert!((0..3).all(|i| b[0][i] <= b[1][i] && b[0][i].is_finite() && b[1][i].is_finite()));
    let roots: BTreeSet<Entity> = world
        .iter::<Instance>()
        .filter(|(_, i)| i.prototype == proto)
        .map(|(e, _)| e)
        .collect();
    let (part, _) = reader.load(&world, &roots).unwrap();
    check_load_emits(&world, &part, reader.emits()).unwrap();
    assert_eq!(
        part.entities().collect::<BTreeSet<_>>(),
        world.closure(roots.iter().copied()),
        "a part is the closure of its roots, with the planning world's ids"
    );
    assert!(part.entity_count() < world.entity_count());
    assert!(roots.iter().all(|e| part.has::<Instance>(*e)));
}
