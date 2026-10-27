//! The corpus oracle set's `skate/cas` (toolkit/testdata's registry): CAS frames of one Oodle-compressed block each
//! (block kinds 0x11, 0x15 and 0x19), cut as stored from the entries of skate.'s `Data`, written as `<n>.casblock`.
//! At most `--per-toc` (default 16) entries' first such block are taken from each superbundle TOC, in its listing
//! order, so the corpus spans the TOCs. The bytes are the game's own; the test decodes them with the game's Oodle
//! library and with oozextract. With `--zlib` it takes the zlib-compressed blocks (kind 2) instead, the zlib streams
//! `repos/primase-compress/tests/oracles/codecs.py` cuts for `corpus/general/codecs.tsv`.
//!
//! `cargo run --release -p primase-reskate --example corpus_skate_cas -- <skate. Data> <oo2core_9_win64.dll> <out>
//! [--per-toc N] [--zlib]`, `<out>` being `<oracles.root>/corpus/skate/cas` (the library decodes the bundle manifests the
//! listing reads).

use std::collections::BTreeSet;
use std::path::PathBuf;

use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::cas;
use primase_frostbite::folder::{Folder, Source};

type BoxError = Box<dyn std::error::Error>;

/// The CAS block kinds the Oodle library decodes.
const OODLE: [u8; 3] = [0x11, 0x15, 0x19];

fn main() -> Result<(), BoxError> {
    let mut paths = Vec::new();
    let (mut per_toc, mut kinds): (usize, &[u8]) = (16, &OODLE);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--per-toc" => per_toc = args.next().ok_or("--per-toc needs a value")?.parse()?,
            "--zlib" => kinds = &[2],
            s if s.starts_with("--") => return Err(format!("unknown option {s}").into()),
            s => paths.push(PathBuf::from(s)),
        }
    }
    let [data, dll, out] = paths.as_slice() else {
        return Err(
            "usage: corpus_skate_cas <skate. Data> <oo2core_9_win64.dll> <out> [--per-toc N] [--zlib]".into(),
        );
    };
    let folder = Folder::open(data)?;
    // SAFETY: the caller names the game's own Oodle library.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9)? };
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    std::fs::create_dir_all(out)?;
    let (mut seen, mut written) = (BTreeSet::new(), 0usize);
    for toc in &folder.tocs {
        let mut taken = 0usize;
        'toc: for bundle in &src.listing(toc)?.bundles {
            for e in bundle.entries() {
                if taken == per_toc {
                    break 'toc;
                }
                if !seen.insert(e.file) {
                    continue;
                }
                let raw = src.raw(e.file)?;
                let Some(block) = cas::blocks(&raw)?.into_iter().find(|b| kinds.contains(&b.kind)) else {
                    continue;
                };
                // the block's eight-byte header and its payload: a frame of its own
                let frame = raw
                    .get(block.offset..block.offset + 8 + block.payload.len())
                    .ok_or("a block past its file")?;
                std::fs::write(out.join(format!("{written:05}.casblock")), frame)?;
                written += 1;
                taken += 1;
            }
        }
    }
    println!("{written} single-block CAS frames written to {}", out.display());
    Ok(())
}
