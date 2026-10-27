//! Container structure from the published CgBinaryProgram description, checked
//! over every packaged shader occurrence. Microcode semantics are a separate gate.
mod inputs;

use inputs::{Depth, Game, Query, depth, expect_found, install, seed};
use rme_formats::pak::Pak;
use rsx_reader::cgbinary::{Stage, parse_prefix};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = entry.expect("directory entry").path();
        if p.is_dir() {
            archives(&p, out);
        } else if p.extension().is_some_and(|s| s.eq_ignore_ascii_case("pak")) {
            out.push(p);
        }
    }
}

#[test]
fn every_packaged_vertex_and_fragment_container() {
    let Some(root) = install(Game::Shred) else { return };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", paths.len());
    let depth = depth();
    let seed = seed();
    if depth == Depth::Smoke {
        let n = paths.len();
        paths.rotate_left(seed as usize % n);
    }
    let (mut files, mut programs) = ([0usize; 2], [0usize; 2]);
    let (mut visited, mut shader_archives, mut parameters) = (0, 0, 0);
    for path in &paths {
        if depth == Depth::Smoke && shader_archives >= 8 {
            break;
        }
        visited += 1;
        let pak = Pak::open(path).unwrap();
        let mut found = false;
        for (i, entry) in pak.entries().iter().enumerate() {
            let kind = if entry.name.ends_with(b".vsp") {
                0
            } else if entry.name.ends_with(b".psp") {
                1
            } else {
                continue;
            };
            found = true;
            files[kind] += 1;
            let bytes = pak.payload(i).unwrap();
            let label = format!("{} {:?}", path.display(), String::from_utf8_lossy(&entry.name));
            assert!(!bytes.is_empty(), "empty shader {label}");
            let mut offset = 0;
            while offset < bytes.len() {
                let p = parse_prefix(&bytes[offset..])
                    .unwrap_or_else(|e| panic!("{label} program at {offset}: {e}"));
                assert_eq!(matches!(p.stage, Stage::Fragment(_)), kind == 1, "{label}");
                programs[kind] += 1;
                parameters += p.parameters.len();
                let end = offset + p.bytes.len();
                let padded = end.checked_add(63).expect("padding overflow") & !63;
                let padding = bytes
                    .get(end..padded)
                    .unwrap_or_else(|| panic!("truncated 64-byte padding {label}"));
                assert!(padding.iter().all(|&b| b == 0), "nonzero padding {label}");
                offset = padded;
            }
            assert_eq!(offset, bytes.len(), "{label}");
        }
        shader_archives += usize::from(found);
    }
    expect_found(
        Query::Install(Game::Shred, None),
        "CgBinary vertex programs",
        programs[0],
    );
    expect_found(
        Query::Install(Game::Shred, None),
        "CgBinary fragment programs",
        programs[1],
    );
    eprintln!(
        "SHRED CgBinary {depth:?} seed={seed}: PAKs={visited} shader_PAKs={shader_archives} VSP={} PSP={} vertex={} fragment={} parameters={parameters}",
        files[0], files[1], programs[0], programs[1]
    );
    if depth == Depth::Full {
        assert_eq!(files, [743, 765]);
        assert_eq!(programs, [199017, 181851]);
        assert_eq!(parameters, 8041219);
    }
}
