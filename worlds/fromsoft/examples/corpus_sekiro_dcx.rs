//! The corpus oracle set's `sekiro/dcx` (toolkit/testdata's registry): KRAK-compressed DCX files exactly as Sekiro's
//! archives store them, read from the install with the user's keys and written as `<hash>.dcx` (the entry's path
//! hash, hex): the first `--limit` of them (default 256) in hash order whose stored size is at most `--max-bytes`
//! (default 4 MiB). The bytes are the game's own; the test decodes them with the game's Oodle library and with
//! oozextract, independently of this program. With `--compression DFLT` it writes the zlib-compressed envelopes instead,
//! the zlib streams `https://github.com/Kalmarv/primase-foundations/blob/main/tests/oracles/codecs.py` cuts for `corpus/general/codecs.tsv`.
//!
//! `cargo run --release -p primase-fromsoft --example corpus_sekiro_dcx -- <Sekiro install> <keys> <out>
//! [--limit N] [--max-bytes N] [--compression KRAK|DFLT]`, `<keys>` the folder of `Data1.pem` to `Data5.pem`
//! (`keys.sekiro`), `<out>` being `<oracles.root>/corpus/sekiro/dcx`.

use primase_fromsoft::game::ParamKey;
use std::path::PathBuf;

use fromsoft_formats::{Archive, dcx};

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let mut paths = Vec::new();
    let (mut limit, mut max_bytes, mut compression) = (256usize, 4usize << 20, *b"KRAK");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--limit" => limit = value()?.parse()?,
            "--max-bytes" => max_bytes = value()?.parse()?,
            "--compression" => {
                compression = match value()?.as_str() {
                    "KRAK" => *b"KRAK",
                    "DFLT" => *b"DFLT",
                    other => return Err(format!("--compression {other}: KRAK or DFLT").into()),
                }
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}").into()),
            s => paths.push(PathBuf::from(s)),
        }
    }
    let [install, keys, out] = paths.as_slice() else {
        return Err(
            "usage: corpus_sekiro_dcx <Sekiro install> <keys> <out> [--limit N] [--max-bytes N] [--compression KRAK|DFLT]"
                .into(),
        );
    };
    let game = fromsoft_formats::Game::Sekiro;
    let archive = Archive::open_game(
        install,
        game,
        &fromsoft_formats::Keys::from_folder(keys, game, game.param_key_sha256())?,
    )?;
    std::fs::create_dir_all(out)?;
    let mut written = 0usize;
    for (&hash, indexed) in &archive.entries {
        if written == limit {
            break;
        }
        if indexed.entry.padded_size > max_bytes {
            continue;
        }
        // as stored: the archive pads an entry to 16 bytes with zeros, which a DCX allows after its payload
        let bytes = archive.read_stored(hash)?;
        if dcx::compression(&bytes) != Some(compression) {
            continue;
        }
        std::fs::write(out.join(format!("{hash:016x}.dcx")), &bytes)?;
        written += 1;
    }
    println!(
        "{written} {} DCX files written to {}",
        String::from_utf8_lossy(&compression),
        out.display()
    );
    Ok(())
}
