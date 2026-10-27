//! `bake-fromsoft-layers` on Sekiro's m11_01 (the region of `oracles.baseline.sekiro/scene`, as `baked_area.rs` reads
//! it), and a SHA-256 of every baked texture (by name: its size, its levels) and of the lot. The digest is the evidence
//! that a change under the bake (the program executor behind the shader) leaves its output byte for byte as it was.
//!
//! The full gate (`[run] gate = 'full'`) bakes the whole region (1,632 textures): run before and after and compare, or
//! give the earlier total as the debug key `fromsoft.bake_digest`. The smoke gate, every batch, bakes one fixed cell,
//! the centre one of the region's 3 by 3 grid (`common::cells`), and holds its digest to [`SMOKE_DIGEST`], committed: a
//! change that alters the bake there updates it, with the measurement that shows why.
#![allow(clippy::unwrap_used)]

mod inputs;

use inputs::{Game, OracleSet, Tool, install, oodle, oracle};
use primase_core::{Name, Texture, World};
use primase_fromsoft::{dcx::Decoder, sekiro};
use sha2::{Digest, Sha256};
mod common;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The smoke gate's digest of the centre cell's baked textures, and their count.
const SMOKE_DIGEST: (usize, &str) = (
    1121,
    "353669b67254a4627633e39b4fc7a26989f6a97722d4742ce64578e8621dee7e",
);

/// The centre cell of the 3 by 3 grid: the one the smoke gate bakes.
const SMOKE_CELL: usize = 4;

/// The baked textures of `region` of m11_01: their count and the digest of the lot.
fn baked_digest(
    install: &std::path::Path,
    dll: &primase_foundations::compress::oodle::Oodle,
    region: [f32; 4],
) -> (usize, String) {
    let mut world = World::default();
    sekiro::read_area(
        install,
        "m11_01",
        &Decoder {
            oodle: Some(dll),
            max_output: 1024 * 1024 * 1024,
        },
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: region,
                margin: 30.0,
            }),
            ..Default::default()
        },
        &mut world,
    )
    .unwrap();
    // the bake's rows on the threads the configuration allows: the same bake on any count
    world.insert_resource(common::threads());
    let started = std::time::Instant::now();
    let run = common::run(
        &mut world,
        &format!(
            "{}, resolve-fromsoft-shaders{{unbound-textures=preview}}, bake-fromsoft-layers{{max-size=512,sample-time=0}}, lower-fromsoft-materials{{layers=preview,water-colour=48,98,122,190}}",
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
    for (what, count) in &bake.log.approximations {
        println!("approximation ({count}): {what}");
    }
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
    (digests.len(), hex(&total.finalize()))
}

#[test]
fn the_baked_textures_of_m11_01() {
    let (Some(install), Some(dll), Some(reference)) = (
        install(Game::Sekiro),
        oodle(Tool::Oodle6),
        oracle(OracleSet::Baseline, Game::Sekiro, "scene"),
    ) else {
        return;
    };
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(reference.join("scene.json")).unwrap()).unwrap();
    let region: [f32; 4] = serde_json::from_value(manifest["region"].clone()).unwrap();
    let dll = common::oodle(dll).unwrap();
    if inputs::depth() == inputs::Depth::Smoke {
        let cell = common::cells(region)[SMOKE_CELL];
        let (count, total) = baked_digest(&install, &dll, cell);
        println!("smoke: cell {SMOKE_CELL} {cell:?}: {count} baked textures, digest {total}");
        assert_eq!(
            (count, total.as_str()),
            SMOKE_DIGEST,
            "the centre cell's baked textures differ from the committed digest"
        );
        return;
    }
    let (count, total) = baked_digest(&install, &dll, region);
    println!("{count} baked textures, digest {total}");
    if let Some(expected) = inputs::debug("fromsoft.bake_digest") {
        assert_eq!(total, expected, "the baked textures differ from the digest given");
    }
}
