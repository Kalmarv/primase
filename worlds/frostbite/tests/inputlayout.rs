//! Input layouts: a hand-laid layout read to its elements and usages, the refusals, and the rule that sends a vertex
//! program's inputs to its elements; and, on the game's data (the skate. install and its Oodle 9 library), every input
//! layout of the game configuration, level root and customization TOCs read, and the vertex programs of the game
//! configuration's raster permutations compared with their layouts: each input of the program must be the one element
//! its semantic name and index name.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_compress::oodle::{Oodle, Version};
use primase_core::World;
use primase_core::shader::Stage;
use primase_frostbite::bundle::Kind;
use primase_frostbite::components::{FrostbiteInputLayout, FrostbitePermutation};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::inputlayout::{self, Found, RTYPE, dxgi_components, semantic_name};
use primase_frostbite::permutation;
use primase_testdata::{Game, Tool, install_file, tool};

/// A layout of the elements `(first, last, offset, dxgi, usage)`, laid as the game's are, and its metadata.
fn layout(id: u64, elements: &[(u32, u32, u32, u8, u8)]) -> (Vec<u8>, [u8; 16]) {
    let n = elements.len();
    let usages_at = 0x20 + (20 * n).next_multiple_of(16);
    let size = usages_at + n.next_multiple_of(16);
    let mut d = vec![0u8; size];
    d[0..4].copy_from_slice(&(n as u32).to_le_bytes());
    d[4..12].copy_from_slice(&0x20u64.to_le_bytes());
    d[12..20].copy_from_slice(&(usages_at as u64).to_le_bytes());
    for (i, &(first, last, offset, dxgi, usage)) in elements.iter().enumerate() {
        let at = 0x20 + 20 * i;
        d[at..at + 4].copy_from_slice(&first.to_le_bytes());
        d[at + 4..at + 8].copy_from_slice(&last.to_le_bytes());
        d[at + 8..at + 12].copy_from_slice(&offset.to_le_bytes());
        d[at + 16..at + 20].copy_from_slice(&(u32::from(dxgi) << 8 | 6).to_le_bytes());
        d[usages_at + i] = usage;
    }
    d.extend(4u32.to_le_bytes());
    d.extend(0xcu32.to_le_bytes());
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&(size as u32).to_le_bytes());
    m[4..8].copy_from_slice(&8u32.to_le_bytes());
    m[8..16].copy_from_slice(&id.to_le_bytes());
    (d, m)
}

#[test]
fn a_layout_reads_to_its_elements_and_usages() {
    // position in slot 0; two texture coordinates and a tangent space in slots of their own
    let (d, m) = layout(
        77,
        &[
            (0, 0, 0, 6, 1),
            (2, 3, 0, 34, 33),
            (3, 4, 0, 34, 34),
            (4, 5, 0, 28, 52),
        ],
    );
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(l.id, 77);
    assert_eq!(l.usages, [1, 33, 34, 52]);
    assert_eq!(
        l.elements
            .iter()
            .map(|e| (e.first, e.last, e.dxgi()))
            .collect::<Vec<_>>(),
        [(0, 0, 6), (2, 3, 34), (3, 4, 34), (4, 5, 28)]
    );
    // the inputs of a program: POSITION0 is usage 1; TEXCOORD2 and TEXCOORD3 are the usages 33 and 34 by their indices
    assert_eq!(
        l.resolve(&[
            ("POSITION", 0),
            ("TEXCOORD", 2),
            ("TEXCOORD", 3),
            ("TEXCOORD", 0),
            ("NORMAL", 0)
        ]),
        [
            Some(Found::ByName(0)),
            Some(Found::ByName(1)),
            Some(Found::ByName(2)),
            None,
            None
        ]
    );
}

#[test]
fn an_input_finds_its_element_by_name_then_by_elimination() {
    assert_eq!(
        [1, 6, 151, 30, 33, 52].map(semantic_name),
        ["POSITION", "NORMAL", "COLOR", "TEXCOORD", "TEXCOORD", "TEXCOORD"]
    );
    // a normal and a texture coordinate may both be index 0, each found by its name
    let (d, m) = layout(1, &[(0, 0, 0, 6, 1), (0, 0, 12, 10, 6), (0, 0, 20, 34, 33)]);
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(
        l.resolve(&[("NORMAL", 0), ("TEXCOORD", 0)]),
        [Some(Found::ByName(1)), Some(Found::ByName(2))]
    );
    // a program that reads a position as TEXCOORD0 and usage 30 as a TEXCOORD: the one element left of the index
    let (d, m) = layout(2, &[(0, 0, 0, 6, 1), (2, 3, 0, 28, 30)]);
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(
        l.resolve(&[("TEXCOORD", 0), ("TEXCOORD", 2)]),
        [Some(Found::ByElimination(0)), Some(Found::ByName(1))]
    );
    // two inputs of one index and two elements that name neither: not told apart; and an element taken twice is not
    let (d, m) = layout(3, &[(1, 0, 0, 34, 7), (1, 0, 4, 34, 8), (2, 0, 8, 34, 9)]);
    let l = inputlayout::read(&d, &m).unwrap();
    assert_eq!(l.resolve(&[("COLOR", 1), ("NORMAL", 1)]), [None, None]);
    assert_eq!(
        l.resolve(&[("COLOR", 2), ("NORMAL", 2)]),
        [Some(Found::ByElimination(2)), None]
    );
}

#[test]
fn a_malformed_layout_is_refused() {
    let (d, m) = layout(1, &[(0, 0, 0, 6, 1), (2, 3, 0, 34, 33)]);
    assert!(inputlayout::read(&d, &m).is_ok());
    // a size that is not the metadata's
    let mut short = m;
    short[0..4].copy_from_slice(&((d.len() - 8 - 16) as u32).to_le_bytes());
    assert!(inputlayout::read(&d, &short).is_err());
    // an element count the payload cannot hold, and none
    for n in [0u32, 3, 33] {
        let mut bad = d.clone();
        bad[0..4].copy_from_slice(&n.to_le_bytes());
        assert!(inputlayout::read(&bad, &m).is_err(), "{n} elements");
    }
    // the pointers and the relocations are the ones every layout has
    let mut bad = d.clone();
    bad[4] = 0x24;
    assert!(inputlayout::read(&bad, &m).is_err());
    let mut bad = d.clone();
    let n = bad.len();
    bad[n - 8] = 8;
    assert!(inputlayout::read(&bad, &m).is_err());
}

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
        let made = src.read_entry(&bundle, &entry, &mut w).unwrap();
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
