//! Which pass a program of a material's row draws in, asked of the names of the pass bindings: a raster permutation's
//! shared data names a pass binding ([`primase_frostbite::permutation::SharedData::pass_binding`], the b4 group), and
//! the pass binding is a resource of a bundle with a name. Needs the skate. install and its Oodle 9 library: unset
//! skips. It prints (read-only, no assertion on the names):
//!
//! - `PASSBINDING`: every pass binding resource of the game configuration's and the level root's bundles: its id, its
//!   name, the group names it holds (hashes), and the number of constants;
//! - `USED`: for the materials of the census sample (every twentieth `Win32/items.toc` bundle with a MeshSet), each
//!   program of the row that draws pixels, by the pixel stage's output count: the pass binding its permutations name
//!   (id and the resource's name, or `unnamed`), counted over the permutations of all the materials.
//!
//! The prediction written before the run: if the resource names say the pass (`gbuffer`, `shadow`, `depth`, ...), the
//! programs of one output count (5, the G-buffer) use pass bindings whose names differ from those of the 1-output
//! programs, and every permutation of one program uses one pass binding. If the names are only ids, or one pass binding
//! serves both output counts, the name does not tell the pass.
//!
//! What the one run showed: the game configuration's and the level root's bundles hold 7 pass binding resources and
//! each is named `expressionshader/passbinding/<its id in decimal>`: the names are ids and say no pass. The census
//! sample's drawing permutations name only two pass bindings, neither among those 7 (they are in other bundles, so
//! `unnamed`): `cfa2fc622ebb3e18` for every permutation of the programs with 1, 5 and 6 pixel outputs and for 216
//! of the 288 with 2, `cfa2fc632597812c` for the other 72 of those with 2. Every program uses one pass binding. So one
//! pass binding serves the 1-output and the 5-output programs: the pass binding does not tell the pass.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::shader::Shader;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::MESHSET;
use primase_frostbite::binding::PASS_BINDING;
use primase_frostbite::bundle::Kind;
use primase_frostbite::components::{
    FrostbiteBindings, FrostbiteMaterial, FrostbitePermutation, FrostbiteProgram,
};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::program::Scope;
use primase_testdata::{Game, Tool, install_file, tool};

#[test]
fn the_names_of_the_pass_bindings_a_materials_programs_use() {
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
    // every pass binding resource of the configuration's and the level root's bundles, by id
    let mut names: BTreeMap<u64, String> = BTreeMap::new();
    for bundle in globals.bundles.iter().chain(root.bundles.iter()) {
        for e in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(PASS_BINDING))
        {
            let id = e.resource_id.unwrap();
            let payload = src.payload(e.file, e.size as usize).unwrap();
            let set =
                primase_frostbite::binding::read(PASS_BINDING, &payload, &bundle.manifest.res[e.index].rmeta)
                    .unwrap();
            let groups: Vec<String> = set
                .groups
                .iter()
                .map(|g| format!("{:016x}x{}", g.name, g.params.len()))
                .collect();
            println!("PASSBINDING id {id:016x} name {:?} groups {groups:?}", e.name);
            names.insert(id, e.name.clone());
        }
    }
    println!("{} pass binding resources", names.len());
    // by pixel output count: the pass binding (id) of each permutation, counted
    let mut used: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut per_program: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
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
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
        {
            let made = src.read_entry(bundle, &entry, &mut w).unwrap();
            src.resolve_materials(&scope, &all, made[0], &mut w).unwrap();
        }
        let materials: Vec<FrostbiteMaterial> =
            w.iter::<FrostbiteMaterial>().map(|(_, m)| m.clone()).collect();
        for m in materials {
            for &p in &m.programs {
                let program = w.get::<FrostbiteProgram>(p).unwrap();
                let mut targets = BTreeSet::new();
                let mut passes = BTreeSet::new();
                for &e in &program.permutations {
                    let f = w.get::<FrostbitePermutation>(e).unwrap();
                    if f.permutation.bytecode.len() != 2 {
                        continue;
                    }
                    let outs = f
                        .shaders
                        .get(1)
                        .and_then(|&s| w.get::<Shader>(s))
                        .map(|s| s.interface.outputs.len())
                        .unwrap();
                    targets.insert(outs);
                    let pb = w.get::<FrostbiteBindings>(e).and_then(|b| b.shared.pass_binding);
                    let label = match pb {
                        Some(id) => format!("{id:016x} {}", names.get(&id).map_or("unnamed", String::as_str)),
                        None => "no pass binding".to_owned(),
                    };
                    passes.insert(label.clone());
                    *used.entry(outs).or_default().entry(label).or_default() += 1;
                }
                if let Some(&t) = targets.iter().next() {
                    per_program.entry(t).or_default().push(passes.len());
                }
            }
        }
    }
    for (outs, by) in &used {
        println!("USED pixel outputs {outs}: {by:?}");
    }
    for (outs, v) in &per_program {
        let mut hist: BTreeMap<usize, usize> = BTreeMap::new();
        for n in v {
            *hist.entry(*n).or_default() += 1;
        }
        println!("PER_PROGRAM pixel outputs {outs}: distinct pass bindings per program {hist:?}");
    }
    assert!(!names.is_empty() && !used.is_empty());
}
