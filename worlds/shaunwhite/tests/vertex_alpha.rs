//! The game's levels: the vertex alpha of blended and alpha-tested materials, with the shader's vertex red read as its
//! tint weight and not as a colour.
#![allow(clippy::unwrap_used, reason = "explicit gate assertions")]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Mesh, Name, Semantic, Stream, World};
use primase_testdata::{Game, install_file};

/// Every primitive's `COLOR_0` alpha, as stored bits, in mesh and primitive order.
fn alphas(w: &World) -> Vec<Vec<u32>> {
    w.iter::<Mesh>()
        .flat_map(|(_, m)| {
            m.primitives()
                .iter()
                .map(|p| p.streams.get(&Semantic::Color(0)).cloned())
        })
        .map(|s| match s {
            Some(Stream::F32x4(c)) => c.iter().map(|v| v[3].to_bits()).collect(),
            Some(Stream::U8x4(c)) => c.iter().map(|v| u32::from(v[3])).collect(),
            Some(Stream::U16x4(c)) => c.iter().map(|v| u32::from(v[3])).collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn run(w: &mut World, pipeline: &str) -> RunReport {
    let mut registry = PassRegistry::new();
    primase_shaunwhite::register(&mut registry);
    let pipeline = registry.parse(pipeline).unwrap();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(w, &pipeline, &mut report)
        .unwrap();
    report
}

fn material(w: &World, name: &str) -> Entity {
    w.iter::<Name>()
        .find(|(e, n)| n.0 == name && w.has::<PbrMaterial>(*e))
        .unwrap_or_else(|| panic!("no material {name}"))
        .0
}

#[test]
fn levels_keep_their_vertex_alpha_and_lose_the_vertex_red_as_a_colour() {
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
    // (streams rewritten, exact, tint dropped by a red of 0, tint applied in full) per level, after the selection dropped
    // the HideAfter materials the restored world discards
    let expected = [
        (80, 32, 2, 8),
        (24, 31, 2, 4),
        (9, 5, 1, 2),
        (17, 24, 1, 4),
        (3, 9, 0, 1),
        (4, 14, 1, 2),
    ];
    for (level, path) in files.iter().enumerate() {
        let shared: Vec<_> = files.iter().filter(|p| *p != path).cloned().collect();
        let (mut w, _) = primase_shaunwhite::read::level::read_level(path, &shared).unwrap();
        run(
            &mut w,
            "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=keep,influence=restored},\
             lower-shaunwhite-water{roughness=0.05},lower-shaunwhite-layers{influence=restored},\
             lower-shaunwhite-material",
        );
        let stored = alphas(&w);
        let report = run(&mut w, "lower-shaunwhite-vertex-alpha");
        assert_eq!(alphas(&w), stored, "level {}: an alpha changed", level + 1);
        let log = &report.steps[0].log;
        let count = |k: &str| log.counts.get(k).copied().unwrap_or(0);
        let full = log.approximated_on.values().map(|v| v.len() as u64).sum::<u64>();
        assert!(
            !log.counts.keys().any(|k| k.contains("left as stored")),
            "level {}",
            level + 1
        );
        assert_eq!(
            (
                count("COLOR_0 streams rewritten to vertex alpha alone"),
                count("materials whose vertex alpha is exact"),
                count("materials whose tint the shader's vertex red of 0 removes (tint set to white)"),
                full
            ),
            expected[level],
            "level {}",
            level + 1
        );
        if level == 0 {
            // the lab's three: a red of 0 drops the tint, a red between keeps it
            let crack = w
                .get::<PbrMaterial>(material(&w, "Gen_Decal_CrackTiles01_Mat"))
                .unwrap();
            assert_eq!(crack.base_color[..3], [1.0; 3]);
            let fence = w
                .get::<PbrMaterial>(material(&w, "Gen_Metal_ChainLinkFence01_Mat"))
                .unwrap();
            assert!(fence.base_color[0] < 0.6);
            let named: Vec<String> = log
                .approximated_on
                .values()
                .flatten()
                .filter_map(|e| w.get::<Name>(*e).map(|n| n.0.clone()))
                .collect();
            for n in ["Gen_Glass_Dirty01_Mat", "Gen_Metal_ChainLinkFence01_Mat"] {
                assert!(named.iter().any(|m| m == n), "{n}: {named:?}");
            }
        }
        // idempotent
        let again = run(&mut w, "lower-shaunwhite-vertex-alpha");
        assert_eq!(
            again.steps[0]
                .log
                .counts
                .get("COLOR_0 streams rewritten to vertex alpha alone")
                .copied()
                .unwrap_or(0),
            0
        );
    }
}
