//! The real-data inputs this crate's tests read, each with what it is, where it comes from and whether the
//! landing gate needs it (`primase_testdata::inputs!`; README.md here lists them). Integration tests reach it
//! with `mod inputs;`, unit tests with `#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the
//! crate root, and tests/real_data_inputs.rs checks that every query the crate's sources write is declared
//! here. Declare an input here when a test first asks for it.

primase_testdata::inputs! {
    games {
        Elden = "elden",
        Sekiro = "sekiro",
    }
    installs {
        Elden {
            about: "Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed)",
            source: "the user's own install",
            gate: false,
        },
        Sekiro {
            about: "Sekiro's install folder (Data1 to Data5 BHD/BDT pairs, the Oodle 6 DLL)",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {
        Elden {
            about: "the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits (held to the SHA-256 worlds/fromsoft/src/elden/mod.rs states)",
            source: "the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools",
            gate: false,
        },
        Sekiro {
            about: "the user's Sekiro archive keys: one PKCS#1 `RSA PUBLIC KEY` PEM per archive, Data1.pem to Data5.pem",
            source: "the community unpacker's published keys (UXM's ArchiveKeys.cs), written out as PEM files",
            gate: true,
        },
    }
    tools {
        Oodle6 = "oodle6" {
            about: "an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
            source: "the game's own DLL from the user's install",
            gate: true,
        },
    }
    oracle_sets {
        Corpus = "corpus" {
            about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
            source: "the user's own installs: files extracted as stored",
        },
        SoulsFormats = "soulsformats" {
            about: "SoulsFormats' readings of FromSoftware archives and files (rigid parts, skins, parts, census, frames, formats)",
            source: "independent: SoulsFormats 9c0da6b (Sekiro) and SoulsFormatsNEXT ee1dd61 (Elden Ring), through a thin dump harness kept with the private oracle tools",
        },
        Uxm = "uxm" {
            about: "UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line)",
            source: "third-party: UXM-Selective-Unpack 9501be8's dictionaries",
        },
    }
    declared {
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok") => {
            status: Live,
            gate: true,
            about: r#"one area's high-resolution map collision as its binder stores it: loose Havok 2016.2 tagfiles and the compendium they share, every body and compressed mesh read; toolkit/havok/tests/corpus.rs, schema in toolkit/havok/README.md"#,
            source: r#"the game's own files from the user's install: worlds/fromsoft/examples/corpus_sekiro_havok.rs (the area's `h*.hkxbhd`/`.hkxbdt` binder, each member out of its DCX envelope)"#,
        },
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-soup") => {
            status: Live,
            gate: true,
            about: r#"an independent decoder's raw triangle soup of each `havok` tagfile, `<name>.hkx.soup`; schema in toolkit/havok/README.md; worlds/fromsoft/tests/collision.rs"#,
            source: r#"independent: a clean-room decoder written from the data alone (no primase decoding code, no third-party code), the primase-oracles harness dumps/havok-soup (5f6dde8, Python standard library) run over `corpus.sekiro/havok`; its README states the format facts and its counts.tsv the per-file triangles"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "census") => {
            status: Live,
            gate: false,
            about: r#"an independent census of installs.elden in oracles.soulsformats.sekiro/census's schema (the archives' decrypted headers as `<name>.plain`, entries.tsv, binders.tsv, errors.tsv, samples; hashes 16 hex digits; an archive's shard is its index in UXM's Elden Ring archive list); worlds/fromsoft/tests/census.rs, and asset_collision.rs (the asset collision binders entries.tsv lists)"#,
            source: r#"independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys, RSA and path hash, the Elden Ring harness's `census` mode (kept with the private oracle tools)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "format") => {
            status: Live,
            gate: false,
            about: r#"an independent reader's field-by-field reading of Elden Ring files read from installs.elden (JSON lines, floats as hex bit patterns): `msb.jsonl` (every map layout: its models, every part with kind, model, transform, entity IDs, shadow controls and hit filter, and its spawn point regions with name, position and rotation), `btl.jsonl` (every light bank: its version and lights, or the reference's refusal), `havok.jsonl` (chosen tiles' map collision and assets' collision: every compressed mesh shape's stored sections, vertices, primitives, shape keys and extern shape instances), `matbin.jsonl` (every material binder's members in member order: shader and source paths, key, every parameter's name, key, type and value, every sampler's type, path, key and two trailing floats), `metaparam.jsonl` (every shader bundle's metaparam: binder, bundle and member names, header words, texture slots, render passes, CAParams and MtdParams); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/msb_census.rs, btl_census.rs, havok_census.rs, matbin_census.rs, metaparam_census.rs"#,
            source: r#"independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys and path hash, and HKLib ac40bc9 for Havok, the Elden Ring harness's format modes (kept with the private oracle tools)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "census") => {
            status: Live,
            gate: true,
            about: r#"reference archive outputs; schema in worlds/fromsoft/README.md"#,
            source: r#"independent: SoulsFormats 9c0da6b with UXM's Sekiro dictionary, the dump harness's census mode (kept with the private oracle tools)"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "materials") => {
            status: Live,
            gate: true,
            about: r#"independent MTD fields and block lengths (mtd.jsonl, mtd-boundaries.jsonl); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/mtd_census.rs"#,
            source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`mtd` mode): the fields from its MTD reader, the consumed block lengths from its writer's rewrite of each file"#,
        },
        Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "tpf") => {
            status: Live,
            gate: true,
            about: r#"independent texture-package census (packages.jsonl, area-dds/, errors.tsv); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_census.rs, tpf_pixels.rs"#,
            source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`tpf` mode); area-dds/ holds every distinct DDS under `/map/`"#,
        },
        Query::Oracle(OracleSet::Uxm, Game::Elden, "dictionary.txt") => {
            status: Live,
            gate: false,
            about: r#"caller-provided UTF-8 Elden Ring path dictionary"#,
            source: r#"third-party: UXM's Elden Ring path dictionary"#,
        },
        Query::Oracle(OracleSet::Uxm, Game::Sekiro, "dictionary.txt") => {
            status: Live,
            gate: true,
            about: r#"caller-provided UTF-8 Sekiro path dictionary"#,
            source: r#"third-party: UXM's Sekiro path dictionary"#,
        },
    }
    debug {}
}
