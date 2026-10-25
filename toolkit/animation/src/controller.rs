//! One chosen state's clip and time mapping. No transitions, layer composition, masks
//! or blend-tree duration rule are invented here. The core controller retains those facts.

use primase_core::{Controller, ControllerParameterKind, Entity, Motion};
use crate::{Error, Result};

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

/// Resolve one state of one layer. Values are positional in controller.parameters, finite
/// and of their declared kind (Bool/Trigger 0 or 1, Int integral). No rounding/clamping.
/// Blend1D and Unsupported motions refuse by name: choosing one child or inventing
/// a weighted clip duration would change the source's animation semantics.
pub fn motion(controller: &Controller, layer: usize, state: usize, values: &[f64]) -> Result<StateMotion> {
    if values.len() != controller.parameters.len() {
        return Err(Error(format!("controller {:?}: {} values for {} parameters",
            controller.name, values.len(),controller.parameters.len())));
    }
    for (parameter,&value) in controller.parameters.iter().zip(values) {
        if !value.is_finite() {
            return Err(Error(format!("parameter {:?}: non-finite value {value}",parameter.name)));
        }
        match parameter.kind {
            ControllerParameterKind::Int if value.fract() != 0.0 =>
                return Err(Error(format!("parameter {:?}: fractional Int value {value}",parameter.name))),
            ControllerParameterKind::Bool | ControllerParameterKind::Trigger if value != 0.0 && value != 1.0 =>
                return Err(Error(format!("parameter {:?}: Bool/Trigger value {value} is not 0 or 1",parameter.name))),
            ControllerParameterKind::Float | ControllerParameterKind::Int
                | ControllerParameterKind::Bool | ControllerParameterKind::Trigger => {}
        }
    }
    let layer = controller.layers.get(layer)
        .ok_or_else(||Error(format!("layer {layer} outside controller {:?}",controller.name)))?;
    let state = layer.states.get(state)
        .ok_or_else(||Error(format!("state {state} outside layer {:?}",layer.name)))?;
    let numeric = |index:usize|->Result<f64> {
        let parameter=controller.parameters.get(index).ok_or_else(||Error(format!("state {:?}: parameter index {index} is absent",state.name)))?;
        if !matches!(parameter.kind,ControllerParameterKind::Float | ControllerParameterKind::Int) {
            return Err(Error(format!("state {:?}: parameter {:?} is not numeric",state.name,parameter.name)));
        }
        Ok(values[index])
    };
    let clip=match &state.motion {
        Motion::Clip(entity)=>*entity,
        Motion::Blend1D { .. }=>return Err(Error(format!("state {:?}: Blend1D clip-time rule is not established",state.name))),
        Motion::Unsupported(reason)=>return Err(Error(format!("state {:?}: unsupported motion: {reason}",state.name))),
    };
    let speed=state.speed*match state.speed_parameter {Some(index)=>numeric(index)?,None=>1.0};
    let cycle_offset=match state.cycle_offset_parameter {
        Some(index)=>numeric(index)? as f32,
        None=>state.cycle_offset,
    };
    if !state.speed.is_finite() || !speed.is_finite() || !cycle_offset.is_finite() {
        return Err(Error(format!("state {:?}: speed or cycle offset is not finite",state.name)));
    }
    Ok(StateMotion{clip,speed,cycle_offset})
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{ControllerLayer,ControllerState,ControllerParameter,LayerBlending};

    fn controller()->Controller {
        Controller { name:"controller".into(),parameters:vec![
            ControllerParameter{name:"speed".into(),kind:ControllerParameterKind::Float,default:1.0},
            ControllerParameter{name:"offset".into(),kind:ControllerParameterKind::Int,default:0.0},
            ControllerParameter{name:"enabled".into(),kind:ControllerParameterKind::Bool,default:1.0},
        ],layers:vec![ControllerLayer{name:"layer".into(),default_state:0,default_weight:0.25,
            blending:LayerBlending::Other(17),states:vec![ControllerState{name:"walk".into(),
                motion:Motion::Clip(Entity::from_index(8)),speed:2.0,speed_parameter:Some(0),
                cycle_offset:0.25,cycle_offset_parameter:Some(1)}]}] }
    }

    #[test]
    fn state_time_uses_numeric_speed_and_offset_without_source_identity() {
        let c=controller();
        let sampled=motion(&c,0,0,&[-0.5,2.0,1.0]).unwrap();
        assert_eq!(sampled,StateMotion{clip:Entity::from_index(8),speed:-1.0,cycle_offset:2.0});
        assert_eq!(sampled.clip_time(3.0,4.0),5.0);
        let mut c=c;c.layers[0].states[0].speed_parameter=None;
        c.layers[0].states[0].cycle_offset_parameter=None;
        let sampled=motion(&c,0,0,&[0.0,0.0,0.0]).unwrap();
        assert_eq!(sampled.clip_time(3.0,4.0),7.0);
    }

    #[test]
    fn unsupported_motions_are_retained_but_never_replaced_by_a_child() {
        let mut c=controller();
        c.layers[0].states[0].motion=Motion::Blend1D {parameter:0,
            children:vec![(0.0,Motion::Clip(Entity::from_index(8)))]};
        assert!(motion(&c,0,0,&[0.0,0.0,1.0]).unwrap_err().0.contains("Blend1D"));
        c.layers[0].states[0].motion=Motion::Unsupported("2D source tree".into());
        assert!(motion(&c,0,0,&[0.0,0.0,1.0]).unwrap_err().0.contains("2D source tree"));
    }

    #[test]
    fn invalid_indices_parameter_types_and_nonfinite_math_refuse() {
        let c=controller();
        assert!(motion(&c,1,0,&[1.0,0.0,1.0]).is_err());
        assert!(motion(&c,0,1,&[1.0,0.0,1.0]).is_err());
        for values in [vec![],vec![f64::NAN,0.0,1.0],vec![1.0,0.5,1.0],vec![1.0,0.0,2.0]] {
            assert!(motion(&c,0,0,&values).is_err());
        }
        let mut c=c;c.layers[0].states[0].speed_parameter=Some(2);
        assert!(motion(&c,0,0,&[1.0,0.0,1.0]).unwrap_err().0.contains("not numeric"));
        c.layers[0].states[0].speed_parameter=Some(0);c.layers[0].states[0].speed=f64::MAX;
        assert!(motion(&c,0,0,&[2.0,0.0,1.0]).is_err());
    }
}
