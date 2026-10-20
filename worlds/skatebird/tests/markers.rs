//! SkateBIRD's reset points and mission givers read from the game (`PRIMASE_SKATEBIRD_DATA`, with
//! `PRIMASE_UNITY_TPK`), after the Unity level read, over the levels the game ships. Facts measured independently
//! (UnityPy over the same player): each level stores at most one starting `ResetPosition`, level7 none, seven
//! across the levels; every script's fields decode by the measured layouts (an unknown layout fails the read); every
//! marker has a transform, a name and a source.

#![allow(clippy::unwrap_used)]

use primase_core::{Name, Source, Transform, World};
use primase_skatebird::components::{SkatebirdMissionGiver, SkatebirdResetPosition};
use primase_testdata::real_data;
use primase_unity::TypeTreePackage;
use primase_unity::level::read_level;
use primase_unity::player::Player;

const LEVELS: [&str; 8] = [
    "level1", "level2", "level3", "level4", "level5", "level7", "level12", "level13",
];

#[test]
fn every_level_s_reset_points_and_mission_givers_are_read() {
    let (Some(data), Some(tpk)) = (
        real_data("PRIMASE_SKATEBIRD_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut starting = 0;
    for level in LEVELS {
        let mut w = World::new();
        read_level(&player, level, &mut w).unwrap();
        let r = primase_skatebird::read_markers(&player, level, &mut w).unwrap();
        println!("{level}: {r:?}");
        assert!(r.starting_positions <= 1, "{level}: {r:?}");
        if level == "level7" {
            assert_eq!(r.starting_positions, 0, "level7 stores no starting position");
        }
        starting += r.starting_positions;
        assert_eq!(w.iter::<SkatebirdResetPosition>().count(), r.reset_positions);
        assert_eq!(w.iter::<SkatebirdMissionGiver>().count(), r.mission_givers);
        for (e, _) in w.iter::<SkatebirdResetPosition>() {
            assert!(
                w.has::<Transform>(e) && w.has::<Name>(e) && w.has::<Source>(e),
                "{level}"
            );
        }
        for (e, _) in w.iter::<SkatebirdMissionGiver>() {
            assert!(
                w.has::<Transform>(e) && w.has::<Name>(e) && w.has::<Source>(e),
                "{level}"
            );
        }
    }
    assert_eq!(starting, 7, "one starting position per level but level7");
}
