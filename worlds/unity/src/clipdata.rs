//! An AnimationClip's curves as data (bindings, keys, cubic curves, tracks) and a clip lowered to a `core.clip` for the
//! joints of a rig. Plain types: reading them from a file is [`crate::clip`]'s; the humanoid evaluation
//! ([`crate::humanoid`]) and the passes use these. See the reader's module for how a clip stores its curves.

use std::collections::BTreeSet;

use primase_core::{
    Channel, Clip, ClipTarget, Entity, Extrapolation, Interpolation, JointId, Property, Rest,
};

use crate::components::UNITY;
use crate::{Error, Result, invalid};

/// One curve binding of a clip (`genericBindings`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericBinding {
    /// The CRC-32 of the target's path below the Animator's GameObject.
    pub path: u32,
    /// The attribute: for a Transform (`type_id` 4) 1 position, 2 rotation, 3 scale, 4 Euler angles; else the
    /// property's hash.
    pub attribute: u32,
    /// The target's class id (4 Transform, 95 Animator, 137 SkinnedMeshRenderer, ...).
    pub type_id: i64,
    pub is_pptr: bool,
}

impl GenericBinding {
    /// How many curves this binding takes.
    pub(crate) fn width(&self) -> usize {
        match (self.type_id, self.attribute) {
            (4, 1 | 3 | 4) => 3,
            (4, 2) => 4,
            _ => 1,
        }
    }
}

/// Where a curve was stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Streamed,
    Dense,
    Constant,
}

/// A key of a curve: its time (seconds), value and the slopes (per second) into and out of it. An infinite outgoing
/// slope is a step to the next key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub time: f32,
    pub value: f32,
    pub in_slope: f32,
    pub out_slope: f32,
}

/// One scalar curve: keys in increasing time, evaluated as a cubic Hermite spline between them (a dense curve's slopes
/// are its secants, so it is piecewise linear).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub storage: Storage,
    pub keys: Vec<Key>,
}

impl Curve {
    /// The value at `t` and the slopes into and out of it: a key's own at its time, a segment's derivative inside it,
    /// the end values held with no slope outside. A step segment holds its key's value.
    pub fn sample(&self, t: f32) -> (f32, f32, f32) {
        let k = &self.keys;
        let (Some(first), Some(last)) = (k.first(), k.last()) else {
            return (0.0, 0.0, 0.0);
        };
        if t <= first.time {
            return (
                first.value,
                0.0,
                if t == first.time { first.out_slope } else { 0.0 },
            );
        }
        if t >= last.time {
            return (last.value, if t == last.time { last.in_slope } else { 0.0 }, 0.0);
        }
        let i = k.partition_point(|key| key.time <= t) - 1;
        let (a, b) = (&k[i], &k[i + 1]);
        if t == a.time {
            return (a.value, a.in_slope, a.out_slope);
        }
        if !a.out_slope.is_finite() {
            return (a.value, 0.0, 0.0);
        }
        let (dt, s) = (
            f64::from(b.time - a.time),
            f64::from(t - a.time) / f64::from(b.time - a.time),
        );
        let (p0, p1) = (f64::from(a.value), f64::from(b.value));
        let (m0, m1) = (f64::from(a.out_slope) * dt, f64::from(b.in_slope) * dt);
        let (s2, s3) = (s * s, s * s * s);
        let value = (2.0 * s3 - 3.0 * s2 + 1.0) * p0
            + (s3 - 2.0 * s2 + s) * m0
            + (-2.0 * s3 + 3.0 * s2) * p1
            + (s3 - s2) * m1;
        let slope = ((6.0 * s2 - 6.0 * s) * p0
            + (3.0 * s2 - 4.0 * s + 1.0) * m0
            + (-6.0 * s2 + 6.0 * s) * p1
            + (3.0 * s2 - 2.0 * s) * m1)
            / dt;
        (value as f32, slope as f32, slope as f32)
    }

    /// Whether any slope is a step.
    pub(crate) fn has_steps(&self) -> bool {
        self.keys.iter().any(|k| !k.out_slope.is_finite())
    }

    pub(crate) fn is_constant(&self) -> bool {
        self.keys.len() == 1
    }
}

/// A binding with the curves it takes, in the order of its attribute's components.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub binding: GenericBinding,
    pub curves: Vec<Curve>,
}

/// An AnimationClip's curves as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipData {
    pub name: String,
    /// Seconds, `m_StopTime - m_StartTime`.
    pub duration: f32,
    pub looping: bool,
    pub tracks: Vec<Track>,
    /// Animation events (`m_Events`): not read.
    pub events: usize,
    /// How far a streamed segment's end was from the next key's value (relative), the worst over the segments of a
    /// millisecond or more: the decode's own consistency (the segment is the cubic the next key continues).
    pub worst_continuity_error: f32,
}

/// A humanoid avatar with the way to find the joint of a bone by the hash of its path below the avatar's root.
pub type HumanoidRig<'a> = (
    &'a crate::animator::UnityAvatar,
    &'a dyn Fn(u32) -> Option<(Entity, JointId)>,
);

/// What lowering a clip did: channels made, bindings left out by reason.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct ClipLowering {
    pub channels: usize,
    pub keys: usize,
    /// Rotation keys renormalised to unit length (they were not within 1e-4 of it).
    pub renormalised_keys: usize,
    /// Transform curves whose path is no joint of the rig, by attribute (`position`, `rotation`, `scale`).
    pub unresolved: std::collections::BTreeMap<&'static str, usize>,
    /// Bindings with no joint channel in core, by what they are.
    pub not_lowered: std::collections::BTreeMap<&'static str, usize>,
    /// What the humanoid muscle curves made of a humanoid avatar's bones ([`crate::humanoid`]); `None` for a clip lowered
    /// without a humanoid avatar.
    pub humanoid: Option<crate::humanoid::HumanoidLowering>,
}

/// A joint-channel value or tangent of `property` in the world's frame.
fn convert(property: Property, v: &[f32]) -> Vec<f32> {
    let rest = match property {
        Property::Translation => Rest::Trs {
            translation: [v[0], v[1], v[2]],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Property::Rotation => Rest::Trs {
            translation: [0.0; 3],
            rotation: [v[0], v[1], v[2], v[3]],
            scale: [1.0; 3],
        },
        _ => Rest::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [v[0], v[1], v[2]],
        },
    };
    match rest.converted(&UNITY) {
        Rest::Trs {
            translation,
            rotation,
            scale,
        } => match property {
            Property::Translation => translation.to_vec(),
            Property::Rotation => rotation.to_vec(),
            _ => scale.to_vec(),
        },
        Rest::Affine(_) => unreachable!("a TRS converts to a TRS"),
    }
}

impl Track {
    /// This transform track as a joint channel on `target`, in the world's frame: its curves sampled at the union of
    /// their key times, as a Hermite spline (exact: each curve is a cubic between its keys), as a Linear channel when
    /// every curve is dense or constant, and as a one-key channel when every curve is constant. `None` for a
    /// binding that is not position, rotation or scale of a Transform. An error for steps mixed with smooth segments.
    pub fn channel(
        &self,
        target: ClipTarget,
        duration: f32,
        renormalised: &mut usize,
    ) -> Result<Option<Channel>> {
        let property = match (self.binding.type_id, self.binding.attribute) {
            (4, 1) => Property::Translation,
            (4, 2) => Property::Rotation,
            (4, 3) => Property::Scale,
            _ => return Ok(None),
        };
        let width = self.curves.len();
        let steps = self.curves.iter().filter(|c| c.has_steps()).count();
        if steps > 0 {
            return Err(Error::Unsupported(format!("a {property:?} curve with steps")));
        }
        let mut times: Vec<f32> = self
            .curves
            .iter()
            .flat_map(|c| c.keys.iter().map(|k| k.time))
            .collect();
        // a key a rounding beyond the clip's end is at its end
        times.retain(|t| *t <= duration + 1e-4);
        times.iter_mut().for_each(|t| *t = t.min(duration));
        times.sort_by(f32::total_cmp);
        times.dedup();
        let all_constant = self.curves.iter().all(Curve::is_constant);
        let all_linear = self.curves.iter().all(|c| c.storage != Storage::Streamed);
        let interpolation = if all_constant || all_linear {
            Interpolation::Linear
        } else {
            Interpolation::Hermite
        };
        if all_constant {
            times.truncate(1);
        }
        let mut values = Vec::new();
        for (n, &t) in times.iter().enumerate() {
            let samples: Vec<(f32, f32, f32)> = self.curves.iter().map(|c| c.sample(t)).collect();
            let mut value: Vec<f32> = samples.iter().map(|s| s.0).collect();
            if property == Property::Rotation {
                let length = value.iter().map(|x| x * x).sum::<f32>().sqrt();
                if (length - 1.0).abs() > 1e-4 && length > 0.0 {
                    value.iter_mut().for_each(|x| *x /= length);
                    *renormalised += 1;
                }
            }
            if interpolation == Interpolation::Hermite {
                let before = if n > 0 { t - times[n - 1] } else { 0.0 };
                let after = if n + 1 < times.len() {
                    times[n + 1] - t
                } else {
                    0.0
                };
                let in_tangent: Vec<f32> = samples.iter().map(|s| s.1 * before).collect();
                let out_tangent: Vec<f32> = samples.iter().map(|s| s.2 * after).collect();
                values.extend(convert(property, &in_tangent));
                values.extend(convert(property, &value));
                values.extend(convert(property, &out_tangent));
            } else {
                values.extend(convert(property, &value));
            }
        }
        debug_assert_eq!(width, if property == Property::Rotation { 4 } else { 3 });
        Ok(Some(Channel {
            target,
            property,
            interpolation,
            times,
            values,
        }))
    }
}

impl ClipData {
    /// The clip as a `core.clip` for a rig: the position, rotation and scale curves of the Transforms `resolve` finds
    /// (a path hash to the skeleton and joint it is), in the world's frame; the rest counted in the returned
    /// [`ClipLowering`] by reason.
    pub fn lower(
        &self,
        resolve: &dyn Fn(u32) -> Option<(Entity, JointId)>,
        humanoid: Option<HumanoidRig>,
    ) -> Result<(Clip, ClipLowering)> {
        let humanoid = humanoid.filter(|(a, _)| a.human.is_some());
        let mut report = ClipLowering::default();
        let mut channels = Vec::new();
        let mut seen = BTreeSet::new();
        for track in &self.tracks {
            let b = &track.binding;
            let kind = match (b.type_id, b.attribute) {
                (4, 1) => "position",
                (4, 2) => "rotation",
                (4, 3) => "scale",
                (4, 4) => {
                    *report
                        .not_lowered
                        .entry("Euler angle curves of a Transform")
                        .or_default() += 1;
                    continue;
                }
                // a humanoid avatar's muscle curves are evaluated below
                (95, _) if humanoid.is_some() => continue,
                (95, _) => {
                    *report
                        .not_lowered
                        .entry("Animator (humanoid muscle) curves")
                        .or_default() += 1;
                    continue;
                }
                (137, _) => {
                    *report
                        .not_lowered
                        .entry("SkinnedMeshRenderer blend shape curves")
                        .or_default() += 1;
                    continue;
                }
                _ => {
                    *report
                        .not_lowered
                        .entry("curves of other components")
                        .or_default() += 1;
                    continue;
                }
            };
            let Some((skeleton, joint)) = resolve(b.path) else {
                *report.unresolved.entry(kind).or_default() += 1;
                continue;
            };
            let target = ClipTarget::Joint { skeleton, joint };
            let channel = track
                .channel(target, self.duration, &mut report.renormalised_keys)?
                .expect("a position, rotation or scale curve is a channel");
            if !seen.insert((joint, channel.property)) {
                return Err(invalid(format!(
                    "{}: two {:?} channels for one joint ({:?})",
                    self.name, channel.property, joint
                )));
            }
            report.keys += channel.times.len();
            channels.push(channel);
        }
        if let Some((avatar, bone_resolve)) = humanoid {
            let (bones, lowering) = self.humanoid_channels(avatar, bone_resolve);
            for channel in bones {
                if let ClipTarget::Joint { joint, .. } = channel.target
                    && !seen.insert((joint, channel.property))
                {
                    return Err(invalid(format!(
                        "{}: a humanoid bone's rotation and a Transform rotation curve for one joint ({joint:?})",
                        self.name
                    )));
                }
                report.keys += channel.times.len();
                channels.push(channel);
            }
            report.humanoid = Some(lowering);
        }
        report.channels = channels.len();
        Ok((
            Clip {
                name: self.name.clone(),
                duration: self.duration,
                extrapolation: if self.looping {
                    Extrapolation::Loop
                } else {
                    Extrapolation::Hold
                },
                channels,
                root_motion: None,
                additive: None,
            },
            report,
        ))
    }
}
