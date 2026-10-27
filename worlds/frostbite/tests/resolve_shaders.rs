//! `resolve-frostbite-shaders` on the game's data (the skate. install and its Oodle 9 library; unset skips): on every
//! twentieth bundle of `Win32/items.toc` that holds a MeshSet, the materials of its keys resolved in a scope of the
//! bundle, the game configuration's and the level root's, and the pass run over each bundle's world. Every binding
//! the pass makes must verify (the component's own check against the world). The census is printed: the materials
//! bound and left unbound by reason, the constants of the bound stages by role (Material words against Unknown,
//! counted as constants and as bytes), the vertex inputs by role, and the texture slots by whether the sets name them
//! and the material's block holds a reference. Where the pass leaves materials unbound the shape of what it refused is
//! printed too: the programs of a row that draw pixels, and for each such program its listed permutations, the
//! distinct ones and the distinct input layouts among them. A material is counted once per bundle it is resolved in.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use frostbite_formats::MESHSET;
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::permutation::Kind as PermutationKind;
use frostbite_formats::scope::Scope;
use iridia_interface::{ConstantRole, InputRole};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_core::World;
use primase_core::pass::{Pass, PassLog};
use primase_core::{shader::ShaderBinding, shader::TextureSource};
use primase_frostbite::components::{FrostbiteMaterial, FrostbitePermutation, FrostbiteProgram};
use primase_frostbite::passes::ResolveFrostbiteShaders;
use primase_frostbite::read::entry::read_entry;
use primase_frostbite::read::program::{materials, resolve_materials};
use primase_testdata::{Game, Tool, install_file, tool};

/// A verify message with its digits taken out, so equal rules count together.
fn cut_numbers(why: &str) -> String {
    why.chars().filter(|c| !c.is_ascii_digit()).collect()
}

#[test]
fn the_pass_binds_the_stock_materials_it_can_choose() {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return;
    };
    let folder = Folder::open(data).unwrap();
    // SAFETY: the configured Oodle 9 library is the game's own.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }.unwrap();
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let items = src.listing("Win32/items.toc").unwrap();
    let globals = src.listing("Win32/globals.toc").unwrap();
    let root = src
        .listing("Win32/levels/game/dingolevel_root/dingolevel_root.toc")
        .unwrap();
    let mut log = PassLog::default();
    let (mut bundles, mut meshsets, mut bound) = (0usize, 0usize, 0usize);
    // constants and bytes by role (`material`, or `unknown: <why>` cut at its first colon)
    let mut constants: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    // per bound material: variants, stages, constants and the bytes they cover
    let mut sizes: Vec<[u64; 4]> = Vec::new();
    let mut violations: BTreeMap<String, (usize, String)> = BTreeMap::new();
    // the engine's lists (b0, b4) as signatures of (name, offset, size), distinct per list, and the constants that
    // remain when adjacent constants of one resource with the same Unknown reason are merged into runs
    let mut engine_lists: std::collections::BTreeSet<Vec<(Option<String>, u32, u32)>> =
        std::collections::BTreeSet::new();
    let (mut engine_constants, mut engine_runs, mut all_runs) = (0u64, 0u64, 0u64);
    let mut inputs: BTreeMap<String, usize> = BTreeMap::new();
    let mut textures: BTreeMap<&'static str, usize> = BTreeMap::new();
    // histograms: drawing programs per material with a row; per drawing program the listed permutations, the distinct
    // ones and the distinct input layouts (as "listed/distinct/layouts")
    let mut drawing_per_row: BTreeMap<usize, usize> = BTreeMap::new();
    let mut permutations_per_program: BTreeMap<String, usize> = BTreeMap::new();
    for bundle in items
        .bundles
        .iter()
        .filter(|b| b.manifest.res.iter().any(|a| a.rtype == MESHSET))
        .step_by(20)
    {
        let mut scope_bundles = vec![bundle.clone()];
        scope_bundles.extend(globals.bundles.iter().cloned());
        scope_bundles.extend(root.bundles.iter().cloned());
        let scope = Scope::new(&scope_bundles);
        let all = materials(&src, &scope).unwrap();
        let mut w = World::new();
        bundles += 1;
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
        {
            let made = read_entry(&src, bundle, &entry, &mut w).unwrap();
            resolve_materials(&src, &scope, &all, made[0], &mut w).unwrap();
            meshsets += 1;
        }
        ResolveFrostbiteShaders.run(&mut w, &mut log).unwrap();
        // every component of the world verifies, counted rather than stopped at (shaders and bindings both)
        for (component, _, why) in w.verify_components() {
            let slot = violations
                .entry(format!("{component}: {}", cut_numbers(&why)))
                .or_insert((0, format!("{}: {why}", bundle.name)));
            slot.0 += 1;
        }
        for (_, m) in w
            .iter::<FrostbiteMaterial>()
            .filter(|(_, m)| !m.programs.is_empty())
        {
            let mut drawing = 0usize;
            for &p in &m.programs {
                let program = w.get::<FrostbiteProgram>(p).unwrap();
                let listed: Vec<&FrostbitePermutation> = program
                    .permutations
                    .iter()
                    .filter_map(|&e| w.get::<FrostbitePermutation>(e))
                    .collect();
                if !listed.iter().any(|f| {
                    f.permutation.kind == PermutationKind::Raster && f.permutation.bytecode.len() == 2
                }) {
                    continue;
                }
                drawing += 1;
                let distinct: BTreeSet<u64> = listed.iter().map(|f| f.permutation.id).collect();
                let layouts: BTreeSet<Option<u64>> =
                    listed.iter().map(|f| f.permutation.input_layout).collect();
                *permutations_per_program
                    .entry(format!("{}/{}/{}", listed.len(), distinct.len(), layouts.len()))
                    .or_default() += 1;
            }
            *drawing_per_row.entry(drawing).or_default() += 1;
        }
        for (_, b) in w.iter::<ShaderBinding>() {
            bound += 1;
            let mut size = [0u64; 4];
            size[0] = b
                .stages
                .iter()
                .filter_map(|s| s.variant.as_ref().map(|k| k.value))
                .collect::<std::collections::BTreeSet<_>>()
                .len() as u64;
            for s in &b.stages {
                let mut runs: Vec<(u32, u32, u32, &str)> = Vec::new(); // (resource, offset, end, reason)
                let mut lists: BTreeMap<u32, Vec<(Option<String>, u32, u32)>> = BTreeMap::new();
                for c in &s.constants {
                    let ConstantRole::Unknown { why } = &c.role else {
                        all_runs += 1;
                        continue;
                    };
                    let engine = why.starts_with("the engine fills");
                    if engine {
                        engine_constants += 1;
                        lists
                            .entry(c.resource.0)
                            .or_default()
                            .push((c.name.clone(), c.offset, c.size));
                    }
                    match runs.last_mut() {
                        Some(r) if r.0 == c.resource.0 && r.2 == c.offset && r.3 == why => {
                            r.2 = c.offset + c.size
                        }
                        _ => {
                            runs.push((c.resource.0, c.offset, c.offset + c.size, why));
                            if engine {
                                engine_runs += 1;
                            }
                        }
                    }
                }
                all_runs += runs.len() as u64;
                engine_lists.extend(lists.into_values());
                size[1] += 1;
                size[2] += s.constants.len() as u64;
                size[3] += s.constants.iter().map(|c| u64::from(c.size)).sum::<u64>();
                for c in &s.constants {
                    let role = match &c.role {
                        ConstantRole::Material(_) => "material".to_owned(),
                        ConstantRole::Unknown { why } => {
                            format!("unknown: {}", why.split(':').next().unwrap_or(why))
                        }
                        other => format!("{other:?}"),
                    };
                    let t = constants.entry(role).or_default();
                    t.0 += 1;
                    t.1 += u64::from(c.size);
                }
                for i in &s.inputs {
                    let role = match &i.role {
                        InputRole::Stream(s) => format!("stream {s:?}"),
                        InputRole::Previous => "previous stage".to_owned(),
                        InputRole::Unknown { why } => {
                            format!("unknown: {}", why.split(':').next().unwrap_or(why))
                        }
                        other => format!("{other:?}"),
                    };
                    *inputs.entry(role).or_default() += 1;
                }
                for t in &s.textures {
                    let key = match t.source {
                        TextureSource::Missing => "missing",
                        _ => "another source",
                    };
                    *textures.entry(key).or_default() += 1;
                }
            }
            sizes.push(size);
        }
    }
    println!("{bundles} bundles, {meshsets} meshsets, {bound} bindings");
    println!("counts:");
    for (k, n) in &log.counts {
        println!("  {n:>7}  {k}");
    }
    println!("constants (count, bytes) by role:");
    for (k, (n, bytes)) in &constants {
        println!("  {n:>7} {bytes:>9}  {k}");
    }
    println!("vertex and pixel inputs by role:");
    for (k, n) in &inputs {
        println!("  {n:>7}  {k}");
    }
    println!("texture slots by source: {textures:?}");
    for (what, (n, example)) in &violations {
        println!(
            "VERIFY x{n}: {what}
    example: {example}"
        );
    }
    println!("{} kinds of verify failure", violations.len());
    println!(
        "engine constants {engine_constants} -> {engine_runs} runs; all constants {} -> {all_runs} with Unknown runs merged; {} distinct engine lists (name, offset, size) over {} stages",
        sizes.iter().map(|s| s[2]).sum::<u64>(),
        engine_lists.len(),
        sizes.iter().map(|s| s[1]).sum::<u64>()
    );
    for (what, i) in [
        ("variants", 0),
        ("stages", 1),
        ("constants", 2),
        ("constant bytes", 3),
    ] {
        let mut v: Vec<u64> = sizes.iter().map(|s| s[i]).collect();
        v.sort_unstable();
        if let (Some(&min), Some(&max)) = (v.first(), v.last()) {
            println!(
                "per bound material, {what}: total {} min {min} median {} mean {:.1} max {max}",
                v.iter().sum::<u64>(),
                v[v.len() / 2],
                v.iter().sum::<u64>() as f64 / v.len() as f64
            );
        }
    }
    println!(
        "materials with a program row, by the programs of the row that draw pixels: {drawing_per_row:?}"
    );
    println!("drawing programs by listed/distinct/layouts permutations: {permutations_per_program:?}");
    assert!(
        violations.is_empty(),
        "{} kinds of verify failure",
        violations.len()
    );
    // every material is bound or counted under a reason
    let counted: u64 = log
        .counts
        .iter()
        .filter(|(k, _)| k.starts_with("materials bound") || k.starts_with("materials left unbound"))
        .map(|(_, n)| n)
        .sum();
    assert_eq!(
        bound as u64,
        log.counts.get("materials bound").copied().unwrap_or(0),
        "{:?}",
        log.counts
    );
    assert!(counted > 0 && counted >= bound as u64, "{:?}", log.counts);
}
