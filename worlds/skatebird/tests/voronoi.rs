//! The `Bumped Specular Voronoi` bake of one SkateBIRD level, run as the lab runs it (`PRIMASE_UNITY_V21_DATA` and
//! `PRIMASE_UNITY_TPK`; the level `PRIMASE_SKATEBIRD_VORONOI_LEVEL`, default `level1`): read the level, resolve and lower
//! it, bake the materials with `lower-skatebird-voronoi`, and print a digest of every baked map. The digests are the
//! evidence that a change under the bake (the executor behind the shader) leaves its output byte for byte as it was:
//! run it before and after and compare, or pass the earlier digest in `PRIMASE_SKATEBIRD_VORONOI_DIGEST`.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use primase_core::convert::Reader;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Name, Texture, World};
use primase_skatebird::SkatebirdReader;
use primase_testdata::real_data;
use primase_unity::TypeTreePackage;
use primase_unity::player::Player;
use sha2::{Digest, Sha256};

/// The lab's lowering of a Unity level, with the Voronoi bake after the materials.
const PASSES: [&str; 5] = [
    "resolve-state{assign-class=lod:lod0}",
    "select-lod{form=default}",
    "unity-drawn",
    "lower-unity-pbr",
    "lower-skatebird-voronoi{keywords=DIRECTIONAL,tier=3,cells=2,max-texels=256}",
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_baked_maps_of_a_level() {
    let (Some(data), Some(tpk)) = (
        real_data("PRIMASE_UNITY_V21_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
    ) else {
        return;
    };
    let level = std::env::var("PRIMASE_SKATEBIRD_VORONOI_LEVEL").unwrap_or_else(|_| "level1".into());
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut reader = SkatebirdReader::new(&player, level.clone());
    let (raw, _) = reader.read().unwrap();
    let roots: BTreeSet<_> = raw.entities().collect();
    let (mut world, _): (World, _) = reader.load(&raw, &roots).unwrap();
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_unity::register(&mut registry);
    primase_skatebird::register(&mut registry);
    let pipelines: Vec<_> = PASSES.iter().map(|text| registry.parse(text).unwrap()).collect();
    let manager = PassManager::new(registry);
    let started = std::time::Instant::now();
    let mut report = RunReport::default();
    for pipeline in &pipelines {
        manager.run(&mut world, pipeline, &mut report).unwrap();
    }
    for step in &report.steps {
        if step.pass.starts_with("lower-skatebird-voronoi") {
            println!("{}: {:?}", step.pass, step.log);
        }
    }
    println!("{level}: passes ran in {:.1} s", started.elapsed().as_secs_f64());
    let mut digests = Vec::new();
    for (e, t) in world.iter::<Texture>() {
        let Some(Name(name)) = world.get::<Name>(e) else {
            continue;
        };
        if !name.contains("(Voronoi") {
            continue;
        }
        let mut h = Sha256::new();
        for level in &t.levels {
            h.update(level);
        }
        digests.push(format!("{name}: {}x{} {}", t.width, t.height, hex(&h.finalize())));
    }
    digests.sort();
    for d in &digests {
        println!("{d}");
    }
    let total = {
        let mut h = Sha256::new();
        for d in &digests {
            h.update(d.as_bytes());
        }
        hex(&h.finalize())
    };
    println!("{level}: {} baked maps, digest {total}", digests.len());
    if let Ok(expected) = std::env::var("PRIMASE_SKATEBIRD_VORONOI_DIGEST") {
        assert_eq!(total, expected, "the baked maps differ from the digest given");
    }
}
