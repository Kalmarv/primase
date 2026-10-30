//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Peak = "peak",
    }
    installs {
        Peak {
            about: "PEAK's install folder (`PEAK_Data`: a Unity 6000.3.15f1 player with level AssetBundles)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {}
    tools {
        UnityTpk = "unity_tpk" {
            about: "a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read",
            source: "input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools)",
            gate: true,
        },
    }
    oracle_sets {}
    declared {
        Query::Install(Game::Peak, Some("PEAK_Data")) => {
            status: Live,
            gate: true,
            about: r#"PEAK's data folder (`PEAK_Data`), its scene `level4` read with `tools.unity_tpk`; `worlds/peak/tests/globals.rs`"#,
            source: r#"the user's own install"#,
        },
    }
    debug {}
}
