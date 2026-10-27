//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Gta = "gta",
    }
    installs {
        Gta {
            about: "a GTA V install folder (Enhanced or Legacy: the executable and the `.rpf` archives)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {
        Gta {
            about: "the user's GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); never committed",
            source: "the user's own key material, checked against published digests",
            gate: true,
        },
    }
    tools {}
    oracle_sets {
        CodeWalker = "codewalker" {
            about: "CodeWalker's readings of a GTA V install (archive listings, live-file tables, texture parents, resources, frames, bounds)",
            source: "independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools",
        },
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
    }
    declared {
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "ShadersGen9Conversion.xml") => {
            status: Live,
            gate: false,
            about: r#"optional: CodeWalker's `ShadersGen9Conversion.xml`, which names every effect's texture and constant parameters (current and previous name, constant-buffer offset); `worlds/gta/tests/program_tables.rs` holds the shader database's parameter records to it"#,
            source: r#"independent: CodeWalker.Core 485d56b, a data file of its repository (CodeWalker.Core/ShadersGen9Conversion.xml)"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "bounds") => {
            status: Live,
            gate: true,
            about: r#"props' embedded collision bounds (a ydr's own, a yft's first physics LOD's) as an independent reader (CodeWalker) reads them: a folder of JSON files, one per prop, `{path, leaves: [{type, flags, polygons: [{type, material, vertices (placed)}] or corners (8, placed)}]}`; `lower-gta-embedded-collision`'s triangles are checked against them (needs `installs.gta`, `keys.gta`); `worlds/gta/tests/embedded_bounds.rs`"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "frames.json") => {
            status: Live,
            gate: true,
            about: r#"one ydr's high-detail geometries as an independent reader (CodeWalker) reads them, as stored in GTA's frame: one JSON file `{path, value: {models: [{geometries: [{positions, normals, tangents (xyzw), texcoords, indices}]}]}}`; the frame change, triangle fronts and bitangent sign are checked against it (needs `installs.gta`, `keys.gta`), and its stored tangent frames against the frames its texture coordinates give; `worlds/gta/tests/handedness.rs`, `uv_frames.rs`"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "parity") => {
            status: Live,
            gate: false,
            about: r#"optional: a folder holding `dump.jsonl`, an independent reader's decoding of every file the gate regions' placements reach (the CodeWalker harness's `batch` mode over the list `parity_list` writes; schema in `worlds/gta/tests/parity.rs`)"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools (`batch` mode, written when needed)"#,
        },
        Query::Oracle(OracleSet::CodeWalker, Game::Gta, "resources") => {
            status: Live,
            gate: true,
            about: r#"optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `https://github.com/Kalmarv/rage-engine-formats/blob/main/tests/resource_oracle.rs`)"#,
            source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Gta, "legacy-effects") => {
            status: Unheld("it needs GTA V Legacy's install, which no machine holds (the Enhanced edition has no effect files)"),
            gate: false,
            about: r#"a folder (searched recursively) of GTA V Legacy's compiled effects (`.fxc`, RAGE `rgxe`) as extracted from the install (the Enhanced edition has none); every one is read, its programs' containers checked and lifted; worlds/gta/tests/fxc_census.rs"#,
            source: r#"the user's own install: the effect files extracted from its archives"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Gta, "shader-database.awc") => {
            status: Live,
            gate: false,
            about: r#"GTA V Enhanced's shader database (`sga_win32_60_final.awc`, magic `SGD2`, from update/update2.rpf `common/shaders/fxdb`) as the install stores it after decryption and decompression: every program and effect is read, every program lifted; worlds/gta/tests/fxc_census.rs"#,
            source: r#"the user's own install: the file extracted from its archive, worlds/gta/examples/corpus_gta_shader_database.rs"#,
        },
    }
    debug {
        "gta.archive_filter" => "only archives whose path contains this text",
        "gta.region" => "only this named gate region of worlds/gta/tests/region_pipeline.rs",
    }
}
