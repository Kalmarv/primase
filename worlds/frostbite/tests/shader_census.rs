//! skate.'s compiled shader programs (the CompiledBytecode resources of every superbundle TOC of its `Data`): each read
//! to its containers and its interface, each program of a stage the lift handles lifted, validated and written as
//! WGSL, and the census printed (resources, containers, tags, stages, refusals by reason). A resource the bytecode
//! reader cannot read to its end fails the test; a lift refusal is counted, not a failure, but a program that lifts
//! must validate and write WGSL. Needs the skate. install (`Data`) and the game's Oodle 9 library: unset skips.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_compress::oodle::{Oodle, Version};
use primase_core::shader::Stage;
use primase_frostbite::bundle::Kind;
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::{bytecode, bytecode::RTYPE};
use primase_testdata::{Game, Tool, install_file, tool};

#[test]
fn every_compiled_program_of_the_install() {
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
    let (mut resources, mut containers) = (0usize, 0usize);
    let mut seen = BTreeSet::new();
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_stage: BTreeMap<Stage, usize> = BTreeMap::new();
    let mut unread: BTreeMap<String, usize> = BTreeMap::new();
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    let (mut lifted, mut invalid) = (0usize, Vec::new());
    for toc in &folder.tocs {
        let listing = src.listing(toc).unwrap();
        assert!(listing.unread.is_empty(), "{toc}: {:?}", listing.unread);
        for bundle in &listing.bundles {
            for e in bundle
                .entries()
                .filter(|e| e.kind == Kind::Res && e.resource_type == Some(RTYPE))
            {
                // the same program is in several TOCs' bundles: counted once
                if !seen.insert((e.name.clone(), e.resource_id)) {
                    continue;
                }
                resources += 1;
                let payload = src.payload(e.file, e.size as usize).unwrap();
                // the payload's containers must be found to the end whatever their interfaces say
                let spans = bytecode::containers(&payload).unwrap_or_else(|err| panic!("{}: {err}", e.name));
                containers += spans.len();
                let tag = bytecode::tag(&payload)
                    .map_or_else(String::new, |t| String::from_utf8_lossy(&t).into_owned());
                *tags.entry(tag).or_default() += 1;
                let programs = match bytecode::read(&payload) {
                    Ok(p) => p,
                    Err(err) => {
                        *unread.entry(reason(&err.to_string())).or_default() += 1;
                        continue;
                    }
                };
                for (shader, _) in &programs {
                    *per_stage.entry(shader.interface.stage).or_default() += 1;
                    if !matches!(
                        shader.interface.stage,
                        Stage::Vertex | Stage::Pixel | Stage::Compute
                    ) {
                        continue;
                    }
                    match primase_dxil::lift(&shader.bytecode) {
                        Err(err) => *refused.entry(primase_dxil::refusal_reason(&err)).or_default() += 1,
                        Ok(program) => {
                            lifted += 1;
                            match program.validate() {
                                Err(p) => invalid.push(format!("{}: {p:?}", e.name)),
                                Ok(_) => {
                                    if let Err(err) = primase_shader::emit::wgsl(&program) {
                                        invalid.push(format!("{}: {err}", e.name));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{resources} CompiledBytecode resources, {containers} containers, tags {tags:?}, per stage {per_stage:?}; \
         {lifted} vertex, pixel and compute programs lifted, {} refused, {} resources whose interface is not read",
        refused.values().sum::<usize>(),
        unread.values().sum::<usize>()
    );
    for (why, n) in &unread {
        println!("  interface not read {n:5}  {why}");
    }
    for (why, n) in &refused {
        println!("  lift refused {n:5}  {why}");
    }
    for line in invalid.iter().take(20) {
        println!("  INVALID {line}");
    }
    assert!(resources > 15_000, "{resources} resources");
    assert!(
        invalid.is_empty(),
        "{} lifted programs failed validation or writing",
        invalid.len()
    );
}

/// A reason without the numbers that differ between resources, so the census groups them.
fn reason(text: &str) -> String {
    let after = text.split(": ").last().unwrap_or(text);
    after.chars().filter(|c| !c.is_ascii_digit()).collect()
}
