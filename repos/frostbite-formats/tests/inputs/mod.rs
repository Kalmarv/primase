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
        Oodle5 = "oodle5" {
            about: "an Oodle version-5 DLL (`oo2core_5_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
            source: "the game's own DLL from the user's install",
            gate: true,
        },
        Oodle6 = "oodle6" {
            about: "an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
            source: "the game's own DLL from the user's install",
            gate: true,
        },
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
    oracle_sets {
        Baseline = "baseline" {
            about: "our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result)",
            source: "regression baselines: our own earlier outputs, not independent oracles",
        },
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
    }
    declared {
        Query::Install(Game::Skate, Some("Data")) => {
            status: Live,
            gate: true,
            about: r#"the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only"#,
            source: r#"the user's own install"#,
        },
        Query::Oracle(OracleSet::Baseline, Game::Skate, "fbmod") => {
            status: Live,
            gate: false,
            about: r#"a folder of `.fbmod` files (searched recursively); each is read, written again byte for byte and every payload decoded (`tools.oodle9` for Oodle blocks); repos/frostbite-formats/tests/fbmod.rs. The costume files among them (top level, with Studio's `<name>.readback.glb` beside them when present) also gate the costume steps with `installs.skate/Data`: worlds/reskate/tests/costume.rs"#,
            source: r#"ReSkate Studio 2026-10-12 `cosmetic-mesh-import` output: our own producer's files, a round-trip baseline, not an oracle"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Skate, "cas") => {
            status: Live,
            gate: true,
            about: r#"directory of indexed individual `.casblock` frames"#,
            source: r#"the game's own CAS frames from the user's install: worlds/reskate/examples/corpus_skate_cas.rs (one Oodle block per frame, 16 entries per superbundle TOC)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Skate, "ebx") => {
            status: Live,
            gate: false,
            about: r#"a folder of EBX documents as the game wrote them, mirroring their asset paths (searched recursively); every field's value must survive a rewrite, and the byte layout is compared with the reference writer; repos/frostbite-formats/tests/ebx.rs"#,
            source: r#"the game's own EBX documents"#,
        },
    }
    debug {}
}
