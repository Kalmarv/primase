//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Elden = "elden",
        Riders = "riders",
        Sekiro = "sekiro",
        Skate = "skate",
        Sledding = "sledding",
    }
    installs {}
    keys {}
    tools {
        Dxc = "dxc" {
            about: "Microsoft's `dxc` shader compiler executable",
            source: "third-party: Microsoft's dxc",
            gate: true,
        },
        Warp = "warp" {
            about: "the WARP oracle program that runs a shader on Microsoft's WARP rasterizer",
            source: "third-party: Microsoft's WARP rasterizer, through a thin host program kept with the private oracle tools",
            gate: true,
        },
    }
    oracle_sets {
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
    }
    declared {
        Query::Oracle(OracleSet::Corpus, Game::Elden, "shaders") => {
            status: Live,
            gate: false,
            about: r#"a folder (searched recursively) of the DXIL containers of Elden Ring's Shader Model 6 shader bundles (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; repos/iridia/iridia-dxil/tests/lift_corpus.rs"#,
            source: r#"the user's own install: the containers of its `/shader/*.shaderbnd.dcx` binders and `/shader/*.shaderbdlebnd.dcx` bundles (ray-tracing `[rt]` bundles included), worlds/fromsoft/examples/corpus_elden_shaders.rs"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Riders, "shaders") => {
            status: Live,
            gate: false,
            about: r#"a folder (searched recursively) of the DXBC containers of Riders Republic's shader pack (ShaderDataDX11.bin, split at its containers), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs"#,
            source: r#"the user's own install: the programs split from its shader pack"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "shaders") => {
            status: Live,
            gate: false,
            about: r#"a folder (searched recursively) of the DXBC containers of Sekiro's extracted pixel and vertex programs (`.ppo`, `.vpo`), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs; the detail-blend programs' equations derived and proven, worlds/fromsoft/tests/detail_derive.rs"#,
            source: r#"the user's own install: the programs extracted from its shader archives"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Skate, "shaders") => {
            status: Live,
            gate: false,
            about: r#"a folder (searched recursively) of the DXIL containers of skate.'s CompiledBytecode resources (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; repos/iridia/iridia-dxil/tests/lift_corpus.rs"#,
            source: r#"the user's own install: the containers of its superbundles' CompiledBytecode resources, worlds/reskate/examples/corpus_skate_shaders.rs"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sledding, "shaders") => {
            status: Live,
            gate: false,
            about: r#"a folder (searched recursively) of the DXBC containers of a Unity player's compiled pixel programs, as primase-unity extracts them, lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs"#,
            source: r#"the user's own install: the programs read from its shader assets"#,
        },
    }
    debug {
        "dxil.fuzz_runs" => "how many mutated fixtures the DXIL fuzz test runs (default 3000)",
    }
}
