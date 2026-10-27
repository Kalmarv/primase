#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! Every clip dictionary (`.ycd`) of the install read through [`ClipDictionary::read_resource`]: the container, the
//! animation map and clip map, every animation's sequences and bone ids, every clip's name and the animations it
//! plays. A dictionary that fails is counted by the error's message (the test fails on any); the counts are printed.
//!
//! Needs the GTA install and its key material.
use std::collections::BTreeMap;

use primase_testdata::{Game, install, keys};
use rage_engine_formats::archive::archive_set::Install;
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::ycd::ClipDictionary;

#[test]
fn every_clip_dictionary_reads() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let keys = Keys::from_dir(&keys, None).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let (mut dictionaries, mut animations, mut clips, mut sequences, mut bones, mut bytes) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0u64);
    let mut lists = 0usize;
    let mut failures: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in install.listing().filter(|f| f.extension == "ycd") {
        let result = install
            .resource(f.file, &keys)
            .map_err(|e| e.to_string())
            .and_then(|r| ClipDictionary::read_resource(&r).map_err(|e| e.to_string()));
        match result {
            Ok(d) => {
                dictionaries += 1;
                animations += d.animations.len();
                clips += d.clips.len();
                lists += d.clips.iter().filter(|c| c.list_duration.is_some()).count();
                for (_, a) in &d.animations {
                    sequences += a.sequences.len();
                    bones += a.bone_ids.len();
                    bytes += a.sequences.iter().map(|s| s.data.len() as u64).sum::<u64>();
                }
            }
            Err(e) => failures.entry(e).or_default().push(f.path.to_owned()),
        }
    }
    println!(
        "{dictionaries} dictionaries, {animations} animations, {clips} clips ({lists} lists), {sequences} sequences, {bones} bone ids, {bytes} bytes of channel data"
    );
    for (message, files) in &failures {
        println!("{} files: {message}, e.g. {}", files.len(), files[0]);
    }
    assert!(failures.is_empty(), "{} kinds of failure", failures.len());
    assert!(dictionaries > 0);
}
