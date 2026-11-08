//! The IR ports of the albedo and base-UV derivations (`albedo_ir`, `base_uv_ir`: `iridia_ir::Program`, `iridia-d3d9-ir`,
//! `iridia-analysis`) against the derivations on the older crate, over every record of the stored shader database: the
//! same records derive, to the same cut, with the same cut inputs and refusals, and the derived programs, the projection
//! and the re-encoding, evaluate to the same outputs on the same inputs.
#![allow(clippy::unwrap_used, reason = "explicit fixtures and census assertions")]
mod inputs;
mod support;

use anvil_formats::shaunwhite::shaders::{Database, Payload};
use inputs::{Game, install_file};
use iridia_exec::{PixelShader, ProgramShader, Sample};
use primase_shaunwhite::shaders::{albedo, albedo_ir, base_uv, base_uv_ir};

/// A deterministic float in [0, 1) for the `n`th value of a stream.
fn unit(seed: u64, n: u64) -> f32 {
    let mut x = seed ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Every texture answers a texel of its own, whatever the dimension, the offset or the kind of read: the two executors are asked
/// for the same texel by the same texture, and a sample the program makes without using the answer is not an error.
#[derive(Debug)]
struct Texels;

impl Texels {
    fn texel(texture: u32) -> [f32; 4] {
        let t = texture as f32;
        [0.2 + 0.1 * t, 0.4, 0.6 - 0.05 * t, 0.8]
    }
}

impl iridia_exec::Sampler for Texels {
    fn sample(&mut self, request: Sample) -> Result<[f32; 4], String> {
        Ok(Self::texel(request.texture))
    }

    fn sample_image(&mut self, request: iridia_exec::ImageSample<u32>) -> Result<[f32; 4], String> {
        Ok(Self::texel(request.texture))
    }
}

/// Two evaluations agree where their words are the same, or both NaN, or floats within a relative 1e-5.
fn same(a: u32, b: u32) -> bool {
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    a == b || (x.is_nan() && y.is_nan()) || (x - y).abs() <= 1e-5 * x.abs().max(y.abs()).max(1.0)
}

/// What a program's evaluations on `cases` inputs give, or why it gave none.
fn run(shader: &ProgramShader, seed: u64, cases: u64) -> Vec<Result<Vec<[u32; 4]>, String>> {
    let rows: Vec<Vec<[u32; 4]>> = shader
        .constant_buffers()
        .iter()
        .map(|(slot, rows)| {
            (0..*rows)
                .map(|r| {
                    std::array::from_fn(|l| {
                        unit(seed ^ u64::from(*slot), u64::from(r) * 4 + l as u64).to_bits()
                    })
                })
                .collect()
        })
        .collect();
    (0..cases)
        .map(|case| {
            let inputs: Vec<[u32; 4]> = (0..shader.input_count())
                .map(|i| {
                    std::array::from_fn(|l| {
                        (unit(seed.wrapping_add(case), (i * 4 + l) as u64) * 2.0).to_bits()
                    })
                })
                .collect();
            let buffers: Vec<(u32, &[[u32; 4]])> = shader
                .constant_buffers()
                .iter()
                .zip(&rows)
                .map(|((slot, _), rows)| (*slot, rows.as_slice()))
                .collect();
            shader
                .execute(&inputs, &buffers, &mut Texels)
                .map(|e| e.outputs)
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// The two programs evaluate the same on `cases` inputs: the same outputs, or the same refusal.
fn evaluate_alike(old: ProgramShader, new: ProgramShader, cases: u64) -> Result<(), String> {
    let (a, b) = (run(&old, 7, cases), run(&new, 7, cases));
    for (case, (x, y)) in a.iter().zip(&b).enumerate() {
        match (x, y) {
            (Ok(x), Ok(y)) => {
                if x.len() != y.len() {
                    return Err(format!("case {case}: {} outputs against {}", x.len(), y.len()));
                }
                for (o, (p, q)) in x.iter().zip(y).enumerate() {
                    if !p.iter().zip(q).all(|(a, b)| same(*a, *b)) {
                        return Err(format!(
                            "case {case} output {o}: {:?} against {:?}",
                            p.map(f32::from_bits),
                            q.map(f32::from_bits)
                        ));
                    }
                }
            }
            (Err(_), Err(_)) => {}
            (x, y) => return Err(format!("case {case}: {x:?} against {y:?}")),
        }
    }
    Ok(())
}

/// A cut input's description with the older crate's `Some(ResourceId(n))` spelled as the IR's `ResourceId(n)` (a global of
/// the IR names its resource, where the older one could name none).
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find("Some(ResourceId(") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "Some(".len()..];
        let close = rest.find("))").map_or(rest.len(), |c| c + 1);
        out.push_str(&rest[..close]);
        rest = rest.get(close + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// How many records of the database each census takes.
fn limit() -> usize {
    std::env::var("PRIMASE_IR_PORT_RECORDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(6)
}

fn database() -> Option<Database> {
    let path = install_file(Game::ShaunWhite, "ekshadersPChigh.bin")?;
    Some(anvil_formats::shaunwhite::shaders::read(&std::fs::read(path).unwrap()).unwrap())
}

#[test]
fn the_ir_albedo_derivation_is_the_derivation_on_every_stored_record() {
    let Some(database) = database() else { return };
    let (mut derived, mut refused, mut differing) = (0usize, 0usize, Vec::new());
    let mut tried = 0usize;
    for (i, record) in database.records.iter().enumerate() {
        // a derivation lifts the program once for every instruction and lane choice of its colour (a second a record), so the
        // census takes the records a derivation reaches that far with (`PRIMASE_IR_PORT_RECORDS` of them, 6 by default)
        let Payload::Effect(effect) = &record.payload else {
            continue;
        };
        let reaches = record.key.kind == 4
            && record.key.permutation & 63 == 3
            && (record.key.permutation >> 20) & 7 == 2
            && effect.pixel.is_some();
        if !reaches || tried >= limit() {
            continue;
        }
        tried += 1;
        println!("albedo record {i}: start");
        let started = std::time::Instant::now();
        let binding = support::binding(&database, i as u32);
        for influence in [albedo::Influence::Restored] {
            let old = albedo::derive(&database, i as u32, &binding, influence);
            let new = albedo_ir::derive(&database, i as u32, &binding, influence);
            println!("albedo record {i}: both derivations in {:.1?}", started.elapsed());
            match (old, new) {
                (Ok(old), Ok(new)) => {
                    derived += 1;
                    let (a, b) = (&old.derivation, &new.derivation);
                    let cut = |p: &iridia_d3d9::Projection| (p.after_word, p.temporary, p.lanes);
                    let cut_new = |p: &iridia_d3d9_ir::Projection| (p.after_word, p.temporary, p.lanes);
                    if cut(&a.projection) != cut_new(&b.projection)
                        || a.cut_inputs != b.cut_inputs
                        || a.cuts != b.cuts
                        || a.source_hash != b.source_hash
                    {
                        differing.push(format!(
                            "record {i}: cut {:?} against {:?}, cut inputs {:?} against {:?}",
                            cut(&a.projection),
                            cut_new(&b.projection),
                            a.cut_inputs,
                            b.cut_inputs
                        ));
                        continue;
                    }
                    let projected = evaluate_alike(
                        ProgramShader::new(old.program.clone()).unwrap(),
                        ProgramShader::from_ir(new.program.clone()).unwrap(),
                        24,
                    );
                    let reencoded = evaluate_alike(
                        ProgramShader::new(old.reencode.clone()).unwrap(),
                        ProgramShader::from_ir(new.reencode.clone()).unwrap(),
                        24,
                    );
                    if let Err(e) = projected {
                        differing.push(format!("record {i}: the derived program: {e}"));
                    }
                    if let Err(e) = reencoded {
                        differing.push(format!("record {i}: the re-encoded program: {e}"));
                    }
                }
                (Err(old), Err(new)) => {
                    refused += 1;
                    if plain(&old) != new {
                        differing.push(format!("record {i}: refused with {old:?} against {new:?}"));
                    }
                }
                (Ok(_), Err(new)) => differing.push(format!("record {i}: derives, the IR refuses: {new}")),
                (Err(old), Ok(_)) => differing.push(format!("record {i}: refused ({old}), the IR derives")),
            }
        }
    }
    println!(
        "albedo: {tried} records tried, {derived} derivations on both, {refused} refusals on both, {} differing",
        differing.len()
    );
    for d in differing.iter().take(20) {
        println!("  {d}");
    }
    assert!(
        derived > 0,
        "the database derives no albedo: the comparison saw nothing"
    );
    assert!(differing.is_empty(), "{} records differ", differing.len());
}

#[test]
fn the_ir_base_uv_derivation_is_the_derivation_on_every_stored_record() {
    let Some(database) = database() else { return };
    let Some((material, _)) = support::material("Cell01251_DataBlock", "Gen_Street_Trottoir01_Mat") else {
        return;
    };
    let (mut derived, mut refused, mut differing) = (0usize, 0usize, Vec::new());
    for (i, record) in database.records.iter().enumerate() {
        if !matches!(record.payload, Payload::Effect(_)) {
            continue;
        }
        let binding = support::binding(&database, i as u32);
        let old = base_uv::derive(&database, i as u32, &binding, &material);
        let new = base_uv_ir::derive(&database, i as u32, &binding, &material);
        match (old, new) {
            (Ok(old), Ok(new)) => {
                derived += 1;
                let (a, b) = (&old.derivation, &new.derivation);
                if a.words != b.words
                    || a.cut_inputs.iter().map(|s| plain(s)).collect::<Vec<_>>() != b.cut_inputs
                    || a.cuts != b.cuts
                    || a.source_hash != b.source_hash
                    || a.output_register != b.output_register
                    || old.constants.len() != new.constants.len()
                    || old.inputs.len() != new.inputs.len()
                {
                    differing.push(format!(
                        "record {i}: words {:?} against {:?}, cut inputs {:?} against {:?}",
                        a.words, b.words, a.cut_inputs, b.cut_inputs
                    ));
                    continue;
                }
                if let Err(e) = evaluate_alike(
                    ProgramShader::new(old.program.clone()).unwrap(),
                    ProgramShader::from_ir(new.program.clone()).unwrap(),
                    24,
                ) {
                    differing.push(format!("record {i}: the derived program: {e}"));
                }
            }
            (Err(old), Err(new)) => {
                refused += 1;
                if plain(&old) != new {
                    differing.push(format!("record {i}: refused with {old:?} against {new:?}"));
                }
            }
            (Ok(_), Err(new)) => differing.push(format!("record {i}: derives, the IR refuses: {new}")),
            (Err(old), Ok(_)) => differing.push(format!("record {i}: refused ({old}), the IR derives")),
        }
    }
    println!(
        "base UV: {derived} derivations on both, {refused} refusals on both, {} differing",
        differing.len()
    );
    for d in differing.iter().take(20) {
        println!("  {d}");
    }
    assert!(
        derived > 0,
        "the database derives no base UV: the comparison saw nothing"
    );
    assert!(differing.is_empty(), "{} records differ", differing.len());
}
