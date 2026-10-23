//! Shader permutations and program records: hand-laid permutations read to their ids and GUIDs, the refusals, a GUID's
//! bytecode name, program records read from a hand-laid program lookup; and, on the game's data (the skate. install
//! and its Oodle 9 library), every program of the first program database of `Win32/globals.toc` followed through its
//! permutations to bytecode that reads, and a material key's programs read into a world.

#![allow(clippy::unwrap_used)]

use primase_compress::oodle::{Oodle, Version};
use primase_core::World;
use primase_core::shader::Stage;
use primase_frostbite::bundle::Kind;
use primase_frostbite::components::{FrostbitePermutation, FrostbiteProgram};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::lookup::{self, RECORD_LEN};
use primase_frostbite::permutation::{self, COMPUTE, RASTER};
use primase_frostbite::program::Scope;
use primase_testdata::{Game, Tool, install_file, tool};

/// Metadata: the payload size, the relocation size, the resource's id.
fn meta(size: u32, reloc: u32, id: u64) -> [u8; 16] {
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&size.to_le_bytes());
    m[4..8].copy_from_slice(&reloc.to_le_bytes());
    m[8..16].copy_from_slice(&id.to_le_bytes());
    m
}

const G0: [u8; 16] = [
    0xa5, 0xfa, 0x7a, 0x26, 0x0a, 0x1f, 0xf7, 0x3d, 0x33, 0x84, 0xf6, 0x6a, 0x01, 0x00, 0x28, 0x04,
];
const G1: [u8; 16] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
];

/// A raster permutation with the given GUIDs, laid as the game's are.
fn raster(guids: &[[u8; 16]]) -> (Vec<u8>, [u8; 16]) {
    let mut d = Vec::new();
    d.extend(0x1111_2222_3333_4444u64.to_le_bytes());
    d.extend(0x20u64.to_le_bytes());
    d.extend(0x5555_6666_7777_8888u64.to_le_bytes());
    d.extend(0u64.to_le_bytes());
    for g in guids {
        d.extend(g);
    }
    let size = d.len() as u32;
    d.extend(8u32.to_le_bytes());
    (d, meta(size, 4, 0x99))
}

#[test]
fn a_raster_permutation_reads_to_its_ids_and_programs() {
    let (d, m) = raster(&[G0, G1]);
    let p = permutation::read(RASTER, &d, &m).unwrap();
    assert_eq!(p.kind, permutation::Kind::Raster);
    assert_eq!((p.id, p.shared_data), (0x99, 0x1111_2222_3333_4444));
    assert_eq!(p.input_layout, Some(0x5555_6666_7777_8888));
    assert_eq!(p.bytecode, [G0, G1]);
    // a vertex program alone
    let (d, m) = raster(&[G0]);
    assert_eq!(permutation::read(RASTER, &d, &m).unwrap().bytecode, [G0]);
}

#[test]
fn a_compute_permutation_reads_to_its_program() {
    let mut d = Vec::new();
    d.extend(0xAAAAu64.to_le_bytes());
    d.extend(0x10u64.to_le_bytes());
    d.extend(G1);
    d.extend(8u32.to_le_bytes());
    let p = permutation::read(COMPUTE, &d, &meta(0x20, 4, 7)).unwrap();
    assert_eq!(
        (p.kind, p.id, p.shared_data, p.input_layout),
        (permutation::Kind::Compute, 7, 0xAAAA, None)
    );
    assert_eq!(p.bytecode, [G1]);
}

#[test]
fn a_malformed_permutation_is_refused() {
    let (d, m) = raster(&[G0, G1]);
    let refused = |d: &[u8], m: &[u8; 16], t: u32| permutation::read(t, d, m).unwrap_err().to_string();
    // another type, a metadata that does not match the payload
    assert!(refused(&d, &m, 0x1234).contains("not a permutation"));
    assert!(refused(&d[..d.len() - 1], &m, RASTER).contains("relocations"));
    // a relocation that is not the pointer at 8
    let mut bad = d.clone();
    let n = bad.len();
    bad[n - 4] = 16;
    assert!(refused(&bad, &m, RASTER).contains("pointer at 8"));
    // a pointer that is not where the GUIDs start
    let mut bad = d.clone();
    bad[8] = 0x30;
    assert!(refused(&bad, &m, RASTER).contains("GUIDs"));
    // three programs
    let (d3, m3) = raster(&[G0, G1, G1]);
    assert!(refused(&d3, &m3, RASTER).contains("3 programs"));
    // a compute permutation of the raster layout's size
    assert!(refused(&d, &m, COMPUTE).contains("GUIDs"));
}

/// A GUID's bytecode name: its first three fields are little endian as stored, the rest in order; the example is a
/// CompiledBytecode resource of the install and the bytes its permutation holds.
#[test]
fn a_guid_names_its_bytecode_resource() {
    assert_eq!(
        permutation::bytecode_name(&G0),
        "shaders/bytecode/267afaa5-1f0a-3df7-3384-f66a01002804"
    );
    assert_eq!(
        permutation::bytecode_name(&G1),
        "shaders/bytecode/04030201-0605-0807-090a-0b0c0d0e0f10"
    );
}

/// Program records read from a hand-laid program section: the count and the offset of the first at the start, each
/// record's key, and its permutation ids by the count at 0x60 and the pointer at 0x64.
#[test]
fn program_records_name_their_permutations() {
    let (count, first) = (2usize, 0x20usize);
    let ids_at = first + count * RECORD_LEN;
    let mut d = vec![0u8; ids_at + 24];
    d[0..4].copy_from_slice(&(count as u32).to_le_bytes());
    d[4..12].copy_from_slice(&(first as u64).to_le_bytes());
    for (i, (key, ids)) in [(0xAAu64, vec![5u64, 6]), (0xBB, vec![7])]
        .into_iter()
        .enumerate()
    {
        let at = first + i * RECORD_LEN;
        d[at..at + 8].copy_from_slice(&key.to_le_bytes());
        let ptr = ids_at + if i == 0 { 0 } else { 16 };
        d[at + 0x60..at + 0x64].copy_from_slice(&(ids.len() as u32).to_le_bytes());
        d[at + 0x64..at + 0x6c].copy_from_slice(&(ptr as u64).to_le_bytes());
        for (j, id) in ids.iter().enumerate() {
            d[ptr + 8 * j..ptr + 8 * j + 8].copy_from_slice(&id.to_le_bytes());
        }
    }
    let program = d.len();
    // the lookup rows after the program section and an empty relocation list: none (count 0)
    d.extend(0u32.to_le_bytes());
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&(program as u32).to_le_bytes());
    m[8..12].copy_from_slice(&4u32.to_le_bytes());
    let records = lookup::read_records(&d, &m).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        (records[0].key, &records[0].permutations[..]),
        (0xAA, &[5u64, 6][..])
    );
    assert_eq!(
        (records[1].key, &records[1].permutations[..]),
        (0xBB, &[7u64][..])
    );
    assert_eq!(records[1].raw[0], 0xBB);
    // a permutation array past the section, and records past it
    let mut bad = d.clone();
    bad[first + 0x64] = 0xf0;
    bad[first + 0x65] = 0xff;
    assert!(lookup::read_records(&bad, &m).is_err());
    bad = d.clone();
    bad[0] = 9;
    assert!(lookup::read_records(&bad, &m).is_err());
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
    let made = src.read_entry(&bundle, &database, &mut w).unwrap();
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
        match src.read_material_program(&scope, &database, *key, &mut w) {
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
    let made = src.read_entry(home, &database, &mut w).unwrap();
    let programs = w
        .get::<primase_frostbite::components::FrostbitePrograms>(made[0])
        .unwrap()
        .clone();
    let mut failed = Vec::new();
    for key in programs.lookup.rows.keys().step_by(500) {
        let mut w = World::new();
        if let Err(e) = src.read_material_program(&scope, &database, *key, &mut w) {
            failed.push(format!("{key:#x}: {e}"));
        }
    }
    for f in failed.iter().take(10) {
        println!("  FAILED {f}");
    }
    assert!(failed.is_empty(), "{} material keys did not read", failed.len());
}
