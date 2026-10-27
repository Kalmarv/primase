//! The live map data of each map state places no entity twice: no two live static ymaps place the same archetype at
//! the same position (bit for bit). Both maps live at once (a multiplayer pack's renamed copy of a story map beside
//! the story map) shows up here as thousands of doubled placements.
//!
//! Named classes, counted, not failures:
//! - `script-managed`: one of the maps is script-managed (`CMapData` flags bit 0: IPL groups such as a mission's and
//!   a cutscene's version of a room), alternatives scripts switch between;
//! - `one archive`: both maps are in the same archive, so the game's data itself places the entity twice;
//! - `known`: the pairs in [`KNOWN`], placements two packs both author.
//!
//! Needs `installs.gta` and `keys.gta`.
use std::collections::BTreeMap;
use std::fs::File;

use primase_testdata::{Game, install, keys};
use rage_engine_formats::archive::archive_set::MapState;
use rage_engine_formats::archive::archive_set::{ArchiveSet, Install};
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::map::MapData;
use rage_engine_formats::meta::Meta;

/// Static maps of different packs that author the same placement (ymap names, either order).
const KNOWN: [(&str, &str); 1] = [("h4_yacht_critical_0.ymap", "hei_ch3_12_critical_0.ymap")];

/// A live map placing an entity: its path, its archive, whether it is script-managed.
struct Placer<'a> {
    path: &'a str,
    archive: usize,
    scripted: bool,
}

fn file_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

#[test]
fn no_placement_is_live_twice() {
    let (Some(dir), Some(key_dir)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let keys = Keys::from_dir(&key_dir, None).expect("key material");
    let mut doubled = Vec::new();
    for state in [MapState::Story, MapState::Online] {
        let set = ArchiveSet::build(Install::open(&dir, &keys).expect("install"), &keys, state)
            .expect("archive set");
        // (archetype, position bits) -> the maps placing it
        let mut at: BTreeMap<(u32, [u32; 3]), Vec<Placer>> = BTreeMap::new();
        let mut placements = 0usize;
        for &f in set.tables["ymap"].values() {
            let a = &set.install.archives[f.archive];
            let mut file = File::open(&a.file).expect("open archive");
            let root = a
                .resource(&mut file, f.entry, &keys)
                .and_then(|r| Meta::read(&r))
                .and_then(|m| m.root())
                .expect("ymap decodes");
            let map = MapData::from_root(&root).expect("map data");
            placements += map.entities.len();
            let path = set.path(f);
            for e in &map.entities {
                let placers = at.entry((e.archetype, e.position.map(f32::to_bits))).or_default();
                if !placers.iter().any(|p| p.path == path) {
                    placers.push(Placer {
                        path,
                        archive: f.archive,
                        scripted: map.flags & 1 != 0,
                    });
                }
            }
        }
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        let mut failures = Vec::new();
        for placers in at.values().filter(|v| v.len() > 1) {
            let class = if placers.iter().any(|p| p.scripted) {
                "script-managed"
            } else if placers.iter().all(|p| p.archive == placers[0].archive) {
                "one archive"
            } else if placers.len() == 2
                && KNOWN.iter().any(|&(a, b)| {
                    let (x, y) = (file_name(placers[0].path), file_name(placers[1].path));
                    (x, y) == (a, b) || (x, y) == (b, a)
                })
            {
                "known"
            } else {
                failures.push(placers.iter().map(|p| p.path).collect::<Vec<_>>().join(" | "));
                "doubled"
            };
            *classes.entry(class).or_default() += 1;
        }
        println!(
            "{state:?}: {} live ymaps, {placements} placements; placed more than once: {classes:?}",
            set.tables["ymap"].len()
        );
        for f in failures.iter().take(10) {
            println!("    doubled: {f}");
        }
        doubled.push((state, failures.len()));
    }
    assert!(
        doubled.iter().all(|d| d.1 == 0),
        "doubled placements: {doubled:?}"
    );
}
