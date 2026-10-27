//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Gta = "gta",
    }
    installs {
        Gta {
            about: "a GTA V install folder (Enhanced or Legacy: the executable and the `.rpf` archives)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {
        Gta {
            about: "the user's GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); never committed",
            source: "the user's own key material, checked against published digests",
            gate: true,
        },
    }
    tools {}
    oracle_sets {
        CodeWalker = "codewalker" {
            about: "CodeWalker's readings of a GTA V install (archive listings, live-file tables, texture parents, resources, frames, bounds)",
            source: "independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools",
        },
    }
    declared {
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "active.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's live-file tables of that install (JSON; schema in `repos/rage-engine-formats/tests/archive_set.rs`)"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "gtxd.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's texture dictionary parents of that install with their source files (JSON; schema in `repos/rage-engine-formats/tests/gtxd.rs`)"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's listing of every archive of that install (JSON lines; schema in `repos/rage-engine-formats/tests/census.rs`)"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "resources") => {
            status: Live,
            gate: true,
            about: r#"optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `repos/rage-engine-formats/tests/resource_oracle.rs`)"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "ycd") => {
            status: Live,
            gate: false,
            about: r#"a folder holding `dictionaries.jsonl`: an independent reader's decoding of every clip dictionary (`.ycd`) of that install, the CodeWalker harness's `batch` mode over the install's `.ycd` list (`DumpYcd`: per animation its frames, duration, bone ids, and per sequence block and bone track the channel types and SHA-256 of the channel values and of the evaluated vectors); the channel decoder's values are checked against it (needs `installs.gta`, `keys.gta`); `repos/rage-engine-formats/tests/ycd_channels.rs`"#,
            source: r#"independent: CodeWalker.Core 485d56b (GPL-licensed: run as a tool, no code taken), through a thin dump harness kept with the private oracle tools (`batch` mode)"#,
        },
    }
    debug {
        "gta.archive_filter" => "only archives whose path contains this text",
    }
}
