//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Shred = "shred",
    }
    installs {
        Shred {
            about: "Tony Hawk: SHRED's (PS3, 2010) disc folder: `PS3_GAME/USRDIR` with its PAK archives",
            source: "the user's own disc, extracted unchanged",
            gate: true,
        },
    }
    keys {}
    tools {}
    oracle_sets {
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
        Rpcs3 = "rpcs3" {
            about: "RPCS3's RSX shader disassembly, produced by an external batch driver",
            source: "independent: RPCS3's original TaskFP at 979a52415003fd7a74ddd216e1a5782b3731281f, built as a tool; driver and unchanged upstream decoding source in private modding/tools/rpcs3-cg-oracle",
        },
    }
    declared {
        Query::Install(Game::Shred, Some("PS3_GAME/USRDIR/database/main.db")) => {
            status: Live,
            gate: true,
            about: r#"Tony Hawk: SHRED's SQLite 3 database of levels, sessions, goals and drop spots (142 tables); worlds/shred/tests/main_db.rs"#,
            source: r#"the user's own disc, extracted unchanged"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Shred, "main-db.tsv") => {
            status: Live,
            gate: true,
            about: r#"TSV of every table of SHRED's MAIN.DB (sqlite_master first): `@table` and its column names, then one line per row, its rowid and typed values (`n`, `i:`, `r:` big-endian double, `t:` UTF-8 and `b:` as hex); worlds/shred/tests/main_db.rs"#,
            source: r#"independent: Python's standard-library sqlite3 (SQLite's own C library) reading the database read-only, worlds/shred/tests/oracles/sqlite_dump.py"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Shred, "scripts.pak") => {
            status: Live,
            gate: true,
            about: r#"SHRED's scripts archive, decrypted: the PAK (version 4) inside the disc's `SCRIPTS.SDAT`, whose `script\*.lua` entries are Lua 5.1 bytecode (1,278 scripts, 1,459,346 bytes); worlds/shred/tests/level_params_disc.rs"#,
            source: r#"the user's own disc's SCRIPTS.SDAT decrypted outside primase by RPCS3's own decryptor (`rpcs3 --headless --decrypt`, no key in primase); the plaintext PAK is used unchanged"#,
        },
        Query::Oracle(OracleSet::Rpcs3, Game::Shred, "fragment") => {
            status: Live,
            gate: true,
            about: "per-microcode SHA256 fragment disassembly plus occurrence index: 3267 distinct microcodes covering all 181851 packaged fragment programs; worlds/shred/tests/fragment_disc.rs",
            source: "independent: official RPCS3 979a52415003fd7a74ddd216e1a5782b3731281f TaskFP, run by private modding/tools/rpcs3-cg-oracle (build.ps1, build_driver.py, provenance.json); represented operands and control boundaries compared, omitted modifiers and sampling operands checked with hand-worked fixtures; disagreements are settled with fixtures rather than deferring to this tool",
        },
    }
    debug {}
}
