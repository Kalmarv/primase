//! The models of the install (`installs.stuntboost`, the install) as the documents they store.
//!
//! - M1: every model the listing names unwraps to a glTF document with its buffers, and the four that carry a skin
//!   hold the joints, clips and channels counted from the document itself (`primase-glb`'s reading of its JSON; no
//!   world is read): Door 8 joints in its skin, 1 clip of 24 channels; Dragonfly 35, 4 of 420; Siegfried 34, 3 of 306;
//!   TestAnimation 3, 6 of 54. The skeleton the glTF reader states is these joints with every node above them.
//! - M2: the images a model names are the install's own files, each decoding to a PNG.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use primase_stuntboost::{model_gltf, model_image, models};
use primase_testdata::{Game, install};

#[test]
fn m1_every_model_unwraps_and_the_skinned_ones_hold_their_joints_and_clips() {
    let Some(install) = install(Game::Stuntboost) else {
        return;
    };
    let listed = models(&install).unwrap();
    println!("{} models", listed.len());
    let mut skinned = 0;
    for name in &listed {
        let gltf = model_gltf(&install, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        if !gltf.skins().unwrap().is_empty() {
            skinned += 1;
        }
    }
    assert_eq!(skinned, 4);
    for (name, joints, clips, channels) in [
        ("Door", 8, 1, 24),
        ("Dragonfly", 35, 4, 420),
        ("Siegfried", 34, 3, 306),
        ("TestAnimation", 3, 6, 54),
    ] {
        assert!(listed.iter().any(|n| n == name), "{name} is listed");
        let gltf = model_gltf(&install, name).unwrap();
        let skins = gltf.skins().unwrap();
        let animations = gltf.animations().unwrap();
        assert_eq!(
            (
                skins.iter().map(|s| s.joints.len()).sum::<usize>(),
                animations.len(),
                animations.iter().map(|a| a.channels.len()).sum::<usize>()
            ),
            (joints, clips, channels),
            "{name}"
        );
    }
}

#[test]
fn m2_the_images_a_model_names_decode_to_png() {
    let Some(install) = install(Game::Stuntboost) else {
        return;
    };
    let mut images = 0;
    for name in models(&install).unwrap() {
        let gltf = model_gltf(&install, &name).unwrap();
        let uris: Vec<String> = gltf.json()["images"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["uri"].as_str().map(str::to_owned))
            .collect();
        for uri in uris {
            let png = model_image(&install, &uri).unwrap_or_else(|e| panic!("{name}: {uri}: {e}"));
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "{name}: {uri}");
            images += 1;
        }
    }
    println!("{images} images");
    assert!(images > 0);
}
