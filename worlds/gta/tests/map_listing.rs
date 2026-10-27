//! The install's map listing ([`primase_gta::read::region::list_maps`]) against the whole files: for every live map
//! file of both map states, the header it lists (name, parent, flags, content flags, both extents) is the one the full
//! decode of the file ([`MapData`], as a region read decodes the maps it places) holds, and its entity count the number
//! of entities that decode reads; the listing is the same on one thread and on eight. The content flags agree with what
//! the map holds for the bits the entities state exactly (HD, LOD, SLOD2 and coarser, interiors, physics); bit 4
//! (`slod`) does not follow the entities' levels in the game's own files, so it is counted, not asserted.
//!
//! Needs `installs.gta` and `keys.gta`.
use primase_foundations::parallel::Threads;
use primase_gta::read::region::{States, archive_sets, list_maps};
use rage_engine_formats::archive::archive_set::Install;
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::map::MapData;
use rage_engine_formats::meta::Meta;

#[test]
fn every_listed_header_is_the_decoded_files() {
    let (Some(dir), Some(keys)) = (
        primase_testdata::install(primase_testdata::Game::Gta),
        primase_testdata::keys(primase_testdata::Game::Gta),
    ) else {
        return;
    };
    let keys = Keys::from_dir(&keys, Some(&primase_gta::read::key_digests::DIGESTS)).expect("keys");
    let install = Install::open(&dir, &keys).expect("install");
    let (story, online) = archive_sets(&install, &keys, States::All).expect("archive sets");
    // bit 4 against "an entity at SLOD1 or coarser", over both states: (both, bit only, entities only)
    let mut slod = [(0usize, 0usize, 0usize); 2];
    for (n, (state, set)) in [
        ("story", &story),
        ("online", online.as_ref().expect("online set")),
    ]
    .into_iter()
    .enumerate()
    {
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
            // the content flags agree with what the map holds, bit by bit where the full decode reads it
            // (CodeWalker's meanings: HD and orphan-HD 1, LOD 2, SLOD2 and coarser 4, interiors 8, physics dictionaries
            // 64); bit 16 (SLOD and coarser) is counted: the game's files set it for most maps with SLOD1 entities and
            // leave it off for some, and set it for some maps with none
            let c = l.header.content();
            let levels: Vec<i32> = full.entities.iter().map(|e| e.lod_level).collect();
            let has = |want: &[i32]| levels.iter().any(|l| want.contains(l));
            let entities_slod = has(&[2, 3, 4, 6]);
            let counts = &mut slod[n];
            match (c.slod, entities_slod) {
                (true, true) => counts.0 += 1,
                (true, false) => counts.1 += 1,
                (false, true) => counts.2 += 1,
                (false, false) => {}
            }
            assert_eq!(
                (c.hd, c.lod, c.slod2, c.interiors, c.physics),
                (
                    has(&[0, 5]),
                    has(&[1]),
                    has(&[3, 4, 6]),
                    full.entities
                        .iter()
                        .any(|e| e.kind == rage_engine_formats::map::EntityKind::Interior),
                    !full.physics_dictionaries.is_empty()
                ),
                "{state}: {} content flags {:#x}",
                l.path,
                l.header.content_flags
            );
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
        let (both, bit_only, entities_only) = slod[n];
        println!(
            "{state}: bit 4 set with SLOD1-or-coarser entities in {both} maps, without them in {bit_only}, unset with them in {entities_only}"
        );
    }
}
