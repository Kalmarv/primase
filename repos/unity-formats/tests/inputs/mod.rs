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
    tools {}
    oracle_sets {
        UnityPy = "unitypy" {
            about: "UnityPy's readings of a Unity player (object tables, samples, meshes, textures, materials, shaders, frames, listings)",
            source: "independent: UnityPy, through dump scripts kept with the private oracle tools",
        },
    }
    declared {
        Query::Install(Game::Skatebird, Some("SkateBIRD_Data")) => {
            status: Live,
            gate: true,
            about: r#"SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs`"#,
            source: r#"the user's own install"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Skatebird, "census.json") => {
            status: Live,
            gate: true,
            about: r#"an independent UnityPy object-table census of that folder (JSON; schema in `worlds/unity/tests/serialized_census.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
    }
    debug {}
}
