//! Levels 02 and 03 of the game: dropping the effects removes level 02's light-paint ribbons and keeps level 03's water.
#![allow(clippy::unwrap_used, reason = "explicit gate assertions")]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Instance, Mesh, Name, World};
use primase_shaunwhite::components::MaterialRef;
use primase_testdata::{Game, install_file};

const FILES: [&str; 6] = [
    "DataPC_01.forge",
    "DataPC_02.forge",
    "DataPC_03.forge",
    "DataPC_04.forge",
    "DataPC_05.forge",
    "DataPC_06.forge",
];

/// The names of the placed meshes of `w`.
fn placed(w: &World) -> Vec<String> {
    w.iter::<Instance>()
        .filter_map(|(_, i)| w.get::<Name>(i.prototype).map(|n| n.0.clone()))
        .collect()
}

/// The materials a placed mesh of `w` draws.
fn drawn(w: &World) -> Vec<String> {
    w.iter::<Instance>()
        .filter_map(|(_, i)| w.get::<Mesh>(i.prototype))
        .flat_map(|m| m.primitives().iter().filter_map(|p| p.material))
        .filter_map(|m| w.get::<MaterialRef>(m).map(|r| r.name.clone()))
        .collect()
}

/// Reads level `n` (1 to 6) with the other five as its shared forges, and drops the effects of what it placed.
fn level(n: usize, effects: &str) -> Option<World> {
    let files: Vec<_> = FILES
        .into_iter()
        .map(|f| install_file(Game::ShaunWhite, f))
        .collect::<Option<Vec<_>>>()?;
    let shared: Vec<_> = files
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != n - 1)
        .map(|(_, p)| p.clone())
        .collect();
    let (mut w, _) = primase_shaunwhite::read::level::read_level(&files[n - 1], &shared).unwrap();
    let mut registry = PassRegistry::new();
    primase_shaunwhite::register(&mut registry);
    let pipeline = registry
        .parse(&format!(
            "select-shaunwhite-content{{helpers=keep,effects={effects},characters=keep,missions=keep,influence=restored}}"
        ))
        .unwrap();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(&mut w, &pipeline, &mut report)
        .unwrap();
    Some(w)
}

#[test]
fn level_two_drops_its_light_paint_ribbons_and_level_three_keeps_its_water() {
    let ribbons = ["02B_GFX_LPYellow", "02B_GFX_LPMauve", "02B_GFX_LPBlue"];
    let (Some(kept), Some(dropped)) = (level(2, "keep"), level(2, "drop")) else {
        return;
    };
    let (kept, dropped) = (placed(&kept), placed(&dropped));
    for ribbon in ribbons {
        assert_eq!(
            kept.iter().filter(|n| *n == ribbon).count(),
            1,
            "{ribbon} is placed once"
        );
        assert!(!dropped.iter().any(|n| n == ribbon), "{ribbon} is dropped");
    }
    let (Some(three_kept), Some(three)) = (level(3, "keep"), level(3, "drop")) else {
        return;
    };
    let water = ["03_GFX_Water_Ocean", "03_GFX_Water_PirateShip"];
    let (before, after) = (drawn(&three_kept), drawn(&three));
    for material in water {
        assert!(
            before.iter().any(|m| m == material),
            "{material} is drawn at the start"
        );
        assert!(
            after.iter().any(|m| m == material),
            "{material} is not drawn after the effects drop"
        );
    }
}
