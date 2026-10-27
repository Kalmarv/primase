//! The install's listing against an independent reader's: every archive of the install and, in each, every file
//! with the numbers its table of contents gives.
//!
//! Oracle (`oracles.codewalker.gta/listing.jsonl`, CodeWalker.Core 485d56b through a dump harness; schema in
//! `census.rs`): one archive per line. A resource's oracle record holds its page flags, whose sizes are the pages'
//! bytes ([`PageFlags::size`], the size the inflated body is checked against in `census.rs`). Needs `installs.gta`,
//! `keys.gta` and `oracles.codewalker.gta/listing.jsonl`.
mod inputs;

use std::collections::BTreeMap;

use inputs::{Game, OracleSet, install, keys, oracle};
use rage_engine_formats::archive::archive_set::Install;
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::archive::listing::Storage;
use rage_engine_formats::resource::PageFlags;

#[derive(serde::Deserialize)]
struct OracleArchive {
    path: String,
    entries: Vec<Vec<serde_json::Value>>,
}

#[test]
fn the_listing_is_the_oracles_archives_and_files() {
    let (Some(dir), Some(key_dir), Some(listing)) = (
        install(Game::Gta),
        keys(Game::Gta),
        oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl"),
    ) else {
        return;
    };
    let keys = Keys::from_dir(&key_dir, None).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let oracle: Vec<OracleArchive> = std::fs::read_to_string(listing)
        .expect("listing")
        .lines()
        .map(|l| serde_json::from_str(l).expect("listing line"))
        .collect();

    // the oracle's files by path
    let mut expected: BTreeMap<String, (String, Storage)> = BTreeMap::new();
    for a in &oracle {
        for e in &a.entries {
            let path = e[0].as_str().expect("entry path").to_owned();
            let number = |i: usize| e[i].as_u64().expect("entry number");
            let storage = match e[1].as_str() {
                Some("bin") => Storage::Binary {
                    stored: number(3) as u32,
                    size: number(4) as u32,
                    encrypted: number(5) != 0,
                },
                Some("res") => Storage::Resource {
                    system: PageFlags(number(4) as u32).size(),
                    graphics: PageFlags(number(5) as u32).size(),
                },
                _ => continue,
            };
            expected.insert(path, (a.path.clone(), storage));
        }
    }

    let mut ours = BTreeMap::new();
    let mut wrong = Vec::new();
    for f in install.listing() {
        ours.insert(f.path.to_owned(), (f.archive.to_owned(), f.storage));
        match expected.get(f.path) {
            None => wrong.push(format!("{}: not in the oracle", f.path)),
            Some(want) if *want != (f.archive.to_owned(), f.storage) => {
                wrong.push(format!(
                    "{}: {:?} vs oracle {:?}",
                    f.path,
                    (f.archive, f.storage),
                    want
                ));
            }
            Some(_) => {}
        }
    }
    for path in expected.keys().filter(|p| !ours.contains_key(*p)) {
        wrong.push(format!("{path}: only in the oracle"));
    }
    // every oracle archive is listed, with as many files as the oracle lists in it
    let listed: BTreeMap<&str, usize> = install.archive_listing().map(|a| (a.path, a.files)).collect();
    for a in &oracle {
        let files = a.entries.iter().filter(|e| e[1] != "dir").count();
        if listed.get(a.path.as_str()) != Some(&files) {
            wrong.push(format!(
                "archive {}: {:?} files vs oracle {files}",
                a.path,
                listed.get(a.path.as_str())
            ));
        }
    }
    println!(
        "{} archives, {} files listed; oracle {} archives, {} files",
        listed.len(),
        ours.len(),
        oracle.len(),
        expected.len()
    );
    assert!(
        wrong.is_empty(),
        "{} differences, e.g. {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(5)]
    );
}
