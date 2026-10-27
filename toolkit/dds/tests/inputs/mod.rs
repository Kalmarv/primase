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
    tools {}
    oracle_sets {
        Scripts = "scripts" {
            about: "measurements by our own scripts over game data, separate from the readers they check (normal curl, texture statistics, shadow and state records, the eclipse reference)",
            source: "our own scripts kept with the private oracle tools: separate from the readers under test, not third-party; some producers are not recorded yet",
        },
    }
    declared {
        Query::Oracle(OracleSet::Scripts, Game::Sekiro, "pbr") => {
            status: Live,
            gate: true,
            about: r#"source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs`"#,
            source: r#"the install's own DDS with statistics computed by a script kept with the private oracle tools"#,
        },
    }
    debug {}
}
