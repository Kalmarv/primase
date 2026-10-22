//! The level listing against the part files the game's dumps cover (`PRIMASE_STUNTBOOST_DIR`, the install, and
//! `PRIMASE_STUNTBOOST_ORACLE`, the game's parsers' dumps of it: `<L>_<N>.json`, one per `Level_<L>_<N>.xnb` file; the
//! dump tool finds the parts by file name, not through the sidecar).
//!
//! - L1: every level the listing names has its parts `0..modelCount` dumped, and every dumped level is listed. The
//!   differences the install has are named and counted: `Test_Gym` has parts and no sidecar (not a level the game
//!   lists), and `GrindCurve` and `HorizontalRamps` ship one more part file than their sidecar counts (a lone
//!   `=Skybox()` part).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};

use primase_stuntboost::levels;
use primase_testdata::real_data;
use serde_json::Value;

#[test]
fn l1_the_listing_is_the_levels_the_game_dumped() {
    let (Some(install), Some(oracle)) = (
        real_data("PRIMASE_STUNTBOOST_DIR"),
        real_data("PRIMASE_STUNTBOOST_ORACLE"),
    ) else {
        return;
    };
    let mut dumped: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for entry in std::fs::read_dir(&oracle).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        dumped
            .entry(v["level"].as_str().unwrap().to_owned())
            .or_default()
            .insert(v["part"].as_u64().unwrap());
    }
    assert!(!dumped.is_empty(), "no dumps in {}", oracle.display());
    let listed = levels(&install).unwrap();
    println!("listed {} levels, the game dumped {}", listed.len(), dumped.len());
    let mut differences = Vec::new();
    for entry in &listed {
        let Some(parts) = dumped.get(&entry.name) else {
            differences.push(format!("{}: listed, not dumped", entry.name));
            continue;
        };
        let counted: BTreeSet<u64> = (0..u64::from(entry.model_count)).collect();
        if !counted.is_subset(parts) {
            differences.push(format!(
                "{}: modelCount {} but the game dumped parts {parts:?}",
                entry.name, entry.model_count
            ));
        } else if *parts != counted {
            differences.push(format!(
                "{}: {} part files beyond modelCount {}",
                entry.name,
                parts.len() - counted.len(),
                entry.model_count
            ));
        }
    }
    let names: BTreeSet<&str> = listed.iter().map(|e| e.name.as_str()).collect();
    for level in dumped.keys().filter(|l| !names.contains(l.as_str())) {
        differences.push(format!("{level}: dumped, not listed"));
    }
    for d in &differences {
        println!("L1: {d}");
    }
    assert_eq!(
        differences,
        [
            "GrindCurve: 1 part files beyond modelCount 2",
            "HorizontalRamps: 1 part files beyond modelCount 3",
            "Test_Gym: dumped, not listed",
        ]
    );
}
