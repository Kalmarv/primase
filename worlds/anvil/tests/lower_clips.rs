//! `lower-anvil-clips` on a clip and rig built by hand (a joint named for a bone's CRC-32, tracks of each kind read, one
//! unread and one of a bone the rig lacks, the root-motion pair), and on real Entities of Riders Republic's forges that
//! own a SkeletonComponent and an AnimComponent, placed through the Placer and lowered under the pass manager with its
//! checks on.
use anvil_formats::animation::StoredAnimation;
use anvil_formats::animation::StoredEvents;
use anvil_formats::animation::StoredTrack;
use anvil_formats::animation::Times;
use anvil_formats::archive::Archive;
use anvil_formats::listing::Kind;
use primase_anvil::components::{AnvilClip, AnvilClipBinding};
use primase_anvil::passes::{
    LOWER_ANVIL_CLIPS, LowerAnvilClips, LowerAnvilLods, LowerAnvilMesh, LowerAnvilRig, LowerAnvilSkin,
    OwnTransform, Static16Normal,
};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::clip::{Clip, ClipBinding, ClipTarget, Extrapolation, Interpolation, Property};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::rig::RigInstance;
use primase_core::skeleton::{Joint, JointId, Rest, Skeleton};
use primase_core::{Entity, Name, Transform, World};
use primase_testdata::{Game, install};

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn track(tag: u8, keys: u32, times: Times, values: Vec<u8>) -> StoredTrack {
    StoredTrack {
        tag,
        size: 0,
        keys,
        times,
        values,
    }
}

/// A clip of 1.0 s over the rig's joint `0xA` and the root: tracks, in order, the joint's translation (three keys at 0,
/// 0.5 and 1.0 s), its rotation (the identity, one key), the root's translation and rotation (one key each), a bone
/// the rig lacks, and a track of the unread kind 25.
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
        mappings: vec![0xA, 0xA, 0, 0, 0xFFFF, 0xA],
        track_data_tail: 0,
        ecl: None,
        buffer: Vec::new(),
        cb4e: 0,
        tracks: vec![
            track(
                27,
                3,
                Times::U8(vec![30, 60]),
                floats(&[0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 2.0, 4.0, 6.0]),
            ),
            track(12, 1, Times::U8(vec![]), vec![0xFF, 0xBF, 0xFF, 0xBF, 0xFF, 0x3F]),
            track(27, 1, Times::U8(vec![]), floats(&[0.0, 0.0, 0.0])),
            track(24, 1, Times::U8(vec![]), floats(&[0.0, 0.0, 0.0])),
            track(27, 1, Times::U8(vec![]), floats(&[9.0, 9.0, 9.0])),
            track(75, 1, Times::U8(vec![]), vec![0; 8]),
        ],
    }
}

struct Built {
    w: World,
    rig: Entity,
    clip: Entity,
    binding: Entity,
}

fn built(frame: SourceFrame) -> Built {
    let mut w = World::new();
    let rig = w.spawn();
    w.insert(
        rig,
        Skeleton {
            joints: vec![Joint {
                id: JointId(7),
                name: "0000000a".to_owned(),
                parent: None,
                rest: Rest::Trs {
                    translation: [0.0; 3],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                },
            }],
        },
    );
    let occurrence = w.spawn();
    w.insert(occurrence, RigInstance { skeleton: rig });
    w.insert(
        occurrence,
        Transform {
            cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]],
        },
    );
    let clip = w.spawn();
    w.insert(clip, Name("walk".to_owned()));
    w.insert(
        clip,
        AnvilClip {
            frame,
            id: 0x1234,
            animation: animation(),
            rig,
        },
    );
    let binding = w.spawn();
    w.insert(binding, AnvilClipBinding { clip, occurrence });
    Built {
        w,
        rig,
        clip,
        binding,
    }
}

fn lower(w: &mut World, interpolation: Interpolation, extrapolation: Extrapolation) -> Result<(), String> {
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![Box::new(LowerAnvilClips {
            interpolation,
            extrapolation,
        })],
    };
    manager
        .run(w, &pipeline, &mut RunReport::default())
        .map_err(|e| format!("{e:#}"))
}

#[test]
fn a_clip_lowers_to_channels_by_bone_name_and_root_motion_and_leaves_out_what_it_cannot() {
    let mut b = built(SourceFrame::WORLD);
    lower(&mut b.w, Interpolation::Linear, Extrapolation::Hold).expect("lower");
    let clip = b.w.get::<Clip>(b.clip).expect("clip");
    assert_eq!(
        (clip.name.as_str(), clip.duration, clip.extrapolation),
        ("walk", 1.0, Extrapolation::Hold)
    );
    // the joint's translation and rotation; the unknown bone and the unread kind left out
    assert_eq!(clip.channels.len(), 2);
    let t = &clip.channels[0];
    assert_eq!(
        (&t.target, t.property, t.interpolation),
        (
            &ClipTarget::Joint {
                skeleton: b.rig,
                joint: JointId(7)
            },
            Property::Translation,
            Interpolation::Linear
        )
    );
    assert_eq!(t.times, [0.0, 0.5, 1.0]);
    assert_eq!(t.values, [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 2.0, 4.0, 6.0]);
    let r = &clip.channels[1];
    assert_eq!(r.property, Property::Rotation);
    assert!((r.values[3] - 1.0).abs() < 1e-3);
    // the root: a translation and a rotation, in the rig's space
    let root = clip.root_motion.as_ref().expect("root motion");
    assert!(root.translation.is_some() && root.rotation.is_some());
    // the binding follows the occurrence
    let binding = b.w.get::<ClipBinding>(b.binding).expect("binding");
    assert_eq!(binding.clip, b.clip);
    assert!(binding.rig.is_some() && binding.meshes.is_empty());
}

#[test]
fn the_interpolation_and_extrapolation_are_the_programs_and_required() {
    let mut b = built(SourceFrame::WORLD);
    lower(&mut b.w, Interpolation::Step, Extrapolation::Loop).expect("lower");
    let clip = b.w.get::<Clip>(b.clip).expect("clip");
    assert_eq!(clip.extrapolation, Extrapolation::Loop);
    assert!(
        clip.channels
            .iter()
            .all(|c| c.interpolation == Interpolation::Step)
    );
    assert!(LOWER_ANVIL_CLIPS.instance(&Options::new()).is_err());
    let one = Options::from([("interpolation".to_owned(), "linear".to_owned())]);
    assert!(LOWER_ANVIL_CLIPS.instance(&one).is_err());
    let bad = Options::from([
        ("interpolation".to_owned(), "cubic".to_owned()),
        ("extrapolation".to_owned(), "hold".to_owned()),
    ]);
    assert!(LOWER_ANVIL_CLIPS.instance(&bad).is_err());
}

#[test]
fn the_readers_frame_converts_translations_and_rotations() {
    // ENU: x east, y north, z up become x, z, -y
    let mut b = built(SourceFrame::ENU);
    lower(&mut b.w, Interpolation::Linear, Extrapolation::Hold).expect("lower");
    let clip = b.w.get::<Clip>(b.clip).expect("clip");
    let t = &clip.channels[0];
    assert_eq!(&t.values[3..6], [1.0, 3.0, -2.0]);
}

/// A clip longer than its Length (a key past it) lowers with the later duration.
#[test]
fn a_key_past_the_length_extends_the_duration() {
    let mut b = built(SourceFrame::WORLD);
    b.w.get_mut::<AnvilClip>(b.clip).expect("clip").animation.length = 0.75;
    lower(&mut b.w, Interpolation::Linear, Extrapolation::Hold).expect("lower");
    assert_eq!(b.w.get::<Clip>(b.clip).expect("clip").duration, 1.0);
}

/// Real Entities that own a rig and play Animations: a valve prop (two clips) and a bike cinematic (seven).
#[test]
fn real_entities_place_their_rigs_and_clips_and_lower() {
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
    for name in [
        "MAD_PRP_AREA52_ZombieGas_Valve_Open_Zion",
        "MAD_CHR_Bike_DH_COM_SupremeDHTeam_U_001",
    ] {
        let at = archive
            .listing()
            .find(|(_, l)| l.kind == Some(Kind::Entity) && l.name == name)
            .map(|(forge, l)| anvil_formats::archive::At {
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
        );
        placer.place(at, &mut placed).expect("place");
        assert!(placed.refused.is_empty(), "{name}: {:?}", placed.refused);
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
        let roots = w.iter::<Clip>().filter(|(_, c)| c.root_motion.is_some()).count();
        println!("{name}: {clips} clips, {bindings} bindings, {channels} channels, {roots} with root motion");
        assert!(clips > 0 && bindings > 0, "{name}");
    }
}
