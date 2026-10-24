//! Every character animation binder of the install (`installs.sekiro`, with the install's own Oodle library
//! `tools.oodle6`) read as Havok animation containers.
//!
//! - H1: every `.hkx` of every `chr/c<id>.anibnd.dcx` is a tagfile whose `hkaAnimationContainer` reads. A file with a
//!   skeleton holds one and no animation; a file with an animation holds one animation and one binding that binds it,
//!   with a bone index for each transform track (the binding's track count is the animation's, as is its annotation
//!   track count), and, for a spline compressed one, a compressed block for each of its `numBlocks`, each starting
//!   inside the data. The bone indices are inside the skeleton the binding names (`originalSkeletonName`), which the
//!   binder holds. The counts by class and by type are printed.
//!
//!   The install has one exception, named in `KNOWN` and checked to be exactly it: one file of `c1090` is a tagfile of
//!   SDK 2015.1 (the reader knows 2016.2 and 2018.1). A track bound to a negative bone index drives no bone (two files
//!   of `c7300` have one).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use primase_fromsoft::{Archive, Binder, dcx::Decoder};
use primase_havok::animation::{Tracks, container};
use primase_havok::{Compendium, Tagfile};
use primase_testdata::{Game, Tool, install, oodle};

/// The files the census expects not to read cleanly, by file name, and why.
const KNOWN: [(&str, &str); 1] = [(r"c1090\hkx\a000_205000.hkx", "unsupported SDK version")];

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[allow(unsafe_code)]
fn native(path: &primase_testdata::OodleLibrary) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: caller explicitly supplies a trusted native version-6 decoder.
    unsafe { primase_compress::oodle::Oodle::open(path.path(), primase_compress::oodle::Version::V6) }
}

#[test]
fn h1_every_character_animation_file_reads() -> Result<()> {
    let (Some(dir), Some(dll)) = (install(Game::Sekiro), oodle(Tool::Oodle6)) else {
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
                        .any(|&i| i >= 0 && i as usize >= n) =>
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
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| {
            !KNOWN
                .iter()
                .any(|(file, why)| f.contains(file) && f.contains(why))
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "{} unexpected: {unexpected:#?}",
        unexpected.len()
    );
    assert_eq!(
        failures.len(),
        KNOWN.len(),
        "the known exceptions all happen: {failures:#?}"
    );
    Ok(())
}

/// H2: every spline compressed animation of those binders decodes (`SplineAnimation::parse`: each block must end
/// where the data says the float data begins and then at the next block, so a wrong layout cannot pass by luck), and at
/// its first frame, its middle and its end every bone's pose is finite with a unit rotation. The numbers printed: the
/// animations, blocks and tracks decoded, the largest rotation norm error, and how many absent rotations there are
/// and in how many of those the bone's reference rotation is not the identity (the identity stands in for an absent
/// rotation, so those would be wrong).
#[test]
fn h2_every_spline_animation_decodes() -> Result<()> {
    use primase_havok::spline::{Absent, Component, Rotation, SplineAnimation};

    /// What the statistics below say an absent component is: zero, the identity, one.
    const ABSENT: Absent = Absent::NEUTRAL;

    let (Some(dir), Some(dll)) = (install(Game::Sekiro), oodle(Tool::Oodle6)) else {
        return Ok(());
    };
    let oodle = native(&dll)?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1024 * 1024 * 1024,
    };
    let archive = Archive::open(dir)?;
    let mut decoded = 0;
    let mut blocks = 0;
    let mut tracks = 0;
    let mut absent_rotations = 0;
    let mut tracks_with_reference = 0;
    // by position component x y z: the components of the first blocks' tracks, those a track does not state, and of
    // each the ones whose reference value is not zero (whether an absent position means the reference or zero)
    let mut position_all = [0usize; 3];
    let mut position_all_nonzero = [0usize; 3];
    let mut position_absent = [0usize; 3];
    let mut position_absent_nonzero = [0usize; 3];
    let mut reference_not_identity = 0;
    let mut absent_with_reference = 0;
    let mut worst_norm = 0.0f32;
    let mut failures = Vec::new();
    for id in 0..10000 {
        let path = format!("/chr/c{id:04}.anibnd.dcx");
        if !archive.contains(&path) {
            continue;
        }
        let binder = Binder::read(&archive.read(&path, &decoder)?, &decoder)?;
        let compendium = match binder
            .files
            .iter()
            .find(|f| f.name.as_deref().is_some_and(|n| n.ends_with(".compendium")))
        {
            Some(f) => Some(Compendium::read(&decoder.decode_if_dcx(&f.bytes)?)?),
            None => None,
        };
        let mut skeletons = BTreeMap::new();
        let mut animations = Vec::new();
        for f in &binder.files {
            let name = f.name.clone().unwrap_or_default();
            if !name.to_lowercase().ends_with(".hkx") {
                continue;
            }
            let bytes = decoder.decode_if_dcx(&f.bytes)?;
            let Ok(c) = Tagfile::read(&bytes, compendium.as_ref())
                .map_err(|e| e.to_string())
                .and_then(|tag| container(&tag).map_err(|e| e.to_string()))
            else {
                continue; // H1 reports the files that do not read
            };
            if let Some(s) = c.skeletons.first() {
                skeletons.insert(s.name.clone(), s.clone());
            } else if c.animations.len() == 1 && c.bindings.len() == 1 {
                animations.push((name, c));
            }
        }
        for (name, c) in animations {
            if !matches!(c.animations[0].tracks, Tracks::Spline(_)) {
                continue;
            }
            let spline = match SplineAnimation::parse(&c.animations[0]) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("{path} {name}: {e}"));
                    continue;
                }
            };
            decoded += 1;
            blocks += spline.blocks.len();
            let Some(skeleton) = skeletons.get(&c.bindings[0].original_skeleton_name) else {
                continue;
            };
            for (track, &bone) in c.bindings[0].transform_track_to_bone.iter().enumerate() {
                if let Some(reference) = usize::try_from(bone)
                    .ok()
                    .and_then(|b| skeleton.reference_pose.get(b))
                {
                    if let Some(t) = spline.blocks[0].get(track) {
                        for (k, component) in [&t.position.x, &t.position.y, &t.position.z]
                            .into_iter()
                            .enumerate()
                        {
                            let nonzero = usize::from(reference.translation[k].abs() > 1e-4);
                            position_all[k] += 1;
                            position_all_nonzero[k] += nonzero;
                            if matches!(component, Component::Absent) {
                                position_absent[k] += 1;
                                position_absent_nonzero[k] += nonzero;
                            }
                        }
                    }
                    let r = reference.rotation;
                    tracks_with_reference += 1;
                    reference_not_identity += usize::from(
                        (r[3].abs() - 1.0).abs() > 1e-4 || r[0].abs() + r[1].abs() + r[2].abs() > 1e-4,
                    );
                }
                if let (Some(Rotation::Absent), Some(reference)) = (
                    spline.blocks[0].get(track).map(|t| &t.rotation),
                    usize::try_from(bone)
                        .ok()
                        .and_then(|b| skeleton.reference_pose.get(b)),
                ) {
                    absent_rotations += 1;
                    let r = reference.rotation;
                    if (r[3].abs() - 1.0).abs() > 1e-4 || r[0].abs() + r[1].abs() + r[2].abs() > 1e-4 {
                        absent_with_reference += 1;
                    }
                }
            }
            tracks += c.animations[0].transform_tracks * spline.blocks.len();
            for time in [0.0, c.animations[0].duration / 2.0, c.animations[0].duration] {
                match spline.pose_at(time, &c.bindings[0], skeleton, ABSENT) {
                    Err(e) => failures.push(format!("{path} {name}: at {time}: {e}")),
                    Ok(pose) => {
                        for t in &pose {
                            let finite = t
                                .translation
                                .iter()
                                .chain(&t.rotation)
                                .chain(&t.scale)
                                .all(|v| v.is_finite());
                            let norm = t.rotation.iter().map(|v| v * v).sum::<f32>().sqrt();
                            worst_norm = worst_norm.max((norm - 1.0).abs());
                            if !finite {
                                failures.push(format!("{path} {name}: a pose at {time} is not finite"));
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{decoded} spline animations decoded: {blocks} blocks, {tracks} track-blocks; worst rotation norm error \
         {worst_norm}; {absent_rotations} absent rotations, {absent_with_reference} of them on a bone whose reference \
         rotation is not the identity; of the {tracks_with_reference} tracks of the first blocks, \
         {reference_not_identity} are on such a bone; position components (x y z): {position_all:?} in all, \
         {position_all_nonzero:?} with a nonzero reference, {position_absent:?} absent, {position_absent_nonzero:?} \
         of those with a nonzero reference"
    );
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| {
            !KNOWN
                .iter()
                .any(|(file, why)| f.contains(file) && f.contains(why))
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "{} unexpected: {:#?}",
        unexpected.len(),
        &unexpected[..unexpected.len().min(10)]
    );
    assert!(worst_norm < 1e-3, "a rotation is {worst_norm} from unit length");
    Ok(())
}
