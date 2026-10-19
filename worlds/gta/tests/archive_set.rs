//! The live-file tables of an install against an independent reader's tables of the same install.
//!
//! Oracle (`PRIMASE_GTA_ACTIVE`): JSON `{kind: {name hash (decimal): path}}` for the kinds ymap, ytyp, ydr, ydd,
//! yft, ytd, ybn, paths as in the census listing. That reader applies the multiplayer packs' map changes, so the
//! comparison is with GTA Online's map ([`MapState::Online`]). It follows its own rules where they differ from the
//! game's mount data (`archive_set`). Every difference is put in one of these named classes and counted; anything
//! else fails the gate:
//! - `skipped pack`: our copy is in `patchday27ng`, a pack that reader leaves out on purpose (its source calls this
//!   a hack against the map breaking: run pack by pack, that pack's story overlays and the multiplayer maps would
//!   both be live);
//! - `title-update patch`: our copy comes from `update.rpf\dlc_patch\`, which that reader applies to map data only;
//! - `not mounted`: that reader's copy is in an archive the game does not mount (on-demand packs, archives no change
//!   set enables), as it looks names up over every archive;
//! - `mount order`: both copies are mounted and ours ranks later (overlay archives over plain ones, then mount
//!   order); that reader ignores overlay flags and takes the last copy in file-system order.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
use std::collections::{BTreeMap, BTreeSet};

use primase_gta::read::archive::archive_set::MapState;
use primase_gta::read::archive::archive_set::{ArchiveSet, Install};
use primase_gta::read::archive::keys::Keys;
use primase_testdata::real_data;

#[test]
fn live_files_against_the_oracle() {
    let (Some(dir), Some(key_dir), Some(active)) = (
        real_data("PRIMASE_GTA_DIR"),
        real_data("PRIMASE_GTA_KEYS"),
        real_data("PRIMASE_GTA_ACTIVE"),
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
    println!(
        "{} packs, {} map archives, {} mounted archives ({} overlays)",
        set.packs.len(),
        set.map_archives.len(),
        set.mounted.len(),
        set.overlay.len()
    );
    let oracle: BTreeMap<String, BTreeMap<String, String>> =
        serde_json::from_str(&std::fs::read_to_string(active).expect("oracle")).expect("oracle json");
    let mounted: BTreeSet<usize> = set
        .mounted
        .iter()
        .copied()
        .chain(set.map_archives.values().copied())
        .collect();
    let is_mounted = |path: &str| {
        set.install
            .file(path)
            .is_some_and(|f| mounted.contains(&f.archive))
    };
    // how the live tables rank copies: overlay archives over plain ones, then mount order (map data in the map
    // archives' order)
    let rank = |kind: &str, path: &str| -> Option<(bool, usize)> {
        let archive = set.install.file(path)?.archive;
        let pos = if ["ymap", "ybn", "ynv"].contains(&kind) {
            set.map_archives.values().position(|&a| a == archive)
        } else {
            set.mounted.iter().position(|&a| a == archive)
        }?;
        Some((set.overlay.contains(&archive), pos))
    };
    let class = |kind: &str, ours: Option<&str>, theirs: Option<&str>| -> &'static str {
        if ours.is_some_and(|p| p.contains("\\patchday27ng\\"))
            || theirs.is_some_and(|p| p.contains("\\patchday27ng\\"))
        {
            "skipped pack"
        } else if ours.is_some_and(|p| p.starts_with("update\\update.rpf\\dlc_patch\\")) {
            "title-update patch"
        } else if theirs.is_some_and(|p| !is_mounted(p)) {
            "not mounted"
        } else if let (Some(q), Some(p)) = (ours, theirs)
            && rank(kind, q) > rank(kind, p)
        {
            "mount order"
        } else {
            "unexplained"
        }
    };
    let mut unexplained = Vec::new();
    for (kind, o) in &oracle {
        let ours = set.tables.get(kind).cloned().unwrap_or_default();
        let mut same = 0u64;
        let mut classes: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (h, p) in o {
            let h: u32 = h.parse().expect("hash");
            match ours.get(&h).map(|f| set.path(*f)) {
                Some(q) if q == p => same += 1,
                q => classes
                    .entry(class(kind, q, Some(p)))
                    .or_default()
                    .push(format!("{} | oracle {p}", q.unwrap_or("-"))),
            }
        }
        for (h, f) in &ours {
            if !o.contains_key(&h.to_string()) {
                let q = set.path(*f);
                classes
                    .entry(class(kind, Some(q), None))
                    .or_default()
                    .push(format!("{q} | oracle -"));
            }
        }
        let summary: Vec<String> = classes.iter().map(|(k, v)| format!("{k} {}", v.len())).collect();
        println!(
            "{kind}: {} ours, {} oracle, {same} same; {}",
            ours.len(),
            o.len(),
            summary.join(", ")
        );
        if let Some(v) = classes.get("unexplained") {
            for x in v.iter().take(5) {
                println!("    unexplained: {x}");
            }
            unexplained.extend(v.iter().map(|x| format!("{kind}: {x}")));
        }
    }
    assert!(
        unexplained.is_empty(),
        "{} unexplained differences",
        unexplained.len()
    );
}
