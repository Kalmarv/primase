//! Area images use the independent bake with D3D-decoded inputs. A one-level
//! arithmetic bound is unchanged from the pilot; UV periods are counted apart.
mod common;
#[path = "common/layers.rs"]
mod layer_oracle;
use primase_fromsoft::{dcx::Decoder, layer_bake::Inputs, sekiro};
use primase_testdata::real_data;
use serde_json::Value;
use std::{path::Path, time::Instant};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
struct Measurement {
    image: String,
    period: [f32; 2],
    seconds: f64,
    max: [u8; 4],
    p99: [u8; 4],
}
fn compare(path: &Path, pixels: &[u8], period: [f32; 2], seconds: f64) -> Result<Measurement> {
    let reference = std::fs::read(path)?;
    if pixels.len() != reference.len() {
        return Err(format!("{}: image length", path.display()).into());
    }
    let mut hist = [[0u64; 256]; 4];
    for (i, (&a, &b)) in pixels.iter().zip(&reference).enumerate() {
        hist[i % 4][a.abs_diff(b) as usize] += 1;
    }
    let max = std::array::from_fn(|c| hist[c].iter().rposition(|&n| n > 0).unwrap_or(0) as u8);
    let target = ((pixels.len() / 4) as u64 * 99).div_ceil(100);
    let p99 = std::array::from_fn(|c| {
        let mut sum = 0;
        hist[c]
            .iter()
            .position(|&n| {
                sum += n;
                sum >= target
            })
            .unwrap_or(0) as u8
    });
    Ok(Measurement {
        image: path.display().to_string(),
        period,
        seconds,
        max,
        p99,
    })
}

fn run(scope: &str) -> Result<()> {
    let Some(install) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(root) = real_data("PRIMASE_FROMSOFT_LAYER_AREA") else {
        return Ok(());
    };
    let control = primase_testdata::real_data_value("PRIMASE_FROMSOFT_LAYER_AREA_CONTROL")
        .map(std::path::PathBuf::from);
    if let Some(path) = &control {
        let provenance: Value = serde_json::from_slice(&std::fs::read(path.join("control.json"))?)?;
        println!("Experimental reference control (not a replacement oracle): {provenance}");
    }
    let image_root = control.map_or_else(
        || root.join(format!("{scope}-d3d-control")),
        |path| path.join(scope),
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let counts: Value = serde_json::from_slice(&std::fs::read(root.join("counts.json"))?)?;
    let region = serde_json::from_value(counts[scope]["region"].clone())?;
    let records = counts[scope]["records"].as_array().ok_or("records")?;
    let references: Vec<Value> =
        serde_json::from_slice(&std::fs::read(root.join(format!("{scope}-materials.json")))?)?;
    let mut world = primase_core::World::default();
    sekiro::read_area(
        install,
        "m11_01_00_00",
        &decoder,
        &sekiro::Options {
            region,
            ..Default::default()
        },
        &mut world,
    )?;
    common::run(&mut world, common::LEGACY_GEOMETRY).map_err(|e| e.to_string())?;
    let area = sekiro::layers::AreaBake::from_world(&world)?;
    let start = Instant::now();
    let measurements: Result<Vec<_>> = std::thread::scope(|threads| {
        let jobs: Vec<_> = (0..4)
            .map(|worker| {
                let area = &area;
                let references = &references;
                let image_root = &image_root;
                threads.spawn(move || -> Result<Vec<Measurement>> {
                    let mut measured = Vec::new();
                    for index in (worker..area.groups.len()).step_by(4) {
                        let group = &area.groups[index];
                        let candidates: Vec<_> = references
                            .iter()
                            .filter(|r| layer_oracle::matches(r, &group.material))
                            .collect();
                        if candidates.len() != 1 {
                            return Err(format!("{}: ambiguous material join", group.material.mtd).into());
                        }
                        let key = candidates[0]["key"].as_str().expect("valid area oracle schema");
                        let prepared = area.prepare(index)?;
                        for center in &group.centers {
                            let matched: Vec<_> = records
                                .iter()
                                .enumerate()
                                .filter(|(_, r)| {
                                    r["material"]
                                        .as_str()
                                        .expect("valid area oracle schema")
                                        .eq_ignore_ascii_case(key)
                                        && serde_json::from_value::<[f32; 8]>(r["weights"].clone())
                                            .expect("valid area oracle schema")
                                            == *center
                                })
                                .collect();
                            if matched.len() != 1 {
                                return Err(format!("{key}: unmatched group {center:?}").into());
                            }
                            let (id, record) = matched[0];
                            let size = prepared.bake_size(512)?;
                            if size as u64 != record["size"][0].as_u64().expect("valid area oracle schema")
                                || prepared.period
                                    != serde_json::from_value::<[f32; 2]>(record["period"].clone())?
                            {
                                return Err(format!("{key}: size or period mismatch").into());
                            }
                            let timer = Instant::now();
                            let image = prepared.bake(
                                size,
                                prepared.period,
                                Inputs {
                                    weights_snow: [center[0], center[1], center[2], center[3]],
                                    tint_color: [center[4], center[5], center[6], 1.],
                                    detail: center[7],
                                },
                            )?;
                            let seconds = timer.elapsed().as_secs_f64();
                            for (kind, pixels) in [("albedo", &image.albedo), ("normal", &image.normal)] {
                                measured.push(compare(
                                    &image_root.join(format!("{id}-{kind}.rgba")),
                                    pixels,
                                    prepared.period,
                                    seconds,
                                )?);
                            }
                        }
                    }
                    Ok(measured)
                })
            })
            .collect();
        let mut out = Vec::new();
        for job in jobs {
            out.extend(job.join().map_err(|_| "bake worker panic")??);
        }
        Ok(out)
    });
    let measurements = measurements?;
    assert_eq!(measurements.len(), records.len() * 2);
    for nonunit in [false, true] {
        let bucket: Vec<_> = measurements
            .iter()
            .filter(|m| (m.period != [1.; 2]) == nonunit)
            .collect();
        let max: [u8; 4] = std::array::from_fn(|c| bucket.iter().map(|m| m.max[c]).max().unwrap_or(0));
        let p99: [u8; 4] = std::array::from_fn(|c| bucket.iter().map(|m| m.p99[c]).max().unwrap_or(0));
        let seconds: f64 = bucket.iter().map(|m| m.seconds * 0.5).sum();
        println!(
            "{scope}: nonunit {nonunit}, {} images, max {max:?}, worst-image p99 {p99:?}, worker bake seconds {seconds:.3}",
            bucket.len()
        );
    }
    println!(
        "{scope}: {} variants, four-worker wall {:.3}s",
        records.len(),
        start.elapsed().as_secs_f64()
    );
    // Keep all arithmetic failures from all workers. Returning on the first
    // divergent image hides whether a binding defect affects other variants.
    let failures: Vec<_> = measurements
        .iter()
        .filter(|m| m.max.iter().any(|&value| value > 1))
        .map(|m| format!("{}: max {:?}, p99 {:?}", m.image, m.max, m.p99))
        .collect();
    if !failures.is_empty() {
        return Err(format!(
            "{} images exceed one level:\n{}",
            failures.len(),
            failures.join("\n")
        )
        .into());
    }
    Ok(())
}
#[test]
fn slice_pixels() -> Result<()> {
    run("slice")
}
#[test]
fn castle_pixels() -> Result<()> {
    run("castle")
}
