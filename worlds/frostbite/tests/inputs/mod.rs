//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Skate = "skate",
    }
    installs {
        Skate {
            about: "the skate. install the ReSkate tools use (`Data`: its `layout.toc`, TOCs and CAS archives; the Oodle 9 DLL)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {}
    tools {
        Oodle9 = "oodle9" {
            about: "an Oodle version-9 DLL (`oo2core_9_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
            source: "the game's own DLL from the user's install",
            gate: true,
        },
        ReskateCli = "reskate_cli" {
            about: "ReSkate Studio's CLI executable (`reskate_cli`), an independent EBX reader and reference writer",
            source: "third-party: ReSkate Studio's CLI (20261012)",
            gate: false,
        },
    }
    oracle_sets {}
    declared {
        Query::Install(Game::Skate, Some("Data")) => {
            status: Live,
            gate: true,
            about: r#"the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only"#,
            source: r#"the user's own install"#,
        },
    }
    debug {}
}
