//! `select-anvil-clips` on candidates built by hand (the coverage threshold, the cap and its order, every occurrence
//! bound, a clip the entity already plays left out, the options), and on real Entities of Riders Republic's forges whose
//! rigs are given the library's clips through the Placer, then chosen and lowered under the pass manager with its checks
//! on.
use std::sync::Arc;

use primase_anvil::components::{
    AnvilClip, AnvilClipBinding, AnvilClipCandidates, AnvilRigOccurrence, ClipCandidate, LibraryClip,
    StoredAnimation, StoredEvents,
};
use primase_anvil::passes::{
    LowerAnvilClips, LowerAnvilLods, LowerAnvilMesh, LowerAnvilRig, LowerAnvilSkin, OwnTransform,
    SELECT_ANVIL_CLIPS, SelectAnvilClips, Static16Normal,
};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::listing::Kind;
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::clip::{Clip, ClipBinding, Extrapolation, Interpolation};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Entity, Name, World};
use primase_testdata::{Game, install};

fn animation() -> StoredAnimation {
    StoredAnimation {
        length: 1.0,
        id_hash: 0,
        f2: 0,
        arrays: [Vec::new(), Vec::new()],
        flags: [0; 4],
        events: StoredEvents {
            count: 0,
            bytes: Vec::new(),
        },
        track_data_id: 0,
        track_data_hash: 0,
        mappings: Vec::new(),
        track_data_tail: 0,
        ecl: None,
        buffer: Vec::new(),
        cb4e: 0,
        tracks: Vec::new(),
    }
}

/// A candidate named `name` with id `id`: `covered` of `tracks` joint-driving tracks on the rig.
fn candidate(id: u64, name: &str, tracks: u32, covered: u32) -> ClipCandidate {
    ClipCandidate {
        clip: Arc::new(LibraryClip {
            id,
            name: name.to_owned(),
            animation: animation(),
            joint_tracks: (0..tracks).collect(),
        }),
        covered,
    }
}

fn options(coverage: &str, max: &str) -> Options {
    Options::from([
        ("min-coverage".to_owned(), coverage.to_owned()),
        ("max-clips".to_owned(), max.to_owned()),
    ])
}

/// A rig with two occurrences and the candidates `c`.
fn world(c: Vec<ClipCandidate>) -> (World, Entity, [Entity; 2]) {
    let mut w = World::new();
    let rig = w.spawn();
    w.insert(rig, Name("rider".to_owned()));
    w.insert(
        rig,
        AnvilClipCandidates {
            frame: SourceFrame::WORLD,
            candidates: c,
        },
    );
    let occurrences = [w.spawn(), w.spawn()];
    for (k, o) in occurrences.iter().enumerate() {
        w.insert(*o, Name(format!("rider {k}")));
        w.insert(*o, AnvilRigOccurrence { rig });
    }
    (w, rig, occurrences)
}

fn select(w: &mut World, o: &Options) -> Result<RunReport, String> {
    let pass = SELECT_ANVIL_CLIPS.instance(o)?;
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let mut report = RunReport::default();
    manager
        .run(w, &Pipeline { passes: vec![pass] }, &mut report)
        .map_err(|e| format!("{e:#}"))?;
    Ok(report)
}

fn chosen(w: &World) -> Vec<String> {
    let mut names: Vec<String> = w
        .iter::<AnvilClip>()
        .map(|(e, _)| w.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_default())
        .collect();
    names.sort();
    names
}

#[test]
fn clips_at_or_over_the_coverage_stay_the_highest_first_up_to_the_cap() {
    let (mut w, _, _) = world(vec![
        candidate(1, "half", 4, 2),
        candidate(2, "all", 4, 4),
        candidate(3, "three_quarters", 4, 3),
        candidate(4, "all_of_two", 2, 2),
        candidate(5, "a_quarter", 4, 1),
    ]);
    select(&mut w, &options("0.5", "3")).expect("select");
    // 'a_quarter' is under 0.5; of the other four ('all', 'all_of_two' at 1, then 'three_quarters', 'half') the most
    // covered of those at full coverage, then the next, stay: three
    assert_eq!(chosen(&w), ["all", "all_of_two", "three_quarters"]);
    assert_eq!(w.iter::<AnvilClip>().count(), 3);
    assert_eq!(w.iter::<AnvilClipCandidates>().count(), 0);
}

#[test]
fn every_chosen_clip_binds_to_every_occurrence_of_the_rig() {
    let (mut w, rig, occurrences) = world(vec![candidate(1, "run", 2, 2), candidate(2, "jump", 2, 2)]);
    select(&mut w, &options("1", "8")).expect("select");
    let clips: Vec<Entity> = w.iter::<AnvilClip>().map(|(e, _)| e).collect();
    assert_eq!(clips.len(), 2);
    assert!(w.iter::<AnvilClip>().all(|(_, c)| c.rig == rig));
    assert_eq!(w.iter::<AnvilClipBinding>().count(), 4);
    for clip in clips {
        for o in occurrences {
            assert!(
                w.iter::<AnvilClipBinding>()
                    .any(|(_, b)| b.clip == clip && b.occurrence == o)
            );
        }
    }
}

#[test]
fn a_clip_the_entity_already_plays_is_not_chosen_again() {
    let (mut w, rig, _) = world(vec![candidate(7, "own", 2, 2), candidate(8, "other", 2, 2)]);
    let own = w.spawn();
    w.insert(own, Name("own".to_owned()));
    w.insert(
        own,
        AnvilClip {
            frame: SourceFrame::WORLD,
            id: 7,
            animation: animation(),
            rig,
        },
    );
    let report = select(&mut w, &options("1", "8")).expect("select");
    let names = chosen(&w);
    // the entity's own clip, and the one chosen
    assert_eq!(names, ["other", "own"]);
    assert!(
        report
            .steps
            .iter()
            .any(|s| s.log.counts.keys().any(|k| k.contains("already plays")))
    );
}

#[test]
fn a_rig_with_no_occurrence_gets_no_clips() {
    let mut w = World::new();
    let rig = w.spawn();
    w.insert(
        rig,
        AnvilClipCandidates {
            frame: SourceFrame::WORLD,
            candidates: vec![candidate(1, "run", 2, 2)],
        },
    );
    select(&mut w, &options("1", "8")).expect("select");
    assert_eq!(w.iter::<AnvilClip>().count(), 0);
    assert_eq!(w.iter::<AnvilClipBinding>().count(), 0);
}

#[test]
fn both_options_are_required_and_checked() {
    assert!(SELECT_ANVIL_CLIPS.instance(&Options::new()).is_err());
    assert!(
        SELECT_ANVIL_CLIPS
            .instance(&Options::from([("min-coverage".to_owned(), "0.5".to_owned())]))
            .is_err()
    );
    assert!(
        SELECT_ANVIL_CLIPS
            .instance(&Options::from([("max-clips".to_owned(), "3".to_owned())]))
            .is_err()
    );
    for (f, n) in [
        ("0", "3"),
        ("1.5", "3"),
        ("nan", "3"),
        ("x", "3"),
        ("0.5", "0"),
        ("0.5", "-1"),
        ("0.5", "2.5"),
    ] {
        assert!(SELECT_ANVIL_CLIPS.instance(&options(f, n)).is_err(), "{f} {n}");
    }
    let mut extra = options("0.5", "3");
    extra.insert("cap".to_owned(), "3".to_owned());
    assert!(SELECT_ANVIL_CLIPS.instance(&extra).is_err());
    assert!(SELECT_ANVIL_CLIPS.instance(&options("0.5", "3")).is_ok());
}

/// Real Entities that place a rig, given the library's clips: the candidates measured per rig, then the clips chosen
/// and lowered with the pass checks on.
#[test]
fn real_rigs_are_given_the_library_and_choose_clips_by_coverage() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    for name in ["MAD_ANM_Lizard", "MAD_CIN_DH_Bike_A_001"] {
        let at = archive
            .listing()
            .find(|(_, l)| l.kind == Some(Kind::Entity) && l.name == name)
            .map(|(forge, l)| At {
                forge,
                entry: l.entry,
            })
            .unwrap_or_else(|| panic!("no Entity {name}"));
        let mut w = World::new();
        let mut placed = Placed::default();
        let mut placer = Placer::new(
            &mut archive,
            &mut w,
            SourceFrame::WORLD,
            InstancedLevels::Selector,
        )
        .with_clip_candidates();
        placer.place(at, &mut placed).expect("place");
        assert!(placed.refused.is_empty(), "{name}: {:?}", placed.refused);
        let counts: Vec<(usize, usize)> = w
            .iter::<AnvilClipCandidates>()
            .map(|(_, c)| {
                let full = c
                    .candidates
                    .iter()
                    .filter(|k| usize::try_from(k.covered).is_ok_and(|n| n == k.clip.joint_tracks.len()))
                    .count();
                (c.candidates.len(), full)
            })
            .collect();
        println!("{name}: candidates and those fully covered, per rig: {counts:?}");
        let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
        let pipeline = Pipeline {
            passes: vec![
                Box::new(LowerAnvilMesh {
                    static16_normal: Static16Normal::Omit,
                }),
                Box::new(LowerAnvilLods),
                Box::new(LowerAnvilRig {
                    own_transform: OwnTransform::Ignore,
                }),
                Box::new(LowerAnvilSkin),
                Box::new(SelectAnvilClips {
                    min_coverage: 0.9,
                    max_clips: 8,
                }),
                Box::new(LowerAnvilClips {
                    interpolation: Interpolation::Linear,
                    extrapolation: Extrapolation::Hold,
                }),
            ],
        };
        manager
            .run(&mut w, &pipeline, &mut RunReport::default())
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let clips = w.iter::<Clip>().count();
        let bindings = w.iter::<ClipBinding>().count();
        let channels: usize = w.iter::<Clip>().map(|(_, c)| c.channels.len()).sum();
        println!("{name}: {clips} clips, {bindings} bindings, {channels} channels");
        assert_eq!(w.iter::<AnvilClipCandidates>().count(), 0, "{name}");
    }
}
