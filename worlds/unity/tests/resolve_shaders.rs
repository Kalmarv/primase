//! `resolve-unity-shaders` on a level of Sledding Game (`level1`, Unity 2021, URP) and of SkateBIRD: its materials'
//! programs read (`read_programs`), every bound material's `core.shader_binding` verified (`Component::verify`), and
//! the materials bound and left unbound counted with the reason, for each hardware tier the variants have. Prints the
//! counts.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::ShaderBinding;
use primase_core::{Component, World};
use primase_testdata::{Game, Tool, install_file, tool};
use primase_unity::TypeTreePackage;
use primase_unity::components::{RenderPipeline, UnityMaterial, UnityPrograms, UnityRenderSettings};
use primase_unity::level::read_level;
use primase_unity::player::Player;
use primase_unity::program::read_programs;
use primase_unity::resolve_shaders::ResolveUnityShaders;

fn census(label: &str, data: &std::path::Path, tpk: &std::path::Path, level: &str) {
    let started = std::time::Instant::now();
    let player = Player::open(
        data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_level(&player, level, &mut w).unwrap();
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
    let w = w.subset(&closure);
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
    let sets = ["none".to_owned(), renderer];
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
            ResolveUnityShaders::from_options(&options)
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
}
