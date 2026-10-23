//! The corpus oracle set's `elden/shaders` (toolkit/testdata's registry): the DXBC containers (DXIL programs) of Elden
//! Ring's shader binders, every `/shader/*.shaderbnd.dcx` the path dictionary names and the archives hold, each member
//! that is a container written as `<binder>/<member>`. The bytes are the game's own.
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
        let Some(path) = paths
            .iter()
            .find(|p| p.starts_with("/shader/") && p.ends_with(".shaderbnd.dcx"))
        else {
            continue;
        };
        if !archive.entries.contains_key(hash) {
            continue;
        }
        // the binder decoded from its DCX envelope
        let binder = Binder::read(&archive.read_hash(*hash, &decoder)?, &decoder)?;
        let stem = path
            .trim_start_matches("/shader/")
            .trim_end_matches(".shaderbnd.dcx");
        let folder = out.join(stem);
        binders += 1;
        for (i, member) in binder.files.iter().enumerate() {
            if !member.bytes.starts_with(b"DXBC") {
                continue;
            }
            let name = member
                .name
                .as_deref()
                .and_then(|n| n.rsplit(['\\', '/']).next())
                .map_or_else(|| format!("{i}.dxbc"), str::to_owned);
            std::fs::create_dir_all(&folder)?;
            std::fs::write(folder.join(name), &member.bytes)?;
            written += 1;
        }
    }
    println!(
        "{written} containers of {binders} shader binders written to {}",
        out.display()
    );
    Ok(())
}
