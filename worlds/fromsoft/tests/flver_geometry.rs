//! Area FLVER geometry against the independent pre-bake C# model exporter.
use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::{
    Archive, Binder,
    dcx::Decoder,
    flver::{MaterialGeometry, Model},
};
use primase_testdata::real_data;
use sha2::{Digest, Sha256};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
mod native {
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
    }
}
#[test]
fn area_geometry() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_MODEL_ORACLE") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let mut gate = GateReport::new("FLVER pre-bake C# geometry", Tolerance::Exact);
    let mut derived = GateReport::new("FLVER derived normals/UV maximum ULP", Tolerance::Abs(4.));
    let mut ulp_histogram = std::collections::BTreeMap::<u32, u64>::new();
    let mut models = 0;
    let mut parts = 0;
    let mut failures = Vec::new();
    for line in std::fs::read_to_string(oracle.join("models.jsonl"))?.lines() {
        let w: serde_json::Value = serde_json::from_str(line)?;
        if w["missing"].as_bool() == Some(true) {
            continue;
        }
        let key = w["key"].as_str().ok_or("key")?;
        let (area, model) = key.split_once('/').ok_or("key separator")?;
        let (path, leaf) = if area == "obj" {
            (format!("/obj/{model}.objbnd.dcx"), format!("{model}.flver"))
        } else {
            let stem = format!("{area}_{}", &model[1..]);
            (format!("/map/{area}/{stem}.mapbnd.dcx"), format!("{stem}.flver"))
        };
        let bytes = archive.read(&path, &decoder)?;
        let binder = Binder::read(&bytes, &decoder)?;
        let f = binder
            .files
            .iter()
            .find(|f| {
                f.name
                    .as_deref()
                    .unwrap_or_default()
                    .replace('\\', "/")
                    .rsplit('/')
                    .next()
                    == Some(leaf.as_str())
            })
            .ok_or("model member")?;
        let parsed = Model::read(&f.bytes)?;
        let materials = w["materials"]
            .as_array()
            .ok_or("materials")?
            .iter()
            .map(|m| {
                Ok(MaterialGeometry {
                    skip: !m["Skip"].is_null(),
                    uv_scale: [
                        m["uv_scale"][0].as_f64().ok_or("uv")? as f32,
                        m["uv_scale"][1].as_f64().ok_or("uv")? as f32,
                    ],
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let actual = parsed.geometry(&materials)?;
        let expected = w["parts"].as_array().ok_or("parts")?;
        if actual.len() != expected.len() {
            failures.push(format!(
                "{key}: part count {} != {}",
                actual.len(),
                expected.len()
            ));
            continue;
        }
        for (part, p) in actual.iter().zip(expected) {
            let mut bytes = Vec::new();
            bytes.extend((part.mesh.positions.len() as u32).to_le_bytes());
            bytes.extend((part.mesh.indices.len() as u32).to_le_bytes());
            for x in part
                .mesh
                .positions
                .iter()
                .flatten()
                .chain(part.mesh.normals.iter().flatten())
                .chain(part.mesh.uv.iter().flatten())
            {
                bytes.extend(x.to_le_bytes());
            }
            for i in &part.mesh.indices {
                bytes.extend(i.to_le_bytes());
            }
            let wanted = std::fs::read(oracle.join(p["file"].as_str().ok_or("part file")?))?;
            let hash: String = Sha256::digest(&wanted)
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect();
            assert_eq!(hash, p["sha256"].as_str().ok_or("hash")?);
            let label = format!("{key}/{}", part.source_mesh);
            let n = part.mesh.positions.len();
            if bytes.len() != wanted.len() || bytes[..8] != wanted[..8] {
                gate.fail(&label, "vertex/index counts differ");
                continue;
            }
            gate.compare_bytes(
                format!("{label} positions"),
                &bytes[8..8 + n * 12],
                &wanted[8..8 + n * 12],
            );
            gate.compare_bytes(
                format!("{label} topology"),
                &bytes[8 + n * 32..],
                &wanted[8 + n * 32..],
            );
            let mut maximum = 0;
            for (a, b) in bytes[8 + n * 12..8 + n * 32]
                .as_chunks::<4>()
                .0
                .iter()
                .zip(wanted[8 + n * 12..8 + n * 32].as_chunks::<4>().0)
            {
                let a = f32::from_le_bytes(*a);
                let b = f32::from_le_bytes(*b);
                if !a.is_finite() || !b.is_finite() {
                    return Err("non-finite geometry oracle".into());
                }
                let ordered = |v: f32| {
                    if v.is_sign_negative() {
                        !v.to_bits()
                    } else {
                        v.to_bits() | 0x8000_0000
                    }
                };
                let ulp = if a == b {
                    0
                } else {
                    ordered(a).abs_diff(ordered(b))
                };
                maximum = maximum.max(ulp);
                *ulp_histogram.entry(ulp).or_default() += 1;
            }
            derived.record(label, f64::from(maximum));
            parts += 1;
        }
        models += 1;
    }
    println!(
        "pre-bake models {models}, parts {parts}, failures {}",
        failures.len()
    );
    for f in &failures {
        println!("FAIL {f}");
    }
    println!("{gate}\n{derived}\nDerived-component ULP histogram {ulp_histogram:?}");
    gate.assert_passed();
    derived.assert_passed();
    assert!(failures.is_empty());
    Ok(())
}
