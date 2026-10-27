//! [`Selector::read`] on every program record of the game configuration's and the level root's program databases (the
//! skate. install and its Oodle 9 library; unset skips): each record is read or refused, and the census is printed:
//! the records read (without options: one permutation; with a tree) and the refusals by reason (the numbers of the
//! message replaced by `#`), each with one example (database, record index, record key). For the records read, the
//! trees' sizes (options, nodes, leaves, nodes that mix child and leaf entries) and a check that walking each leaf's
//! own path returns that leaf's permutation. A refusal is data, not a failure: the parser is changed against the
//! shapes it finds. What fails the test is a walk that does not return its leaf, or no record read at all.
//!
//! The prediction written before the run: every record is read (130 of 130 sampled records with a tree held, and the
//! 8 without options had one permutation); if some are refused, the reasons name a shape the sample did not hold.
//!
//! What the one run showed: all 5,137 records (1,518 of the game configuration's database, 3,619 of the level root's)
//! are read, none refused: 473 have no options (and one permutation), 4,664 have a tree of 1 to 7 options; 42,297 nodes
//! (2,444 mix child and leaf entries) and 62,274 leaves, and walking each leaf's own path returns that leaf in all
//! 62,274. Records list 1 to 144 permutations (1, 2, 3, 4, 6, 8, 12, 16, 18, 24, 32, 36, 48, 60, 72, 96, 120 or 144).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::lookup;
use frostbite_formats::selector::{Selector, Target};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_testdata::{Game, Tool, install_file, tool};

fn u32le(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

/// A refusal's message with its numbers replaced, so that equal shapes count together.
fn reason(message: &str) -> String {
    let mut out = String::new();
    let mut in_number = false;
    for c in message.chars() {
        if c.is_ascii_hexdigit() && (in_number || c.is_ascii_digit()) {
            if !in_number {
                out.push('#');
            }
            in_number = true;
        } else if in_number && c == 'x' {
            // the 0x of a hexadecimal number
        } else {
            in_number = false;
            out.push(c);
        }
    }
    out
}

#[test]
fn every_program_record_of_the_install_reads_or_is_refused_by_name() {
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
    let globals = src.listing("Win32/globals.toc").unwrap();
    let root = src
        .listing("Win32/levels/game/dingolevel_root/dingolevel_root.toc")
        .unwrap();
    let (mut records, mut read, mut no_options) = (0usize, 0usize, 0usize);
    let mut refused: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut options: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut nodes, mut leaves, mut mixed, mut walked, mut walk_wrong) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut permutations: BTreeMap<usize, usize> = BTreeMap::new();
    for bundle in globals.bundles.iter().chain(root.bundles.iter()) {
        for (i, a) in bundle.manifest.res.iter().enumerate() {
            if a.rtype != lookup::PROGRAM_LOOKUP {
                continue;
            }
            let entry = bundle.resource(i);
            let payload = src.payload(entry.file, entry.size as usize).unwrap();
            let all = lookup::read_records(&payload, &a.rmeta).unwrap();
            let section = &payload[..u32le(&a.rmeta, 0)];
            for (index, record) in all.iter().enumerate() {
                records += 1;
                match Selector::read(section, &record.raw) {
                    Err(e) => {
                        let slot = refused.entry(reason(&e.to_string())).or_insert_with(|| {
                            (
                                0,
                                format!("{} record {index} key {:#018x}: {e}", entry.name, record.key),
                            )
                        });
                        slot.0 += 1;
                    }
                    Ok(s) => {
                        read += 1;
                        no_options += usize::from(s.nodes.is_empty());
                        *options.entry(s.options.len()).or_default() += 1;
                        *permutations.entry(s.permutations).or_default() += 1;
                        nodes += s.nodes.len();
                        mixed += s
                            .nodes
                            .iter()
                            .filter(|n| {
                                n.edges.iter().any(|e| matches!(e.to, Target::Node(_)))
                                    && n.edges.iter().any(|e| matches!(e.to, Target::Permutation(_)))
                            })
                            .count();
                        let ends = s.leaves();
                        leaves += ends.len();
                        for leaf in &ends {
                            // the values on the way, any other option at a value no node names
                            let mut values = vec![0u64; s.options.len()];
                            for &(option, value) in &leaf.path {
                                values[option] = value;
                            }
                            walked += 1;
                            if s.walk(&values) != Ok(leaf.permutation) {
                                walk_wrong += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{records} records: {read} read ({no_options} without options), {} refused",
        records - read
    );
    println!("options per record: {options:?}");
    println!("permutations per record: {permutations:?}");
    println!(
        "{nodes} nodes ({mixed} mix child and leaf entries), {leaves} leaves; {walked} leaf paths walked, {walk_wrong} wrong"
    );
    for (why, (n, example)) in &refused {
        println!("REFUSED x{n}: {why}\n    example: {example}");
    }
    assert_eq!(walk_wrong, 0);
    assert!(read > 0);
}
