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
/// The proof entry of tile `file` in `folder`: its hash and every meshopt stream decoded independently, hashed; a
/// stream in a mesh buffer file (format 3.0) is read from that file, beside the tiles.
pub fn tile_proof(folder: &Path, file: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let bytes = &std::fs::read(folder.join(file))?[..];
    let chunks = primase_glb::parse(bytes)?;
    let doc: Value = serde_json::from_slice(chunks.json)?;
    let mut buffers = Vec::new();
    for b in doc["buffers"].as_array().unwrap_or(&Vec::new()) {
        buffers.push(match b["uri"].as_str() {
            Some(uri) => Some(std::fs::read(folder.join(uri))?),
            None => None,
        });
    }
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
        let buffer = ext["buffer"].as_u64().ok_or("buffer")? as usize;
        let data = match buffers.get(buffer).ok_or("buffer index")? {
            Some(file) => &file[..],
            None => chunks.bin.ok_or("bin")?,
        };
        let bytes = &data[offset..offset + length];
        let raw = match ext["byteStride"].as_u64() {
            Some(4) => decoded::<4>(bytes, count)?,
            Some(8) => decoded::<8>(bytes, count)?,
            Some(12) => decoded::<12>(bytes, count)?,
            Some(16) => decoded::<16>(bytes, count)?,
            _ => return Err("stride".into()),
        };
        streams.push(json!({"view":i,"sha256":hash(&raw),"bytes":raw.len()}));
    }
    Ok(
        json!({"file":file,"sha256":hash(bytes),"streams":streams,"quantization":doc["asset"]["extras"]["primase.quantization"]}),
    )
}
/// `proof.json`: the entries in index order.
pub fn write_entries(proof: &[Value], output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(output.join("proof.json"), serde_json::to_vec_pretty(proof)?)?;
    Ok(())
}
