//! The corpus oracle set's `sekiro/havok` (toolkit/testdata's registry): one area's high-resolution map collision
//! binder (`/map/<area>/h<area>.hkxbhd` and `.hkxbdt`) written loose, each member out of its DCX envelope under its own
//! name: the area's Havok 2016.2 tagfiles and the compendium they share. The bytes are the game's own;
//! toolkit/havok/tests/corpus.rs reads every body and compressed mesh of them.
//!
//! `cargo run --release -p primase-fromsoft --example corpus_sekiro_havok -- <Sekiro install> <oo2core_6_win64.dll>
//! <out> [--area m11_00_00_00]`: the install, the game's own Oodle library (in its folder), and
//! `<oracles.root>/corpus/sekiro/havok`. The default area is Ashina Castle.

use std::path::PathBuf;

use primase_compress::oodle::{Oodle, Version};
use primase_fromsoft::{Archive, Binder, Game, dcx::Decoder};

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let mut paths = Vec::new();
    let mut area = "m11_00_00_00".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--area" => area = args.next().ok_or("--area needs a value")?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}").into()),
            s => paths.push(PathBuf::from(s)),
        }
    }
    let [install, dll, out] = paths.as_slice() else {
        return Err(
            "usage: corpus_sekiro_havok <Sekiro install> <oo2core_6_win64.dll> <out> [--area m11_00_00_00]"
                .into(),
        );
    };
    let suffix = area
        .strip_prefix('m')
        .ok_or_else(|| format!("{area}: an area is named m<..>"))?;
    let archive = Archive::open(install)?;
    // SAFETY: the caller names the game's own Oodle library.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V6)? };
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    let base = format!("{}/h{suffix}", Game::Sekiro.map_folder(&area)?);
    let binder = Binder::read_split(
        &archive.read(&format!("{base}.hkxbhd"), &decoder)?,
        &archive.read(&format!("{base}.hkxbdt"), &decoder)?,
        &decoder,
    )?;
    std::fs::create_dir_all(out)?;
    let (mut tagfiles, mut compendia) = (0usize, 0usize);
    for file in &binder.files {
        let Some(name) = file.name.as_deref() else {
            return Err(format!("{base}: a member without a name").into());
        };
        let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let leaf = leaf.strip_suffix(".dcx").unwrap_or(leaf).to_ascii_lowercase();
        if leaf.ends_with(".hkx") {
            tagfiles += 1;
        } else if leaf.ends_with(".compendium") {
            compendia += 1;
        } else {
            return Err(format!("{base}: member {name} is neither a tagfile nor a compendium").into());
        }
        std::fs::write(out.join(&leaf), decoder.decode_if_dcx(&file.bytes)?)?;
    }
    println!(
        "{tagfiles} tagfiles and {compendia} compendium of {base} written to {}",
        out.display()
    );
    Ok(())
}
