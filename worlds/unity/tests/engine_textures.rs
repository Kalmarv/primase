//! The textures of Unity's own that the programs of a level read and the material has no property of (a shadow map, a
//! reflection probe, the camera's depth), for Sledding Game (Unity 2021, URP) and SkateBIRD (the built-in pipeline) at the
//! census's tier and keywords `scene`: the pass counts each by name, as the frame's texture where the vocabulary has
//! one or `Missing` with the reason. The counts are printed (`TEXTURES game what: n`) and the names the pass maps are
//! asserted not to be left `Missing`.
//!
//! What the counts show (level1 of each, tier 3, `scene` keywords):
//!
//! - SkateBIRD's programs name their engine textures (`_ShadowMapTexture`, `unity_SpecCube0`, `unity_SpecCube1`,
//!   `unity_ProbeVolumeSH`, the grab textures).
//! - Sledding Game's Unity 2021 URP shader objects store no texture parameters in the pass's common parameters, and
//!   their DXBC containers have no RDEF (no resource names): a 2021.2 player keeps a variant's textures in its
//!   parameter blob (`m_ParameterBlobIndices`), which `primase_unity::program::parameter_blob` reads. Before it was
//!   read the programs named 1 of their 1,375 texture slots; now the engine textures are named like SkateBIRD's
//!   (`_MainLightShadowmapTexture`, `_GlossyEnvironmentCubeMap`, `_ScreenSpaceOcclusionTexture`, `_DBufferTexture0`..).
//!
//! Needs the installs and the type tree package.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::pass::{Options, PassLog};
use primase_testdata::{Game, Tool, install_file, tool};
use primase_unity::components::{UnityMaterial, UnityRenderSettings};
use primase_unity::level::{LevelOptions, read_level_with};
use primase_unity::program::read_programs;
use primase_unity::resolve_shaders::RESOLVE_UNITY_SHADERS;
use unity_formats::TypeTreePackage;
use unity_formats::player::Player;
use unity_formats::player::RenderPipeline;

/// The pass's counts about textures for `level1` of the game at `data`.
fn counts(label: &str, data: &std::path::Path, tpk: &std::path::Path) -> BTreeMap<String, u64> {
    let player = Player::open(
        data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    let quality = player.quality_levels().unwrap().last().map(|l| l.name.clone());
    read_level_with(&player, "level1", &LevelOptions { quality }, &mut w).unwrap();
    let pipeline = w
        .resource::<UnityRenderSettings>()
        .and_then(|s| s.pipeline.clone())
        .unwrap_or(RenderPipeline::BuiltIn);
    let shaders: BTreeSet<String> = w
        .iter::<UnityMaterial>()
        .filter_map(|(_, m)| m.shader.clone())
        .collect();
    let names: Vec<&str> = shaders.iter().map(String::as_str).collect();
    read_programs(&player, &mut w, &names, &pipeline).unwrap();
    let options = Options::from([
        ("tier".to_owned(), "3".to_owned()),
        ("keywords".to_owned(), "scene".to_owned()),
        ("colour-space".to_owned(), "linear".to_owned()),
    ]);
    let mut log = PassLog::default();
    RESOLVE_UNITY_SHADERS
        .instance(&options)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    let textures: BTreeMap<String, u64> = log
        .counts
        .into_iter()
        .filter(|(what, _)| what.contains("exture"))
        .collect();
    for (what, n) in &textures {
        println!("TEXTURES {label} {what}: {n}");
    }
    textures
}

#[test]
fn unitys_own_textures_are_the_frames_where_the_vocabulary_has_one() {
    let Some(tpk) = tool(Tool::UnityTpk) else {
        return;
    };
    let mapped = [
        "_CameraDepthTexture",
        "_CameraOpaqueTexture",
        "_GrabTexture",
        "_GrabScreen0",
        "_MainLightShadowmapTexture",
        "_ShadowMapTexture",
        "_ScreenSpaceOcclusionTexture",
        "unity_SpecCube0",
        "_GlossyEnvironmentCubeMap",
        "unity_ProbeVolumeSH",
    ];
    let mut frame = 0;
    if let Some(data) = install_file(Game::Skatebird, "SkateBIRD_Data") {
        let c = counts("SkateBIRD", &data, &tpk);
        frame += c
            .iter()
            .filter(|(what, _)| what.starts_with("texture parameters that are the frame's"))
            .map(|(_, n)| n)
            .sum::<u64>();
        // the shadow map of the built-in pipeline's forward pass is the screen-space term (SHADOWS_SCREEN on 192 draws)
        assert!(
            c.keys()
                .any(|what| what.contains("ScreenSpaceShadows: _ShadowMapTexture")),
            "{c:?}"
        );
    }
    if let Some(data) = install_file(Game::Sledding, "Sledding Game_Data") {
        let c = counts("Sledding", &data, &tpk);
        frame += c
            .iter()
            .filter(|(what, _)| what.starts_with("texture parameters that are the frame's"))
            .map(|(_, n)| n)
            .sum::<u64>();
        // the parameter blobs name the slots: none is left unnamed (1,374 of 1,375 were before they were read), and
        // the shadow map and the reflection cube are the frame's
        assert!(
            !c.contains_key("texture slots the parameters do not name (the engine's), left Missing"),
            "{c:?}"
        );
        assert!(
            c.keys()
                .any(|what| what.ends_with("ShadowMap: _MainLightShadowmapTexture")),
            "{c:?}"
        );
        for what in c.keys() {
            assert!(
                !mapped
                    .iter()
                    .any(|name| what.ends_with(&format!("left Missing: {name}"))),
                "{what}"
            );
        }
    }
    assert!(frame > 0 || tool(Tool::UnityTpk).is_none());
}
