//! Distinct-content DDS censuses over the games' own files. Sekiro's DDS as its install stores them (`census/dds` of
//! `oracles.scripts.sekiro/pbr`): every file reads, round-trips through the DX10 writer to the same texture, and is
//! written back under the header it had byte for byte. TrackMania Nations Forever's DDS (`oracles.gbxnet.tmnf/tmnf-packs`,
//! legacy 2D textures and cube maps). With directxtex-dump configured (`tools.directxtex_dump`), DirectXTex's library
//! reads each file of both as it is read here, and each file refused here is one DirectXTex refuses too or one it changes
//! as it loads it (the formats this reader names as converted).
#![allow(clippy::unwrap_used)]
mod harness;
mod inputs;

use harness::{agrees, directxtex};
use inputs::{Game, OracleSet, Query, Tool, expect_found, resolve, tool};
use primase_dds::{HeaderKind, WriteHeader, parse, read, write, write_dx10};
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

/// The distinct contents (by SHA256) of the `.dds` files under `root`, by path, and the number of paths.
fn distinct(root: &Path) -> (Vec<(String, Vec<u8>)>, usize) {
    let mut paths = Vec::new();
    collect(root, &mut paths);
    paths.sort();
    let count = paths.len();
    let mut seen = std::collections::BTreeSet::<[u8; 32]>::new();
    let files = paths
        .into_iter()
        .filter_map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            seen.insert(Sha256::digest(&bytes).into())
                .then(|| (path.display().to_string(), bytes))
        })
        .collect();
    (files, count)
}

/// `files` read by DirectXTex's library as they are read here: a file read here DirectXTex reads alike (a cube whose
/// arraySize counts faces with its permissive load, which its default load refuses); a file refused here DirectXTex
/// refuses too, or reads it changed, which the refusal names. Prints how DirectXTex took the files.
fn against_directxtex(name: &str, files: &[(String, Vec<u8>)]) {
    let Some(dump) = tool(Tool::DirectxtexDump) else {
        return;
    };
    let mut gate = GateReport::new(format!("{name} read by DirectXTex"), Tolerance::Exact);
    let mut tally = BTreeMap::<String, usize>::new();
    for chunk in files.chunks(400) {
        let parsed: Vec<_> = chunk.iter().map(|(_, bytes)| parse(bytes)).collect();
        let runs: Vec<_> = chunk
            .iter()
            .zip(&parsed)
            .flat_map(|((_, bytes), p)| {
                let faces = p.as_ref().is_ok_and(|p| p.faces_counted);
                std::iter::once(("read", bytes.as_slice()))
                    .chain(faces.then_some(("read-permissive", bytes.as_slice())))
            })
            .collect();
        let answers = directxtex(&dump, &runs);
        let mut next = answers.iter();
        for ((key, _), p) in chunk.iter().zip(parsed) {
            let default = next.next().unwrap();
            let (class, result) = match p {
                Ok(p) if p.faces_counted => {
                    let permissive = next.next().unwrap();
                    (
                        "read alike by DirectXTex's permissive load; its default load refuses the face-counted cube"
                            .to_owned(),
                        if default.read() {
                            Err("DirectXTex's default load read a face-counted cube".to_owned())
                        } else {
                            agrees(&p.dds, permissive)
                        },
                    )
                }
                Ok(p) => ("read alike by DirectXTex".to_owned(), agrees(&p.dds, default)),
                Err(e) if !default.read() => (format!("refused here and by DirectXTex: {e}"), Ok(())),
                Err(e) => {
                    let format = &default.meta["format"];
                    let class = format!("refused here, read changed by DirectXTex (as DXGI {format}): {e}");
                    // a refusal names what DirectXTex changes on load; any other is a file DirectXTex reads that this does not
                    let named = e.to_string().contains("DirectXTex");
                    (
                        class,
                        if named {
                            Ok(())
                        } else {
                            Err(format!("refused here, read by DirectXTex: {e}"))
                        },
                    )
                }
            };
            *tally.entry(class).or_default() += 1;
            match result {
                Ok(()) => gate.record(key, 0.0),
                Err(e) => gate.fail(key, e),
            }
        }
    }
    eprintln!("{name}: DirectXTex={tally:#?}\n{gate}");
    gate.assert_passed();
}

#[test]
fn sekiro_dds_read_round_trip_and_write_back_byte_for_byte() {
    let query = Query::Oracle(OracleSet::Scripts, Game::Sekiro, "pbr");
    let Some(pbr) = resolve(query) else { return };
    let root = pbr.join("census").join("dds");
    let name = query.key();
    let (files, path_count) = distinct(&root);
    expect_found(query, "DDS files", path_count);
    let mut legacy = 0;
    let mut counts = BTreeMap::<String, usize>::new();
    let mut read_gate = GateReport::new(format!("{name} read"), Tolerance::Exact);
    let mut roundtrip = GateReport::new(format!("{name} stored levels"), Tolerance::Exact);
    let mut oracle = GateReport::new(
        format!("{name} byte identity under the same header"),
        Tolerance::Exact,
    );
    let mut header_differences = BTreeMap::<usize, u64>::new();
    for (key, bytes) in &files {
        match read(bytes) {
            Err(e) => read_gate.fail(key, e.to_string()),
            Ok(t) => {
                *counts
                    .entry(format!("{:?}/{:?}", t.format, t.color_space))
                    .or_default() += 1;
                read_gate.record(key, 0.0);
                match write_dx10(&t).and_then(|b| read(&b)) {
                    Ok(other) if other == t => roundtrip.record(key, 0.0),
                    Ok(_) => roundtrip.fail(key, "texture changed"),
                    Err(e) => roundtrip.fail(key, e.to_string()),
                }
                // The file rewritten under the header it had: Sekiro's writer emits DirectXTex's canonical header, so
                // the whole file comes back byte for byte (header fields that differ are counted, to say where).
                let parsed = parse(bytes).unwrap();
                let (header, start) = match parsed.header {
                    HeaderKind::Legacy(_) => (WriteHeader::Legacy, 128),
                    HeaderKind::Dx10 => (WriteHeader::Dx10, 148),
                };
                if header == WriteHeader::Legacy {
                    legacy += 1;
                }
                match write(&parsed.dds, header) {
                    Ok(b) => {
                        oracle.compare_bytes(key, &b, bytes);
                        for offset in (4..start).step_by(4) {
                            if b[offset..offset + 4] != bytes[offset..offset + 4] {
                                *header_differences.entry(offset).or_default() += 1;
                            }
                        }
                        assert_eq!(
                            u32::from_le_bytes(b[8..12].try_into().unwrap()) & 0x21007,
                            0x21007
                        );
                    }
                    Err(e) => oracle.fail(key, e.to_string()),
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
        files.len()
    );
    eprintln!(
        "{oracle}\nheader field differences by byte offset={header_differences:?}\nlegacy headers: {legacy}"
    );
    oracle.assert_passed();
    read_gate.assert_passed();
    roundtrip.assert_passed();
    against_directxtex(&name, &files);
}

#[test]
fn tmnf_dds_read_as_directxtex_reads_them() {
    let query = Query::Oracle(OracleSet::Gbxnet, Game::Tmnf, "tmnf-packs");
    let Some(root) = resolve(query) else { return };
    let (files, path_count) = distinct(&root);
    expect_found(query, "DDS files", path_count);
    let mut shapes = BTreeMap::<String, usize>::new();
    for (_, bytes) in &files {
        let class = match parse(bytes) {
            Ok(p) => format!("{:?} {:?} {:?}", p.dds.shape, p.dds.format, p.header),
            Err(e) => format!("refused: {e}"),
        };
        *shapes.entry(class).or_default() += 1;
    }
    eprintln!(
        "{}: {path_count} paths, {} distinct contents (SHA256)\nshapes={shapes:#?}",
        query.key(),
        files.len()
    );
    against_directxtex(&query.key(), &files);
}
