//! A read-only dump of skate.'s program records and their materials, for decoding which program of a row is a
//! material's main pass and which permutation of a program its options pick (the selector). Needs the skate. install
//! and its Oodle 9 library: unset skips. It prints lines of JSON a script reads, and checks only that the arrays it
//! prints lie inside the program section.
//!
//! - `REC`: every 40th record of every program database in the game configuration's and the level root's bundles: the
//!   168-byte record as stored, and for each of its twelve `(count, pointer)` items (from 0x18, 12 bytes each) the
//!   section bytes from 8 before the pointer to `count * 16` after it (cut at 4096).
//! - `MAT`: the materials of every twentieth `Win32/items.toc` bundle that holds a MeshSet (the census' sample): the
//!   depot block's parameters (name, type, count, value bytes), the programs of its row (index, record key, kind
//!   word, and for each permutation its id, input layout id, the layout's usages, the vertex stage's output count and
//!   the pixel stage's), and the usages of the stored and the draw declarations of the sections that use the key.
//!   The records of those programs are printed as `REC` too (flagged `"row": true`).
//!
//! The prediction written before the run (hypothesis H: a material's option values walk a record's B and D arrays to
//! one permutation of its E array):
//! 1. B's triples `(start, count, k)` tile D from 0 with no gap, so the counts sum to D's count; `k <= count`; D's
//!    values with bit 15 set are `0x8000 + i` for each permutation index `i < E.count` exactly once, so D's flagged
//!    values are E indices; the unflagged ones are indices into something of fewer than E.count entries. Any record
//!    where the tiling or the flagged set fails falsifies this reading of the arrays.
//! 2. A names options (its 16-byte items are `{u64 name, u32 type, u16 count, u16 extra}`): the depot block of a
//!    material whose program has options names every option of A of that program's record (same low 48 bits), so a
//!    value is available for each. If options are missing from the block for many materials, H's walk cannot be
//!    driven by the material alone.
//! 3. The permutation H lands on has the input layout of the material's mesh: its layout's usages are all in the draw
//!    declaration of the sections that use the key. The counts of materials walking to none, to several, and to a
//!    layout mismatch are read from the dump by script, after the run.
//! 4. For which program of a row is the main pass: programs that write more pixel targets (5 against 1) are the
//!    G-buffer pass and the 1-target ones depth or shadow passes with an alpha test; the record has a word that tells
//!    them apart (its kind word 2 is 1 for every raster record seen, so it is not that word).
//!
//! What the one run showed (129 sampled records + 9 records of the census materials' programs, 64 materials; read by
//! script from the printed lines):
//! 1. Held. In all 130 records that have B and D (8 have neither), B's triples are `(start, count, k)` after one
//!    leading `u16` the pointer includes, they tile D from 0 to its count, D's flagged values are exactly
//!    `0x8000 + i` for `i < E.count` once each, and C is a `u64` per D entry (its count equals D's, the entries are
//!    8 bytes, not 16). The unflagged D values are indices of other groups: every group but the first is named by
//!    exactly one entry (a tree, group 0 the root). So a record is a decision tree: a group is a node, an entry is an
//!    edge (the `u64` value hash in C, then the child group or, flagged, the permutation index in E). Six records
//!    have groups that mix child and leaf entries.
//! 2. Falsified for the material as the driver: 43 of the 1345 options in A (of the 9 records of the census
//!    materials' programs) are named in the material's depot block. The options are the engine's, not the
//!    material's. `k` is not the index of the option a node tests: the value set of a node is not determined by
//!    `A[k]`'s type (type `e4b00182` has 18 distinct sets), so what `k` is and which option a node tests are not
//!    decoded.
//! 3. Not computed: no walk is decoded, so no material walks to none, one or several permutations.
//! 4. Not settled: only 5 of the census materials' programs that draw pixels are among the 9 records, so no record
//!    word can be shown to tell the pass apart (every word that "separated" them is a pointer or a hash, unique per
//!    record). Rows read `P1`, `P5` and vertex-only programs in any order.

#![allow(clippy::unwrap_used)]

use std::fmt::Write as _;

use primase_core::shader::Shader;
use primase_core::{Entity, World};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::MESHSET;
use primase_frostbite::bundle::Kind;
use primase_frostbite::components::{
    FrostbiteInputLayout, FrostbiteMaterial, FrostbiteMeshSet, FrostbitePermutation, FrostbiteProgram,
};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::lookup::{self, RECORD_LEN};
use primase_frostbite::program::Scope;
use primase_testdata::{Game, Tool, install_file, tool};

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        write!(s, "{x:02x}").unwrap();
        s
    })
}

fn u32le(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

fn u64le(b: &[u8], at: usize) -> usize {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap()) as usize
}

/// One `REC` line: the record as stored and the bytes around each item's array.
fn record_line(db: &str, index: usize, raw: &[u8; RECORD_LEN], section: &[u8], row: bool) -> String {
    let mut items = String::new();
    for j in 0..12 {
        let (count, ptr) = (u32le(raw, 0x18 + 12 * j), u64le(raw, 0x1c + 12 * j));
        let from = ptr.saturating_sub(8).min(section.len());
        let to = (ptr + (count * 16).min(4096)).min(section.len());
        let inside = count == 0 || ptr + count <= section.len();
        write!(
            items,
            "{}{{\"j\":{j},\"count\":{count},\"ptr\":{ptr},\"inside\":{inside},\"bytes\":\"{}\"}}",
            if j == 0 { "" } else { "," },
            hex(&section[from..to])
        )
        .unwrap();
    }
    format!(
        "REC {{\"db\":\"{db}\",\"index\":{index},\"row\":{row},\"raw\":\"{}\",\"items\":[{items}]}}",
        hex(raw)
    )
}

#[test]
fn dump_program_records_and_their_materials() {
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
    let items = src.listing("Win32/items.toc").unwrap();
    let globals = src.listing("Win32/globals.toc").unwrap();
    let root = src
        .listing("Win32/levels/game/dingolevel_root/dingolevel_root.toc")
        .unwrap();

    // every program database of the configuration's and the level root's bundles: its records and its payload
    struct Db {
        name: String,
        records: Vec<lookup::ProgramRecord>,
        section: Vec<u8>,
    }
    let mut dbs: Vec<Db> = Vec::new();
    for bundle in globals.bundles.iter().chain(root.bundles.iter()) {
        for (i, a) in bundle.manifest.res.iter().enumerate() {
            if a.rtype != lookup::PROGRAM_LOOKUP {
                continue;
            }
            let entry = bundle.resource(i);
            let payload = src.payload(entry.file, entry.size as usize).unwrap();
            let records = lookup::read_records(&payload, &a.rmeta).unwrap();
            let program = u32le(&a.rmeta, 0);
            dbs.push(Db {
                name: entry.name.clone(),
                records,
                section: payload[..program].to_vec(),
            });
        }
    }
    println!(
        "DBS {:?}",
        dbs.iter().map(|d| (&d.name, d.records.len())).collect::<Vec<_>>()
    );
    let mut sampled = 0usize;
    for db in &dbs {
        for (i, r) in db.records.iter().enumerate().step_by(40) {
            println!("{}", record_line(&db.name, i, &r.raw, &db.section, false));
            sampled += 1;
        }
    }

    let mut printed_rows = std::collections::BTreeSet::new();
    let mut materials = 0usize;
    for bundle in items
        .bundles
        .iter()
        .filter(|b| b.manifest.res.iter().any(|a| a.rtype == MESHSET))
        .step_by(20)
    {
        let mut scope_bundles = vec![bundle.clone()];
        scope_bundles.extend(globals.bundles.iter().cloned());
        scope_bundles.extend(root.bundles.iter().cloned());
        let scope = Scope::new(&scope_bundles);
        let all = src.materials(&scope).unwrap();
        let mut w = World::new();
        let mut sets = Vec::new();
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
        {
            let made = src.read_entry(bundle, &entry, &mut w).unwrap();
            src.resolve_materials(&scope, &all, made[0], &mut w).unwrap();
            sets.push(made[0]);
        }
        let usages = |d: &primase_frostbite::meshset::Decl| {
            let mut u: Vec<u8> = d.elements.iter().map(|e| e.usage).collect();
            u.sort_unstable();
            u.dedup();
            u
        };
        let ms: Vec<(Entity, FrostbiteMaterial)> = w
            .iter::<FrostbiteMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        for (_, m) in ms {
            materials += 1;
            // the sections of this bundle's meshsets that use the key
            let mut sections = Vec::new();
            for &s in &sets {
                let set = w.get::<FrostbiteMeshSet>(s).unwrap();
                for lod in &set.meshset.lods {
                    for sec in lod.sections.iter().filter(|x| x.key == m.key) {
                        sections.push(format!(
                            "{{\"stored\":{:?},\"draw\":{:?}}}",
                            usages(&sec.decl),
                            usages(&sec.draw_decl)
                        ));
                    }
                }
            }
            sections.sort();
            sections.dedup();
            let params: Vec<String> = m
                .depot
                .iter()
                .flat_map(|d| {
                    d.parameters.iter().map(|p| {
                        let v = &d.block.data[p.at..p.at + p.len];
                        format!(
                            "{{\"name\":\"{:016x}\",\"ty\":\"{:08x}\",\"count\":{},\"extra\":{},\"bytes\":\"{}\"}}",
                            p.name,
                            p.ty,
                            p.count,
                            p.extra,
                            hex(&v[..v.len().min(84)])
                        )
                    })
                })
                .collect();
            let mut programs = Vec::new();
            for &p in &m.programs {
                let program = w.get::<FrostbiteProgram>(p).unwrap();
                let kind = u32le(&program.record, 8);
                let mut perms = Vec::new();
                for &e in &program.permutations {
                    let f = w.get::<FrostbitePermutation>(e).unwrap();
                    let layout = w.get::<FrostbiteInputLayout>(e).map(|l| {
                        let mut u = l.0.usages.clone();
                        u.sort_unstable();
                        u.dedup();
                        u
                    });
                    let outs = |i: usize| {
                        f.shaders
                            .get(i)
                            .and_then(|&s| w.get::<Shader>(s))
                            .map(|s| s.interface.outputs.len())
                    };
                    perms.push(format!(
                        "{{\"id\":\"{:016x}\",\"layout\":{},\"layout_usages\":{:?},\"vs_out\":{:?},\"ps_out\":{:?}}}",
                        f.permutation.id,
                        f.permutation.input_layout.map_or("null".to_owned(), |l| format!("\"{l:016x}\"")),
                        layout,
                        outs(0),
                        if f.permutation.bytecode.len() == 2 { outs(1) } else { None }
                    ));
                }
                programs.push(format!(
                    "{{\"index\":{},\"record_key\":\"{:016x}\",\"kind\":{kind},\"perms\":[{}]}}",
                    program.index,
                    program.record_key,
                    perms.join(",")
                ));
                // the record itself, once, from whichever database holds it
                if printed_rows.insert(program.record_key)
                    && let Some((db, i)) = dbs.iter().find_map(|d| {
                        d.records
                            .get(program.index as usize)
                            .filter(|r| r.key == program.record_key)
                            .map(|_| (d, program.index as usize))
                    })
                {
                    println!(
                        "{}",
                        record_line(&db.name, i, &db.records[i].raw, &db.section, true)
                    );
                }
            }
            println!(
                "MAT {{\"bundle\":\"{}\",\"key\":\"{:016x}\",\"params\":[{}],\"programs\":[{}],\"sections\":[{}]}}",
                bundle.name,
                m.key,
                params.join(","),
                programs.join(","),
                sections.join(",")
            );
        }
    }
    println!(
        "DONE {sampled} sampled records, {materials} materials, {} row records",
        printed_rows.len()
    );
    assert!(sampled > 20 && materials > 20);
}
