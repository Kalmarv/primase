//! The corpus oracle set's `sekiro/lznib` (in GAME_DATA's `oracles`): every Kraken-enveloped stream in Sekiro's
//! archives whose first block is an LZB16 block (Oodle decoder type 2, block header `8C 02`), as `<hash>.payload` (the
//! stream as the DCX envelope stores it, from the block header on; `<hash>` the entry's path hash, hex) beside
//! `<hash>.decoded`, what the game's Oodle 6 library decodes it to. The streams are the game's own and the decoded
//! bytes the game library's, so `tests/lznib_gate.rs` checks oozextract against them without the library.
//!
//! `cargo run --release -p primase-fromsoft --example corpus_sekiro_lzb16 -- <Sekiro install> <keys> <oo2core_6 DLL>
//! <out>`, `<keys>` the folder of `Data1.pem` to `Data5.pem` (`keys.sekiro`), the DLL `tools.oodle6`, `<out>` being
//! `<oracles.root>/corpus/sekiro/lznib`.

use primase_foundations::compress::Decompressor;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_fromsoft::game::ParamKey;
use std::path::PathBuf;

use fromsoft_formats::dcx::{Compression, Envelope};
use fromsoft_formats::{Archive, Game, Keys};

type BoxError = Box<dyn std::error::Error>;

/// An Oodle block header naming decoder type 2 (LZB16).
const LZB16: [u8; 2] = [0x8C, 0x02];

fn main() -> Result<(), BoxError> {
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let [install, keys, library, out] = paths.as_slice() else {
        return Err("usage: corpus_sekiro_lzb16 <Sekiro install> <keys> <oo2core_6 DLL> <out>".into());
    };
    let game = Game::Sekiro;
    let archive = Archive::open_game(
        install,
        game,
        &Keys::from_folder(keys, game, game.param_key_sha256())?,
    )?;
    // SAFETY: the caller names the game's own version-6 library.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(library, Version::V6)? };
    std::fs::create_dir_all(out)?;
    let mut written = 0usize;
    for &hash in archive.entries.keys() {
        let stored = archive.read_stored(hash)?;
        let Ok(envelope) = Envelope::parse(&stored, usize::MAX) else {
            continue;
        };
        if envelope.compression != Compression::Kraken || !envelope.payload.starts_with(&LZB16) {
            continue;
        }
        let decoded = oodle.decompress(envelope.payload, envelope.decoded_size)?;
        std::fs::write(out.join(format!("{hash:016x}.payload")), envelope.payload)?;
        std::fs::write(out.join(format!("{hash:016x}.decoded")), decoded)?;
        written += 1;
    }
    println!("{written} LZB16 streams written to {}", out.display());
    Ok(())
}
