//! The KTX2 web textures checked from the files a web export wrote, with an independent decoder: every KTX2 image's
//! every mip level decoded by the pure-Rust Basis Universal transcoder (their hashes printed for the viewer's WASM
//! transcoder to compare), each image against its plain (PNG) set's, which is the encoder's input (the plain set is
//! written from the same prepared world; an image of another size there is an error): PNG images exactly those
//! pixels, cut-out visibility changed for fewer than 0.1% of pixels at each material's cutoff, blended alpha p99
//! within 8 levels, and per-role codec quality. Runs on the synthetic fixture always, and on a real export
//! (`oracles.baseline.sekiro/ktx2-tiles`) when configured, against the bounds the README states.
#![cfg(all(feature = "ktx2", feature = "fixture"))]
#![allow(clippy::unwrap_used)]
use basisu::{DecodeFlags, TargetFormat, Transcoder};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod quality;
use quality::Quality;

type R<T> = Result<T, Box<dyn std::error::Error>>;

/// An image's bytes: its file beside the tiles (format 2.0, `uri`), or its view of the GLB's binary chunk (1.x).
fn image_bytes(
    folder: &Path,
    doc: &Value,
    bin: Option<&[u8]>,
    image: &Value,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if let Some(uri) = image["uri"].as_str() {
        return Ok(std::fs::read(folder.join(uri))?);
    }
    let view = &doc["bufferViews"][image["bufferView"].as_u64().ok_or("image view")? as usize];
    let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let length = view["byteLength"].as_u64().ok_or("image length")? as usize;
    Ok(bin
        .ok_or("BIN")?
        .get(start..start + length)
        .ok_or("image bounds")?
        .to_vec())
}

/// What an audit found.
#[derive(Default)]
struct Audit {
    /// KTX2 images and their mip levels, all decoded.
    images: u64,
    levels: u64,
    /// Distinct images compared with the plain set, and of them PNG.
    compared: u64,
    png: u64,
    near_zero: u64,
    /// Codec error against the encoder's input, by role.
    quality: BTreeMap<String, Quality>,
}

/// Audit the web export in `folder` (`tiles.json`, its tiles and images) against its plain set in `folder/plain`.
fn audit(folder: &Path) -> R<Audit> {
    let mut out = Audit::default();
    let index: Value = serde_json::from_slice(&std::fs::read(folder.join("tiles.json"))?)?;
    let plain_folder = folder.join("plain");
    let plain_index: Value = serde_json::from_slice(&std::fs::read(plain_folder.join("tiles.json"))?)?;
    // The plain set's tiling is its own (PNG tiles are larger, so a cell may split where the KTX2 one does not): an
    // image's reference is found by its name, the texture it is, in whichever plain tile holds it. A name holds one
    // content in every plain tile.
    let mut plain_tiles = Vec::new();
    for tile in plain_index["tiles"].as_array().ok_or("plain tiles")? {
        plain_tiles.push(std::fs::read(
            plain_folder.join(tile["file"].as_str().ok_or("plain filename")?),
        )?);
    }
    let mut plain_docs = Vec::new();
    for bytes in &plain_tiles {
        let chunks = primase_glb::parse(bytes)?;
        plain_docs.push((serde_json::from_slice::<Value>(chunks.json)?, chunks.bin));
    }
    let mut by_name = BTreeMap::<String, Vec<u8>>::new();
    for (doc, bin) in &plain_docs {
        for image in doc["images"].as_array().map_or(&[][..], Vec::as_slice) {
            let name = image["name"].as_str().ok_or("plain image name")?;
            let png = image_bytes(&plain_folder, doc, *bin, image)?;
            match by_name.get(name) {
                Some(known) if *known != png => {
                    return Err(format!("plain image {name} has two contents").into());
                }
                Some(_) => {}
                None => {
                    by_name.insert(name.to_owned(), png);
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    for tile in index["tiles"].as_array().ok_or("tiles")? {
        let name = tile["file"].as_str().ok_or("tile file")?;
        let glb = std::fs::read(folder.join(name))?;
        let chunks = primase_glb::parse(&glb)?;
        let doc: Value = serde_json::from_slice(chunks.json)?;
        let Some(image_list) = doc["images"].as_array() else {
            continue;
        };
        for (i, image) in image_list.iter().enumerate() {
            let bytes = &image_bytes(folder, &doc, chunks.bin, image)?[..];
            let is_png = image["mimeType"] == "image/png";
            let (width, height, decoded) = if is_png {
                let decoded = primase_image::read_png(bytes)?.image;
                let primase_image::Samples::U8(pixels) = decoded.samples else {
                    return Err("RGBA8 PNG required".into());
                };
                (decoded.width, decoded.height, pixels)
            } else {
                let decoder = Transcoder::new(bytes).map_err(|e| format!("KTX2 decoder: {e:?}"))?;
                let mut hashes = Vec::new();
                for mip in 0..decoder.level_count() {
                    let pixels = decoder
                        .transcode(mip, TargetFormat::Rgba32, DecodeFlags::NONE)
                        .map_err(|e| format!("KTX2 mip: {e:?}"))?;
                    hashes.push(
                        Sha256::digest(&pixels)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>(),
                    );
                    out.levels += 1;
                }
                out.images += 1;
                let (width, height) = (
                    u32::from_le_bytes(bytes[20..24].try_into()?),
                    u32::from_le_bytes(bytes[24..28].try_into()?),
                );
                // a complete mip chain down to 1 x 1
                assert_eq!(
                    hashes.len() as u32,
                    32 - width.max(height).leading_zeros(),
                    "{name} image {i}: mip levels"
                );
                println!(
                    "{}",
                    json!({"file":name,"image":i,"width":width,"height":height,"hashes":hashes})
                );
                (
                    width,
                    height,
                    decoder
                        .transcode(0, TargetFormat::Rgba32, DecodeFlags::NONE)
                        .map_err(|e| format!("{e:?}"))?,
                )
            };
            let role = image["extras"]["primase.web_texture"]["role"]
                .as_str()
                .ok_or("texture role")?;
            if role == "data" {
                continue;
            }
            // a role copy (`name~mask`) is its source's image in the plain set
            let source_name = image["name"]
                .as_str()
                .map(|n| n.split('~').next().unwrap_or(n))
                .ok_or("image name")?;
            let png = &by_name.get(source_name).ok_or("reference image name")?[..];
            let key = (
                Sha256::digest(png).to_vec(),
                role.to_owned(),
                width,
                height,
                Sha256::digest(bytes).to_vec(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.compared += 1;
            out.png += u64::from(is_png);
            out.near_zero += image["extras"]["primase.web_texture"]["near_zero_cap_average_to_flat"]
                .as_u64()
                .unwrap_or(0);
            // the encoder's input: the plain set's image, written from the same prepared world, is the prepared
            // pixels; at another size it is not, and nothing independent gives them
            let plain = primase_image::read_png(png)?.image;
            let primase_image::Samples::U8(reference) = plain.samples else {
                return Err(format!("{source_name}: the plain set's image is not RGBA8").into());
            };
            if (plain.width, plain.height) != (width, height) {
                return Err(format!(
                    "{name} image {i} ({source_name}): {width} x {height}, its plain twin {} x {}: the plain set must \
                     be written from the same prepared world, so it holds the encoder's input",
                    plain.width, plain.height
                )
                .into());
            }
            // the consumer's alpha rules, from the materials that bind the image as base colour, in every plain tile
            let bindings = plain_docs.iter().flat_map(|(plain_doc, _)| {
                plain_doc["materials"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(move |m| (plain_doc, m))
            });
            for (plain_doc, material) in bindings {
                let Some(t) = material
                    .pointer("/pbrMetallicRoughness/baseColorTexture/index")
                    .and_then(Value::as_u64)
                else {
                    continue;
                };
                let source = plain_doc["textures"][t as usize]["source"]
                    .as_u64()
                    .ok_or("PNG texture source")?;
                if plain_doc["images"][source as usize]["name"].as_str() != Some(source_name)
                    || role != "colour"
                {
                    continue;
                }
                let factor = material
                    .pointer("/pbrMetallicRoughness/baseColorFactor/3")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.) as f32;
                if material["alphaMode"] == "MASK" {
                    let cutoff = material["alphaCutoff"].as_f64().unwrap_or(0.5) as f32;
                    let flips = reference
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(decoded.as_chunks::<4>().0)
                        .filter(|(a, b)| {
                            (a[3] as f32 / 255. * factor < cutoff) != (b[3] as f32 / 255. * factor < cutoff)
                        })
                        .count();
                    assert!(
                        (flips as u64) * 1000 < (reference.len() / 4) as u64,
                        "cutout flips exceed bound: {}",
                        image["name"]
                    );
                } else if material["alphaMode"] == "BLEND" {
                    let mut alpha = Quality::default();
                    alpha.add(&reference, &decoded, false);
                    assert!(
                        alpha.report()["channel_p99_rgba"][3]
                            .as_u64()
                            .ok_or("alpha p99")?
                            <= 8,
                        "blend alpha exceeds bound: {}",
                        image["name"]
                    );
                }
            }
            if is_png {
                assert_eq!(
                    reference, decoded,
                    "{}: PNG must exactly preserve the prepared pixels",
                    image["name"]
                );
            }
            out.quality
                .entry(role.into())
                .or_default()
                .add(&reference, &decoded, role == "normal");
        }
    }
    eprintln!(
        "{}",
        json!({"images":out.images,"levels":out.levels,"compared":out.compared,"png":out.png,"near_zero_cap_average_to_flat":out.near_zero})
    );
    for (role, stats) in &out.quality {
        eprintln!("{}", json!({"role":role,"quality":stats.report()}));
    }
    Ok(out)
}

/// The largest colour codec error at the 99th percentile, over the RGB channels.
fn colour_p99(a: &Audit) -> u64 {
    a.quality.get("colour").map_or(0, |q| {
        (0..3)
            .map(|c| q.report()["channel_p99_rgba"][c].as_u64().unwrap())
            .max()
            .unwrap()
    })
}

#[test]
fn the_synthetic_export_decodes_and_keeps_its_bounds() -> R<()> {
    use primase_core::pass::{Pass, PassManager, PassRegistry, Pipeline, RunReport};
    let dir = std::env::temp_dir().join(format!("primase-web-ktx2-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut world = primase_web::fixture::synthetic_world();
    let passes: Vec<Box<dyn Pass>> = vec![Box::new(primase_meshops::BakeNonrigid {
        mirrors: primase_meshops::Mirrors::Bake,
    })];
    PassManager::new(PassRegistry::new()).run(&mut world, &Pipeline { passes }, &mut RunReport::default())?;
    let mut options = primase_web::Options::new(primase_web::TileSize::new(200.)?);
    options.ktx2 = true;
    let set = primase_web::write(&world, &options)?;
    set.write_to(&dir)?;
    // the one KTX2 image is reported as a lossy encoding, once
    let lossy: Vec<u64> = set
        .notices
        .iter()
        .filter(|n| n.reason.starts_with("images encoded as KTX2"))
        .map(|n| n.count)
        .collect();
    assert_eq!(lossy, [1]);
    options.ktx2 = false;
    primase_web::write(&world, &options)?.write_to(&dir.join("plain"))?;
    let a = audit(&dir)?;
    // the 8 x 8 gradient: one KTX2 image, mips 8, 4, 2, 1
    assert_eq!((a.images, a.levels, a.compared, a.png), (1, 4, 1, 0));
    // the synthetic gradient's bound (README: 8 channel values)
    assert!(colour_p99(&a) <= 8, "{}", colour_p99(&a));
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn a_real_export_decodes_and_keeps_the_measured_bounds() -> R<()> {
    let Some(dir) = primase_testdata::oracle(
        primase_testdata::OracleSet::Baseline,
        primase_testdata::Game::Sekiro,
        "ktx2-tiles",
    ) else {
        return Ok(());
    };
    let a = audit(&dir)?;
    assert!(a.images > 0, "no KTX2 image in the export");
    // README: colour codec error p99 at most 5 levels per channel on slice A, as measured ([5, 4, 5] over the whole
    // slice and over its one cell)
    assert!(colour_p99(&a) <= 5, "colour codec p99 {}", colour_p99(&a));
    Ok(())
}

#[test]
fn counted_channel_and_angle_statistics() {
    let mut q = Quality::default();
    q.add(
        &[10, 20, 30, 40, 10, 20, 30, 40],
        &[11, 22, 33, 44, 13, 24, 35, 46],
        false,
    );
    assert_eq!(q.channel_max, [3, 4, 5, 6]);
    assert_eq!(q.channel_sum, [4, 6, 8, 10]);
    assert_eq!(q.report()["channel_p99_rgba"], json!([3, 4, 5, 6]));
    let mut q = Quality::default();
    q.add(&[128, 128, 255, 255], &[128, 128, 255, 255], true);
    assert!(q.angle_max < 0.0001);
    assert!(q.report()["normal_angle_p99_upper_deg"].as_f64().expect("p99") <= 0.001);
    q.add(&[128, 128, 255, 255], &[128, 128, 0, 255], true);
    assert_eq!(q.report()["normal_over_10_deg"], 1);
    assert_eq!(q.report()["normal_over_10_share"], 0.5);
}
