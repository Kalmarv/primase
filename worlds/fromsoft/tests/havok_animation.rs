//! Every character animation binder of the install (`installs.sekiro`, with the install's own Oodle library
//! `tools.oodle6`) read as Havok animation containers.
//!
//! - H1: every `.hkx` of every `chr/c<id>.anibnd.dcx` is a tagfile whose `hkaAnimationContainer` reads. A file with a
//!   skeleton holds one and no animation; a file with an animation holds one animation and one binding that binds it,
//!   with a bone index for each transform track (the binding's track count is the animation's, as is its annotation
//!   track count), and, for a spline compressed one, a compressed block for each of its `numBlocks`, each starting
//!   inside the data. The bone indices are inside the skeleton the binding names (`originalSkeletonName`), which the
//!   binder holds. The counts by class and by type are printed.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use primase_fromsoft::{Archive, Binder, dcx::Decoder};
use primase_havok::animation::{Tracks, container};
use primase_havok::{Compendium, Tagfile};
use primase_testdata::{Game, Tool, install, tool};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[allow(unsafe_code)]
fn native(path: &std::path::Path) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: caller explicitly supplies a trusted native version-6 decoder.
    unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
}

#[test]
fn h1_every_character_animation_file_reads() -> Result<()> {
    let (Some(dir), Some(dll)) = (install(Game::Sekiro), tool(Tool::Oodle6)) else {
        return Ok(());
    };
    let oodle = native(&dll)?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1024 * 1024 * 1024,
    };
    let archive = Archive::open(dir)?;
    let mut binders = 0;
    let mut files = 0;
    let mut skeleton_files = 0;
    let mut bones = 0;
    let mut by_class = BTreeMap::<String, usize>::new();
    let mut by_type = BTreeMap::<String, usize>::new();
    let mut tracks = 0;
    let mut extracted = 0;
    let mut failures = Vec::new();
    for id in 0..10000 {
        let path = format!("/chr/c{id:04}.anibnd.dcx");
        if !archive.contains(&path) {
            continue;
        }
        binders += 1;
        let binder = Binder::read(&archive.read(&path, &decoder)?, &decoder)?;
        let compendium = match binder
            .files
            .iter()
            .find(|f| f.name.as_deref().is_some_and(|n| n.ends_with(".compendium")))
        {
            Some(f) => Some(Compendium::read(&decoder.decode_if_dcx(&f.bytes)?)?),
            None => None,
        };
        let mut skeleton_bones = BTreeMap::<String, usize>::new();
        let mut animations = Vec::new();
        for f in &binder.files {
            let name = f.name.clone().unwrap_or_default();
            if !name.to_lowercase().ends_with(".hkx") {
                continue;
            }
            files += 1;
            let bytes = decoder.decode_if_dcx(&f.bytes)?;
            let c = Tagfile::read(&bytes, compendium.as_ref())
                .map_err(|e| {
                    let sdk = bytes
                        .windows(4)
                        .position(|w| w == b"SDKV")
                        .and_then(|i| bytes.get(i + 4..i + 12))
                        .map(|v| String::from_utf8_lossy(v).into_owned());
                    format!("{e} (SDKV {sdk:?})")
                })
                .and_then(|tag| container(&tag).map_err(|e| e.to_string()));
            let c = match c {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{path} {name}: {e}"));
                    continue;
                }
            };
            if !c.skeletons.is_empty() {
                skeleton_files += 1;
                if c.skeletons.len() != 1 || !c.animations.is_empty() || !c.bindings.is_empty() {
                    failures.push(format!("{path} {name}: a skeleton file holding more"));
                }
                bones += c.skeletons[0].bones.len();
                skeleton_bones.insert(c.skeletons[0].name.clone(), c.skeletons[0].bones.len());
                continue;
            }
            if c.animations.len() != 1 || c.bindings.len() != 1 || c.bindings[0].animation != Some(0) {
                failures.push(format!(
                    "{path} {name}: {} animations, {} bindings",
                    c.animations.len(),
                    c.bindings.len()
                ));
                continue;
            }
            animations.push((name, c));
        }
        for (name, c) in animations {
            let a = &c.animations[0];
            let b = &c.bindings[0];
            *by_class.entry(a.class.clone()).or_default() += 1;
            *by_type.entry(format!("{:?}", a.kind)).or_default() += 1;
            tracks += a.transform_tracks;
            extracted += usize::from(a.extracted_motion.is_some());
            let mut bad = Vec::new();
            if b.transform_track_to_bone.len() != a.transform_tracks {
                bad.push("track count differs from the binding's");
            }
            if a.annotation_tracks.len() != a.transform_tracks {
                bad.push("annotation tracks differ from the transform tracks");
            }
            match skeleton_bones.get(&b.original_skeleton_name) {
                None => bad.push("the binding names a skeleton the binder does not hold"),
                Some(&n)
                    if b.transform_track_to_bone
                        .iter()
                        .any(|&i| i < 0 || i as usize >= n) =>
                {
                    bad.push("a track drives a bone outside the skeleton");
                }
                Some(_) => {}
            }
            if let Tracks::Spline(s) = &a.tracks
                && (s.block_offsets.len() != s.num_blocks as usize
                    || s.float_block_offsets.len() != s.num_blocks as usize
                    || s.block_offsets.iter().any(|&o| o as usize >= s.data.len()))
            {
                bad.push("spline blocks do not start inside the data");
            }
            if !bad.is_empty() {
                failures.push(format!("{path} {name}: {}", bad.join("; ")));
            }
        }
    }
    println!(
        "{binders} binders, {files} hkx files, {skeleton_files} skeletons ({bones} bones), {tracks} transform tracks, \
         {extracted} with extracted motion; classes {by_class:?}; types {by_type:?}"
    );
    assert!(binders > 0);
    assert!(
        failures.is_empty(),
        "{} failures, first: {:#?}",
        failures.len(),
        &failures[..failures.len().min(10)]
    );
    Ok(())
}
