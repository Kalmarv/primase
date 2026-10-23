//! The corpus oracle set's `gta/shader-database.awc` (toolkit/testdata's registry): GTA V Enhanced's shader database
//! (`sga_win32_60_final.awc` of `common/shaders/fxdb`), decrypted and unpacked as the install stores it
//! ([`primase_gta::read::source::GtaSource::shader_database_bytes`]). The bytes are the game's own.
//!
//! `cargo run --release -p primase-gta --example corpus_gta_shader_database -- <GTA V Enhanced install> <keys folder>
//! <out>`: the install, its key material (`keys.gta`), and `<oracles.root>/corpus/gta`, which receives
//! `shader-database.awc`.

use std::path::PathBuf;

use primase_gta::read::source::GtaSource;

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let [install, keys, out] = args.as_slice() else {
        return Err("usage: corpus_gta_shader_database <GTA V Enhanced install> <keys folder> <out>".into());
    };
    let bytes = GtaSource::open(install, keys)?.shader_database_bytes()?;
    std::fs::create_dir_all(out)?;
    let file = out.join("shader-database.awc");
    std::fs::write(&file, &bytes)?;
    println!("{} bytes written to {}", bytes.len(), file.display());
    Ok(())
}
