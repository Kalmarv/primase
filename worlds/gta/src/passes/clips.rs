//! `lower-gta-clips`: a clip dictionary's animations ([`GtaAnimation`]) as core clips on the skeleton they animate.
//!
//! An animation's bone tracks are, by track number:
//! - **0, 1, 2**: a bone's local translation, rotation (a unit quaternion) and scale: channels on the joint of the
//!   skeleton whose bone has the track's tag (`Property::Translation`, `Rotation`, `Scale`). A track of a bone the
//!   skeleton lacks is left out (counted).
//! - **5, 6**: the mover's translation and rotation, the clip's [`RootMotion`] (the first of each; the others are left
//!   out, counted).
//! - **every other number**: known only by its number, kept as controls (`Property::Scalar` on a
//!   [`ClipTarget::Control`] named `gta.track<number>.bone<tag>`, with `.<k>` for the k-th value of a track of several).
//!
//! Keys are the stored frames, evenly over `[0, duration]` (the last frame at `duration`), interpolated linearly
//! (rotations spherically); a channel whose every frame is the same keeps one key. Values are converted from GTA's
//! frame into the world's as a skeleton's rest is ([`Rest::converted`]). The extrapolation holds the end values: the
//! animation's own looping is not known.
use std::collections::BTreeSet;

use primase_core::asset::{Asset, Role};
use primase_core::clip::{
    Channel, Clip, ClipTarget, Extrapolation, Interpolation, Property, RootMotion, RootSpace,
};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::skeleton::{JointId, Rest, Skeleton};
use primase_core::{Entity, Name, World};

use crate::components::{GTA, GtaAnimation, GtaAnimationTrack, GtaSkeleton};

/// `lower-gta-clips` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerGtaClips;

/// The key times of an animation of `frames` frames over `duration` seconds; one key at 0 when it has one frame or no
/// duration.
fn key_times(frames: usize, duration: f32) -> Vec<f32> {
    if frames < 2 || duration <= 0.0 {
        return vec![0.0];
    }
    let last = frames - 1;
    (0..frames)
        .map(|i| {
            if i == last {
                duration
            } else {
                ((i as f64 * f64::from(duration)) / last as f64) as f32
            }
        })
        .collect()
}

/// A track's frames, or its first only when they are all the same.
fn frames_of(t: &GtaAnimationTrack) -> Vec<&[f32]> {
    let width = usize::from(t.width).max(1);
    let all: Vec<&[f32]> = t.samples.chunks(width).collect();
    match all.split_first() {
        Some((first, rest)) if rest.iter().all(|f| f == first) => vec![*first],
        _ => all,
    }
}

/// A value of a joint property converted from GTA's frame into the world's.
fn converted(property: Property, v: &[f32]) -> Vec<f32> {
    let mut trs = Rest::Trs {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };
    let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
    if let Rest::Trs {
        translation,
        rotation,
        scale,
    } = &mut trs
    {
        match property {
            Property::Translation => *translation = [at(0), at(1), at(2)],
            Property::Rotation => *rotation = [at(0), at(1), at(2), at(3)],
            Property::Scale => *scale = [at(0), at(1), at(2)],
            Property::Scalar | Property::Visibility | Property::Values(_) => {}
        }
    }
    match (property, trs.converted(&GTA)) {
        (Property::Translation, Rest::Trs { translation: t, .. }) => t.to_vec(),
        (Property::Rotation, Rest::Trs { rotation: r, .. }) => r.to_vec(),
        (Property::Scale, Rest::Trs { scale: s, .. }) => s.to_vec(),
        (Property::Scalar | Property::Visibility | Property::Values(_), _) => v.to_vec(),
        (Property::Translation | Property::Rotation | Property::Scale, Rest::Affine(_)) => {
            unreachable!("GTA signed axis permutation preserves TRS")
        }
    }
}

/// A unit quaternion from `q` (the zero quaternion becomes the identity).
fn unit(q: &mut [f32]) {
    let len = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if len > 0.0 {
        q.iter_mut().for_each(|x| *x /= len);
    } else if let Some(w) = q.get_mut(3) {
        *w = 1.0;
    }
}

/// A track's keys as `(times, values)` in the world's frame.
fn keys(t: &GtaAnimationTrack, property: Property, times: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut frames = frames_of(t);
    frames.truncate(times.len());
    let mut values = Vec::with_capacity(frames.len().saturating_mul(usize::from(t.width)));
    for f in &frames {
        let mut v = if property == Property::Scalar {
            f.to_vec()
        } else {
            converted(property, f)
        };
        if property == Property::Rotation {
            unit(&mut v);
        }
        values.extend(v);
    }
    let n = frames.len();
    (times.iter().copied().take(n).collect(), values)
}

/// `lower-gta-clips`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by the game's own rule.
pub const LOWER_GTA_CLIPS: PassSpec = PassSpec {
    name: "lower-gta-clips",
    options: &[],
    make: |_| Ok(Box::new(LowerGtaClips)),
};

impl Pass for LowerGtaClips {
    fn name(&self) -> &'static str {
        "lower-gta-clips"
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.animation", "core.skeleton"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.clip"]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.animation"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.asset"]
    }
    fn approximates(&self) -> &'static str {
        "holds the end values (an animation's looping is not known); a track of another number than 0, 1, 2, 5, 6 is a control"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let animations: Vec<(Entity, GtaAnimation)> =
            w.iter::<GtaAnimation>().map(|(e, a)| (e, a.clone())).collect();
        let mut made: Vec<(Entity, Entity)> = Vec::new();
        for (e, a) in animations {
            let skeleton = a.skeleton;
            let core = w
                .get::<Skeleton>(skeleton)
                .ok_or_else(|| format!("{skeleton:?} holds no core.skeleton (run lower-gta-skin first)"))?;
            let bones = w
                .get::<GtaSkeleton>(skeleton)
                .ok_or_else(|| format!("{skeleton:?} holds no gta.skeleton"))?;
            let frames = usize::try_from(a.frames).map_err(|e| e.to_string())?;
            let times = key_times(frames, a.duration);
            let mut channels: Vec<Channel> = Vec::new();
            let mut root = RootMotion {
                space: RootSpace::Rig,
                translation: None,
                rotation: None,
            };
            let mut seen: BTreeSet<(u16, u8)> = BTreeSet::new();
            for t in &a.tracks {
                if !seen.insert((t.bone, t.track)) {
                    log.count("tracks repeating a bone and track number, left out", 1);
                    continue;
                }
                match t.track {
                    0..=2 => {
                        let property = match t.track {
                            0 => Property::Translation,
                            1 => Property::Rotation,
                            _ => Property::Scale,
                        };
                        let joint = bones
                            .0
                            .bone_of_tag(t.bone)
                            .and_then(|i| u32::try_from(i).ok())
                            .map(JointId)
                            .filter(|j| core.index_of(*j).is_some());
                        let Some(joint) = joint else {
                            log.count("bone tracks of bones the skeleton lacks, left out", 1);
                            continue;
                        };
                        let (times, values) = keys(t, property, &times);
                        channels.push(Channel {
                            target: ClipTarget::Joint { skeleton, joint },
                            property,
                            interpolation: Interpolation::Linear,
                            times,
                            values,
                        });
                    }
                    5 | 6 => {
                        let (property, slot) = if t.track == 5 {
                            (Property::Translation, &mut root.translation)
                        } else {
                            (Property::Rotation, &mut root.rotation)
                        };
                        if slot.is_some() {
                            log.count("mover tracks past the first of their kind, left out", 1);
                            continue;
                        }
                        let (times, values) = keys(t, property, &times);
                        *slot = Some((Interpolation::Linear, times, values));
                    }
                    _ => {
                        let width = usize::from(t.width).max(1);
                        for k in 0..width {
                            let name = if width == 1 {
                                format!("gta.track{}.bone{:#06x}", t.track, t.bone)
                            } else {
                                format!("gta.track{}.bone{:#06x}.{k}", t.track, t.bone)
                            };
                            let mut frames = frames_of(t);
                            frames.truncate(times.len());
                            let values: Vec<f32> =
                                frames.iter().map(|f| f.get(k).copied().unwrap_or(0.0)).collect();
                            channels.push(Channel {
                                target: ClipTarget::Control { name },
                                property: Property::Scalar,
                                interpolation: Interpolation::Linear,
                                times: times.iter().copied().take(values.len()).collect(),
                                values,
                            });
                        }
                    }
                }
            }
            let name = w
                .get::<Name>(e)
                .map_or_else(|| format!("{:#010x}", a.hash), |n| n.0.clone());
            let has_root = root.translation.is_some() || root.rotation.is_some();
            w.remove::<GtaAnimation>(e);
            w.insert(
                e,
                Clip {
                    name,
                    duration: a.duration,
                    extrapolation: Extrapolation::Hold,
                    channels,
                    root_motion: has_root.then_some(root),
                    additive: None,
                },
            );
            log.count("clips made", 1);
            made.push((e, skeleton));
        }
        // a clip of an asset's rig is a member of the asset
        let assets: Vec<Entity> = w.iter::<Asset>().map(|(a, _)| a).collect();
        for asset in assets {
            let Some(members) = w.get_mut::<Asset>(asset).map(|a| &mut a.members) else {
                continue;
            };
            let rigs: Vec<Entity> = members
                .iter()
                .filter(|(_, r)| *r == Role::Rig)
                .map(|(m, _)| *m)
                .collect();
            for (clip, skeleton) in &made {
                if rigs.contains(skeleton) {
                    members.push((*clip, Role::Clip));
                    log.count("clips added to their rig's asset", 1);
                }
            }
        }
        Ok(())
    }
}
