//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Elden = "elden",
        General = "general",
        Riders = "riders",
        Sekiro = "sekiro",
        Skate = "skate",
        Sledding = "sledding",
    }
    installs {}
    keys {}
    tools {
        Fxc = "fxc" {
            about: "Microsoft's `fxc` shader compiler executable",
            source: "third-party: Microsoft's fxc",
            gate: false,
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
        D3dDisassembler = "d3d-disassembler" {
            about: "Microsoft's D3D shader disassembly of a game's shader containers (`NNNN.dxbc` with its listing)",
            source: "independent: Microsoft's D3D Shader Disassembler, run over the user's own install",
        },
    }
    declared {
        Query::Oracle(OracleSet::Corpus, Game::Elden, "shaders") => {
            status: Live,
            gate: false,
            about: r#"a folder (searched recursively) of the DXIL containers of Elden Ring's Shader Model 6 shader bundles (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; repos/iridia/iridia-dxil/tests/lift_corpus.rs"#,
            source: r#"the user's own install: the containers of its `/shader/*.shaderbnd.dcx` binders and `/shader/*.shaderbdlebnd.dcx` bundles (ray-tracing `[rt]` bundles included), worlds/fromsoft/examples/corpus_elden_shaders.rs"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::General, "dxbc") => {
            status: Live,
            gate: true,
            about: r#"Sekiro's 390 pixel programs (`.ppo`), Microsoft's disassembly of each (`.ppo.asm`) and the bindings, inputs and constant layout read from that text (`.ppo.reflection.json`); toolkit/dxbc/tests/disassembly.rs, reflection.rs; schema in toolkit/dxbc/README.md"#,
            source: r#"independent: the game's own programs (`corpus.sekiro/shaders`) disassembled by Microsoft's fxc 10.0.26100 (`fxc /nologo /dumpbin`), the reflection read from the disassembly's text by toolkit/dxbc/tests/oracles/dxbc_corpus.py"#,
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
        Query::Oracle(OracleSet::D3dDisassembler, Game::Riders, "shader-pack") => {
            status: Live,
            gate: false,
            about: r#"a folder of DXBC containers `NNNN.dxbc` (Riders Republic's ShaderDataDX11.bin, Steam build 25394987, split at its containers: 2,443) each with `NNNN.txt`, Microsoft's D3D Shader Disassembler text of it; toolkit/dxbc/tests/disassembly.rs"#,
            source: r#"independent: Microsoft's D3D Shader Disassembler, run over the user's own install's pack (kept in a private cache)"#,
        },
    }
    debug {
        "shader.diff_limit" => "how many programs of a corpus the WARP comparison runs",
        "shader.warp_all" => "`1` compares every program the WARP oracle can bind",
    }
}
