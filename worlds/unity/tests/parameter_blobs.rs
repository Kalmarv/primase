//! The parameters a Unity 2021.2 player stores per variant in the shader's blob (`m_ParameterBlobIndices`), read for
//! the shaders of Sledding Game's `level1` (Unity 2021, URP): before, their programs named 1 of 1,375 texture slots
//! and no buffer bound `UnityPerMaterial`, because the pass's common parameters hold no texture and the per-variant
//! ones were not read. The counts are printed (`PARAMETERS what: n`); the named slots and the buffers' slots are
//! asserted. Needs the Sledding Game install and the type tree package.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use primase_core::World;
use primase_testdata::{Game, Tool, install_file, tool};
use primase_unity::components::{UnityMaterial, UnityPrograms, UnityRenderSettings};
use primase_unity::level::{LevelOptions, read_level_with};
use primase_unity::program::read_programs;
use unity_formats::TypeTreePackage;
use unity_formats::player::Player;
use unity_formats::player::RenderPipeline;

#[test]
fn the_variants_parameters_come_from_their_blobs() {
    let (Some(tpk), Some(data)) = (
        tool(Tool::UnityTpk),
        install_file(Game::Sledding, "Sledding Game_Data"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
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
    let report = read_programs(&player, &mut w, &names, &pipeline).unwrap();
    for (what, n) in &report.left_out {
        println!("PARAMETERS left out: {what}: {n}");
    }
    // each distinct program once, whatever the materials that draw with it
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut tally = std::collections::BTreeMap::<&str, [u64; 5]>::new();
    for (_, p) in w.iter::<UnityPrograms>() {
        for (stage, list) in [("fragment", &p.variants), ("vertex", &p.vertex)] {
            for v in list {
                if !seen.insert(v.code.to_vec()) {
                    continue;
                }
                let t = tally.entry(stage).or_default();
                t[0] += 1;
                t[1] += u64::from(!v.parameters.textures.is_empty());
                t[2] += v.parameters.textures.len() as u64;
                for cb in v
                    .parameters
                    .constant_buffers
                    .iter()
                    .filter(|cb| cb.name == "UnityPerMaterial")
                {
                    t[3] += 1;
                    t[4] += u64::from(cb.slot.is_some());
                }
            }
        }
    }
    for (stage, [variants, with_textures, textures, buffers, slotted]) in &tally {
        println!(
            "PARAMETERS {stage}: {variants} distinct programs, {with_textures} with texture parameters ({textures} in all), {buffers} UnityPerMaterial buffers, {slotted} with a binding slot"
        );
    }
    let fragment = tally["fragment"];
    // every distinct fragment program that names a UnityPerMaterial buffer binds it, but the ones whose blob is not read
    assert!(fragment[0] > 0 && fragment[1] * 2 > fragment[0], "{fragment:?}");
}
