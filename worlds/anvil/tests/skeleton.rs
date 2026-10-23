//! A Skeleton body laid out by hand in the layout of Riders Republic's (docs/types.md "Skeleton"), two bones, one
//! pose group of one pose, and `lower-anvil-skeleton` on it: joints with their parents, rests and the hash names, the
//! frame conversion, the skeleton's own transform composed or refused.
use primase_anvil::components::AnvilSkeleton;
use primase_anvil::passes::LOWER_ANVIL_SKELETON;
use primase_anvil::read::skeleton::StoredSkeleton;
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::skeleton::{JointId, Rest, Skeleton};
use primase_core::{Component, World};

fn object(id: u64, kind: u32) -> Vec<u8> {
    let mut b = id.to_le_bytes().to_vec();
    b.extend(kind.to_le_bytes());
    b
}

fn f32x4(v: [f32; 4]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// One bone: its object (tag 0), name hash, parent (a pointer: none or a reference), mirror (none), the display pointer
/// (none), the four f32 x 4, the byte, the enum, no modifiers, no hashes, the second enum, the float and the key.
fn bone(id: u64, hash: u32, parent: Option<u64>, local: ([f32; 4], [f32; 4]), key: u32) -> Vec<u8> {
    let mut b = vec![0];
    b.extend(object(id, 0x9574_1049));
    b.extend(hash.to_le_bytes());
    match parent {
        None => b.push(3),
        Some(p) => {
            b.push(2);
            b.extend(p.to_le_bytes());
        }
    }
    b.push(3); // the mirror
    b.push(3); // BoneDisplayData
    b.extend(f32x4([0.0; 4])); // global position
    b.extend(f32x4([0.0, 0.0, 0.0, 1.0])); // global rotation
    b.extend(f32x4(local.0));
    b.extend(f32x4(local.1));
    b.push(0);
    b.extend(5u32.to_le_bytes());
    b.extend(0u32.to_le_bytes()); // BoneModifier count
    b.extend(0u32.to_le_bytes()); // the u32 array
    b.extend(7u32.to_le_bytes());
    b.extend(1.0f32.to_le_bytes());
    b.extend(key.to_le_bytes());
    b
}

/// A reference as the forge stores one: a tag and an id in 10 bytes.
const REFERENCE: [u8; 10] = [1, 1, 0x66, 0xB5, 0xAD, 0x34, 0xD0, 1, 0, 0];

fn skeleton(own: ([f32; 4], [f32; 4])) -> Vec<u8> {
    let mut b = vec![1];
    b.extend(8u32.to_le_bytes());
    b.extend(2u32.to_le_bytes());
    b.extend(bone(
        0x100,
        0x07C1_59A2,
        None,
        ([0.0; 4], [0.0, 0.0, 0.0, 1.0]),
        0x0001_0000,
    ));
    // a child 2 m up its parent's z, turned a quarter about x
    let q = std::f32::consts::FRAC_1_SQRT_2;
    b.extend(bone(
        0x101,
        0x94DB_AC3E,
        Some(0x100),
        ([0.0, 0.0, 2.0, 0.0], [q, 0.0, 0.0, q]),
        1,
    ));
    b.push(2); // the root: a reference to the first bone
    b.extend(0x100u64.to_le_bytes());
    b.extend(f32x4(own.0));
    b.extend(f32x4(own.1));
    b.extend(REFERENCE); // BodyPartMapping
    // one pose group of one pose of one pose bone
    b.extend(1u32.to_le_bytes());
    b.extend(object(0x200, 0xC8F8_ABFF));
    b.extend(1u32.to_le_bytes());
    b.extend(object(0x201, 0x209C_F072));
    b.extend(0.5f32.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    b.extend(object(0x202, 0x071C_EE50));
    b.extend([7u8; 16]);
    b.push(3); // IKData: none
    b.extend(REFERENCE); // LookAt
    b.push(3); // ReflexSystem
    b.extend(1.0f32.to_le_bytes());
    b.extend(2.0f32.to_le_bytes());
    b.push(0); // Reflex3SkeletonConstraints: an object with 8 bytes
    b.extend(object(0x300, 0x8E3F_B47A));
    b.extend(8u32.to_le_bytes());
    b.extend([9u8; 8]);
    b.extend(1u32.to_le_bytes());
    b.extend(0x94DB_AC3Eu32.to_le_bytes());
    b
}

const IDENTITY: ([f32; 4], [f32; 4]) = ([0.0; 4], [0.0, 0.0, 0.0, 1.0]);

#[test]
fn a_skeleton_reads_to_its_last_byte() {
    let body = skeleton(IDENTITY);
    let s = StoredSkeleton::parse(&body).expect("skeleton");
    assert_eq!((s.kind, s.bones.len(), s.root), (8, 2, Some(0x100)));
    assert_eq!(s.bones[0].name_hash, 0x07C1_59A2);
    assert_eq!(s.bones[1].parent, Some(0x100));
    assert_eq!(s.bones[1].local_position, [0.0, 0.0, 2.0, 0.0]);
    assert_eq!(s.bones[1].key & 0xFFFF, 1);
    assert_eq!(s.pose_groups.len(), 1);
    assert_eq!(s.pose_groups[0].poses[0].bones[0].bytes, [7; 16]);
    assert_eq!(s.pose_groups[0].poses[0].value, 0.5);
    assert_eq!(s.reflex3.as_ref().map(|r| r.1.len()), Some(8));
    assert_eq!(s.hashes, vec![0x94DB_AC3E]);
    assert!(s.ik_data.is_none());
    let mut long = body.clone();
    long.push(0);
    assert!(StoredSkeleton::parse(&long).is_err());
    assert!(StoredSkeleton::parse(&body[..body.len() - 1]).is_err());
}

fn lowered(own: ([f32; 4], [f32; 4]), frame: SourceFrame, own_transform: &str) -> Result<Skeleton, String> {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(
        e,
        AnvilSkeleton {
            frame,
            skeleton: StoredSkeleton::parse(&skeleton(own)).expect("skeleton"),
        },
    );
    let pass = LOWER_ANVIL_SKELETON.instance(&Options::from([(
        "own-transform".to_owned(),
        own_transform.to_owned(),
    )]))?;
    pass.run(&mut w, &mut PassLog::default())?;
    assert!(w.get::<AnvilSkeleton>(e).is_none());
    w.get::<Skeleton>(e)
        .cloned()
        .ok_or_else(|| "no skeleton".to_owned())
}

#[test]
fn the_joints_have_parents_rests_and_hash_names() {
    let s = lowered(IDENTITY, SourceFrame::WORLD, "refuse").expect("lowered");
    assert_eq!(s.joints.len(), 2);
    assert_eq!(
        (s.joints[0].name.as_str(), s.joints[0].parent),
        ("07c159a2", None)
    );
    assert_eq!(
        (s.joints[1].name.as_str(), s.joints[1].parent),
        ("94dbac3e", Some(JointId(0)))
    );
    let Rest::Trs {
        translation,
        rotation,
        scale,
    } = s.joints[1].rest
    else {
        panic!("not a TRS");
    };
    assert_eq!(translation, [0.0, 0.0, 2.0]);
    assert_eq!(scale, [1.0; 3]);
    assert!((rotation[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!(Skeleton::NAME == "core.skeleton");
}

#[test]
fn the_frame_converts_the_rests() {
    // ENU: the source's x east, y north, z up become x, z, -y
    let s = lowered(IDENTITY, SourceFrame::ENU, "refuse").expect("lowered");
    let Rest::Trs { translation, .. } = s.joints[1].rest else {
        panic!("not a TRS");
    };
    assert_eq!(translation, [0.0, 2.0, -0.0]);
}

#[test]
fn an_own_transform_is_composed_into_the_roots_or_refused() {
    let own = ([10.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]);
    let error = lowered(own, SourceFrame::WORLD, "refuse").expect_err("refused");
    assert!(error.contains("own-transform=refuse"), "{error}");
    let s = lowered(own, SourceFrame::WORLD, "compose").expect("composed");
    let Rest::Trs { translation, .. } = s.joints[0].rest else {
        panic!("not a TRS");
    };
    assert_eq!(translation, [10.0, 0.0, 0.0]);
    // ignored: the root's rest as stored
    let s = lowered(own, SourceFrame::WORLD, "ignore").expect("ignored");
    let Rest::Trs { translation, .. } = s.joints[0].rest else {
        panic!("not a TRS");
    };
    assert_eq!(translation, [0.0, 0.0, 0.0]);
    // the child's rest is its parent's space and keeps
    let Rest::Trs { translation, .. } = s.joints[1].rest else {
        panic!("not a TRS");
    };
    assert_eq!(translation, [0.0, 0.0, 2.0]);
}

/// `replace` makes the root's rest the own position and rotation (the engine's runtime pose), whatever the root stored,
/// and leaves the rest of the tree.
#[test]
fn an_own_transform_replaces_the_roots_rest() {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let own = ([10.0, 0.0, 0.964, 0.0], [0.0, 0.0, half, half]);
    let s = lowered(own, SourceFrame::WORLD, "replace").expect("replaced");
    let Rest::Trs {
        translation,
        rotation,
        ..
    } = s.joints[0].rest
    else {
        panic!("not a TRS");
    };
    assert_eq!(translation, [10.0, 0.0, 0.964]);
    assert_eq!(rotation, [0.0, 0.0, half, half]);
    // a child keeps its stored rest
    let Rest::Trs { translation, .. } = s.joints[1].rest else {
        panic!("not a TRS");
    };
    assert_eq!(translation, [0.0, 0.0, 2.0]);
    // the identity as own: the root's rest is the identity, not what it stored
    let s = lowered(IDENTITY, SourceFrame::WORLD, "replace").expect("replaced");
    let Rest::Trs { rotation, .. } = s.joints[0].rest else {
        panic!("not a TRS");
    };
    assert_eq!(rotation, [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn the_option_is_required() {
    assert!(LOWER_ANVIL_SKELETON.instance(&Options::new()).is_err());
    assert!(
        LOWER_ANVIL_SKELETON
            .instance(&Options::from([("own-transform".to_owned(), "x".to_owned())]))
            .is_err()
    );
}
