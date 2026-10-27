//! A clip lowered to a `core.clip` for the joints of a rig: the clip's curves (`unity_formats::clip`) sampled as Hermite
//! channels in the world's frame. The humanoid evaluation ([`crate::humanoid`]) and the passes use these.

use std::collections::BTreeSet;

use primase_core::{
    Channel, Clip, ClipTarget, Entity, Extrapolation, Interpolation, JointId, MorphId, Property, Rest,
};
use unity_formats::clip::{ClipData, Curve, Storage, Track};
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

/// This transform track as a joint channel on `target`, in the world's frame: its curves sampled at the union of
/// their key times, as a Hermite spline (each curve is a cubic between its keys, which the Hermite form with the curve's own
/// values and slopes at the keys states exactly; the tangents are per second), as a Linear channel when
/// every curve is dense or constant, and as a one-key channel when every curve is constant. `None` for a
/// binding that is not position, rotation or scale of a Transform. An error for steps mixed with smooth segments.
pub fn track_channel(
    track: &Track,
    target: ClipTarget,
    duration: f32,
    renormalised: &mut usize,
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
    let all_linear = track.curves.iter().all(|c| c.storage != Storage::Streamed);
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
                *renormalised += 1;
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
                    &mut report.renormalised_keys,
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
        let channel = track_channel(track, target, data.duration, &mut report.renormalised_keys)?
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
    report.channels = channels.len();
    Ok((
        Clip {
            name: data.name.clone(),
            duration: data.duration,
            extrapolation: if data.looping {
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

#[cfg(test)]
mod tests {
    use unity_formats::clip::{GenericBinding, Key};

    use super::*;

    fn key(time: f32, value: f32, in_slope: f32, out_slope: f32) -> Key {
        Key {
            time,
            value,
            in_slope,
            out_slope,
        }
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
        let channel = track_channel(&track, target, 0.5, &mut 0)
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
        let mut renormalised = 0;
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
            tracks,
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
}
