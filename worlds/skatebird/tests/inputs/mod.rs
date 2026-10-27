//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Skatebird = "skatebird",
    }
    installs {
        Skatebird {
            about: "SkateBIRD's install folder (`SkateBIRD_Data`: a Unity 2019.4.20f1 player)",
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
        Query::Install(Game::Skatebird, Some("SkateBIRD_Data")) => {
            status: Live,
            gate: true,
            about: r#"SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs`"#,
            source: r#"the user's own install"#,
        },
    }
    debug {
        "skatebird.voronoi_digest" => "the digest the Voronoi bake test's maps must have",
        "skatebird.voronoi_level" => "the SkateBIRD level the Voronoi bake test bakes (default `level1`)",
    }
}
