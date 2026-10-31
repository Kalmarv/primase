//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Tmnf = "tmnf",
    }
    installs {
        Tmnf {
            about: "TrackMania Nations Forever's install folder (Steam: `Packs/*.pak` with `packlist.dat`, `GameData` with the campaigns' maps under `Tracks/Campaigns`)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {}
    tools {}
    oracle_sets {}
    declared {}
    debug {}
}
