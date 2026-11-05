//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Sekiro = "sekiro",
        Tmnf = "tmnf",
    }
    installs {}
    keys {}
    tools {
        DirectxtexDump = "directxtex_dump" {
            about: "directxtex-dump: what Microsoft's DirectXTex library reads from a DDS file (`read`, `read-permissive`, `batch`), written out as the library returns it",
            source: "third-party library: primase-oracles' dumps/directxtex built against DirectXTex may2026 (MIT)",
            gate: true,
        },
    }
    oracle_sets {
        Gbxnet = "gbxnet" {
            about: "what GBX.NET 2.5.0 (third-party, MIT) reads from the corpus, written by a thin dump harness",
            source: "GBX.NET 2.5.0 with GBX.NET.LZO 2.1.6, run over the corpus files",
        },
        Scripts = "scripts" {
            about: "measurements by our own scripts over game data, separate from the readers they check (normal curl, texture statistics, shadow and state records, the eclipse reference)",
            source: "our own scripts kept with the private oracle tools: separate from the readers under test, not third-party; some producers are not recorded yet",
        },
    }
    declared {
        Query::Oracle(OracleSet::Gbxnet, Game::Tmnf, "tmnf-packs") => {
            status: Live,
            gate: false,
            about: r#"GBX.NET 2.5.0's extraction of TrackMania Nations Forever's three packs: `<pack>/<path>` (each file's plaintext under its real name), among them the game's 50 DDS files, 13 of them legacy cube maps; tests/census.rs"#,
            source: r#"the gbxnet-pak harness (GBX.NET 2.5.0's NadeoPak) over the install's `Packs/*.pak` with `packlist.dat`'s keys"#,
        },
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "pbr") => {
            status: Live,
            gate: true,
            about: r#"source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs`"#,
            source: r#"the install's own DDS with statistics computed by a script kept with the private oracle tools"#,
        },
    }
    debug {}
}
