//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Riders = "riders",
    }
    installs {
        Riders {
            about: "Riders Republic's folder of `.forge` archives (Steam build 25394987)",
            source: "the user's own install",
            gate: false,
        },
    }
    keys {}
    tools {}
    oracle_sets {
        RrAssetStudio = "rrassetstudio" {
            about: "RRAssetStudio's read of every Mesh of a Riders Republic install (`summary.jsonl`, one row per Mesh)",
            source: "third-party: RRAssetStudio v3, run over the user's own install by the primase-oracles harness",
        },
    }
    declared {
        Query::Install(Game::Riders, Some("DataPC_MAD_USNAT.forge")) => {
            status: Live,
            gate: false,
            about: r#"a large Anvil forge of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT.forge`, 15,650,919,308 bytes, 767,463 entries; its tables are read, about 163 MB); https://github.com/Kalmarv/anvil-formats/blob/main/tests/mesh_shape.rs, worlds/anvil/tests/shapes.rs, meshes.rs"#,
            source: r#"the user's own install; two MeshShape bodies' prefixes, section runs and tails hand-checked in a separate read-only decode (games/riders-republic docs/recon/meshshape-second-fixture.md, c068d20)"#,
        },
        Query::Install(Game::Riders, Some("DataPC_MAD_USNAT_patch_01.forge")) => {
            status: Live,
            gate: false,
            about: r#"the patch forge of installs.riders/DataPC_MAD_USNAT.forge (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT_patch_01.forge`, 1,331,014,794 bytes, 320,777 entries); worlds/anvil/tests/meshes.rs"#,
            source: r#"the user's own install; four Mesh bodies' descriptors, buffers and splits hand-checked in a separate read-only decode (games/riders-republic docs/recon/mesh-second-fixture.md, a03c970)"#,
        },
        Query::Install(Game::Riders, Some("DataPC_MAD_WhiteRoom.forge")) => {
            status: Live,
            gate: false,
            about: r#"the base forge of installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge's patch (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom.forge`, 8,951,822 bytes, 388 entries); https://github.com/Kalmarv/anvil-formats/blob/main/tests/data.rs"#,
            source: r#"the user's own install; two resource blocks' sizes, checksums, names and class ids hand-checked in a separate read-only decode (games/riders-republic docs/recon/archive-reader-fit.md, a9f4959)"#,
        },
        Query::Install(Game::Riders, Some("DataPC_MAD_WhiteRoom_patch_01.forge")) => {
            status: Live,
            gate: false,
            about: r#"one Anvil forge archive of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom_patch_01.forge`, 1,021,872 bytes, 337 entries); https://github.com/Kalmarv/anvil-formats/blob/main/tests/forge.rs"#,
            source: r#"the user's own install; rows hand-checked against a separate read-only census of the file"#,
        },
        Query::Oracle(OracleSet::RrAssetStudio, Game::Riders, "meshes") => {
            status: Live,
            gate: false,
            about: r#"RRAssetStudio v3's read of every Mesh in installs.riders (a folder: summary.jsonl, one row per Mesh with counts, submeshes and material ids, and SHA-256s of its positions as f32 and absolute indices as u32, little-endian; sample/; identity.json); worlds/anvil/tests/census.rs, run by hand"#,
            source: r#"a third-party oracle: RRAssetStudio v3 (zip SHA-256 f13edec5...) run over the user's own install by the primase-oracles harness dumps/rras-anvil (d5c48a9); identity.json pins its modules and the forges"#,
        },
    }
    debug {
        "anvil.bake_digest" => "the digest the baked Riders Republic Materials must have",
    }
}
