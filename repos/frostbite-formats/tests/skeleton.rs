//! Skeleton assets and the skeleton a skinned mesh's bone lists address: on the skate. install (and its Oodle 9
//! library), the character skeleton `animation/dingo/animbase_default_skeleton` read from the globals bundle, and the
//! hat bucket's meshes (bone lists of 386 bones) linked to it by the resolve pass; and the reader's checks on a
//! skeleton by itself.

#![allow(clippy::unwrap_used)]

use frostbite_formats::skeleton::{SkeletonJoint, name_hash, verify_hierarchy};

fn joint(name: &str, parent: Option<u32>) -> SkeletonJoint {
    SkeletonJoint {
        name: name.to_owned(),
        name_hash: name_hash(name),
        parent,
        child: None,
        sibling: None,
        kind: 1,
    }
}

#[test]
fn a_joint_name_is_hashed_with_fnv1_and_the_parents_may_not_lead_back() {
    // the hashes the skeleton asset of the game stores for three of its joints
    assert_eq!(name_hash("Reference"), 3_124_388_676);
    assert_eq!(name_hash("AITrajectory"), 1_180_344_196);
    assert_eq!(name_hash("Head"), 806_557_287);
    let chain = [joint("a", None), joint("b", Some(0)), joint("c", Some(1))];
    assert!(verify_hierarchy(&chain).is_ok());
    let cycle = [joint("a", Some(2)), joint("b", Some(0)), joint("c", Some(1))];
    assert!(verify_hierarchy(&cycle).unwrap_err().contains("lead back"));
    let past = [joint("a", Some(5))];
    assert!(verify_hierarchy(&past).is_err());
}
