//! The real-data inputs the examples read, each with what it is, where it comes from and whether the landing gate
//! needs it (`primase_testdata::inputs!`). The examples include this module with
//! `#[path = "../tests/inputs/mod.rs"] mod inputs;`; tests/real_data_inputs.rs checks that every query the examples
//! write is declared here.

primase_testdata::inputs! {
    games {
        Elden = "elden",
    }
    installs {
        Elden {
            about: "Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed, the Oodle 6 DLL)",
            source: "the user's own install",
            gate: false,
        },
    }
    keys {
        Elden {
            about: "the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits",
            source: "the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools",
            gate: false,
        },
    }
    tools {}
    oracle_sets {}
    declared {}
    debug {}
}
