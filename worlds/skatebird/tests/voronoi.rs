//! The `Bumped Specular Voronoi` bake of one SkateBIRD level, run as the lab runs it
//! (`installs.skatebird/SkateBIRD_Data` and `tools.unity_tpk`; the level the debug key `skatebird.voronoi_level` names,
//! default `level1`): read the level, resolve and lower it, bake the materials with `lower-skatebird-voronoi`, and
//! print a digest of every baked map. The digests are the evidence that a change under the bake (the executor behind
//! the shader) leaves its output byte for byte as it was: run it before and after and compare, or give the earlier
//! digest as the debug key `skatebird.voronoi_digest`.

#![allow(clippy::unwrap_used)]

mod inputs;

use std::collections::BTreeSet;

use inputs::{Game, Tool, debug, install_file, tool};
use primase_core::convert::Reader;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Name, Texture, World};
use primase_skatebird::SkatebirdReader;
use sha2::{Digest, Sha256};
use unity_formats::TypeTreePackage;
use unity_formats::player::Player;

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
        install_file(Game::Skatebird, "SkateBIRD_Data"),
        tool(Tool::UnityTpk),
    ) else {
        return;
    };
    let level = debug("skatebird.voronoi_level").unwrap_or_else(|| "level1".into());
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    // the top quality level: the bake does not depend on the skinned meshes' influences
    let mut reader = SkatebirdReader::new(&player, level.clone()).with_quality("Fantastic");
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
    if let Some(expected) = debug("skatebird.voronoi_digest") {
        assert_eq!(total, expected, "the baked maps differ from the digest given");
    }
}
