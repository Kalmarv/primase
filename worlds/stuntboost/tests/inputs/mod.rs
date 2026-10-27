//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Stuntboost = "stuntboost",
    }
    installs {
        Stuntboost {
            about: "STUNTBOOST's install folder (Steam buildid 25832744: `Content/Models/Export`)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {}
    tools {}
    oracle_sets {
        GameCode = "game-code" {
            about: "a game's own code run over its install (STUNTBOOST's parsers via reflection: one JSON per level part)",
            source: "independent: the game's own code via reflection, the oracle tool kept with the game's private tools",
        },
    }
    declared {
        Query::Install(Game::Stuntboost, Some("Content/Effects")) => {
            status: Live,
            gate: false,
            about: r#"STUNTBOOST's compiled effects (`D2/*.xnb`, `D3/*.xnb`: MonoGame `EffectReader`, MGFX version 12, OpenGL profile): every one is read to its shaders, parameters and passes; worlds/stuntboost/tests/effects.rs"#,
            source: r#"the user's own install"#,
        },
        Query::Oracle(OracleSet::GameCode, Game::Stuntboost, "levels") => {
            status: Live,
            gate: true,
            about: r#"the game's own parsers run over that install: per level part `<L>_<N>.json` (node names as its `ParsedName` reads them; its collision model as `SE.CollisionModel` loads it: meshes, grippable bits, ramps, ledges) and `<L>_<N>.tris.bin` (every collision triangle); worlds/stuntboost/tests/oracle.rs"#,
            source: r#"independent: the game's own code via reflection, the oracle tool kept with the game's private tools"#,
        },
    }
    debug {}
}
