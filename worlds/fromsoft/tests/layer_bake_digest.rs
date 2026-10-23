//! `bake-fromsoft-layers` on Sekiro's m11_01 (the region of `oracles.baseline.sekiro/scene`, as `baked_area.rs` reads
//! it), and a SHA-256 of every baked texture (by name: its size, its levels) and of the lot. The digest is the evidence
//! that a change under the bake (the program executor behind the shader) leaves its output byte for byte as it was: run
//! before and after and compare, or give the earlier total as the debug key `fromsoft.bake_digest`.
#![allow(clippy::unwrap_used)]

use primase_core::{Name, Texture, World};
use primase_fromsoft::{dcx::Decoder, sekiro};
use primase_testdata::{Game, OracleSet, Tool, install, oracle, tool};
use sha2::{Digest, Sha256};
mod common;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_baked_textures_of_m11_01() {
    let (Some(install), Some(dll), Some(reference)) = (
        install(Game::Sekiro),
        tool(Tool::Oodle6),
        oracle(OracleSet::Baseline, Game::Sekiro, "scene"),
    ) else {
        return;
    };
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(reference.join("scene.json")).unwrap()).unwrap();
    let dll = common::oodle(dll).unwrap();
    let mut world = World::default();
    sekiro::read_area(
        &install,
        "m11_01",
        &Decoder {
            oodle: Some(&dll),
            max_output: 1024 * 1024 * 1024,
        },
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: serde_json::from_value(manifest["region"].clone()).unwrap(),
                margin: 30.0,
            }),
            ..Default::default()
        },
        &mut world,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let run = common::run(
        &mut world,
        &format!(
            "{}, resolve-fromsoft-shaders{{unbound-textures=preview}}, bake-fromsoft-layers{{max-size=512,sample-time=0}}, lower-fromsoft-materials{{layers=preview}}",
            common::LEGACY_GEOMETRY
        ),
    )
    .unwrap();
    let bake = &run.steps[run.steps.len() - 2];
    println!(
        "all passes {:.1} s; {}: {:?}",
        started.elapsed().as_secs_f64(),
        bake.pass,
        bake.log.counts
    );
    let mut digests = std::collections::BTreeMap::new();
    for (e, t) in world.iter::<Texture>() {
        let Some(Name(name)) = world.get::<Name>(e) else {
            continue;
        };
        if !name.starts_with("bake_") {
            continue;
        }
        let mut h = Sha256::new();
        h.update(t.width.to_le_bytes());
        h.update(t.height.to_le_bytes());
        for level in &t.levels {
            h.update(level);
        }
        digests.insert(name.clone(), hex(&h.finalize()));
    }
    let mut total = Sha256::new();
    for (name, d) in &digests {
        total.update(name.as_bytes());
        total.update(d.as_bytes());
    }
    let total = hex(&total.finalize());
    println!("{} baked textures, digest {total}", digests.len());
    if let Some(expected) = primase_testdata::debug("fromsoft.bake_digest") {
        assert_eq!(total, expected, "the baked textures differ from the digest given");
    }
}
