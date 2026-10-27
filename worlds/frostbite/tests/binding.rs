//! Binding sets and pass bindings: a hand-laid set read to its groups, parameters and entries, the register rule over
//! it, the refusals, and the shared data that names the sets; and, on the game's data (the skate. install and its
//! Oodle 9 library), every binding set and pass binding of the game configuration, level root and customization TOCs
//! read, and the shaders of the game configuration's permutations compared with the rule: each constant buffer's size
//! with the group the rule names, each resource with the entry the rule names.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use frostbite_formats::MESHSET;
use frostbite_formats::binding::{self, BINDING_SET, PASS_BINDING};
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::permutation::{self};
use frostbite_formats::scope::Scope;
use iridia_interface::{D3dClass, ResourceKind};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_core::World;
use primase_frostbite::components::{
    FrostbiteBindings, FrostbiteMaterial, FrostbiteMeshSet, FrostbitePermutation, FrostbiteProgram,
};
use primase_frostbite::read::entry::read_entry;
use primase_frostbite::read::program::{self, resolve_materials};
use primase_testdata::{Game, Tool, install_file, tool};

#[test]
fn the_stock_bindings_read_and_agree_with_the_shaders() {
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
    // every binding set and pass binding of three TOCs reads to its end
    let (mut read, mut failed) = (0usize, Vec::new());
    for toc in [
        "Win32/globals.toc",
        "Win32/levels/game/dingolevel_root/dingolevel_root.toc",
        "Win32/customization.toc",
    ] {
        let listing = src.listing(toc).unwrap();
        for bundle in &listing.bundles {
            for e in bundle.entries().filter(|e| {
                e.kind == Kind::Res && matches!(e.resource_type, Some(BINDING_SET | PASS_BINDING))
            }) {
                let payload = src.payload(e.file, e.size as usize).unwrap();
                match binding::read(
                    e.resource_type.unwrap(),
                    &payload,
                    &bundle.manifest.res[e.index].rmeta,
                ) {
                    Ok(_) => read += 1,
                    Err(err) => failed.push(format!("{}: {err}", e.name)),
                }
            }
        }
    }
    println!(
        "{read} binding sets and pass bindings read, {} failed",
        failed.len()
    );
    for f in failed.iter().take(10) {
        println!("  FAILED {f}");
    }
    assert!(failed.is_empty(), "{} did not read", failed.len());

    // the game configuration's permutations (every 8th) against their shaders' resources
    let bundle = src
        .bundle(
            "Win32/globals.toc",
            "win32/configurations/gameconfigurations/delmargameconfiguration",
        )
        .unwrap();
    let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut permutations = 0usize;
    for entry in bundle
        .entries()
        .filter(|e| e.kind == Kind::Res && e.resource_type == Some(permutation::RASTER))
    {
        let mut w = World::new();
        let made = read_entry(&src, &bundle, &entry, &mut w).unwrap();
        let bindings = w.get::<FrostbiteBindings>(made[0]).unwrap().clone();
        let rule = bindings.bindings();
        permutations += 1;
        for &shader in &w.get::<FrostbitePermutation>(made[0]).unwrap().shaders {
            let interface = &w.get::<primase_core::shader::Shader>(shader).unwrap().interface;
            for r in &interface.resources {
                let Some(d3d) = r.provenance.d3d else { continue };
                if d3d.space != 0 {
                    continue;
                }
                let (key, ok) = match (&r.kind, d3d.class) {
                    (ResourceKind::ConstantBuffer { layout }, D3dClass::ConstantBuffer)
                        if matches!(d3d.register, 0 | 1 | 4) =>
                    {
                        (
                            format!("b{}", d3d.register),
                            rule.constant_buffer(d3d.register)
                                .is_some_and(|g| g.buffer_end() == layout.size),
                        )
                    }
                    (ResourceKind::ConstantBuffer { .. }, _) => {
                        (format!("b{} (no rule)", d3d.register), false)
                    }
                    (_, D3dClass::ShaderResource) => ("t".to_owned(), rule.resource(d3d.register).is_some()),
                    (_, class) => (format!("{class:?} (no rule)"), false),
                };
                let t = tally.entry(key).or_default();
                t.0 += usize::from(ok);
                t.1 += 1;
            }
        }
    }
    println!("{permutations} permutations: resources by the rule (matching, total): {tally:?}");
    for key in ["b4", "t"] {
        let (ok, all) = tally[key];
        assert!(ok * 100 >= all * 99, "{key}: {ok} of {all}");
    }
    let (ok, all) = tally["b1"];
    assert!(ok * 100 >= all * 95, "b1: {ok} of {all}");
}

/// A material's depot block fills the parameters of its programs' set 0 by name: on every fortieth bundle of
/// `Win32/items.toc` that holds a MeshSet (the materials of its keys, their programs' permutations), each parameter of
/// set 0's two groups is named by the material's block with the same type and count, and [`Bindings::parameter`] finds
/// it there.
#[test]
fn a_materials_block_names_the_parameters_of_set_0() {
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
    // by slot (set, group): the parameters, those the block names, those with the same type and count
    let mut tally: BTreeMap<(usize, usize), [usize; 3]> = BTreeMap::new();
    let mut materials = 0usize;
    for bundle in items
        .bundles
        .iter()
        .filter(|b| b.manifest.res.iter().any(|a| a.rtype == MESHSET))
        .step_by(40)
    {
        let mut bundles = vec![bundle.clone()];
        bundles.extend(globals.bundles.iter().cloned());
        bundles.extend(root.bundles.iter().cloned());
        let scope = Scope::new(&bundles);
        let all = program::materials(&src, &scope).unwrap();
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
        {
            let mut w = World::new();
            let made = read_entry(&src, bundle, &entry, &mut w).unwrap();
            resolve_materials(&src, &scope, &all, made[0], &mut w).unwrap();
            let set = w.get::<FrostbiteMeshSet>(made[0]).unwrap().clone();
            for &e in set.materials.values() {
                let m = w.get::<FrostbiteMaterial>(e).unwrap().clone();
                let Some(depot) = m.depot.as_ref().filter(|_| !m.programs.is_empty()) else {
                    continue;
                };
                materials += 1;
                let block: BTreeMap<u64, (u32, u16)> = depot
                    .parameters
                    .iter()
                    .map(|p| (p.name, (p.ty, p.count)))
                    .collect();
                for &program in &m.programs {
                    for &p in &w.get::<FrostbiteProgram>(program).unwrap().permutations {
                        let Some(b) = w.get::<FrostbiteBindings>(p) else {
                            continue;
                        };
                        let rule = b.bindings();
                        for (si, s) in b.sets.iter().enumerate() {
                            for (gi, g) in s.iter().flat_map(|s| s.groups.iter()).enumerate() {
                                for q in &g.params {
                                    let t = tally.entry((si, gi)).or_default();
                                    t[0] += 1;
                                    if let Some(&(ty, count)) = block.get(&q.name) {
                                        t[1] += 1;
                                        t[2] += usize::from((ty, count) == (q.ty, q.count));
                                    }
                                    if si == 0 {
                                        let slot = rule.parameter(q.name).unwrap();
                                        assert_eq!((slot.set, slot.group), (0, gi));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{materials} materials, (set, group) -> [parameters, named by the block, same type and count]: {tally:?}"
    );
    assert!(materials >= 20, "{materials} materials");
    for group in [(0, 0), (0, 1)] {
        let [all, named, same] = tally[&group];
        assert!(
            all > 0 && named == all && same == all,
            "set 0 group {}: {all} {named} {same}",
            group.1
        );
    }
}
