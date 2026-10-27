//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        General = "general",
    }
    installs {}
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
    }
    oracle_sets {
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
    }
    declared {
        Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsv") => {
            status: Live,
            gate: true,
            about: r#"TSV: codec, compressed path, oracle decoded path (tab-separated)"#,
            source: r#"independent: the games' own streams (Sekiro's DFLT DCX envelopes, skate.'s zlib CAS blocks, Riders Republic's terrain LZ4 blocks, GTA V's raw deflate archive files, cut by the corpus_* example programs) decoded by Python's zlib and the lz4 package 4.4.5, repos/primase-foundations/tests/oracles/codecs.py"#,
        },
    }
    debug {
        "oodle.dump" => "a folder under the scratch root: the Oodle gate writes there the payload and the game library's output of each stream oozextract refuses or decodes differently",
    }
}
