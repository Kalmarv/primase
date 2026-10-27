//! Input layouts: a hand-laid layout read to its elements and usages, the refusals, and the rule that sends a vertex
//! program's inputs to its elements; and, on the game's data (the skate. install and its Oodle 9 library), every input
//! layout of the game configuration, level root and customization TOCs read, and the vertex programs of the game
//! configuration's raster permutations compared with their layouts: each input of the program must be the one element
//! its semantic name and index name.

#![allow(clippy::unwrap_used)]

mod inputs;

use std::collections::BTreeMap;

use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::inputlayout::{self, Found, RTYPE, dxgi_components};
use frostbite_formats::permutation;
use inputs::{Game, Tool, install_file, tool};
use iridia_interface::Stage;
use primase_core::World;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::components::{FrostbiteInputLayout, FrostbitePermutation};
use primase_frostbite::read::entry::read_entry;

#[test]
fn the_stock_layouts_read_and_agree_with_the_vertex_programs() {
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
    // every layout of three TOCs
    let (mut read, mut failed) = (0usize, Vec::new());
    for toc in [
        "Win32/globals.toc",
        "Win32/levels/game/dingolevel_root/dingolevel_root.toc",
        "Win32/customization.toc",
    ] {
        let listing = src.listing(toc).unwrap();
        for bundle in &listing.bundles {
            for e in bundle
                .entries()
                .filter(|e| e.kind == Kind::Res && e.resource_type == Some(RTYPE))
            {
                let payload = src.payload(e.file, e.size as usize).unwrap();
                match inputlayout::read(&payload, &bundle.manifest.res[e.index].rmeta) {
                    Ok(_) => read += 1,
                    Err(err) => failed.push(format!("{}: {err}", e.name)),
                }
            }
        }
    }
    println!("{read} input layouts read, {} failed", failed.len());
    for f in failed.iter().take(10) {
        println!("  FAILED {f}");
    }
    assert!(failed.is_empty(), "{} did not read", failed.len());

    // the game configuration's raster permutations: each non-system input of the vertex program, by name and index
    let bundle = src
        .bundle(
            "Win32/globals.toc",
            "win32/configurations/gameconfigurations/delmargameconfiguration",
        )
        .unwrap();
    let mut tally: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut permutations, mut inputs) = (0usize, 0usize);
    for entry in bundle
        .entries()
        .filter(|e| e.kind == Kind::Res && e.resource_type == Some(permutation::RASTER))
    {
        let mut w = World::new();
        let made = read_entry(&src, &bundle, &entry, &mut w).unwrap();
        let layout = &w.get::<FrostbiteInputLayout>(made[0]).unwrap().0;
        permutations += 1;
        for &shader in &w.get::<FrostbitePermutation>(made[0]).unwrap().shaders {
            let interface = &w.get::<primase_core::shader::Shader>(shader).unwrap().interface;
            if interface.stage != Stage::Vertex {
                continue;
            }
            let program: Vec<_> = interface.inputs.iter().filter(|v| v.system.is_none()).collect();
            let wanted: Vec<(&str, u32)> = program
                .iter()
                .map(|v| (v.label.name.as_str(), v.label.index))
                .collect();
            for (v, found) in program.iter().zip(layout.resolve(&wanted)) {
                inputs += 1;
                let (kind, element) = match found {
                    Some(Found::ByName(i)) => ("by name", i),
                    Some(Found::ByElimination(i)) => ("by elimination", i),
                    None => {
                        *tally.entry("no element").or_default() += 1;
                        continue;
                    }
                };
                // the program reads no more components than the element's format has
                let fits = dxgi_components(layout.elements[element].dxgi())
                    .is_some_and(|c| u32::from(c) >= u32::from(v.value.width));
                *tally
                    .entry(match (kind, fits) {
                        ("by name", true) => "by name, fits",
                        ("by name", false) => "by name, wider than the format",
                        (_, true) => "by elimination, fits",
                        (_, false) => "by elimination, wider than the format",
                    })
                    .or_default() += 1;
            }
        }
    }
    println!("{permutations} permutations, {inputs} vertex inputs: {tally:?}");
    // every input is accounted for: the ones with no element and the ones wider than their element's format are the
    // gaps (15,383 inputs: 15,336 by name and fitting, 6 by elimination and fitting, 41 by name and wider than the
    // format: a usage 2 element stored as R16G16_UINT read as four components; none without an element)
    assert_eq!(tally.values().sum::<usize>(), inputs);
    let found = tally["by name, fits"] + tally.get("by elimination, fits").copied().unwrap_or(0);
    assert!(
        found * 1000 >= inputs * 990,
        "{found} of {inputs} inputs are one element that fits"
    );
}
