//! LZ4 streams for `corpus/general/codecs.tsv` (toolkit/testdata's registry): the LZ4 blocks of Riders Republic's
//! terrain chunks (TNLChunk blocks 1 and 8, each a raw LZ4 block of one tile), cut as stored from the forges given and
//! written as `<n>.lz4`, the first `--limit` (default 64) in forge and entry order. The bytes are the game's own;
//! `repos/primase-foundations/tests/oracles/codecs.py` decodes them with the `lz4` package for the references.
//!
//! `cargo run --release -p primase-anvil --example corpus_riders_lz4 -- <out> <forge>... [--limit N]`.

use std::path::PathBuf;

use anvil_formats::archive::{Archive, At};
use anvil_formats::tnl::{StoredTnlChunk, TNL_CHUNK};

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let mut paths = Vec::new();
    let mut limit = 64usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--limit" => limit = args.next().ok_or("--limit needs a value")?.parse()?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}").into()),
            s => paths.push(PathBuf::from(s)),
        }
    }
    let Some((out, forges)) = paths.split_first().filter(|(_, f)| !f.is_empty()) else {
        return Err("usage: corpus_riders_lz4 <out> <forge>... [--limit N]".into());
    };
    let mut archive = Archive::open(forges)?;
    std::fs::create_dir_all(out)?;
    let mut written = 0usize;
    'forges: for forge in 0..forges.len() {
        let entries: Vec<usize> = archive
            .forge(forge)
            .map(|f| {
                (f.names.iter().enumerate())
                    .filter(|(_, n)| n.type_id == TNL_CHUNK)
                    .map(|(k, _)| k)
                    .collect()
            })
            .unwrap_or_default();
        for entry in entries {
            let resource = archive.resource(At { forge, entry })?;
            let chunk = StoredTnlChunk::parse(&resource.body)?;
            for block in [&chunk.blocks[1], &chunk.blocks[8]] {
                if written == limit {
                    break 'forges;
                }
                if block.is_empty() {
                    continue;
                }
                std::fs::write(out.join(format!("{written:05}.lz4")), block)?;
                written += 1;
            }
        }
    }
    println!("{written} LZ4 blocks written to {}", out.display());
    Ok(())
}
