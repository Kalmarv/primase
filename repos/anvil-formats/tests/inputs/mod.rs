//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Riders = "riders",
        ShaunWhite = "shaunwhite",
    }
    installs {
        Riders {
            about: "Riders Republic's folder of `.forge` archives (Steam build 25394987)",
            source: "the user's own install",
            gate: false,
        },
        ShaunWhite {
            about: "Shaun White Skateboarding's (PC, 2010) folder of `.forge` archives, version 26",
            source: "the user's own copy: the forges as the release's archives hold them, extracted unchanged",
            gate: false,
        },
    }
    keys {}
    tools {}
    oracle_sets {}
    declared {
        Query::Install(Game::Riders, Some("DataPC_MAD_USNAT.forge")) => {
            status: Live,
            gate: false,
            about: r#"a large Anvil forge of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT.forge`, 15,650,919,308 bytes, 767,463 entries; its tables are read, about 163 MB); repos/anvil-formats/tests/mesh_shape.rs, worlds/anvil/tests/shapes.rs, meshes.rs"#,
            source: r#"the user's own install; two MeshShape bodies' prefixes, section runs and tails hand-checked in a separate read-only decode (games/riders-republic docs/recon/meshshape-second-fixture.md, c068d20)"#,
        },
        Query::Install(Game::Riders, Some("DataPC_MAD_WhiteRoom.forge")) => {
            status: Live,
            gate: false,
            about: r#"the base forge of installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge's patch (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom.forge`, 8,951,822 bytes, 388 entries); repos/anvil-formats/tests/data.rs"#,
            source: r#"the user's own install; two resource blocks' sizes, checksums, names and class ids hand-checked in a separate read-only decode (games/riders-republic docs/recon/archive-reader-fit.md, a9f4959)"#,
        },
        Query::Install(Game::Riders, Some("DataPC_MAD_WhiteRoom_patch_01.forge")) => {
            status: Live,
            gate: false,
            about: r#"one Anvil forge archive of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom_patch_01.forge`, 1,021,872 bytes, 337 entries); repos/anvil-formats/tests/forge.rs"#,
            source: r#"the user's own install; rows hand-checked against a separate read-only census of the file"#,
        },
        Query::Install(Game::ShaunWhite, Some("DataPC.forge")) => {
            status: Live,
            gate: false,
            about: r#"one Anvil forge of version 26 (Shaun White Skateboarding, PC 1.0: `DataPC.forge`, 20,545,833 bytes, 38 entries: the game's bootstrap, settings and localization), its tables, FILEDATA headers and every entry's data blocks (LZO2A) read; repos/anvil-formats/tests/forge.rs, data.rs"#,
            source: r#"the user's own copy; the first two rows (GlobalMetaFile, Graphic Bootstrap) and the second's two blocks hand-checked in a hex dump, and AnvilToolkit 1.3.7's version-25 forge reader and AC2 compression table as the third-party reading of the layout"#,
        },
        Query::Install(Game::ShaunWhite, Some("DataPC_01.forge")) => {
            status: Live,
            gate: true,
            about: r#"Shaun White Skateboarding PC 1.0 level-01 forge (version 26): every texture in all 253 GridCellDataBlock entries; focused collision shape and inert-component fixtures in four cells; level placements, materials, collision and stored water-flag lowering, harbour tint and placement; 110 omni and 79 spot placements plus the root SkyLight and sun; restored metro mural and CliffDry layer weights; exact metro graffiti wall key for the REF-backed albedo derivation; worlds/shaunwhite/tests/albedo.rs and texture.rs, collision.rs, level.rs, layers.rs"#,
            source: r#"the user's own copy, extracted unchanged; object directory/envelopes hand-checked separately, texture fields cross-checked with AnvilToolkit 1.3.7's AC2/Brotherhood readers and AC2 schema, duplicated compiled metadata and exact mip lengths checked on every texture; BC1/2/3 decoded against third-party bcdec_rs 0.2.0; code 12's BC3 storage established by sizes and normal-map content, not the AC2 enum; collision fields cross-checked with the AC2 schema and hand-inspected inert and multi-inert entity/shape references; light fields checked with the AC2 schema and exact body ends, SkyLight fields with native property descriptors and serializer; physical light units, colour encoding and direction signs remain explicit lowering approximations; restored layer equation from the shipped SDB2 HLSL and Microsoft-disassembled SM3 pixel shader, native material property hashes identify layer tints and alpha switches"#,
        },
    }
    debug {}
}
