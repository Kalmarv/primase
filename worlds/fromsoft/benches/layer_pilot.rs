//! The program executor on the layer bake's own work: every pipeline state of `oracles.baseline.sekiro/layer-pilot` (as
//! `examples/layer_pilot_states.rs` writes them, `tests/layer_pilot.rs` holds them to WARP) evaluated on one thread,
//! as [`PipelineState::evaluate`] runs it. Before measuring it prints the pixels' digest (every target's bits and
//! written lanes, state by state): an optimisation keeps it, and the instruction and sample counts it prints.
//!
//! `cargo bench -p primase-fromsoft --bench layer_pilot`, with the real-data configuration (CONTRIBUTING.md); without
//! the oracle set it measures nothing.
#![allow(clippy::unwrap_used)]

#[path = "../tests/inputs/mod.rs"]
mod inputs;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use inputs::{Game, OracleSet, oracle};
use primase_foundations::parallel::Threads;
use primase_fromsoft::layer_bake::{Encoded, PipelineState};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// A state folder's encoding: `state.json` and every other file beside it.
fn encoded(folder: &Path) -> Encoded {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() && path.file_name().is_some_and(|n| n != "state.json") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            files.insert(name, std::fs::read(&path).unwrap());
        }
    }
    Encoded {
        description: serde_json::from_slice(&std::fs::read(folder.join("state.json")).unwrap()).unwrap(),
        files,
    }
}

fn layer_pilot(c: &mut Criterion) {
    let Some(root) = oracle(OracleSet::Baseline, Game::Sekiro, "layer-pilot") else {
        return;
    };
    let mut folders: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("state.json").is_file())
        .collect();
    folders.sort();
    let states: Vec<PipelineState> = folders
        .iter()
        .map(|f| PipelineState::decode(&encoded(f)).unwrap())
        .collect();
    let pixels: u64 = states.iter().map(|s| u64::from(s.size) * u64::from(s.size)).sum();
    // what the measured runs must keep: the pixels' bits, and the counts
    let mut all = Sha256::new();
    let (mut instructions, mut samples) = (0u64, 0u64);
    for s in &states {
        let raw = s.evaluate(Threads::ONE).unwrap();
        let mut h = Sha256::new();
        for target in &raw.pixels {
            for (bits, mask) in target {
                for b in bits {
                    h.update(b.to_le_bytes());
                }
                h.update([*mask]);
            }
        }
        all.update(h.finalize());
        instructions += raw.instructions;
        samples += raw.samples;
    }
    let digest: String = all.finalize().iter().map(|b| format!("{b:02x}")).collect();
    println!(
        "layer pilot: {} states, {pixels} pixels, {instructions} instructions, {samples} samples; digest {digest}",
        states.len()
    );
    let mut group = c.benchmark_group("layer pilot");
    group.throughput(Throughput::Elements(pixels));
    // a pass over every state takes about a second: ten of them, in the time they take
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(15));
    group.bench_function("evaluate every state", |b| {
        b.iter(|| {
            for s in &states {
                std::hint::black_box(s.evaluate(Threads::ONE).unwrap());
            }
        })
    });
    group.finish();
}

criterion_group!(benches, layer_pilot);
criterion_main!(benches);
