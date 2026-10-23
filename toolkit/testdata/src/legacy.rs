//! The older `PRIMASE_*` names, for this release only: what each stands for in the structured configuration
//! ([`OLDER`]). A configuration still setting one (the older flat file, or the environment) sets that query's file or
//! folder, debug key or `[run] require` ([`crate::config`]), and says once that it is deprecated. Tests ask by query;
//! nothing asks by an older name. The table and the older flat file go in the next release.
use std::ffi::OsString;

use crate::query::Query;
use crate::registry::{Game, OracleSet};

/// Every older name with what it stands for; [`Query::Retired`] for a name whose obligation its owners retired.
/// `PRIMASE_REQUIRE_REAL_DATA` stands for `[run] require` ([`crate::config`]), not for a query.
pub const OLDER: &[(&str, Query)] = {
    use Game::*;
    use OracleSet::*;
    use Query::*;
    &[
        ("PRIMASE_SCENES_GTA", Retired),
        ("PRIMASE_SCENES_SEKIRO", Retired),
        ("PRIMASE_MAPS_GTA", Retired),
        ("PRIMASE_GTA_DIR", Install(Gta, None)),
        ("PRIMASE_GTA_KEYS", Keys(Gta)),
        ("PRIMASE_GTA_LISTING", Oracle(CodeWalker, Gta, "listing.jsonl")),
        ("PRIMASE_GTA_ACTIVE", Oracle(CodeWalker, Gta, "active.json")),
        ("PRIMASE_GTA_RESOURCES", Oracle(CodeWalker, Gta, "resources")),
        ("PRIMASE_GTA_PARITY", Oracle(CodeWalker, Gta, "parity")),
        ("PRIMASE_GTA_GTXD", Oracle(CodeWalker, Gta, "gtxd.json")),
        ("PRIMASE_GTA_FRAMES", Oracle(CodeWalker, Gta, "frames.json")),
        ("PRIMASE_GTA_BOUNDS", Oracle(CodeWalker, Gta, "bounds")),
        (
            "PRIMASE_GTA_SHADER_CONVERSION",
            Oracle(CodeWalker, Gta, "ShadersGen9Conversion.xml"),
        ),
        ("PRIMASE_GTA_SHADERS", Oracle(Corpus, Gta, "legacy-effects")),
        (
            "PRIMASE_GTA_SHADER_DB",
            Oracle(Corpus, Gta, "shader-database.awc"),
        ),
        ("PRIMASE_GTA_FILTER", Debug("gta.archive_filter")),
        ("PRIMASE_GTA_REGION", Debug("gta.region")),
        ("PRIMASE_SCENE_FILTER", Debug("scene.filter")),
        ("PRIMASE_RESKATE_PATCH", Oracle(Studio, Gta, "legion/patch")),
        ("PRIMASE_RESKATE_BLEND_PATCH", Oracle(Studio, Gta, "legion/patch")),
        ("PRIMASE_RESKATE_BLEND_MAP", Oracle(Baseline, Gta, "legion/map")),
        (
            "PRIMASE_RESKATE_BLEND_SPEC",
            Oracle(Baseline, Gta, "legion/blend-spec.json"),
        ),
        ("PRIMASE_SKATE_DATA", Install(Skate, Some("Data"))),
        (
            "PRIMASE_UNITY_V21_DATA",
            Install(Skatebird, Some("SkateBIRD_Data")),
        ),
        (
            "PRIMASE_UNITY_V21_CENSUS",
            Oracle(UnityPy, Skatebird, "census.json"),
        ),
        (
            "PRIMASE_UNITY_V21_LISTING",
            Oracle(UnityPy, Skatebird, "listing.json"),
        ),
        (
            "PRIMASE_UNITY_DATA",
            Install(Sledding, Some("Sledding Game_Data")),
        ),
        ("PRIMASE_UNITY_TPK", Tool(crate::registry::Tool::UnityTpk)),
        ("PRIMASE_UNITY_LISTING", Oracle(UnityPy, Sledding, "listing.json")),
        ("PRIMASE_UNITY_ORACLE", Oracle(UnityPy, Sledding, "samples.json")),
        ("PRIMASE_UNITY_SCAN", Oracle(UnityPy, Sledding, "scan.json")),
        ("PRIMASE_UNITY_MESHES", Oracle(UnityPy, Sledding, "meshes.json")),
        ("PRIMASE_UNITY_TEXTURES", Oracle(UnityPy, Sledding, "textures")),
        (
            "PRIMASE_UNITY_MATERIALS",
            Oracle(UnityPy, Sledding, "materials.json"),
        ),
        ("PRIMASE_UNITY_SUN", Oracle(UnityPy, Sledding, "sun.json")),
        (
            "PRIMASE_UNITY_SHADERS",
            Oracle(UnityPy, Sledding, "shader-state.json"),
        ),
        (
            "PRIMASE_UNITY_PROGRAMS",
            Oracle(UnityPy, Sledding, "shader-programs.json"),
        ),
        ("PRIMASE_UNITY_FRAMES", Oracle(UnityPy, Sledding, "frames.json")),
        ("PRIMASE_UNITY_PORT", Oracle(Port, Sledding, "level1")),
        (
            "PRIMASE_UNITY_PORT_MATERIALS",
            Oracle(Port, Sledding, "materials.json"),
        ),
        ("PRIMASE_UNITY_PORT_TERRAIN", Oracle(Port, Sledding, "terrain")),
        ("PRIMASE_STUNTBOOST_DIR", Install(Stuntboost, None)),
        (
            "PRIMASE_STUNTBOOST_ORACLE",
            Oracle(GameCode, Stuntboost, "levels"),
        ),
        ("PRIMASE_ANVIL_DIR", Install(Riders, None)),
        (
            "PRIMASE_ANVIL_FORGE",
            Install(Riders, Some("DataPC_MAD_WhiteRoom_patch_01.forge")),
        ),
        (
            "PRIMASE_ANVIL_FORGE_BASE",
            Install(Riders, Some("DataPC_MAD_WhiteRoom.forge")),
        ),
        (
            "PRIMASE_ANVIL_FORGE_USNAT",
            Install(Riders, Some("DataPC_MAD_USNAT.forge")),
        ),
        (
            "PRIMASE_ANVIL_FORGE_USNAT_PATCH",
            Install(Riders, Some("DataPC_MAD_USNAT_patch_01.forge")),
        ),
        (
            "PRIMASE_ANVIL_RRAS_MESHES",
            Oracle(RrAssetStudio, Riders, "meshes"),
        ),
        ("PRIMASE_OUT", Scratch(None)),
        ("PRIMASE_WARP_SCRATCH", Scratch(Some("warp"))),
        ("PRIMASE_ALLOW_LIVE", Debug("allow_live")),
        ("PRIMASE_SCENE_SKATE", Tool(crate::registry::Tool::SceneSkate)),
        (
            "PRIMASE_GLTF_VALIDATOR",
            Tool(crate::registry::Tool::GltfValidator),
        ),
        ("PRIMASE_KHRONOS_GLTF", Oracle(Khronos, General, "sample-assets")),
        ("PRIMASE_OODLE5", Tool(crate::registry::Tool::Oodle5)),
        ("PRIMASE_OODLE6", Tool(crate::registry::Tool::Oodle6)),
        ("PRIMASE_OODLE9", Tool(crate::registry::Tool::Oodle9)),
        ("PRIMASE_DCX", Oracle(Corpus, Sekiro, "dcx")),
        ("PRIMASE_CAS_SAMPLES", Oracle(Corpus, Skate, "cas")),
        ("PRIMASE_CODEC_CORPUS", Oracle(Corpus, General, "codecs.tsv")),
        ("PRIMASE_SEKIRO_DIR", Install(Sekiro, None)),
        ("PRIMASE_ELDEN_DIR", Install(Elden, None)),
        ("PRIMASE_ELDEN_KEYS", Keys(Elden)),
        ("PRIMASE_ELDEN_NAMES", Oracle(Uxm, Elden, "dictionary.txt")),
        ("PRIMASE_ELDEN_BHD_ORACLE", Oracle(SoulsFormats, Elden, "census")),
        (
            "PRIMASE_ELDEN_FORMAT_ORACLE",
            Oracle(SoulsFormats, Elden, "format"),
        ),
        ("PRIMASE_FROMSOFT_NAMES", Oracle(Uxm, Sekiro, "dictionary.txt")),
        (
            "PRIMASE_FROMSOFT_RIGID_ORACLE",
            Oracle(SoulsFormats, Sekiro, "m11_01"),
        ),
        (
            "PRIMASE_FROMSOFT_SKIN_ORACLE",
            Oracle(SoulsFormats, Sekiro, "skin"),
        ),
        (
            "PRIMASE_FROMSOFT_PART_ORACLE",
            Oracle(SoulsFormats, Sekiro, "parts"),
        ),
        (
            "PRIMASE_FROMSOFT_BHD_ORACLE",
            Oracle(SoulsFormats, Sekiro, "census"),
        ),
        ("PRIMASE_FROMSOFT_FRAMES", Oracle(SoulsFormats, Sekiro, "frames")),
        (
            "PRIMASE_FROMSOFT_MATERIAL_ORACLE",
            Oracle(SoulsFormats, Sekiro, "materials"),
        ),
        ("PRIMASE_FROMSOFT_TPF_ORACLE", Oracle(SoulsFormats, Sekiro, "tpf")),
        (
            "PRIMASE_FROMSOFT_OBJECT_ORACLE",
            Oracle(SoulsFormats, Sekiro, "objects"),
        ),
        (
            "PRIMASE_FROMSOFT_SHADOW_ORACLE",
            Oracle(Scripts, Sekiro, "shadow"),
        ),
        ("PRIMASE_FROMSOFT_STATE_ORACLE", Oracle(Scripts, Sekiro, "state")),
        ("PRIMASE_FROMSOFT_PBR_ORACLE", Oracle(Scripts, Sekiro, "pbr")),
        (
            "PRIMASE_FROMSOFT_REFLECTANCE_ORACLE",
            Oracle(Scripts, Sekiro, "reflectance"),
        ),
        (
            "PRIMASE_FROMSOFT_NORMAL_CURL",
            Oracle(Scripts, Sekiro, "normal-curl.json"),
        ),
        ("PRIMASE_FROMSOFT_DDS_ORACLE", Oracle(Scripts, Sekiro, "dds")),
        (
            "PRIMASE_FROMSOFT_LAYER_PILOT",
            Oracle(Baseline, Sekiro, "layer-pilot"),
        ),
        ("PRIMASE_FROMSOFT_SCENE", Oracle(Baseline, Sekiro, "scene")),
        ("PRIMASE_WEB_KTX2_TILES", Oracle(Baseline, Sekiro, "ktx2-tiles")),
        ("PRIMASE_HAVOK_CORPUS", Oracle(Corpus, Sekiro, "havok")),
        (
            "PRIMASE_HAVOK_OBJECT_CORPUS",
            Oracle(Corpus, Sekiro, "object-hkx"),
        ),
        (
            "PRIMASE_SKATE_SKELETON_ORACLE",
            Oracle(Reskate, Skate, "renderskeleton.json"),
        ),
        (
            "PRIMASE_SKATE_ECLIPSE_REFERENCE",
            Oracle(Scripts, Skate, "eclipse"),
        ),
        ("PRIMASE_SKATE_MASK_ORACLE", Oracle(Pillow, Sekiro, "skate-mask")),
        ("PRIMASE_FROSTBITE_EBX", Oracle(Corpus, Skate, "ebx")),
        ("PRIMASE_RESKATE_CLI", Tool(crate::registry::Tool::ReskateCli)),
        ("PRIMASE_RESKATE_FBMOD", Oracle(Baseline, Skate, "fbmod")),
        ("PRIMASE_DXBC_SHADERS", Oracle(Corpus, General, "dxbc")),
        (
            "PRIMASE_DXBC_PACK_DISASSEMBLY",
            Oracle(D3dDisassembler, Riders, "shader-pack"),
        ),
        ("PRIMASE_SHADER_CORPUS_SEKIRO", Oracle(Corpus, Sekiro, "shaders")),
        ("PRIMASE_SHADER_CORPUS_RR", Oracle(Corpus, Riders, "shaders")),
        ("PRIMASE_SHADER_CORPUS_UNITY", Oracle(Corpus, Sledding, "shaders")),
        ("PRIMASE_SHADER_DIFF_LIMIT", Debug("shader.diff_limit")),
        ("PRIMASE_SHADER_WARP_ALL", Debug("shader.warp_all")),
        (
            "PRIMASE_SKATEBIRD_VORONOI_LEVEL",
            Debug("skatebird.voronoi_level"),
        ),
        (
            "PRIMASE_SKATEBIRD_VORONOI_DIGEST",
            Debug("skatebird.voronoi_digest"),
        ),
        ("PRIMASE_ANVIL_BAKE_DIGEST", Debug("anvil.bake_digest")),
        ("PRIMASE_FROMSOFT_BAKE_DIGEST", Debug("fromsoft.bake_digest")),
        ("PRIMASE_DXIL_FUZZ_RUNS", Debug("dxil.fuzz_runs")),
        ("PRIMASE_DXC", Tool(crate::registry::Tool::Dxc)),
        ("PRIMASE_FXC", Tool(crate::registry::Tool::Fxc)),
        ("PRIMASE_WARP", Tool(crate::registry::Tool::Warp)),
    ]
};

/// What the older name `name` stands for; `None` for a name that is not one.
pub fn translate(name: &str) -> Option<Query> {
    OLDER.iter().find(|(n, _)| *n == name).map(|(_, q)| *q)
}

/// Every older name set in the environment, with its value.
pub(crate) fn from_environment(
    environment: &impl Fn(&str) -> Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    OLDER
        .iter()
        .filter_map(|(name, _)| environment(name).map(|value| (*name, value)))
        .collect()
}
