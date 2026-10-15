//! Independent MTD values, including every decoded string and stored parameter.
use primase_core::gate::real_data;
use primase_fromsoft::{
    Archive, Binder,
    dcx::Decoder,
    mtd::{MaterialDefinition, Value},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
fn native(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: caller explicitly supplies a trusted version-6 native library.
    unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
}
#[test]
fn all_material_definitions() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_MATERIAL_ORACLE") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let dll = native(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let binder = Binder::read(
        &archive.read("/mtd/allmaterialbnd.mtdbnd.dcx", &decoder)?,
        &decoder,
    )?;
    let lines = std::fs::read_to_string(oracle.join("mtd.jsonl"))?;
    let expected = lines
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let files: Vec<_> = binder
        .files
        .iter()
        .filter(|f| {
            f.name
                .as_deref()
                .is_some_and(|n| n.to_lowercase().ends_with(".mtd"))
        })
        .collect();
    assert_eq!(files.len(), expected.len());
    let mut parameters = 0;
    let mut textures = 0;
    let boundaries = std::fs::read_to_string(oracle.join("mtd-boundaries.jsonl"))?;
    let boundaries: Vec<serde_json::Value> = boundaries
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    assert_eq!(boundaries.len(), files.len());
    let mut length_files = 0;
    let mut outer_deltas = BTreeMap::<i64, usize>::new();
    for (index, (file, w)) in files.iter().zip(&expected).enumerate() {
        assert_eq!(file.name.as_deref(), w["name"].as_str());
        let bytes = if file.bytes.starts_with(b"DCX\0") {
            decoder.decode(&file.bytes)?
        } else {
            file.bytes.clone()
        };
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect();
        assert_eq!(hash, w["sha256"].as_str().ok_or("hash")?);
        let m = MaterialDefinition::read(&bytes)
            .map_err(|e| format!("{}: {e}", file.name.as_deref().unwrap_or("")))?;
        let b = &boundaries[index];
        assert_eq!(b["name"], w["name"]);
        assert_eq!(serde_json::to_value(&m.block_lengths)?, b["blocks"]);
        length_files += usize::from(m.block_lengths.iter().any(|b| b.declared != b.consumed));
        let outer = m.block_lengths.last().ok_or("outer block")?;
        *outer_deltas
            .entry(outer.declared as i64 - outer.consumed as i64)
            .or_default() += 1;
        assert_eq!(m.shader, w["shader"].as_str().ok_or("shader")?);
        assert_eq!(m.description, w["description"].as_str().ok_or("description")?);
        let ps = w["parameters"].as_array().ok_or("parameters")?;
        assert_eq!(m.parameters.len(), ps.len());
        for (p, v) in m.parameters.iter().zip(ps) {
            assert_eq!(p.name, v["name"].as_str().ok_or("name")?);
            let values = if let Some(a) = v["value"].as_array() {
                a.clone()
            } else {
                vec![v["value"].clone()]
            };
            match &p.value {
                Value::Bool(b) => assert_eq!(*b, v["value"].as_bool().ok_or("bool")?),
                Value::Int(a) => assert_eq!(
                    *a,
                    values
                        .iter()
                        .map(|x| x.as_i64().map(|i| i as i32).ok_or("int"))
                        .collect::<std::result::Result<Vec<_>, _>>()?
                ),
                Value::Float(a) => assert_eq!(
                    a.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    values
                        .iter()
                        .map(|x| x.as_f64().map(|x| (x as f32).to_bits()).ok_or("float"))
                        .collect::<std::result::Result<Vec<_>, _>>()?
                ),
            }
            parameters += 1;
        }
        let ts = w["textures"].as_array().ok_or("textures")?;
        assert_eq!(m.textures.len(), ts.len());
        for (t, v) in m.textures.iter().zip(ts) {
            assert_eq!(t.kind, v["kind"].as_str().ok_or("kind")?);
            assert_eq!(t.path, v["path"].as_str().ok_or("path")?);
            assert_eq!(i64::from(t.uv_number), v["uv_number"].as_i64().ok_or("UV")?);
            assert_eq!(
                i64::from(t.shader_data_index),
                v["shader_data_index"].as_i64().ok_or("index")?
            );
            assert_eq!(
                t.extra.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                v["extra"]
                    .as_array()
                    .ok_or("extra")?
                    .iter()
                    .map(|x| x.as_f64().map(|x| (x as f32).to_bits()).ok_or("float"))
                    .collect::<std::result::Result<Vec<_>, _>>()?
            );
            textures += 1;
        }
    }
    println!(
        "Structure-authoritative MTD rule: {length_files} files with block-length differences; outer declared-minus-consumed bytes {outer_deltas:?}"
    );
    println!(
        "MTD definitions {}, parameters {parameters}, texture declarations {textures}: exact",
        files.len()
    );
    Ok(())
}
