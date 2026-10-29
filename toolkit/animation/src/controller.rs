//! One chosen state's clip and time mapping. No transitions, layer composition, masks
//! or blend-tree duration rule are invented here. The core controller retains those facts.

use crate::layers::{self, LayerPlay};
use crate::pose::Evaluated;
use crate::{Error, Result};
use primase_core::{
    Clip, Controller, ControllerParameterKind, Entity, JointId, LayerBlending, LayerMask, Motion, RigPose,
    Skeleton,
};
use std::collections::BTreeMap;

/// The resolved clip of a state, with no source object identity or World access.
#[derive(Clone, Debug, PartialEq)]
pub struct StateMotion {
    pub clip: Entity,
    /// Clip seconds per state second; state speed times its optional numeric parameter.
    pub speed: f64,
    /// Fraction of clip duration, stated by the state or its optional numeric parameter.
    pub cycle_offset: f32,
}

impl StateMotion {
    /// Before the clip's own looping: state_time * speed + cycle_offset * duration.
    pub fn clip_time(&self, state_time: f64, clip_duration: f32) -> f64 {
        state_time * self.speed + f64::from(self.cycle_offset) * f64::from(clip_duration)
    }
}

/// The parameter values are one per controller parameter, finite and of their declared kind (Bool/Trigger 0 or 1, Int
/// integral). No rounding or clamping.
pub fn check_values(controller: &Controller, values: &[f64]) -> Result<()> {
    if values.len() != controller.parameters.len() {
        return Err(Error(format!(
            "controller {:?}: {} values for {} parameters",
            controller.name,
            values.len(),
            controller.parameters.len()
        )));
    }
    for (parameter, &value) in controller.parameters.iter().zip(values) {
        if !value.is_finite() {
            return Err(Error(format!(
                "parameter {:?}: non-finite value {value}",
                parameter.name
            )));
        }
        match parameter.kind {
            ControllerParameterKind::Int if value.fract() != 0.0 => {
                return Err(Error(format!(
                    "parameter {:?}: fractional Int value {value}",
                    parameter.name
                )));
            }
            ControllerParameterKind::Bool | ControllerParameterKind::Trigger
                if value != 0.0 && value != 1.0 =>
            {
                return Err(Error(format!(
                    "parameter {:?}: Bool/Trigger value {value} is not 0 or 1",
                    parameter.name
                )));
            }
            ControllerParameterKind::Float
            | ControllerParameterKind::Int
            | ControllerParameterKind::Bool
            | ControllerParameterKind::Trigger => {}
        }
    }
    Ok(())
}

/// Resolve one state of one layer. Values are positional in controller.parameters, finite
/// and of their declared kind (Bool/Trigger 0 or 1, Int integral). No rounding/clamping.
/// Blend1D and Unsupported motions refuse by name: choosing one child or inventing
/// a weighted clip duration would change the source's animation semantics.
pub fn motion(controller: &Controller, layer: usize, state: usize, values: &[f64]) -> Result<StateMotion> {
    check_values(controller, values)?;
    let layer = controller
        .layers
        .get(layer)
        .ok_or_else(|| Error(format!("layer {layer} outside controller {:?}", controller.name)))?;
    let state = layer
        .states
        .get(state)
        .ok_or_else(|| Error(format!("state {state} outside layer {:?}", layer.name)))?;
    let numeric = |index: usize| -> Result<f64> {
        let parameter = controller.parameters.get(index).ok_or_else(|| {
            Error(format!(
                "state {:?}: parameter index {index} is absent",
                state.name
            ))
        })?;
        if !matches!(
            parameter.kind,
            ControllerParameterKind::Float | ControllerParameterKind::Int
        ) {
            return Err(Error(format!(
                "state {:?}: parameter {:?} is not numeric",
                state.name, parameter.name
            )));
        }
        Ok(values[index])
    };
    let clip = match &state.motion {
        Motion::Clip(entity) => *entity,
        Motion::Blend1D { .. } => {
            return Err(Error(format!(
                "state {:?}: Blend1D clip-time rule is not established",
                state.name
            )));
        }
        Motion::Pending(_) => {
            return Err(Error(format!(
                "state {:?}: its clip is not loaded yet",
                state.name
            )));
        }
        Motion::Unsupported(reason) => {
            return Err(Error(format!(
                "state {:?}: unsupported motion: {reason}",
                state.name
            )));
        }
    };
    let speed = state.speed
        * match state.speed_parameter {
            Some(index) => numeric(index)?,
            None => 1.0,
        };
    let cycle_offset = match state.cycle_offset_parameter {
        Some(index) => numeric(index)? as f32,
        None => state.cycle_offset,
    };
    if !state.speed.is_finite() || !speed.is_finite() || !cycle_offset.is_finite() {
        return Err(Error(format!(
            "state {:?}: speed or cycle offset is not finite",
            state.name
        )));
    }
    Ok(StateMotion {
        clip,
        speed,
        cycle_offset,
    })
}

/// What a layer does at the chosen states and parameter values.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerAction {
    /// Contributes nothing: the layer has no states, or its weight is 0 (whatever its states hold).
    Nothing,
    /// Plays `state`.
    Plays { state: usize, motion: StateMotion },
    /// The layer is applicable but its chosen state's motion is not (a state machine, a blend, a refusal the source
    /// stated): why. A clip given for the layer ([`Slot`]) is played in its place.
    Unplayable(String),
    /// The layer cannot be applied by core's rule (a blending mode, a mask or a weight it has no form for): why. Nothing
    /// takes its place.
    Refused(String),
}

/// One layer of a controller resolved for its rig's skeleton: the weight and mask it applies with, and what it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerPlan {
    pub layer: usize,
    pub name: String,
    /// The weight in 0 to 1: the weight parameter's value, else the default weight.
    pub weight: f64,
    /// The layer is additive: its clip must state itself a delta.
    pub requires_delta: bool,
    /// The joint weights on the rig's skeleton (the entries of the mask that name it); `None`: every joint at full
    /// strength.
    pub mask: Option<BTreeMap<JointId, f32>>,
    pub action: LayerAction,
}

/// Every layer of `controller` for the skeleton held by `skeleton`, in order, at the states chosen (`states[layer]`; a
/// layer past the end of the slice, or `None`, is in its default state) and the parameter `values` ([`check_values`]).
///
/// A layer is refused (not skipped) for what core states but the toolkit cannot apply: a blending mode other than
/// Override and Additive, a mask of the kind [`LayerMask::Unsupported`], a weight outside 0 to 1, a state outside its
/// states. Unless its weight is 0, which makes a layer contribute nothing whatever it holds.
pub fn layer_stack(
    controller: &Controller,
    skeleton: Entity,
    states: &[Option<usize>],
    values: &[f64],
) -> Result<Vec<LayerPlan>> {
    check_values(controller, values)?;
    let mut plans = Vec::with_capacity(controller.layers.len());
    for (index, layer) in controller.layers.iter().enumerate() {
        let weight = match layer.weight_parameter {
            Some(p) => *values
                .get(p)
                .ok_or_else(|| Error(format!("layer {:?}: weight parameter {p} is absent", layer.name)))?,
            None => f64::from(layer.default_weight),
        };
        let mask = match &layer.mask {
            Some(LayerMask::Joints(joints)) => Some(
                joints
                    .iter()
                    .filter(|j| j.skeleton == skeleton)
                    .map(|j| (j.joint, j.weight))
                    .collect::<BTreeMap<_, _>>(),
            ),
            _ => None,
        };
        let refused = if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            Some(format!("its weight {weight} is not in 0 to 1"))
        } else if let LayerBlending::Other(n) = layer.blending {
            Some(format!("its blending mode {n} has no composition rule here"))
        } else if let Some(LayerMask::Unsupported(why)) = &layer.mask {
            Some(format!("its mask is not stated: {why}"))
        } else {
            None
        };
        let action = match refused {
            _ if weight == 0.0 || layer.states.is_empty() => LayerAction::Nothing,
            Some(why) => LayerAction::Refused(why),
            None => match states.get(index).copied().flatten().or(layer.default_state) {
                Some(state) if state < layer.states.len() => match motion(controller, index, state, values) {
                    Ok(motion) => LayerAction::Plays { state, motion },
                    Err(why) => LayerAction::Unplayable(why.0),
                },
                Some(state) => LayerAction::Refused(format!(
                    "state {state} is outside its {} states",
                    layer.states.len()
                )),
                None => LayerAction::Nothing,
            },
        };
        plans.push(LayerPlan {
            layer: index,
            name: layer.name.clone(),
            weight,
            requires_delta: layer.blending == LayerBlending::Additive,
            mask,
            action,
        });
    }
    Ok(plans)
}

/// A clip played in a layer's place: the layer's weight, mask and blending apply to it.
#[derive(Clone, Copy, Debug)]
pub struct Slot<'a> {
    pub layer: usize,
    pub clip: &'a Clip,
    /// Seconds into the clip.
    pub time: f64,
}

/// The pose of `plans` composed ([`layers::compose`]) over `base`, the layers' clips found by `clip_of` and played at
/// `state_time` seconds into their states, with `slot` (if any) played instead of its layer's state.
///
/// A layer that plays a clip `clip_of` does not have, one that is [`LayerAction::Unplayable`] with no slot to fill it,
/// and one that is [`LayerAction::Refused`] are errors naming the layer and why: the pose of a stack with a layer left
/// out would be another pose than the source's, not a partial one.
pub fn evaluate_stack<'a>(
    skeleton_entity: Entity,
    skeleton: &Skeleton,
    base: Option<&RigPose>,
    plans: &'a [LayerPlan],
    clip_of: &dyn Fn(Entity) -> Option<&'a Clip>,
    state_time: f64,
    slot: Option<Slot<'a>>,
) -> Result<Evaluated> {
    if let Some(s) = slot {
        match plans.get(s.layer).map(|p| &p.action) {
            None => {
                return Err(Error(format!(
                    "layer {} outside the {} layers",
                    s.layer,
                    plans.len()
                )));
            }
            Some(LayerAction::Refused(why)) => {
                return Err(Error(format!(
                    "layer {:?} cannot be filled: {why}",
                    plans[s.layer].name
                )));
            }
            Some(_) => {}
        }
    }
    let mut plays: Vec<LayerPlay<'_>> = Vec::new();
    for plan in plans {
        let slotted = slot.filter(|s| s.layer == plan.layer);
        let (clip, time) = match (&plan.action, slotted) {
            (LayerAction::Nothing, _) => continue,
            (_, Some(s)) => (s.clip, s.time),
            (LayerAction::Plays { motion, .. }, None) => {
                let clip = clip_of(motion.clip).ok_or_else(|| {
                    Error(format!(
                        "layer {:?}: clip entity {} is not loaded",
                        plan.name,
                        motion.clip.index()
                    ))
                })?;
                (clip, motion.clip_time(state_time, clip.duration))
            }
            (LayerAction::Unplayable(why), None) => {
                return Err(Error(format!(
                    "layer {:?}: its state's motion is not applied: {why}",
                    plan.name
                )));
            }
            (LayerAction::Refused(why), None) => {
                return Err(Error(format!("layer {:?} is not applied: {why}", plan.name)));
            }
        };
        plays.push(LayerPlay {
            clip,
            time,
            weight: plan.weight,
            mask: plan.mask.as_ref(),
            requires_delta: plan.requires_delta,
        });
    }
    layers::compose(skeleton_entity, skeleton, base, &plays)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{ControllerLayer, ControllerParameter, ControllerState, LayerBlending};

    fn controller() -> Controller {
        Controller {
            name: "controller".into(),
            parameters: vec![
                ControllerParameter {
                    name: "speed".into(),
                    kind: ControllerParameterKind::Float,
                    default: 1.0,
                },
                ControllerParameter {
                    name: "offset".into(),
                    kind: ControllerParameterKind::Int,
                    default: 0.0,
                },
                ControllerParameter {
                    name: "enabled".into(),
                    kind: ControllerParameterKind::Bool,
                    default: 1.0,
                },
            ],
            layers: vec![ControllerLayer {
                name: "layer".into(),
                default_state: Some(0),
                default_weight: 0.25,
                weight_parameter: None,
                mask: None,
                blending: LayerBlending::Other(17),
                states: vec![ControllerState {
                    name: "walk".into(),
                    motion: Motion::Clip(Entity::from_index(8)),
                    speed: 2.0,
                    speed_parameter: Some(0),
                    cycle_offset: 0.25,
                    cycle_offset_parameter: Some(1),
                }],
            }],
        }
    }

    #[test]
    fn state_time_uses_numeric_speed_and_offset_without_source_identity() {
        let c = controller();
        let sampled = motion(&c, 0, 0, &[-0.5, 2.0, 1.0]).unwrap();
        assert_eq!(
            sampled,
            StateMotion {
                clip: Entity::from_index(8),
                speed: -1.0,
                cycle_offset: 2.0
            }
        );
        assert_eq!(sampled.clip_time(3.0, 4.0), 5.0);
        let mut c = c;
        c.layers[0].states[0].speed_parameter = None;
        c.layers[0].states[0].cycle_offset_parameter = None;
        let sampled = motion(&c, 0, 0, &[0.0, 0.0, 0.0]).unwrap();
        assert_eq!(sampled.clip_time(3.0, 4.0), 7.0);
    }

    #[test]
    fn unsupported_motions_are_retained_but_never_replaced_by_a_child() {
        let mut c = controller();
        c.layers[0].states[0].motion = Motion::Blend1D {
            parameter: 0,
            children: vec![(0.0, Motion::Clip(Entity::from_index(8)))],
        };
        assert!(
            motion(&c, 0, 0, &[0.0, 0.0, 1.0])
                .unwrap_err()
                .0
                .contains("Blend1D")
        );
        c.layers[0].states[0].motion = Motion::Unsupported("2D source tree".into());
        assert!(
            motion(&c, 0, 0, &[0.0, 0.0, 1.0])
                .unwrap_err()
                .0
                .contains("2D source tree")
        );
    }

    #[test]
    fn invalid_indices_parameter_types_and_nonfinite_math_refuse() {
        let c = controller();
        assert!(motion(&c, 1, 0, &[1.0, 0.0, 1.0]).is_err());
        assert!(motion(&c, 0, 1, &[1.0, 0.0, 1.0]).is_err());
        // a layer with no states plays nothing: its state 0 is outside it, named by the layer
        let mut empty = c.clone();
        empty.layers[0].states.clear();
        empty.layers[0].default_state = None;
        let e = motion(&empty, 0, 0, &[1.0, 0.0, 1.0]).unwrap_err().0;
        assert!(e.contains("state 0 outside layer \"layer\""), "{e}");
        for values in [
            vec![],
            vec![f64::NAN, 0.0, 1.0],
            vec![1.0, 0.5, 1.0],
            vec![1.0, 0.0, 2.0],
        ] {
            assert!(motion(&c, 0, 0, &values).is_err());
        }
        let mut c = c;
        c.layers[0].states[0].speed_parameter = Some(2);
        assert!(
            motion(&c, 0, 0, &[1.0, 0.0, 1.0])
                .unwrap_err()
                .0
                .contains("not numeric")
        );
        c.layers[0].states[0].speed_parameter = Some(0);
        c.layers[0].states[0].speed = f64::MAX;
        assert!(motion(&c, 0, 0, &[2.0, 0.0, 1.0]).is_err());
    }

    // ---- layer stacks ----

    fn skel() -> Entity {
        Entity::from_index(5)
    }

    fn rotating(name: &str, joint: u32, quarter_turn: bool) -> Clip {
        use primase_core::{Channel, ClipTarget, Extrapolation, Interpolation, Property};
        let s = std::f32::consts::FRAC_1_SQRT_2;
        Clip {
            name: name.into(),
            duration: 1.0,
            extrapolation: Extrapolation::Hold,
            channels: vec![Channel {
                target: ClipTarget::Joint {
                    skeleton: skel(),
                    joint: primase_core::JointId(joint),
                },
                property: Property::Rotation,
                interpolation: Interpolation::Step,
                times: vec![0.0],
                values: if quarter_turn {
                    vec![0.0, 0.0, s, s]
                } else {
                    vec![0.0, 0.0, 0.0, 1.0]
                },
            }],
            root_motion: None,
            additive: None,
        }
    }

    fn state(name: &str, motion: Motion) -> ControllerState {
        ControllerState {
            name: name.into(),
            motion,
            speed: 1.0,
            speed_parameter: None,
            cycle_offset: 0.0,
            cycle_offset_parameter: None,
        }
    }

    fn layer(name: &str, weight: f32, states: Vec<ControllerState>) -> ControllerLayer {
        ControllerLayer {
            name: name.into(),
            default_state: (!states.is_empty()).then_some(0),
            states,
            default_weight: weight,
            weight_parameter: None,
            blending: LayerBlending::Override,
            mask: None,
        }
    }

    /// A hand layer of two selectable poses under a state machine's layer (its motion not lowered) and a layer whose
    /// weight a parameter sets (the shape of Elden Ring's player graph).
    fn stack() -> Controller {
        let mut weighted = layer(
            "weighted",
            0.0,
            vec![state("clip", Motion::Clip(Entity::from_index(12)))],
        );
        weighted.weight_parameter = Some(0);
        Controller {
            name: "stack".into(),
            parameters: vec![ControllerParameter {
                name: "blend".into(),
                kind: ControllerParameterKind::Float,
                default: 0.0,
            }],
            layers: vec![
                layer(
                    "hands",
                    1.0,
                    vec![
                        state("fist", Motion::Clip(Entity::from_index(10))),
                        state("open", Motion::Clip(Entity::from_index(11))),
                    ],
                ),
                layer(
                    "machine",
                    1.0,
                    vec![state("idle", Motion::Unsupported("a state machine".into()))],
                ),
                weighted,
            ],
        }
    }

    #[test]
    fn a_layer_stack_resolves_states_weights_and_masks() {
        let c = stack();
        let plans = layer_stack(&c, skel(), &[], &[0.0]).unwrap();
        assert!(
            matches!(&plans[0].action, LayerAction::Plays { state: 0, motion } if motion.clip == Entity::from_index(10))
        );
        assert!(matches!(&plans[1].action, LayerAction::Unplayable(why) if why.contains("a state machine")));
        assert_eq!(
            plans[2].action,
            LayerAction::Nothing,
            "weight 0 contributes nothing"
        );
        // a chosen state, and a weight parameter at 0.5
        let plans = layer_stack(&c, skel(), &[Some(1)], &[0.5]).unwrap();
        assert!(matches!(&plans[0].action, LayerAction::Plays { state: 1, .. }));
        assert_eq!(plans[2].weight, 0.5);
        assert!(matches!(&plans[2].action, LayerAction::Plays { .. }));
        // refusals are named: a state outside, a weight outside 0 to 1, a blending mode, a mask
        let plans = layer_stack(&c, skel(), &[Some(2)], &[0.0]).unwrap();
        assert!(matches!(&plans[0].action, LayerAction::Refused(why) if why.contains("outside")));
        let plans = layer_stack(&c, skel(), &[], &[1.5]).unwrap();
        assert!(matches!(&plans[2].action, LayerAction::Refused(why) if why.contains("not in 0 to 1")));
        let mut other = stack();
        other.layers[0].blending = LayerBlending::Other(9);
        other.layers[1].mask = Some(primase_core::LayerMask::Unsupported("body mask".into()));
        let plans = layer_stack(&other, skel(), &[], &[0.0]).unwrap();
        assert!(matches!(&plans[0].action, LayerAction::Refused(why) if why.contains("blending mode 9")));
        assert!(matches!(&plans[1].action, LayerAction::Refused(why) if why.contains("body mask")));
        // an additive layer asks for a delta clip
        let mut additive = stack();
        additive.layers[0].blending = LayerBlending::Additive;
        assert!(layer_stack(&additive, skel(), &[], &[0.0]).unwrap()[0].requires_delta);
        // a mask keeps the entries of this rig's skeleton only
        let mut masked = stack();
        let joint = |skeleton, joint: u32, weight| primase_core::MaskedJoint {
            skeleton,
            joint: primase_core::JointId(joint),
            weight,
        };
        masked.layers[0].mask = Some(primase_core::LayerMask::Joints(vec![
            joint(skel(), 1, 1.0),
            joint(Entity::from_index(6), 2, 1.0),
        ]));
        let mask = layer_stack(&masked, skel(), &[], &[0.0]).unwrap()[0]
            .mask
            .clone()
            .unwrap();
        assert_eq!(mask.len(), 1);
        assert_eq!(mask[&primase_core::JointId(1)], 1.0);
        // parameter values are checked once for the stack
        assert!(layer_stack(&c, skel(), &[], &[]).is_err());
    }

    #[test]
    fn a_stack_plays_the_hand_layer_under_a_clip_given_for_the_machines_slot() {
        use primase_core::{Rest, Skeleton};
        let c = stack();
        let rest = Rest::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        let skeleton = Skeleton {
            joints: (0..2)
                .map(|i| primase_core::Joint {
                    id: primase_core::JointId(i),
                    name: format!("j{i}"),
                    parent: None,
                    rest,
                })
                .collect(),
        };
        // the hand layer's fist turns joint 1; the open hand turns nothing; the played clip drives joint 0 only
        let fist = rotating("fist", 1, true);
        let open = rotating("open", 1, false);
        let walk = rotating("walk", 0, true);
        let clips = [(Entity::from_index(10), fist), (Entity::from_index(11), open)];
        let clip_of = |e: Entity| clips.iter().find(|(k, _)| *k == e).map(|(_, c)| c);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let plans = layer_stack(&c, skel(), &[], &[0.0]).unwrap();
        let slot = Slot {
            layer: 1,
            clip: &walk,
            time: 0.0,
        };
        let e = evaluate_stack(skel(), &skeleton, None, &plans, &clip_of, 0.0, Some(slot)).unwrap();
        let rot = |id: u32| match e
            .pose
            .joints
            .iter()
            .find(|(j, _)| *j == primase_core::JointId(id))
            .unwrap()
            .1
        {
            Rest::Trs { rotation, .. } => rotation,
            Rest::Affine(_) => panic!("TRS expected"),
        };
        assert_eq!(
            rot(1),
            [0.0, 0.0, s, s],
            "the fist survives the clip that does not drive it"
        );
        assert_eq!(rot(0), [0.0, 0.0, s, s]);
        // the open hand is the chosen state: joint 1 is the open pose's
        let plans = layer_stack(&c, skel(), &[Some(1)], &[0.0]).unwrap();
        let e = evaluate_stack(skel(), &skeleton, None, &plans, &clip_of, 0.0, Some(slot)).unwrap();
        let open_rot = match e
            .pose
            .joints
            .iter()
            .find(|(j, _)| *j == primase_core::JointId(1))
            .unwrap()
            .1
        {
            Rest::Trs { rotation, .. } => rotation,
            Rest::Affine(_) => panic!("TRS expected"),
        };
        assert_eq!(open_rot, [0.0, 0.0, 0.0, 1.0]);
        // without a clip for the machine's slot the stack is refused by name, not posed without the layer
        let err = evaluate_stack(skel(), &skeleton, None, &plans, &clip_of, 0.0, None).unwrap_err();
        assert!(
            err.0.contains("machine") && err.0.contains("a state machine"),
            "{}",
            err.0
        );
        // a clip the stack does not have is named
        let none = |_: Entity| None;
        let err = evaluate_stack(skel(), &skeleton, None, &plans, &none, 0.0, Some(slot)).unwrap_err();
        assert!(
            err.0.contains("hands") && err.0.contains("not loaded"),
            "{}",
            err.0
        );
        // a refused layer cannot be filled, and a layer outside the stack is an error
        let mut refused = stack();
        refused.layers[1].blending = LayerBlending::Other(3);
        let plans = layer_stack(&refused, skel(), &[], &[0.0]).unwrap();
        let err = evaluate_stack(skel(), &skeleton, None, &plans, &clip_of, 0.0, Some(slot)).unwrap_err();
        assert!(err.0.contains("cannot be filled"), "{}", err.0);
        let plans = layer_stack(&c, skel(), &[], &[0.0]).unwrap();
        let outside = Slot { layer: 9, ..slot };
        assert!(evaluate_stack(skel(), &skeleton, None, &plans, &clip_of, 0.0, Some(outside)).is_err());
    }
}
