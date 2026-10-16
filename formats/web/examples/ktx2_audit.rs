//! Hash every image's mip pixels (image files in 2.0, embedded in 1.x) with an independent pure-Rust transcoder.
use basisu::{DecodeFlags, TargetFormat, Transcoder};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[path = "support/quality.rs"]
mod quality_stats;
use quality_stats::Quality;

// Independent reference: integer source bins, linear-light colour and normalized normals.
fn reference(
    image: primase_image::Image,
    w: u32,
    h: u32,
    role: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let primase_image::Samples::U8(data) = image.samples else {
        return Err("16-bit reference".into());
    };
    if image.width == w && image.height == h {
        return Ok(data);
    }
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = (x * image.width / w, y * image.height / h);
            let (x1, y1) = (
                ((x + 1) * image.width / w).max(x0 + 1),
                ((y + 1) * image.height / h).max(y0 + 1),
            );
            let mut c = [0.; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    if role == "normal" {
                        let pos = ((sy * image.width + sx) * 4) as usize;
                        let v = [data[pos], data[pos + 1], data[pos + 2]].map(|v| v as f64 / 127.5 - 1.);
                        let length = v.iter().map(|v| v * v).sum::<f64>().sqrt();
                        for k in 0..3 {
                            c[k] += v[k] / length;
                        }
                        c[3] += data[pos + 3] as f64 / 255.;
                        continue;
                    }
                    for k in 0..4 {
                        let v = data[((sy * image.width + sx) * 4) as usize + k] as f64 / 255.;
                        c[k] += if role == "colour" && k < 3 {
                            if v <= 0.04045 {
                                v / 12.92
                            } else {
                                ((v + 0.055) / 1.055).powf(2.4)
                            }
                        } else {
                            v
                        };
                    }
                }
            }
            for v in &mut c {
                *v /= f64::from((x1 - x0) * (y1 - y0));
            }
            if role == "normal" {
                let normal = [c[0], c[1], c[2]];
                let length = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
                for k in 0..3 {
                    c[k] = if length >= 3f64.sqrt() / 255. {
                        (normal[k] / length + 1.) / 2.
                    } else {
                        [0.5, 0.5, 1.][k]
                    };
                }
            }
            for (k, mut v) in c.into_iter().enumerate() {
                if role == "colour" && k < 3 {
                    v = if v <= 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1. / 2.4) - 0.055
                    };
                }
                out.push((v * 255.).round().clamp(0., 255.) as u8);
            }
        }
    }
    Ok(out)
}

/// An image's bytes: its file beside the tiles (format 2.0, `uri`), or its view of the GLB's binary chunk (1.x).
fn image_bytes(
    folder: &std::path::Path,
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = std::path::PathBuf::from(std::env::args().nth(1).ok_or("usage: ktx2_audit <tile folder>")?);
    let worst_dir = std::env::args().nth(2).map(std::path::PathBuf::from);
    let colour_dir = std::env::args().nth(3).map(std::path::PathBuf::from);
    let mut worst_angle = 0.;
    let index: Value = serde_json::from_slice(&std::fs::read(folder.join("tiles.json"))?)?;
    let mut images = 0;
    let mut levels = 0;
    let plain_folder = folder.join("plain");
    let plain_index: Option<Value> = std::fs::read(plain_folder.join("tiles.json"))
        .ok()
        .map(|b| serde_json::from_slice(&b))
        .transpose()?;
    let mut quality = BTreeMap::<String, Quality>::new();
    let mut source_quality = BTreeMap::<String, Quality>::new();
    let mut seen = BTreeSet::new();
    let mut near_zero = 0u64;
    let mut png_images = 0;
    for tile in index["tiles"].as_array().ok_or("tiles")? {
        let name = tile["file"].as_str().ok_or("tile file")?;
        let glb = std::fs::read(folder.join(name))?;
        let chunks = primase_gltf::glb::parse(&glb)?;
        let doc: Value = serde_json::from_slice(chunks.json)?;
        let Some(image_list) = doc["images"].as_array() else {
            continue;
        };
        let plain_bytes = if let Some(index) = &plain_index {
            let entry = index["tiles"]
                .as_array()
                .ok_or("plain tiles")?
                .iter()
                .find(|p| p["col"] == tile["col"] && p["row"] == tile["row"])
                .ok_or("plain coordinates")?;
            Some(std::fs::read(
                plain_folder.join(entry["file"].as_str().ok_or("plain filename")?),
            )?)
        } else {
            None
        };
        let plain = plain_bytes.as_deref().map(primase_gltf::glb::parse).transpose()?;
        let plain_doc: Option<Value> = plain.map(|p| serde_json::from_slice(p.json)).transpose()?;
        for (i, image) in image_list.iter().enumerate() {
            let bytes = &image_bytes(&folder, &doc, chunks.bin, image)?[..];
            let is_png = image["mimeType"] == "image/png";
            let mut hashes = Vec::new();
            let (width, height, decoded) = if is_png {
                let decoded = primase_image::read_png(bytes)?.image;
                let primase_image::Samples::U8(pixels) = decoded.samples else {
                    return Err("RGBA8 PNG required".into());
                };
                (decoded.width, decoded.height, pixels)
            } else {
                let decoder = Transcoder::new(bytes).map_err(|e| format!("KTX2 decoder: {e:?}"))?;
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
                    levels += 1;
                }
                (
                    u32::from_le_bytes(bytes[20..24].try_into()?),
                    u32::from_le_bytes(bytes[24..28].try_into()?),
                    decoder
                        .transcode(0, TargetFormat::Rgba32, DecodeFlags::NONE)
                        .map_err(|e| format!("{e:?}"))?,
                )
            };
            if let (Some(plain), Some(plain_doc)) = (plain, &plain_doc) {
                let role = image["extras"]["primase.web_texture"]["role"]
                    .as_str()
                    .ok_or("texture role")?;
                let png_image = plain_doc["images"]
                    .as_array()
                    .ok_or("plain images")?
                    .iter()
                    .find(|p| p["name"] == image["name"])
                    .ok_or("reference image name")?;
                let png = &image_bytes(&plain_folder, plain_doc, plain.bin, png_image)?[..];
                let key = (
                    Sha256::digest(png).to_vec(),
                    role.to_owned(),
                    width,
                    height,
                    Sha256::digest(bytes).to_vec(),
                );
                if seen.insert(key) {
                    near_zero += image["extras"]["primase.web_texture"]["near_zero_cap_average_to_flat"]
                        .as_u64()
                        .unwrap_or(0);
                    if is_png {
                        png_images += 1;
                    }
                    eprintln!(
                        "{}",
                        json!({"image":image["name"],"metadata":image["extras"]["primase.web_texture"]})
                    );
                    let source_image = primase_image::read_png(png)?.image;
                    let reference = reference(source_image.clone(), width, height, role)?;
                    // Derive consumer alpha rules from actual material bindings, not encoder metadata.
                    for material in plain_doc["materials"].as_array().ok_or("materials")? {
                        let Some(t) = material
                            .pointer("/pbrMetallicRoughness/baseColorTexture/index")
                            .and_then(Value::as_u64)
                        else {
                            continue;
                        };
                        let texture = &plain_doc["textures"][t as usize];
                        let source = texture["source"].as_u64().ok_or("PNG texture source")?;
                        if plain_doc["images"][source as usize]["name"] != image["name"] {
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
                                    (a[3] as f32 / 255. * factor < cutoff)
                                        != (b[3] as f32 / 255. * factor < cutoff)
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
                                "blend alpha exceeds bound"
                            );
                        }
                    }
                    if let Some(dir) = &colour_dir
                        && role == "colour"
                    {
                        std::fs::create_dir_all(dir)?;
                        let hash = Sha256::digest(&reference)
                            .iter()
                            .map(|v| format!("{v:02x}"))
                            .collect::<String>();
                        std::fs::write(
                            dir.join(format!("{width}x{height}-{hash}.png")),
                            primase_image::write_png(&primase_image::Image {
                                width,
                                height,
                                samples: primase_image::Samples::U8(reference.clone()),
                            })?,
                        )?;
                    }
                    if is_png {
                        assert_eq!(reference, decoded, "PNG must exactly preserve capped pixels");
                    }
                    if let Some(dir) = &worst_dir
                        && role == "normal"
                    {
                        let mut stats = Quality::default();
                        stats.add(&reference, &decoded, true);
                        if stats.angle_max > worst_angle {
                            worst_angle = stats.angle_max;
                            std::fs::create_dir_all(dir)?;
                            std::fs::write(
                                dir.join("normal-input.png"),
                                primase_image::write_png(&primase_image::Image {
                                    width,
                                    height,
                                    samples: primase_image::Samples::U8(reference.clone()),
                                })?,
                            )?;
                            std::fs::write(dir.join("normal-source.png"), png)?;
                            std::fs::write(dir.join("normal.ktx2"), bytes)?;
                            std::fs::write(
                                dir.join("normal.json"),
                                serde_json::to_vec_pretty(
                                    &json!({"tile":name,"image":image,"quality":stats.report()}),
                                )?,
                            )?;
                        }
                    }
                    quality
                        .entry(role.into())
                        .or_default()
                        .add(&reference, &decoded, role == "normal");
                    // A separate end-to-end comparison exposes loss from the cap.
                    // Nearest-centre reconstruction is explicit, not a claim about GPU filtering.
                    let mut reconstructed =
                        Vec::with_capacity((source_image.width * source_image.height * 4) as usize);
                    for y in 0..source_image.height {
                        for x in 0..source_image.width {
                            let sx = (((2 * x + 1) as u64 * width as u64) / (2 * source_image.width) as u64)
                                .min(width as u64 - 1) as usize;
                            let sy = (((2 * y + 1) as u64 * height as u64) / (2 * source_image.height) as u64)
                                .min(height as u64 - 1) as usize;
                            let offset = (sy * width as usize + sx) * 4;
                            reconstructed.extend_from_slice(&decoded[offset..offset + 4]);
                        }
                    }
                    let primase_image::Samples::U8(original) = source_image.samples else {
                        return Err("16-bit source".into());
                    };
                    source_quality.entry(role.into()).or_default().add(
                        &original,
                        &reconstructed,
                        role == "normal",
                    );
                }
            }
            if !is_png {
                println!(
                    "{}",
                    json!({"file":name,"image":i,"width":width,"height":height,"hashes":hashes})
                );
                images += 1;
            }
        }
    }
    eprintln!(
        "{}",
        json!({"images":images,"levels":levels,"failed":0,"decoder":"basisu Rust","distinct_png_images":png_images,"near_zero_cap_average_to_flat":near_zero})
    );
    for (role, stats) in quality {
        eprintln!(
            "{}",
            json!({"role":role,"reference":"role-aware capped pixels before compression","quality":stats.report()})
        );
    }
    for (role, stats) in source_quality {
        eprintln!(
            "{}",
            json!({"role":role,"reference":"original pixels vs capped decoded image, nearest-centre reconstruction","quality":stats.report()})
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    #[test]
    fn reference_filters_linear_colour_and_retains_odd_edges() {
        let image = primase_image::Image {
            width: 2,
            height: 1,
            samples: primase_image::Samples::U8(vec![0, 0, 0, 255, 255, 255, 255, 255]),
        };
        assert_eq!(
            reference(image.clone(), 1, 1, "colour").expect("colour"),
            [188, 188, 188, 255]
        );
        assert_eq!(
            reference(image, 1, 1, "mask").expect("mask"),
            [128, 128, 128, 255]
        );
        let image = primase_image::Image {
            width: 3,
            height: 1,
            samples: primase_image::Samples::U8(vec![0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255]),
        };
        assert_eq!(
            reference(image, 1, 1, "mask").expect("odd edge"),
            [85, 85, 85, 255]
        );
    }
}
