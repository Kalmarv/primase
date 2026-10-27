//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Elden = "elden",
        Gta = "gta",
        Riders = "riders",
        Sekiro = "sekiro",
        Skate = "skate",
        Skatebird = "skatebird",
        Sledding = "sledding",
        Stuntboost = "stuntboost",
    }
    installs {
        Elden {
            about: "Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed)",
            source: "the user's own install",
            gate: false,
        },
        Gta {
            about: "a GTA V install folder (Enhanced or Legacy: the executable and the `.rpf` archives)",
            source: "the user's own install",
            gate: true,
        },
        Riders {
            about: "Riders Republic's folder of `.forge` archives (Steam build 25394987)",
            source: "the user's own install",
            gate: false,
        },
        Sekiro {
            about: "Sekiro's install folder (Data1 to Data5 BHD/BDT pairs, the Oodle 6 DLL)",
            source: "the user's own install",
            gate: true,
        },
        Skate {
            about: "the skate. install the ReSkate tools use (`Data`: its `layout.toc`, TOCs and CAS archives; the Oodle 9 DLL)",
            source: "the user's own install",
            gate: true,
        },
        Skatebird {
            about: "SkateBIRD's install folder (`SkateBIRD_Data`: a Unity 2019.4.20f1 player)",
            source: "the user's own install",
            gate: true,
        },
        Sledding {
            about: "Sledding Game's install folder (`Sledding Game_Data`: a Unity 6000.3 player)",
            source: "the user's own install",
            gate: true,
        },
        Stuntboost {
            about: "STUNTBOOST's install folder (Steam buildid 25832744: `Content/Models/Export`)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {
        Elden {
            about: "the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits (held to the SHA-256 worlds/fromsoft/src/elden/mod.rs states)",
            source: "the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools",
            gate: false,
        },
        Gta {
            about: "the user's GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); never committed",
            source: "the user's own key material, checked against published digests",
            gate: true,
        },
    }
    tools {
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
        UnityTpk = "unity_tpk" {
            about: "a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read",
            source: "input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools)",
            gate: true,
        },
    }
    oracle_sets {
        Baseline = "baseline" {
            about: "our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result)",
            source: "regression baselines: our own earlier outputs, not independent oracles",
        },
        Uxm = "uxm" {
            about: "UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line)",
            source: "third-party: UXM-Selective-Unpack 9501be8's dictionaries",
        },
    }
    declared {
        Query::Install(Game::Skate, Some("Data")) => {
            status: Live,
            gate: true,
            about: r#"the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only"#,
            source: r#"the user's own install"#,
        },
        Query::Install(Game::Skatebird, Some("SkateBIRD_Data")) => {
            status: Live,
            gate: true,
            about: r#"SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs`"#,
            source: r#"the user's own install"#,
        },
        Query::Install(Game::Sledding, Some("Sledding Game_Data")) => {
            status: Live,
            gate: true,
            about: r#"a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files)"#,
            source: r#"the user's own install"#,
        },
        Query::Oracle(OracleSet::Baseline, Game::Sekiro, "scene") => {
            status: Live,
            gate: true,
            about: r#"our earlier exporter's skate-scene folder of an area: scene.json (region, materials), its collision tables (a regression baseline, not an independent reference) and textures; checks/tests/web_texture_decode.rs, worlds/fromsoft/tests/material_assignments.rs, shadow_census.rs"#,
            source: r#"regression baseline: our earlier C# exporter's scene of the area, not an independent reference; placements are checked against the SoulsFormats dump instead"#,
        },
        Query::Oracle(OracleSet::Uxm, Game::Sekiro, "dictionary.txt") => {
            status: Live,
            gate: true,
            about: r#"caller-provided UTF-8 Sekiro path dictionary"#,
            source: r#"third-party: UXM's Sekiro path dictionary"#,
        },
    }
    debug {}
}
