//! Texture dictionary parents against an independent reader's table of the same install.
//!
//! Oracle (`PRIMASE_GTA_GTXD`): JSON `{parents: {child hash: parent hash}, sources: {path: [[child, parent], ...]}}`,
//! `sources` listing every gtxd file and `vehicles.meta` of the install with its pairs. That reader also takes the
//! vehicles' texture dictionary pairs from `vehicles.meta` (named class `vehicles`: they are vehicle data, read with
//! the vehicles); every other difference fails the gate.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
use std::collections::{BTreeMap, BTreeSet};

use primase_gta::gtxd::TextureParents;
use primase_gta::{ArchiveSet, Install, Keys, MapState};
use primase_testdata::real_data;

#[derive(serde::Deserialize)]
struct Oracle {
    parents: BTreeMap<String, u32>,
    sources: BTreeMap<String, Vec<[u32; 2]>>,
}

#[test]
fn texture_parents_against_the_oracle() {
    let (Some(dir), Some(key_dir), Some(oracle)) = (
        real_data("PRIMASE_GTA_DIR"),
        real_data("PRIMASE_GTA_KEYS"),
        real_data("PRIMASE_GTA_GTXD"),
    ) else {
        return;
    };
    let keys = Keys::from_dir(&key_dir).expect("key material");
    let set = ArchiveSet::build(
        Install::open(&dir, &keys).expect("install"),
        &keys,
        MapState::Online,
    )
    .expect("archive set");
    let ours = TextureParents::read(&set, &keys).expect("texture parents");
    let o: Oracle =
        serde_json::from_str(&std::fs::read_to_string(oracle).expect("oracle")).expect("oracle json");
    let theirs: BTreeMap<u32, u32> = o
        .parents
        .iter()
        .map(|(k, v)| (k.parse().expect("hash"), *v))
        .collect();
    let vehicles: BTreeSet<[u32; 2]> = o
        .sources
        .iter()
        .filter(|(p, _)| p.ends_with("vehicles.meta"))
        .flat_map(|(_, v)| v.iter().copied())
        .collect();
    println!(
        "ours: {} parents from {:?}, {} conflicts; oracle: {}",
        ours.parents.len(),
        ours.sources,
        ours.conflicts.len(),
        theirs.len()
    );
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unexplained = Vec::new();
    for (&c, &p) in &theirs {
        match ours.parents.get(&c) {
            Some(&q) if q == p => *classes.entry("same").or_default() += 1,
            None if vehicles.contains(&[c, p]) => *classes.entry("vehicles").or_default() += 1,
            q => unexplained.push(format!("{c:#010x}: ours {q:?}, oracle {p:#010x}")),
        }
    }
    for (&c, &p) in &ours.parents {
        if !theirs.contains_key(&c) {
            unexplained.push(format!("{} -> {}: ours only", ours.names[&c], ours.names[&p]));
        }
    }
    println!("{classes:?}, unexplained {}", unexplained.len());
    for u in unexplained.iter().take(10) {
        println!("    {u}");
    }
    assert!(unexplained.is_empty());
}
