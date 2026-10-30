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
    oracle_sets {
        UnityEditor = "unity-editor" {
            about: "what Unity's own editor makes of bytes cut from a player as stored (a script's steps run by the engine on them)",
            source: "independent: Unity's editor (6000.6.4f1, batchmode), through editor scripts kept with the private oracle tools",
        },
    }
    declared {
        Query::Install(Game::Peak, Some("PEAK_Data")) => {
            status: Live,
            gate: true,
            about: r#"PEAK's data folder (`PEAK_Data`), its scene `level4` read with `tools.unity_tpk`; `worlds/peak/tests/globals.rs`"#,
            source: r#"the user's own install"#,
        },
        Query::Oracle(OracleSet::UnityEditor, Game::Peak, "light-volume-level4") => {
            status: Live,
            gate: true,
            about: r#"level4's light volume as the engine assembles it (`volume_rgba16f.bin`: the RGBAHalf Texture3D's level 0, 166,712,832 bytes, after `SetPixels` and `Apply` of its 572 BC6H and BC4 slices' `GetPixels`, as `Peak.CompressableLightMap.Decompress` does); `worlds/peak/tests/globals.rs`"#,
            source: r#"Unity's editor 6000.6.4f1 (the player is 6000.3.15f1) on the slices cut out as stored by UnityPy, through scripts kept with the private oracle tools"#,
        },
    }
    debug {}
}
