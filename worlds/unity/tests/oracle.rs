//! Differential gate against an independent reader's decoded values (`PRIMASE_UNITY_ORACLE`: a JSON file of samples,
//! `{"samples": [{"file", "path_id", "class_id", "value"}]}`, byte strings as `{"$bytes": length, "sha256": hex}`,
//! pairs as lists, non-finite floats as strings; a byte string the oracle lists as numbers is digested to compare),
//! for objects of the data folder `PRIMASE_UNITY_DATA` decoded with
//! the type tree package `PRIMASE_UNITY_TPK`. Values compare as parsed JSON (`compare_json`, no tolerance), per class.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};

use primase_core::gate::{GateReport, Tolerance, compare_json};
use primase_testdata::real_data;
use primase_unity::{SerializedFile, TypeTreePackage};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

fn bytes_json(b: &[u8]) -> Json {
    let h: String = Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect();
    json!({"$bytes": b.len(), "sha256": h})
}

/// Where ours is a byte string's digest and the oracle lists the bytes as numbers (0..=255), digest the oracle's list
/// (the same bytes in another representation).
fn bytes_alike(oracle: &mut Json, ours: &Json) {
    match (oracle, ours) {
        (o @ Json::Array(_), Json::Object(m)) if m.contains_key("$bytes") => {
            let b: Option<Vec<u8>> = o
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect();
            if let Some(b) = b {
                *o = bytes_json(&b);
            }
        }
        (Json::Array(a), Json::Array(b)) => {
            for (x, y) in a.iter_mut().zip(b) {
                bytes_alike(x, y);
            }
        }
        (Json::Object(a), Json::Object(b)) => {
            for (k, x) in a.iter_mut() {
                if let Some(y) = b.get(k) {
                    bytes_alike(x, y);
                }
            }
        }
        _ => {}
    }
}

#[test]
fn values_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_ORACLE"),
    ) else {
        return;
    };
    let package = TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap();
    let oracle: Json = serde_json::from_slice(&std::fs::read(&oracle).unwrap()).unwrap();
    let mut files: HashMap<String, SerializedFile> = HashMap::new();
    let mut per_class: BTreeMap<String, GateReport> = BTreeMap::new();
    let mut numbers = 0u64;
    for s in oracle["samples"].as_array().unwrap() {
        let rel = s["file"].as_str().unwrap();
        let path_id = s["path_id"].as_i64().unwrap();
        let file = files
            .entry(rel.to_owned())
            .or_insert_with(|| SerializedFile::parse(std::fs::read(data.join(rel)).unwrap()).unwrap());
        let o = *file.object(path_id).unwrap();
        let class = package
            .class_name(o.class_id, file.unity)
            .map_or_else(|| format!("class {}", o.class_id), str::to_owned);
        let gate = per_class.entry(class.clone()).or_insert_with(|| {
            GateReport::new(format!("{class}: values equal the oracle's"), Tolerance::Exact)
        });
        let item = format!("{rel}/{path_id}");
        let tree = package.type_tree(o.class_id, file.unity).unwrap();
        match file.decode_with(&o, &tree) {
            Ok(d) => {
                let ours = d.value.to_json(&bytes_json);
                let mut theirs = s["value"].clone();
                bytes_alike(&mut theirs, &ours);
                let c = compare_json(&theirs, &ours, 0);
                numbers += c.numbers;
                if c.equal() {
                    gate.record(&item, 0.0);
                } else {
                    gate.fail(&item, c.differences.join("; "));
                }
            }
            Err(e) => gate.fail(&item, e.to_string()),
        }
    }
    let (mut checked, mut failed) = (0, 0);
    for g in per_class.values() {
        checked += g.checked;
        failed += g.failures.len();
        if !g.failures.is_empty() {
            println!("{g}");
        }
    }
    println!(
        "{checked} sampled objects of {} classes against {}: {failed} differ; {numbers} numbers compared",
        per_class.len(),
        oracle["oracle"].as_str().unwrap_or("the oracle")
    );
    assert_eq!(failed, 0);
}
