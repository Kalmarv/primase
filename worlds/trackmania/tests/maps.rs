#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The campaigns' maps (`installs.tmnf`, `GameData/Tracks/Campaigns/Nations`) read through the reader with the game's
//! own files.
//!
//! - M1: every block of every campaign map is placed from its model's mobil, every solid it shows (each of its parts:
//!   visuals, collision and lights) and every file those name read.
//! - M2: every campaign map states its decoration's mood (`trackmania.mood`): its sun, remapping, shadows and light
//!   map mood, and the sun light of its remap folder exactly where that folder holds one.

mod inputs;

use std::collections::BTreeMap;

use inputs::{Game, install};
use primase_core::convert::Reader;
use primase_trackmania::components::TrackmaniaMood;
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
    let mut moods: BTreeMap<String, u32> = BTreeMap::new();
    for path in &maps {
        let bytes = files.read(path).unwrap().unwrap();
        let mut reader = TrackmaniaReader::new(&files, &bytes, path.clone(), FRAME);
        let (world, lines) = reader.read().unwrap();
        match world.resource::<TrackmaniaMood>() {
            Some(m)
                if m.sun.is_some()
                    && m.shadows.is_some()
                    && m.light_map.is_some()
                    && m.remap_folder.as_ref().is_some_and(|f| {
                        files.find(&format!("{f}Directional.SceneLight.Gbx")).is_some()
                            == m.sun_light.is_some()
                    }) =>
            {
                *moods
                    .entry(format!("{} sun light {}", m.decoration, m.sun_light.is_some()))
                    .or_default() += 1;
            }
            other => {
                failures.insert(format!("{path} (mood)"), format!("{other:?}"));
            }
        }
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
            && r.skipped.is_empty()
            && r.unread_files.is_empty())
        {
            failures.insert(
                path.clone(),
                format!(
                    "models {:?} mobils {:?} solids {:?} parts {:?} files {:?}",
                    r.no_model, r.no_mobil, r.no_solid, r.skipped, r.unread_files
                ),
            );
        }
    }
    println!("moods: {moods:?}");
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
