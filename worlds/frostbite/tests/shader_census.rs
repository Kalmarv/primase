//! skate.'s compiled shader programs (the CompiledBytecode resources of every superbundle TOC of its `Data`): each read
//! to its containers and its interface, each program of a stage the lift handles lifted onto the IR, validated and written as
//! WGSL, and the census printed (resources, containers, tags, stages, refusals by reason). A resource the bytecode
//! reader cannot read to its end fails the test; a lift refusal is counted, not a failure, and so is a program WGSL
//! cannot express (a texture atomic's old value: the writer's refusal is counted by reason), but a program that lifts
//! must validate. Needs the skate. install (`Data`) and the game's Oodle 9 library: unset skips.

#![allow(clippy::unwrap_used)]

mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use frostbite_formats::bundle::Kind;
use frostbite_formats::bytecode::{self as stored, RTYPE};
use frostbite_formats::folder::{Folder, Source};
use inputs::{Game, Tool, install_file, tool};
use iridia_interface::Stage;
use iridia_naga::{BindingPlan, Pipeline, Profile, wgsl_for};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::bytecode;

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
    let mut text_form: BTreeMap<String, usize> = BTreeMap::new();
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
                let spans = stored::containers(&payload).unwrap_or_else(|err| panic!("{}: {err}", e.name));
                containers += spans.len();
                let tag = stored::tag(&payload)
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
                    match primase_core::shader::lift_dxil(&shader.bytecode) {
                        Err(err) => *refused.entry(err.group()).or_default() += 1,
                        Ok(program) => {
                            lifted += 1;
                            match iridia_ir::validate(&program) {
                                Err(p) => invalid.push(format!("{}: {p:?}", e.name)),
                                Ok(_) => {
                                    // WGSL cannot write every program (a texture atomic's old value): the writer says why
                                    let written = BindingPlan::d3d(&program.interface).and_then(|plan| {
                                        let pipeline = Pipeline { plan, feeds: None };
                                        wgsl_for(&program, &Profile::NAGA, &pipeline).map_err(|e| e.0)
                                    });
                                    if let Err(why) = written {
                                        *text_form.entry(reason(&why)).or_default() += 1;
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
         {lifted} vertex, pixel and compute programs lifted ({} WGSL cannot write), {} refused, \
         {} resources whose interface is not read",
        text_form.values().sum::<usize>(),
        refused.values().sum::<usize>(),
        unread.values().sum::<usize>()
    );
    for (why, n) in &unread {
        println!("  interface not read {n:5}  {why}");
    }
    for (why, n) in &refused {
        println!("  lift refused {n:5}  {why}");
    }
    for (why, n) in &text_form {
        println!("  WGSL refused {n:5}  {why}");
    }
    for line in invalid.iter().take(20) {
        println!("  INVALID {line}");
    }
    assert!(resources > 15_000, "{resources} resources");
    assert!(
        invalid.is_empty(),
        "{} lifted programs failed validation",
        invalid.len()
    );
}

/// A reason without the numbers that differ between resources, so the census groups them.
fn reason(text: &str) -> String {
    let after = text.split(": ").last().unwrap_or(text);
    after.chars().filter(|c| !c.is_ascii_digit()).collect()
}
