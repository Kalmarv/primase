//! What a test may ask the real-data configuration for: the games' installs and keys, the tools, the oracle sets and
//! the debug keys, each declared once with what it is, where it comes from and whether the landing gate needs it. The
//! configuration file names them by their keys (`installs.sekiro`, `tools.warp`, `oracles.root`); see the crate
//! documentation for its layout. Every file a test asks for inside an install or an oracle set is declared too
//! ([`DECLARED`]), with what it holds, where it comes from and whether its obligation stands: a query for an undeclared
//! file fails, so a mistyped path never skips quietly.

use crate::query::Query;

/// A game primase reads, as the configuration names its install, keys and oracle folders. [`Game::General`] is no
/// game's: oracle sets about a format or a tool rather than one game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Game {
    Sekiro,
    Elden,
    Gta,
    Riders,
    Skatebird,
    Sledding,
    Stuntboost,
    ShaunWhite,
    Skate,
    General,
}

impl Game {
    /// Its key in the configuration and its folder name under an oracle set.
    pub const fn key(self) -> &'static str {
        match self {
            Game::Sekiro => "sekiro",
            Game::Elden => "elden",
            Game::Gta => "gta",
            Game::Riders => "riders",
            Game::Skatebird => "skatebird",
            Game::Sledding => "sledding",
            Game::Stuntboost => "stuntboost",
            Game::ShaunWhite => "shaunwhite",
            Game::Skate => "skate",
            Game::General => "general",
        }
    }
}

/// A program a test runs or a file of one (an executable, a game's DLL, a type tree package).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    Warp,
    Dxc,
    Fxc,
    ReskateCli,
    GltfValidator,
    Oodle5,
    Oodle6,
    Oodle9,
    SceneSkate,
    UnityTpk,
    Dotnet,
    PythonPillow,
}

impl Tool {
    /// Its key in the configuration's `[tools]`.
    pub const fn key(self) -> &'static str {
        match self {
            Tool::Warp => "warp",
            Tool::Dxc => "dxc",
            Tool::Fxc => "fxc",
            Tool::ReskateCli => "reskate_cli",
            Tool::GltfValidator => "gltf_validator",
            Tool::Oodle5 => "oodle5",
            Tool::Oodle6 => "oodle6",
            Tool::Oodle9 => "oodle9",
            Tool::SceneSkate => "scene_skate",
            Tool::UnityTpk => "unity_tpk",
            Tool::Dotnet => "dotnet",
            Tool::PythonPillow => "python_pillow",
        }
    }
}

/// An oracle set: the outputs of one producer, under `<oracles.root>/<set>/<game>/`. The producer decides how far its
/// outputs are independent of what they check ([`Entry::source`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OracleSet {
    UnityPy,
    CodeWalker,
    SoulsFormats,
    Uxm,
    GameCode,
    RrAssetStudio,
    Khronos,
    Studio,
    Reskate,
    D3dDisassembler,
    Pillow,
    Port,
    Corpus,
    Scripts,
    Baseline,
}

impl OracleSet {
    /// Its folder under `oracles.root`.
    pub const fn key(self) -> &'static str {
        match self {
            OracleSet::UnityPy => "unitypy",
            OracleSet::CodeWalker => "codewalker",
            OracleSet::SoulsFormats => "soulsformats",
            OracleSet::Uxm => "uxm",
            OracleSet::GameCode => "game-code",
            OracleSet::RrAssetStudio => "rrassetstudio",
            OracleSet::Khronos => "khronos",
            OracleSet::Studio => "studio",
            OracleSet::Reskate => "reskate",
            OracleSet::D3dDisassembler => "d3d-disassembler",
            OracleSet::Pillow => "pillow",
            OracleSet::Port => "port",
            OracleSet::Corpus => "corpus",
            OracleSet::Scripts => "scripts",
            OracleSet::Baseline => "baseline",
        }
    }
}

/// One declared configuration entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its key in the configuration (`installs.sekiro`, `tools.warp`, `oracles.codewalker`).
    pub key: &'static str,
    /// What it is and the layout it must have.
    pub about: &'static str,
    /// Where its data comes from, and so how far it is independent of what it checks.
    pub source: &'static str,
    /// The landing gate must have it (`[run] require = 'gate'`).
    pub gate: bool,
}

/// An oracle set: one producer's outputs, under `<oracles.root>/<set>/`; its files are declared one by one
/// ([`DECLARED`]), each with whether the landing gate needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set {
    /// Its folder under `oracles.root`, as `oracles.<set>`.
    pub key: &'static str,
    /// What its files are.
    pub about: &'static str,
    /// Its producer, and so how far its files are independent of what they check.
    pub source: &'static str,
}

/// Every install, by [`Game::key`].
pub const INSTALLS: &[Entry] = &[
    Entry {
        key: "installs.sekiro",
        about: "Sekiro's install folder (Data1 to Data5 BHD/BDT pairs, the Oodle 6 DLL)",
        source: "the user's own install",
        gate: true,
    },
    Entry {
        key: "installs.elden",
        about: "Elden Ring's `Game` folder (Data0 to Data3, `sd/sd`, DLC archives when installed)",
        source: "the user's own install",
        gate: false,
    },
    Entry {
        key: "installs.gta",
        about: "a GTA V install folder (Enhanced or Legacy: the executable and the `.rpf` archives)",
        source: "the user's own install",
        gate: true,
    },
    Entry {
        key: "installs.riders",
        about: "Riders Republic's folder of `.forge` archives (Steam build 25394987)",
        source: "the user's own install",
        gate: false,
    },
    Entry {
        key: "installs.shaunwhite",
        about: "Shaun White Skateboarding's (PC, 2010) folder of `.forge` archives, version 26",
        source: "the user's own copy: the forges as the release's archives hold them, extracted unchanged",
        gate: false,
    },
    Entry {
        key: "installs.skatebird",
        about: "SkateBIRD's install folder (`SkateBIRD_Data`: a Unity 2019.4.20f1 player)",
        source: "the user's own install",
        gate: true,
    },
    Entry {
        key: "installs.sledding",
        about: "Sledding Game's install folder (`Sledding Game_Data`: a Unity 6000.3 player)",
        source: "the user's own install",
        gate: true,
    },
    Entry {
        key: "installs.stuntboost",
        about: "STUNTBOOST's install folder (Steam buildid 25832744: `Content/Models/Export`)",
        source: "the user's own install",
        gate: true,
    },
    Entry {
        key: "installs.skate",
        about: "the skate. install the ReSkate tools use (`Data`: its `layout.toc`, TOCs and CAS archives; the Oodle 9 DLL)",
        source: "the user's own install",
        gate: true,
    },
];

/// Every game's archive key material, by [`Game::key`].
pub const KEYS: &[Entry] = &[
    Entry {
        key: "keys.gta",
        about: "the user's GTA V archive key material in the community layout (`gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`, `gtav_hash_lut.dat`, optional `gtav_aes_key.dat`); never committed",
        source: "the user's own key material, checked against published digests",
        gate: true,
    },
    Entry {
        key: "keys.elden",
        about: "the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem, and `regulation.key`, the key of `regulation.bin` as 64 hexadecimal digits (held to the SHA-256 worlds/fromsoft/src/elden/mod.rs states)",
        source: "the community unpacker's published keys (UXM's ArchiveKeys.cs) and SoulsFormats' RegulationDecryptor key for the game, written out by a script kept with the game's private tools",
        gate: false,
    },
];

/// Every tool, by [`Tool::key`].
pub const TOOLS: &[Entry] = &[
    Entry {
        key: "tools.warp",
        about: "the WARP oracle program that runs a shader on Microsoft's WARP rasterizer",
        source: "third-party: Microsoft's WARP rasterizer, through a thin host program kept with the private oracle tools",
        gate: true,
    },
    Entry {
        key: "tools.dxc",
        about: "Microsoft's `dxc` shader compiler executable",
        source: "third-party: Microsoft's dxc",
        gate: true,
    },
    Entry {
        key: "tools.fxc",
        about: "Microsoft's `fxc` shader compiler executable",
        source: "third-party: Microsoft's fxc",
        gate: false,
    },
    Entry {
        key: "tools.reskate_cli",
        about: "ReSkate Studio's CLI executable (`reskate_cli`), an independent EBX reader and reference writer",
        source: "third-party: ReSkate Studio's CLI (20261012)",
        gate: false,
    },
    Entry {
        key: "tools.gltf_validator",
        about: "the Khronos glTF-Validator executable (2.0.0-dev.3.10 release build)",
        source: "third-party: Khronos glTF-Validator 2.0.0-dev.3.10",
        gate: true,
    },
    Entry {
        key: "tools.oodle5",
        about: "an Oodle version-5 DLL (`oo2core_5_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
        source: "the game's own DLL from the user's install",
        gate: true,
    },
    Entry {
        key: "tools.oodle6",
        about: "an Oodle version-6 DLL (`oo2core_6_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
        source: "the game's own DLL from the user's install",
        gate: true,
    },
    Entry {
        key: "tools.oodle9",
        about: "an Oodle version-9 DLL (`oo2core_9_win64.dll`); on Windows only: elsewhere oozextract decodes and nothing is configured",
        source: "the game's own DLL from the user's install",
        gate: true,
    },
    Entry {
        key: "tools.scene_skate",
        about: "our own skate-scene validator (`scene-skate.dll`, run through `tools.dotnet`, or its executable)",
        source: "our own C# validator, rebuilt from its last source (commit d78228a): it refuses what its checks refuse, it is not an oracle",
        gate: true,
    },
    Entry {
        key: "tools.unity_tpk",
        about: "a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read",
        source: "input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools)",
        gate: true,
    },
    Entry {
        key: "tools.dotnet",
        about: "the .NET host (`dotnet`, with the .NET 8 runtime) that runs the C# programs given as assemblies: `scripts.sekiro/dds`'s DdsWorker.dll and a `tools.scene_skate` that is a .dll",
        source: "third-party: Microsoft's .NET 8 runtime",
        gate: true,
    },
    Entry {
        key: "tools.python_pillow",
        about: "a Python 3 interpreter with Pillow 11.0.0, which runs the Pillow oracle worker (toolkit/texcodec/tests/oracles/worker.py)",
        source: "third-party: Pillow 11.0.0",
        gate: true,
    },
];

/// Every oracle set, by [`OracleSet::key`]: its folder under `oracles.root` holds one folder per [`Game::key`].
pub const ORACLE_SETS: &[Set] = &[
    Set {
        key: "oracles.unitypy",
        about: "UnityPy's readings of a Unity player (object tables, samples, meshes, textures, materials, shaders, frames, listings)",
        source: "independent: UnityPy, through dump scripts kept with the private oracle tools",
    },
    Set {
        key: "oracles.codewalker",
        about: "CodeWalker's readings of a GTA V install (archive listings, live-file tables, texture parents, resources, frames, bounds)",
        source: "independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools",
    },
    Set {
        key: "oracles.soulsformats",
        about: "SoulsFormats' readings of FromSoftware archives and files (rigid parts, skins, parts, census, frames, formats)",
        source: "independent: SoulsFormats 9c0da6b (Sekiro) and SoulsFormatsNEXT ee1dd61 (Elden Ring), through a thin dump harness kept with the private oracle tools",
    },
    Set {
        key: "oracles.uxm",
        about: "UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line)",
        source: "third-party: UXM-Selective-Unpack 9501be8's dictionaries",
    },
    Set {
        key: "oracles.game-code",
        about: "a game's own code run over its install (STUNTBOOST's parsers via reflection: one JSON per level part)",
        source: "independent: the game's own code via reflection, the oracle tool kept with the game's private tools",
    },
    Set {
        key: "oracles.rrassetstudio",
        about: "RRAssetStudio's read of every Mesh of a Riders Republic install (`summary.jsonl`, one row per Mesh)",
        source: "third-party: RRAssetStudio v3, run over the user's own install by the primase-oracles harness",
    },
    Set {
        key: "oracles.khronos",
        about: "Khronos glTF-Sample-Assets, one folder per model, with their expected readings",
        source: "third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expectations by pygltflib and Pillow",
    },
    Set {
        key: "oracles.studio",
        about: "ReSkate Studio's compiles of maps our writer built (Patch folders)",
        source: "third-party: ReSkate Studio 20261012's compile output",
    },
    Set {
        key: "oracles.reskate",
        about: "ReSkate's own readers' readings of skate. data (the render skeleton)",
        source: "ReSkate's EBX reader (the source primase's EBX reader is ported from), through our C++ harness: a port check",
    },
    Set {
        key: "oracles.d3d-disassembler",
        about: "Microsoft's D3D shader disassembly of a game's shader containers (`NNNN.dxbc` with its listing)",
        source: "independent: Microsoft's D3D Shader Disassembler, run over the user's own install",
    },
    Set {
        key: "oracles.pillow",
        about: "Pillow's decodings of game textures and what is computed from them (the skate mask cases)",
        source: "independent: Pillow's decode and channel mapping of the game's own textures, generated by a script kept with the private oracle tools",
    },
    Set {
        key: "oracles.port",
        about: "another exporter's output for a game (its level folder, materials joined to their objects, terrain unpacked)",
        source: "another exporter's output: an appearance baseline, not a reading of the source",
    },
    Set {
        key: "oracles.corpus",
        about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
        source: "the user's own installs: files extracted as stored",
    },
    Set {
        key: "oracles.scripts",
        about: "measurements by our own scripts over game data, separate from the readers they check (normal curl, texture statistics, shadow and state records, the eclipse reference)",
        source: "our own scripts kept with the private oracle tools: separate from the readers under test, not third-party; some producers are not recorded yet",
    },
    Set {
        key: "oracles.baseline",
        about: "our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result)",
        source: "regression baselines: our own earlier outputs, not independent oracles",
    },
];

/// Whether a declared file's obligation stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The obligation stands and the input it names is acceptable.
    Live,
    /// The obligation stands, but no acceptable input exists yet (the existing data is not independent, or its
    /// producer is gone): `source` names the replacement. It is a landing-gate input, so a gate run without it fails:
    /// its tests never pass by skipping.
    Blocked,
    /// The obligation stands and the input is acceptable, but no machine here holds it (`why`: an install the owner
    /// does not have). Never a landing-gate input: where it is not there, its tests skip naming `why`, whatever
    /// `[run] require` says; a machine that comes to hold it configures it as any other.
    Unheld { why: &'static str },
}

/// A file or folder a test asks for inside an install ([`Query::Install`] with a path) or an oracle set
/// ([`Query::Oracle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declared {
    pub query: Query,
    pub status: Status,
    /// The landing gate must have it (`[run] require = 'gate'`).
    pub gate: bool,
    /// What it is, its layout or schema, and the tests that read it.
    pub about: &'static str,
    /// Where its data comes from, and so how far it is independent of what it checks.
    pub source: &'static str,
}

/// The declaration of `query`, a file in an install or an oracle set; `None` for an undeclared one, and for queries
/// the configuration's keys name whole (an install, keys, a tool, the scratch folder, a debug key).
pub fn declaration(query: Query) -> Option<&'static Declared> {
    DECLARED.iter().find(|d| d.query == query)
}

/// Every file of an install or an oracle set a test asks for, by its query's key.
pub const DECLARED: &[Declared] = &[
    Declared {
        query: Query::Install(Game::Riders, Some("DataPC_MAD_USNAT.forge")),
        status: Status::Live,
        gate: false,
        about: r#"a large Anvil forge of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT.forge`, 15,650,919,308 bytes, 767,463 entries; its tables are read, about 163 MB); worlds/anvil/tests/mesh_shape.rs, shapes.rs, meshes.rs"#,
        source: r#"the user's own install; two MeshShape bodies' prefixes, section runs and tails hand-checked in a separate read-only decode (games/riders-republic docs/recon/meshshape-second-fixture.md, c068d20)"#,
    },
    Declared {
        query: Query::Install(Game::Riders, Some("DataPC_MAD_USNAT_patch_01.forge")),
        status: Status::Live,
        gate: false,
        about: r#"the patch forge of installs.riders/DataPC_MAD_USNAT.forge (Riders Republic, Steam build 25394987: `DataPC_MAD_USNAT_patch_01.forge`, 1,331,014,794 bytes, 320,777 entries); worlds/anvil/tests/meshes.rs"#,
        source: r#"the user's own install; four Mesh bodies' descriptors, buffers and splits hand-checked in a separate read-only decode (games/riders-republic docs/recon/mesh-second-fixture.md, a03c970)"#,
    },
    Declared {
        query: Query::Install(Game::Riders, Some("DataPC_MAD_WhiteRoom.forge")),
        status: Status::Live,
        gate: false,
        about: r#"the base forge of installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge's patch (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom.forge`, 8,951,822 bytes, 388 entries); worlds/anvil/tests/data.rs"#,
        source: r#"the user's own install; two resource blocks' sizes, checksums, names and class ids hand-checked in a separate read-only decode (games/riders-republic docs/recon/archive-reader-fit.md, a9f4959)"#,
    },
    Declared {
        query: Query::Install(Game::Riders, Some("DataPC_MAD_WhiteRoom_patch_01.forge")),
        status: Status::Live,
        gate: false,
        about: r#"one Anvil forge archive of version 27 (Riders Republic, Steam build 25394987: `DataPC_MAD_WhiteRoom_patch_01.forge`, 1,021,872 bytes, 337 entries); worlds/anvil/tests/forge.rs"#,
        source: r#"the user's own install; rows hand-checked against a separate read-only census of the file"#,
    },
    Declared {
        query: Query::Install(Game::ShaunWhite, Some("DataPC.forge")),
        status: Status::Live,
        gate: false,
        about: r#"one Anvil forge of version 26 (Shaun White Skateboarding, PC 1.0: `DataPC.forge`, 20,545,833 bytes, 38 entries: the game's bootstrap, settings and localization), its tables, FILEDATA headers and every entry's data blocks (LZO2A) read; worlds/anvil/tests/forge.rs, data.rs"#,
        source: r#"the user's own copy; the first two rows (GlobalMetaFile, Graphic Bootstrap) and the second's two blocks hand-checked in a hex dump, and AnvilToolkit 1.3.7's version-25 forge reader and AC2 compression table as the third-party reading of the layout"#,
    },
    Declared {
        query: Query::Install(Game::ShaunWhite, Some("DataPC_01.forge")),
        status: Status::Live,
        gate: true,
        about: r#"Shaun White Skateboarding PC 1.0 level-01 forge (version 26): every texture in all 253 GridCellDataBlock entries; focused collision shape and inert-component fixtures in four cells; level placements, materials and collision lowering; worlds/shaunwhite/tests/texture.rs, collision.rs, level.rs"#,
        source: r#"the user's own copy, extracted unchanged; object directory/envelopes hand-checked separately, texture fields cross-checked with AnvilToolkit 1.3.7's AC2/Brotherhood readers and AC2 schema, duplicated compiled metadata and exact mip lengths checked on every texture; BC1/2/3 decoded against third-party bcdec_rs 0.2.0; code 12's BC3 storage established by sizes and normal-map content, not the AC2 enum; collision fields cross-checked with the AC2 schema and hand-inspected inert and multi-inert entity/shape references"#,
    },
    Declared {
        query: Query::Install(Game::ShaunWhite, Some("DataPC_02.forge")),
        status: Status::Live,
        gate: true,
        about: r#"Shaun White Skateboarding PC 1.0 shared level-02 objects referenced by level 01; worlds/shaunwhite/tests/level.rs"#,
        source: r#"the user's own copy, extracted unchanged; source object ids resolve mesh, texture and collision references across forges"#,
    },
    Declared {
        query: Query::Install(Game::ShaunWhite, Some("DataPC_04.forge")),
        status: Status::Live,
        gate: true,
        about: r#"Shaun White Skateboarding PC 1.0 shared level-04 objects referenced by level 01; worlds/shaunwhite/tests/level.rs"#,
        source: r#"the user's own copy, extracted unchanged; source object ids resolve mesh, texture and collision references across forges"#,
    },
    Declared {
        query: Query::Install(Game::ShaunWhite, Some("DataPC_extra.forge")),
        status: Status::Live,
        gate: true,
        about: r#"Shaun White Skateboarding PC 1.0 standalone textures: all 48 TextureMapSpec entries and their owned TextureMaps; worlds/shaunwhite/tests/texture.rs"#,
        source: r#"the user's own copy, extracted unchanged; same field/layout checks as DataPC_01.forge and all stored mip levels decoded against third-party bcdec_rs 0.2.0"#,
    },
    Declared {
        query: Query::Install(Game::Skate, Some("Data")),
        status: Status::Live,
        gate: true,
        about: r#"the skate. install's `Data` folder (its `layout.toc`, TOCs and CAS archives), read only"#,
        source: r#"the user's own install"#,
    },
    Declared {
        query: Query::Install(Game::Skatebird, Some("SkateBIRD_Data")),
        status: Status::Live,
        gate: true,
        about: r#"SkateBIRD's data folder (`SkateBIRD_Data`, a Unity 2019.4.20f1 player: SerializedFile version 21 containers), read with `tools.unity_tpk`; `worlds/unity/tests/serialized_census.rs`, `worlds/skatebird/tests/markers.rs`"#,
        source: r#"the user's own install"#,
    },
    Declared {
        query: Query::Install(Game::Sledding, Some("Sledding Game_Data")),
        status: Status::Live,
        gate: true,
        about: r#"a Unity player's data folder (`<Game>_Data`, holding `globalgamemanagers` and the `levelN` / `*.assets` files)"#,
        source: r#"the user's own install"#,
    },
    Declared {
        query: Query::Install(Game::Stuntboost, Some("Content/Effects")),
        status: Status::Live,
        gate: false,
        about: r#"STUNTBOOST's compiled effects (`D2/*.xnb`, `D3/*.xnb`: MonoGame `EffectReader`, MGFX version 12, OpenGL profile): every one is read to its shaders, parameters and passes; worlds/stuntboost/tests/effects.rs"#,
        source: r#"the user's own install"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Gta, "legion/blend-spec.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: a frozen resolution of that record against that Patch (a regression baseline of our own stage), `{"materials": {name: key}}`"#,
        source: r#"regression baseline: an earlier run of our own stage, frozen; not an independent oracle"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Gta, "legion/map"),
        status: Status::Live,
        gate: true,
        about: r#"a map folder the ReSkate writer built with `vertex_blend` materials (holding `vertex_blend.json`); `worlds/reskate/tests/postcompile_vbrecord.rs`"#,
        source: r#"our own writer's output: the stage's input, not a reference"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Sekiro, "ktx2-tiles"),
        status: Status::Live,
        gate: true,
        about: r#"a web tile export with KTX2 textures and its plain set, both written from the same prepared cell worlds (one conversion writing both sets, `primase_web::convert::tiles`): tiles.json, the tiles, images/, plain/tiles.json and its tiles; formats/web/tests/ktx2_audit.rs"#,
        source: r#"our own writer's output: examples/examples/sekiro_ktx2_tiles.rs writes it (slice A of Sekiro's m11_01, both sets in one conversion), so the plain set holds the KTX2 encoder's input; the checks are an independent decoder's (the pure-Rust basisu transcoder, against the plain set) and the viewer's WASM transcoder's (the printed hashes)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Sekiro, "layer-pilot"),
        status: Status::Live,
        gate: true,
        about: r#"layer-bake pipeline states of an area's layered materials (cases.json; per case, size cap and weight variant a state folder) with WARP's outputs for each (warp/); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/layer_pilot.rs"#,
        source: r#"the states: our own bake's (its stated policy), written by worlds/fromsoft/examples/layer_pilot_states.rs over the user's install (m11_01, region -170,180,-50,300); independent: Microsoft's WARP (d3d10warp 6.2.26100.9711) running the game's programs on them, through a thin quad host kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Sekiro, "outfits"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of the old C# character exporter's manifests, one `<id>.json` per character (`c1010`): the NPC row chosen, the display masks it showed (`options.DisplayMasks`), and per source mesh its material, display mask and whether it was included, hidden by its mask or left out as equipment; worlds/fromsoft/tests/npc_outfits.rs"#,
        source: r#"regression baseline: the manifests our earlier C# exporter (sekiro-skate SekiroExtract, CharacterExport.cs) wrote for the Sekiro character pack 0.2.x; our own output, so it detects a change of the selection and checks no correctness"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Sekiro, "scene"),
        status: Status::Live,
        gate: true,
        about: r#"our earlier exporter's skate-scene folder of an area: scene.json (region, materials), its collision tables (a regression baseline, not an independent reference) and textures; checks/tests/web_texture_decode.rs, worlds/fromsoft/tests/material_assignments.rs, shadow_census.rs"#,
        source: r#"regression baseline: our earlier C# exporter's scene of the area, not an independent reference; placements are checked against the SoulsFormats dump instead"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Skate, "fbmod"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of `.fbmod` files (searched recursively); each is read, written again byte for byte and every payload decoded (`tools.oodle9` for Oodle blocks); worlds/frostbite/tests/fbmod.rs. The costume files among them (top level, with Studio's `<name>.readback.glb` beside them when present) also gate the costume steps with `installs.skate/Data`: worlds/reskate/tests/costume.rs"#,
        source: r#"ReSkate Studio 2026-10-12 `cosmetic-mesh-import` output: our own producer's files, a round-trip baseline, not an oracle"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "active.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's live-file tables of that install (JSON; schema in `worlds/gta/tests/archive_set.rs`)"#,
        source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "ShadersGen9Conversion.xml"),
        status: Status::Live,
        gate: false,
        about: r#"optional: CodeWalker's `ShadersGen9Conversion.xml`, which names every effect's texture and constant parameters (current and previous name, constant-buffer offset); `worlds/gta/tests/program_tables.rs` holds the shader database's parameter records to it"#,
        source: r#"independent: CodeWalker.Core 485d56b, a data file of its repository (CodeWalker.Core/ShadersGen9Conversion.xml)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "bounds"),
        status: Status::Live,
        gate: true,
        about: r#"props' embedded collision bounds (a ydr's own, a yft's first physics LOD's) as an independent reader (CodeWalker) reads them: a folder of JSON files, one per prop, `{path, leaves: [{type, flags, polygons: [{type, material, vertices (placed)}] or corners (8, placed)}]}`; `lower-gta-embedded-collision`'s triangles are checked against them (needs `installs.gta`, `keys.gta`); `worlds/gta/tests/embedded_bounds.rs`"#,
        source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "frames.json"),
        status: Status::Live,
        gate: true,
        about: r#"one ydr's high-detail geometries as an independent reader (CodeWalker) reads them, as stored in GTA's frame: one JSON file `{path, value: {models: [{geometries: [{positions, normals, tangents (xyzw), texcoords, indices}]}]}}`; the frame change, triangle fronts and bitangent sign are checked against it (needs `installs.gta`, `keys.gta`), and its stored tangent frames against the frames its texture coordinates give; `worlds/gta/tests/handedness.rs`, `uv_frames.rs`"#,
        source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "gtxd.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's texture dictionary parents of that install with their source files (JSON; schema in `worlds/gta/tests/gtxd.rs`)"#,
        source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's listing of every archive of that install (JSON lines; schema in `worlds/gta/tests/census.rs`)"#,
        source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "parity"),
        status: Status::Live,
        gate: false,
        about: r#"optional: a folder holding `dump.jsonl`, an independent reader's decoding of every file the gate regions' placements reach (the CodeWalker harness's `batch` mode over the list `parity_list` writes; schema in `worlds/gta/tests/parity.rs`)"#,
        source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools (`batch` mode, written when needed)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "resources"),
        status: Status::Live,
        gate: true,
        about: r#"optional: a folder of an independent reader's decodings of chosen drawables and texture dictionaries of that install (JSON; schema in `worlds/gta/tests/resource_oracle.rs`)"#,
        source: r#"independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::CodeWalker, Game::Gta, "ycd"),
        status: Status::Live,
        gate: false,
        about: r#"a folder holding `dictionaries.jsonl`: an independent reader's decoding of every clip dictionary (`.ycd`) of that install, the CodeWalker harness's `batch` mode over the install's `.ycd` list (`DumpYcd`: per animation its frames, duration, bone ids, and per sequence block and bone track the channel types and SHA-256 of the channel values and of the evaluated vectors); the channel decoder's values are checked against it (needs `installs.gta`, `keys.gta`); `worlds/gta/tests/ycd_channels.rs`"#,
        source: r#"independent: CodeWalker.Core 485d56b (GPL-licensed: run as a tool, no code taken), through a thin dump harness kept with the private oracle tools (`batch` mode)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Elden, "shaders"),
        status: Status::Live,
        gate: false,
        about: r#"a folder (searched recursively) of the DXIL containers of Elden Ring's Shader Model 6 shader bundles (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; toolkit/dxil/tests/lift_corpus.rs"#,
        source: r#"the user's own install: the containers of its `/shader/*.shaderbnd.dcx` binders and `/shader/*.shaderbdlebnd.dcx` bundles (ray-tracing `[rt]` bundles included), worlds/fromsoft/examples/corpus_elden_shaders.rs"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsv"),
        status: Status::Live,
        gate: true,
        about: r#"TSV: codec, compressed path, oracle decoded path (tab-separated)"#,
        source: r#"independent: the games' own streams (Sekiro's DFLT DCX envelopes, skate.'s zlib CAS blocks, Riders Republic's terrain LZ4 blocks, GTA V's raw deflate archive files, cut by the corpus_* example programs) decoded by Python's zlib and the lz4 package 4.4.5, toolkit/compress/tests/oracles/codecs.py"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::General, "dxbc"),
        status: Status::Live,
        gate: true,
        about: r#"Sekiro's 390 pixel programs (`.ppo`), Microsoft's disassembly of each (`.ppo.asm`) and the bindings, inputs and constant layout read from that text (`.ppo.reflection.json`); toolkit/dxbc/tests/disassembly.rs, reflection.rs; schema in toolkit/dxbc/README.md"#,
        source: r#"independent: the game's own programs (`corpus.sekiro/shaders`) disassembled by Microsoft's fxc 10.0.26100 (`fxc /nologo /dumpbin`), the reflection read from the disassembly's text by toolkit/dxbc/tests/oracles/dxbc_corpus.py"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Gta, "legacy-effects"),
        status: Status::Unheld {
            why: "it needs GTA V Legacy's install, which no machine holds (the Enhanced edition has no effect files)",
        },
        gate: false,
        about: r#"a folder (searched recursively) of GTA V Legacy's compiled effects (`.fxc`, RAGE `rgxe`) as extracted from the install (the Enhanced edition has none); every one is read, its programs' containers checked and lifted; worlds/gta/tests/fxc_census.rs"#,
        source: r#"the user's own install: the effect files extracted from its archives"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Gta, "shader-database.awc"),
        status: Status::Live,
        gate: false,
        about: r#"GTA V Enhanced's shader database (`sga_win32_60_final.awc`, magic `SGD2`, from update/update2.rpf `common/shaders/fxdb`) as the install stores it after decryption and decompression: every program and effect is read, every program lifted; worlds/gta/tests/fxc_census.rs"#,
        source: r#"the user's own install: the file extracted from its archive, worlds/gta/examples/corpus_gta_shader_database.rs"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Riders, "shaders"),
        status: Status::Live,
        gate: false,
        about: r#"a folder (searched recursively) of the DXBC containers of Riders Republic's shader pack (ShaderDataDX11.bin, split at its containers), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs"#,
        source: r#"the user's own install: the programs split from its shader pack"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Sekiro, "dcx"),
        status: Status::Live,
        gate: true,
        about: r#"folder recursively containing loose KRAK DCX files"#,
        source: r#"the game's own files from the user's install: worlds/fromsoft/examples/corpus_sekiro_dcx.rs (KRAK envelopes as stored, the first 256 in hash order up to 4 MiB)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Sekiro, "lznib"),
        status: Status::Live,
        gate: true,
        about: r#"a folder of Sekiro's LZNIB streams (Oodle decoder type 2, `8C 02`): `<hash>.payload`, the stream as the archive stores it in its DCX envelope from the block header on, and `<hash>.decoded`, the game's Oodle library's output for it; worlds/fromsoft/tests/lznib_gate.rs"#,
        source: r#"the game's own library's output for the game's own streams: worlds/fromsoft/tests/krak_archive.rs with the debug key `oodle.dump` on a machine with the Oodle 6 library, the files it keeps for the streams oozextract cannot decode or decodes differently"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok"),
        status: Status::Live,
        gate: true,
        about: r#"one area's high-resolution map collision as its binder stores it: loose Havok 2016.2 tagfiles and the compendium they share, every body and compressed mesh read; toolkit/havok/tests/corpus.rs, schema in toolkit/havok/README.md"#,
        source: r#"the game's own files from the user's install: worlds/fromsoft/examples/corpus_sekiro_havok.rs (the area's `h*.hkxbhd`/`.hkxbdt` binder, each member out of its DCX envelope)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-soup"),
        status: Status::Live,
        gate: true,
        about: r#"an independent decoder's raw triangle soup of each `havok` tagfile, `<name>.hkx.soup`; schema in toolkit/havok/README.md; worlds/fromsoft/tests/collision.rs"#,
        source: r#"independent: a clean-room decoder written from the data alone (no primase decoding code, no third-party code), the primase-oracles harness dumps/havok-soup (5f6dde8, Python standard library) run over `corpus.sekiro/havok`; its README states the format facts and its counts.tsv the per-file triangles"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Sekiro, "object-hkx"),
        status: Status::Live,
        gate: true,
        about: r#"a folder of Havok tagfiles holding object convex bodies, checked against their stored half-spaces; `toolkit/havok/tests/convex_corpus.rs`"#,
        source: r#"the game's own tagfiles, checked against their own stored half-spaces"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Sekiro, "shaders"),
        status: Status::Live,
        gate: false,
        about: r#"a folder (searched recursively) of the DXBC containers of Sekiro's extracted pixel and vertex programs (`.ppo`, `.vpo`), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs; the detail-blend programs' equations derived and proven, worlds/fromsoft/tests/detail_derive.rs"#,
        source: r#"the user's own install: the programs extracted from its shader archives"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Skate, "cas"),
        status: Status::Live,
        gate: true,
        about: r#"directory of indexed individual `.casblock` frames"#,
        source: r#"the game's own CAS frames from the user's install: worlds/reskate/examples/corpus_skate_cas.rs (one Oodle block per frame, 16 entries per superbundle TOC)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Skate, "ebx"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of EBX documents as the game wrote them, mirroring their asset paths (searched recursively); every field's value must survive a rewrite, and the byte layout is compared with the reference writer; worlds/frostbite/tests/ebx.rs"#,
        source: r#"the game's own EBX documents"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Skate, "shaders"),
        status: Status::Live,
        gate: false,
        about: r#"a folder (searched recursively) of the DXIL containers of skate.'s CompiledBytecode resources (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; toolkit/dxil/tests/lift_corpus.rs"#,
        source: r#"the user's own install: the containers of its superbundles' CompiledBytecode resources, worlds/reskate/examples/corpus_skate_shaders.rs"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Corpus, Game::Sledding, "shaders"),
        status: Status::Live,
        gate: false,
        about: r#"a folder (searched recursively) of the DXBC containers of a Unity player's compiled pixel programs, as primase-unity extracts them, lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs"#,
        source: r#"the user's own install: the programs read from its shader assets"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::D3dDisassembler, Game::Riders, "shader-pack"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of DXBC containers `NNNN.dxbc` (Riders Republic's ShaderDataDX11.bin, Steam build 25394987, split at its containers: 2,443) each with `NNNN.txt`, Microsoft's D3D Shader Disassembler text of it; toolkit/dxbc/tests/disassembly.rs"#,
        source: r#"independent: Microsoft's D3D Shader Disassembler, run over the user's own install's pack (kept in a private cache)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::GameCode, Game::Stuntboost, "levels"),
        status: Status::Live,
        gate: true,
        about: r#"the game's own parsers run over that install: per level part `<L>_<N>.json` (node names as its `ParsedName` reads them; its collision model as `SE.CollisionModel` loads it: meshes, grippable bits, ramps, ledges) and `<L>_<N>.tris.bin` (every collision triangle); worlds/stuntboost/tests/oracle.rs"#,
        source: r#"independent: the game's own code via reflection, the oracle tool kept with the game's private tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Khronos, Game::General, "sample-assets"),
        status: Status::Live,
        gate: true,
        about: r#"a folder of Khronos glTF-Sample-Assets models, one folder per model (`<Model>/<Model>.glb` or `.gltf` with its buffers, `expected.json` written by pygltflib from the same file, `validator.json` by the Khronos validator and, for a model with images, `images.json` by Pillow; for a JPEG image `<Model>/images/<n>.jpg` and the RGBA8 rows Pillow decodes of it, `<Model>/images/<n>.rgba`)"#,
        source: r#"third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expected.json by pygltflib 1.16.5; images.json by Pillow 11.0.0"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Pillow, Game::Sekiro, "skate-mask"),
        status: Status::Live,
        gate: true,
        about: r#"PNGs and independent mask pixel hashes in `cases.json`; schema below"#,
        source: r#"independent: Pillow's decode and channel mapping of the game's own textures, generated by a script kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Port, Game::Sledding, "level1"),
        status: Status::Live,
        gate: true,
        about: r#"optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`)"#,
        source: r#"another exporter's output: an appearance baseline, not a reading of the source"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Port, Game::Sledding, "materials.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: another exporter's output folder for that game's `level1` and its materials joined to their objects (JSON; schema in `worlds/unity/tests/lower.rs`)"#,
        source: r#"another exporter's output, joined by a script kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Port, Game::Sledding, "terrain"),
        status: Status::Live,
        gate: true,
        about: r#"optional: that exporter's terrain pieces unpacked (raw arrays, `index.json`, its layer means; schema in `worlds/unity/tests/terrain.rs`)"#,
        source: r#"another exporter's terrain, unpacked by a script kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Reskate, Game::Skate, "renderskeleton.json"),
        status: Status::Live,
        gate: false,
        about: r#"skate.'s render skeleton as ReSkate's EBX reader reads it (`renderskeleton.json`: every bone's name, parent, name hash, rest and world basis rows and translation, inverse bind rows); worlds/reskate/tests/game_skeleton.rs"#,
        source: r#"ReSkate's EBX reader (the source primase's EBX reader is ported from) through our C++ harness `cos_skeleton` (games/sekiro tools/reskate-spec, commit 1e43b57): checks the port and the field reading, not the interpretation; the test's independent evidence is the asset's own poses agreeing with each other"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::RrAssetStudio, Game::Riders, "meshes"),
        status: Status::Live,
        gate: false,
        about: r#"RRAssetStudio v3's read of every Mesh in installs.riders (a folder: summary.jsonl, one row per Mesh with counts, submeshes and material ids, and SHA-256s of its positions as f32 and absolute indices as u32, little-endian; sample/; identity.json); worlds/anvil/tests/census.rs, run by hand"#,
        source: r#"a third-party oracle: RRAssetStudio v3 (zip SHA-256 f13edec5...) run over the user's own install by the primase-oracles harness dumps/rras-anvil (d5c48a9); identity.json pins its modules and the forges"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Scripts, Game::Sekiro, "dds"),
        status: Status::Live,
        gate: true,
        about: r#"a folder holding the DDS decoding worker `DdsWorker.dll` and the libraries it loads; protocol in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_pixels.rs"#,
        source: r#"independent: BCnEncoder.Net 2.2.1 (MIT OR Unlicense), through a thin decoding worker kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Scripts, Game::Sekiro, "normal-curl.json"),
        status: Status::Live,
        gate: true,
        about: r#"a JSON list of curl measurements of Sekiro normal maps by a script separate from the reader (`name`, `verdict` per map); `worlds/fromsoft/tests/normal_basis.rs`"#,
        source: r#"our own measurement script over the game's normal maps (separate from the reader under test, not a third-party reader)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Scripts, Game::Sekiro, "pbr"),
        status: Status::Live,
        gate: true,
        about: r#"source DDS in `census/dds` plus independent `texture-statistics.json` channel extrema and means; `worlds/fromsoft/tests/pbr_census.rs`"#,
        source: r#"the install's own DDS with statistics computed by a script kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Scripts, Game::Sekiro, "reflectance"),
        status: Status::Live,
        gate: true,
        about: r#"a folder: `reflectance.json`, naming one layered material's G-buffer program (path, sha256, the dielectric literal's offset and bits) and its DDS inputs (paths, sha256), every path relative to the folder, and the program and DDS files it names; the static bake must reproduce the traced reflectance at every texel while the other lowered channels stay unchanged; `worlds/fromsoft/src/sekiro/passes/bake_assembly.rs`"#,
        source: r#"the game's own shader program and textures from the user's install, with the literal's offset read from the program's bytes; the file only names and hashes them"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Scripts, Game::Sekiro, "shadow"),
        status: Status::Live,
        gate: true,
        about: r#"original decompressed `map.msb` and independent `shadows.json` (source SHA-256 and ordered part names plus six raw shadow fields); `worlds/fromsoft/tests/shadow_census.rs`"#,
        source: r#"the install's map.msb with shadows.json; the JSON's producer is not recorded yet"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Scripts, Game::Sekiro, "state"),
        status: Status::Live,
        gate: true,
        about: r#"independent MSB part entity IDs and groups (`entities.jsonl`) and event-script decode (`emevd.jsonl`); schema in worlds/fromsoft/README.md"#,
        source: r#"a private dump tool's MSB/EMEVD reading; only its build output is retained, so its independence is not yet shown"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Scripts, Game::Skate, "eclipse"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of the decodings of three skate. clips' ECLP resources (`c_proto_ofbnb_idle_lfoot`, `c_proto_onb_inair_bs_grab_method_loop`, `c_proto_onb_trick_o_varialheel_high`): per clip `<clip>.eclp` (the bytes), `<clip>.tracks.json` (formats and every rotation, translation and scalar track's frames and values), and `sha256.txt` of the bytes; worlds/reskate/tests/game_eclipse.rs"#,
        source: r#"our own earlier Python decoder (from the executable's encoder), a port check, not an independent oracle"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "census"),
        status: Status::Live,
        gate: false,
        about: r#"an independent census of installs.elden in oracles.soulsformats.sekiro/census's schema (the archives' decrypted headers as `<name>.plain`, entries.tsv, binders.tsv, errors.tsv, samples; hashes 16 hex digits; an archive's shard is its index in UXM's Elden Ring archive list); worlds/fromsoft/tests/census.rs, and asset_collision.rs (the asset collision binders entries.tsv lists)"#,
        source: r#"independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys, RSA and path hash, the Elden Ring harness's `census` mode (kept with the private oracle tools)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "format"),
        status: Status::Live,
        gate: false,
        about: r#"an independent reader's field-by-field reading of Elden Ring files read from installs.elden (JSON lines, floats as hex bit patterns): `msb.jsonl` (every map layout: its models, every part with kind, model, transform, entity IDs, shadow controls and hit filter, and its spawn point regions with name, position and rotation), `btl.jsonl` (every light bank: its version and lights, or the reference's refusal), `havok.jsonl` (chosen tiles' map collision and assets' collision: every compressed mesh shape's stored sections, vertices, primitives, shape keys and extern shape instances), `matbin.jsonl` (every material binder's members in member order: shader and source paths, key, every parameter's name, key, type and value, every sampler's type, path, key and two trailing floats), `metaparam.jsonl` (every shader bundle's metaparam: binder, bundle and member names, header words, texture slots, render passes, CAParams and MtdParams); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/msb_census.rs, btl_census.rs, havok_census.rs, matbin_census.rs, metaparam_census.rs"#,
        source: r#"independent: SoulsFormatsNEXT ee1dd61 with UXM 9501be8's keys and path hash, and HKLib ac40bc9 for Havok, the Elden Ring harness's format modes (kept with the private oracle tools)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Elden, "npc.jsonl"),
        status: Status::Live,
        gate: false,
        about: r#"JSON lines: one of the NPC param's header facts (`kind` param: member, regulation version, paramType, dataVersion, detected and definition row size, format flags, unk06, row count), one per row (`kind` row: id, name, the 32 modelDispMask cells), then one per Enemy and DummyEnemy part of every map the UXM dictionary names (`kind` enemy or dummy: map path, part name, model, NPCParamID); worlds/fromsoft/tests/npc_outfits.rs"#,
        source: r#"independent: SoulsFormatsNEXT ee1dd61 (RegulationDecryptor, PARAM with Paramdex's ER/Defs/NpcParam.xml as Smithbox cbd477a8 ships it, applied at the regulation's version; MSBE), the Elden Ring harness's `npc` mode (kept with the private oracle tools)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "census"),
        status: Status::Live,
        gate: true,
        about: r#"reference archive outputs; schema in worlds/fromsoft/README.md"#,
        source: r#"independent: SoulsFormats 9c0da6b with UXM's Sekiro dictionary, the dump harness's census mode (kept with the private oracle tools)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "frames"),
        status: Status::Live,
        gate: true,
        about: r#"a folder holding `frames.jsonl`: SoulsFormats' reading of a few Sekiro models as stored (per mesh its LOD0 face set, per vertex position, normal, tangents and UV0); `worlds/fromsoft/tests/handedness.rs`"#,
        source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (frames mode)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "m11_01"),
        status: Status::Live,
        gate: true,
        about: r#"a SoulsFormats dump of an area: parts.jsonl (every MSB map piece and object part with its model and matrix), models.jsonl, missing-rigid.jsonl (placed models it could not dump, with why) and per placed model a binary of mesh/vertex counts followed by position and normal f32 triples in model space; worlds/fromsoft/tests/rigid_census.rs, sekiro_scene.rs"#,
        source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`rigid` mode)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "materials"),
        status: Status::Live,
        gate: true,
        about: r#"independent MTD fields and block lengths (mtd.jsonl, mtd-boundaries.jsonl); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/mtd_census.rs"#,
        source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`mtd` mode): the fields from its MTD reader, the consumed block lengths from its writer's rewrite of each file"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "npc"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of SoulsFormats dumps of the Sekiro NPC param and of characters' materials: npcparam.json (NpcParam.param of gameparam.parambnd.dcx read by PARAM with Paramdex's NpcParam.xml: header facts, row size, every row's id, name and every cell by its internal name), per character id `<id>.materials.json` (the FLVER's materials by index, name and MTD, and each mesh's material index), and `enemies.jsonl` with `maps.json` (every Enemy and DummyEnemy part of the install's ten MSBS maps: map, model name, NPCParamID, ThinkParamID); worlds/fromsoft/tests/npc_outfits.rs"#,
        source: r#"independent: SoulsFormats 9c0da6b with Paramdex's SDT/Defs/NpcParam.xml (sha256 dc4be2da), through a thin dump harness kept with the private oracle tools (`params`, `chrmaterials` and `enemies` modes)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "objects"),
        status: Status::Live,
        gate: true,
        about: r#"the game's object main .hkx files (179, castle) with Havoc's reading of each (`<name>.hkx.json`: every object, its class and fields as stored, floats as bits); bodies, shape classes, convex cores and poses checked exactly, compressed meshes counted as uncovered; worlds/fromsoft/tests/object_collision.rs"#,
        source: r#"independent: Havoc (SoulsAssetPipeline d11caf9), through a thin dump harness kept with the private oracle tools (`havok-json` mode); no independent decoder reads Sekiro's compressed meshes (HKLib supports Havok 2018 only)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "parts"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of SoulsFormats dumps of the Wolf's parts, one folder per part name (`bd_m_9120_l`): the files of oracles.soulsformats.sekiro/skin's skeleton.json, meshes.jsonl, meshes/<i>.bin and chrbnd.json (the partsbnd's members), for the part's FLVER; worlds/fromsoft/tests/sekiro_parts.rs"#,
        source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`skin` mode, area `part:<name>`)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "skin"),
        status: Status::Live,
        gate: false,
        about: r#"a folder of SoulsFormats dumps of Sekiro characters, one folder per character id (`c1010`): skeleton.json (every FLVER bone's name, parent, stored transform and ComputeLocalTransform), meshes.jsonl (each mesh's dynamic flag, palette, face sets and layouts), meshes/<i>.bin (per vertex position, normal, bone indices and weights as stored), dummies.json, anibnd.json; worlds/fromsoft/tests/character_skin.rs"#,
        source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`skin` mode)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::SoulsFormats, Game::Sekiro, "tpf"),
        status: Status::Live,
        gate: true,
        about: r#"independent texture-package census (packages.jsonl, area-dds/, errors.tsv); schema in worlds/fromsoft/README.md; worlds/fromsoft/tests/tpf_census.rs, tpf_pixels.rs"#,
        source: r#"independent: SoulsFormats 9c0da6b, through a thin dump harness kept with the private oracle tools (`tpf` mode); area-dds/ holds every distinct DDS under `/map/`"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Studio, Game::Gta, "legion/patch"),
        status: Status::Live,
        gate: true,
        about: r#"ReSkate Studio's Patch folder (its `layout.toc`, `Win32` TOCs and CAS archives) of the map in `oracles.baseline.gta/legion/map`, compiled by a build that carries the vertex-blend weight (validated: 20261012); `worlds/reskate/tests/postcompile_fb.rs`, `postcompile_vbrecord.rs`"#,
        source: r#"third-party: ReSkate Studio 20261012's compile of that map"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Studio, Game::Skate, "physics"),
        status: Status::Live,
        gate: false,
        about: r#"ReSkate Studio's PhysicsResource of each collision object of a small map of known shapes (a triangle, a quad, a box and the box turned, an 8 x 8 and a 64 x 64 bumpy grid, a hull-mode box, a floor): `oracle.json` holds every object's input mesh (positions, indices), transform, collision mode and packed material, `<mesh>_<slot>.bin` Studio's resource of it; worlds/reskate/tests/physics.rs"#,
        source: r#"third-party: ReSkate Studio 20261012's compile-map of that map (--no-stream --no-lods --no-gi --time-of-day noon)"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Skatebird, "census.json"),
        status: Status::Live,
        gate: true,
        about: r#"an independent UnityPy object-table census of that folder (JSON; schema in `worlds/unity/tests/serialized_census.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Skatebird, "listing.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.skatebird/SkateBIRD_Data` (JSON; schema in `worlds/unity/tests/listing.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "frames.json"),
        status: Status::Live,
        gate: true,
        about: r#"an independent reader's (UnityPy) decoding of a few meshes in Unity's own frame (vertices, normals, tangents, UV0, submesh triangles); `worlds/unity/tests/handedness.rs`"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "listing.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's (UnityPy) object names and BuildSettings scene list of `installs.sledding/Sledding Game_Data` (JSON; schema in `worlds/unity/tests/listing.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "materials.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's materials of that folder (JSON; schema in `worlds/unity/tests/material.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "meshes.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's decoded meshes of that folder (JSON; schema in `worlds/unity/tests/mesh.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "samples.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's decoded sample objects of that folder (JSON; schema in `worlds/unity/tests/oracle.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "scan.json"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's scan of a level of that folder (JSON; schema in `worlds/unity/tests/hierarchy.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "shader-programs.json"),
        status: Status::Live,
        gate: true,
        about: r#"an independent reader's (UnityPy) compiled fragment programs of every shader (per pass the player variants, keywords, program type and code hash, and the constant buffers, textures and samplers); `worlds/unity/tests/program.rs`"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "shader-state.json"),
        status: Status::Live,
        gate: true,
        about: r#"an independent reader's (UnityPy) shader render state of every material (subshader tags; per pass LightMode, blend factors, zwrite and cull with their bound properties); `worlds/unity/tests/material.rs`"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "sun.json"),
        status: Status::Live,
        gate: true,
        about: r#"an independent reader's (UnityPy) record of the scan level's RenderSettings sun as stored, the render-pipeline asset classes, the shining directional lights and the main-light choice; `worlds/unity/tests/light.rs`"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::UnityPy, Game::Sledding, "textures"),
        status: Status::Live,
        gate: true,
        about: r#"optional: an independent reader's decoded textures of that folder (a folder of PNGs and `index.json`; schema in `worlds/unity/tests/texture.rs`)"#,
        source: r#"independent: UnityPy, through dump scripts kept with the private oracle tools"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Uxm, Game::Elden, "dictionary.txt"),
        status: Status::Live,
        gate: false,
        about: r#"caller-provided UTF-8 Elden Ring path dictionary"#,
        source: r#"third-party: UXM's Elden Ring path dictionary"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Uxm, Game::Sekiro, "dictionary.txt"),
        status: Status::Live,
        gate: true,
        about: r#"caller-provided UTF-8 Sekiro path dictionary"#,
        source: r#"third-party: UXM's Sekiro path dictionary"#,
    },
    Declared {
        query: Query::Oracle(OracleSet::Baseline, Game::Elden, "malenia-binding-report.json"),
        status: Status::Live,
        gate: false,
        about: r#"the lab's report of a read of Elden Ring's c2120 (Malenia) with game shaders (its `views[*].game_shader.bound` rows: per bound program the material constants as the lab bound them); worlds/fromsoft/tests/malenia_visibility.rs executes the petal program on those constants"#,
        source: r#"regression baseline: the primase-lab smoke report of an earlier run of our own lab, frozen; not an independent oracle"#,
    },
];

/// The scratch root: where real-data tests write what they keep (`[scratch] root`); the OS temporary folder when unset.
pub const SCRATCH: Entry = Entry {
    key: "scratch.root",
    about: "where real-data tests write their outputs (the OS temporary folder when unset)",
    source: "",
    gate: false,
};

/// Every debug key `PRIMASE_DEBUG` may set (`key=value;key=value`): switches and parameters of the real-data tests,
/// none of them data.
pub const DEBUG: &[(&str, &str)] = &[
    (
        "gta.archive_filter",
        "only archives whose path contains this text",
    ),
    (
        "gta.region",
        "only this named gate region of worlds/gta/tests/region_pipeline.rs",
    ),
    ("scene.filter", "only scene folders whose path contains this text"),
    (
        "allow_live",
        "`1` lets a real-data gate report input folders that changed during the run and continue",
    ),
    (
        "shader.diff_limit",
        "how many programs of a corpus the WARP comparison runs",
    ),
    (
        "shader.warp_all",
        "`1` compares every program the WARP oracle can bind",
    ),
    (
        "oodle.dump",
        "a folder under the scratch root: the Oodle gate writes there the payload and the game library's output of each stream oozextract refuses or decodes differently",
    ),
    (
        "skatebird.voronoi_level",
        "the SkateBIRD level the Voronoi bake test bakes (default `level1`)",
    ),
    (
        "skatebird.voronoi_digest",
        "the digest the Voronoi bake test's maps must have",
    ),
    (
        "fromsoft.bake_digest",
        "the digest the baked layer textures of Sekiro's m11_01 must have (printed by an earlier run)",
    ),
    (
        "anvil.bake_digest",
        "the digest the baked Riders Republic Materials must have",
    ),
    (
        "dxil.fuzz_runs",
        "how many mutated fixtures the DXIL fuzz test runs (default 3000)",
    ),
    (
        "fromsoft.memory_cap_gb",
        "the most GiB the FromSoft memory census lets a read hold before it aborts naming the stage (default 10; worlds/fromsoft/tests/memory_census.rs)",
    ),
    (
        "fromsoft.census_areas",
        "which areas the FromSoft memory census reads: a comma list of `<game>/<area>` or `<area>` (default every area it lists; one to a process so an abort loses no other)",
    ),
    (
        "fromsoft.detail_census",
        "which characters the detail-blend census samples: `<game>:<ids>;<game>:<ids>`, each id `cNNNN` (default `elden:c2120,c3251,c2030,c2010;sekiro:c1070`; worlds/fromsoft/tests/detail_census.rs)",
    ),
];
