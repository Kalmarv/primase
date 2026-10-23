//! What a test may ask the real-data configuration for: the games' installs and keys, the tools, the oracle sets and
//! the debug keys, each declared once with what it is, where it comes from and whether the landing gate needs it. The
//! configuration file names them by their keys (`installs.sekiro`, `tools.warp`, `oracles.root`); see the crate
//! documentation for its layout.

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
        about: "the user's Elden Ring archive keys, one PKCS#1 `RSA PUBLIC KEY` PEM per archive named after its stem",
        source: "the community unpacker's published keys (UXM's ArchiveKeys.cs), written out by a script kept with the game's private tools",
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
        about: "an Oodle version-5 DLL (`oo2core_5_win64.dll`)",
        source: "the game's own DLL from the user's install",
        gate: true,
    },
    Entry {
        key: "tools.oodle6",
        about: "an Oodle version-6 DLL (`oo2core_6_win64.dll`)",
        source: "the game's own DLL from the user's install",
        gate: true,
    },
    Entry {
        key: "tools.oodle9",
        about: "an Oodle version-9 DLL (`oo2core_9_win64.dll`)",
        source: "the game's own DLL from the user's install",
        gate: true,
    },
    Entry {
        key: "tools.scene_skate",
        about: "our own skate-scene validator (`scene-skate.dll`, run with `dotnet`, or its executable)",
        source: "our own C# validator, rebuilt from its last source (commit d78228a): it refuses what its checks refuse, it is not an oracle",
        gate: true,
    },
    Entry {
        key: "tools.unity_tpk",
        about: "a type tree package (TPK, uncompressed or LZ4) with the class layouts for the Unity players read",
        source: "input data: class layouts for the player's Unity version (repacked with a script kept with the private oracle tools)",
        gate: true,
    },
];

/// Every oracle set, by [`OracleSet::key`]: its folder under `oracles.root` holds one folder per [`Game::key`].
pub const ORACLE_SETS: &[Entry] = &[
    Entry {
        key: "oracles.unitypy",
        about: "UnityPy's readings of a Unity player (object tables, samples, meshes, textures, materials, shaders, frames, listings)",
        source: "independent: UnityPy, through dump scripts kept with the private oracle tools",
        gate: true,
    },
    Entry {
        key: "oracles.codewalker",
        about: "CodeWalker's readings of a GTA V install (archive listings, live-file tables, texture parents, resources, frames, bounds)",
        source: "independent: CodeWalker.Core 485d56b, through a thin dump harness kept with the private oracle tools",
        gate: true,
    },
    Entry {
        key: "oracles.soulsformats",
        about: "SoulsFormats' readings of FromSoftware archives and files (rigid parts, skins, parts, census, frames, formats)",
        source: "independent: SoulsFormats 9c0da6b (Sekiro) and SoulsFormatsNEXT ee1dd61 (Elden Ring), through a thin dump harness kept with the private oracle tools",
        gate: true,
    },
    Entry {
        key: "oracles.uxm",
        about: "UXM's path dictionaries (`dictionary.txt`, UTF-8, one archive path per line)",
        source: "third-party: UXM-Selective-Unpack 9501be8's dictionaries",
        gate: true,
    },
    Entry {
        key: "oracles.game-code",
        about: "a game's own code run over its install (STUNTBOOST's parsers via reflection: one JSON per level part)",
        source: "independent: the game's own code via reflection, the oracle tool kept with the game's private tools",
        gate: true,
    },
    Entry {
        key: "oracles.rrassetstudio",
        about: "RRAssetStudio's read of every Mesh of a Riders Republic install (`summary.jsonl`, one row per Mesh)",
        source: "third-party: RRAssetStudio v3, run over the user's own install by the primase-oracles harness",
        gate: false,
    },
    Entry {
        key: "oracles.khronos",
        about: "Khronos glTF-Sample-Assets, one folder per model, with their expected readings",
        source: "third-party: Khronos glTF-Sample-Assets at commit 15cd02d304e54cb42a4db4eefe719f76190f4b68; expectations by pygltflib and Pillow",
        gate: true,
    },
    Entry {
        key: "oracles.studio",
        about: "ReSkate Studio's compiles of maps our writer built (Patch folders)",
        source: "third-party: ReSkate Studio 20261012's compile output",
        gate: true,
    },
    Entry {
        key: "oracles.reskate",
        about: "ReSkate's own readers' readings of skate. data (the render skeleton)",
        source: "ReSkate's EBX reader (the source primase's EBX reader is ported from), through our C++ harness: a port check",
        gate: false,
    },
    Entry {
        key: "oracles.d3d-disassembler",
        about: "Microsoft's D3D shader disassembly of a game's shader containers (`NNNN.dxbc` with its listing)",
        source: "independent: Microsoft's D3D Shader Disassembler, run over the user's own install",
        gate: false,
    },
    Entry {
        key: "oracles.pillow",
        about: "Pillow's decodings of game textures and what is computed from them (the skate mask cases)",
        source: "independent: Pillow's decode and channel mapping of the game's own textures, generated by a script kept with the private oracle tools",
        gate: true,
    },
    Entry {
        key: "oracles.port",
        about: "another exporter's output for a game (its level folder, materials joined to their objects, terrain unpacked)",
        source: "another exporter's output: an appearance baseline, not a reading of the source",
        gate: true,
    },
    Entry {
        key: "oracles.corpus",
        about: "the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames)",
        source: "the user's own installs: files extracted as stored",
        gate: false,
    },
    Entry {
        key: "oracles.scripts",
        about: "measurements by our own scripts over game data, separate from the readers they check (normal curl, texture statistics, shadow and state records, the eclipse reference)",
        source: "our own scripts kept with the private oracle tools: separate from the readers under test, not third-party; some producers are not recorded yet",
        gate: true,
    },
    Entry {
        key: "oracles.baseline",
        about: "our own earlier outputs kept to detect change (an earlier exporter's scene, a map our writer built, a frozen stage result)",
        source: "regression baselines: our own earlier outputs, not independent oracles",
        gate: true,
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
        "how many programs of a corpus the executor comparisons run",
    ),
    (
        "shader.diff_trace",
        "a DXBC container whose executor and evaluator requests and outputs are printed",
    ),
    (
        "shader.diff_case",
        "which test case shader.diff_trace prints (default 0)",
    ),
    (
        "shader.diff_text",
        "a DXBC container whose lifted program is printed as text",
    ),
    (
        "shader.warp_all",
        "`1` compares every program the WARP oracle can bind",
    ),
    (
        "shader.warp_evaluator",
        "`1` runs the DXBC evaluator in the executor's place in the WARP comparison",
    ),
    (
        "shader.speed_runs",
        "how many invocations of each program the executor timing runs (default 200)",
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
];
