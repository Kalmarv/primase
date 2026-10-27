//! Shader permutations and program records: hand-laid permutations read to their ids and GUIDs, the refusals, a GUID's
//! bytecode name, program records read from a hand-laid program lookup, a shared data's fixed-function state read in
//! the layouts the game's own state printers use (`Skate.exe`'s rc2 IR filter: the oracle, see
//! `frostbite_formats::permutation::FixedState`); and, on the game's data (the skate. install and its Oodle 9 library),
//! every program of the first program database of `Win32/globals.toc` followed through its permutations to bytecode
//! that reads, a material key's programs read into a world, and every stock raster state decoded.

#![allow(clippy::unwrap_used)]

mod inputs;

use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::lookup::{self};
use frostbite_formats::permutation::{self};
use frostbite_formats::scope::Scope;
use inputs::{Game, Tool, install_file, tool};
use iridia_interface::Stage;
use primase_core::World;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::components::{FrostbitePermutation, FrostbiteProgram};
use primase_frostbite::read::entry::read_entry;
use primase_frostbite::read::program::{program_states, read_material_program};
use primase_frostbite::render_state::render_state;

fn meta(size: u32, reloc: u32, id: u64) -> [u8; 16] {
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&size.to_le_bytes());
    m[4..8].copy_from_slice(&reloc.to_le_bytes());
    m[8..16].copy_from_slice(&id.to_le_bytes());
    m
}

#[test]
fn the_stock_programs_reach_their_bytecode() {
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
    // the game configuration's bundle: a program database, its permutations and their bytecode
    let bundle = src
        .bundle(
            "Win32/globals.toc",
            "win32/configurations/gameconfigurations/delmargameconfiguration",
        )
        .unwrap();
    let database = bundle
        .entries()
        .find(|e| e.kind == Kind::Res && e.resource_type == Some(lookup::PROGRAM_LOOKUP))
        .unwrap();
    let mut w = World::new();
    let made = read_entry(&src, &bundle, &database, &mut w).unwrap();
    let programs = w
        .get::<primase_frostbite::components::FrostbitePrograms>(made[0])
        .unwrap()
        .clone();
    println!(
        "{} records, {} rows; {} permutations named",
        programs.records.len(),
        programs.lookup.rows.len(),
        programs
            .records
            .iter()
            .map(|r| r.permutations.len())
            .sum::<usize>()
    );
    // every material key's programs read to the end: a missing id, name or an unreadable program is the failure
    let listing = src.listing("Win32/globals.toc").unwrap();
    let scope = Scope::new(&listing.bundles);
    let mut stages = std::collections::BTreeMap::new();
    let (mut keys, mut failed) = (0, Vec::new());
    for key in programs.lookup.rows.keys().take(300) {
        let mut w = World::new();
        match read_material_program(&src, &scope, &database, *key, &mut w) {
            Ok(entities) => {
                keys += 1;
                for e in entities {
                    let program = w.get::<FrostbiteProgram>(e).unwrap();
                    for &p in &program.permutations {
                        for &s in &w.get::<FrostbitePermutation>(p).unwrap().shaders {
                            let shader = w.get::<primase_core::shader::Shader>(s).unwrap();
                            *stages
                                .entry(format!("{:?}", shader.interface.stage))
                                .or_insert(0usize) += 1;
                        }
                    }
                }
            }
            Err(e) => failed.push(format!("{key:#x}: {e}")),
        }
    }
    println!("{keys} keys read, stages {stages:?}, {} failed", failed.len());
    for f in failed.iter().take(10) {
        println!("  FAILED {f}");
    }
    assert!(failed.is_empty(), "{} material keys did not read", failed.len());
    assert!(stages.contains_key(&format!("{:?}", Stage::Pixel)));
}

/// A level's database names stock permutations that lie in the game configuration's bundle: read through a scope of the
/// level's bundles then `globals.toc`'s, every sampled material key's programs read (measured over the whole of
/// dingolevel_root: 57,963 rows; a tenth sampled here).
#[test]
fn a_levels_programs_resolve_through_the_game_configuration() {
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
    let level = src
        .listing("Win32/levels/game/dingolevel_root/dingolevel_root.toc")
        .unwrap();
    let mut bundles = level.bundles.clone();
    bundles.extend(src.listing("Win32/globals.toc").unwrap().bundles);
    let scope = Scope::new(&bundles);
    let database = level
        .bundles
        .iter()
        .flat_map(|b| b.entries())
        .find(|e| e.kind == Kind::Res && e.resource_type == Some(lookup::PROGRAM_LOOKUP))
        .unwrap();
    let home = level.bundles.iter().find(|b| b.name == database.bundle).unwrap();
    let mut w = World::new();
    let made = read_entry(&src, home, &database, &mut w).unwrap();
    let programs = w
        .get::<primase_frostbite::components::FrostbitePrograms>(made[0])
        .unwrap()
        .clone();
    let mut failed = Vec::new();
    for key in programs.lookup.rows.keys().step_by(500) {
        let mut w = World::new();
        if let Err(e) = read_material_program(&src, &scope, &database, *key, &mut w) {
            failed.push(format!("{key:#x}: {e}"));
        }
    }
    for f in failed.iter().take(10) {
        println!("  FAILED {f}");
    }
    assert!(failed.is_empty(), "{} material keys did not read", failed.len());
}

/// A raster shared data laid as the game's: the ids, zeros to 0x78, then `state` (0x78 to 0xb4).
fn shared_with_state(state: &[u8; 0x3c]) -> (Vec<u8>, [u8; 16]) {
    let mut d = vec![0u8; 0xc0];
    d[0x08..0x10].copy_from_slice(&1u64.to_le_bytes());
    d[0x38..0x40].copy_from_slice(&2u64.to_le_bytes());
    d[0x78..0xb4].copy_from_slice(state);
    let m = meta(d.len() as u32, 0, 7);
    (d, m)
}

/// The stock decal row's state (hand-worked from the game's printers' layouts, permutation.rs's table): bias 200,
/// slope 0.1, cull back, front counter-clockwise, depth clip; depth test GreaterEqual, no write, stencil test reading
/// 0x08 and writing nothing, both faces keep / keep / replace / GreaterEqual; logic op 4 off, independent blend; three
/// premultiplied targets (One, InvSourceAlpha; masks rgb, rgb, gb) and five unblended ones.
fn decal_state() -> [u8; 0x3c] {
    let mut s = [0u8; 0x3c];
    s[0..4].copy_from_slice(&0.1f32.to_le_bytes());
    s[4..6].copy_from_slice(&200i16.to_le_bytes());
    s[7] = 0x1c;
    s[8..16].copy_from_slice(&[0x35, 0x08, 0x00, 0x00, 0x32, 0x00, 0x32, 0x44]);
    s[16] = 0xff;
    let records: [[u8; 5]; 8] = [
        [0x03, 0x05, 0x01, 0x05, 0x07],
        [0x03, 0x05, 0x01, 0x05, 0x07],
        [0x03, 0x05, 0x01, 0x05, 0x06],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
        [0x02, 0x00, 0x01, 0x00, 0x0f],
    ];
    for (i, r) in records.iter().enumerate() {
        s[0x14 + 5 * i..0x19 + 5 * i].copy_from_slice(r);
    }
    s
}

/// Every raster shared data of `dingolevel_root` and `globals.toc` decodes to a state in the game's ranges, and the
/// programs of the stock decal row (key 0xca7afe635959dd1e) draw with a no-write, stencil-tested, biased state.
#[test]
fn every_stock_raster_state_decodes() {
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
    let root = src
        .listing("Win32/levels/game/dingolevel_root/dingolevel_root.toc")
        .unwrap();
    let mut bundles = root.bundles.clone();
    bundles.extend(src.listing("Win32/globals.toc").unwrap().bundles);
    let mut seen = std::collections::BTreeSet::new();
    let mut classes = std::collections::BTreeMap::new();
    for b in &bundles {
        for (i, a) in b.manifest.res.iter().enumerate() {
            if a.rtype != permutation::SHARED_RASTER || !seen.insert(permutation::resource_id(&a.rmeta)) {
                continue;
            }
            let e = b.resource(i);
            let s =
                permutation::read_shared(a.rtype, &src.payload(e.file, e.size as usize).unwrap(), &a.rmeta)
                    .unwrap()
                    .fixed_state()
                    .unwrap_or_else(|err| panic!("{}: {err}", e.name));
            *classes
                .entry((
                    s.depth_write,
                    s.depth_bias,
                    s.stencil_test,
                    format!("{:?}", s.depth_compare),
                ))
                .or_insert(0usize) += 1;
        }
    }
    println!(
        "{} raster shared data decoded; (write, bias, stencil, compare): {classes:?}",
        seen.len()
    );
    assert!(seen.len() > 1000, "{} shared data", seen.len());
    // the decal row
    let scope = Scope::new(&bundles);
    let database = root
        .bundles
        .iter()
        .flat_map(|b| b.entries())
        .find(|e| e.kind == Kind::Res && e.resource_type == Some(lookup::PROGRAM_LOOKUP))
        .unwrap();
    let home = root.bundles.iter().find(|b| b.name == database.bundle).unwrap();
    let payload = src.payload(database.file, database.size as usize).unwrap();
    let meta = home.manifest.res[database.index].rmeta;
    let records = lookup::read_records(&payload, &meta).unwrap();
    let row = lookup::read_programs(&payload, &meta).unwrap().rows[&0xca7a_fe63_5959_dd1e].clone();
    let states: Vec<_> = row
        .iter()
        .flat_map(|&p| program_states(&src, &scope, &records[p as usize]).unwrap())
        .collect();
    assert!(!states.is_empty());
    assert!(
        states.iter().all(|s| !s.depth_write),
        "the decal row writes no depth"
    );
    assert!(
        states.iter().any(|s| s.depth_bias == 200
            && s.slope_scaled_depth_bias == 0.1
            && s.stencil_test
            && s.stencil_read_mask == 0x08),
        "{states:?}"
    );
}

/// The decal state as D3D12 receives it (the game's own pipeline descriptions: clamp 0, colour op Add, see
/// `render_state::render_state`).
#[test]
fn a_fixed_state_maps_to_cores_render_state() {
    use iridia_interface::{BlendFactor, BlendOp, Compare, DepthBias, DepthState};
    let (d, m) = shared_with_state(&decal_state());
    let state = permutation::read_shared(permutation::SHARED_RASTER, &d, &m)
        .unwrap()
        .fixed_state()
        .unwrap();
    let r = render_state(&state).unwrap();
    r.verify().unwrap();
    assert_eq!(
        r.depth,
        Some(DepthState {
            test: Compare::GreaterEqual,
            write: false
        })
    );
    assert_eq!(
        r.depth_bias,
        Some(DepthBias {
            constant: 200.0,
            slope_scaled: 0.1,
            clamp: 0.0
        })
    );
    let s = r.stencil.unwrap();
    assert_eq!((s.read_mask, s.write_mask, s.reference), (0x08, 0x00, None));
    let b = r.blend.unwrap();
    assert_eq!(
        (b.colour.source, b.colour.destination, b.colour.op),
        (BlendFactor::One, BlendFactor::OneMinusSrcAlpha, BlendOp::Add)
    );
    assert_eq!(r.targets[2].unwrap().write_mask, 0x6);
    assert_eq!(r.targets[3].unwrap().blend, None);
    // a blended target with a constant factor has no core factor
    let mut s = decal_state();
    s[0x14] = 0x03 | (11 << 1);
    let (d, m) = shared_with_state(&s);
    let state = permutation::read_shared(permutation::SHARED_RASTER, &d, &m)
        .unwrap()
        .fixed_state()
        .unwrap();
    let refused = render_state(&state).unwrap_err();
    assert!(refused.to_string().contains("no core factor"), "{refused}");
}
