//! A layered clip-selection controller and its rig binding: the clips a source's states choose, the layers they play
//! in, and how a layer applies to the rig's joints (its weight, which a controller parameter may set, and its mask).
//! The animation toolkit evaluates it.
//!
//! Meaning (core's rule for layers; a source whose engine composes otherwise keeps that in its dialect):
//! - Layers compose in order, the first at the bottom. A layer plays its selected state's clip over the pose below it
//!   (`core.clip`'s own rule: a joint or channel the clip does not drive keeps the pose below, a delta clip is
//!   composed onto it), and the result is the pose below mixed toward that one, per joint, by the layer's weight times
//!   the joint's weight in its [`LayerMask`]. A morph, control, draw-group, material or root-motion channel follows the
//!   same rule: the layer does not change one its clip does not drive, and mixes one it drives by its weight. A
//!   discrete value (a draw group), and a value with nothing below it to mix with, take the layer's only at weight 1;
//!   at a weight between 0 and 1 that is a refusal, never a guess.
//! - The weight is the numeric parameter's value where [`ControllerLayer::weight_parameter`] names one, else
//!   [`ControllerLayer::default_weight`]; the toolkit refuses a weight outside 0..=1 instead of clamping it. A layer of
//!   weight 0 contributes nothing, whatever its states hold.
//! - [`LayerBlending::Override`] plays the clip as it stands. [`LayerBlending::Additive`] requires the clip to state
//!   itself a delta (`Clip::additive`): a clip that does not is refused by name, not taken as a delta by assumption.
//!   [`LayerBlending::Other`] is refused.
//! - A mask lists the joints the layer drives and their weights; a joint not listed has weight 0. A layer with no mask
//!   drives every joint at full strength. A mask core has no form for stays named ([`LayerMask::Unsupported`]) and the
//!   layer is refused by name.
//!
//! What stays in the source dialect: transitions between states, the events and fades that turn a layer on or off,
//! synchronised layers and IK. This projection neither applies them nor replaces them with something else.
use crate::{Clip, Component, Entity, Geometry, JointId, JsonError, RigInstance, Skeleton, World};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControllerParameterKind {
    Float,
    Int,
    Bool,
    Trigger,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerParameter {
    pub name: String,
    pub kind: ControllerParameterKind,
    /// Finite. Int is integral; Bool/Trigger are exactly 0 or 1. Never rounded or clamped.
    pub default: f64,
}

/// The layer's source-stated composition. Unknown values stay explicit, not guessed as override.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerBlending {
    Override,
    Additive,
    Other(i64),
}

/// A selected state's motion. Blend thresholds are finite and strictly increasing, one or more
/// children. A lowering keeps an unsupported tree as a named refusal instead of selecting a child.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Motion {
    Clip(Entity),
    /// A clip the source lists and has not loaded yet (the entity holds no `core.clip`, or not yet the one this state
    /// plays): the state plays it once it is loaded and the lowering that made this controller has run again, which states
    /// a [`Motion::Clip`]. Until then a consumer refuses the state by name, like [`Motion::Unsupported`], and may ask the
    /// source to load the entity. Not verified against the entity's components: it may hold `core.clip` already, between
    /// the load and that lowering.
    Pending(Entity),
    Blend1D {
        parameter: usize,
        children: Vec<(f32, Motion)>,
    },
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerState {
    pub name: String,
    pub motion: Motion,
    pub speed: f64,
    /// Numeric controller parameter multiplying speed, or none.
    pub speed_parameter: Option<usize>,
    pub cycle_offset: f32,
    /// Numeric controller parameter supplying the cycle offset, or none.
    pub cycle_offset_parameter: Option<usize>,
}

/// One joint's weight in a layer's mask, named as `core.clip` names a joint channel's target: by the skeleton and the
/// joint's identity in it (never by name). A controller may therefore be bound to any rig whose skeleton an entry names;
/// the entries of another skeleton do not apply to it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskedJoint {
    pub skeleton: Entity,
    #[serde(deserialize_with = "joint_id")]
    pub joint: JointId,
    /// Finite, 0 to 1; a source's weight outside that range is not lowered (never clamped).
    pub weight: f32,
}

fn joint_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<JointId, D::Error> {
    u32::deserialize(d).map(JointId)
}

/// An `Option` field that must be present in the JSON (`null` for none): serde fills an absent one in as `None`
/// otherwise, and for a mask that would read as "unmasked".
fn stated<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

/// Which joints a layer drives, and how strongly (see the module documentation).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum LayerMask {
    /// The listed joints at their weights; a joint not listed has weight 0. Each (skeleton, joint) once.
    Joints(Vec<MaskedJoint>),
    /// A mask the source states and core has no form for, or a layer rule it cannot state (the way
    /// [`Motion::Unsupported`] names a motion): a non-empty reason. The layer is refused by name, never applied unmasked.
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerLayer {
    pub name: String,
    pub states: Vec<ControllerState>,
    /// The state the layer starts in: an index of `states`. `None` exactly when the layer has no states: such a layer
    /// plays nothing (a source's empty layer, kept so the layers still line up with the source's).
    pub default_state: Option<usize>,
    /// As stored; finite, never silently clamped to a consumer's supported range. The weight the layer has when no
    /// parameter sets one (`weight_parameter`).
    pub default_weight: f32,
    /// A numeric controller parameter (an index of `parameters`) whose value is the layer's weight, where the source
    /// binds the weight to one; `None`: `default_weight`.
    #[serde(deserialize_with = "stated")]
    pub weight_parameter: Option<usize>,
    pub blending: LayerBlending,
    /// Which joints the layer drives; `None`: every joint at full strength. Stored as a `null` when none: a stored
    /// controller without the field is refused, so an absent mask never reads as an unmasked layer.
    #[serde(deserialize_with = "stated")]
    pub mask: Option<LayerMask>,
}

/// `core.controller`: states and the clip motions their parameters choose. Transitions and
/// body/skeleton masks remain in the source dialect until a lowering can represent their semantics.
/// Layer blending and default weights are retained even where a consumer cannot evaluate them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Controller {
    pub name: String,
    pub parameters: Vec<ControllerParameter>,
    pub layers: Vec<ControllerLayer>,
}

/// `core.controller_binding`: a controller driving a rig occurrence, not a skeleton definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerBinding {
    pub controller: Entity,
    pub rig: Entity,
}

impl Controller {
    fn numeric_parameter(&self, index: usize) -> Result<(), String> {
        let parameter = self
            .parameters
            .get(index)
            .ok_or_else(|| format!("parameter index {index} is out of range"))?;
        if !matches!(
            parameter.kind,
            ControllerParameterKind::Float | ControllerParameterKind::Int
        ) {
            return Err(format!(
                "parameter {} is {:?}, not numeric",
                parameter.name, parameter.kind
            ));
        }
        Ok(())
    }
}

impl Component for Controller {
    const NAME: &'static str = "core.controller";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn references(&self, out: &mut Vec<Entity>) {
        let mut pending: Vec<_> = self
            .layers
            .iter()
            .flat_map(|layer| layer.states.iter().map(|state| &state.motion))
            .collect();
        while let Some(motion) = pending.pop() {
            match motion {
                Motion::Clip(clip) | Motion::Pending(clip) => out.push(*clip),
                Motion::Blend1D { children, .. } => pending.extend(children.iter().map(|(_, motion)| motion)),
                Motion::Unsupported(_) => {}
            }
        }
        for layer in &self.layers {
            if let Some(LayerMask::Joints(joints)) = &layer.mask {
                out.extend(joints.iter().map(|j| j.skeleton));
            }
        }
    }
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("empty controller name".into());
        }
        let mut names = BTreeSet::new();
        for parameter in &self.parameters {
            if parameter.name.trim().is_empty() || !names.insert(&parameter.name) {
                return Err(format!(
                    "empty or duplicate controller parameter {:?}",
                    parameter.name
                ));
            }
            if !parameter.default.is_finite() {
                return Err(format!("{}: non-finite default", parameter.name));
            }
            match parameter.kind {
                ControllerParameterKind::Int if parameter.default.fract() != 0. => {
                    return Err(format!("{}: fractional Int default", parameter.name));
                }
                ControllerParameterKind::Bool | ControllerParameterKind::Trigger
                    if parameter.default != 0. && parameter.default != 1. =>
                {
                    return Err(format!("{}: Bool/Trigger default is not 0 or 1", parameter.name));
                }
                _ => {}
            }
        }
        for (layer_index, layer) in self.layers.iter().enumerate() {
            if layer.name.trim().is_empty() {
                return Err(format!("layer {layer_index}: empty name"));
            }
            if !layer.default_weight.is_finite() {
                return Err(format!("layer {layer_index}: non-finite weight"));
            }
            if let Some(parameter) = layer.weight_parameter {
                self.numeric_parameter(parameter)
                    .map_err(|why| format!("layer {layer_index}: weight {why}"))?;
            }
            match &layer.mask {
                None => {}
                Some(LayerMask::Unsupported(why)) if why.trim().is_empty() => {
                    return Err(format!("layer {layer_index}: unsupported mask has no reason"));
                }
                Some(LayerMask::Unsupported(_)) => {}
                Some(LayerMask::Joints(joints)) => {
                    let mut seen = BTreeSet::new();
                    for masked in joints {
                        if !masked.weight.is_finite() || !(0.0..=1.0).contains(&masked.weight) {
                            return Err(format!(
                                "layer {layer_index}: mask weight {} is not in 0 to 1",
                                masked.weight
                            ));
                        }
                        if !seen.insert((masked.skeleton, masked.joint)) {
                            return Err(format!(
                                "layer {layer_index}: joint {:?} is masked twice",
                                masked.joint
                            ));
                        }
                        let skeleton = w.get::<Skeleton>(masked.skeleton).ok_or_else(|| {
                            format!(
                                "layer {layer_index}: mask skeleton {:?} holds no core.skeleton",
                                masked.skeleton
                            )
                        })?;
                        if skeleton.index_of(masked.joint).is_none() {
                            return Err(format!(
                                "layer {layer_index}: masked joint {:?} is not in skeleton {:?}",
                                masked.joint, masked.skeleton
                            ));
                        }
                    }
                }
            }
            match layer.default_state {
                Some(default) if default >= layer.states.len() => {
                    return Err(format!("layer {layer_index}: default state is out of range"));
                }
                None if !layer.states.is_empty() => {
                    return Err(format!("layer {layer_index}: has states but no default state"));
                }
                _ => {}
            }
            for (state_index, state) in layer.states.iter().enumerate() {
                if state.name.trim().is_empty() {
                    return Err(format!("layer {layer_index} state {state_index}: empty name"));
                }
                if !state.speed.is_finite() || !state.cycle_offset.is_finite() {
                    return Err(format!(
                        "layer {layer_index} state {state_index}: non-finite speed/offset"
                    ));
                }
                for parameter in [state.speed_parameter, state.cycle_offset_parameter]
                    .into_iter()
                    .flatten()
                {
                    self.numeric_parameter(parameter)?;
                }
                let mut pending = vec![&state.motion];
                while let Some(motion) = pending.pop() {
                    match motion {
                        Motion::Clip(clip) => {
                            if !w.has::<Clip>(*clip) {
                                return Err(format!("motion {clip:?} holds no core.clip"));
                            }
                        }
                        Motion::Blend1D { parameter, children } => {
                            self.numeric_parameter(*parameter)?;
                            if children.is_empty() {
                                return Err("1D blend has no children".into());
                            }
                            if !children.iter().all(|(threshold, _)| threshold.is_finite())
                                || !children.windows(2).all(|pair| pair[0].0 < pair[1].0)
                            {
                                return Err(
                                    "1D blend thresholds must be finite and strictly increasing".into()
                                );
                            }
                            pending.extend(children.iter().map(|(_, motion)| motion));
                        }
                        Motion::Pending(_) => {}
                        Motion::Unsupported(why) => {
                            if why.trim().is_empty() {
                                return Err("unsupported motion has no reason".into());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Component for ControllerBinding {
    const NAME: &'static str = "core.controller_binding";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend([self.controller, self.rig]);
    }
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        if !w.has::<Controller>(self.controller) {
            return Err(format!("{:?} holds no core.controller", self.controller));
        }
        if !w.has::<RigInstance>(self.rig) {
            return Err(format!("{:?} holds no core.rig_instance", self.rig));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Extrapolation, Transform};
    fn fixture() -> (World, Entity, Controller) {
        let mut w = World::new();
        let clip = w.spawn();
        w.insert(
            clip,
            Clip {
                name: "motion".into(),
                duration: 1.,
                extrapolation: Extrapolation::Hold,
                channels: vec![],
                root_motion: None,
                additive: None,
            },
        );
        let controller = w.spawn();
        let value = Controller {
            name: "character".into(),
            parameters: vec![
                ControllerParameter {
                    name: "speed".into(),
                    kind: ControllerParameterKind::Float,
                    default: 0.,
                },
                ControllerParameter {
                    name: "index".into(),
                    kind: ControllerParameterKind::Int,
                    default: 1.,
                },
                ControllerParameter {
                    name: "flag".into(),
                    kind: ControllerParameterKind::Bool,
                    default: 0.,
                },
                ControllerParameter {
                    name: "fire".into(),
                    kind: ControllerParameterKind::Trigger,
                    default: 1.,
                },
            ],
            layers: vec![ControllerLayer {
                name: "base".into(),
                default_state: Some(0),
                default_weight: 0.75,
                weight_parameter: None,
                blending: LayerBlending::Override,
                mask: None,
                states: vec![ControllerState {
                    name: "idle".into(),
                    speed: -1.,
                    speed_parameter: Some(0),
                    cycle_offset: 1.5,
                    cycle_offset_parameter: Some(1),
                    motion: Motion::Blend1D {
                        parameter: 0,
                        children: vec![
                            (-1., Motion::Clip(clip)),
                            (
                                1.,
                                Motion::Unsupported("2D tree remains in source dialect".into()),
                            ),
                        ],
                    },
                }],
            }],
        };
        (w, controller, value)
    }
    #[test]
    fn validates_parameter_defaults_without_rounding_or_clamping() {
        let (w, e, c) = fixture();
        assert!(c.verify(e, &w).is_ok());
        for (index, value) in [(0, f64::NAN), (0, f64::INFINITY), (1, 1.5), (2, 2.), (3, -1.)] {
            let mut bad = c.clone();
            bad.parameters[index].default = value;
            assert!(bad.verify(e, &w).is_err());
        }
        let mut bad = c.clone();
        bad.parameters[1].name = "speed".into();
        assert!(bad.verify(e, &w).is_err());
        let mut bad = c.clone();
        bad.parameters[1].name.clear();
        assert!(bad.verify(e, &w).is_err());
    }
    #[test]
    fn numeric_refs_blends_and_default_states_are_checked() {
        let (w, e, c) = fixture();
        for index in [2, 3, 4] {
            let mut bad = c.clone();
            bad.layers[0].states[0].speed_parameter = Some(index);
            assert!(bad.verify(e, &w).is_err());
            let mut bad = c.clone();
            bad.layers[0].states[0].cycle_offset_parameter = Some(index);
            assert!(bad.verify(e, &w).is_err());
            let mut bad = c.clone();
            bad.layers[0].states[0].motion = Motion::Blend1D {
                parameter: index,
                children: vec![(0., Motion::Unsupported("not lowered".into()))],
            };
            assert!(bad.verify(e, &w).is_err());
        }
        for thresholds in [vec![], vec![1., 1.], vec![1., 0.], vec![f32::NAN]] {
            let mut bad = c.clone();
            bad.layers[0].states[0].motion = Motion::Blend1D {
                parameter: 0,
                children: thresholds
                    .into_iter()
                    .map(|t| (t, Motion::Unsupported("not lowered".into())))
                    .collect(),
            };
            assert!(bad.verify(e, &w).is_err());
        }
        let mut bad = c.clone();
        bad.layers[0].default_state = Some(1);
        assert!(bad.verify(e, &w).is_err());
        // states without a default state are refused
        let mut bad = c.clone();
        bad.layers[0].default_state = None;
        assert!(
            bad.verify(e, &w)
                .unwrap_err()
                .contains("has states but no default state")
        );
        let mut bad = c.clone();
        bad.layers[0].default_weight = f32::NAN;
        assert!(bad.verify(e, &w).is_err());
        let mut bad = c.clone();
        bad.layers[0].states[0].speed = f64::INFINITY;
        assert!(bad.verify(e, &w).is_err());
        let mut bad = c;
        bad.layers[0].states[0].cycle_offset = f32::INFINITY;
        assert!(bad.verify(e, &w).is_err());
    }
    #[test]
    fn recursive_motion_references_require_clips_and_keep_named_refusals() {
        let (mut w, e, mut c) = fixture();
        let absent = w.spawn();
        c.layers[0].states[0].motion = Motion::Blend1D {
            parameter: 0,
            children: vec![(
                0.,
                Motion::Blend1D {
                    parameter: 1,
                    children: vec![(0., Motion::Clip(absent))],
                },
            )],
        };
        let mut refs = Vec::new();
        c.references(&mut refs);
        assert_eq!(refs, vec![absent]);
        assert!(c.verify(e, &w).is_err());
        c.layers[0].states[0].motion = Motion::Unsupported(String::new());
        assert!(c.verify(e, &w).is_err());
        c.layers[0].states[0].motion = Motion::Unsupported("Direct blend is not lowered".into());
        assert!(c.verify(e, &w).is_ok());
    }
    /// A layer with no states plays nothing and has no default state; both halves of that are required, and it survives
    /// JSON as a null.
    #[test]
    fn an_empty_layer_has_no_default_state() {
        let (w, e, mut c) = fixture();
        let mut empty = c.layers[0].clone();
        empty.name = "empty".into();
        empty.states.clear();
        empty.default_state = None;
        c.layers.push(empty);
        assert!(c.verify(e, &w).is_ok());
        let json = c.to_json().unwrap().unwrap();
        assert_eq!(json["layers"][0]["default_state"], 0);
        assert!(json["layers"][1]["default_state"].is_null());
        assert_eq!(serde_json::from_value::<Controller>(json).unwrap(), c);
        // a default state with no states to be in is out of range
        c.layers[1].default_state = Some(0);
        assert!(
            c.verify(e, &w)
                .unwrap_err()
                .contains("default state is out of range")
        );
    }
    fn skeleton_of(w: &mut World, joints: u32) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Skeleton {
                joints: (0..joints)
                    .map(|i| crate::Joint {
                        id: JointId(i),
                        name: format!("j{i}"),
                        parent: i.checked_sub(1).map(JointId),
                        rest: crate::Rest::Trs {
                            translation: [0.; 3],
                            rotation: [0., 0., 0., 1.],
                            scale: [1.; 3],
                        },
                    })
                    .collect(),
            },
        );
        e
    }
    fn masked(skeleton: Entity, joint: u32, weight: f32) -> MaskedJoint {
        MaskedJoint {
            skeleton,
            joint: JointId(joint),
            weight,
        }
    }
    /// A mask names joints as a clip does, by skeleton and joint id; it is checked against those skeletons, and an
    /// invalid weight, a repeated joint, a skeleton that is none, a joint that is not in it, a reasonless refusal or a
    /// weight parameter that is not numeric is refused.
    #[test]
    fn a_layer_mask_is_checked_against_the_skeletons_it_names() {
        let (mut w, e, mut c) = fixture();
        let a = skeleton_of(&mut w, 3);
        let b = skeleton_of(&mut w, 2);
        c.layers[0].mask = Some(LayerMask::Joints(vec![
            masked(a, 0, 1.),
            masked(a, 2, 0.25),
            masked(b, 0, 0.),
        ]));
        c.layers[0].weight_parameter = Some(0);
        assert!(c.verify(e, &w).is_ok());
        let mut refs = Vec::new();
        c.references(&mut refs);
        assert!(refs.contains(&a) && refs.contains(&b));
        for (mask, why) in [
            (vec![masked(a, 0, 1.), masked(a, 0, 0.5)], "masked twice"),
            (vec![masked(a, 0, 1.0001)], "not in 0 to 1"),
            (vec![masked(a, 0, -0.5)], "not in 0 to 1"),
            (vec![masked(a, 0, f32::NAN)], "not in 0 to 1"),
            (vec![masked(a, 3, 1.)], "is not in skeleton"),
            (vec![masked(b, 2, 1.)], "is not in skeleton"),
            (vec![masked(e, 0, 1.)], "holds no core.skeleton"),
        ] {
            let mut bad = c.clone();
            bad.layers[0].mask = Some(LayerMask::Joints(mask));
            let err = bad.verify(e, &w).unwrap_err();
            assert!(err.contains(why), "{err}");
        }
        let mut bad = c.clone();
        bad.layers[0].mask = Some(LayerMask::Unsupported(" ".into()));
        assert!(bad.verify(e, &w).unwrap_err().contains("no reason"));
        bad.layers[0].mask = Some(LayerMask::Unsupported("a humanoid body mask".into()));
        assert!(bad.verify(e, &w).is_ok());
        for index in [2, 3, 4] {
            let mut bad = c.clone();
            bad.layers[0].weight_parameter = Some(index);
            assert!(bad.verify(e, &w).unwrap_err().contains("weight"));
        }
    }
    /// The mask and the weight parameter survive JSON, `null` when none, and a stored controller without either field
    /// is refused rather than read as an unmasked layer.
    #[test]
    fn a_layer_mask_round_trips_and_its_absence_is_not_unmasked() {
        let (mut w, _, mut c) = fixture();
        let a = skeleton_of(&mut w, 2);
        c.layers[0].weight_parameter = Some(0);
        c.layers[0].mask = Some(LayerMask::Joints(vec![masked(a, 1, 0.5)]));
        let json = c.to_json().unwrap().unwrap();
        assert_eq!(serde_json::from_value::<Controller>(json.clone()).unwrap(), c);
        c.layers[0].mask = Some(LayerMask::Unsupported("why".into()));
        let json = c.to_json().unwrap().unwrap();
        assert_eq!(
            json["layers"][0]["mask"],
            serde_json::json!({"Unsupported": "why"})
        );
        assert_eq!(serde_json::from_value::<Controller>(json).unwrap(), c);
        c.layers[0].mask = None;
        c.layers[0].weight_parameter = None;
        let json = c.to_json().unwrap().unwrap();
        assert!(json["layers"][0]["mask"].is_null() && json["layers"][0]["weight_parameter"].is_null());
        assert_eq!(serde_json::from_value::<Controller>(json.clone()).unwrap(), c);
        for field in ["mask", "weight_parameter"] {
            let mut stripped = json.clone();
            stripped["layers"][0].as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Controller>(stripped).is_err(), "{field}");
        }
    }
    #[test]
    fn controller_and_binding_json_round_trip_keep_entity_ids_and_layer_modes() {
        let (mut w, e, mut c) = fixture();
        for blending in [
            LayerBlending::Override,
            LayerBlending::Additive,
            LayerBlending::Other(17),
        ] {
            c.layers[0].blending = blending;
            assert_eq!(
                serde_json::from_value::<Controller>(c.to_json().unwrap().unwrap()).unwrap(),
                c
            );
        }
        w.insert(e, c.clone());
        let rig = w.spawn();
        let binding = ControllerBinding { controller: e, rig };
        assert!(binding.verify(e, &w).is_err());
        let skeleton = w.spawn();
        w.insert(rig, RigInstance { skeleton });
        w.insert(rig, Transform::IDENTITY);
        assert!(binding.verify(e, &w).is_ok());
        let mut refs = Vec::new();
        binding.references(&mut refs);
        assert_eq!(refs, vec![e, rig]);
        assert_eq!(
            serde_json::from_value::<ControllerBinding>(binding.to_json().unwrap().unwrap()).unwrap(),
            binding
        );
        assert!(c.remap_primitives(&[0]).is_none());
        assert!(binding.remap_primitives(&[0]).is_none());
        let mut json = c.to_json().unwrap().unwrap();
        json["forgotten_source_field"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Controller>(json).is_err());
        let mut invalid = binding;
        invalid.controller = skeleton;
        assert!(invalid.verify(e, &w).is_err());
    }
}
