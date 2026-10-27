//! The corpus oracle set's `skate/shaders` (toolkit/testdata's registry): the DXBC containers (DXIL programs) of the
//! CompiledBytecode resources of every superbundle TOC of skate.'s `Data` (a program in several TOCs' bundles written
//! once), each as `<resource name>#<n>.dxbc` with the name's slashes as underscores. The bytes are the game's own.
//!
//! `cargo run --release -p primase-reskate --example corpus_skate_shaders -- <skate. Data> <oo2core_9_win64.dll>
//! <out>`, `<out>` being `<oracles.root>/corpus/skate/shaders`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use frostbite_formats::bundle::Kind;
use frostbite_formats::bytecode;
use frostbite_formats::folder::{Folder, Source};
use primase_foundations::compress::oodle::{Oodle, Version};

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let [data, dll, out] = args.as_slice() else {
        return Err("usage: corpus_skate_shaders <skate. Data> <oo2core_9_win64.dll> <out>".into());
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
        for bundle in &src.listing(toc)?.bundles {
            for e in bundle
                .entries()
                .filter(|e| e.kind == Kind::Res && e.resource_type == Some(bytecode::RTYPE))
            {
                if !seen.insert((e.name.clone(), e.resource_id)) {
                    continue;
                }
                let payload = src.payload(e.file, e.size as usize)?;
                let stem = e.name.replace(['/', '\\'], "_");
                for (i, span) in bytecode::containers(&payload)?.iter().enumerate() {
                    let container = payload
                        .get(span.offset..span.offset + span.size)
                        .ok_or("a container past its payload")?;
                    std::fs::write(out.join(format!("{stem}#{i}.dxbc")), container)?;
                    written += 1;
                }
            }
        }
    }
    println!(
        "{written} containers of {} resources written to {}",
        seen.len(),
        out.display()
    );
    Ok(())
}
