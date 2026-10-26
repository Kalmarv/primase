//! Level 01 of the game: the metro's blue arrow is marked, its tile decal is gone (a HideAfter material of the restored
//! world), glass and the window glow are not marked.
#![allow(clippy::unwrap_used, reason = "explicit gate assertions")]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{DrawLayer, DrawOrder, MaterialUsage, Name};
use primase_testdata::{Game, install_file};

#[test]
fn level_one_marks_the_metro_arrow_and_not_its_glass() {
    let files = [
        install_file(Game::ShaunWhite, "DataPC_01.forge"),
        install_file(Game::ShaunWhite, "DataPC_02.forge"),
        install_file(Game::ShaunWhite, "DataPC_03.forge"),
        install_file(Game::ShaunWhite, "DataPC_04.forge"),
        install_file(Game::ShaunWhite, "DataPC_05.forge"),
        install_file(Game::ShaunWhite, "DataPC_06.forge"),
    ];
    let Some(files) = files.into_iter().collect::<Option<Vec<_>>>() else {
        return;
    };
    let shared: Vec<_> = files[1..].to_vec();
    let (mut w, _) = primase_shaunwhite::read::level::read_level(&files[0], &shared).unwrap();
    let mut registry = PassRegistry::new();
    primase_shaunwhite::register(&mut registry);
    let pipeline = registry
        .parse(
            "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=keep,influence=restored},\
             lower-shaunwhite-water{roughness=0.05},lower-shaunwhite-layers{influence=restored},\
             lower-shaunwhite-material,mark-shaunwhite-decals{gap=0.005,area=1,share=0.1}",
        )
        .unwrap();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(&mut w, &pipeline, &mut report)
        .unwrap();
    let marked: Vec<String> = w
        .iter::<Name>()
        .filter(|(e, _)| DrawOrder::of(&w, *e).layer == DrawLayer::Decal && w.has::<DrawOrder>(*e))
        .map(|(_, n)| n.0.clone())
        .collect();
    for decal in [
        "Gen_Grafitti_SingleArrow_Blue01_Mat",
        "Gen_Street_LinePieton01_Mat",
    ] {
        assert!(
            marked.iter().any(|n| n == decal),
            "{decal} is not marked: {marked:?}"
        );
    }
    // a HideAfter material is discarded by the shader at the restored world's influence: the selection dropped it
    assert!(
        !w.iter::<Name>()
            .any(|(_, n)| n.0.contains("01_Influence_MetroTilesDecal_Mat")),
        "the metro tile decal is still in the world"
    );
    for not in [
        "Gen_Glass01_Mat",
        "Gen_Glass_Dirty01_Mat",
        "Gen_Window_ShellLight01_Mat",
    ] {
        assert!(!marked.iter().any(|n| n == not), "{not} is marked");
    }
    assert!(
        w.iter::<Name>()
            .filter(|(e, _)| w.has::<MaterialUsage>(*e))
            .all(|(e, _)| DrawOrder::of(&w, e).layer != DrawLayer::Decal),
        "water or vegetation marked"
    );
    assert_eq!(marked.len(), 48, "{marked:?}");
}
