//! Every Animation of Riders Republic's forges read to its last byte: the resource's longer header (file header 1 and
//! its table), the body's header, mappings and tracks, with the counts the census of the six forges (build 25394987)
//! gives.
use std::collections::BTreeMap;

use primase_anvil::read::animation::{ANIMATION, StoredAnimation};
use primase_anvil::read::archive::{Archive, At};
use primase_testdata::{Game, install};

#[test]
fn every_animation_reads_to_its_last_byte() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let forges = [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ];
    let mut archive = Archive::open(&forges.map(|f| dir.join(f))).expect("archive");
    let mut at = Vec::new();
    for forge in 0..forges.len() {
        let names = &archive.forge(forge).expect("forge").names;
        at.extend(
            names
                .iter()
                .enumerate()
                .filter(|(_, n)| n.type_id == ANIMATION)
                .map(|(entry, n)| (At { forge, entry }, n.name.clone())),
        );
    }
    let (mut events, mut ecl, mut streamed, mut tracks, mut keys) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut kinds: BTreeMap<(u8, u8), u64> = BTreeMap::new();
    let mut rows: BTreeMap<usize, u64> = BTreeMap::new();
    for (at, name) in &at {
        let r = archive.resource(*at).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((r.file_header, r.extension), (1, ANIMATION), "{name}");
        *rows.entry(r.header_rows.len()).or_default() += 1;
        let a = StoredAnimation::parse(&r.body).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(a.tracks.len(), a.mappings.len(), "{name}");
        events += u64::from(a.events.count > 0);
        ecl += u64::from(a.ecl.is_some());
        streamed += u64::from(a.flags != [0; 4]);
        tracks += u64::try_from(a.tracks.len()).expect("count");
        for t in &a.tracks {
            keys += u64::from(t.keys);
            *kinds.entry((t.kind(), t.time_form())).or_default() += 1;
        }
    }
    println!(
        "{} Animations, {events} with events, {ecl} with a TTECLAnimation, {streamed} with a boolean set, {tracks} \
         tracks of {keys} keys; tracks by (kind, time form) {kinds:?}; header table rows {rows:?}",
        at.len()
    );
    assert_eq!(at.len(), 9_087);
    assert_eq!((events, ecl, streamed), (1_923, 4_053, 390));
    assert_eq!((tracks, keys), (1_396_592, 25_241_022));
    // the kinds the census finds, and the time forms each is stored in
    assert_eq!(
        kinds
            .keys()
            .map(|&(kind, _)| kind)
            .collect::<std::collections::BTreeSet<_>>(),
        [3, 4, 5, 7, 8, 9, 16, 17, 18, 21, 22, 25].into()
    );
}
