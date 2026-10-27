//! `resolve-unity-shaders` on a level of Sledding Game (`level1`, Unity 2021, URP) and of SkateBIRD: its materials'
//! programs read (`read_programs`), every bound material's `core.shader_binding` verified (`Component::verify`), and
//! the materials bound and left unbound counted with the reason, for each hardware tier the variants have. Prints the
//! counts.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use primase_core::pass::{Options, PassLog};
use primase_core::shader::ShaderBinding;
use primase_core::{Component, World};
use primase_testdata::{Game, Tool, install_file, tool};
use primase_unity::components::{
    UnityLight, UnityMaterial, UnityPrograms, UnityRenderSettings, UnityRenderer,
};
use primase_unity::level::{LevelOptions, read_level_with};
use primase_unity::program::read_programs;
use primase_unity::resolve_shaders::RESOLVE_UNITY_SHADERS;
use unity_formats::TypeTreePackage;
use unity_formats::player::Player;
use unity_formats::player::RenderPipeline;

fn census(label: &str, data: &std::path::Path, tpk: &std::path::Path, level: &str) {
    let started = std::time::Instant::now();
    let player = Player::open(
        data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    // the quality level is a runtime setting: the highest the player lists
    let quality = player.quality_levels().unwrap().last().map(|l| l.name.clone());
    read_level_with(&player, level, &LevelOptions { quality }, &mut w).unwrap();
    println!("  level read in {:?}", started.elapsed());
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
    println!("  programs read in {:?}", started.elapsed());
    let materials = w.iter::<UnityMaterial>().count();
    // the materials alone (with what they refer to), so each run clones little
    let roots: BTreeSet<_> = w.iter::<UnityPrograms>().map(|(e, _)| e).collect();
    let closure = w.closure(roots.iter().copied());
    // the lights and renderers state the scene's draws; their meshes are not needed
    let lights: Vec<_> = w.iter::<UnityLight>().map(|(_, l)| l.clone()).collect();
    let renderers: Vec<_> = w.iter::<UnityRenderer>().map(|(_, r)| r.clone()).collect();
    let mut w = w.subset(&closure);
    for l in lights {
        let e = w.spawn();
        w.insert(e, l);
    }
    for r in renderers {
        let e = w.spawn();
        w.insert(e, r);
    }
    println!("  {} entities kept, {:?}", closure.len(), started.elapsed());
    // what the passes' variants require of a draw: the keywords every fragment variant of a pass has, by count of passes
    let mut required: std::collections::BTreeMap<Vec<String>, usize> = Default::default();
    for (_, p) in w.iter::<UnityPrograms>() {
        let mut always: Option<BTreeSet<String>> = None;
        for v in &p.variants {
            let k: BTreeSet<String> = v.keywords.iter().cloned().collect();
            always = Some(match always {
                None => k,
                Some(a) => a.intersection(&k).cloned().collect(),
            });
        }
        *required
            .entry(always.unwrap_or_default().into_iter().collect())
            .or_default() += 1;
    }
    println!("  keywords every fragment variant of a material's pass has, by materials: {required:?}");
    // the renderer's keywords: the commonest non-empty set every variant of a pass carries (the draw's, which the
    // renderer sets, not the material), stated here as the census's choice
    let renderer: String = required
        .iter()
        .filter(|(k, _)| !k.is_empty())
        .max_by_key(|(_, n)| **n)
        .map(|(k, _)| k.join("+"))
        .unwrap_or_default();
    println!("  renderer keywords chosen for the census: {renderer}");
    // and the scene's own state, which gives each material its draw's keywords
    let sets = ["none".to_owned(), renderer, "scene".to_owned()];
    println!(
        "{label} {level}: pipeline {pipeline:?}, {materials} materials, {} given programs, left out {:?}",
        report.attached, report.left_out
    );
    let tiers: BTreeSet<usize> = w
        .iter::<UnityPrograms>()
        .flat_map(|(_, p)| p.variants.iter().map(|v| v.tier))
        .collect();
    println!("  hardware tiers in the fragment variants: {tiers:?}");
    for tier in tiers {
        for keywords in &sets {
            let mut world = w.clone();
            let options = Options::from([
                ("tier".to_owned(), tier.to_string()),
                ("keywords".to_owned(), keywords.clone()),
                ("colour-space".to_owned(), "linear".to_owned()),
            ]);
            let mut log = PassLog::default();
            RESOLVE_UNITY_SHADERS
                .instance(&options)
                .unwrap()
                .run(&mut world, &mut log)
                .unwrap();
            let mut checked = 0;
            // the stated render state: a depth state on every bound material, and how many state a stencil, a depth
            // bias, a blend, a write mask that is not all four channels
            let (mut depth, mut stencil, mut bias, mut blended, mut masked) = (0usize, 0, 0, 0, 0);
            for (e, b) in world.iter::<ShaderBinding>() {
                b.verify(e, &world).unwrap_or_else(|why| panic!("{label}: {why}"));
                checked += 1;
                depth += usize::from(b.state.depth.is_some());
                stencil += usize::from(b.state.stencil.is_some());
                bias += usize::from(b.state.depth_bias.is_some());
                blended += usize::from(b.state.blend.is_some());
                masked += usize::from(b.state.targets[0].is_some_and(|t| t.write_mask != 0xf));
            }
            println!(
                "  render state: depth {depth}, stencil {stencil}, depth bias {bias}, blend {blended}, write mask not rgba {masked} of {checked}"
            );
            assert_eq!(depth, checked, "{label}: a bound material without a depth state");
            // SkateBIRD's forward base pass is stored only under a light type: the draw has it though no keyword says
            // so, and every material finds its variants
            if label == "SkateBIRD" && keywords == "none" {
                assert_eq!(
                    checked, materials,
                    "{label}: materials left unbound with no keywords stated"
                );
            }
            // the scene's own state gives every material its renderer keywords: all of both games' materials bind
            if keywords == "scene" {
                assert_eq!(
                    checked, materials,
                    "{label}: materials left unbound with the scene's keywords"
                );
            }
            println!(
                "  tier {tier}, keywords {keywords}: {checked} bindings verified, {:?}",
                started.elapsed()
            );
            for (what, n) in &log.counts {
                println!("    {n:>6}  {what}");
            }
        }
    }
}

#[test]
fn a_levels_materials_are_bound_or_left_with_a_reason() {
    let Some(tpk) = tool(Tool::UnityTpk) else {
        return;
    };
    if let Some(data) = install_file(Game::Sledding, "Sledding Game_Data") {
        census("Sledding Game", &data, &tpk, "level1");
    }
    if let Some(data) = install_file(Game::Skatebird, "SkateBIRD_Data") {
        census("SkateBIRD", &data, &tpk, "level1");
    }
}

/// The URP asset settings read for every quality level of Sledding Game agree with the variants the player keeps. The
/// build's shader stripping drops the variants of a keyword no level's asset enables (`RemoveIf(false)`: soft shadows,
/// light cookies), and, for light layers (`SelectOrRemove(true)`), the variants without it when every level enables it. A
/// layout that read the wrong bytes would not agree. The other settings are printed against the keywords they stand for:
/// a keyword their feature enables is still set per frame, so the stored variants keep both states.
#[test]
fn a_urp_assets_settings_agree_with_the_variants_the_player_keeps() {
    use unity_formats::pipeline::UnityUrpSettings;
    let Some(tpk) = tool(Tool::UnityTpk) else {
        return;
    };
    let Some(data) = install_file(Game::Sledding, "Sledding Game_Data") else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let levels = player.quality_levels().unwrap();
    let settings: Vec<(String, UnityUrpSettings)> = levels
        .iter()
        .map(|l| {
            let asset = player.urp_asset(Some(&l.name)).unwrap().unwrap();
            let (_, bytes) = player.behaviour(&asset).unwrap();
            (
                l.name.clone(),
                unity_formats::pipeline::read_urp_settings(&bytes).unwrap(),
            )
        })
        .collect();
    for (name, s) in &settings {
        println!("  {name}: {s:?}");
    }
    let mut w = World::new();
    let quality = levels.last().map(|l| l.name.clone());
    read_level_with(&player, "level1", &LevelOptions { quality }, &mut w).unwrap();
    let urp = w.resource::<UnityRenderSettings>().unwrap().urp.clone();
    assert_eq!(
        urp.as_ref(),
        settings.last().map(|(_, s)| s),
        "the level carries the read quality level's settings"
    );
    let shaders: BTreeSet<String> = w
        .iter::<UnityMaterial>()
        .filter_map(|(_, m)| m.shader.clone())
        .collect();
    let names: Vec<&str> = shaders.iter().map(String::as_str).collect();
    read_programs(&player, &mut w, &names, &RenderPipeline::Universal).unwrap();
    // (keyword, the setting it stands for, whether the build keeps only variants with it when every level enables it)
    type Flag = fn(&UnityUrpSettings) -> bool;
    let flags: [(&str, Flag, bool); 7] = [
        ("_SHADOWS_SOFT", |s| s.soft_shadows, false),
        ("_LIGHT_COOKIES", |s| s.light_cookies, false),
        ("_LIGHT_LAYERS", |s| s.light_layers, true),
        ("_ADDITIONAL_LIGHT_SHADOWS", |s| s.additional_light_shadows, false),
        (
            "_REFLECTION_PROBE_BLENDING",
            |s| s.reflection_probe_blending,
            false,
        ),
        (
            "_REFLECTION_PROBE_BOX_PROJECTION",
            |s| s.reflection_probe_box_projection,
            false,
        ),
        ("_REFLECTION_PROBE_ATLAS", |s| s.reflection_probe_atlas, false),
    ];
    for (keyword, flag, selected) in flags {
        let all = settings.iter().all(|(_, s)| flag(s));
        let any = settings.iter().any(|(_, s)| flag(s));
        // per pass: the stage variants (fragment and vertex) that mention the keyword
        let (mut users, mut always) = (0, 0);
        for (_, p) in w.iter::<UnityPrograms>() {
            for list in [&p.variants, &p.vertex] {
                let with = list
                    .iter()
                    .filter(|v| v.keywords.iter().any(|k| k == keyword))
                    .count();
                if with == 0 {
                    continue;
                }
                users += 1;
                always += usize::from(with == list.len());
                if all && selected {
                    assert_eq!(
                        with,
                        list.len(),
                        "{keyword}: every level sets it, yet {} has variants without it",
                        p.shader
                    );
                }
            }
        }
        println!(
            "  {keyword}: every level sets it {all}, some level {any}; {users} stages use it, {always} in every variant"
        );
        if !any && matches!(keyword, "_SHADOWS_SOFT" | "_LIGHT_COOKIES" | "_LIGHT_LAYERS") {
            assert_eq!(users, 0, "{keyword}: no level sets it, yet variants have it");
        }
    }
}
