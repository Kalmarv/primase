//! A clip-selection state machine and its rig binding. Evaluation belongs to the animation toolkit.
//! Source-specific transitions and body/skeleton layer masks remain in the source dialect; this
//! projection does not claim to apply them or silently replace them with unmasked transitions.
use crate::{Clip, Component, Entity, Geometry, JsonError, RigInstance, World};
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerLayer {
    pub name: String,
    pub states: Vec<ControllerState>,
    pub default_state: usize,
    /// As stored; finite, never silently clamped to a consumer's supported range.
    pub default_weight: f32,
    pub blending: LayerBlending,
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
                Motion::Clip(clip) => out.push(*clip),
                Motion::Blend1D { children, .. } => pending.extend(children.iter().map(|(_, motion)| motion)),
                Motion::Unsupported(_) => {}
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
            if layer.default_state >= layer.states.len() {
                return Err(format!("layer {layer_index}: default state is out of range"));
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
                default_state: 0,
                default_weight: 0.75,
                blending: LayerBlending::Override,
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
        bad.layers[0].default_state = 1;
        assert!(bad.verify(e, &w).is_err());
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
