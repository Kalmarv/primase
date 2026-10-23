//! Raw deflate streams for `corpus/general/codecs.tsv` (toolkit/testdata's registry): compressed binary files of GTA
//! V's archives as stored, decrypted with the install's keys ([`primase_gta::read::source::GtaSource::stored`]), written
//! as `<n>.deflate`: the first `--limit` (default 64) in the listing's order whose stream is at most `--max-bytes`
//! (default 1 MiB), nested archives left out. The bytes are the game's own; `toolkit/compress/tests/oracles/codecs.py`
//! inflates them with Python's zlib for the references.
//!
//! `cargo run --release -p primase-gta --example corpus_gta_deflate -- <GTA V install> <keys folder> <out>
//! [--limit N] [--max-bytes N]`.

use std::path::PathBuf;

use primase_gta::read::archive::listing::Storage;
use primase_gta::read::source::GtaSource;

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let mut paths = Vec::new();
    let (mut limit, mut max_bytes) = (64usize, 1u32 << 20);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--limit" => limit = value()?.parse()?,
            "--max-bytes" => max_bytes = value()?.parse()?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}").into()),
            s => paths.push(PathBuf::from(s)),
        }
    }
    let [install, keys, out] = paths.as_slice() else {
        return Err(
            "usage: corpus_gta_deflate <GTA V install> <keys folder> <out> [--limit N] [--max-bytes N]"
                .into(),
        );
    };
    let source = GtaSource::open(install, keys)?;
    std::fs::create_dir_all(out)?;
    let files: Vec<_> = source
        .install()
        .listing()
        .filter(|f| !f.is_archive())
        .filter(|f| matches!(f.storage, Storage::Binary { stored, .. } if stored != 0 && stored <= max_bytes))
        .map(|f| f.file)
        .collect();
    let mut written = 0usize;
    for file in files {
        if written == limit {
            break;
        }
        let (stream, Some(_)) = source.stored(file)? else {
            continue;
        };
        std::fs::write(out.join(format!("{written:05}.deflate")), &stream)?;
        written += 1;
    }
    println!("{written} deflate streams written to {}", out.display());
    Ok(())
}
