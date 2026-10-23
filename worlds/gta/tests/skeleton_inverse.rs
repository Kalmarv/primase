#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! A skeleton's stored inverse bind transforms against the rest poses it stores: for every bone of every skeleton of
//! the streamed ped fragments and the component ped dictionaries, the bone's pose (its transform into the drawable's
//! space, composed from the stored local transforms up the parents, `Skeleton::pose`) times its stored inverse is the
//! identity (each entry within `TOLERANCE`). The skeletons carry their inverses (the pointer at 0x28), the product
//! agrees with the poses the lights and fragments are placed by (checked against CodeWalker's frames), and the
//! counts and the worst deviation are printed.
//!
//! Needs the GTA install and its key material.
use primase_gta::rage::drawable::{DrawableDictionary, Fragment, Skeleton};
use primase_gta::read::archive::archive_set::Install;
use primase_gta::read::archive::keys::Keys;
use primase_testdata::{Game, install, keys};

/// Metres and unit-less rotation entries: measured at most 3.8e-6 over 96,126 bones of 966 skeletons.
const TOLERANCE: f32 = 1e-4;

/// `a` after `b`, both rows `[x, y, z, translation]` of an affine map on column vectors (`p' = A (p, 1)`).
fn after(a: &[[f32; 4]; 3], b: &[[f32; 4]; 3]) -> [[f32; 4]; 3] {
    std::array::from_fn(|r| {
        std::array::from_fn(|c| {
            let linear: f32 = (0..3).map(|k| a[r][k] * b[k][c]).sum();
            if c == 3 { linear + a[r][3] } else { linear }
        })
    })
}

#[derive(Default, Debug)]
struct Tally {
    skeletons: usize,
    bones: usize,
    without_inverse: usize,
    worst: f32,
    worst_at: String,
}

fn check(s: &Skeleton, file: &str, tally: &mut Tally) {
    tally.skeletons += 1;
    let Some(inverse) = &s.inverse else {
        tally.without_inverse += 1;
        return;
    };
    assert_eq!(inverse.len(), s.local.len(), "{file}: inverses per bone");
    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    for (bone, (pose, inv)) in s.pose().iter().zip(inverse).enumerate() {
        tally.bones += 1;
        let product = after(pose, inv);
        let off = product
            .iter()
            .flatten()
            .zip(identity.iter().flatten())
            .map(|(p, i)| (p - i).abs())
            .fold(0.0f32, f32::max);
        if off > tally.worst {
            tally.worst = off;
            tally.worst_at = format!("{file} bone {bone}");
        }
    }
}

#[test]
fn stored_inverse_binds_undo_the_stored_poses() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let keys = Keys::from_dir(&keys).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let mut tally = Tally::default();
    for f in install.listing() {
        if f.extension == "yft" && f.archive.contains("streamedpeds") {
            let r = install.resource(f.file, &keys).expect("resource");
            let frag = Fragment::read_resource(&r).unwrap_or_else(|e| panic!("{}: {e}", f.path));
            if let Some(s) = frag.drawable.as_ref().and_then(|d| d.skeleton.as_ref()) {
                check(s, f.path, &mut tally);
            }
        } else if f.extension == "ydd" && f.archive.contains("componentpeds") {
            let r = install.resource(f.file, &keys).expect("resource");
            let dict = DrawableDictionary::read_resource(&r).unwrap_or_else(|e| panic!("{}: {e}", f.path));
            for s in dict.drawables.iter().filter_map(|d| d.skeleton.as_ref()) {
                check(s, f.path, &mut tally);
            }
        }
    }
    println!("{tally:?}");
    assert!(tally.skeletons > 0 && tally.bones > 0);
    assert_eq!(tally.without_inverse, 0, "skeletons without inverse binds");
    assert!(
        tally.worst <= TOLERANCE,
        "pose x inverse is {} from the identity at {}",
        tally.worst,
        tally.worst_at
    );
}
