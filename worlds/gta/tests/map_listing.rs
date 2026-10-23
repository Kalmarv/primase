//! The install's map listing ([`primase_gta::read::region::list_maps`]) against the whole files: for every live map
//! file of both map states, the header it lists (name, parent, flags, content flags, both extents) is the one the full
//! decode of the file ([`MapData`], as a region read decodes the maps it places) holds, and its entity count the number
//! of entities that decode reads; the listing is the same on one thread and on eight.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
use primase_core::parallel::Threads;
use primase_gta::rage::map::MapData;
use primase_gta::rage::meta::Meta;
use primase_gta::read::archive::archive_set::Install;
use primase_gta::read::archive::keys::Keys;
use primase_gta::read::region::{States, archive_sets, list_maps};

#[test]
fn every_listed_header_is_the_decoded_files() {
    let (Some(dir), Some(keys)) = (
        primase_testdata::real_data("PRIMASE_GTA_DIR"),
        primase_testdata::real_data("PRIMASE_GTA_KEYS"),
    ) else {
        return;
    };
    let keys = Keys::from_dir(&keys).expect("keys");
    let install = Install::open(&dir, &keys).expect("install");
    let (story, online) = archive_sets(&install, &keys, States::All).expect("archive sets");
    for (state, set) in [
        ("story", &story),
        ("online", online.as_ref().expect("online set")),
    ] {
        let started = std::time::Instant::now();
        let listed = list_maps(set, &keys, Threads::new(8).unwrap()).expect("listing");
        println!("{state}: {} maps listed in {:?}", listed.len(), started.elapsed());
        assert!(!listed.is_empty(), "{state}: no maps");
        assert!(
            list_maps(set, &keys, Threads::ONE).expect("listing") == listed,
            "{state}: one thread"
        );
        for l in &listed {
            let full = MapData::from_root(
                &Meta::read(&install.resource(l.file, &keys).expect("resource"))
                    .expect("meta")
                    .root()
                    .expect("root"),
            )
            .expect("map data");
            let h = &l.header;
            assert_eq!(
                (
                    h.name,
                    h.parent,
                    h.flags,
                    h.content_flags,
                    h.streaming_extents,
                    h.entities_extents,
                    l.entities
                ),
                (
                    full.name,
                    full.parent,
                    full.flags,
                    full.content_flags,
                    full.streaming_extents,
                    full.entities_extents,
                    full.entities.len()
                ),
                "{state}: {}",
                l.path
            );
        }
    }
}
