//! A clip lowered to a `core.clip` for the joints of a rig: the clip's curves (`unity_formats::clip`) sampled as Hermite
//! channels in the world's frame. The humanoid evaluation ([`crate::humanoid`]) and the passes use these.

use std::collections::BTreeSet;

use primase_core::{
    Channel, Clip, ClipTarget, Entity, Extrapolation, Interpolation, JointId, MorphId, Property, Rest,
};
use unity_formats::clip::{ClipData, Curve, Key, Storage, Track};
use unity_formats::{Error, Result};

use crate::components::UNITY;
use crate::invalid;

/// A humanoid bone's joint in the rig: its skeleton, the joint, and the translation the scene gives it (the Transform's local
/// position, in Unity's frame).
pub type RigBone = (Entity, JointId, [f64; 3]);

/// A humanoid avatar with the way to find the joint of a bone by the hash of its path below the avatar's root.
pub type HumanoidRig<'a> = (
    &'a crate::animator::UnityAvatar,
    &'a dyn Fn(u32) -> Option<RigBone>,
);

/// What lowering a clip did: channels made, bindings left out by reason.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct ClipLowering {
    pub channels: usize,
    pub keys: usize,
    /// Rotation keys renormalised to unit length (they were not within 1e-4 of it).
    pub renormalised_keys: usize,
    /// Channels of weighted legacy curves (a Bezier curve in time and value, no cubic in time), sampled at
    /// [`WEIGHTED_STEPS`] keys a stretch between the curves' own keys, and the largest difference of such a channel from
    /// its curves halfway between its keys.
    pub weighted_channels: usize,
    pub worst_weighted_error: f32,
    /// Whether a ping-pong clip (a legacy clip's wrap mode 4) was unrolled into a loop of twice its length: forwards,
    /// then its keys mirrored.
    pub ping_pong_unrolled: bool,
    /// Transform curves whose path is no joint of the rig, by attribute (`position`, `rotation`, `scale`).
    pub unresolved: std::collections::BTreeMap<&'static str, usize>,
    /// Blend shape curves lowered to morph channels, and those whose renderer or shape the character lacks.
    pub morph_channels: usize,
    pub unresolved_morphs: usize,
    /// Bindings with no joint channel in core, by what they are.
    pub not_lowered: std::collections::BTreeMap<&'static str, usize>,
    /// What the humanoid muscle curves made of a humanoid avatar's bones ([`crate::humanoid`]); `None` for a clip lowered
    /// without a humanoid avatar.
    pub humanoid: Option<crate::humanoid::HumanoidLowering>,
}

/// A channel value or tangent of `property` in the world's frame.
fn convert(property: Property, v: &[f32]) -> Vec<f32> {
    if matches!(
        property,
        Property::Scalar | Property::Visibility | Property::Values(_)
    ) {
        return v.to_vec();
    }
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
        Property::Scale => Rest::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [v[0], v[1], v[2]],
        },
        Property::Scalar | Property::Visibility | Property::Values(_) => {
            unreachable!("non-spatial values returned above")
        }
    };
    match rest.converted(&UNITY) {
        Rest::Trs {
            translation,
            rotation,
            scale,
        } => match property {
            Property::Translation => translation.to_vec(),
            Property::Rotation => rotation.to_vec(),
            Property::Scale => scale.to_vec(),
            Property::Scalar | Property::Visibility | Property::Values(_) => {
                unreachable!("non-spatial values returned above")
            }
        },
        Rest::Affine(_) => unreachable!("a TRS converts to a TRS"),
    }
}

/// The keys a channel of weighted curves has between two keys of its curves.
pub const WEIGHTED_STEPS: usize = 8;

/// This transform track as a joint channel on `target`, in the world's frame: its curves sampled at the union of
/// their key times, as a Hermite spline (each curve is a cubic between its keys, which the Hermite form with the curve's own
/// values and slopes at the keys states exactly; the tangents are per second), as a Linear channel when
/// every curve is dense or constant, and as a one-key channel when every curve is constant. A track with a weighted
/// segment is sampled at [`WEIGHTED_STEPS`] keys a stretch, its values and slopes exact at each, and counted in `report`
/// with the largest difference from its curves halfway between keys. `None` for a binding that is not position,
/// rotation or scale of a Transform. An error for steps mixed with smooth segments.
pub fn track_channel(
    track: &Track,
    target: ClipTarget,
    duration: f32,
    report: &mut ClipLowering,
) -> Result<Option<Channel>> {
    let property = match (track.binding.type_id, track.binding.attribute) {
        (4, 1) => Property::Translation,
        (4, 2) => Property::Rotation,
        (4, 3) => Property::Scale,
        (137, _) => Property::Scalar,
        _ => return Ok(None),
    };
    let width = track.curves.len();
    let steps = track.curves.iter().filter(|c| c.has_steps()).count();
    // a scalar curve may step throughout; a transform's, or a scalar's with steps among smooth segments, may not
    let stepped = steps > 0
        && property == Property::Scalar
        && track
            .curves
            .iter()
            .all(|c| c.keys.iter().rev().skip(1).all(|k| !k.out_slope.is_finite()));
    if steps > 0 && !stepped {
        return Err(Error::Unsupported(format!("a {property:?} curve with steps")));
    }
    let mut times: Vec<f32> = track
        .curves
        .iter()
        .flat_map(|c| c.keys.iter().map(|k| k.time))
        .collect();
    // a key a rounding beyond the clip's end is at its end
    times.retain(|t| *t <= duration + 1e-4);
    times.iter_mut().for_each(|t| *t = t.min(duration));
    times.sort_by(f32::total_cmp);
    times.dedup();
    let all_constant = track.curves.iter().all(Curve::is_constant);
    // dense and constant curves are linear between their keys; streamed and legacy keyframe curves are cubics
    let all_linear = track
        .curves
        .iter()
        .all(|c| matches!(c.storage, Storage::Dense | Storage::Constant));
    let weighted = !stepped && !all_constant && track.curves.iter().any(Curve::is_weighted);
    if weighted {
        let mut dense = Vec::with_capacity(times.len().saturating_mul(WEIGHTED_STEPS));
        for w in times.windows(2) {
            dense
                .extend((0..WEIGHTED_STEPS).map(|i| w[0] + (w[1] - w[0]) * i as f32 / WEIGHTED_STEPS as f32));
        }
        dense.extend(times.last());
        dense.dedup();
        times = dense;
    }
    let interpolation = if stepped {
        Interpolation::Step
    } else if all_constant || all_linear {
        Interpolation::Linear
    } else {
        Interpolation::Hermite
    };
    if all_constant {
        times.truncate(1);
    }
    let mut values = Vec::new();
    for &t in &times {
        let samples: Vec<(f32, f32, f32)> = track.curves.iter().map(|c| c.sample(t)).collect();
        let mut value: Vec<f32> = samples.iter().map(|s| s.0).collect();
        if property == Property::Rotation {
            let length = value.iter().map(|x| x * x).sum::<f32>().sqrt();
            if (length - 1.0).abs() > 1e-4 && length > 0.0 {
                value.iter_mut().for_each(|x| *x /= length);
                report.renormalised_keys += 1;
            }
        }
        if interpolation == Interpolation::Hermite {
            // core.clip's tangents are per second (the evaluator scales them by the segment's length)
            let in_tangent: Vec<f32> = samples.iter().map(|s| s.1).collect();
            let out_tangent: Vec<f32> = samples.iter().map(|s| s.2).collect();
            values.extend(convert(property, &in_tangent));
            values.extend(convert(property, &value));
            values.extend(convert(property, &out_tangent));
        } else {
            values.extend(convert(property, &value));
        }
    }
    debug_assert_eq!(width, property.width());
    if weighted {
        report.weighted_channels += 1;
        // each curve against the Hermite spline of its samples, halfway between them
        for c in &track.curves {
            let spline = Curve {
                storage: Storage::Streamed,
                keys: times
                    .iter()
                    .map(|&t| {
                        let (value, in_slope, out_slope) = c.sample(t);
                        Key::new(t, value, in_slope, out_slope)
                    })
                    .collect(),
            };
            for w in times.windows(2) {
                let mid = 0.5 * (w[0] + w[1]);
                let error = (spline.sample(mid).0 - c.sample(mid).0).abs();
                report.worst_weighted_error = report.worst_weighted_error.max(error);
            }
        }
    }
    Ok(Some(Channel {
        target,
        property,
        interpolation,
        times,
        values,
    }))
}

/// The clip as a `core.clip` for a rig: the position, rotation and scale curves of the Transforms `resolve` finds
/// (a path hash to the skeleton and joint it is), in the world's frame; the rest counted in the returned
/// [`ClipLowering`] by reason.
pub fn lower_clip(
    data: &ClipData,
    resolve: &dyn Fn(u32) -> Option<(Entity, JointId)>,
    morph_resolve: &dyn Fn(u32, u32) -> Option<(Entity, MorphId)>,
    humanoid: Option<HumanoidRig>,
) -> Result<(Clip, ClipLowering)> {
    let humanoid = humanoid.filter(|(a, _)| a.human.is_some());
    let mut report = ClipLowering::default();
    let mut channels = Vec::new();
    let mut seen = BTreeSet::new();
    let mut morph_seen = BTreeSet::new();
    for track in &data.tracks {
        let b = &track.binding;
        if b.is_pptr {
            *report
                .not_lowered
                .entry("object reference (PPtr) curves: a material or sprite swap")
                .or_default() += 1;
            continue;
        }
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
            // a blend shape: the curve's path names the renderer, its attribute the CRC-32 of the shape's name
            (137, _) => {
                let Some((geometry, morph)) = morph_resolve(b.path, b.attribute) else {
                    report.unresolved_morphs += 1;
                    continue;
                };
                let channel = track_channel(
                    track,
                    ClipTarget::Morph { geometry, morph },
                    data.duration,
                    &mut report,
                )?
                .expect("a blend shape curve is a channel");
                if !morph_seen.insert((geometry, morph)) {
                    return Err(invalid(format!(
                        "{}: two channels for one blend shape ({morph:?})",
                        data.name
                    )));
                }
                report.keys += channel.times.len();
                report.morph_channels += 1;
                channels.push(channel);
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
        let channel = track_channel(track, target, data.duration, &mut report)?
            .expect("a position, rotation or scale curve is a channel");
        if !seen.insert((joint, channel.property)) {
            return Err(invalid(format!(
                "{}: two {:?} channels for one joint ({:?})",
                data.name, channel.property, joint
            )));
        }
        report.keys += channel.times.len();
        channels.push(channel);
    }
    if let Some((avatar, bone_resolve)) = humanoid {
        let (bones, mut lowering) = crate::humanoid::humanoid_channels(data, avatar, bone_resolve);
        for channel in bones {
            if let ClipTarget::Joint { joint, .. } = channel.target
                && !seen.insert((joint, channel.property))
            {
                // a Transform position curve on a humanoid bone is the clip's own word about its translation
                if channel.property == Property::Translation {
                    lowering.translations_kept_from_curves += 1;
                    continue;
                }
                return Err(invalid(format!(
                    "{}: a humanoid bone's rotation and a Transform rotation curve for one joint ({joint:?})",
                    data.name
                )));
            }
            report.keys += channel.times.len();
            channels.push(channel);
        }
        report.humanoid = Some(lowering);
    }
    let mut duration = data.duration;
    if data.ping_pong {
        for c in &mut channels {
            ping_pong(c, data.duration);
        }
        duration *= 2.0;
        report.ping_pong_unrolled = true;
    }
    report.channels = channels.len();
    Ok((
        Clip {
            name: data.name.clone(),
            duration,
            extrapolation: if data.looping || data.ping_pong {
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

/// `c` played forwards over `duration`, then backwards to its start, over `2 duration`: each key mirrored at
/// `2 duration - t`, a Hermite key's tangents negated and swapped (at the turn, the key's outgoing tangent is its
/// incoming one negated), a step's value held over the mirror of the stretch it held forwards.
fn ping_pong(c: &mut Channel, duration: f32) {
    let width = c.property.width();
    let hermite = c.interpolation == Interpolation::Hermite;
    let per_key = if hermite { 3 * width } else { width };
    let (times, forward) = (c.times.clone(), c.values.clone());
    let end = 2.0 * duration;
    for (i, &t) in times.iter().enumerate().rev() {
        let key = &forward[i * per_key..(i + 1) * per_key];
        let (at, values): (f32, Vec<f32>) = match c.interpolation {
            // backwards, a key's value holds from the mirror of where the next key began (the clip's end, for the last)
            Interpolation::Step => (end - times.get(i + 1).copied().unwrap_or(duration), key.to_vec()),
            Interpolation::Linear => (end - t, key.to_vec()),
            Interpolation::Hermite => {
                let (into, rest) = key.split_at(width);
                let (value, out) = rest.split_at(width);
                let mirrored = out
                    .iter()
                    .map(|x| -x)
                    .chain(value.iter().copied())
                    .chain(into.iter().map(|x| -x))
                    .collect();
                (end - t, mirrored)
            }
        };
        let Some(&last) = c.times.last() else { return };
        if at > last {
            c.times.push(at);
            c.values.extend(values);
        } else if at == last && hermite {
            let n = c.values.len();
            c.values[n - width..].copy_from_slice(&values[2 * width..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use unity_formats::clip::GenericBinding;

    use super::*;

    fn key(time: f32, value: f32, in_slope: f32, out_slope: f32) -> Key {
        Key::new(time, value, in_slope, out_slope)
    }

    /// Hand-worked: a position whose three curves have two keys 0.5 s apart, the first leaving at 2 per second, the second
    /// arriving at -4 per second. The channel's tangents are those slopes, per second (core.clip's evaluator scales them
    /// by the segment's length: 0.5), not slopes times a segment length already. (The z curve is negated into core's frame.)
    #[test]
    fn a_hermite_channel_holds_per_second_tangents() {
        let curve = Curve {
            storage: Storage::Streamed,
            keys: vec![key(0.0, 0.0, 0.0, 2.0), key(0.5, 1.0, -4.0, 0.0)],
        };
        let track = Track {
            binding: GenericBinding {
                path: 0,
                attribute: 1,
                type_id: 4,
                is_pptr: false,
            },
            curves: vec![curve.clone(), curve.clone(), curve],
        };
        let mut world = primase_core::World::new();
        let target = ClipTarget::Joint {
            skeleton: world.spawn(),
            joint: JointId(0),
        };
        let channel = track_channel(&track, target, 0.5, &mut ClipLowering::default())
            .expect("lowers")
            .expect("a position");
        assert_eq!(channel.interpolation, Interpolation::Hermite);
        assert_eq!(channel.times, [0.0, 0.5]);
        // per key: in-tangent, value, out-tangent, each x y z (z negated)
        let x: Vec<f32> = (0..6).map(|k| channel.values[k * 3]).collect();
        assert_eq!(x, [0.0, 0.0, 2.0, -4.0, 1.0, 0.0]);
        let z: Vec<f32> = (0..6).map(|k| channel.values[k * 3 + 2]).collect();
        assert_eq!(z, [-0.0, -0.0, -2.0, 4.0, -1.0, -0.0]);
    }

    #[test]
    fn transform_tracks_become_joint_channels() {
        let binding = |attribute| GenericBinding {
            path: 1,
            attribute,
            type_id: 4,
            is_pptr: false,
        };
        let line = |a: f32, b: f32| Curve {
            storage: Storage::Streamed,
            keys: vec![key(0.0, a, 0.0, b - a), key(1.0, b, b - a, 0.0)],
        };
        let mut world = primase_core::World::new();
        let target = ClipTarget::Joint {
            skeleton: world.spawn(),
            joint: JointId(0),
        };
        let track = Track {
            binding: binding(1),
            curves: vec![
                line(0.0, 1.0),
                Curve {
                    storage: Storage::Constant,
                    keys: vec![key(0.0, 5.0, 0.0, 0.0)],
                },
                Curve {
                    storage: Storage::Streamed,
                    keys: vec![
                        key(0.0, 0.0, 0.0, 2.0),
                        key(0.5, 1.0, 2.0, 2.0),
                        key(1.0, 2.0, 2.0, 0.0),
                    ],
                },
            ],
        };
        let mut renormalised = ClipLowering::default();
        let c = track_channel(&track, target.clone(), 1.0, &mut renormalised)
            .unwrap()
            .unwrap();
        assert_eq!(c.property, Property::Translation);
        assert_eq!(c.interpolation, Interpolation::Hermite);
        assert_eq!(c.times, vec![0.0, 0.5, 1.0]);
        // 9 values a key: in tangents, value, out tangents; x runs 0 to 1 and z 0 to 2 along straight lines
        assert_eq!(c.values.len(), 27);
        let key1 = &c.values[9..18];
        assert!(
            (key1[3] - 0.5).abs() < 1e-6 && (key1[4] - 5.0).abs() < 1e-6 && (key1[5] + 1.0).abs() < 1e-6,
            "{key1:?}"
        );
        let constant = Track {
            binding: binding(3),
            curves: vec![
                Curve {
                    storage: Storage::Constant,
                    keys: vec![key(0.0, 1.0, 0.0, 0.0)]
                };
                3
            ],
        };
        let c = track_channel(&constant, target.clone(), 1.0, &mut renormalised)
            .unwrap()
            .unwrap();
        assert_eq!(
            (c.interpolation, c.times.len(), c.values.len()),
            (Interpolation::Linear, 1, 3)
        );
        let euler = Track {
            binding: binding(4),
            curves: vec![],
        };
        assert!(
            track_channel(&euler, target, 1.0, &mut renormalised)
                .unwrap()
                .is_none()
        );
    }

    /// A SkinnedMeshRenderer's blend shape curve is a morph channel on the target its renderer path and shape name hashes
    /// resolve to, in Unity's weights (0 to 100) as stored; one with no such target is counted, not lowered, and two for
    /// one target are refused.
    #[test]
    fn blend_shape_curves_become_morph_channels() {
        let track = |path: u32, shape: u32| Track {
            binding: GenericBinding {
                path,
                attribute: shape,
                type_id: 137,
                is_pptr: false,
            },
            curves: vec![Curve {
                storage: Storage::Streamed,
                keys: vec![key(0.0, 0.0, 0.0, 100.0), key(1.0, 100.0, 100.0, 0.0)],
            }],
        };
        let mut world = primase_core::World::new();
        let geometry = world.spawn();
        let clip = |tracks: Vec<Track>| ClipData {
            name: "blink".into(),
            duration: 1.0,
            looping: false,
            ping_pong: false,
            legacy: false,
            tracks,
            pptr_targets: Vec::new(),
            events: 0,
            worst_continuity_error: 0.0,
        };
        let morph =
            |path: u32, shape: u32| (path == 5 && shape == 6).then_some((geometry, primase_core::MorphId(3)));
        let none = |_: u32| None;
        let (lowered, report) =
            lower_clip(&clip(vec![track(5, 6), track(5, 99)]), &none, &morph, None).unwrap();
        assert_eq!(
            (report.morph_channels, report.unresolved_morphs, report.channels),
            (1, 1, 1)
        );
        let c = &lowered.channels[0];
        assert_eq!(
            c.target,
            ClipTarget::Morph {
                geometry,
                morph: primase_core::MorphId(3)
            }
        );
        assert_eq!(
            (c.property, c.interpolation),
            (Property::Scalar, Interpolation::Hermite)
        );
        assert_eq!(c.times, vec![0.0, 1.0]);
        // in tangent, value, out tangent a key; the values are not scaled
        assert_eq!(c.values.len(), 6);
        assert!((c.values[4] - 100.0).abs() < 1e-5, "{:?}", c.values);
        assert!(lower_clip(&clip(vec![track(5, 6), track(5, 6)]), &none, &morph, None).is_err());
    }

    fn legacy_clip(tracks: Vec<Track>, ping_pong: bool) -> ClipData {
        ClipData {
            name: "door".into(),
            duration: 1.0,
            looping: false,
            ping_pong,
            legacy: true,
            tracks,
            pptr_targets: Vec::new(),
            events: 0,
            worst_continuity_error: 0.0,
        }
    }

    fn scale_track(curve: Curve) -> Track {
        Track {
            binding: GenericBinding {
                path: 1,
                attribute: 3,
                type_id: 4,
                is_pptr: false,
            },
            curves: vec![curve.clone(), curve.clone(), curve],
        }
    }

    /// A legacy clip's keyframe curves are cubics between their keys, so a Hermite channel; an object reference
    /// curve (a material swap on a SkinnedMeshRenderer, class 137 like a blend shape's) is counted, not lowered as a
    /// blend shape.
    #[test]
    fn legacy_keyframes_are_hermite_and_pptr_curves_are_not_blend_shapes() {
        let curve = Curve {
            storage: Storage::Keyframes,
            keys: vec![key(0.0, 1.0, 0.0, 2.0), key(1.0, 2.0, 0.5, 0.0)],
        };
        let swap = Track {
            binding: GenericBinding {
                path: 5,
                attribute: 0,
                type_id: 137,
                is_pptr: true,
            },
            curves: vec![Curve {
                storage: Storage::Keyframes,
                keys: vec![key(0.0, 0.0, f32::INFINITY, f32::INFINITY)],
            }],
        };
        let mut world = primase_core::World::new();
        let skeleton = world.spawn();
        let joint = |_: u32| Some((skeleton, JointId(0)));
        let morph =
            |_: u32, _: u32| -> Option<(Entity, MorphId)> { panic!("a PPtr curve is no blend shape") };
        let (clip, report) = lower_clip(
            &legacy_clip(vec![scale_track(curve), swap], false),
            &joint,
            &morph,
            None,
        )
        .unwrap();
        assert_eq!(clip.channels.len(), 1);
        assert_eq!(clip.channels[0].interpolation, Interpolation::Hermite);
        assert_eq!(report.not_lowered.values().sum::<usize>(), 1);
        assert!(report.not_lowered.keys().any(|k| k.contains("PPtr")));
    }

    /// A weighted segment is sampled at `WEIGHTED_STEPS` keys, each with the curve's exact value and slope; the Hermite
    /// spline between them is within a small error of the Bezier curve, which the report states.
    #[test]
    fn weighted_curves_are_sampled_with_their_error_reported() {
        let curve = Curve {
            storage: Storage::Keyframes,
            keys: vec![
                Key {
                    out_weight: Some(0.9),
                    ..key(0.0, 0.0, 0.0, 3.0)
                },
                Key {
                    in_weight: Some(0.1),
                    ..key(1.0, 1.0, 0.0, 0.0)
                },
            ],
        };
        let mut world = primase_core::World::new();
        let target = ClipTarget::Joint {
            skeleton: world.spawn(),
            joint: JointId(0),
        };
        let mut report = ClipLowering::default();
        let c = track_channel(&scale_track(curve.clone()), target, 1.0, &mut report)
            .unwrap()
            .unwrap();
        assert_eq!(c.times.len(), WEIGHTED_STEPS + 1);
        assert_eq!(report.weighted_channels, 1);
        for (i, &t) in c.times.iter().enumerate() {
            assert!((c.values[i * 9 + 3] - curve.sample(t).0).abs() < 1e-6, "{t}");
        }
        assert!(
            report.worst_weighted_error > 0.0 && report.worst_weighted_error < 0.05,
            "{}",
            report.worst_weighted_error
        );
    }

    /// A ping-pong clip is a loop of twice its length: its keys, then the same keys mirrored, a Hermite key's tangents
    /// negated and swapped.
    #[test]
    fn ping_pong_clips_unroll_into_a_loop() {
        let curve = Curve {
            storage: Storage::Keyframes,
            keys: vec![
                key(0.0, 1.0, 0.0, 2.0),
                key(0.25, 1.5, 1.0, 1.0),
                key(1.0, 2.0, 0.5, 0.0),
            ],
        };
        let mut world = primase_core::World::new();
        let skeleton = world.spawn();
        let joint = |_: u32| Some((skeleton, JointId(0)));
        let morph = |_: u32, _: u32| None;
        let (clip, report) =
            lower_clip(&legacy_clip(vec![scale_track(curve)], true), &joint, &morph, None).unwrap();
        assert!(report.ping_pong_unrolled);
        assert_eq!((clip.duration, clip.extrapolation), (2.0, Extrapolation::Loop));
        let c = &clip.channels[0];
        assert_eq!(c.times, vec![0.0, 0.25, 1.0, 1.75, 2.0]);
        // the x scale of each key: in tangent, value, out tangent
        let x = |k: usize| [c.values[k * 9], c.values[k * 9 + 3], c.values[k * 9 + 6]];
        assert_eq!(x(2), [0.5, 2.0, -0.5]);
        assert_eq!(x(3), [-1.0, 1.5, -1.0]);
        assert_eq!(x(4), [-2.0, 1.0, -0.0]);
    }
}
