//! Export a scene folder as web tiles and a plain comparison set.
use primase_core::pass::{Pass, PassLog};
use primase_core::{Name, World};
use primase_skate_scene::{SkateCollision, SkateGrindCurve, SkateMarker, SkatePlacement, SkateSoup};
use primase_web::{Options, ViewerItem};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
fn decoded<const N: usize>(bytes: &[u8], count: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>>
where
    [u8; N]: Default,
{
    Ok(meshopt::decode_vertex_buffer::<[u8; N]>(bytes, count)?
        .into_iter()
        .flatten()
        .collect())
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 && !(args.len() == 4 && args[3] == "--ktx2") {
        return Err("usage: scene_to_tiles <scene folder> <output folder> [--ktx2]".into());
    }
    let folder = Path::new(&args[1]);
    let output = Path::new(&args[2]);
    let mut world = World::new();
    primase_skate_scene::read(folder, &mut world)?;
    primase_gltf::load_mesh_files(&mut world, folder)?;
    primase_image::load_textures(&mut world, folder)?;
    primase_skate_scene::LowerSkatePbr.run(&mut world, &mut PassLog::default())?;
    primase_image::normalize_normals(&mut world)?;
    let mut options = Options {
        base_dir: folder.into(),
        ..Default::default()
    };
    if args.len() == 4 {
        #[cfg(feature = "ktx2")]
        {
            options.ktx2 = true;
        }
        #[cfg(not(feature = "ktx2"))]
        return Err("--ktx2 requires the ktx2 cargo feature".into());
    }
    options.split_by_tile = world
        .iter::<SkatePlacement>()
        .filter_map(|(e, p)| p.split_by_tile.then_some(e))
        .collect();
    let name = |e| {
        world
            .get::<Name>(e)
            .map_or_else(|| format!("Entity_{}", e.index()), |n| n.0.clone())
    };
    for (e, c) in world.iter::<SkateGrindCurve>() {
        if let Some(p) = c.points.first() {
            options.viewer_items.push(ViewerItem {
                name: name(e),
                kind: "grind_curve".into(),
                anchor: p.map(f64::from),
                data: serde_json::to_value(c)?,
            });
        }
    }
    for (e, m) in world.iter::<SkateMarker>() {
        let anchor = m
            .xyz()
            .map(|(x, y, z)| [x as f64, y.unwrap_or(0.) as f64, z as f64])
            .or_else(|| m.m.map(|v| [v[9] as f64, v[10] as f64, v[11] as f64]))
            .unwrap_or([0.; 3]);
        options.viewer_items.push(ViewerItem {
            name: name(e),
            kind: if m.kind == primase_skate_scene::MarkerKind::Spawn {
                "spawn"
            } else {
                "marker"
            }
            .into(),
            anchor,
            data: serde_json::to_value(m)?,
        });
    }
    for (e, _) in world.iter::<SkateCollision>() {
        if let Some(s) = world.get::<SkateSoup>(e) {
            if s.triangles.is_empty() {
                continue;
            }
            options.viewer_items.push(ViewerItem {
                name: name(e),
                kind: "collision".into(),
                anchor: s.positions.first().copied().unwrap_or([0.; 3]).map(f64::from),
                data: json!({"positions":s.positions,"triangles":s.triangles,"surfaces":s.surfaces}),
            });
        }
    }
    let set = primase_web::write(&world, &options)?;
    set.write_to(output)?;
    let mut proof = Vec::new();
    for tile in &set.index.tiles {
        let bytes = &set.files[&tile.file];
        let chunks = primase_gltf::glb::parse(bytes)?;
        let doc: Value = serde_json::from_slice(chunks.json)?;
        let mut streams = Vec::new();
        for (i, v) in doc["bufferViews"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .enumerate()
        {
            let ext = &v["extensions"]["EXT_meshopt_compression"];
            if !ext.is_object() {
                continue;
            }
            let offset = ext["byteOffset"].as_u64().ok_or("offset")? as usize;
            let length = ext["byteLength"].as_u64().ok_or("length")? as usize;
            let count = ext["count"].as_u64().ok_or("count")? as usize;
            let bytes = &chunks.bin.ok_or("bin")?[offset..offset + length];
            let raw = match ext["byteStride"].as_u64() {
                Some(4) => decoded::<4>(bytes, count)?,
                Some(8) => decoded::<8>(bytes, count)?,
                Some(12) => decoded::<12>(bytes, count)?,
                Some(16) => decoded::<16>(bytes, count)?,
                _ => return Err("stride".into()),
            };
            streams.push(json!({"view":i,"sha256":hash(&raw),"bytes":raw.len()}));
        }
        proof.push(json!({"file":tile.file,"sha256":hash(bytes),"streams":streams,"quantization":doc["asset"]["extras"]["primase.quantization"]}));
    }
    std::fs::write(output.join("proof.json"), serde_json::to_vec_pretty(&proof)?)?;
    options.compress = false;
    options.gpu_instancing = false;
    #[cfg(feature = "ktx2")]
    {
        options.ktx2 = false;
    }
    let plain = primase_web::write(&world, &options)?;
    plain.write_to(&output.join("plain"))?;
    println!(
        "tiles={} triangles={} placements={} instanced={} viewer_items={} bytes={} plain_bytes={}",
        set.index.tiles.len(),
        set.index.tiles.iter().map(|t| t.triangles).sum::<u64>(),
        set.index.tiles.iter().map(|t| t.placements).sum::<u64>(),
        set.index
            .tiles
            .iter()
            .map(|t| t.instanced_placements)
            .sum::<u64>(),
        options.viewer_items.len(),
        set.files.values().map(Vec::len).sum::<usize>(),
        plain.files.values().map(Vec::len).sum::<usize>()
    );
    Ok(())
}
