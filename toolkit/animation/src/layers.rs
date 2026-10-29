//! Layers of clips composed into one pose, by the rule `core.controller` states: each layer plays its clip over the pose
//! below it and the result is mixed from the pose below toward that one by the layer's weight and, for a joint, the
//! joint's weight in the layer's mask. A joint or channel the clip does not drive is the pose below's already, so mixing it
//! changes nothing.
//!
//! The mix of a joint is [`pose::blend`]'s own (translations and scales the weighted sum, rotations the weighted sum of
//! the unit quaternions in one hemisphere, normalised). A joint weight of 0 returns the pose below exactly, and one of
//! 1 the layer's exactly, without a renormalisation. A mask weighs joints only; a morph, control, material or root
//! motion channel the layer drives is mixed by the layer's weight, and a draw group (a discrete value) is the layer's at
//! weight 1. A value with nothing below it to mix with, at a weight between 0 and 1, is an error: the baseline it would
//! be mixed with is not known here.

use std::collections::BTreeMap;

use primase_core::{Clip, Entity, JointId, Rest, RigPose, Skeleton};

use crate::pose::{self, Evaluated, Trs};
use crate::{Error, Result};

/// One layer to compose: its clip at a time, its weight and its joint weights.
#[derive(Clone, Debug)]
pub struct LayerPlay<'a> {
    pub clip: &'a Clip,
    /// Seconds into the clip, as [`pose::evaluate`] takes them.
    pub time: f64,
    /// Finite, 0 to 1 (never clamped).
    pub weight: f64,
    /// The weight of each joint; a joint not listed has weight 0. `None`: every joint at full strength.
    pub mask: Option<&'a BTreeMap<JointId, f32>>,
    /// The layer is additive: its clip must state itself a delta (`Clip::additive`), else the layer is refused.
    pub requires_delta: bool,
}

/// `layers` composed from the first (the bottom) over `base` (else the skeleton's rests), on the skeleton held by
/// `skeleton_entity`.
///
/// A layer of weight 0 contributes nothing and its clip is not evaluated. An error names the layer by its position.
pub fn compose(
    skeleton_entity: Entity,
    skeleton: &Skeleton,
    base: Option<&RigPose>,
    layers: &[LayerPlay<'_>],
) -> Result<Evaluated> {
    let locals = pose::locals(skeleton, base);
    let mut acc = Evaluated {
        pose: RigPose {
            joints: skeleton.joints.iter().map(|j| (j.id, locals[&j.id])).collect(),
            root_applied: base.and_then(|b| b.root_applied),
        },
        morphs: Vec::new(),
        controls: Vec::new(),
        draw_groups: Vec::new(),
        material_parameters: Vec::new(),
        root: None,
        other_skeletons: 0,
    };
    for (at, layer) in layers.iter().enumerate() {
        let wrap = |e: Error| Error(format!("layer {at}: {}", e.0));
        if !layer.weight.is_finite() || !(0.0..=1.0).contains(&layer.weight) {
            return Err(wrap(Error(format!("weight {} is not in 0 to 1", layer.weight))));
        }
        if layer.weight == 0.0 {
            continue;
        }
        if layer.requires_delta && layer.clip.additive.is_none() {
            return Err(wrap(Error(format!(
                "an additive layer plays clip {:?}, which does not state itself a delta",
                layer.clip.name
            ))));
        }
        let ev = pose::evaluate(layer.clip, skeleton_entity, skeleton, Some(&acc.pose), layer.time)
            .map_err(wrap)?;
        mix_into(&mut acc, &ev, layer.weight, layer.mask).map_err(wrap)?;
    }
    Ok(acc)
}

/// `upper` (a clip evaluated over `acc`'s pose) mixed into `acc`.
fn mix_into(
    acc: &mut Evaluated,
    upper: &Evaluated,
    weight: f64,
    mask: Option<&BTreeMap<JointId, f32>>,
) -> Result<()> {
    if upper.pose.joints.len() != acc.pose.joints.len() {
        return Err(Error("the layer's pose is of other joints".into()));
    }
    for ((id, lower), (uid, upper)) in acc.pose.joints.iter_mut().zip(&upper.pose.joints) {
        if id != uid {
            return Err(Error("the layer's pose is of other joints".into()));
        }
        let w = weight
            * match mask {
                None => 1.0,
                Some(m) => f64::from(m.get(id).copied().unwrap_or(0.0)),
            };
        if w <= 0.0 || *lower == *upper {
            continue;
        }
        if w >= 1.0 {
            *lower = *upper;
            continue;
        }
        let parts = [
            (
                Trs::of(lower).map_err(|e| Error(format!("joint {id:?}: {}", e.0)))?,
                1.0 - w,
            ),
            (
                Trs::of(upper).map_err(|e| Error(format!("joint {id:?}: {}", e.0)))?,
                w,
            ),
        ];
        *lower = pose::blend_trs(&parts)
            .map_err(|e| Error(format!("joint {id:?}: {}", e.0)))?
            .rest();
    }
    acc.pose.root_applied = upper.pose.root_applied;
    acc.other_skeletons = acc.other_skeletons.max(upper.other_skeletons);
    for (geometry, morph, v) in &upper.morphs {
        let at = acc.morphs.iter().position(|m| m.0 == *geometry && m.1 == *morph);
        let lower = at.map(|i| f64::from(acc.morphs[i].2));
        let mixed = mix_scalar(lower, f64::from(*v), weight, &format!("morph {morph:?}"))? as f32;
        match at {
            Some(i) => acc.morphs[i].2 = mixed,
            None => acc.morphs.push((*geometry, *morph, mixed)),
        }
    }
    for (name, v) in &upper.controls {
        let at = acc.controls.iter().position(|c| c.0 == *name);
        let lower = at.map(|i| f64::from(acc.controls[i].1));
        let mixed = mix_scalar(lower, f64::from(*v), weight, &format!("control {name:?}"))? as f32;
        match at {
            Some(i) => acc.controls[i].1 = mixed,
            None => acc.controls.push((name.clone(), mixed)),
        }
    }
    for (scope, group, v) in &upper.draw_groups {
        if weight != 1.0 {
            return Err(Error(format!(
                "draw group {scope:?}/{group} is a discrete value: it takes a layer's only at weight 1, not {weight}"
            )));
        }
        match acc
            .draw_groups
            .iter_mut()
            .find(|d| d.0 == *scope && d.1 == *group)
        {
            Some(d) => d.2 = *v,
            None => acc.draw_groups.push((scope.clone(), *group, *v)),
        }
    }
    for (entity, name, v) in &upper.material_parameters {
        let at = acc
            .material_parameters
            .iter()
            .position(|m| m.0 == *entity && m.1 == *name);
        match at {
            Some(i) => {
                let lower = &acc.material_parameters[i].2;
                if lower.len() != v.len() {
                    return Err(Error(format!(
                        "material parameter {name:?} changes width between layers"
                    )));
                }
                let mixed: Vec<f32> = lower
                    .iter()
                    .zip(v)
                    .map(|(l, u)| {
                        mix_scalar(Some(f64::from(*l)), f64::from(*u), weight, name).map(|x| x as f32)
                    })
                    .collect::<Result<_>>()?;
                acc.material_parameters[i].2 = mixed;
            }
            None => {
                let mixed: Vec<f32> = v
                    .iter()
                    .map(|u| mix_scalar(None, f64::from(*u), weight, name).map(|x| x as f32))
                    .collect::<Result<_>>()?;
                acc.material_parameters.push((*entity, name.clone(), mixed));
            }
        }
    }
    if let Some(root) = upper.root {
        acc.root = Some(match acc.root {
            _ if weight >= 1.0 => root,
            None => {
                return Err(Error(format!(
                    "root motion has nothing below it to mix with at weight {weight}"
                )));
            }
            Some(lower) => {
                let parts = [
                    (
                        Trs::of(&Rest::Affine(lower)).map_err(|e| Error(format!("root motion: {}", e.0)))?,
                        1.0 - weight,
                    ),
                    (
                        Trs::of(&Rest::Affine(root)).map_err(|e| Error(format!("root motion: {}", e.0)))?,
                        weight,
                    ),
                ];
                let blended = pose::blend_trs(&parts).map_err(|e| Error(format!("root motion: {}", e.0)))?;
                pose::transform(&blended.matrix())
            }
        });
    }
    Ok(())
}

/// A scalar the layer drives, mixed with the value below it by `weight`; with none below, only at weight 1.
fn mix_scalar(lower: Option<f64>, upper: f64, weight: f64, what: &str) -> Result<f64> {
    match lower {
        _ if weight >= 1.0 => Ok(upper),
        Some(l) => Ok(l + (upper - l) * weight),
        None => Err(Error(format!(
            "{what} has nothing below it to mix with at weight {weight}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{
        AdditiveReference, Channel, ClipTarget, Extrapolation, Interpolation, Joint, MorphId, Property,
    };

    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
    fn skel() -> Entity {
        Entity::from_index(5)
    }

    /// Joints 0 (the root), 1 (its child, the "hand") and 2 (another child, the "face"), at the identity.
    fn skeleton() -> Skeleton {
        let rest = Rest::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        Skeleton {
            joints: (0..3)
                .map(|i| Joint {
                    id: JointId(i),
                    name: format!("j{i}"),
                    parent: (i > 0).then_some(JointId(0)),
                    rest,
                })
                .collect(),
        }
    }

    fn channel(target: ClipTarget, property: Property, values: &[f32]) -> Channel {
        Channel {
            target,
            property,
            interpolation: Interpolation::Step,
            times: vec![0.0],
            values: values.to_vec(),
        }
    }

    fn joint(joint: u32, property: Property, values: &[f32]) -> Channel {
        channel(
            ClipTarget::Joint {
                skeleton: skel(),
                joint: JointId(joint),
            },
            property,
            values,
        )
    }

    fn clip(name: &str, additive: Option<AdditiveReference>, channels: Vec<Channel>) -> Clip {
        Clip {
            name: name.into(),
            duration: 1.0,
            extrapolation: Extrapolation::Hold,
            channels,
            root_motion: None,
            additive,
        }
    }

    fn play<'a>(clip: &'a Clip, weight: f64, mask: Option<&'a BTreeMap<JointId, f32>>) -> LayerPlay<'a> {
        LayerPlay {
            clip,
            time: 0.0,
            weight,
            mask,
            requires_delta: false,
        }
    }

    fn rotation_of(e: &Evaluated, id: u32) -> [f32; 4] {
        let Rest::Trs { rotation, .. } = e.pose.joints.iter().find(|(j, _)| *j == JointId(id)).unwrap().1
        else {
            panic!("joint {id} is not TRS")
        };
        rotation
    }

    fn translation_of(e: &Evaluated, id: u32) -> [f32; 3] {
        let Rest::Trs { translation, .. } = e.pose.joints.iter().find(|(j, _)| *j == JointId(id)).unwrap().1
        else {
            panic!("joint {id} is not TRS")
        };
        translation
    }

    /// The hand pose below and a walk above that does not drive the hand (the shape of the finger case, in miniature):
    /// the hand keeps the lower layer's pose exactly, the walk's own joint is the walk's, in either order.
    #[test]
    fn a_joint_the_upper_clip_does_not_drive_keeps_the_lower_layers_pose() {
        let s = skeleton();
        let hand = clip(
            "hand",
            None,
            vec![joint(1, Property::Rotation, &[0.0, 0.0, S, S])],
        );
        let walk = clip(
            "walk",
            None,
            vec![joint(0, Property::Translation, &[1.0, 2.0, 3.0])],
        );
        let e = compose(
            skel(),
            &s,
            None,
            &[play(&hand, 1.0, None), play(&walk, 1.0, None)],
        )
        .unwrap();
        assert_eq!(
            rotation_of(&e, 1),
            [0.0, 0.0, S, S],
            "kept exactly, not renormalised"
        );
        assert_eq!(translation_of(&e, 0), [1.0, 2.0, 3.0]);
        assert_eq!(rotation_of(&e, 2), [0.0, 0.0, 0.0, 1.0]);
        let e = compose(
            skel(),
            &s,
            None,
            &[play(&walk, 1.0, None), play(&hand, 1.0, None)],
        )
        .unwrap();
        assert_eq!(rotation_of(&e, 1), [0.0, 0.0, S, S]);
        assert_eq!(translation_of(&e, 0), [1.0, 2.0, 3.0]);
    }

    /// Weight w mixes a driven joint from the pose below toward the layer's: half a quarter turn at 0.5 (hand-worked:
    /// the eighth turn about z is (0, 0, sin 22.5 degrees, cos 22.5 degrees)); 0 leaves it exactly; a weight outside 0
    /// to 1 is an error, not clamped.
    #[test]
    fn a_layers_weight_mixes_toward_its_pose() {
        let s = skeleton();
        let turn = clip(
            "turn",
            None,
            vec![joint(1, Property::Rotation, &[0.0, 0.0, S, S])],
        );
        let eighth = [
            0.0,
            0.0,
            (std::f32::consts::PI / 8.0).sin(),
            (std::f32::consts::PI / 8.0).cos(),
        ];
        let e = compose(skel(), &s, None, &[play(&turn, 0.5, None)]).unwrap();
        for (a, b) in rotation_of(&e, 1).iter().zip(&eighth) {
            assert!((a - b).abs() < 1e-6, "{:?} vs {eighth:?}", rotation_of(&e, 1));
        }
        let e = compose(skel(), &s, None, &[play(&turn, 0.0, None)]).unwrap();
        assert_eq!(rotation_of(&e, 1), [0.0, 0.0, 0.0, 1.0]);
        for bad in [-0.1, 1.1, f64::NAN] {
            let err = compose(skel(), &s, None, &[play(&turn, bad, None)]).unwrap_err();
            assert!(
                err.0.contains("layer 0") && err.0.contains("not in 0 to 1"),
                "{}",
                err.0
            );
        }
    }

    /// A mask weighs joints: a joint not listed is not driven, a listed one is mixed by weight times its weight.
    #[test]
    fn a_mask_lists_the_joints_a_layer_drives() {
        let s = skeleton();
        let both = clip(
            "both",
            None,
            vec![
                joint(1, Property::Rotation, &[0.0, 0.0, S, S]),
                joint(2, Property::Rotation, &[0.0, 0.0, S, S]),
            ],
        );
        let mask: BTreeMap<JointId, f32> = [(JointId(2), 1.0), (JointId(0), 1.0)].into();
        let e = compose(skel(), &s, None, &[play(&both, 1.0, Some(&mask))]).unwrap();
        assert_eq!(
            rotation_of(&e, 1),
            [0.0, 0.0, 0.0, 1.0],
            "joint 1 is not in the mask"
        );
        assert_eq!(rotation_of(&e, 2), [0.0, 0.0, S, S]);
        let half: BTreeMap<JointId, f32> = [(JointId(2), 0.5)].into();
        let e = compose(skel(), &s, None, &[play(&both, 0.5, Some(&half))]).unwrap();
        // 0.5 * 0.5 = 0.25 of the way, as `pose::blend` mixes: 0.75 * (0, 0, 0, 1) + 0.25 * (0, 0, S, S), normalised
        let (z, w) = (0.25 * S, 0.75 + 0.25 * S);
        let expected = z / z.hypot(w);
        assert!(
            (rotation_of(&e, 2)[2] - expected).abs() < 1e-6,
            "{:?}",
            rotation_of(&e, 2)
        );
    }

    /// An additive layer must play a clip that states itself a delta; one that does is composed onto the pose below (a
    /// delta quarter turn about z on a quarter turn about z is a half turn).
    #[test]
    fn an_additive_layer_needs_a_clip_that_states_itself_a_delta() {
        let s = skeleton();
        let base = clip(
            "base",
            None,
            vec![joint(1, Property::Rotation, &[0.0, 0.0, S, S])],
        );
        let plain = clip(
            "plain",
            None,
            vec![joint(1, Property::Rotation, &[0.0, 0.0, S, S])],
        );
        let delta = clip(
            "delta",
            Some(AdditiveReference::Identity),
            vec![joint(1, Property::Rotation, &[0.0, 0.0, S, S])],
        );
        let additive = |c| LayerPlay {
            requires_delta: true,
            ..play(c, 1.0, None)
        };
        let err = compose(skel(), &s, None, &[play(&base, 1.0, None), additive(&plain)]).unwrap_err();
        assert!(
            err.0.contains("layer 1") && err.0.contains("does not state itself a delta"),
            "{}",
            err.0
        );
        let e = compose(skel(), &s, None, &[play(&base, 1.0, None), additive(&delta)]).unwrap();
        let q = rotation_of(&e, 1);
        assert!(
            q[0].abs() < 1e-6 && q[1].abs() < 1e-6 && (q[2] - 1.0).abs() < 1e-6 && q[3].abs() < 1e-6,
            "{q:?}"
        );
    }

    /// A morph and a control a layer drives follow the joints' rule: mixed by weight with a value below, the layer's at
    /// weight 1, an error at a weight between with nothing below; a draw group is the layer's only at weight 1.
    #[test]
    fn other_channels_follow_the_same_rule() {
        let s = skeleton();
        let morph = |v: f32| {
            channel(
                ClipTarget::Morph {
                    geometry: Entity::from_index(9),
                    morph: MorphId(3),
                },
                Property::Scalar,
                &[v],
            )
        };
        let control = |v: f32| channel(ClipTarget::Control { name: "c".into() }, Property::Scalar, &[v]);
        let group = channel(
            ClipTarget::DrawGroup {
                scope: "s".into(),
                group: 2,
            },
            Property::Visibility,
            &[1.0],
        );
        let below = clip("below", None, vec![morph(1.0), control(4.0)]);
        let above = clip("above", None, vec![morph(0.0), control(0.0), group]);
        let e = compose(
            skel(),
            &s,
            None,
            &[play(&below, 1.0, None), play(&above, 1.0, None)],
        )
        .unwrap();
        assert_eq!(e.morphs, [(Entity::from_index(9), MorphId(3), 0.0)]);
        assert_eq!(e.draw_groups, [("s".to_owned(), 2, Some(true))]);
        let quiet = clip("quiet", None, vec![morph(0.0), control(0.0)]);
        let e = compose(
            skel(),
            &s,
            None,
            &[play(&below, 1.0, None), play(&quiet, 0.25, None)],
        )
        .unwrap();
        assert_eq!(e.morphs[0].2, 0.75);
        assert_eq!(e.controls, [("c".to_owned(), 3.0)]);
        let err = compose(skel(), &s, None, &[play(&quiet, 0.5, None)]).unwrap_err();
        assert!(err.0.contains("nothing below it to mix with"), "{}", err.0);
        assert!(compose(skel(), &s, None, &[play(&quiet, 1.0, None)]).is_ok());
        let err = compose(
            skel(),
            &s,
            None,
            &[play(&below, 1.0, None), play(&above, 0.5, None)],
        )
        .unwrap_err();
        assert!(
            err.0.contains("draw group") && err.0.contains("weight 1"),
            "{}",
            err.0
        );
    }
}
