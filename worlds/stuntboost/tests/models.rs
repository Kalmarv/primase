//! The models of the install (`PRIMASE_STUNTBOOST_DIR`, the install) through the glTF reader.
//!
//! - M1: every model the listing names reads, and the four that carry a skin read with the skeletons and clips the
//!   files hold. Counts taken from the files' own JSON, not from the reader: the skin's joints with every node above
//!   them (the skeleton the reader states), the animations, and their channels. Door 9 joints (8 in its skin), 1 clip of
//!   24 channels; Dragonfly 36 (35), 4 of 420; Siegfried 35 (34), 3 of 306; TestAnimation 4 (3), 6 of 54.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use primase_core::World;
use primase_stuntboost::{models, read_model};
use primase_testdata::real_data;

#[test]
fn m1_every_model_reads_and_the_skinned_ones_hold_their_joints_and_clips() {
    let Some(install) = real_data("PRIMASE_STUNTBOOST_DIR") else {
        return;
    };
    let listed = models(&install).unwrap();
    println!("{} models", listed.len());
    let mut skinned = 0;
    for name in &listed {
        let mut w = World::new();
        let r = read_model(&install, name, &mut w).unwrap_or_else(|e| panic!("{name}: {e}"));
        if r.gltf.skeletons > 0 {
            skinned += 1;
            println!(
                "{name}: {} skeletons, {} joints, {} clips, {} channels",
                r.gltf.skeletons, r.gltf.joints, r.gltf.clips, r.gltf.channels
            );
        }
    }
    assert!(skinned >= 4, "{skinned} models with a skeleton");
    for (name, joints, clips, channels) in [
        ("Door", 9, 1, 24),
        ("Dragonfly", 36, 4, 420),
        ("Siegfried", 35, 3, 306),
        ("TestAnimation", 4, 6, 54),
    ] {
        assert!(listed.iter().any(|n| n == name), "{name} is listed");
        let mut w = World::new();
        let r = read_model(&install, name, &mut w).unwrap();
        assert_eq!(
            (r.gltf.joints, r.gltf.clips, r.gltf.channels),
            (joints, clips, channels),
            "{name}"
        );
    }
}
