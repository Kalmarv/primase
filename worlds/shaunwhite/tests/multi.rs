//! The four multiplayer worlds (`installs.shaunwhite`): each root entry's one player spawner is read as a start marker.
//! Translations are those of the Entities' matrices (ENU: east, north, up), read by a separate census of the forges' root
//! entries.
use primase_core::frame::SourceFrame;
use primase_core::{Marker, Name, Transform};
use primase_shaunwhite::read::level::read_level;
use primase_testdata::{Game, install};

#[test]
fn each_multiplayer_world_has_its_player_spawner_as_a_start() {
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    for (forge, name, at) in [
        (
            "DataPC_02_Multi.forge",
            "MULTI_Spawner_Player_001",
            [-112.10, -74.34, 22.93],
        ),
        (
            "DataPC_03_Multi.forge",
            "MULTI_Spawner_Player_001",
            [-63.00, 58.00, 10.45],
        ),
        (
            "DataPC_04_Multi.forge",
            "04_MULTI_Spawner_Player_001",
            [0.38, -62.76, 0.0],
        ),
        (
            "DataPC_05_Multi.forge",
            "MULTI_Spawner_Player_001",
            [64.68, 97.42, 3.10],
        ),
    ] {
        let (w, _) = read_level(&folder.join(forge), &[]).unwrap();
        let starts: Vec<_> = w
            .iter::<Name>()
            .filter(|(e, _)| w.get::<Marker>(*e).is_some_and(|m| m.kind == "start"))
            .map(|(e, n)| (e, n.0.clone()))
            .collect();
        println!(
            "{forge}: starts {:?}",
            starts.iter().map(|(_, n)| n).collect::<Vec<_>>()
        );
        let (e, _) = starts
            .iter()
            .find(|(_, n)| n == name)
            .unwrap_or_else(|| panic!("{forge}: no start {name}: {starts:?}"));
        let want = SourceFrame::ENU.point(at);
        let got = w.get::<Transform>(*e).unwrap().cols[3];
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01),
            "{forge}: {got:?} against {want:?}"
        );
    }
}
