//! Mesh gate against an independent reader's decoded meshes (`oracles.unitypy.sledding/meshes.json`, JSON: `meshes`
//! with `file`, `path_id`, `vertex_count`, `compressed`, `channels` by name (`m_Vertices`, `m_Normals`, `m_Tangents`,
//! `m_Colors`, `m_UV0`..`m_UV7`) each `dim` and either `sha256` of its values as little-endian f32 or the `values`, and
//! `indices` as `count` and `sha256` of little-endian u32s), for the player `installs.sledding/Sledding Game_Data` read
//! with `tools.unity_tpk`.
//!
//! Stored channels and index buffers exactly (by digest; the oracle keeps 8-bit normalised colours as their bytes, so
//! ours are compared times 255); a compressed mesh's channel lengths and indices exactly, its values, dequantised
//! and rebuilt (the oracle in f64, we in f32), within the bounds of `bound`, counted per channel.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use unity_formats::TypeTreePackage;
use unity_formats::Value;
use unity_formats::mesh::UnityMeshData;
use unity_formats::player::{ObjRef, Player};

fn digest_f32(v: impl Iterator<Item = f32>) -> String {
    let b: Vec<u8> = v.flat_map(|x| x.to_le_bytes()).collect();
    Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect()
}

const NAMES: [&str; 12] = [
    "m_Vertices",
    "m_Normals",
    "m_Tangents",
    "m_Colors",
    "m_UV0",
    "m_UV1",
    "m_UV2",
    "m_UV3",
    "m_UV4",
    "m_UV5",
    "m_UV6",
    "m_UV7",
];

/// A compressed channel's differences from the oracle: values, not bit-equal, beyond the bound, largest absolute
/// difference.
#[derive(Default)]
struct Diff {
    values: u64,
    differ: u64,
    beyond: u64,
    max: f64,
    at: String,
}

/// The packed vector a compressed mesh stores channel `ch` in.
const PACKED: [&str; 5] = ["m_Vertices", "m_Normals", "m_Tangents", "m_FloatColors", "m_UV"];

/// The largest magnitude of the ends of channel `ch`'s packed vector in mesh `r`, max(|start|, |start + range|).
fn ends(player: &Player, r: &ObjRef, ch: usize) -> f64 {
    let m = player.decode(r).unwrap();
    let v = m
        .get("m_CompressedMesh")
        .and_then(|c| c.get(PACKED[ch.min(4)]))
        .unwrap();
    let f = |k: &str| f64::from(v.get(k).and_then(Value::as_f32).unwrap());
    let (start, range) = (f("m_Start"), f("m_Range"));
    start.abs().max((start + range).abs())
}

/// The bound for component `k` of compressed channel `ch` whose packed vector's ends reach magnitude `end`.
///
/// We dequantise `item * (range / n) + start` in f32, the oracle in f64: the quotient and product each round by half an
/// epsilon of at most |range| <= 2 `end`, the sum by half an epsilon of at most `end`, 2.5 epsilon `end` in all, taken
/// as e = 4 epsilon max(1, `end`). A normal's or tangent's z is rebuilt as sqrt(1 - x^2 - y^2): x and y each within e
/// on the unit disc move 1 - x^2 - y^2 by at most 2 sqrt(2) e + 2 e^2 < 3 e, its f32 evaluation by 2 epsilon more, and
/// |sqrt(a) - sqrt(b)| <= sqrt(|a - b|), so sqrt(3 e + 2 epsilon). A tangent's w is a stored sign: exact.
fn bound(ch: usize, k: usize, end: f64) -> f64 {
    let eps = f64::from(f32::EPSILON);
    let e = 4.0 * eps * end.max(1.0);
    match (ch, k) {
        (1 | 2, 2) => (3.0 * e + 2.0 * eps).sqrt(),
        (2, 3) => 0.0,
        _ => e,
    }
}

#[test]
fn meshes_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        install_file(Game::Sledding, "Sledding Game_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Sledding, "meshes.json"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let oracle: Json = serde_json::from_slice(&std::fs::read(&oracle).unwrap()).unwrap();
    let mut stored = GateReport::new(
        "stored meshes: channels and indices bit for bit",
        Tolerance::Exact,
    );
    let mut compressed = GateReport::new(
        "compressed meshes: channel lengths and indices exactly",
        Tolerance::Exact,
    );
    let mut diffs: BTreeMap<&str, Diff> = BTreeMap::new();
    for m in oracle["meshes"].as_array().unwrap() {
        let file = m["file"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_lowercase();
        let r = ObjRef {
            file,
            path_id: m["path_id"].as_i64().unwrap(),
        };
        let item = format!("{}/{} {}", r.file, r.path_id, m["name"].as_str().unwrap_or(""));
        let is_compressed = m["compressed"].as_bool().unwrap();
        let gate = if is_compressed {
            &mut compressed
        } else {
            &mut stored
        };
        let ours = match UnityMeshData::read(&player, &r) {
            Ok(x) => x,
            Err(e) => {
                gate.fail(&item, e.to_string());
                continue;
            }
        };
        let mut problems = Vec::new();
        if ours.vertex_count as u64 != m["vertex_count"].as_u64().unwrap() {
            problems.push(format!(
                "vertex count {} vs {}",
                ours.vertex_count, m["vertex_count"]
            ));
        }
        for (ch, name) in NAMES.iter().enumerate() {
            let theirs = &m["channels"][name];
            let c = match (&ours.channels[ch], theirs.is_null()) {
                (None, true) => continue,
                (Some(_), true) => {
                    problems.push(format!("{name} only in ours"));
                    continue;
                }
                (None, false) => {
                    problems.push(format!("{name} missing"));
                    continue;
                }
                (Some(c), false) => c,
            };
            if c.dim as u64 != theirs["dim"].as_u64().unwrap() {
                problems.push(format!("{name} dimension {} vs {}", c.dim, theirs["dim"]));
                continue;
            }
            if let Some(h) = theirs["sha256"].as_str() {
                let same = digest_f32(c.values.iter().copied()) == h
                    || (ch == 3 && digest_f32(c.values.iter().map(|x| (x * 255.0).round())) == h);
                if !same {
                    problems.push(format!("{name} values differ"));
                }
                continue;
            }
            let t = theirs["values"].as_array().unwrap();
            if t.len() != c.values.len() {
                problems.push(format!("{name} {} vs {} values", c.values.len(), t.len()));
                continue;
            }
            let end = ends(&player, &r, ch);
            let d = diffs.entry(name).or_default();
            for (i, (&a, b)) in c.values.iter().zip(t).enumerate() {
                let b = b.as_f64().unwrap();
                let x = (f64::from(a) - b).abs();
                d.values += 1;
                if f64::from(a) != b {
                    d.differ += 1;
                }
                if x > bound(ch, i % c.dim, end) {
                    d.beyond += 1;
                }
                if x > d.max {
                    d.max = x;
                    d.at = format!("{item} value {i}: {a} vs {b}");
                }
            }
        }
        if ours.indices.len() as u64 != m["indices"]["count"].as_u64().unwrap() || {
            let b: Vec<u8> = ours.indices.iter().flat_map(|x| x.to_le_bytes()).collect();
            let h: String = Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect();
            h != m["indices"]["sha256"].as_str().unwrap()
        } {
            problems.push("indices differ".into());
        }
        if problems.is_empty() {
            gate.record(&item, 0.0);
        } else {
            gate.fail(&item, problems.join("; "));
        }
    }
    println!("{stored}\n{compressed}");
    for (name, d) in &diffs {
        println!(
            "compressed {name} (dequantised in f32 vs the oracle's f64): {} values, {} not equal, {} beyond the bound, \
             largest difference {:e} ({})",
            d.values, d.differ, d.beyond, d.max, d.at
        );
    }
    stored.assert_passed();
    compressed.assert_passed();
    assert!(
        diffs.values().all(|d| d.beyond == 0),
        "compressed values beyond the bound"
    );
}
