//! `lower-anvil-clips{interpolation=linear|step,extrapolation=hold|loop}`: an Animation read for a rig
//! ([`AnvilClip`]) becomes a `core.clip` on the rig's skeleton, and each Animation an Entity plays on its placed rig
//! ([`AnvilClipBinding`]) a `core.clip_binding`.
//!
//! - **Channels** (read): a track's bone is the CRC-32 of its name, the name a rig's joint carries
//!   (`lower-anvil-rig`, `lower-anvil-skeleton`); a track of kind 9 drives the joint's translation, one of kinds 3, 4,
//!   7 and 8 its rotation, each key at its time in seconds ([`StoredTrack::times`](crate::components::StoredTrack)) and
//!   in the world's frame (the reader's [`SourceFrame`](primase_core::frame::SourceFrame): a translation a point, a
//!   rotation conjugated as a skeleton's rest is). Every clip's tracks of the hash 0 are a translation and a rotation:
//!   the clip's **root motion**, in the rig's space (what the aerial clips move: 2.36 m up over the 2.3 s of a flip),
//!   which core states apart from the joints.
//! - **Duration**: the clip's `Length`, or its last key's time where that is later.
//! - **Interpolation** and **extrapolation** (stated, not read): the engine's between keys and outside them isn't
//!   established by the data (a track's kind may select the class the registry names `Linear`, `Nearest` or
//!   `Cardinal`), so both are the program's choice: `linear` (rotations by spherical interpolation) or `step`, and
//!   `hold` or `loop`.
//!
//! Left out, counted: the tracks of kinds not read (5, 16, 17, 18, 21, 22, 25: [`StoredTrack::values`](
//! crate::components::StoredTrack)), the tracks of a bone the rig has no joint for, the events, the `TTECLAnimation`
//! and the clip's other fields. A clip with no track it can lower is lowered with no channel.

use std::collections::BTreeMap;

use primase_core::clip::{
    Channel, Clip, ClipBinding, ClipTarget, Extrapolation, Interpolation, Property, RootMotion, RootSpace,
};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::rig::RigInstance;
use primase_core::skeleton::{JointId, Rest, Skeleton};
use primase_core::{Component, Entity, World};

use crate::components::{AnimationError, AnvilClip, AnvilClipBinding, StoredAnimation, Values};

/// How a clip's keys are interpolated and what it does outside them (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerAnvilClips {
    pub interpolation: Interpolation,
    pub extrapolation: Extrapolation,
}

impl LowerAnvilClips {
    /// From its options, both required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilClips, String> {
        if let Some(k) = o
            .keys()
            .find(|k| !["interpolation", "extrapolation"].contains(&k.as_str()))
        {
            return Err(format!("unknown option {k}"));
        }
        let interpolation = match o.get("interpolation").map(String::as_str) {
            Some("linear") => Interpolation::Linear,
            Some("step") => Interpolation::Step,
            Some(v) => return Err(format!("interpolation={v} is not linear or step")),
            None => return Err("interpolation is required: interpolation=linear|step".to_owned()),
        };
        let extrapolation = match o.get("extrapolation").map(String::as_str) {
            Some("hold") => Extrapolation::Hold,
            Some("loop") => Extrapolation::Loop,
            Some(v) => return Err(format!("extrapolation={v} is not hold or loop")),
            None => return Err("extrapolation is required: extrapolation=hold|loop".to_owned()),
        };
        Ok(LowerAnvilClips {
            interpolation,
            extrapolation,
        })
    }
}

/// A root-motion curve: its interpolation, key times and flattened values.
type Curve = (Interpolation, Vec<f32>, Vec<f32>);

/// A rotation `q` converted from `frame` into the world's, as a skeleton's rests are ([`Rest::converted`]).
fn rotation(frame: &primase_core::frame::SourceFrame, q: [f32; 4]) -> [f32; 4] {
    match (Rest::Trs {
        translation: [0.0; 3],
        rotation: q,
        scale: [1.0; 3],
    })
    .converted(frame)
    {
        Rest::Trs { rotation, .. } => rotation,
        Rest::Affine(_) => q,
    }
}

/// The clip `animation` as a `core.clip` for the skeleton `skeleton` (an entity holding `skeletons`' joints), with the
/// counts of what it leaves out.
fn lower(
    name: &str,
    animation: &StoredAnimation,
    frame: &primase_core::frame::SourceFrame,
    skeleton: (Entity, &Skeleton),
    how: &LowerAnvilClips,
    log: &mut PassLog,
) -> Result<Clip, String> {
    let (entity, joints) = skeleton;
    let by_name: BTreeMap<u32, JointId> = joints
        .joints
        .iter()
        .rev()
        .filter_map(|j| u32::from_str_radix(&j.name, 16).ok().map(|h| (h, j.id)))
        .collect();
    let mut channels = Vec::new();
    let mut root: (Option<Curve>, Option<Curve>) = (None, None);
    let mut last = 0.0f32;
    for (track, &hash) in animation.tracks.iter().zip(&animation.mappings) {
        let values = match track.values() {
            Ok(v) => v,
            Err(AnimationError::Unread { kind }) => {
                log.omit(&format!("tracks of kind {kind}, whose values aren't read"), 1);
                continue;
            }
            Err(e) => return Err(format!("{name}: {e}")),
        };
        let times = track.times();
        last = last.max(times.last().copied().unwrap_or(0.0));
        let (property, flat): (Property, Vec<f32>) = match values {
            Values::Vec3(v) => (
                Property::Translation,
                v.iter().flat_map(|&p| frame.point(p)).collect(),
            ),
            Values::Quaternions(q) => (
                Property::Rotation,
                q.iter().flat_map(|&q| rotation(frame, q)).collect(),
            ),
        };
        if hash == 0 {
            let curve = Some((how.interpolation, times, flat));
            match property {
                Property::Translation => root.0 = curve,
                _ => root.1 = curve,
            }
            continue;
        }
        let Some(&joint) = by_name.get(&hash) else {
            log.omit("tracks of a bone the rig has no joint for", 1);
            continue;
        };
        channels.push(Channel {
            target: ClipTarget::Joint {
                skeleton: entity,
                joint,
            },
            property,
            interpolation: how.interpolation,
            times,
            values: flat,
        });
    }
    if animation.events.count > 0 {
        log.omit(
            "animation events (their elements aren't read)",
            u64::from(animation.events.count),
        );
    }
    log.count(
        "channels lowered",
        u64::try_from(channels.len()).unwrap_or(u64::MAX),
    );
    Ok(Clip {
        name: name.to_owned(),
        duration: animation.length.max(last),
        extrapolation: how.extrapolation,
        channels,
        root_motion: (root.0.is_some() || root.1.is_some()).then_some(RootMotion {
            space: RootSpace::Rig,
            translation: root.0,
            rotation: root.1,
        }),
        additive: None,
    })
}

impl Pass for LowerAnvilClips {
    fn name(&self) -> &'static str {
        "lower-anvil-clips"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "interpolation".to_owned(),
                match self.interpolation {
                    Interpolation::Step => "step",
                    _ => "linear",
                }
                .to_owned(),
            ),
            (
                "extrapolation".to_owned(),
                match self.extrapolation {
                    Extrapolation::Loop => "loop",
                    Extrapolation::Hold => "hold",
                }
                .to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[
            AnvilClip::NAME,
            AnvilClipBinding::NAME,
            Skeleton::NAME,
            RigInstance::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[Clip::NAME, ClipBinding::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilClip::NAME, AnvilClipBinding::NAME]
    }
    fn approximates(&self) -> &'static str {
        "the keys' interpolation and what a clip does outside them a stated choice (the engine's isn't read)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let clips: Vec<Entity> = w.iter::<AnvilClip>().map(|(e, _)| e).collect();
        for e in clips {
            let Some(c) = w.remove::<AnvilClip>(e) else {
                continue;
            };
            let name = w
                .get::<primase_core::Name>(e)
                .map_or_else(String::new, |n| n.0.clone());
            let skeleton = w
                .get::<Skeleton>(c.rig)
                .cloned()
                .ok_or_else(|| format!("{name}: its rig {:?} holds no core.skeleton", c.rig))?;
            let clip = lower(&name, &c.animation, &c.frame, (c.rig, &skeleton), self, log)?;
            log.count("clips lowered", 1);
            w.insert(e, clip);
        }
        let bindings: Vec<Entity> = w.iter::<AnvilClipBinding>().map(|(e, _)| e).collect();
        for e in bindings {
            let Some(b) = w.remove::<AnvilClipBinding>(e) else {
                continue;
            };
            w.insert(
                e,
                ClipBinding {
                    clip: b.clip,
                    rig: Some(b.occurrence),
                    meshes: Vec::new(),
                },
            );
            log.count("clip bindings lowered", 1);
        }
        Ok(())
    }
}
