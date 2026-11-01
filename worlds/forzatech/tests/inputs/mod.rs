//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Fh6 = "fh6",
    }
    installs {
        Fh6 {
            about: "Forza Horizon 6's install folder (the folder that holds `media`)",
            source: "the user's own install",
            gate: false,
        },
    }
    keys {}
    tools {}
    oracle_sets {}
    declared {
        Query::Install(Game::Fh6, Some("media/Tracks/Brio")) => {
            status: Live,
            gate: false,
            about: r#"the Tokyo track's folder (`media/Tracks/Brio`: four GeoChunks with their chunk maps and contents lists, `Ribbon_00`'s `.pvs` and template libraries, `bin.zip` with its asset manifest, materials and shaders) and, through it, the install's texture library (`media/_library/Textures.zip` and the `TexturesPG` zips); tests/shibuya.rs"#,
            source: r#"the user's own install"#,
        },
    }
    debug {}
}
