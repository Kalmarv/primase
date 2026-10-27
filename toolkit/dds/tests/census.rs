//! Distinct-content DDS census over the game's own files (Sekiro's DDS as its install stores them, `census/dds` of
//! `oracles.scripts.sekiro/pbr`): every file reads, round-trips through the DX10 writer to the same texture, and a
//! legacy DXT file's payload is written back byte for byte by the legacy writer, header differences measured.
#![allow(clippy::unwrap_used)]
mod inputs;

use inputs::{Game, OracleSet, Query, expect_found, resolve};
use primase_dds::{read, write_dx10, write_legacy};
use primase_testdata::gate::{GateReport, Tolerance};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
/// Every `.dds` file under `dir`.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let p = entry.path();
        if kind.is_dir() {
            collect(&p, out);
        } else if kind.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dds")) {
            out.push(p);
        }
    }
}
#[test]
fn sekiro_dds_read_round_trip_and_keep_their_legacy_payload() {
    let query = Query::Oracle(OracleSet::Scripts, Game::Sekiro, "pbr");
    let Some(pbr) = resolve(query) else { return };
    let root = pbr.join("census").join("dds");
    let name = query.key();
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    expect_found(query, "DDS files", files.len());
    let mut outside_legacy = 0;
    let mut counts = BTreeMap::<String, usize>::new();
    let mut read_gate = GateReport::new(format!("{name} read"), Tolerance::Exact);
    let mut roundtrip = GateReport::new(format!("{name} stored levels"), Tolerance::Exact);
    let mut oracle = GateReport::new(format!("{name} legacy payload identity"), Tolerance::Exact);
    let mut header_differences = BTreeMap::<usize, u64>::new();
    let path_count = files.len();
    let mut contents = std::collections::BTreeSet::<[u8; 32]>::new();
    for path in files {
        let key = path.display().to_string();
        let bytes = std::fs::read(&path).unwrap();
        if !contents.insert(Sha256::digest(&bytes).into()) {
            continue;
        }
        match read(&bytes) {
            Err(e) => read_gate.fail(&key, e.to_string()),
            Ok(t) => {
                *counts
                    .entry(format!("{:?}/{:?}", t.format, t.color_space))
                    .or_default() += 1;
                read_gate.record(&key, 0.0);
                match write_dx10(&t).and_then(|b| read(&b)) {
                    Ok(other) if other == t => roundtrip.record(&key, 0.0),
                    Ok(_) => roundtrip.fail(&key, "texture changed"),
                    Err(e) => roundtrip.fail(&key, e.to_string()),
                }
                if matches!(&bytes[84..88], b"DXT1" | b"DXT3" | b"DXT5") {
                    match write_legacy(&t) {
                        Ok(b) => {
                            assert_eq!(read(&b).unwrap(), t);
                            oracle.compare_bytes(&key, &b[128..], &bytes[128..]);
                            for offset in (4..128).step_by(4) {
                                if b[offset..offset + 4] != bytes[offset..offset + 4] {
                                    *header_differences.entry(offset).or_default() += 1;
                                }
                            }
                            let flags = u32::from_le_bytes(b[8..12].try_into().unwrap());
                            assert_eq!(flags, 0x81007 | if t.levels.len() > 1 { 0x20000 } else { 0 });
                        }
                        Err(e) => oracle.fail(&key, e.to_string()),
                    }
                } else {
                    outside_legacy += 1;
                }
            }
        }
    }
    eprintln!(
        "root={}\nformats={counts:#?}\n{read_gate}\n{roundtrip}",
        root.display()
    );
    eprintln!(
        "{name}: {path_count} paths, {} distinct contents (SHA256)",
        contents.len()
    );
    eprintln!(
        "{oracle}\nheader field differences by byte offset={header_differences:?}\noutside legacy DXT writer scope: {outside_legacy}"
    );
    oracle.assert_passed();
    read_gate.assert_passed();
    roundtrip.assert_passed();
}
