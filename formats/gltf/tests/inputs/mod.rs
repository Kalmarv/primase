//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        General = "general",
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
        Khronos = "khronos" {
            about: "Khronos glTF-Sample-Assets, one folder per model, with their expected readings",
            source: "third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expectations by pygltflib and Pillow",
        },
    }
    declared {
        Query::Oracle(OracleSet::Khronos, Game::General, "sample-assets") => {
            status: Live,
            gate: true,
            about: r#"a folder of Khronos glTF-Sample-Assets models, one folder per model (`<Model>/<Model>.glb` or `.gltf` with its buffers, `expected.json` written by pygltflib from the same file, `validator.json` by the Khronos validator and, for a model with images, `images.json` by Pillow; for a JPEG image `<Model>/images/<n>.jpg` and the RGBA8 rows Pillow decodes of it, `<Model>/images/<n>.rgba`)"#,
            source: r#"third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expected.json by pygltflib 1.16.5; images.json by Pillow 11.0.0"#,
        },
    }
    debug {}
}
