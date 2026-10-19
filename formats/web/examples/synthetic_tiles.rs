//! Generate a tiny public fixture from literal geometry and pixels only.
mod support;
use basisu::{DecodeFlags, TargetFormat, Transcoder};
use primase_web::Options;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(std::env::args().nth(1).ok_or("usage: synthetic_tiles <output>")?);
    let mut world = primase_web::fixture::synthetic_world();
    // the web writer takes rigid placements: the mirrored one's mirror goes into a copy of its geometry
    let mut report = primase_core::pass::RunReport::default();
    primase_core::pass::PassManager::new(primase_core::pass::PassRegistry::new()).run(
        &mut world,
        &primase_core::pass::Pipeline {
            passes: vec![Box::new(primase_meshops::BakeNonrigid {
                mirrors: primase_meshops::Mirrors::Bake,
            })],
        },
        &mut report,
    )?;
    let mut options = Options {
        ktx2: true,
        viewer_items: primase_web::fixture::synthetic_viewer_items(),
        ..Options::new(primase_web::TileSize::new(200.)?)
    };
    let set = primase_web::write(&world, &options)?;
    set.write_to(&output)?;
    let mut proof = Vec::new();
    for tile in &set.index.tiles {
        proof.push(support::tile_proof(&tile.file, &set.files[&tile.file])?);
    }
    support::write_entries(&proof, &output)?;
    let mut reference = Vec::new();
    for tile in &set.index.tiles {
        let chunks = primase_glb::parse(&set.files[&tile.file])?;
        let doc: Value = serde_json::from_slice(chunks.json)?;
        if let Some(images) = doc["images"].as_array() {
            for (i, image) in images.iter().enumerate() {
                if image["mimeType"] != "image/ktx2" {
                    continue;
                }
                let uri = image["uri"].as_str().ok_or("image uri")?;
                let decoder = Transcoder::new(set.images.get(uri).ok_or("image file")?)
                    .map_err(|e| format!("{e:?}"))?;
                let mut hashes = Vec::new();
                for mip in 0..decoder.level_count() {
                    let pixels = decoder
                        .transcode(mip, TargetFormat::Rgba32, DecodeFlags::NONE)
                        .map_err(|e| format!("{e:?}"))?;
                    hashes.push(
                        Sha256::digest(&pixels)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>(),
                    );
                }
                reference.push(json!({"file":tile.file,"image":i,"hashes":hashes}));
            }
        }
    }
    std::fs::write(
        output.join("ktx2-reference.json"),
        serde_json::to_vec_pretty(&reference)?,
    )?;
    options.ktx2 = false;
    options.compress = false;
    options.gpu_instancing = false;
    primase_web::write(&world, &options)?.write_to(&output.join("plain"))?;
    Ok(())
}
