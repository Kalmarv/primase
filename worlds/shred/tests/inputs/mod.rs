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
    }
    declared {
        Query::Install(Game::Shred, Some("PS3_GAME/USRDIR/database/main.db")) => {
            status: Live,
            gate: true,
            about: r#"Tony Hawk: SHRED's SQLite 3 database of levels, sessions, goals and drop spots (142 tables); worlds/shred/tests/main_db.rs"#,
            source: r#"the user's own disc, extracted unchanged"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Shred, "scripts.pak") => {
            status: Live,
            gate: true,
            about: r#"SHRED's scripts archive, decrypted: the PAK (version 4) inside the disc's `SCRIPTS.SDAT`, whose `script\*.lua` entries are Lua 5.1 bytecode (1,278 scripts, 1,459,346 bytes); worlds/shred/tests/level_params_disc.rs"#,
            source: r#"the user's own disc's SCRIPTS.SDAT decrypted outside primase by RPCS3's own decryptor (`rpcs3 --headless --decrypt`, no key in primase); the plaintext PAK is used unchanged"#,
        },
    }
    debug {}
}
