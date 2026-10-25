//! Havok animation data in a world: a skeleton as `core.skeleton` ([`insert_skeleton`]), an animation as it is stored
//! (`havok.animation`, [`insert_animation`]), and the pass that makes clips of the animations a program asks for
//! (`lower-havok-clips`, [`LowerHavokClips`]).
//!
//! Reading takes no choice: the skeleton is the file's, each animation keeps its decoded blocks and its binding. The
//! source's frame is the caller's (a reader knows its game's), given to both, so the skeleton and the clips that drive
//! it are in the world's frame. Lowering states what the format leaves open: what stands in for a component a track
//! does not state (`absent`, from [`crate::spline::Absent`]; the source's files say neutral, see [`crate::spline`]),
//! how many keys a clip has for each frame the animation was authored at (`rate`), and which animations (`clips`).
//! The spline an animation is evaluated as continuously is sampled into keys, and between keys `core.clip`
//! interpolates, not the spline: the pass reports that as an approximation. A channel whose values are one value all
//! along is one key.
//!
//! An animation's extracted motion (`hkaDefaultAnimatedReferenceFrame`: one sample a frame, `x y z` and an angle about
//! the stored `up` axis, each the frame's whole motion from the start) is the clip's root motion when the program
//! says so (`root-motion`). Its samples are the clip's keys as stored, and the clip's linear interpolation between
//! them is the animation's own (a lerp of the sample, an angle about one axis), so nothing of it is approximated; what
//! is a convention is the sign of the angle, which the program's choice (`full`, not `translation`) takes from the
//! SDK's axis and angle and which the census of the install does not settle (see [`RootMotionUse`]).

use primase_core::frame::SourceFrame;
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassRegistry, PassSpec};
use primase_core::{
    AdditiveReference, Channel, Clip, ClipTarget, Component, Entity, Extrapolation, Interpolation, Joint,
    JointId, Name, Property, Rest, RootMotion, RootSpace, Skeleton as CoreSkeleton, World,
};

use crate::animation::{Binding, Container, QsTransform, ReferenceFrame, Skeleton, Tracks};
use crate::invalid;
use crate::spline::{Absent, Fallback, SplineAnimation};

/// `havok.animation`: a spline compressed animation as stored (its decoded blocks, its binding's track to bone table,
/// the reference pose of the skeleton it binds, its extracted motion), on an entity with `core.name`.
#[derive(Clone, Debug)]
pub struct HavokAnimation {
    /// The entity holding the `core.skeleton` made of the Havok skeleton this animation binds.
    pub skeleton: Entity,
    pub spline: SplineAnimation,
    /// One a transform track: the bone it drives, or negative for none.
    pub track_to_bone: Vec<i16>,
    /// The skeleton's reference transform of each bone, in the file's frame (what [`Fallback::Reference`] means).
    pub reference_pose: Vec<QsTransform>,
    /// The animation's reference frame motion (root motion), as stored; not lowered.
    pub extracted_motion: Option<ReferenceFrame>,
    /// `hkaAnimationBinding::blendHint` (0: normal).
    pub blend_hint: u32,
    /// The file's frame in the world's: what the lowered clips are converted by.
    pub frame: SourceFrame,
}

impl Component for HavokAnimation {
    const NAME: &'static str = "havok.animation";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.skeleton);
    }
    fn payload_bytes(&self) -> u64 {
        // the blocks' control points dominate: counted as the decoded values
        self.spline
            .blocks
            .iter()
            .flatten()
            .map(|t| {
                let vector = |v: &crate::spline::Vector| {
                    [&v.x, &v.y, &v.z]
                        .into_iter()
                        .map(|c| match c {
                            crate::spline::Component::Spline(p) => p.len() as u64 * 4,
                            _ => 4,
                        })
                        .sum::<u64>()
                };
                vector(&t.position)
                    + vector(&t.scale)
                    + match &t.rotation {
                        crate::spline::Rotation::Spline { points, .. } => points.len() as u64 * 16,
                        _ => 16,
                    }
            })
            .sum()
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    /// The skeleton entity holds a `core.skeleton` with a joint for each bone of the reference pose, and the track
    /// table names only bones of it (or none).
    fn verify(&self, _e: Entity, w: &World) -> std::result::Result<(), String> {
        let skeleton = w
            .get::<CoreSkeleton>(self.skeleton)
            .ok_or_else(|| format!("{:?} holds no core.skeleton", self.skeleton))?;
        if skeleton.joints.len() != self.reference_pose.len() {
            return Err(format!(
                "{} joints, {} reference transforms",
                skeleton.joints.len(),
                self.reference_pose.len()
            ));
        }
        if let Some(bone) = self
            .track_to_bone
            .iter()
            .find(|&&b| usize::try_from(b).is_ok_and(|b| b >= skeleton.joints.len()))
        {
            return Err(format!("a track drives bone {bone}, outside the skeleton"));
        }
        Ok(())
    }
}

/// A Havok skeleton as a `core.skeleton` in the world's frame (`frame` is the file's): joint `i` is bone `i` with its
/// stored name, parent and reference transform. The entity also has `core.name`.
pub fn insert_skeleton(w: &mut World, s: &Skeleton, frame: &SourceFrame) -> crate::Result<Entity> {
    let mut joints = Vec::with_capacity(s.bones.len());
    for (i, bone) in s.bones.iter().enumerate() {
        let q = s
            .reference_pose
            .get(i)
            .ok_or_else(|| invalid(0, format!("bone {i} has no reference transform")))?;
        let parent = match s.parents.get(i) {
            Some(&p) if p < 0 => None,
            Some(&p) if (p as usize) < s.bones.len() => Some(JointId(p as u32)),
            _ => return Err(invalid(0, format!("bone {i}: parent outside the skeleton"))),
        };
        joints.push(Joint {
            id: JointId(i as u32),
            name: bone.name.clone(),
            parent,
            rest: Rest::Trs {
                translation: [q.translation[0], q.translation[1], q.translation[2]],
                rotation: q.rotation,
                scale: [q.scale[0], q.scale[1], q.scale[2]],
            }
            .converted(frame),
        });
    }
    let e = w.spawn();
    w.insert(e, CoreSkeleton { joints });
    w.insert(e, Name(s.name.clone()));
    Ok(e)
}

/// The animation a container holds (one animation and the binding that binds it) as a `havok.animation` entity named
/// `name`, bound to `skeleton` (the entity [`insert_skeleton`] made of `havok`). `None` for an animation that is not
/// spline compressed (its tracks are not read here).
pub fn insert_animation(
    w: &mut World,
    name: &str,
    container: &Container,
    skeleton: Entity,
    havok: &Skeleton,
    frame: &SourceFrame,
) -> crate::Result<Option<Entity>> {
    let ([animation], [binding]) = (container.animations.as_slice(), container.bindings.as_slice()) else {
        return Err(invalid(
            0,
            format!(
                "{name}: {} animations and {} bindings, one of each expected",
                container.animations.len(),
                container.bindings.len()
            ),
        ));
    };
    if binding.animation != Some(0) {
        return Err(invalid(
            0,
            format!("{name}: the binding does not bind its animation"),
        ));
    }
    if !matches!(animation.tracks, Tracks::Spline(_)) {
        return Ok(None);
    }
    let spline = SplineAnimation::parse(animation).map_err(|e| name_the_bone(e, binding, havok))?;
    if binding.transform_track_to_bone.len() != animation.transform_tracks {
        return Err(invalid(
            0,
            format!("{name}: track count differs from the binding's"),
        ));
    }
    if let Some(b) = binding
        .transform_track_to_bone
        .iter()
        .find(|&&b| usize::try_from(b).is_ok_and(|b| b >= havok.bones.len()))
    {
        return Err(invalid(
            0,
            format!("{name}: a track drives bone {b}, outside the skeleton"),
        ));
    }
    let e = w.spawn();
    w.insert(
        e,
        HavokAnimation {
            skeleton,
            spline,
            track_to_bone: binding.transform_track_to_bone.clone(),
            reference_pose: havok.reference_pose.clone(),
            extracted_motion: animation.extracted_motion.clone(),
            blend_hint: binding.blend_hint,
            frame: *frame,
        },
    );
    w.insert(e, Name(name.to_owned()));
    Ok(Some(e))
}

/// A spline error that names a track ("block 0 track 119 scale: ...") also names the bone `binding` has that track
/// drive in `havok`, so the refusal of an animation says which bone's data it is.
fn name_the_bone(e: crate::Error, binding: &Binding, havok: &Skeleton) -> crate::Error {
    let bone = e
        .detail
        .split_once(" track ")
        .and_then(|(_, rest)| rest.split(' ').next())
        .and_then(|t| t.parse::<usize>().ok())
        .and_then(|t| binding.transform_track_to_bone.get(t))
        .and_then(|&b| usize::try_from(b).ok())
        .and_then(|b| havok.bones.get(b));
    match bone {
        Some(bone) => invalid(
            e.offset,
            format!("{} (the track drives the bone {})", e.detail, bone.name),
        ),
        None => e,
    }
}

/// Which animations `lower-havok-clips` lowers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clips {
    All,
    /// By `core.name`, each of them must exist.
    Named(Vec<String>),
}

/// What `lower-havok-clips` makes of an animation's extracted motion (root motion).
///
/// `translation` lowers the path (`x y z` of every sample, in the world's frame) and no turn; `full` also lowers the
/// rotation, taking a sample's fourth value as an angle in radians about the stored `up` axis, a right-handed turn as
/// the SDK's axis and angle quaternion is. The data's own sign tests do not decide that convention (the heading of a
/// step against the angle: 222 steps follow +angle and 380 follow -angle of 602, and 9 of 41 animations with a turning
/// frame have a slope of +1, 3 of -1, the rest none), so `full` is a choice the program states, not a default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootMotionUse {
    /// The extracted motion is left out of the clip and counted.
    Leave,
    Translation,
    Full,
}

/// `lower-havok-clips{absent=neutral|reference,rate=N,clips=all|NAME;NAME,root-motion=leave|translation|full}` (all
/// required): each selected
/// `havok.animation` also gets a `core.clip` (its name, its duration, hold at the ends) with a joint channel for each
/// property of each track bound to a bone: translation, rotation and scale of the bone's joint, in the world's frame.
/// `absent` is what stands in for a component a track does not state (zero, the identity and one, or the bone's
/// reference transform); `rate` is the keys for each frame the animation was authored at (1 to 16): each key is the
/// spline sampled at its time, and a channel whose values do not change is a single key. Approximation: between keys
/// the clip interpolates (linearly, rotations by slerp), the spline is evaluated at the keys only.
///
/// An animation whose blend hint is 1 (additive) is lowered with `additive: Identity`: its tracks' values are deltas, not
/// absolute transforms. Measured on the source's c3000 (146 such animations, 3,950 tracks, 721 animations as control): of
/// the stated positions that differ by more than 2 cm between the reference pose and zero, all 1,524 are nearer zero; of
/// the stated rotations that differ by more than 10 degrees between the reference and the identity, 900 of 918 are nearer
/// the identity (the 575 normal animations: 9,017 of 10,424 positions and 12,696 of 13,323 rotations nearer the reference).
/// What a track does not state is then the neutral value (zero, the identity, one) whatever `absent` says: the reference
/// transform is not a delta. That is an assumption: that an untracked component of an additive animation is no delta
/// (the files state no scale at all, and the positions or rotations of many tracks are absent), which the counted
/// approximation names. Not settled: the side a Havok delta composes on (core states `R_base * R_clip`; the Havok
/// SDK headers found, 2009 `NORMAL = 0, ADDITIVE = 1` and Skyrim SE's `kNormal, kAdditive`, state no side, and no source
/// found names a later `ADDITIVE_DEPRECATED`). Any other blend hint is left out and reported as unsupported (`omit`).
/// `root-motion` is [`RootMotionUse`]: what is not lowered of an animation's extracted motion is counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerHavokClips {
    pub absent: Fallback,
    pub rate: u32,
    pub clips: Clips,
    pub root_motion: RootMotionUse,
}

impl Choice for Fallback {
    const VALUES: &'static [(&'static str, Self)] =
        &[("neutral", Fallback::Neutral), ("reference", Fallback::Reference)];
}

impl Choice for RootMotionUse {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("leave", RootMotionUse::Leave),
        ("translation", RootMotionUse::Translation),
        ("full", RootMotionUse::Full),
    ];
}

/// `lower-havok-clips`' declaration ([`PassRegistry::add`]): every option required (see [`LowerHavokClips`]).
pub const LOWER_HAVOK_CLIPS: PassSpec = PassSpec {
    name: "lower-havok-clips",
    options: &[
        OptionSpec::choice::<Fallback>(
            "absent",
            "what stands in for a component a track does not state: zero, the identity and one, or the bone's \
             reference transform",
        ),
        OptionSpec::required(
            "rate",
            "1",
            "keys for each frame the animation was authored at, 1 to 16",
        ),
        OptionSpec::required(
            "clips",
            "all",
            "all, or the animations' core.names, `;` between, each of which must exist",
        ),
        OptionSpec::choice::<RootMotionUse>(
            "root-motion",
            "what of an animation's extracted motion is lowered: none (counted), its path, or its path and turn",
        ),
    ],
    make: lower_havok_clips,
};

fn lower_havok_clips(g: &Given) -> std::result::Result<Box<dyn Pass>, String> {
    let v = g.required("rate");
    let rate = v
        .parse::<u32>()
        .ok()
        .filter(|r| (1..=16).contains(r))
        .ok_or_else(|| format!("rate={v}: a whole number of keys a frame, 1 to 16"))?;
    let v = g.required("clips");
    let clips = if v == "all" {
        Clips::All
    } else {
        let names: Vec<String> = v.split(';').map(str::trim).map(str::to_owned).collect();
        if names.iter().any(String::is_empty) {
            return Err(format!("clips={v}: an empty name"));
        }
        Clips::Named(names)
    };
    Ok(Box::new(LowerHavokClips {
        absent: g.choice::<Fallback>("absent"),
        rate,
        clips,
        root_motion: g.choice::<RootMotionUse>("root-motion"),
    }))
}

/// A transform in the world's frame, as the clip's channels hold it.
struct Key {
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

fn convert(t: &QsTransform, frame: &SourceFrame) -> Key {
    match (Rest::Trs {
        translation: [t.translation[0], t.translation[1], t.translation[2]],
        rotation: t.rotation,
        scale: [t.scale[0], t.scale[1], t.scale[2]],
    })
    .converted(frame)
    {
        Rest::Trs {
            translation,
            rotation,
            scale,
        } => Key {
            translation,
            rotation,
            scale,
        },
        // converted() keeps the TRS form
        Rest::Affine(_) => Key {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
    }
}

/// The key times in seconds of an animation of `frames` frames `frame_duration` apart, `rate` a frame: strictly
/// increasing, the last the duration.
fn key_times(frames: u32, frame_duration: f32, rate: u32, duration: f32) -> Vec<f32> {
    let count = (frames.max(1) - 1) * rate + 1;
    let mut times: Vec<f32> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let t = ((f64::from(i) * f64::from(frame_duration)) / f64::from(rate)) as f32;
        let t = if i + 1 == count { duration } else { t.min(duration) };
        if times.last().is_none_or(|&p| t > p) {
            times.push(t);
        }
    }
    times
}

/// A channel of `values` (`width` a key); one key at 0 when every key is the same value.
fn channel(target: ClipTarget, property: Property, times: &[f32], values: Vec<f32>, width: usize) -> Channel {
    let constant = values.chunks(width).all(|k| k == &values[..width]);
    if constant {
        Channel {
            target,
            property,
            interpolation: Interpolation::Linear,
            times: vec![0.0],
            values: values[..width].to_vec(),
        }
    } else {
        Channel {
            target,
            property,
            interpolation: Interpolation::Linear,
            times: times.to_vec(),
            values,
        }
    }
}

/// The clip's root motion from the animation's extracted motion: one key a stored sample, at `i * duration / (n - 1)`.
fn root_motion(
    r: &ReferenceFrame,
    frame: &SourceFrame,
    clip_duration: f32,
    full: bool,
    name: &str,
) -> std::result::Result<RootMotion, String> {
    let n = r.samples.len();
    if n == 0 {
        return Err(format!("{name}: the extracted motion has no samples"));
    }
    if r.duration > clip_duration * (1.0 + 1e-4) || r.duration < 0.0 || !r.duration.is_finite() {
        return Err(format!(
            "{name}: the extracted motion lasts {} s, the animation {clip_duration} s",
            r.duration
        ));
    }
    let up = [r.up[0], r.up[1], r.up[2]];
    let length = up.iter().map(|v| v * v).sum::<f32>().sqrt();
    if full && (length < 1e-6 || (length - 1.0).abs() > 1e-3) {
        return Err(format!(
            "{name}: the extracted motion's up axis {up:?} is not a unit vector"
        ));
    }
    let times: Vec<f32> = if n == 1 {
        vec![0.0]
    } else {
        (0..n)
            .map(|i| {
                let t = ((i as f64 * f64::from(r.duration)) / (n - 1) as f64) as f32;
                t.min(clip_duration)
            })
            .collect()
    };
    let (mut translation, mut rotation) = (Vec::with_capacity(3 * n), Vec::<f32>::with_capacity(4 * n));
    for sample in &r.samples {
        let angle = f64::from(sample[3]);
        let (s, c) = ((angle / 2.0).sin() as f32, (angle / 2.0).cos() as f32);
        let q = if full {
            [up[0] * s, up[1] * s, up[2] * s, c]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        let mut key = convert(
            &QsTransform {
                translation: [sample[0], sample[1], sample[2], 0.0],
                rotation: q,
                scale: [1.0, 1.0, 1.0, 0.0],
            },
            frame,
        );
        if rotation.len() >= 4 {
            let previous = &rotation[rotation.len() - 4..];
            if previous
                .iter()
                .zip(&key.rotation)
                .map(|(p, q)| p * q)
                .sum::<f32>()
                < 0.0
            {
                key.rotation = key.rotation.map(|v| -v);
            }
        }
        translation.extend(key.translation);
        rotation.extend(key.rotation);
    }
    Ok(RootMotion {
        space: RootSpace::Rig,
        translation: Some((Interpolation::Linear, times.clone(), translation)),
        rotation: full.then_some((Interpolation::Linear, times, rotation)),
    })
}

fn lower(
    a: &HavokAnimation,
    name: &str,
    pass: &LowerHavokClips,
    additive: bool,
) -> std::result::Result<Clip, String> {
    let s = &a.spline;
    let times = key_times(s.num_frames, s.frame_duration, pass.rate, s.duration);
    // a delta's absent component is no change, not the reference transform
    let fallback = if additive { Fallback::Neutral } else { pass.absent };
    let absent = Absent {
        position: fallback,
        rotation: fallback,
        scale: fallback,
    };
    let mut channels = Vec::new();
    for (track, &bone) in a.track_to_bone.iter().enumerate() {
        let Some(reference) = usize::try_from(bone).ok().and_then(|b| a.reference_pose.get(b)) else {
            continue; // a negative bone: the track drives none
        };
        let target = ClipTarget::Joint {
            skeleton: a.skeleton,
            joint: JointId(bone as u32),
        };
        let (mut translation, mut rotation, mut scale) = (Vec::new(), Vec::<f32>::new(), Vec::new());
        for &t in &times {
            let sample = s
                .sample(track, t, reference, absent)
                .map_err(|e| format!("{name}: {e}"))?;
            let mut key = convert(&sample, &a.frame);
            // the same rotation by its nearer sign, so a slerp between keys takes the short way
            if rotation.len() >= 4 {
                let previous = &rotation[rotation.len() - 4..];
                if previous
                    .iter()
                    .zip(&key.rotation)
                    .map(|(p, q)| p * q)
                    .sum::<f32>()
                    < 0.0
                {
                    key.rotation = key.rotation.map(|v| -v);
                }
            }
            translation.extend(key.translation);
            rotation.extend(key.rotation);
            scale.extend(key.scale);
        }
        channels.push(channel(
            target.clone(),
            Property::Translation,
            &times,
            translation,
            3,
        ));
        channels.push(channel(target.clone(), Property::Rotation, &times, rotation, 4));
        channels.push(channel(target, Property::Scale, &times, scale, 3));
    }
    Ok(Clip {
        name: name.to_owned(),
        duration: s.duration,
        extrapolation: Extrapolation::Hold,
        channels,
        root_motion: match (&a.extracted_motion, pass.root_motion) {
            (Some(r), RootMotionUse::Translation | RootMotionUse::Full) => Some(root_motion(
                r,
                &a.frame,
                s.duration,
                pass.root_motion == RootMotionUse::Full,
                name,
            )?),
            _ => None,
        },
        additive: additive.then_some(AdditiveReference::Identity),
    })
}

impl Pass for LowerHavokClips {
    fn name(&self) -> &'static str {
        "lower-havok-clips"
    }
    fn options(&self) -> Options {
        Options::from([
            ("absent".to_owned(), self.absent.text().to_owned()),
            ("rate".to_owned(), self.rate.to_string()),
            ("root-motion".to_owned(), self.root_motion.text().to_owned()),
            (
                "clips".to_owned(),
                match &self.clips {
                    Clips::All => "all".to_owned(),
                    Clips::Named(n) => n.join(";"),
                },
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[HavokAnimation::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Clip::NAME]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[HavokAnimation::NAME]
    }
    fn approximates(&self) -> &'static str {
        "clip keys are the spline sampled at a fixed rate; between keys core.clip interpolates, not the spline"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let all: Vec<(Entity, String)> = w
            .iter::<HavokAnimation>()
            .map(|(e, _)| (e, w.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_default()))
            .collect();
        let chosen: Vec<&(Entity, String)> = match &self.clips {
            Clips::All => all.iter().collect(),
            Clips::Named(names) => {
                for n in names {
                    if !all.iter().any(|(_, a)| a == n) {
                        return Err(format!(
                            "clips: no havok.animation named {n} ({} animations)",
                            all.len()
                        ));
                    }
                }
                all.iter().filter(|(_, n)| names.contains(n)).collect()
            }
        };
        let mut lowered = 0;
        let mut additive_lowered = 0;
        for (e, name) in chosen {
            let a = w
                .get::<HavokAnimation>(*e)
                .cloned()
                .ok_or_else(|| format!("{name}: no havok.animation"))?;
            let additive = match a.blend_hint {
                0 => false,
                // the track values of a blend hint 1 animation are deltas from the identity (see the pass's documentation)
                1 => true,
                hint => {
                    log.omit(
                        &format!("animations with blend hint {hint}, whose values are not known to be absolute or deltas"),
                        1,
                    );
                    continue;
                }
            };
            // one animation that cannot be lowered (a sample that fails) is left without a clip and reported with its reason:
            // the others are still lowered
            let clip = match lower(&a, name, self, additive) {
                Ok(clip) => clip,
                Err(why) => {
                    let why = why.strip_prefix(&format!("{name}: ")).unwrap_or(&why);
                    log.omit(&format!("animations not lowered to clips: {why}"), 1);
                    continue;
                }
            };
            for u in &a.spline.unreadable {
                let bone = a
                    .track_to_bone
                    .get(u.track)
                    .and_then(|&b| usize::try_from(b).ok())
                    .and_then(|b| w.get::<CoreSkeleton>(a.skeleton).and_then(|s| s.joints.get(b)))
                    .map_or_else(|| format!("track {}", u.track), |j| j.name.clone());
                log.count(
                    &format!(
                        "{} of {bone} in {name}: a stored value or range is NaN or infinite, read as absent (as `absent` says)",
                        u.part
                    ),
                    1,
                );
            }
            if a.extracted_motion.is_some() {
                match self.root_motion {
                    RootMotionUse::Leave => {
                        log.count("clips whose extracted motion (root motion) is left out", 1);
                    }
                    RootMotionUse::Translation => {
                        log.count("clips with root motion lowered as a path (its turn left out)", 1);
                    }
                    RootMotionUse::Full => {
                        log.count(
                            "clips with root motion lowered with its turn (angle about up as the SDK's axis and angle; the sign is not settled by the data)",
                            1,
                        );
                    }
                }
            }
            if clip.additive.is_some() {
                additive_lowered += 1;
            }
            w.insert(*e, clip);
            lowered += 1;
        }
        if additive_lowered > 0 {
            log.approximate(
                concat!(
                    "additive animations (blend hint 1) lowered as deltas from the identity, composed as core.clip states; ",
                    "a component a track does not state is taken as no delta, and the side a Havok delta composes on ",
                    "is not verified"
                ),
                additive_lowered,
            );
        }
        if lowered > 0 {
            log.approximate(
                &format!(
                    "clip keys sampled from the spline at {} per frame; between keys interpolated by core.clip, \
                     not by the spline",
                    self.rate
                ),
                lowered,
            );
        }
        Ok(())
    }
}

/// Register this crate's passes by their declarations: [`LOWER_HAVOK_CLIPS`].
pub fn register(reg: &mut PassRegistry) {
    reg.add(&LOWER_HAVOK_CLIPS);
}

#[cfg(test)]
mod tests {
    use primase_core::frame::Axis;
    use primase_core::pass::{PassManager, RunReport};

    use super::*;
    use crate::spline::{Component as C, Knots, Rotation, Track, Vector};

    fn vector(x: C, y: C, z: C, knots: Option<Knots>) -> Vector {
        Vector { knots, x, y, z }
    }

    fn rest(translation: [f32; 3]) -> QsTransform {
        QsTransform {
            translation: [translation[0], translation[1], translation[2], 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0, 0.0],
        }
    }

    /// Three frames a second apart. Track 0 drives bone 1: x ramps 0 to 10 over the animation (two control points, degree
    /// 1), y is static 2, z absent; the rotation is a spline from the identity to a quarter turn about y; the scale is
    /// absent. Track 1 drives no bone.
    fn world(frame: SourceFrame, hint: u32) -> (World, Entity, Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            CoreSkeleton {
                joints: (0..2)
                    .map(|i| Joint {
                        id: JointId(i),
                        name: format!("b{i}"),
                        parent: (i > 0).then(|| JointId(i - 1)),
                        rest: Rest::Trs {
                            translation: [0.0; 3],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0; 3],
                        },
                    })
                    .collect(),
            },
        );
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let moving = Track {
            position: vector(
                C::Spline(vec![0.0, 10.0]),
                C::Static(2.0),
                C::Absent,
                Some(Knots {
                    degree: 1,
                    values: vec![0, 0, 2, 2],
                }),
            ),
            rotation: Rotation::Spline {
                knots: Knots {
                    degree: 1,
                    values: vec![0, 0, 2, 2],
                },
                points: vec![[0.0, 0.0, 0.0, 1.0], [0.0, q, 0.0, q]],
            },
            scale: vector(C::Absent, C::Absent, C::Absent, None),
        };
        let idle = Track {
            position: vector(C::Absent, C::Absent, C::Absent, None),
            rotation: Rotation::Absent,
            scale: vector(C::Absent, C::Absent, C::Absent, None),
        };
        let e = w.spawn();
        w.insert(
            e,
            HavokAnimation {
                skeleton,
                spline: SplineAnimation {
                    blocks: vec![vec![moving, idle]],
                    unreadable: vec![],
                    duration: 2.0,
                    num_frames: 3,
                    max_frames_per_block: 256,
                    frame_duration: 1.0,
                },
                track_to_bone: vec![1, -1],
                reference_pose: vec![rest([0.0; 3]), rest([5.0, 6.0, 7.0])],
                extracted_motion: None,
                blend_hint: hint,
                frame,
            },
        );
        w.insert(e, Name("a000_000000".into()));
        (w, e, skeleton)
    }

    fn run(w: &mut World, options: &[(&str, &str)]) -> std::result::Result<PassLog, String> {
        let mut options: Options = options.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        // the tests that are not about root motion leave it out
        options
            .entry("root-motion".into())
            .or_insert_with(|| "leave".into());
        let pass = LOWER_HAVOK_CLIPS.instance(&options)?;
        let mut log = PassLog::default();
        pass.run(w, &mut log)?;
        Ok(log)
    }

    fn property(clip: &Clip, p: Property) -> &Channel {
        clip.channels.iter().find(|c| c.property == p).unwrap()
    }

    #[test]
    fn a_clip_has_a_channel_for_each_property_of_each_bound_track_and_samples_the_spline_at_its_keys() {
        let (mut w, e, skeleton) = world(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0), 0);
        let log = run(&mut w, &[("absent", "neutral"), ("rate", "1"), ("clips", "all")]).unwrap();
        let clip = w.get::<Clip>(e).unwrap();
        assert_eq!((clip.name.as_str(), clip.duration), ("a000_000000", 2.0));
        // track 1 drives no bone: three channels of track 0 only
        assert_eq!(clip.channels.len(), 3);
        let target = ClipTarget::Joint {
            skeleton,
            joint: JointId(1),
        };
        assert!(clip.channels.iter().all(|c| c.target == target));
        let position = property(clip, Property::Translation);
        // x ramps, y static, z absent: neutral zero
        assert_eq!(position.times, [0.0, 1.0, 2.0]);
        assert_eq!(position.values, [0.0, 2.0, 0.0, 5.0, 2.0, 0.0, 10.0, 2.0, 0.0]);
        let rotation = property(clip, Property::Rotation);
        assert_eq!(rotation.times, [0.0, 1.0, 2.0]);
        // the middle key is the blend of the control points, normalised: an eighth of a turn about y
        let eighth = (std::f32::consts::PI / 8.0).sin();
        assert!(
            (rotation.values[5] - eighth).abs() < 1e-6,
            "{:?}",
            rotation.values
        );
        // an absent scale is one value all along: one key
        let scale = property(clip, Property::Scale);
        assert_eq!(
            (scale.times.as_slice(), scale.values.as_slice()),
            ([0.0].as_slice(), [1.0, 1.0, 1.0].as_slice())
        );
        assert!(
            log.approximations
                .keys()
                .any(|k| k.contains("sampled from the spline at 1 per frame"))
        );
        assert_eq!(log.approximations.values().sum::<u64>(), 1);
    }

    /// A blend hint 1 animation is a clip of deltas: `additive: Identity`, its values as sampled, and what a track does not
    /// state is the neutral value even where the caller asks for the reference (z stays 0, not the reference's 7).
    /// An animation that cannot be sampled (its track list names a track the blocks do not hold) is left without a clip and
    /// reported once with its reason; the animation beside it is still lowered.
    #[test]
    fn one_animation_that_fails_to_sample_does_not_stop_the_others() {
        let (mut w, good, _) = world(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0), 0);
        let bad = w.spawn();
        let mut broken = w.get::<HavokAnimation>(good).unwrap().clone();
        broken.track_to_bone = vec![1, -1, 1];
        w.insert(bad, broken);
        w.insert(bad, Name("a000_000001".into()));
        let log = run(&mut w, &[("absent", "neutral"), ("rate", "1"), ("clips", "all")]).unwrap();
        assert!(w.has::<Clip>(good));
        assert!(!w.has::<Clip>(bad));
        let reasons: Vec<&String> = log
            .omitted
            .keys()
            .filter(|k| k.starts_with("animations not lowered to clips"))
            .collect();
        assert!(
            reasons.len() == 1 && reasons[0].contains("no track 2") && !reasons[0].contains("a000_000001"),
            "{:?}",
            log.omitted
        );
        assert_eq!(log.omitted[reasons[0]], 1);
        assert_eq!(
            log.approximations.values().sum::<u64>(),
            1,
            "only the clip that was lowered is counted"
        );
    }

    #[test]
    fn a_blend_hint_one_animation_is_lowered_as_deltas_from_the_identity() {
        let frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0);
        let (mut w, e, _) = world(frame, 1);
        let log = run(
            &mut w,
            &[("absent", "reference"), ("rate", "1"), ("clips", "all")],
        )
        .unwrap();
        let clip = w.get::<Clip>(e).unwrap();
        assert_eq!(clip.additive, Some(AdditiveReference::Identity));
        let position = property(clip, Property::Translation);
        assert_eq!(position.values, [0.0, 2.0, 0.0, 5.0, 2.0, 0.0, 10.0, 2.0, 0.0]);
        assert_eq!(property(clip, Property::Scale).values, [1.0, 1.0, 1.0]);
        assert!(log.omitted.is_empty());
        assert_eq!(
            log.approximations
                .iter()
                .find(|(k, _)| k.contains("lowered as deltas from the identity"))
                .map(|(_, n)| *n),
            Some(1)
        );
        // a normal animation of the same data is not additive
        let (mut w, e, _) = world(frame, 0);
        run(
            &mut w,
            &[("absent", "reference"), ("rate", "1"), ("clips", "all")],
        )
        .unwrap();
        assert_eq!(w.get::<Clip>(e).unwrap().additive, None);
    }

    #[test]
    fn the_rate_is_keys_for_each_frame_and_absent_is_the_callers_choice() {
        let (mut w, e, _) = world(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0), 0);
        run(
            &mut w,
            &[("absent", "reference"), ("rate", "2"), ("clips", "a000_000000")],
        )
        .unwrap();
        let clip = w.get::<Clip>(e).unwrap();
        let position = property(clip, Property::Translation);
        assert_eq!(position.times, [0.0, 0.5, 1.0, 1.5, 2.0]);
        // z is the reference's 7, not zero, at every key; x at the half frame is 2.5
        assert_eq!(position.values[3], 2.5);
        assert!(position.values.chunks(3).all(|k| k[2] == 7.0));
        // the scale falls back to the reference's: still all ones
        assert_eq!(property(clip, Property::Scale).values, [1.0, 1.0, 1.0]);
    }

    /// A frame that mirrors z carries over to the channels: the translation as a point, the rotation about y as a
    /// rotation in the mirrored space (its x and z components' signs follow the mirror).
    #[test]
    fn the_clip_is_in_the_worlds_frame() {
        let mirror = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
        let (mut w, e, _) = world(mirror, 0);
        run(
            &mut w,
            &[("absent", "reference"), ("rate", "1"), ("clips", "all")],
        )
        .unwrap();
        let clip = w.get::<Clip>(e).unwrap();
        let position = property(clip, Property::Translation);
        assert_eq!(&position.values[..3], &[0.0, 2.0, -7.0]);
        // a mirrored rotation about y stays one about y, turning the other way
        let rotation = property(clip, Property::Rotation);
        assert!(
            (rotation.values[9] + std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "{:?}",
            rotation.values
        );
    }

    #[test]
    fn options_are_required_and_checked_and_an_unknown_blend_hint_is_left_out_as_unsupported() {
        for bad in [
            vec![("rate", "1"), ("clips", "all")],
            vec![("absent", "neutral"), ("clips", "all")],
            vec![("absent", "neutral"), ("rate", "1")],
            vec![("absent", "zero"), ("rate", "1"), ("clips", "all")],
            vec![("absent", "neutral"), ("rate", "0"), ("clips", "all")],
            vec![("absent", "neutral"), ("rate", "17"), ("clips", "all")],
            vec![("absent", "neutral"), ("rate", "1"), ("clips", "a;;b")],
            vec![("absent", "neutral"), ("rate", "1"), ("clips", "all")],
            vec![
                ("absent", "neutral"),
                ("rate", "1"),
                ("clips", "all"),
                ("root-motion", "some"),
            ],
        ] {
            let options: Options = bad.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
            assert!(LOWER_HAVOK_CLIPS.instance(&options).is_err(), "{bad:?}");
        }
        let (mut w, e, _) = world(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0), 2);
        let log = run(&mut w, &[("absent", "neutral"), ("rate", "1"), ("clips", "all")]).unwrap();
        assert!(!w.has::<Clip>(e));
        assert_eq!(
            log.omitted["animations with blend hint 2, whose values are not known to be absolute or deltas"],
            1
        );
        assert!(log.counts.is_empty(), "{:?}", log.counts);
        // a name the world does not have is an error naming it
        let (mut w, _, _) = world(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0), 0);
        let e = run(
            &mut w,
            &[("absent", "neutral"), ("rate", "1"), ("clips", "missing")],
        )
        .unwrap_err();
        assert!(e.contains("missing"), "{e}");
    }

    /// Three samples (frames a second apart) of a path along z that turns a quarter turn about up at the end.
    fn with_motion(w: &mut World, e: Entity) {
        let mut a = w.get::<HavokAnimation>(e).unwrap().clone();
        a.extracted_motion = Some(ReferenceFrame {
            frame_type: 0,
            up: [0.0, 1.0, 0.0, 0.0],
            forward: [0.0, 0.0, 1.0, 0.0],
            duration: 2.0,
            samples: vec![
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [1.0, 0.0, 2.0, std::f32::consts::FRAC_PI_2],
            ],
        });
        w.insert(e, a);
    }

    #[test]
    fn root_motion_is_the_samples_in_the_worlds_frame_when_the_program_says_so() {
        let mirror = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
        for (choice, turn) in [("leave", false), ("translation", false), ("full", true)] {
            let (mut w, e, _) = world(mirror, 0);
            with_motion(&mut w, e);
            let log = run(
                &mut w,
                &[
                    ("absent", "neutral"),
                    ("rate", "4"),
                    ("clips", "all"),
                    ("root-motion", choice),
                ],
            )
            .unwrap();
            let clip = w.get::<Clip>(e).unwrap();
            let Some(root) = &clip.root_motion else {
                assert_eq!(choice, "leave");
                assert_eq!(
                    log.counts["clips whose extracted motion (root motion) is left out"],
                    1
                );
                continue;
            };
            // one key a stored sample whatever the rate, at i * duration / (n - 1)
            let (_, times, values) = root.translation.as_ref().unwrap();
            assert_eq!(times, &[0.0, 1.0, 2.0]);
            // the mirror flips z
            assert_eq!(values, &[0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 1.0, 0.0, -2.0]);
            assert_eq!(root.rotation.is_some(), turn, "{choice}");
            if let Some((_, rt, rv)) = &root.rotation {
                assert_eq!(rt, times);
                // a quarter turn about up in the file's frame: a rotation of 90 degrees about y, here mirrored
                let (s, c) = (
                    std::f32::consts::FRAC_PI_4.sin(),
                    std::f32::consts::FRAC_PI_4.cos(),
                );
                assert!(
                    rv[8..]
                        .iter()
                        .zip([0.0, -s, 0.0, c])
                        .all(|(a, b)| (a - b).abs() < 1e-6),
                    "{rv:?}"
                );
                assert_eq!(&rv[..4], &[0.0, 0.0, 0.0, 1.0]);
            }
        }
    }

    #[test]
    fn root_motion_that_cannot_be_the_clips_leaves_the_animation_unlowered_with_the_reason() {
        let (mut w, e, _) = world(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0), 0);
        with_motion(&mut w, e);
        let mut a = w.get::<HavokAnimation>(e).unwrap().clone();
        a.extracted_motion.as_mut().unwrap().duration = 9.0;
        w.insert(e, a.clone());
        // the animation is left without a clip and reported with the reason, not an error of the pass
        let log = run(
            &mut w,
            &[
                ("absent", "neutral"),
                ("rate", "1"),
                ("clips", "all"),
                ("root-motion", "translation"),
            ],
        )
        .unwrap();
        assert!(!w.has::<Clip>(e));
        let why: Vec<&String> = log.omitted.keys().collect();
        assert!(
            why.len() == 1 && why[0].contains("extracted motion lasts 9"),
            "{why:?}"
        );
        a.extracted_motion.as_mut().unwrap().duration = 2.0;
        a.extracted_motion.as_mut().unwrap().up = [0.0, 2.0, 0.0, 0.0];
        w.insert(e, a);
        let log = run(
            &mut w,
            &[
                ("absent", "neutral"),
                ("rate", "1"),
                ("clips", "all"),
                ("root-motion", "full"),
            ],
        )
        .unwrap();
        assert!(!w.has::<Clip>(e));
        let why: Vec<&String> = log.omitted.keys().collect();
        assert!(why.len() == 1 && why[0].contains("not a unit vector"), "{why:?}");
    }

    #[test]
    fn the_registered_pass_declares_what_it_does() {
        let mut reg = PassRegistry::new();
        register(&mut reg);
        let (mut w, e, _) = world(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0), 0);
        let manager = PassManager::new(reg).with_pass_checks(true);
        let pipeline = manager
            .registry
            .parse("lower-havok-clips{absent=neutral,rate=1,clips=all,root-motion=leave}")
            .unwrap();
        let mut report = RunReport::default();
        manager.run(&mut w, &pipeline, &mut report).unwrap();
        assert!(w.has::<Clip>(e));
        assert_eq!(w.verify_components(), vec![]);
    }

    /// A refusal that names a track names the bone it drives (the one refused animation: block 0 track 119
    /// scale of `c2030/a001_009000`, a leaf bone `R_Egg`, stored with NaN bounds).
    #[test]
    fn a_refused_track_names_the_bone_it_drives() {
        let skeleton = Skeleton {
            name: "c0".into(),
            parents: vec![-1, 0],
            bones: ["Master", "R_Egg"]
                .map(|n| crate::animation::Bone {
                    name: n.into(),
                    lock_translation: false,
                })
                .to_vec(),
            reference_pose: vec![rest([0.0; 3]), rest([0.0; 3])],
            reference_floats: vec![],
            float_slots: vec![],
        };
        let binding = Binding {
            original_skeleton_name: "c0".into(),
            animation: Some(0),
            transform_track_to_bone: vec![0, 1],
            float_track_to_slot: vec![],
            partition_indices: vec![],
            blend_hint: 0,
        };
        let e = name_the_bone(
            invalid(0x4214, "block 0 track 1 scale: a stored float is not finite"),
            &binding,
            &skeleton,
        );
        assert!(
            e.detail.ends_with("(the track drives the bone R_Egg)"),
            "{}",
            e.detail
        );
        assert_eq!(e.offset, 0x4214);
        // an error that names no track, or a track the binding lacks, is passed on as it is
        for detail in [
            "the block runs past the animation data",
            "block 0 track 9 scale: x",
        ] {
            assert_eq!(
                name_the_bone(invalid(1, detail), &binding, &skeleton).detail,
                detail
            );
        }
    }

    #[test]
    fn a_skeleton_and_an_animation_are_inserted_in_the_worlds_frame() {
        let mirror = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
        let skeleton = Skeleton {
            name: "c0".into(),
            parents: vec![-1, 0],
            bones: ["Master", "Child"]
                .map(|n| crate::animation::Bone {
                    name: n.into(),
                    lock_translation: false,
                })
                .to_vec(),
            reference_pose: vec![rest([0.0; 3]), rest([1.0, 2.0, 3.0])],
            reference_floats: vec![],
            float_slots: vec![],
        };
        let mut w = World::new();
        let e = insert_skeleton(&mut w, &skeleton, &mirror).unwrap();
        let s = w.get::<CoreSkeleton>(e).unwrap();
        assert_eq!(s.joints[1].parent, Some(JointId(0)));
        assert_eq!(s.joints[1].name, "Child");
        let Rest::Trs { translation, .. } = s.joints[1].rest else {
            panic!("a TRS rest")
        };
        assert_eq!(translation, [1.0, 2.0, -3.0]);
        let mut bad = skeleton.clone();
        bad.parents = vec![-1, 5];
        assert!(insert_skeleton(&mut World::new(), &bad, &mirror).is_err());
        // an animation container that does not hold exactly one animation and one binding is refused
        let empty = Container {
            skeletons: vec![],
            animations: vec![],
            bindings: vec![],
        };
        assert!(insert_animation(&mut w, "x", &empty, e, &skeleton, &mirror).is_err());
    }
}
