//! Our D3D pixels of every area texture equal bcdec's exactly, and bcdec agrees with BCnEncoder.NET and Pillow within
//! one channel level.
//!
//! `oracles.scripts.sekiro/dds-area` is the dds-area producer's comparison of three third-party decoders over every
//! distinct area DDS of the SoulsFormats census: bcdec (bcdec_rs 0.2.0), BCnEncoder.NET 2.2.1 (the census's own
//! `DdsWorker.dll`) and Pillow 11.0.0. Per texture it saves the digest of the base decoder's pixels (bcdec's for the
//! block-compressed formats, BCnEncoder's for uncompressed RGB32, which bcdec does not decode) and each other decoder's
//! differences from them. `area_pixels` decodes every texture with our decoder and requires its pixels to hash to the
//! saved base digest, and the saved differences to be at most one channel level; it runs none of the three decoders.
//! Our BC7 decoder is bcdec's, so BC7's exact match checks our block walk and layout, and its arithmetic rests on
//! BCnEncoder's and Pillow's agreement. The comparison names the census and worker assemblies it was made from, and
//! the test refuses it when they differ from those configured.
mod common;
mod inputs;

use inputs::{Game, OracleSet, oracle};
use primase_core::TexFormat;
use primase_testdata::gate::{GateReport, Tolerance};
use primase_texcodec::{Decoder, TexDecoder};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The area textures BCnEncoder.NET 2.2.1 cannot decode over `oracles.soulsformats.sekiro/tpf`, by its reason,
/// measured on that corpus: m25's Galaxy and MilkyWay maps.
const PINNED_REFERENCE_REFUSALS: &[(&str, usize)] = &[("Unsupported compression format: Unknown", 2)];

/// How the saved comparison is made again: the oracle home's recipe for the entry.
const REBUILD: &str =
    "rebuild it with primase-oracles' tools/oracle-home/rebuild.py rebuild scripts/sekiro/dds-area";

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02X}")).collect()
}

/// The sha256 of each assembly of the worker's folder, by file name, as the producer's provenance names them.
fn assemblies(folder: &Path) -> Result<Value> {
    let mut digests = serde_json::Map::new();
    let mut names: Vec<String> = std::fs::read_dir(folder)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<_>>()?;
    names.retain(|n| n.ends_with(".dll"));
    names.sort();
    for n in names {
        let digest = sha256(&std::fs::read(folder.join(&n))?);
        digests.insert(n, Value::String(digest));
    }
    Ok(Value::Object(digests))
}

/// The distinct area DDS of the census by digest, the number of textures BCnEncoder could not decode by its reason,
/// and the number of area texture references.
#[allow(clippy::type_complexity)]
fn area_textures(census: &Path) -> Result<(Vec<String>, BTreeMap<String, usize>, usize)> {
    let mut hashes = std::collections::BTreeSet::new();
    let mut unreferenced: BTreeMap<String, usize> = BTreeMap::new();
    let mut path_count = 0;
    for line in std::fs::read_to_string(census.join("packages.jsonl"))?.lines() {
        let p: Value = serde_json::from_str(line)?;
        if p["area"] != true {
            continue;
        }
        for t in p["textures"].as_array().ok_or("textures")? {
            if t["kind"] == 0 {
                path_count += 1;
                // the census writes `rgba_error` in place of `rgba` for a texture BCnEncoder cannot decode
                if t["rgba"].is_null() {
                    let why = t["rgba_error"].as_str().ok_or("neither rgba nor rgba_error")?;
                    *unreferenced.entry(why.to_owned()).or_default() += 1;
                    continue;
                }
                hashes.insert(t["sha256"].as_str().ok_or("hash")?.to_owned());
            }
        }
    }
    Ok((hashes.into_iter().collect(), unreferenced, path_count))
}

#[test]
fn area_pixels() -> Result<()> {
    let Some(census) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "tpf") else {
        return Ok(());
    };
    let Some(worker) = oracle(OracleSet::Scripts, Game::Sekiro, "dds") else {
        return Ok(());
    };
    let Some(saved) = oracle(OracleSet::Scripts, Game::Sekiro, "dds-area") else {
        return Ok(());
    };
    let made_from: Value = serde_json::from_slice(&std::fs::read(saved.join("provenance.json"))?)?;
    assert_eq!(
        made_from["census"].as_str(),
        Some(sha256(&std::fs::read(census.join("packages.jsonl"))?).as_str()),
        "the saved comparison was made from another census than the configured one; {REBUILD}"
    );
    assert_eq!(
        made_from["worker"],
        assemblies(&worker)?,
        "the saved comparison was made with another DdsWorker than the configured one; {REBUILD}"
    );
    let mut comparison: BTreeMap<String, Value> = BTreeMap::new();
    for line in std::fs::read_to_string(saved.join("comparison.jsonl"))?.lines() {
        let v: Value = serde_json::from_str(line)?;
        comparison.insert(v["sha256"].as_str().ok_or("sha256")?.to_owned(), v);
    }
    let (hashes, unreferenced, path_count) = area_textures(&census)?;
    assert!(
        comparison.keys().eq(hashes.iter()),
        "the saved comparison covers other textures than the census's {} ({} saved); {REBUILD}",
        hashes.len(),
        comparison.len()
    );

    // our pixels of each texture, on the configured threads, by their digest
    let ours = primase_foundations::parallel::map(common::threads(), &hashes, |_, hash| {
        (|| -> Result<(TexFormat, String)> {
            let dds = std::fs::read(census.join("area-dds").join(format!("{hash}.dds")))?;
            let t = primase_dds::read(&dds)?;
            let rgba = Decoder::default().decode_rgba8(t.format, t.width, t.height, &t.levels[0])?;
            Ok((t.format, sha256(&rgba)))
        })()
        .map_err(|e| e.to_string())
    })
    .map_err(|p| p.to_string())?;

    // ours against the base, exactly; each other decoder against the base within one level, overall and by format
    let mut exact = BTreeMap::<String, GateReport>::new();
    let mut agree = BTreeMap::<String, GateReport>::new();
    let mut by_format = BTreeMap::<String, (GateReport, [u64; 256])>::new();
    let mut failures = Vec::new();
    for (hash, ours) in hashes.iter().zip(ours) {
        let (format, digest) = match ours {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{hash}: {e}"));
                continue;
            }
        };
        let saved = &comparison[hash];
        let base = saved["base"]["decoder"].as_str().ok_or("base decoder")?;
        let report = exact
            .entry(base.to_owned())
            .or_insert_with(|| GateReport::new(format!("TPF area ours vs {base}"), Tolerance::Exact));
        if saved["base"]["sha256"].as_str() == Some(digest.as_str()) {
            report.record(hash.as_str(), 0.);
        } else {
            report.fail(
                hash.as_str(),
                format!("{format:?}: our pixels differ from {base}'s"),
            );
        }
        for (other, d) in saved["differences"].as_object().ok_or("differences")? {
            let label = format!("{base} vs {other}");
            let overall = agree
                .entry(label.clone())
                .or_insert_with(|| GateReport::new(format!("TPF area {label}"), Tolerance::Abs(1.)));
            let label = format!("{format:?}: {label}");
            let (by, histogram) = by_format
                .entry(label.clone())
                .or_insert_with(|| (GateReport::new(&label, Tolerance::Abs(1.)), [0; 256]));
            if let Some(lengths) = d.get("lengths") {
                overall.fail(hash.as_str(), format!("lengths differ: {lengths}"));
                by.fail(hash.as_str(), format!("lengths differ: {lengths}"));
                continue;
            }
            let max = d["max"].as_f64().ok_or("max")?;
            overall.record(hash.as_str(), max);
            by.record(hash.as_str(), max);
            for pair in d["histogram"].as_array().ok_or("histogram")? {
                let (diff, n) = (pair[0].as_u64().ok_or("diff")?, pair[1].as_u64().ok_or("count")?);
                histogram[usize::try_from(diff)?.min(255)] += n;
            }
        }
    }
    println!("area references {path_count}, distinct DDS {}", hashes.len());
    // BCnEncoder's refusals, pinned: a new reason, or more of one, fails
    for (why, n) in &unreferenced {
        println!("  not decoded by BCnEncoder ({n}): {why}");
    }
    let over: Vec<String> = unreferenced
        .iter()
        .filter(|(why, n)| {
            PINNED_REFERENCE_REFUSALS
                .iter()
                .all(|(p, pin)| p != why || *n > pin)
        })
        .map(|(why, n)| format!("{why}: {n}"))
        .collect();
    assert!(over.is_empty(), "BCnEncoder refusals beyond the pins: {over:?}");
    for report in exact.values().chain(agree.values()) {
        println!("{report}");
    }
    for (report, histogram) in by_format.values() {
        println!(
            "{report}, channel histogram {:?}",
            histogram
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .collect::<Vec<_>>()
        );
    }
    for e in &failures {
        println!("FAIL {e}");
    }
    assert!(failures.is_empty());
    for report in exact.values() {
        assert!(
            report.passed(),
            "our pixels must equal the saved exact digest of bcdec's (BCnEncoder's for RGB32) on every area texture:\n{report}"
        );
    }
    for report in agree.values() {
        assert!(
            report.passed(),
            "bcdec must agree with BCnEncoder and Pillow (BCnEncoder with Pillow on RGB32) within one channel level, as the saved differences record:\n{report}"
        );
    }
    Ok(())
}
