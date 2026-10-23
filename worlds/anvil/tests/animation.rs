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

/// Every track of a kind read decodes, its quaternions are unit, its times rise within the clip, and the first key of
/// each rotation kind equals the stored local rotation of the bone it drives where the bone has one rotation in every
/// Skeleton that holds it (the spike that tells each decode from its alternatives).
#[test]
fn the_values_and_times_of_every_track_of_a_kind_read_decode() {
    use std::collections::{HashMap, HashSet};

    use primase_anvil::read::animation::Values;
    use primase_anvil::read::listing::Kind;
    use primase_anvil::read::skeleton::StoredSkeleton;

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
    // each bone's stored local rotations over the Skeletons that hold it
    let skeletons: Vec<At> = archive
        .listing()
        .filter(|(_, l)| l.kind == Some(Kind::Skeleton))
        .map(|(forge, l)| At {
            forge,
            entry: l.entry,
        })
        .collect();
    let mut rests: HashMap<u32, HashSet<[i32; 4]>> = HashMap::new();
    let mut rest_of: HashMap<u32, [f32; 4]> = HashMap::new();
    for at in skeletons {
        let s = StoredSkeleton::parse(&archive.resource(at).expect("skeleton").body).expect("skeleton");
        for b in &s.bones {
            let key = b.local_rotation.map(|v| (v * 1e5).round() as i32);
            rests.entry(b.name_hash).or_default().insert(key);
            rest_of.insert(b.name_hash, b.local_rotation);
        }
    }
    let animations: Vec<At> = (0..forges.len())
        .flat_map(|forge| {
            archive
                .forge(forge)
                .expect("forge")
                .names
                .iter()
                .enumerate()
                .filter(|(_, n)| n.type_id == ANIMATION)
                .map(move |(entry, _)| At { forge, entry })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut decoded: BTreeMap<u8, u64> = BTreeMap::new();
    let mut unread: BTreeMap<u8, u64> = BTreeMap::new();
    let mut not_unit = 0u64;
    let mut late = 0u64;
    let mut not_rising = 0u64;
    let mut matched: BTreeMap<u8, (u64, u64)> = BTreeMap::new();
    for at in animations {
        let body = archive.resource(at).expect("resource").body;
        let a = StoredAnimation::parse(&body).expect("animation");
        for (t, hash) in a.tracks.iter().zip(&a.mappings) {
            let times = t.times();
            if times.windows(2).any(|w| w[0] >= w[1]) {
                not_rising += 1;
            }
            if times
                .last()
                .is_some_and(|&last| last > a.length + 1.0 / 30.0 + 1e-3)
            {
                late += 1;
            }
            let values = match t.values() {
                Ok(v) => v,
                Err(primase_anvil::read::animation::Error::Unread { kind }) => {
                    *unread.entry(kind).or_default() += 1;
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            *decoded.entry(t.kind()).or_default() += 1;
            let Values::Quaternions(q) = values else {
                continue;
            };
            not_unit += u64::try_from(
                q.iter()
                    .filter(|q| (q.iter().map(|v| v * v).sum::<f32>().sqrt() - 1.0).abs() > 1e-3)
                    .count(),
            )
            .expect("count");
            let one_rest = rests.get(hash).is_some_and(|r| r.len() == 1);
            if let (true, Some(rest), Some(first)) = (one_rest, rest_of.get(hash), q.first()) {
                let dot: f32 = first.iter().zip(rest).map(|(a, b)| a * b).sum();
                let e = matched.entry(t.kind()).or_default();
                e.1 += 1;
                e.0 += u64::from(1.0 - dot.abs() < 1e-5);
            }
        }
    }
    println!(
        "decoded tracks by kind {decoded:?}; unread {unread:?}; quaternions not unit {not_unit}; tracks whose last \
         time is past the clip's Length {late}; times not rising {not_rising}; first keys equal to the bone's stored \
         rotation (matches, tracks with one such rotation) by kind {matched:?}"
    );
    assert_eq!(
        decoded,
        BTreeMap::from([(3, 56_316), (4, 91_519), (7, 36_916), (8, 20_595), (9, 310_926)])
    );
    assert_eq!(
        unread,
        BTreeMap::from([
            (5, 153_187),
            (16, 706_403),
            (17, 51),
            (18, 4_066),
            (21, 8_697),
            (22, 7_908),
            (25, 8)
        ])
    );
    assert_eq!((not_unit, late, not_rising), (0, 0, 0));
    assert_eq!(
        matched,
        BTreeMap::from([
            (3, (6_341, 34_228)),
            (4, (1_493, 6_871)),
            (7, (3_178, 17_817)),
            (8, (364, 1_019))
        ])
    );
}
