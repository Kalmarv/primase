#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The campaigns' maps (`installs.tmnf`, `GameData/Tracks/Campaigns/Nations`) read through the reader with the game's
//! own files.
//!
//! - M1: every block of every campaign map is placed from its model's mobil, every solid it shows and every file
//!   those name read.

mod inputs;

use std::collections::BTreeMap;

use inputs::{Game, install};
use primase_core::convert::Reader;
use primase_trackmania::read::GameFiles;
use primase_trackmania::{FRAME, TrackmaniaReader};

#[test]
fn m1_every_campaign_block_is_placed() {
    let Some(root) = install(Game::Tmnf) else {
        return;
    };
    let files = GameFiles::open(&root).unwrap();
    let maps: Vec<String> = files
        .loose_paths()
        .filter(|p| p.starts_with(r"tracks\campaigns\nations\") && p.ends_with(".challenge.gbx"))
        .map(str::to_owned)
        .collect();
    assert!(!maps.is_empty(), "no campaign maps in {}", root.display());
    let mut failures = BTreeMap::new();
    for path in &maps {
        let bytes = files.read(path).unwrap().unwrap();
        let mut reader = TrackmaniaReader::new(&files, &bytes, path.clone(), FRAME);
        let (_world, lines) = reader.read().unwrap();
        let r = &reader.report;
        assert_eq!(
            r.placed + r.empty,
            r.blocks - r.no_model.values().sum::<u32>() as usize
        );
        println!("{path}: {}", lines[0].1);
        if !(r.no_model.is_empty()
            && r.no_mobil.is_empty()
            && r.unshown.is_empty()
            && r.no_solid.is_empty()
            && r.unread_files.is_empty())
        {
            failures.insert(
                path.clone(),
                format!(
                    "models {:?} mobils {:?} solids {:?} files {:?}",
                    r.no_model, r.no_mobil, r.no_solid, r.unread_files
                ),
            );
        }
    }
    for (p, r) in &failures {
        println!("NOT PLACED {p}: {r}");
    }
    assert!(
        failures.is_empty(),
        "{} of {} maps not wholly placed",
        failures.len(),
        maps.len()
    );
}
