//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Sekiro = "sekiro",
        Elden = "elden",
    }
    installs {}
    keys {}
    tools {}
    oracle_sets {
        Capture = "capture" {
            about: "cloth operator inputs and outputs recorded from the running games",
            source: "the user's offline game, observed around cloth calls",
        },
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
    }
    declared {
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-cloth") => {
            status: Live,
            gate: true,
            about: "cloth tagfiles named by the captured steps",
            source: "the game files extracted as stored",
        },
        Query::Oracle(OracleSet::Capture, Game::Sekiro, "havok-cloth") => {
            status: Live,
            gate: true,
            about: "cloth steps and captures.tsv identifying their installed definitions",
            source: "the recorder observing the running game",
        },
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-cloth") => {
            status: Live,
            gate: true,
            about: "cloth tagfiles named by the captured steps",
            source: "the game files extracted as stored",
        },
        Query::Oracle(OracleSet::Capture, Game::Elden, "havok-cloth") => {
            status: Live,
            gate: true,
            about: "cloth steps and captures.tsv identifying their installed definitions",
            source: "the recorder observing the running game",
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "object-hkx") => {
            status: Live,
            gate: true,
            about: r#"a folder of Havok tagfiles holding object convex bodies, checked against their stored half-spaces; `toolkit/havok/tests/convex_corpus.rs`"#,
            source: r#"the game's own tagfiles, checked against their own stored half-spaces"#,
        },
    }
    debug {}
}
