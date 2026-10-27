//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Sekiro = "sekiro",
    }
    installs {}
    keys {}
    tools {
        GltfValidator = "gltf_validator" {
            about: "the Khronos glTF-Validator executable (2.0.0-dev.3.10 release build)",
            source: "third-party: Khronos glTF-Validator 2.0.0-dev.3.10",
            gate: true,
        },
    }
    oracle_sets {
        Baseline = "baseline" {
            about: "our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result)",
            source: "regression baselines: our own earlier outputs, not independent oracles",
        },
    }
    declared {
        Query::Oracle(OracleSet::Baseline, Game::Sekiro, "ktx2-tiles") => {
            status: Live,
            gate: true,
            about: r#"a web tile export with KTX2 textures and its plain set, both written from the same prepared cell worlds (one conversion writing both sets, `primase_web::convert::tiles`): tiles.json, the tiles, images/, plain/tiles.json and its tiles; formats/web/tests/ktx2_audit.rs"#,
            source: r#"our own writer's output: examples/examples/sekiro_ktx2_tiles.rs writes it (slice A of Sekiro's m11_01, both sets in one conversion), so the plain set holds the KTX2 encoder's input; the checks are an independent decoder's (the pure-Rust basisu transcoder, against the plain set) and the viewer's WASM transcoder's (the printed hashes)"#,
        },
    }
    debug {}
}
