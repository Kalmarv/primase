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

use primase_compress::oodle::{Oodle, Version};
use primase_core::pass::{Pass, PassLog};
use primase_core::shader::{ConstantRole, InputRole, ShaderBinding, TextureSource};
use primase_core::{Component, World};
use primase_frostbite::MESHSET;
use primase_frostbite::bundle::Kind;
use primase_frostbite::components::{FrostbiteMaterial, FrostbitePermutation, FrostbiteProgram};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::passes::ResolveFrostbiteShaders;
use primase_frostbite::permutation::Kind as PermutationKind;
use primase_frostbite::program::Scope;
use primase_testdata::{Game, Tool, install_file, tool};

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
        let all = src.materials(&scope).unwrap();
        let mut w = World::new();
        bundles += 1;
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
        {
            let made = src.read_entry(bundle, &entry, &mut w).unwrap();
            src.resolve_materials(&scope, &all, made[0], &mut w).unwrap();
            meshsets += 1;
        }
        ResolveFrostbiteShaders.run(&mut w, &mut log).unwrap();
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
        for (e, b) in w.iter::<ShaderBinding>() {
            bound += 1;
            b.verify(e, &w)
                .unwrap_or_else(|why| panic!("{} does not verify: {why}", bundle.name));
            for s in &b.stages {
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
    println!(
        "materials with a program row, by the programs of the row that draw pixels: {drawing_per_row:?}"
    );
    println!("drawing programs by listed/distinct/layouts permutations: {permutations_per_program:?}");
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
