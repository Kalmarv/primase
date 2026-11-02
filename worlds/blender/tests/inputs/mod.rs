//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        General = "general",
    }
    installs {}
    keys {}
    tools {
        Blender = "blender" {
            about: "Blender's executable (`blender.exe`, 5.2.3 LTS), run in the background only: the save and render stages build and render the scenes the tests write",
            source: "third-party: the Blender Foundation's Blender 5.2.3 LTS release build",
            gate: true,
        },
    }
    oracle_sets {}
    declared {}
    debug {}
}
