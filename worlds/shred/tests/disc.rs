mod inputs;

use inputs::{Depth, Game, Query, depth, expect_found, install, seed};
use primase_shred::read::{cstr, pak::Pak};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap_or_else(|e| panic!("{}: {e}", dir.display())).path();
        if path.is_dir() {
            archives(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")) {
            out.push(path);
        }
    }
}

#[test]
fn disc_archives_and_every_cstr() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(Query::Install(Game::Shred, None), "PAK archives", paths.len());
    let depth = depth();
    let seed = seed();
    if depth == Depth::Smoke {
        // Stable seeded rotation over the sorted corpus, eight archives per smoke run.
        let n = paths.len();
        paths.rotate_left(seed as usize % n);
        paths.truncate(8);
    }
    let (mut entries, mut streams, mut blocks, mut stored) = (0, 0, 0, 0);
    for path in &paths {
        let p = Pak::open(path).unwrap_or_else(|e| panic!("{} seed={seed}: {e}", path.display()));
        entries += p.entries().len();
        for (i, e) in p.entries().iter().enumerate() {
            assert_eq!(e.hash, primase_foundations::hash::sdbm(&e.name));
            if e.attributes == 2 {
                let d = cstr::decode(p.encoded(i).unwrap(), e.size).unwrap_or_else(|err| {
                    panic!(
                        "{} entry={i} offset={:#x} seed={seed}: {err}",
                        path.display(),
                        e.offset
                    )
                });
                assert_eq!(d.bytes.len(), e.size);
                streams += 1;
                blocks += d.blocks;
                stored += d.stored_blocks;
            }
        }
    }
    eprintln!(
        "SHRED {depth:?} seed={seed}: {} PAKs, {entries} entries, {streams} streams, {blocks} blocks, {stored} stored",
        paths.len()
    );
    if depth == Depth::Full {
        assert_eq!(
            (paths.len(), entries, streams, blocks, stored),
            (336, 67_320, 31_227, 111_391, 4_221)
        );
    }
}
