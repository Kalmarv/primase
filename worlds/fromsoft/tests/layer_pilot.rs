//! The layer bake against Microsoft's WARP rasterizer running the same programs on the same state. The bake's policy (the
//! frozen frame, the vertex feeds, the layer weights) is ours and stated; what WARP checks is the execution: the vertex
//! program at the cell's corners, the pixel program over the grid with the rasterizer's interpolation, and base-mip
//! bilinear sampling with the sRGB albedo linearized before filtering.
//!
//! `oracles.baseline.sekiro/layer-pilot` holds `cases.json` and, per case, size cap and layer-weight variant, a folder
//! with the bake's pipeline state as `layer_pilot_states` writes it (`state.json` and its files:
//! [`primase_fromsoft::layer_bake::PipelineState::encode`]) and `warp/`, what the private WARP host's quad mode made of
//! it: `warp.json` (its status by stage), `target<r>.f32` (each target register's four f32 lanes per pixel, row-major),
//! `alpha.f32` (the albedo alpha sampled beside the program) and `vertex-outputs.json` (the vertex program's output
//! registers at the four corners, as the state's `outputs`; null for a register stream output cannot carry, which
//! `warp.json` lists under `vertex.not_streamed`: the clip distances, which no pixel input reads).
//!
//! Our executor's and WARP's outputs each go through the bake's own rules ([`PipelineState::bake`]): the albedo and
//! normal bytes must agree within one level, the second output's x and the emission within one level of 1/255 or a
//! thousandth of the value, the albedo alpha within one level, and the vertex outputs within four units in the last
//! place. Float differences are reported per target.
use primase_core::parallel::Threads;
use primase_fromsoft::layer_bake::{Encoded, PipelineState, TARGETS};
use primase_shader::Raw;
use primase_testdata::{Game, OracleSet, oracle};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A state folder's encoding: `state.json` and every other file beside it.
fn encoded(folder: &Path) -> Result<Encoded> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(folder)? {
        let path = entry?.path();
        if path.is_file() && path.file_name().is_some_and(|n| n != "state.json") {
            let name = path.file_name().ok_or("name")?.to_string_lossy().into_owned();
            files.insert(name, std::fs::read(&path)?);
        }
    }
    Ok(Encoded {
        description: serde_json::from_slice(&std::fs::read(folder.join("state.json"))?)?,
        files,
    })
}

/// Little-endian f32 words.
fn words(bytes: &[u8]) -> Result<Vec<u32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err("a .f32 file of other than whole words".into());
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect())
}

/// How far apart two floats are in units in the last place (`u64::MAX` across a sign or for a non-finite value).
fn ulps(a: f32, b: f32) -> u64 {
    if a == b {
        return 0;
    }
    if !a.is_finite() || !b.is_finite() || a.is_sign_negative() != b.is_sign_negative() {
        return u64::MAX;
    }
    u64::from(a.to_bits().abs_diff(b.to_bits()))
}

/// The largest per-lane difference of two byte images.
fn byte_max(a: &[u8], b: &[u8]) -> Result<[u8; 4]> {
    if a.len() != b.len() {
        return Err("images of different sizes".into());
    }
    let mut max = [0u8; 4];
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        max[i % 4] = max[i % 4].max(x.abs_diff(*y));
    }
    Ok(max)
}

#[derive(Default)]
struct Census {
    states: usize,
    vertex_runs: usize,
    alpha_images: usize,
    pixels: usize,
    worst_bytes: [u8; 4],
    worst_normal: [u8; 4],
    worst_vertex_ulps: u64,
    not_streamed: usize,
    worst_float: [f64; 5],
}

#[test]
fn the_bake_runs_as_warp_runs_its_state() -> Result<()> {
    let Some(root) = oracle(OracleSet::Baseline, Game::Sekiro, "layer-pilot") else {
        return Ok(());
    };
    let cases: Value = serde_json::from_slice(&std::fs::read(root.join("cases.json"))?)?;
    let mut c = Census::default();
    let mut failures = Vec::new();
    for case in cases.as_array().ok_or("cases")? {
        let kind = format!(
            "{} vertex program {} alpha test {}",
            case["family"], case["vertex_program"], case["alpha_test"]
        );
        for s in case["states"].as_array().ok_or("states")? {
            let name = s["folder"].as_str().ok_or("folder")?;
            let folder = root.join(name);
            let at = format!("{name} ({kind})");
            let state = PipelineState::decode(&encoded(&folder)?)?;
            let warp = folder.join("warp");
            let status: Value = serde_json::from_slice(&std::fs::read(warp.join("warp.json"))?)?;
            let refused: Vec<String> = ["vertex", "pixel", "alpha"]
                .iter()
                .filter(|stage| status[**stage]["status"] == "refused")
                .map(|stage| format!("{stage}: {}", status[*stage]["reason"]))
                .collect();
            if !refused.is_empty() {
                failures.push(format!("{at}: WARP refused {refused:?}"));
                continue;
            }
            c.states += 1;
            let n = state.size as usize * state.size as usize;
            c.pixels += n;
            // the pixel program: ours and WARP's targets through the bake's rules
            let ours = state.evaluate(Threads::ONE)?;
            let mut targets = Vec::with_capacity(TARGETS.len());
            for (t, &(register, _)) in TARGETS.iter().enumerate() {
                let theirs = words(&std::fs::read(warp.join(format!("target{register}.f32")))?)?;
                if theirs.len() != n * 4 {
                    return Err(format!("{at}: target{register}.f32 is not the grid").into());
                }
                // the lanes our program writes, with WARP's values in them
                let pixels: Vec<([u32; 4], u8)> = ours.pixels[t]
                    .iter()
                    .enumerate()
                    .map(|(i, (_, mask))| {
                        (
                            [
                                theirs[i * 4],
                                theirs[i * 4 + 1],
                                theirs[i * 4 + 2],
                                theirs[i * 4 + 3],
                            ],
                            *mask,
                        )
                    })
                    .collect();
                for (i, (bits, mask)) in ours.pixels[t].iter().enumerate() {
                    for lane in (0..4).filter(|l| mask >> l & 1 != 0) {
                        let d = (f64::from(f32::from_bits(bits[lane]))
                            - f64::from(f32::from_bits(pixels[i].0[lane])))
                        .abs();
                        c.worst_float[t] = c.worst_float[t].max(d);
                    }
                }
                targets.push(pixels);
            }
            let theirs = Raw {
                pixels: targets,
                instructions: 0,
                samples: 0,
            };
            let (a, b) = (state.bake(&ours)?, state.bake(&theirs)?);
            let albedo = byte_max(&a.albedo, &b.albedo)?;
            let normal = byte_max(&a.normal, &b.normal)?;
            for k in 0..4 {
                c.worst_bytes[k] = c.worst_bytes[k].max(albedo[k]);
                c.worst_normal[k] = c.worst_normal[k].max(normal[k]);
            }
            if albedo.iter().chain(&normal).any(|&d| d > 1) {
                failures.push(format!("{at}: albedo {albedo:?}, normal {normal:?} levels apart"));
            }
            let near = |x: f32, y: f32| (x - y).abs() <= (1. / 255f32).max(x.abs().max(y.abs()) * 1e-3);
            match (&a.gloss, &b.gloss) {
                (Some(x), Some(y)) if x.iter().zip(y).all(|(x, y)| near(*x, *y)) => {}
                (None, None) => {}
                _ => failures.push(format!("{at}: the second output's x differs")),
            }
            match (&a.emissive, &b.emissive) {
                (Some(x), Some(y))
                    if x.iter()
                        .flatten()
                        .zip(y.iter().flatten())
                        .all(|(x, y)| near(*x, *y)) => {}
                (None, None) => {}
                _ => failures.push(format!("{at}: the emission differs")),
            }
            // the albedo alpha, sampled beside the program
            if state.alpha.is_some() {
                c.alpha_images += 1;
                let theirs = words(&std::fs::read(warp.join("alpha.f32"))?)?;
                if theirs.len() != n {
                    return Err(format!("{at}: alpha.f32 is not the grid").into());
                }
                let worst = a
                    .albedo
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(&theirs)
                    .map(|(ours, &w)| {
                        ours[3].abs_diff((f32::from_bits(w).clamp(0., 1.) * 255.).round_ties_even() as u8)
                    })
                    .max()
                    .unwrap_or(0);
                if worst > 1 {
                    failures.push(format!("{at}: albedo alpha {worst} levels apart"));
                }
            }
            // the vertex program at the corners
            if let Some(vertex) = &state.vertex {
                c.vertex_runs += 1;
                let theirs: Value =
                    serde_json::from_slice(&std::fs::read(warp.join("vertex-outputs.json"))?)?;
                let not_streamed: Vec<u64> = status["vertex"]["not_streamed"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|n| n["register"].as_u64()).collect())
                    .unwrap_or_default();
                c.not_streamed += not_streamed.len();
                for (corner, registers) in vertex.outputs.iter().enumerate() {
                    for (r, lanes) in registers.iter().enumerate() {
                        if theirs[corner][r].is_null() && not_streamed.contains(&(r as u64)) {
                            continue;
                        }
                        for (lane, &bits) in lanes.iter().enumerate() {
                            let Some(w) = theirs[corner][r][lane].as_str() else {
                                failures.push(format!("{at}: WARP wrote no o{r}.{lane} at corner {corner}"));
                                continue;
                            };
                            let d = ulps(f32::from_bits(bits), f32::from_bits(u32::from_str_radix(w, 16)?));
                            c.worst_vertex_ulps = c.worst_vertex_ulps.max(d);
                            if d > 4 {
                                failures
                                    .push(format!("{at}: o{r}.{lane} at corner {corner} is {d} ulps apart"));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(c.states > 0, "no state checked");
    println!(
        "layer pilot against WARP: {} states, {} pixels; vertex runs {}, alpha images {}; worst albedo levels {:?}, \
         normal levels {:?}, vertex ulps {} (registers stream output cannot carry {}), float difference by \
         target {:?}",
        c.states,
        c.pixels,
        c.vertex_runs,
        c.alpha_images,
        c.worst_bytes,
        c.worst_normal,
        c.worst_vertex_ulps,
        c.not_streamed,
        c.worst_float
    );
    for f in &failures {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
    Ok(())
}
