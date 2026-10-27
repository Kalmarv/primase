//! Source census plus hand-worked fixtures in material.rs. The disc is the source,
//! not an independent decoding oracle; joins test independently stored references.
use primase_shred::read::{
    material::{self, UniformNames},
    pak::Pak,
};
use primase_testdata::{Game, install};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = entry.unwrap_or_else(|e| panic!("{}: {e}", dir.display())).path();
        if p.is_dir() {
            archives(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pak")) {
            out.push(p);
        }
    }
}

#[test]
fn all_material_banks_and_source_reference_census() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    let mut textures = BTreeSet::new();
    let mut shaders: BTreeMap<String, UniformNames> = BTreeMap::new();
    let mut banks = None;
    for path in paths {
        let pak = Pak::open(&path).unwrap();
        for (i, e) in pak.entries().iter().enumerate() {
            let name = std::str::from_utf8(&e.name).unwrap();
            if let Some(stem) = name.strip_suffix(".thp") {
                textures.insert(stem.to_lowercase());
            }
            if let Some(stem) = name.strip_suffix(".psp")
                && !shaders.contains_key(stem)
            {
                shaders.insert(
                    stem.into(),
                    material::fragment_uniform_names(&pak.payload(i).unwrap())
                        .unwrap_or_else(|e| panic!("{stem}: {e}")),
                );
            }
        }
        if path
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("materials.pak"))
        {
            banks = Some(pak);
        }
    }
    let pak = banks.expect("MATERIALS.PAK");
    let mut all = Vec::new();
    let mut missing_shaders = 0;
    for (bank, prefix, count) in [
        ("actors", "actor", 17),
        ("characters", "char", 1065),
        ("environments", "env", 3020),
    ] {
        let stem = format!("database\\materials\\{bank}\\{prefix}_materials");
        let payload = |ext: &str| {
            let name = format!("{stem}.{ext}");
            let i = pak
                .entries()
                .iter()
                .position(|e| e.name == name.as_bytes())
                .unwrap();
            pak.payload(i).unwrap().into_owned()
        };
        let materials = material::bank(bank, &payload("mbp"), &payload("mdp"), |name| {
            if let Some(names) = shaders.get(name) {
                Ok(names.clone())
            } else {
                missing_shaders += 1;
                Ok(UniformNames::new())
            }
        })
        .unwrap();
        assert_eq!(materials.len(), count);
        all.extend(materials);
    }
    let (mut named, mut unnamed, mut empty, mut resolved, mut missing) = (0, 0, 0, 0, 0);
    let mut named_samplers = 0;
    for m in &all {
        for p in &m.parameters {
            if let Some(n) = &p.name {
                assert_eq!(primase_foundations::hash::sdbm(n.as_bytes()), p.hash);
                named += 1;
            } else {
                unnamed += 1;
            }
        }
        for s in &m.samplers {
            if let Some(name) = &s.name {
                assert_eq!(primase_foundations::hash::sdbm(name.as_bytes()), s.hash);
                assert_eq!(
                    s.name_source,
                    Some(primase_shred::components::SamplerNameSource::ExecutableStringSdbm)
                );
                named_samplers += 1;
            } else {
                assert!(s.name_source.is_none());
            }
            if s.texture.is_empty() {
                empty += 1;
            } else if textures.contains(&s.texture.to_lowercase()) {
                resolved += 1;
            } else {
                missing += 1;
            }
        }
    }
    eprintln!(
        "SHRED materials: {} banks / {} records; parameters named={named} unnamed={unnamed}; textures resolved={resolved} empty={empty} missing={missing}; missing shaders={missing_shaders}",
        3,
        all.len()
    );
    // Missing references are present in the shipped source, retained and counted,
    // never substituted. These are census expectations, not a claim of resolution.
    assert_eq!(
        (named, unnamed, empty, resolved, missing, missing_shaders),
        (28121, 44115, 15319, 14117, 985, 1)
    );
    assert_eq!(named_samplers, 13_757);
    eprintln!(
        "SHRED sampler names: {named_samplers} named / {} unnamed from executable-string dictionary",
        30_421 - named_samplers
    );
    let mut world = primase_core::World::new();
    assert_eq!(material::attach(&mut world, all).unwrap().len(), 4102);
}
