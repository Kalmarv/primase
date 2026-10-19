//! `tile_proof <tile folder>`: a written tile set's `proof.json` (each tile's hash and its meshopt streams decoded
//! independently, hashed), in the order of the tiles' cells (column, row, subcell), for gates that compare two
//! conversions' tiles by more than their bytes.
mod support;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = PathBuf::from(std::env::args().nth(1).ok_or("usage: tile_proof <tile folder>")?);
    let index: Value = serde_json::from_slice(&std::fs::read(folder.join("tiles.json"))?)?;
    let mut proof = BTreeMap::new();
    for t in index["tiles"].as_array().ok_or("tiles.json has no tiles")? {
        let file = t["file"].as_str().ok_or("a tile without a file")?;
        let cell = t.get("cell").map(|c| {
            (
                c["level"].as_u64().unwrap_or(0),
                c["x"].as_u64().unwrap_or(0),
                c["y"].as_u64().unwrap_or(0),
            )
        });
        let key = (
            t["col"].as_i64().unwrap_or(0),
            t["row"].as_i64().unwrap_or(0),
            cell,
        );
        proof.insert(
            key,
            support::tile_proof(file, &std::fs::read(folder.join(file))?)?,
        );
    }
    support::write_entries(&proof.into_values().collect::<Vec<_>>(), &folder)?;
    println!(
        "proof.json: {} tiles",
        index["tiles"].as_array().map_or(0, Vec::len)
    );
    Ok(())
}
