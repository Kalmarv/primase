//! The corpus oracle set's `elden/shaders` (toolkit/testdata's registry): every DXBC container (DXIL program) of Elden
//! Ring's shader binders, each `/shader/*.shaderbnd.dcx` and `/shader/*.shaderbdlebnd.dcx` (a bundle of one binder per
//! material: `shaderbdle`, its DLC halves and `speedtree`) the path dictionary names and the archives hold, development
//! patches left out: a binder's container written as `<binder>/<member>`, a bundle's as
//! `<bundle>/<material>/<member>`. The bytes are the game's own.
//!
//! `cargo run --release -p primase-fromsoft --example corpus_elden_shaders -- <Elden Ring Game folder> <keys folder>
//! <dictionary.txt> <oo2core_6_win64.dll> <out>`: the install, the archive keys (`keys.elden`), UXM's path dictionary
//! (`oracles.uxm.elden/dictionary.txt`), the game's own Oodle library (in its Game folder), and
//! `<oracles.root>/corpus/elden/shaders`.

use std::path::PathBuf;

use primase_compress::oodle::{Oodle, Version};
use primase_fromsoft::{Archive, Binder, Game, Keys, NameDictionary, dcx::Decoder};

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let [install, keys, dictionary, dll, out] = args.as_slice() else {
        return Err(
            "usage: corpus_elden_shaders <Game folder> <keys folder> <dictionary.txt> <oo2core_6_win64.dll> <out>"
                .into(),
        );
    };
    let keys = Keys::from_folder(keys, Game::EldenRing)?;
    let archive = Archive::open_game(install, Game::EldenRing, &keys)?;
    let names = NameDictionary::read_for(dictionary, Game::EldenRing)?;
    // SAFETY: the caller names the game's own Oodle library.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V6)? };
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    let (mut binders, mut written) = (0usize, 0usize);
    for (hash, paths) in &names.names {
        // a shader binder, or a bundle of them (one per material), not a development patch
        let Some((path, stem)) = paths.iter().find_map(|p| {
            let name = p.strip_prefix("/shader/")?;
            let stem = name
                .strip_suffix(".shaderbnd.dcx")
                .or_else(|| name.strip_suffix(".shaderbdlebnd.dcx"))?;
            (!stem.contains(".devpatch")).then_some((p, stem))
        }) else {
            continue;
        };
        if !archive.entries.contains_key(hash) {
            continue;
        }
        // the binder decoded from its DCX envelope
        let binder = Binder::read(&archive.read_hash(*hash, &decoder)?, &decoder)
            .map_err(|e| format!("{path}: {e}"))?;
        binders += 1;
        written += write_containers(&binder, &out.join(stem), &decoder, 0)?;
    }
    println!(
        "{written} containers of {binders} shader binders written to {}",
        out.display()
    );
    Ok(())
}

/// Every container of `binder` written into `folder` by its member's name, a member that is itself a binder (a
/// bundle's material, DCX-wrapped or not) into a folder named after it; the count written.
fn write_containers(
    binder: &Binder,
    folder: &std::path::Path,
    decoder: &Decoder<'_>,
    depth: usize,
) -> Result<usize, BoxError> {
    if depth > 4 {
        return Err(format!("{}: binders nested too deep", folder.display()).into());
    }
    let mut written = 0;
    for (i, member) in binder.files.iter().enumerate() {
        let name = member
            .name
            .as_deref()
            .and_then(|n| n.rsplit(['\\', '/']).next())
            .map_or_else(|| format!("{i}"), str::to_owned);
        let bytes = decoder.decode_if_dcx(&member.bytes)?;
        if bytes.starts_with(b"BND4") {
            // a material's binder: its folder is its name without the extensions
            let stem = name.split('.').next().unwrap_or(&name);
            let inner = Binder::read(&bytes, decoder)?;
            written += write_containers(&inner, &folder.join(stem), decoder, depth + 1)?;
        } else if bytes.starts_with(b"DXBC") {
            std::fs::create_dir_all(folder)?;
            std::fs::write(folder.join(&name), &bytes)?;
            written += 1;
        }
    }
    Ok(written)
}
