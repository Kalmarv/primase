//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Skatebird = "skatebird",
        Sledding = "sledding",
    }
    installs {
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
        Port = "port" {
            about: "another exporter's output for a game (its level folder, materials joined to their objects, terrain unpacked)",
            source: "another exporter's output: an appearance baseline, not a reading of the source",
        },
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
        Query::Install(Game::Sledding, Some("Sledding Game_Data")) => {
            status: Live,
            gate: true,
            about: r#"a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files)"#,
            source: r#"the user's own install"#,
        },
        Query::Oracle(OracleSet::Port, Game::Sledding, "level1") => {
            status: Live,
            gate: true,
            about: r#"optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`)"#,
            source: r#"another exporter's output: an appearance baseline, not a reading of the source"#,
        },
        Query::Oracle(OracleSet::Port, Game::Sledding, "materials.json") => {
            status: Live,
            gate: true,
            about: r#"optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`)"#,
            source: r#"another exporter's output, joined by a script kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::Port, Game::Sledding, "terrain") => {
            status: Live,
            gate: true,
            about: r#"optional: that exporter's terrain pieces unpacked (raw arrays, `index.json`, its layer means; schema in `worlds/unity/tests/terrain.rs`)"#,
            source: r#"another exporter's terrain, unpacked by a script kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Skatebird, "listing.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.skatebird/SkateBIRD_Data` (JSON; schema in `worlds/unity/tests/listing.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "frames.json") => {
            status: Live,
            gate: true,
            about: r#"an independent reader's (UnityPy) decoding of a few meshes in Unity's own frame (vertices, normals, tangents, UV0, submesh triangles); `worlds/unity/tests/handedness.rs`"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "listing.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.sledding/Sledding Game_Data` (JSON; schema in `worlds/unity/tests/listing.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "materials.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's materials of that folder (JSON; schema in `worlds/unity/tests/material.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "meshes.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's decoded meshes of that folder (JSON; schema in `worlds/unity/tests/mesh.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "samples.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's decoded sample objects of that folder (JSON; schema in `worlds/unity/tests/oracle.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "scan.json") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's scan of a level of that folder (JSON; schema in `worlds/unity/tests/hierarchy.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "shader-programs.json") => {
            status: Live,
            gate: true,
            about: r#"an independent reader's (UnityPy) compiled fragment programs of every shader (per pass the player variants, keywords, program type and code hash, and the constant buffers, textures and samplers); `worlds/unity/tests/program.rs`"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "shader-state.json") => {
            status: Live,
            gate: true,
            about: r#"an independent reader's (UnityPy) shader render state of every material (subshader tags; per pass LightMode, blend factors, zwrite and cull with their bound properties); `worlds/unity/tests/material.rs`"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "sun.json") => {
            status: Live,
            gate: true,
            about: r#"an independent reader's (UnityPy) record of the scan level's RenderSettings sun as stored, the render-pipeline asset classes, the shining directional lights and the main-light choice; `worlds/unity/tests/light.rs`"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::UnityPy, Game::Sledding, "textures") => {
            status: Live,
            gate: true,
            about: r#"optional: an independent reader's decoded textures of that folder (a folder of PNGs and `index.json`; schema in `worlds/unity/tests/texture.rs`)"#,
            source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
        },
    }
    debug {}
}
