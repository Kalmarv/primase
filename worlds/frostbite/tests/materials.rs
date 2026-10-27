//! A MeshSet's section keys resolved to materials the way the game resolves them: the ShaderBlockDepot block, the
//! program row (through its permutations to the shaders) and the streamable texture set of each key. On the game's
//! data (the skate. install and its Oodle 9 library): MeshSets of `Win32/items.toc` read, their keys resolved in a
//! scope of their own bundle, the game configuration's (`globals.toc`) and the level root's, and the unresolved parts
//! counted (a key the stock data itself leaves without a depot block or a row is named, not an error).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use frostbite_formats::MESHSET;
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::scope::Scope;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_core::World;
use primase_frostbite::components::{FrostbiteMaterial, FrostbiteMeshSet, FrostbiteProgram};
use primase_frostbite::read::entry::read_entry;
use primase_frostbite::read::program::{materials, resolve_materials};
use primase_testdata::{Game, Tool, install_file, tool};

#[test]
fn item_meshsets_resolve_their_materials() {
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
    let (mut meshsets, mut sections, mut keys) = (0usize, 0usize, 0usize);
    let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
    let (mut with_program, mut with_depot) = (0usize, 0usize);
    // every 40th bundle that holds a MeshSet: its own bundle, then the game configuration's and the level root's
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
        let materials = materials(&src, &scope).unwrap();
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
        {
            let mut w = World::new();
            let made = read_entry(&src, bundle, &entry, &mut w).unwrap();
            resolve_materials(&src, &scope, &materials, made[0], &mut w).unwrap();
            let set = w.get::<FrostbiteMeshSet>(made[0]).unwrap();
            meshsets += 1;
            sections += set.meshset.lods.iter().map(|l| l.sections.len()).sum::<usize>();
            for (key, e) in &set.materials {
                keys += 1;
                let m = w.get::<FrostbiteMaterial>(*e).unwrap();
                assert_eq!(m.key, *key);
                with_depot += usize::from(m.depot.is_some());
                with_program += usize::from(!m.programs.is_empty());
                for u in &m.unresolved {
                    *unresolved
                        .entry(u.split(':').next().unwrap().to_owned())
                        .or_default() += 1;
                }
                for &p in &m.programs {
                    assert!(!w.get::<FrostbiteProgram>(p).unwrap().permutations.is_empty());
                }
            }
        }
    }
    println!(
        "{meshsets} meshsets, {sections} sections, {keys} keys: {with_depot} with a depot block, {with_program} with programs; unresolved {unresolved:?}"
    );
    assert!(meshsets > 5 && keys > 20, "{meshsets} meshsets, {keys} keys");
    // the stock data leaves a few keys without a part; most must resolve
    let missing: usize = unresolved.values().sum();
    assert!(
        missing * 20 < keys * 3,
        "{missing} unresolved parts of {keys} keys"
    );
}
