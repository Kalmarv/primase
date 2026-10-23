//! `Interface::verify` on the interface of every compiled program of the skate. install (the CompiledBytecode
//! resources of every superbundle TOC, read as `shader_census` reads them; needs the install and the game's Oodle 9
//! library: unset skips). The rules that read a program's resources (a D3D register class that fits the kind, one
//! resource to a register, a structured stride of a multiple of 4 up to 2048, colour outputs of a pixel stage numbered
//! 0 to 7, a tbuffer's layout inside its size, a hidden counter that belongs to a storage resource) are asked of
//! the interfaces the DXBC and DXIL readers produce. The census is printed: the programs read, those whose interface
//! fails to verify, by the rule's message with its numbers cut out and one example (resource name, TOC). A failure is
//! data to classify (the reader is wrong, or the rule misreads D3D), not a failure of the test.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_compress::oodle::{Oodle, Version};
use primase_frostbite::bundle::Kind;
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::{bytecode, bytecode::RTYPE};
use primase_testdata::{Game, Tool, install_file, tool};

#[test]
fn every_compiled_programs_interface_verifies_or_is_counted_by_rule() {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return;
    };
    let folder = Folder::open(data).unwrap();
    // SAFETY: the configured Oodle 9 library is the game's own.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }.unwrap();
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let mut seen = BTreeSet::new();
    let (mut programs, mut unread) = (0usize, 0usize);
    let mut failed: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for toc in &folder.tocs {
        let listing = src.listing(toc).unwrap();
        for bundle in &listing.bundles {
            for e in bundle
                .entries()
                .filter(|e| e.kind == Kind::Res && e.resource_type == Some(RTYPE))
            {
                // the same program is in several TOCs' bundles: counted once
                if !seen.insert((e.name.clone(), e.resource_id)) {
                    continue;
                }
                let payload = src.payload(e.file, e.size as usize).unwrap();
                let Ok(read) = bytecode::read(&payload) else {
                    unread += 1;
                    continue;
                };
                for (shader, _) in &read {
                    programs += 1;
                    if let Err(why) = shader.interface.verify() {
                        let rule: String = why
                            .split_whitespace()
                            .filter(|w| !w.chars().any(|c| c.is_ascii_digit()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let slot = failed
                            .entry(format!("{:?}: {rule}", shader.interface.stage))
                            .or_insert_with(|| (0, format!("{} ({toc}): {why}", e.name)));
                        slot.0 += 1;
                    }
                }
            }
        }
    }
    println!(
        "{programs} programs read ({unread} resources unread), {} fail to verify",
        failed.values().map(|f| f.0).sum::<usize>()
    );
    for (rule, (n, example)) in &failed {
        println!("FAILED x{n}: {rule}\n    example: {example}");
    }
    assert!(programs > 0);
}
